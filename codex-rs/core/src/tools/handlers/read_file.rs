use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use codex_utils_string::take_bytes_at_char_boundary;
use serde::Deserialize;
use serde::Serialize;
use tokio::process::Command;
use tokio::time::timeout;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;

pub struct ReadFileHandler;

const MAX_LINE_LENGTH: usize = 500;
const TAB_WIDTH: usize = 4;
const DEFAULT_GREP_LIMIT: usize = 100;
const MAX_GREP_LIMIT: usize = 2000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

// TODO(jif) add support for block comments
const COMMENT_PREFIXES: &[&str] = &["#", "//", "--"];

/// JSON arguments accepted by the `read_file` tool handler.
#[derive(Deserialize)]
struct ReadFileArgs {
    /// Legacy single-read path. Kept for backwards compatibility.
    #[serde(default)]
    file_path: Option<String>,
    /// 1-indexed line number to start reading from; defaults to 1.
    #[serde(default = "defaults::offset")]
    offset: usize,
    /// Maximum number of lines to return; defaults to 2000.
    #[serde(default = "defaults::limit")]
    limit: usize,
    /// Determines whether the handler reads a simple slice or indentation-aware block.
    #[serde(default)]
    mode: ReadMode,
    /// Optional indentation configuration used when `mode` is `Indentation`.
    #[serde(default)]
    indentation: Option<IndentationArgs>,
    /// Batched explicit reads.
    #[serde(default)]
    reads: Option<Vec<ReadRequest>>,
    /// Native ripgrep stage for file or match discovery.
    #[serde(default)]
    grep: Option<GrepRequest>,
    /// Ordered pipeline stages that can chain grep and read work together.
    #[serde(default)]
    pipeline: Option<Vec<PipelineStep>>,
    /// High-level investigation presets that can emit persistent maps.
    #[serde(default)]
    map: Option<MapRequest>,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ReadMode {
    #[default]
    Slice,
    Indentation,
}

#[derive(Deserialize, Clone)]
struct ReadRequest {
    file_path: String,
    #[serde(default = "defaults::offset")]
    offset: usize,
    #[serde(default = "defaults::limit")]
    limit: usize,
    #[serde(default)]
    mode: ReadMode,
    #[serde(default)]
    indentation: Option<IndentationArgs>,
}

#[derive(Deserialize, Clone)]
struct GrepRequest {
    pattern: String,
    #[serde(default)]
    include: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "defaults::grep_limit")]
    limit: usize,
    #[serde(default)]
    output: GrepOutput,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum GrepOutput {
    #[default]
    Files,
    Matches,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "op", rename_all = "snake_case")]
enum PipelineStep {
    Grep(GrepRequest),
    Read(PipelineReadStep),
}

#[derive(Deserialize, Clone)]
struct PipelineReadStep {
    #[serde(default)]
    reads: Option<Vec<ReadRequest>>,
    #[serde(default)]
    from: Option<PipelineInput>,
    #[serde(default)]
    mode: Option<ReadMode>,
    #[serde(default)]
    indentation: Option<IndentationArgs>,
    #[serde(default)]
    match_window_before: Option<usize>,
    #[serde(default)]
    match_window_after: Option<usize>,
    #[serde(default = "defaults::limit")]
    limit: usize,
}

#[derive(Deserialize, Clone)]
struct MapRequest {
    preset: MapPreset,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "defaults::map_limit")]
    limit: usize,
    #[serde(default = "defaults::write_artifacts")]
    write_artifacts: bool,
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum MapPreset {
    #[default]
    ProtocolMap,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum PipelineInput {
    PreviousFiles,
    PreviousMatches,
}

/// Additional configuration for indentation-aware reads.
#[derive(Deserialize, Clone)]
struct IndentationArgs {
    /// Optional explicit anchor line; defaults to `offset` when omitted.
    #[serde(default)]
    anchor_line: Option<usize>,
    /// Maximum indentation depth to collect; `0` means unlimited.
    #[serde(default = "defaults::max_levels")]
    max_levels: usize,
    /// Whether to include sibling blocks at the same indentation level.
    #[serde(default = "defaults::include_siblings")]
    include_siblings: bool,
    /// Whether to include header lines above the anchor block. This made on a best effort basis.
    #[serde(default = "defaults::include_header")]
    include_header: bool,
    /// Optional hard cap on returned lines; defaults to the global `limit`.
    #[serde(default)]
    max_lines: Option<usize>,
}

#[derive(Clone, Debug)]
struct LineRecord {
    number: usize,
    raw: String,
    display: String,
    indent: usize,
}

#[derive(Clone, Debug)]
struct GrepMatch {
    path: PathBuf,
    line: usize,
    text: String,
}

#[derive(Clone, Debug)]
struct ReadArtifact {
    path: PathBuf,
    label: Option<String>,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct ProtocolMapArtifact {
    preset: &'static str,
    root: String,
    generated_at: String,
    hot_files: Vec<HotFileRecord>,
    message_families: Vec<MessageFamilyRecord>,
    version_clues: Vec<VersionClueRecord>,
    evidence: Vec<String>,
}

#[derive(Serialize, Clone)]
struct HotFileRecord {
    path: String,
    score: usize,
    reasons: Vec<String>,
}

#[derive(Serialize)]
struct MessageFamilyRecord {
    name: String,
    count: usize,
    examples: Vec<String>,
}

#[derive(Serialize, Clone)]
struct VersionClueRecord {
    path: String,
    line: usize,
    text: String,
}

#[derive(Default)]
struct HotFileAccumulator {
    score: usize,
    reasons: Vec<String>,
}

enum PipelineValue {
    Files(Vec<PathBuf>),
    Matches(Vec<GrepMatch>),
    Reads(Vec<ReadArtifact>),
}

impl LineRecord {
    fn trimmed(&self) -> &str {
        self.raw.trim_start()
    }

    fn is_blank(&self) -> bool {
        self.trimmed().is_empty()
    }

    fn is_comment(&self) -> bool {
        COMMENT_PREFIXES
            .iter()
            .any(|prefix| self.raw.trim().starts_with(prefix))
    }
}

#[async_trait]
impl ToolHandler for ReadFileHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation { payload, turn, .. } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "read_file handler received unsupported payload".to_string(),
                ));
            }
        };

        let args: ReadFileArgs = parse_arguments(&arguments)?;

        let ReadFileArgs {
            file_path,
            offset,
            limit,
            mode,
            indentation,
            reads,
            grep,
            pipeline,
            map,
        } = args;

        let body = if let Some(map) = map {
            execute_map_request(&turn, &map).await?
        } else if let Some(pipeline) = pipeline {
            render_pipeline_value(execute_pipeline(&turn, pipeline).await?)
        } else if let Some(grep) = grep {
            render_pipeline_value(execute_grep_step(&turn, &grep).await?)
        } else if let Some(reads) = reads {
            let artifacts = execute_explicit_reads(&turn, reads).await?;
            render_read_artifacts(&artifacts)
        } else if let Some(file_path) = file_path {
            validate_read_bounds(offset, limit)?;
            let path = turn.resolve_path(Some(file_path));
            let collected = execute_single_read(
                &path,
                ReadRequest {
                    file_path: path.display().to_string(),
                    offset,
                    limit,
                    mode,
                    indentation,
                },
            )
            .await?;
            collected.lines.join("\n")
        } else {
            return Err(FunctionCallError::RespondToModel(
                "read_file requires one of: file_path, reads, grep, pipeline, or map".to_string(),
            ));
        };

        Ok(FunctionToolOutput::from_text(body, Some(true)))
    }
}

