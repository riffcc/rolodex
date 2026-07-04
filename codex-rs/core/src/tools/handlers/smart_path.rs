use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::function_tool::FunctionCallError;

pub(super) fn canonicalize_path(path: &Path, label: &str) -> Result<PathBuf, FunctionCallError> {
    path.canonicalize().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "unable to resolve {label} `{}`: {err}",
            path.display()
        ))
    })
}

pub(super) fn resolve_existing_path(
    cwd: &Path,
    requested_path: &str,
    label: &str,
) -> Result<PathBuf, FunctionCallError> {
    let raw_path = Path::new(requested_path);
    let primary = path_from_cwd(cwd, raw_path);

    match primary.canonicalize() {
        Ok(path) => Ok(path),
        Err(primary_err) => {
            if let Some(fallback) = without_redundant_cwd_prefix(cwd, raw_path)
                && let Ok(path) = fallback.canonicalize()
            {
                return Ok(path);
            }

            Err(FunctionCallError::RespondToModel(format!(
                "unable to resolve {label} `{}`: {primary_err}",
                primary.display()
            )))
        }
    }
}

fn path_from_cwd(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn without_redundant_cwd_prefix(cwd: &Path, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }

    let cwd_name = cwd.file_name()?;
    let mut components = path.components();
    let first = components.next()?;
    let Component::Normal(first) = first else {
        return None;
    };
    if first != cwd_name {
        return None;
    }

    let stripped = components.as_path();
    if stripped.as_os_str().is_empty() {
        return None;
    }

    Some(cwd.join(stripped))
}

#[cfg(test)]
#[path = "smart_path_tests.rs"]
mod tests;
