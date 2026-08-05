use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gpui_mcp_html::Decorations;
use tempfile::NamedTempFile;

use crate::OutputDecorations;

/// Generate the window module consumed by exported or scaffolded GPUI applications.
pub(crate) fn persist_output_window_module(
    project_root: &Path,
    decorations: OutputDecorations,
) -> Result<PathBuf, OutputWindowError> {
    let directory = project_root.join(".gpui-studio/generated");
    fs::create_dir_all(&directory).map_err(|source| OutputWindowError::Io {
        operation: "create generated output directory",
        path: directory.clone(),
        source,
    })?;
    let destination = directory.join("window.rs");
    let source = match decorations {
        OutputDecorations::Native => Decorations::Native,
        OutputDecorations::Custom => Decorations::Custom,
    }
    .module_source();
    let mut temporary =
        NamedTempFile::new_in(&directory).map_err(|source| OutputWindowError::Io {
            operation: "stage generated window module",
            path: directory,
            source,
        })?;
    temporary
        .write_all(source.as_bytes())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| OutputWindowError::Io {
            operation: "flush generated window module",
            path: temporary.path().to_owned(),
            source,
        })?;
    temporary
        .persist(&destination)
        .map_err(|source| OutputWindowError::Persist {
            path: destination.clone(),
            source,
        })?;
    Ok(destination)
}

/// Failure to generate the runnable GPUI window policy.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OutputWindowError {
    #[error("{operation} `{}`", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("replace generated window module `{}`", path.display())]
    Persist {
        path: PathBuf,
        #[source]
        source: tempfile::PersistError,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::persist_output_window_module;
    use crate::OutputDecorations;

    #[test]
    fn output_toggle_generates_the_gpui_policy_for_each_platform_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let path = persist_output_window_module(root.path(), OutputDecorations::Custom)?;
        let custom = fs::read_to_string(&path)?;
        assert!(custom.contains("appears_transparent: true"));
        assert!(custom.contains("WindowDecorations::Client"));

        persist_output_window_module(root.path(), OutputDecorations::Native)?;
        let native = fs::read_to_string(path)?;
        assert!(native.contains("appears_transparent: false"));
        assert!(native.contains("WindowDecorations::Server"));
        Ok(())
    }
}