fn validate_read_bounds(offset: usize, limit: usize) -> Result<(), FunctionCallError> {
    if offset == 0 {
        return Err(FunctionCallError::RespondToModel(
            "offset must be a 1-indexed line number".to_string(),
        ));
    }
    if limit == 0 {
        return Err(FunctionCallError::RespondToModel(
            "limit must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

async fn execute_pipeline(
    turn: &crate::codex::TurnContext,
    pipeline: Vec<PipelineStep>,
) -> Result<PipelineValue, FunctionCallError> {
    let mut current: Option<PipelineValue> = None;
    for step in pipeline {
        current = Some(match step {
            PipelineStep::Grep(grep) => execute_grep_step(turn, &grep).await?,
            PipelineStep::Read(read) => execute_pipeline_read_step(turn, current, &read).await?,
        });
    }

    current.ok_or_else(|| {
        FunctionCallError::RespondToModel("pipeline must contain at least one step".to_string())
    })
}

async fn execute_explicit_reads(
    turn: &crate::codex::TurnContext,
    reads: Vec<ReadRequest>,
) -> Result<Vec<ReadArtifact>, FunctionCallError> {
    let mut artifacts = Vec::with_capacity(reads.len());
    for read in reads {
        validate_read_bounds(read.offset, read.limit)?;
        let path = turn.resolve_path(Some(read.file_path.clone()));
        artifacts.push(execute_single_read(&path, read).await?);
    }
    Ok(artifacts)
}

async fn execute_pipeline_read_step(
    turn: &crate::codex::TurnContext,
    current: Option<PipelineValue>,
    step: &PipelineReadStep,
) -> Result<PipelineValue, FunctionCallError> {
    if let Some(reads) = step.reads.clone() {
        return Ok(PipelineValue::Reads(
            execute_explicit_reads(turn, reads).await?,
        ));
    }

    match (step.from, current) {
        (Some(PipelineInput::PreviousFiles), Some(PipelineValue::Files(paths))) => {
            let mut artifacts = Vec::with_capacity(paths.len());
            for path in paths {
                let request = ReadRequest {
                    file_path: path.display().to_string(),
                    offset: defaults::offset(),
                    limit: step.limit,
                    mode: step.mode.unwrap_or_default(),
                    indentation: step.indentation.clone(),
                };
                artifacts.push(execute_single_read(&path, request).await?);
            }
            Ok(PipelineValue::Reads(artifacts))
        }
        (Some(PipelineInput::PreviousMatches), Some(PipelineValue::Matches(matches))) => {
            let mut artifacts = Vec::with_capacity(matches.len());
            for found in matches {
                let mode = step.mode.unwrap_or(ReadMode::Indentation);
                let request = request_from_match(&found, step, mode);
                let mut artifact = execute_single_read(&found.path, request).await?;
                artifact.label = Some(format!("match L{}", found.line));
                artifacts.push(artifact);
            }
            Ok(PipelineValue::Reads(artifacts))
        }
        (Some(PipelineInput::PreviousFiles), Some(PipelineValue::Reads(_)))
        | (Some(PipelineInput::PreviousMatches), Some(PipelineValue::Reads(_)))
        | (Some(PipelineInput::PreviousFiles), Some(PipelineValue::Matches(_)))
        | (Some(PipelineInput::PreviousMatches), Some(PipelineValue::Files(_)))
        | (None, _) => Err(FunctionCallError::RespondToModel(
            "read pipeline step needs either explicit reads or a compatible `from` source"
                .to_string(),
        )),
        (_, None) => Err(FunctionCallError::RespondToModel(
            "read pipeline step cannot run before any pipeline output exists".to_string(),
        )),
    }
}

fn request_from_match(found: &GrepMatch, step: &PipelineReadStep, mode: ReadMode) -> ReadRequest {
    match mode {
        ReadMode::Slice => {
            let before = step.match_window_before.unwrap_or(6);
            let after = step.match_window_after.unwrap_or(12);
            let offset = found.line.saturating_sub(before).max(1);
            let limit = before + after + 1;
            ReadRequest {
                file_path: found.path.display().to_string(),
                offset,
                limit,
                mode,
                indentation: None,
            }
        }
        ReadMode::Indentation => {
            let mut indentation = step.indentation.clone().unwrap_or_default();
            indentation.anchor_line = Some(found.line);
            ReadRequest {
                file_path: found.path.display().to_string(),
                offset: found.line,
                limit: step.limit,
                mode,
                indentation: Some(indentation),
            }
        }
    }
}

async fn execute_single_read(
    path: &Path,
    request: ReadRequest,
) -> Result<ReadArtifact, FunctionCallError> {
    validate_read_bounds(request.offset, request.limit)?;
    let lines = match request.mode {
        ReadMode::Slice => slice::read(path, request.offset, request.limit).await?,
        ReadMode::Indentation => {
            let indentation = request.indentation.unwrap_or_default();
            indentation::read_block(path, request.offset, request.limit, indentation).await?
        }
    };
    Ok(ReadArtifact {
        path: path.to_path_buf(),
        label: None,
        lines,
    })
}

async fn execute_grep_step(
    turn: &crate::codex::TurnContext,
    grep: &GrepRequest,
) -> Result<PipelineValue, FunctionCallError> {
    let pattern = grep.pattern.trim();
    if pattern.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "grep pattern must not be empty".to_string(),
        ));
    }

    let limit = grep.limit.min(MAX_GREP_LIMIT);
    if limit == 0 {
        return Err(FunctionCallError::RespondToModel(
            "grep limit must be greater than zero".to_string(),
        ));
    }

    let search_path = turn.resolve_path(grep.path.clone());
    verify_path_exists(&search_path).await?;

    let include = grep.include.as_deref().map(str::trim).and_then(|val| {
        if val.is_empty() {
            None
        } else {
            Some(val.to_string())
        }
    });

    match grep.output {
        GrepOutput::Files => {
            let files =
                run_rg_file_search(pattern, include.as_deref(), &search_path, limit, &turn.cwd)
                    .await?;
            Ok(PipelineValue::Files(files))
        }
        GrepOutput::Matches => {
            let matches =
                run_rg_match_search(pattern, include.as_deref(), &search_path, limit, &turn.cwd)
                    .await?;
            Ok(PipelineValue::Matches(matches))
        }
    }
}

async fn verify_path_exists(path: &Path) -> Result<(), FunctionCallError> {
    tokio::fs::metadata(path).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("unable to access `{}`: {err}", path.display()))
    })?;
    Ok(())
}

