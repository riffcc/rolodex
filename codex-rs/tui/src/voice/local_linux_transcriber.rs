use flate2::read::GzDecoder;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::Mutex;
use tar::Archive;
use tracing::info;
use transcribe_rs::TranscriptionEngine;
use transcribe_rs::engines::parakeet::ParakeetEngine;
use transcribe_rs::engines::parakeet::ParakeetInferenceParams;
use transcribe_rs::engines::parakeet::ParakeetModelParams;
use transcribe_rs::engines::parakeet::TimestampGranularity;

const LOCAL_VOICE_MODEL_ID: &str = "parakeet-tdt-0.6b-v3";
const LOCAL_VOICE_MODEL_DIRNAME: &str = "parakeet-tdt-0.6b-v3-int8";
const LOCAL_VOICE_MODEL_URL: &str = "https://blob.handy.computer/parakeet-v3-int8.tar.gz";

static DEFAULT_TRANSCRIBER: LazyLock<Mutex<LocalVoiceTranscriber>> =
    LazyLock::new(|| Mutex::new(LocalVoiceTranscriber::default()));

#[derive(Default)]
struct LocalVoiceTranscriber {
    loaded_model_dir: Option<PathBuf>,
    engine: Option<ParakeetEngine>,
}

pub(super) fn transcribe_local(samples: Vec<f32>) -> Result<String, String> {
    let model_dir = ensure_local_voice_assets()?;
    let mut transcriber = DEFAULT_TRANSCRIBER
        .lock()
        .map_err(|_| "local voice transcriber mutex poisoned".to_string())?;
    transcriber.transcribe(samples, &model_dir)
}

impl LocalVoiceTranscriber {
    fn transcribe(&mut self, samples: Vec<f32>, model_dir: &Path) -> Result<String, String> {
        if samples.is_empty() {
            return Ok(String::new());
        }

        self.ensure_model_loaded(model_dir)?;

        let result = match self.engine.as_mut() {
            Some(engine) => {
                let params = ParakeetInferenceParams {
                    timestamp_granularity: TimestampGranularity::Segment,
                    ..Default::default()
                };
                engine
                    .transcribe_samples(samples, Some(params))
                    .map_err(|err| format!("Parakeet transcription failed: {err}"))?
                    .text
            }
            None => return Err("no local voice engine loaded".to_string()),
        };

        Ok(result.trim().to_string())
    }

    fn ensure_model_loaded(&mut self, model_dir: &Path) -> Result<(), String> {
        if self.loaded_model_dir.as_deref() == Some(model_dir) && self.engine.is_some() {
            return Ok(());
        }

        let mut engine = ParakeetEngine::new();
        engine
            .load_model_with_params(model_dir, ParakeetModelParams::int8())
            .map_err(|err| {
                format!(
                    "failed to load Parakeet model {}: {err}",
                    local_voice_model_id()
                )
            })?;
        self.engine = Some(engine);
        self.loaded_model_dir = Some(model_dir.to_path_buf());
        info!("loaded local voice model from {}", model_dir.display());
        Ok(())
    }
}

fn ensure_local_voice_assets() -> Result<PathBuf, String> {
    let local_voice_root = local_voice_root()?;
    let model_dir = local_voice_root
        .join("models")
        .join(local_voice_model_dirname());
    let ready_marker = local_voice_ready_marker(&local_voice_root);

    if ready_marker.is_file() {
        return Ok(model_dir);
    }

    fs::create_dir_all(local_voice_root.join("models"))
        .map_err(|e| format!("failed to create local voice model directory: {e}"))?;
    download_and_extract_local_voice_model(&model_dir, &ready_marker)?;

    if !ready_marker.is_file() {
        return Err("local voice model installed but encoder file is missing".to_string());
    }

    Ok(model_dir)
}

fn download_and_extract_local_voice_model(
    model_dir: &Path,
    ready_marker: &Path,
) -> Result<(), String> {
    let cache_dir = local_voice_cache_dir()?;
    fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("failed to create local voice cache directory: {e}"))?;
    let archive_path = cache_dir.join(format!("{}.tar.gz", local_voice_model_dirname()));

    if !archive_path.is_file() {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create runtime for local voice bootstrap: {e}"))?;
        let bytes = rt.block_on(async {
            let response = reqwest::get(local_voice_model_url())
                .await
                .map_err(|e| format!("failed to download local voice model: {e}"))?;
            let status = response.status();
            if !status.is_success() {
                return Err(format!(
                    "failed to download local voice model: HTTP {status}"
                ));
            }
            response
                .bytes()
                .await
                .map_err(|e| format!("failed to read local voice model archive: {e}"))
        })?;
        fs::write(&archive_path, &bytes)
            .map_err(|e| format!("failed to write local voice archive: {e}"))?;
    }

    let tmp_dir = cache_dir.join(format!(
        "extract-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)
            .map_err(|e| format!("failed to clear stale local voice temp dir: {e}"))?;
    }
    fs::create_dir_all(&tmp_dir)
        .map_err(|e| format!("failed to create local voice temp dir: {e}"))?;

    let extract_result = (|| -> Result<(), String> {
        let file = fs::File::open(&archive_path)
            .map_err(|e| format!("failed to open local voice archive: {e}"))?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive
            .unpack(&tmp_dir)
            .map_err(|e| format!("failed to unpack local voice archive: {e}"))?;
        Ok(())
    })();

    if extract_result.is_err() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return extract_result;
    }

    let extracted_model_dir = tmp_dir.join(local_voice_model_dirname());
    if !extracted_model_dir.is_dir() {
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(format!(
            "local voice archive missing expected directory {}",
            local_voice_model_dirname()
        ));
    }

    if model_dir.exists() {
        fs::remove_dir_all(model_dir)
            .map_err(|e| format!("failed to replace local voice model directory: {e}"))?;
    }
    fs::rename(&extracted_model_dir, model_dir)
        .map_err(|e| format!("failed to install local voice model: {e}"))?;
    let _ = fs::remove_dir_all(&tmp_dir);

    if !ready_marker.is_file() {
        return Err("local voice model installed but encoder file is missing".to_string());
    }

    Ok(())
}

