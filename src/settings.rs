use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{AuthoringBackend, CanvasSettings, ThemeSelection};

const SETTINGS_VERSION: u16 = 4;
const LEGACY_SETTINGS_VERSIONS: [u16; 3] = [1, 2, 3];
const MAX_SETTINGS_BYTES: u64 = 64 * 1024;

/// Primary Studio workspace mode restored between local sessions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum StudioMode {
    /// Select tool: native canvas, click-to-select, drag-to-move elements.
    #[default]
    Design,
    /// HTML/CSS/RON source surfaces.
    Source,
    /// Preview: run the component's interactions like the live app.
    Test,
    /// Annotate tool: visual annotation and revision comparison.
    Compare,
    /// Move tool: pan the canvas viewport (does not move elements).
    Move,
}

/// Optional editor-only state; never required to render a project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSettings {
    /// Versioned settings schema.
    pub version: u16,
    /// Absolute path or path relative to the Studio repository.
    pub project: PathBuf,
    /// Last active workspace mode.
    pub mode: StudioMode,
    /// Last active source projection. This does not select a different runtime.
    #[serde(default)]
    pub backend: AuthoringBackend,
    /// Responsive preview, output decorations, zoom, orientation, and snapping settings.
    #[serde(default)]
    pub canvas: CanvasSettings,
    /// Active editor-chrome theme, independent of project preview themes.
    #[serde(default)]
    pub theme: ThemeSelection,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            project: PathBuf::from("examples/welcome"),
            mode: StudioMode::Design,
            backend: AuthoringBackend::Html,
            canvas: CanvasSettings::default(),
            theme: ThemeSelection::default(),
        }
    }
}

impl WorkspaceSettings {
    /// Load `.gpui-studio/workspace.ron`, falling back to portable defaults when absent.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, unreadable, invalid, or unsupported settings.
    pub fn load(studio_root: &Path) -> Result<Self, WorkspaceSettingsError> {
        let path = studio_root.join(".gpui-studio/workspace.ron");
        if !path.exists() {
            return Ok(Self::default());
        }
        let metadata = fs::metadata(&path).map_err(|source| WorkspaceSettingsError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_SETTINGS_BYTES {
            return Err(WorkspaceSettingsError::TooLarge {
                found: metadata.len(),
                maximum: MAX_SETTINGS_BYTES,
            });
        }
        let source = fs::read_to_string(&path).map_err(|source| WorkspaceSettingsError::Io {
            path: path.clone(),
            source,
        })?;
        let mut settings: Self = ron::from_str(&source)?;
        if LEGACY_SETTINGS_VERSIONS.contains(&settings.version) {
            settings.version = SETTINGS_VERSION;
        } else if settings.version != SETTINGS_VERSION {
            return Err(WorkspaceSettingsError::UnsupportedVersion {
                found: settings.version,
                supported: SETTINGS_VERSION,
            });
        }
        Ok(settings)
    }

    /// Persist editor-only workspace state through a flushed staged file.
    ///
    /// # Errors
    ///
    /// Returns an error when serialization, staging, flushing, or replacement fails.
    pub fn save(&self, studio_root: &Path) -> Result<(), WorkspaceSettingsError> {
        let directory = studio_root.join(".gpui-studio");
        fs::create_dir_all(&directory).map_err(|source| WorkspaceSettingsError::Io {
            path: directory.clone(),
            source,
        })?;
        let destination = directory.join("workspace.ron");
        let source = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        let mut temporary =
            NamedTempFile::new_in(&directory).map_err(|source| WorkspaceSettingsError::Io {
                path: directory,
                source,
            })?;
        temporary
            .write_all(source.as_bytes())
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| WorkspaceSettingsError::Io {
                path: temporary.path().to_owned(),
                source,
            })?;
        temporary
            .persist(&destination)
            .map_err(|source| WorkspaceSettingsError::Persist {
                path: destination,
                source,
            })?;
        Ok(())
    }

    /// Resolve the configured project without requiring machine-specific paths in source control.
    #[must_use]
    pub fn project_root(&self, studio_root: &Path) -> PathBuf {
        if self.project.is_absolute() {
            self.project.clone()
        } else {
            studio_root.join(&self.project)
        }
    }
}

/// Invalid editor-only workspace settings.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceSettingsError {
    /// Reading settings failed.
    #[error("read workspace settings `{}`", path.display())]
    Io {
        /// Settings path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// Settings exceeded the bounded local input size.
    #[error("workspace settings are {found} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed bytes.
        found: u64,
        /// Maximum bytes.
        maximum: u64,
    },
    /// RON parsing failed.
    #[error("parse workspace settings")]
    Parse(#[from] ron::error::SpannedError),
    /// RON serialization failed.
    #[error("serialize workspace settings")]
    Serialize(#[from] ron::Error),
    /// Settings used an unsupported schema revision.
    #[error("workspace settings version {found} is unsupported; expected {supported}")]
    UnsupportedVersion {
        /// Observed version.
        found: u16,
        /// Supported version.
        supported: u16,
    },
    /// Atomic settings replacement failed.
    #[error("replace workspace settings `{}`", path.display())]
    Persist {
        /// Destination that could not be replaced.
        path: PathBuf,
        /// Staged-file persistence error.
        #[source]
        source: tempfile::PersistError,
    },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::{OutputDecorations, ThemeMode, ThemeSelection};

    use super::{StudioMode, WorkspaceSettings, WorkspaceSettingsError};

    #[test]
    fn missing_settings_use_portable_welcome_project() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let settings = WorkspaceSettings::load(root.path())?;

        assert_eq!(settings.mode, StudioMode::Design);
        assert_eq!(
            settings.project_root(root.path()),
            root.path().join("examples/welcome")
        );
        Ok(())
    }

    #[test]
    fn unknown_fields_fail_closed() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        fs::create_dir(root.path().join(".gpui-studio"))?;
        fs::write(
            root.path().join(".gpui-studio/workspace.ron"),
            "(version:1,project:\".\",mode:Design,network:true)",
        )?;

        assert!(matches!(
            WorkspaceSettings::load(root.path()),
            Err(WorkspaceSettingsError::Parse(_))
        ));
        Ok(())
    }

    #[test]
    fn selected_theme_round_trips_in_workspace_state() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let settings = WorkspaceSettings {
            theme: ThemeSelection {
                name: "Paper".to_owned(),
                mode: ThemeMode::Light,
            },
            ..WorkspaceSettings::default()
        };

        settings.save(root.path())?;

        assert_eq!(WorkspaceSettings::load(root.path())?, settings);
        Ok(())
    }

    #[test]
    fn legacy_preview_chrome_migrates_to_output_decorations()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        fs::create_dir(root.path().join(".gpui-studio"))?;
        fs::write(
            root.path().join(".gpui-studio/workspace.ron"),
            "(version:3,project:\".\",mode:Design,canvas:(preset:Desktop,chrome:Browser,zoom_percent:100,quarter_turns:0,snap_enabled:true,snap_grid:8))",
        )?;

        let settings = WorkspaceSettings::load(root.path())?;
        assert_eq!(settings.canvas.decorations, OutputDecorations::Custom);
        Ok(())
    }
}