async fn execute_map_request(
    turn: &crate::codex::TurnContext,
    map: &MapRequest,
) -> Result<String, FunctionCallError> {
    let search_path = turn.resolve_path(map.path.clone());
    verify_path_exists(&search_path).await?;
    let root = map_root(&search_path);

    match map.preset {
        MapPreset::ProtocolMap => {
            let hot_files = build_protocol_hot_files(root, map.limit, &turn.cwd).await?;
            let message_families = discover_message_families(root, map.limit, &turn.cwd).await?;
            let version_clues = discover_version_clues(root, map.limit, &turn.cwd).await?;

            let artifact = ProtocolMapArtifact {
                preset: "protocol_map",
                root: root.display().to_string(),
                generated_at: chrono::Utc::now().to_rfc3339(),
                hot_files: hot_files.clone(),
                message_families,
                version_clues: version_clues.clone(),
                evidence: hot_files
                    .iter()
                    .take(8)
                    .flat_map(|record| {
                        record
                            .reasons
                            .iter()
                            .map(move |reason| format!("{} :: {}", record.path, reason))
                    })
                    .collect(),
            };

            let mut artifact_paths = Vec::new();
            if map.write_artifacts {
                artifact_paths = write_protocol_map_artifacts(root, &artifact).await?;
            }

            Ok(render_protocol_map_summary(&artifact, &artifact_paths))
        }
    }
}

fn map_root(path: &Path) -> &Path {
    if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

async fn write_protocol_map_artifacts(
    root: &Path,
    artifact: &ProtocolMapArtifact,
) -> Result<Vec<PathBuf>, FunctionCallError> {
    let maps_dir = root.join(".palace").join("maps");
    tokio::fs::create_dir_all(&maps_dir).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to create `{}`: {err}",
            maps_dir.display()
        ))
    })?;

    let protocol_path = maps_dir.join("protocol-graph.json");
    let hot_files_path = maps_dir.join("hot-files.json");
    let version_path = maps_dir.join("version-map.json");
    let repo_spine_path = maps_dir.join("repo-spine.json");

    let protocol_json = serde_json::to_string_pretty(artifact).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize protocol map: {err}"))
    })?;
    let hot_files_json = serde_json::to_string_pretty(&artifact.hot_files).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize hot files map: {err}"))
    })?;
    let version_json = serde_json::to_string_pretty(&artifact.version_clues).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to serialize version map: {err}"))
    })?;
    let repo_spine_json =
        serde_json::to_string_pretty(&artifact.hot_files.iter().take(10).collect::<Vec<_>>())
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to serialize repo spine: {err}"))
            })?;

    for (path, content) in [
        (&protocol_path, protocol_json),
        (&hot_files_path, hot_files_json),
        (&version_path, version_json),
        (&repo_spine_path, repo_spine_json),
    ] {
        tokio::fs::write(path, content).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to write `{}`: {err}",
                path.display()
            ))
        })?;
    }

    Ok(vec![
        protocol_path,
        hot_files_path,
        version_path,
        repo_spine_path,
    ])
}

fn render_protocol_map_summary(
    artifact: &ProtocolMapArtifact,
    artifact_paths: &[PathBuf],
) -> String {
    let mut sections = vec![
        format!("protocol_map root: {}", artifact.root),
        format!("generated_at: {}", artifact.generated_at),
    ];

    if !artifact_paths.is_empty() {
        sections.push("artifacts:".to_string());
        for path in artifact_paths {
            sections.push(format!("- {}", path.display()));
        }
    }

    sections.push("hot_files:".to_string());
    if artifact.hot_files.is_empty() {
        sections.push("- none".to_string());
    } else {
        for file in artifact.hot_files.iter().take(12) {
            sections.push(format!(
                "- {} [score={}] {}",
                file.path,
                file.score,
                file.reasons.join("; ")
            ));
        }
    }

    sections.push("message_families:".to_string());
    if artifact.message_families.is_empty() {
        sections.push("- none".to_string());
    } else {
        for family in artifact.message_families.iter().take(12) {
            sections.push(format!(
                "- {} ({}) {}",
                family.name,
                family.count,
                family.examples.join(", ")
            ));
        }
    }

    sections.push("version_clues:".to_string());
    if artifact.version_clues.is_empty() {
        sections.push("- none".to_string());
    } else {
        for clue in artifact.version_clues.iter().take(12) {
            sections.push(format!("- {}:L{}: {}", clue.path, clue.line, clue.text));
        }
    }

    sections.join("\n")
}

