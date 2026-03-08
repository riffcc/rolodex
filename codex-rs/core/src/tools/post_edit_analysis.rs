use std::collections::HashSet;
use std::path::Path;

use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_utils_absolute_path::AbsolutePathBuf;
use llm_code_sdk::tools::smart::CodeLayer;
use llm_code_sdk::tools::smart::SmartReadTool;

const MAX_ANALYZED_FILES: usize = 4;
const MAX_LAYER_LINES: usize = 12;
const MAX_LAYER_CHARS: usize = 700;

pub(crate) fn append_post_edit_analysis(
    body: &mut FunctionCallOutputBody,
    project_root: &Path,
    edited_paths: &[AbsolutePathBuf],
) {
    let Some(analysis) = build_post_edit_analysis(project_root, edited_paths) else {
        return;
    };

    match body {
        FunctionCallOutputBody::Text(text) => {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&analysis);
        }
        FunctionCallOutputBody::ContentItems(items) => {
            items.push(FunctionCallOutputContentItem::InputText { text: analysis });
        }
    }
}

fn build_post_edit_analysis(
    project_root: &Path,
    edited_paths: &[AbsolutePathBuf],
) -> Option<String> {
    let paths = normalize_paths(project_root, edited_paths);
    if paths.is_empty() {
        return None;
    }

    let smart_read = SmartReadTool::new(project_root);
    let mut sections = Vec::new();

    for rel_path in paths {
        let layers = select_layers(&rel_path, edited_paths.len());
        if layers.is_empty() {
            continue;
        }

        let mut layer_sections = Vec::new();
        for layer in layers {
            if let Ok(view) = smart_read.read_at_layer(&rel_path, layer) {
                let content = compact_layer_content(&view.to_context());
                if !content.is_empty() {
                    layer_sections.push(format!(
                        "### {} [{}]\n{}",
                        rel_path,
                        layer_name(layer),
                        content
                    ));
                }
            }
        }

        if !layer_sections.is_empty() {
            sections.push(layer_sections.join("\n\n"));
        }
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!("## SmartRead Impact\n\n{}", sections.join("\n\n")))
    }
}

fn normalize_paths(project_root: &Path, edited_paths: &[AbsolutePathBuf]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for path in edited_paths {
        let abs = path.as_path();
        let Ok(relative) = abs.strip_prefix(project_root) else {
            continue;
        };
        let rel = relative.to_string_lossy().to_string();
        if seen.insert(rel.clone()) {
            normalized.push(rel);
        }
        if normalized.len() >= MAX_ANALYZED_FILES {
            break;
        }
    }

    normalized
}

fn select_layers(rel_path: &str, edited_file_count: usize) -> Vec<CodeLayer> {
    let extension = Path::new(rel_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();

    if extension == "lean" {
        return vec![CodeLayer::Ast, CodeLayer::TheoryGraph];
    }

    let supported = matches!(
        extension,
        "rs"
            | "py"
            | "js"
            | "jsx"
            | "mjs"
            | "ts"
            | "tsx"
            | "go"
            | "pl"
            | "pm"
            | "cgi"
            | "t"
            | "nim"
            | "nims"
            | "nimble"
    );

    if !supported {
        return Vec::new();
    }

    let is_test_like = rel_path.contains("/test")
        || rel_path.contains("/tests/")
        || rel_path.ends_with("_test.rs")
        || rel_path.ends_with("_test.go")
        || rel_path.ends_with(".test.ts")
        || rel_path.ends_with(".test.js");

    if is_test_like || edited_file_count > 2 {
        vec![CodeLayer::Ast, CodeLayer::CallGraph]
    } else {
        vec![CodeLayer::Ast, CodeLayer::CallGraph, CodeLayer::Dfg]
    }
}

fn compact_layer_content(content: &str) -> String {
    let mut lines = Vec::new();
    let mut source_lines = content.lines().filter(|line| !line.trim().is_empty());
    for line in source_lines.by_ref().take(MAX_LAYER_LINES) {
        lines.push(line);
    }

    let mut compact = lines.join("\n");
    if compact.len() > MAX_LAYER_CHARS {
        compact.truncate(MAX_LAYER_CHARS);
        compact.push_str("\n[...]");
    } else if source_lines.next().is_some() {
        compact.push_str("\n[...]");
    }

    compact
}

fn layer_name(layer: CodeLayer) -> &'static str {
    match layer {
        CodeLayer::Raw => "raw",
        CodeLayer::Ast => "ast",
        CodeLayer::CallGraph => "call_graph",
        CodeLayer::Cfg => "cfg",
        CodeLayer::Dfg => "dfg",
        CodeLayer::Pdg => "pdg",
        CodeLayer::TheoryGraph => "theory_graph",
    }
}
