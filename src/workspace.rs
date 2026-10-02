//! The workspace: which projects are open, recent projects, and how each was
//! last viewed. Stored once per user, outside any project.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

const VERSION: u16 = 1;
const MAX_RECENT: usize = 24;
const MAX_BYTES: u64 = 512 * 1024;

/// A project the user opened before.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    /// Absolute folder.
    pub path: PathBuf,
    /// Display name.
    pub name: String,
    /// Seconds since the Unix epoch.
    pub opened_at: u64,
}

/// How a project was last viewed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// Current page.
    pub page: usize,
    /// Canvas zoom.
    pub zoom: f32,
    /// Canvas pan X.
    pub pan_x: f32,
    /// Canvas pan Y.
    pub pan_y: f32,
}

/// Panel layout shared by every project.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelLayout {
    /// Left sidebar width.
    pub left_width: f32,
    /// Right sidebar width.
    pub right_width: f32,
    /// Whether the sidebars are shown.
    pub panels_visible: bool,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            left_width: 240.0,
            right_width: 272.0,
            panels_visible: true,
        }
    }
}

impl PanelLayout {
    /// Clamp widths to usable bounds.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            left_width: self.left_width.clamp(180.0, 480.0),
            right_width: self.right_width.clamp(220.0, 520.0),
            ..self
        }
    }
}

/// Persisted workspace.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceState {
    version: u16,
    /// Most recent first.
    pub recent: Vec<RecentProject>,
    /// Open tabs, in order.
    pub open: Vec<PathBuf>,
    /// Active tab; `None` shows Home.
    pub active: Option<usize>,
    /// Per-project view, keyed by folder.
    pub views: BTreeMap<PathBuf, ViewState>,
    /// Dark editor theme.
    pub dark: bool,
    /// Sidebar layout.
    #[serde(default)]
    pub panels: PanelLayout,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            version: VERSION,
            recent: Vec::new(),
            open: Vec::new(),
            active: None,
            views: BTreeMap::new(),
            dark: false,
            panels: PanelLayout::default(),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where the workspace file lives: `$GPUI_STUDIO_CONFIG_DIR` or the platform
/// config directory.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("GPUI_STUDIO_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("workspace.ron"));
    }
    directories::ProjectDirs::from("dev", "gpui-studio", "GPUI Studio")
        .map(|dirs| dirs.config_dir().join("workspace.ron"))
}

impl WorkspaceState {
    /// Load, falling back to an empty workspace when missing or unreadable.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let read = || -> Result<Self> {
            let metadata = fs::metadata(path)?;
            anyhow::ensure!(metadata.len() <= MAX_BYTES, "workspace file is too large");
            let state: Self = ron::from_str(&fs::read_to_string(path)?)?;
            anyhow::ensure!(state.version == VERSION, "unsupported workspace version");
            Ok(state)
        };
        match read() {
            Ok(mut state) => {
                state.normalize();
                state
            }
            Err(error) => {
                if path.exists() {
                    eprintln!("ignoring workspace {}: {error:#}", path.display());
                }
                Self::default()
            }
        }
    }

    /// Write atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("workspace path has no parent")?;
        fs::create_dir_all(parent)?;
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;
        let mut file = NamedTempFile::new_in(parent)?;
        file.write_all(text.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)?;
        Ok(())
    }

    fn normalize(&mut self) {
        self.open.dedup();
        if self.active.is_some_and(|a| a >= self.open.len()) {
            self.active = self.open.len().checked_sub(1);
        }
        self.recent.truncate(MAX_RECENT);
        self.panels = self.panels.clamped();
    }

    /// Record that a project was opened.
    pub fn touch_recent(&mut self, path: &Path, name: &str) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(
            0,
            RecentProject {
                path: path.to_owned(),
                name: name.to_owned(),
                opened_at: now(),
            },
        );
        self.recent.truncate(MAX_RECENT);
    }

    /// Forget a recent project.
    pub fn forget_recent(&mut self, path: &Path) {
        self.recent.retain(|r| r.path != path);
    }

    /// Open (or focus) a tab for a project. Returns its index.
    pub fn open_tab(&mut self, path: &Path) -> usize {
        let index = match self.open.iter().position(|p| p == path) {
            Some(index) => index,
            None => {
                self.open.push(path.to_owned());
                self.open.len() - 1
            }
        };
        self.active = Some(index);
        index
    }

    /// Close a tab, activating a neighbor (or Home).
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.open.len() {
            return;
        }
        self.open.remove(index);
        self.active = match self.active {
            _ if self.open.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active == index => Some(index.min(self.open.len() - 1)),
            other => other,
        };
    }

    /// Move a tab.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.open.len() || to >= self.open.len() || from == to {
            return;
        }
        let tab = self.open.remove(from);
        self.open.insert(to, tab);
        if let Some(active) = self.active {
            self.active = Some(if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            });
        }
    }
}

/// "3 minutes ago"-style relative time.
#[must_use]
pub fn relative_time(then: u64) -> String {
    let seconds = now().saturating_sub(then);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        86_400..2_592_000 => format!("{} d ago", seconds / 86_400),
        _ => format!("{} mo ago", seconds / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_recents_and_views_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cfg/workspace.ron");
        let mut state = WorkspaceState::load(&file);
        assert_eq!(state, WorkspaceState::default());
        let (a, b, c) = (Path::new("/a"), Path::new("/b"), Path::new("/c"));
        assert_eq!(state.open_tab(a), 0);
        assert_eq!(state.open_tab(b), 1);
        assert_eq!(state.open_tab(a), 0, "reopening focuses the existing tab");
        state.open_tab(c);
        state.move_tab(2, 0);
        assert_eq!(state.open, vec![c.to_owned(), a.to_owned(), b.to_owned()]);
        assert_eq!(state.active, Some(0));
        state.close_tab(0);
        assert_eq!(state.active, Some(0));
        state.touch_recent(a, "A");
        state.touch_recent(b, "B");
        state.touch_recent(a, "A");
        assert_eq!(
            state
                .recent
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        state.views.insert(
            a.to_owned(),
            ViewState {
                page: 1,
                zoom: 0.5,
                pan_x: 3.0,
                pan_y: 4.0,
            },
        );
        state.panels.left_width = 9_999.0;
        state.save(&file).unwrap();
        let loaded = WorkspaceState::load(&file);
        assert_eq!(loaded.views, state.views);
        assert_eq!(
            loaded.panels.left_width, 480.0,
            "loading clamps panel widths"
        );
        state.close_tab(0);
        state.close_tab(0);
        assert_eq!(state.active, None, "closing the last tab shows Home");
        fs::write(&file, "garbage").unwrap();
        assert_eq!(WorkspaceState::load(&file), WorkspaceState::default());
        assert_eq!(relative_time(now()), "just now");
    }
}