async fn build_protocol_hot_files(
    root: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<HotFileRecord>, FunctionCallError> {
    let mut scored: BTreeMap<String, HotFileAccumulator> = BTreeMap::new();

    for path in run_rg_files_listing(root, cwd).await? {
        let normalized = path.display().to_string();
        let lower = normalized.to_lowercase();
        for (needle, weight, reason) in [
            ("protocol", 8, "filename suggests protocol spine"),
            ("communication", 8, "filename suggests communication schema"),
            ("message", 7, "filename suggests message definitions"),
            ("packet", 7, "filename suggests packet definitions"),
            ("rpc", 6, "filename suggests RPC surface"),
            ("api", 5, "filename suggests API boundary"),
            ("schema", 5, "filename suggests schema boundary"),
            ("opcode", 6, "filename suggests opcode table"),
            ("wire", 5, "filename suggests wire format"),
        ] {
            if lower.contains(needle) {
                record_hot_file_reason(&mut scored, &normalized, weight, reason);
            }
        }
    }

    for (pattern, weight, reason) in [
        (
            "[A-Z]{2,}TO[A-Z]{2,}_[A-Z0-9_]+",
            10,
            "contains protocol/message family identifiers",
        ),
        (
            "(protocol|packet|message|request|response|opcode)",
            4,
            "contains protocol terminology",
        ),
        (
            "(protocolVersion|version|fver|sequence_id)",
            3,
            "contains versioning clues",
        ),
    ] {
        for found in run_rg_match_search(pattern, None, root, limit * 8, cwd).await? {
            record_hot_file_reason(
                &mut scored,
                &found.path.display().to_string(),
                weight,
                &format!("{reason}: {}", found.text.trim()),
            );
        }
    }

    let mut hot_files = scored
        .into_iter()
        .map(|(path, acc)| HotFileRecord {
            path,
            score: acc.score,
            reasons: acc.reasons,
        })
        .collect::<Vec<_>>();
    hot_files.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    hot_files.truncate(limit.max(1));
    Ok(hot_files)
}

fn record_hot_file_reason(
    scored: &mut BTreeMap<String, HotFileAccumulator>,
    path: &str,
    weight: usize,
    reason: &str,
) {
    let entry = scored.entry(path.to_string()).or_default();
    entry.score += weight;
    if !entry.reasons.iter().any(|existing| existing == reason) {
        entry.reasons.push(reason.to_string());
    }
}

async fn discover_message_families(
    root: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<MessageFamilyRecord>, FunctionCallError> {
    let mut families: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for found in run_rg_match_search(
        "[A-Z]{2,}TO[A-Z]{2,}_[A-Z0-9_]+",
        None,
        root,
        limit * 16,
        cwd,
    )
    .await?
    {
        for token in extract_protocol_tokens(&found.text) {
            let family = token.split('_').next().unwrap_or(&token).to_string();
            let entry = families.entry(family).or_insert_with(|| (0, Vec::new()));
            entry.0 += 1;
            if entry.1.len() < 6 && !entry.1.iter().any(|existing| existing == &token) {
                entry.1.push(token.clone());
            }
        }
    }

    let mut out = families
        .into_iter()
        .map(|(name, (count, examples))| MessageFamilyRecord {
            name,
            count,
            examples,
        })
        .collect::<Vec<_>>();
    out.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    out.truncate(limit.max(1));
    Ok(out)
}

fn extract_protocol_tokens(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| {
            token.contains("TO")
                && token.contains('_')
                && token
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        })
        .map(ToString::to_string)
        .collect()
}

async fn discover_version_clues(
    root: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<VersionClueRecord>, FunctionCallError> {
    let clues = run_rg_match_search(
        "(protocolVersion|version|fver|sequence_id)",
        None,
        root,
        limit * 4,
        cwd,
    )
    .await?;
    Ok(clues
        .into_iter()
        .take(limit.max(1))
        .map(|found| VersionClueRecord {
            path: found.path.display().to_string(),
            line: found.line,
            text: found.text,
        })
        .collect())
}

async fn run_rg_file_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<PathBuf>, FunctionCallError> {
    let output = run_rg_command(
        pattern,
        include,
        search_path,
        cwd,
        &["--files-with-matches", "--sortr=modified"],
    )
    .await?;

    match output.status.code() {
        Some(0) => Ok(parse_file_results(&output.stdout, limit)),
        Some(1) => Ok(Vec::new()),
        _ => Err(rg_error(&output.stderr)),
    }
}

async fn run_rg_match_search(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    limit: usize,
    cwd: &Path,
) -> Result<Vec<GrepMatch>, FunctionCallError> {
    let output = run_rg_command(
        pattern,
        include,
        search_path,
        cwd,
        &["--line-number", "--with-filename", "--color", "never"],
    )
    .await?;

    match output.status.code() {
        Some(0) => Ok(parse_match_results(&output.stdout, limit)),
        Some(1) => Ok(Vec::new()),
        _ => Err(rg_error(&output.stderr)),
    }
}

async fn run_rg_files_listing(
    search_path: &Path,
    cwd: &Path,
) -> Result<Vec<PathBuf>, FunctionCallError> {
    let mut command = Command::new("rg");
    command
        .current_dir(cwd)
        .arg("--files")
        .arg("--no-messages")
        .arg("--")
        .arg(search_path);

    match timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            FunctionCallError::RespondToModel(
                "rg file listing timed out after 30 seconds".to_string(),
            )
        })? {
        Ok(output) if output.status.success() => Ok(parse_file_results(&output.stdout, usize::MAX)),
        Ok(output) if output.status.code() == Some(1) => Ok(Vec::new()),
        Ok(output) => Err(rg_error(&output.stderr)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let mut files = Vec::new();
            collect_files_fallback(search_path, &mut files).await?;
            Ok(files)
        }
        Err(err) => Err(FunctionCallError::RespondToModel(format!(
            "failed to launch rg for file listing: {err}"
        ))),
    }
}