fn local_voice_root() -> Result<PathBuf, String> {
    let configured_root = std::env::var_os("RIFF_CODEX_LOCAL_VOICE_ROOT").map(PathBuf::from);
    let codex_root = default_local_voice_root()?;
    let legacy_root = legacy_handy_root()?;
    Ok(select_local_voice_root(
        configured_root,
        &codex_root,
        &legacy_root,
    ))
}

fn select_local_voice_root(
    configured_root: Option<PathBuf>,
    codex_root: &Path,
    legacy_root: &Path,
) -> PathBuf {
    if let Some(root) = configured_root {
        return root;
    }
    if local_voice_model_ready(codex_root) {
        return codex_root.to_path_buf();
    }
    if local_voice_model_ready(legacy_root) {
        return legacy_root.to_path_buf();
    }
    codex_root.to_path_buf()
}

fn default_local_voice_root() -> Result<PathBuf, String> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| "failed to resolve local data directory for voice bootstrap".to_string())?;
    Ok(data_dir.join("com.openai.codex").join("voice"))
}

fn legacy_handy_root() -> Result<PathBuf, String> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| "failed to resolve local data directory for voice bootstrap".to_string())?;
    Ok(data_dir.join("com.pais.handy"))
}

fn local_voice_cache_dir() -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var("RIFF_CODEX_LOCAL_VOICE_CACHE_DIR") {
        return Ok(PathBuf::from(root));
    }
    let cache_dir = dirs::cache_dir()
        .ok_or_else(|| "failed to resolve cache directory for voice bootstrap".to_string())?;
    Ok(cache_dir.join("riff-codex"))
}

fn local_voice_model_ready(root: &Path) -> bool {
    local_voice_ready_marker(root).is_file()
}

fn local_voice_ready_marker(root: &Path) -> PathBuf {
    root.join("models")
        .join(local_voice_model_dirname())
        .join("encoder-model.int8.onnx")
}

fn local_voice_model_id() -> &'static str {
    LOCAL_VOICE_MODEL_ID
}

fn local_voice_model_dirname() -> &'static str {
    LOCAL_VOICE_MODEL_DIRNAME
}

fn local_voice_model_url() -> &'static str {
    LOCAL_VOICE_MODEL_URL
}

#[cfg(test)]
mod tests {
    use super::local_voice_ready_marker;
    use super::select_local_voice_root;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn select_local_voice_root_prefers_configured_root() {
        let codex_root = PathBuf::from("/tmp/codex-voice");
        let legacy_root = PathBuf::from("/tmp/legacy-handy");
        let configured_root = Some(PathBuf::from("/tmp/custom-voice"));

        let root = select_local_voice_root(configured_root.clone(), &codex_root, &legacy_root);

        assert_eq!(root, configured_root.expect("configured root should exist"));
    }

    #[test]
    fn select_local_voice_root_prefers_codex_model_when_present() {
        let sandbox = tempdir().expect("tempdir should be created");
        let codex_root = sandbox.path().join("codex");
        let legacy_root = sandbox.path().join("legacy");
        write_ready_marker(&codex_root);
        write_ready_marker(&legacy_root);

        let root = select_local_voice_root(None, &codex_root, &legacy_root);

        assert_eq!(root, codex_root);
    }

    #[test]
    fn select_local_voice_root_reuses_legacy_model_when_codex_model_is_missing() {
        let sandbox = tempdir().expect("tempdir should be created");
        let codex_root = sandbox.path().join("codex");
        let legacy_root = sandbox.path().join("legacy");
        write_ready_marker(&legacy_root);

        let root = select_local_voice_root(None, &codex_root, &legacy_root);

        assert_eq!(root, legacy_root);
    }

    #[test]
    fn select_local_voice_root_defaults_to_codex_root_for_new_installs() {
        let sandbox = tempdir().expect("tempdir should be created");
        let codex_root = sandbox.path().join("codex");
        let legacy_root = sandbox.path().join("legacy");

        let root = select_local_voice_root(None, &codex_root, &legacy_root);

        assert_eq!(root, codex_root);
    }

    fn write_ready_marker(root: &Path) {
        let ready_marker = local_voice_ready_marker(root);
        fs::create_dir_all(
            ready_marker
                .parent()
                .expect("ready marker should have a parent directory"),
        )
        .expect("ready marker directory should be created");
        fs::write(ready_marker, "ok").expect("ready marker should be written");
    }
}