async fn collect_files_fallback(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), FunctionCallError> {
    let metadata = tokio::fs::metadata(root).await.map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to inspect `{}`: {err}", root.display()))
    })?;

    if metadata.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }

    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to read directory `{}`: {err}",
                dir.display()
            ))
        })?;
        while let Some(entry) = entries.next_entry().await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to iterate directory `{}`: {err}",
                dir.display()
            ))
        })? {
            let path = entry.path();
            let file_type = entry.file_type().await.map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to inspect `{}`: {err}",
                    path.display()
                ))
            })?;
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }
    Ok(())
}

async fn run_rg_command(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    cwd: &Path,
    extra_args: &[&str],
) -> Result<std::process::Output, FunctionCallError> {
    let files_only = extra_args.contains(&"--files-with-matches");
    let mut command = Command::new("rg");
    command.current_dir(cwd);
    for arg in extra_args {
        command.arg(arg);
    }
    command.arg("--regexp").arg(pattern).arg("--no-messages");

    if let Some(glob) = include {
        command.arg("--glob").arg(glob);
    }

    command.arg("--").arg(search_path);

    match timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            FunctionCallError::RespondToModel("rg timed out after 30 seconds".to_string())
        })? {
        Ok(output) => Ok(output),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            run_grep_fallback(pattern, include, search_path, cwd, files_only).await
        }
        Err(err) => Err(FunctionCallError::RespondToModel(format!(
            "failed to launch rg: {err}. Ensure ripgrep is installed and on PATH."
        ))),
    }
}

async fn run_grep_fallback(
    pattern: &str,
    include: Option<&str>,
    search_path: &Path,
    cwd: &Path,
    files_only: bool,
) -> Result<std::process::Output, FunctionCallError> {
    let mut command = Command::new("grep");
    command.current_dir(cwd).arg("-R");
    if files_only {
        command.arg("-l");
    } else {
        command.arg("-n").arg("-H");
    }
    if let Some(glob) = include {
        command.arg(format!("--include={glob}"));
    }
    command.arg("-E").arg(pattern).arg(search_path);

    timeout(COMMAND_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            FunctionCallError::RespondToModel(
                "grep fallback timed out after 30 seconds".to_string(),
            )
        })?
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to launch rg, and grep fallback also failed: {err}"
            ))
        })
}

fn rg_error(stderr: &[u8]) -> FunctionCallError {
    let stderr = String::from_utf8_lossy(stderr);
    FunctionCallError::RespondToModel(format!("rg failed: {stderr}"))
}

fn parse_file_results(stdout: &[u8], limit: usize) -> Vec<PathBuf> {
    stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            if line.is_empty() {
                return None;
            }
            std::str::from_utf8(line).ok().map(PathBuf::from)
        })
        .take(limit)
        .collect()
}

fn parse_match_results(stdout: &[u8], limit: usize) -> Vec<GrepMatch> {
    let mut matches = Vec::new();
    for line in stdout.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let mut parts = text.splitn(3, ':');
        let Some(path) = parts.next() else { continue };
        let Some(line_no) = parts.next() else {
            continue;
        };
        let Some(match_text) = parts.next() else {
            continue;
        };
        let Ok(line) = line_no.parse::<usize>() else {
            continue;
        };
        matches.push(GrepMatch {
            path: PathBuf::from(path),
            line,
            text: match_text.to_string(),
        });
        if matches.len() == limit {
            break;
        }
    }
    matches
}

fn render_pipeline_value(value: PipelineValue) -> String {
    match value {
        PipelineValue::Files(files) => {
            if files.is_empty() {
                "No matches found.".to_string()
            } else {
                files
                    .into_iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        PipelineValue::Matches(matches) => {
            if matches.is_empty() {
                "No matches found.".to_string()
            } else {
                matches
                    .into_iter()
                    .map(|found| {
                        format!("{}:L{}: {}", found.path.display(), found.line, found.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        PipelineValue::Reads(reads) => render_read_artifacts(&reads),
    }
}

fn render_read_artifacts(artifacts: &[ReadArtifact]) -> String {
    if artifacts.is_empty() {
        return "No reads produced.".to_string();
    }
    if artifacts.len() == 1 && artifacts[0].label.is_none() {
        return artifacts[0].lines.join("\n");
    }

    let mut grouped: BTreeMap<String, Vec<&ReadArtifact>> = BTreeMap::new();
    for artifact in artifacts {
        grouped
            .entry(artifact.path.display().to_string())
            .or_default()
            .push(artifact);
    }

    let mut sections = Vec::new();
    for (path, artifacts) in grouped {
        for artifact in artifacts {
            let header = artifact
                .label
                .as_ref()
                .map(|label| format!("==> {path} ({label})"))
                .unwrap_or_else(|| format!("==> {path}"));
            sections.push(format!("{header}\n{}", artifact.lines.join("\n")));
        }
    }
    sections.join("\n\n")
}

mod slice {
    use crate::function_tool::FunctionCallError;
    use crate::tools::handlers::read_file::format_line;
    use std::path::Path;
    use tokio::fs::File;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;

    pub async fn read(
        path: &Path,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<String>, FunctionCallError> {
        let file = File::open(path).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
        })?;

        let mut reader = BufReader::new(file);
        let mut collected = Vec::new();
        let mut seen = 0usize;
        let mut buffer = Vec::new();

        loop {
            buffer.clear();
            let bytes_read = reader.read_until(b'\n', &mut buffer).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
            })?;

            if bytes_read == 0 {
                break;
            }

            if buffer.last() == Some(&b'\n') {
                buffer.pop();
                if buffer.last() == Some(&b'\r') {
                    buffer.pop();
                }
            }

            seen += 1;

            if seen < offset {
                continue;
            }

            if collected.len() == limit {
                break;
            }

            let formatted = format_line(&buffer);
            collected.push(format!("L{seen}: {formatted}"));

            if collected.len() == limit {
                break;
            }
        }

        if seen < offset {
            return Err(FunctionCallError::RespondToModel(
                "offset exceeds file length".to_string(),
            ));
        }

        Ok(collected)
    }
}

mod indentation {
    use crate::function_tool::FunctionCallError;
    use crate::tools::handlers::read_file::IndentationArgs;
    use crate::tools::handlers::read_file::LineRecord;
    use crate::tools::handlers::read_file::TAB_WIDTH;
    use crate::tools::handlers::read_file::format_line;
    use crate::tools::handlers::read_file::trim_empty_lines;
    use std::collections::VecDeque;
    use std::path::Path;
    use tokio::fs::File;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;

    pub async fn read_block(
        path: &Path,
        offset: usize,
        limit: usize,
        options: IndentationArgs,
    ) -> Result<Vec<String>, FunctionCallError> {
        let anchor_line = options.anchor_line.unwrap_or(offset);
        if anchor_line == 0 {
            return Err(FunctionCallError::RespondToModel(
                "anchor_line must be a 1-indexed line number".to_string(),
            ));
        }

        let guard_limit = options.max_lines.unwrap_or(limit);
        if guard_limit == 0 {
            return Err(FunctionCallError::RespondToModel(
                "max_lines must be greater than zero".to_string(),
            ));
        }

        let collected = collect_file_lines(path).await?;
        if collected.is_empty() || anchor_line > collected.len() {
            return Err(FunctionCallError::RespondToModel(
                "anchor_line exceeds file length".to_string(),
            ));
        }

        let anchor_index = anchor_line - 1;
        let effective_indents = compute_effective_indents(&collected);
        let anchor_indent = effective_indents[anchor_index];

        // Compute the min indent
        let min_indent = if options.max_levels == 0 {
            0
        } else {
            anchor_indent.saturating_sub(options.max_levels * TAB_WIDTH)
        };

        // Cap requested lines by guard_limit and file length
        let final_limit = limit.min(guard_limit).min(collected.len());

        if final_limit == 1 {
            return Ok(vec![format!(
                "L{}: {}",
                collected[anchor_index].number, collected[anchor_index].display
            )]);
        }

        // Cursors
        let mut i: isize = anchor_index as isize - 1; // up (inclusive)
        let mut j: usize = anchor_index + 1; // down (inclusive)
        let mut i_counter_min_indent = 0;
        let mut j_counter_min_indent = 0;

        let mut out = VecDeque::with_capacity(limit);
        out.push_back(&collected[anchor_index]);

        while out.len() < final_limit {
            let mut progressed = 0;

            // Up.
            if i >= 0 {
                let iu = i as usize;
                if effective_indents[iu] >= min_indent {
                    out.push_front(&collected[iu]);
                    progressed += 1;
                    i -= 1;

                    // We do not include the siblings (not applied to comments).
                    if effective_indents[iu] == min_indent && !options.include_siblings {
                        let allow_header_comment =
                            options.include_header && collected[iu].is_comment();
                        let can_take_line = allow_header_comment || i_counter_min_indent == 0;

                        if can_take_line {
                            i_counter_min_indent += 1;
                        } else {
                            // This line shouldn't have been taken.
                            out.pop_front();
                            progressed -= 1;
                            i = -1; // consider using Option<usize> or a control flag instead of a sentinel
                        }
                    }

                    // Short-cut.
                    if out.len() >= final_limit {
                        break;
                    }
                } else {
                    // Stop moving up.
                    i = -1;
                }
            }

            // Down.
            if j < collected.len() {
                let ju = j;
                if effective_indents[ju] >= min_indent {
                    out.push_back(&collected[ju]);
                    progressed += 1;
                    j += 1;

                    // We do not include the siblings (applied to comments).
                    if effective_indents[ju] == min_indent && !options.include_siblings {
                        if j_counter_min_indent > 0 {
                            // This line shouldn't have been taken.
                            out.pop_back();
                            progressed -= 1;
                            j = collected.len();
                        }
                        j_counter_min_indent += 1;
                    }
                } else {
                    // Stop moving down.
                    j = collected.len();
                }
            }

            if progressed == 0 {
                break;
            }
        }

        // Trim empty lines
        trim_empty_lines(&mut out);

        Ok(out
            .into_iter()
            .map(|record| format!("L{}: {}", record.number, record.display))
            .collect())
    }

    async fn collect_file_lines(path: &Path) -> Result<Vec<LineRecord>, FunctionCallError> {
        let file = File::open(path).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
        })?;

        let mut reader = BufReader::new(file);
        let mut buffer = Vec::new();
        let mut lines = Vec::new();
        let mut number = 0usize;

        loop {
            buffer.clear();
            let bytes_read = reader.read_until(b'\n', &mut buffer).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("failed to read file: {err}"))
            })?;

            if bytes_read == 0 {
                break;
            }

            if buffer.last() == Some(&b'\n') {
                buffer.pop();
                if buffer.last() == Some(&b'\r') {
                    buffer.pop();
                }
            }

            number += 1;
            let raw = String::from_utf8_lossy(&buffer).into_owned();
            let indent = measure_indent(&raw);
            let display = format_line(&buffer);
            lines.push(LineRecord {
                number,
                raw,
                display,
                indent,
            });
        }

        Ok(lines)
    }

    fn compute_effective_indents(records: &[LineRecord]) -> Vec<usize> {
        let mut effective = Vec::with_capacity(records.len());
        let mut previous_indent = 0usize;
        for record in records {
            if record.is_blank() {
                effective.push(previous_indent);
            } else {
                previous_indent = record.indent;
                effective.push(previous_indent);
            }
        }
        effective
    }

    fn measure_indent(line: &str) -> usize {
        line.chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
            .sum()
    }
}

fn format_line(bytes: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    if decoded.len() > MAX_LINE_LENGTH {
        take_bytes_at_char_boundary(&decoded, MAX_LINE_LENGTH).to_string()
    } else {
        decoded.into_owned()
    }
}

fn trim_empty_lines(out: &mut VecDeque<&LineRecord>) {
    while matches!(out.front(), Some(line) if line.raw.trim().is_empty()) {
        out.pop_front();
    }
    while matches!(out.back(), Some(line) if line.raw.trim().is_empty()) {
        out.pop_back();
    }
}

mod defaults {
    use super::*;

    impl Default for IndentationArgs {
        fn default() -> Self {
            Self {
                anchor_line: None,
                max_levels: max_levels(),
                include_siblings: include_siblings(),
                include_header: include_header(),
                max_lines: None,
            }
        }
    }

    pub fn offset() -> usize {
        1
    }

    pub fn limit() -> usize {
        2000
    }

    pub fn grep_limit() -> usize {
        DEFAULT_GREP_LIMIT
    }

    pub fn map_limit() -> usize {
        12
    }

    pub fn write_artifacts() -> bool {
        false
    }

    pub fn max_levels() -> usize {
        0
    }

    pub fn include_siblings() -> bool {
        false
    }

    pub fn include_header() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::indentation::read_block;
    use super::slice::read;
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn reads_requested_range() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "alpha
beta
gamma
"
        )?;

        let lines = read(temp.path(), 2, 2).await?;
        assert_eq!(lines, vec!["L2: beta".to_string(), "L3: gamma".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn errors_when_offset_exceeds_length() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        writeln!(temp, "only")?;

        let err = read(temp.path(), 3, 1)
            .await
            .expect_err("offset exceeds length");
        assert_eq!(
            err,
            FunctionCallError::RespondToModel("offset exceeds file length".to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn reads_non_utf8_lines() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        temp.as_file_mut().write_all(b"\xff\xfe\nplain\n")?;

        let lines = read(temp.path(), 1, 2).await?;
        let expected_first = format!("L1: {}{}", '\u{FFFD}', '\u{FFFD}');
        assert_eq!(lines, vec![expected_first, "L2: plain".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn trims_crlf_endings() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(temp, "one\r\ntwo\r\n")?;

        let lines = read(temp.path(), 1, 2).await?;
        assert_eq!(lines, vec!["L1: one".to_string(), "L2: two".to_string()]);
        Ok(())
    }

    #[tokio::test]
    async fn respects_limit_even_with_more_lines() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "first
second
third
"
        )?;

        let lines = read(temp.path(), 1, 2).await?;
        assert_eq!(
            lines,
            vec!["L1: first".to_string(), "L2: second".to_string()]
        );
        Ok(())
    }

    #[tokio::test]
    async fn truncates_lines_longer_than_max_length() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        let long_line = "x".repeat(MAX_LINE_LENGTH + 50);
        writeln!(temp, "{long_line}")?;

        let lines = read(temp.path(), 1, 1).await?;
        let expected = "x".repeat(MAX_LINE_LENGTH);
        assert_eq!(lines, vec![format!("L1: {expected}")]);
        Ok(())
    }

    #[test]
    fn parses_ripgrep_match_results() {
        let stdout = b"/tmp/a.rs:12:fn alpha()\n/tmp/b.rs:44:let beta = 1;\n";
        let parsed = parse_match_results(stdout, 10);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].path, PathBuf::from("/tmp/a.rs"));
        assert_eq!(parsed[0].line, 12);
        assert_eq!(parsed[0].text, "fn alpha()");
        assert_eq!(parsed[1].path, PathBuf::from("/tmp/b.rs"));
        assert_eq!(parsed[1].line, 44);
    }

    #[test]
    fn renders_multi_artifact_output_with_headers() {
        let rendered = render_read_artifacts(&[
            ReadArtifact {
                path: PathBuf::from("/tmp/a.rs"),
                label: Some("match L12".to_string()),
                lines: vec![
                    "L10: fn alpha() {".to_string(),
                    "L12:     beta();".to_string(),
                ],
            },
            ReadArtifact {
                path: PathBuf::from("/tmp/b.rs"),
                label: None,
                lines: vec!["L1: let gamma = 3;".to_string()],
            },
        ]);
        assert!(rendered.contains("==> /tmp/a.rs (match L12)"));
        assert!(rendered.contains("==> /tmp/b.rs"));
        assert!(rendered.contains("L12:     beta();"));
    }

    #[test]
    fn extracts_protocol_family_tokens_from_match_text() {
        let tokens = extract_protocol_tokens("CLTOMA_FUSE_REGISTER and MATOCL_FUSE_REPLY");
        assert_eq!(
            tokens,
            vec![
                "CLTOMA_FUSE_REGISTER".to_string(),
                "MATOCL_FUSE_REPLY".to_string()
            ]
        );
    }
    #[tokio::test]
    async fn indentation_mode_captures_block() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "fn outer() {{
    if cond {{
        inner();
    }}
    tail();
}}
"
        )?;

        let options = IndentationArgs {
            anchor_line: Some(3),
            include_siblings: false,
            max_levels: 1,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 3, 10, options).await?;

        assert_eq!(
            lines,
            vec![
                "L2:     if cond {".to_string(),
                "L3:         inner();".to_string(),
                "L4:     }".to_string()
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_expands_parents() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "mod root {{
    fn outer() {{
        if cond {{
            inner();
        }}
    }}
}}
"
        )?;

        let mut options = IndentationArgs {
            anchor_line: Some(4),
            max_levels: 2,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 4, 50, options.clone()).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     fn outer() {".to_string(),
                "L3:         if cond {".to_string(),
                "L4:             inner();".to_string(),
                "L5:         }".to_string(),
                "L6:     }".to_string(),
            ]
        );

        options.max_levels = 3;
        let expanded = read_block(temp.path(), 4, 50, options).await?;
        assert_eq!(
            expanded,
            vec![
                "L1: mod root {".to_string(),
                "L2:     fn outer() {".to_string(),
                "L3:         if cond {".to_string(),
                "L4:             inner();".to_string(),
                "L5:         }".to_string(),
                "L6:     }".to_string(),
                "L7: }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_respects_sibling_flag() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "fn wrapper() {{
    if first {{
        do_first();
    }}
    if second {{
        do_second();
    }}
}}
"
        )?;

        let mut options = IndentationArgs {
            anchor_line: Some(3),
            include_siblings: false,
            max_levels: 1,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 3, 50, options.clone()).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     if first {".to_string(),
                "L3:         do_first();".to_string(),
                "L4:     }".to_string(),
            ]
        );

        options.include_siblings = true;
        let with_siblings = read_block(temp.path(), 3, 50, options).await?;
        assert_eq!(
            with_siblings,
            vec![
                "L2:     if first {".to_string(),
                "L3:         do_first();".to_string(),
                "L4:     }".to_string(),
                "L5:     if second {".to_string(),
                "L6:         do_second();".to_string(),
                "L7:     }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_python_sample() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "class Foo:
    def __init__(self, size):
        self.size = size
    def double(self, value):
        if value is None:
            return 0
        result = value * self.size
        return result
class Bar:
    def compute(self):
        helper = Foo(2)
        return helper.double(5)
"
        )?;

        let options = IndentationArgs {
            anchor_line: Some(7),
            include_siblings: true,
            max_levels: 1,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 1, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L2:     def __init__(self, size):".to_string(),
                "L3:         self.size = size".to_string(),
                "L4:     def double(self, value):".to_string(),
                "L5:         if value is None:".to_string(),
                "L6:             return 0".to_string(),
                "L7:         result = value * self.size".to_string(),
                "L8:         return result".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn indentation_mode_handles_javascript_sample() -> anyhow::Result<()> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "export function makeThing() {{
    const cache = new Map();
    function ensure(key) {{
        if (!cache.has(key)) {{
            cache.set(key, []);
        }}
        return cache.get(key);
    }}
    const handlers = {{
        init() {{
            console.log(\"init\");
        }},
        run() {{
            if (Math.random() > 0.5) {{
                return \"heads\";
            }}
            return \"tails\";
        }},
    }};
    return {{ cache, handlers }};
}}
export function other() {{
    return makeThing();
}}
"
        )?;

        let options = IndentationArgs {
            anchor_line: Some(15),
            max_levels: 1,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 15, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L10:         init() {".to_string(),
                "L11:             console.log(\"init\");".to_string(),
                "L12:         },".to_string(),
                "L13:         run() {".to_string(),
                "L14:             if (Math.random() > 0.5) {".to_string(),
                "L15:                 return \"heads\";".to_string(),
                "L16:             }".to_string(),
                "L17:             return \"tails\";".to_string(),
                "L18:         },".to_string(),
            ]
        );
        Ok(())
    }

    fn write_cpp_sample() -> anyhow::Result<NamedTempFile> {
        let mut temp = NamedTempFile::new()?;
        use std::io::Write as _;
        write!(
            temp,
            "#include <vector>
#include <string>

namespace sample {{
class Runner {{
public:
    void setup() {{
        if (enabled_) {{
            init();
        }}
    }}

    // Run the code
    int run() const {{
        switch (mode_) {{
            case Mode::Fast:
                return fast();
            case Mode::Slow:
                return slow();
            default:
                return fallback();
        }}
    }}

private:
    bool enabled_ = false;
    Mode mode_ = Mode::Fast;

    int fast() const {{
        return 1;
    }}
}};
}}  // namespace sample
"
        )?;
        Ok(temp)
    }

    #[tokio::test]
    async fn indentation_mode_handles_cpp_sample_shallow() -> anyhow::Result<()> {
        let temp = write_cpp_sample()?;

        let options = IndentationArgs {
            include_siblings: false,
            anchor_line: Some(18),
            max_levels: 1,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 18, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L15:         switch (mode_) {".to_string(),
                "L16:             case Mode::Fast:".to_string(),
                "L17:                 return fast();".to_string(),
                "L18:             case Mode::Slow:".to_string(),
                "L19:                 return slow();".to_string(),
                "L20:             default:".to_string(),
                "L21:                 return fallback();".to_string(),
                "L22:         }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_cpp_sample() -> anyhow::Result<()> {
        let temp = write_cpp_sample()?;

        let options = IndentationArgs {
            include_siblings: false,
            anchor_line: Some(18),
            max_levels: 2,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 18, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L13:     // Run the code".to_string(),
                "L14:     int run() const {".to_string(),
                "L15:         switch (mode_) {".to_string(),
                "L16:             case Mode::Fast:".to_string(),
                "L17:                 return fast();".to_string(),
                "L18:             case Mode::Slow:".to_string(),
                "L19:                 return slow();".to_string(),
                "L20:             default:".to_string(),
                "L21:                 return fallback();".to_string(),
                "L22:         }".to_string(),
                "L23:     }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_cpp_sample_no_headers() -> anyhow::Result<()> {
        let temp = write_cpp_sample()?;

        let options = IndentationArgs {
            include_siblings: false,
            include_header: false,
            anchor_line: Some(18),
            max_levels: 2,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 18, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L14:     int run() const {".to_string(),
                "L15:         switch (mode_) {".to_string(),
                "L16:             case Mode::Fast:".to_string(),
                "L17:                 return fast();".to_string(),
                "L18:             case Mode::Slow:".to_string(),
                "L19:                 return slow();".to_string(),
                "L20:             default:".to_string(),
                "L21:                 return fallback();".to_string(),
                "L22:         }".to_string(),
                "L23:     }".to_string(),
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn indentation_mode_handles_cpp_sample_siblings() -> anyhow::Result<()> {
        let temp = write_cpp_sample()?;

        let options = IndentationArgs {
            include_siblings: true,
            include_header: false,
            anchor_line: Some(18),
            max_levels: 2,
            ..Default::default()
        };

        let lines = read_block(temp.path(), 18, 200, options).await?;
        assert_eq!(
            lines,
            vec![
                "L7:     void setup() {".to_string(),
                "L8:         if (enabled_) {".to_string(),
                "L9:             init();".to_string(),
                "L10:         }".to_string(),
                "L11:     }".to_string(),
                "L12: ".to_string(),
                "L13:     // Run the code".to_string(),
                "L14:     int run() const {".to_string(),
                "L15:         switch (mode_) {".to_string(),
                "L16:             case Mode::Fast:".to_string(),
                "L17:                 return fast();".to_string(),
                "L18:             case Mode::Slow:".to_string(),
                "L19:                 return slow();".to_string(),
                "L20:             default:".to_string(),
                "L21:                 return fallback();".to_string(),
                "L22:         }".to_string(),
                "L23:     }".to_string(),
            ]
        );
        Ok(())
    }
}
