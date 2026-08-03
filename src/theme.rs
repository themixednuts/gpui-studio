//! Layered, offline editor themes projected into the live HTML Studio shell.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::{Deserialize, Serialize};

const MAX_THEME_BYTES: u64 = 128 * 1024;
const MAX_THEME_FILES: usize = 64;
const THEME_EVENT_CAPACITY: usize = 128;
const BUNDLED_THEMES: [(&str, &str); 3] = [
    ("aurora.toml", include_str!("../themes/aurora.toml")),
    ("foundry.toml", include_str!("../themes/foundry.toml")),
    ("paper.toml", include_str!("../themes/paper.toml")),
];

/// Light or dark theme variant.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum ThemeMode {
    /// Light editor chrome.
    Light,
    /// Dark editor chrome.
    #[default]
    Dark,
}

/// Persisted editor theme selection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSelection {
    /// Human-readable theme name.
    pub name: String,
    /// Selected authored variant.
    pub mode: ThemeMode,
}

impl Default for ThemeSelection {
    fn default() -> Self {
        Self {
            name: "Aurora".to_owned(),
            mode: ThemeMode::Dark,
        }
    }
}

impl ThemeSelection {
    /// Compact label suitable for the Studio toolbar.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{} · {:?}", self.name, self.mode).to_uppercase()
    }
}

/// One filesystem layer supplying theme overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThemeLocation {
    /// Per-user themes, overridden by project themes of the same name.
    User(PathBuf),
    /// Project-local themes with the highest precedence.
    Project(PathBuf),
}

impl ThemeLocation {
    fn path(&self) -> &Path {
        match self {
            Self::User(path) | Self::Project(path) => path,
        }
    }
}

/// One selectable theme variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AvailableTheme {
    /// Theme name.
    pub name: String,
    /// Authored light or dark mode.
    pub mode: ThemeMode,
}

impl AvailableTheme {
    fn selection(&self) -> ThemeSelection {
        ThemeSelection {
            name: self.name.clone(),
            mode: self.mode,
        }
    }
}

/// Fully resolved semantic editor tokens.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThemeTokens {
    /// Outer application background.
    pub background: String,
    /// Persistent top and bottom editor chrome.
    pub chrome: String,
    /// Main shell background.
    pub shell: String,
    /// Side panel background.
    pub panel: String,
    /// Canvas-stage background.
    pub stage: String,
    /// Interactive surface background.
    pub surface: String,
    /// Hovered interactive surface.
    pub surface_hover: String,
    /// Deep inset surface.
    pub recessed: String,
    /// Standard border.
    pub border: String,
    /// Emphasized border.
    pub border_strong: String,
    /// Normal foreground.
    pub text: String,
    /// Emphasized foreground.
    pub text_strong: String,
    /// Muted foreground.
    pub text_muted: String,
    /// Primary accent.
    pub accent: String,
    /// Hovered primary accent.
    pub accent_hover: String,
    /// Foreground drawn on the accent.
    pub accent_text: String,
    /// Positive state accent.
    pub success: String,
    /// Embedded preview backdrop.
    pub preview: String,
    /// Embedded preview border.
    pub preview_border: String,
    /// Review-task surface.
    pub review_surface: String,
    /// Review-task border.
    pub review_border: String,
    /// Review-task foreground.
    pub review_text: String,
    /// Proportional UI font.
    pub font_family: String,
    /// Monospace UI font.
    pub mono_family: String,
}

impl ThemeTokens {
    fn defaults(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Light => Self::literal_light(),
            ThemeMode::Dark => Self::literal_dark(),
        }
    }

    fn from_definition(definition: &ThemeDefinition, mode: ThemeMode) -> Self {
        let fallback = match mode {
            ThemeMode::Light => Self::literal_light(),
            ThemeMode::Dark => Self::literal_dark(),
        };
        let variant = definition.variant(mode);
        let color = |name: &str, default: &str| {
            variant
                .colors
                .get(name)
                .or_else(|| definition.colors.get(name))
                .cloned()
                .unwrap_or_else(|| default.to_owned())
        };
        Self {
            background: color("background", &fallback.background),
            chrome: color("chrome", &fallback.chrome),
            shell: color("shell", &fallback.shell),
            panel: color("panel", &fallback.panel),
            stage: color("stage", &fallback.stage),
            surface: color("surface", &fallback.surface),
            surface_hover: color("surface_hover", &fallback.surface_hover),
            recessed: color("recessed", &fallback.recessed),
            border: color("border", &fallback.border),
            border_strong: color("border_strong", &fallback.border_strong),
            text: color("text", &fallback.text),
            text_strong: color("text_strong", &fallback.text_strong),
            text_muted: color("text_muted", &fallback.text_muted),
            accent: color("accent", &fallback.accent),
            accent_hover: color("accent_hover", &fallback.accent_hover),
            accent_text: color("accent_text", &fallback.accent_text),
            success: color("success", &fallback.success),
            preview: color("preview", &fallback.preview),
            preview_border: color("preview_border", &fallback.preview_border),
            review_surface: color("review_surface", &fallback.review_surface),
            review_border: color("review_border", &fallback.review_border),
            review_text: color("review_text", &fallback.review_text),
            font_family: variant
                .font
                .family
                .clone()
                .or_else(|| definition.font.family.clone())
                .unwrap_or(fallback.font_family),
            mono_family: variant
                .font
                .mono_family
                .clone()
                .or_else(|| definition.font.mono_family.clone())
                .unwrap_or(fallback.mono_family),
        }
    }

    fn literal_dark() -> Self {
        Self::literal(
            "#171812", "#20221b", "#1b1d17", "#24261f", "#d7d8cd", "#f0efe3", "#777a6d", "#d49334",
            "#17130d", "#deddd3",
        )
    }

    fn literal_light() -> Self {
        Self::literal(
            "#d8d3c7", "#ece8de", "#e2ddd1", "#d1cbc0", "#34352f", "#171914", "#6e6c63", "#b56f22",
            "#fff8e8", "#f7f5ee",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn literal(
        background: &str,
        shell: &str,
        panel: &str,
        stage: &str,
        text: &str,
        text_strong: &str,
        text_muted: &str,
        accent: &str,
        accent_text: &str,
        preview: &str,
    ) -> Self {
        Self {
            background: background.to_owned(),
            chrome: background.to_owned(),
            shell: shell.to_owned(),
            panel: panel.to_owned(),
            stage: stage.to_owned(),
            surface: if background == "#171812" {
                "#262820"
            } else {
                "#f4f1e9"
            }
            .to_owned(),
            surface_hover: if background == "#171812" {
                "#33362a"
            } else {
                "#fffdf7"
            }
            .to_owned(),
            recessed: if background == "#171812" {
                "#11130f"
            } else {
                "#cbc5b8"
            }
            .to_owned(),
            border: if background == "#171812" {
                "#3a3c32"
            } else {
                "#aaa292"
            }
            .to_owned(),
            border_strong: if background == "#171812" {
                "#55584a"
            } else {
                "#756d5e"
            }
            .to_owned(),
            text: text.to_owned(),
            text_strong: text_strong.to_owned(),
            text_muted: text_muted.to_owned(),
            accent: accent.to_owned(),
            accent_hover: if background == "#171812" {
                "#dfa042"
            } else {
                "#ce8735"
            }
            .to_owned(),
            accent_text: accent_text.to_owned(),
            success: if background == "#171812" {
                "#78b85c"
            } else {
                "#4f7d43"
            }
            .to_owned(),
            preview: preview.to_owned(),
            preview_border: if background == "#171812" {
                "#67695d"
            } else {
                "#938b7c"
            }
            .to_owned(),
            review_surface: if background == "#171812" {
                "#20251d"
            } else {
                "#dce5d4"
            }
            .to_owned(),
            review_border: if background == "#171812" {
                "#41513a"
            } else {
                "#8aa27c"
            }
            .to_owned(),
            review_text: if background == "#171812" {
                "#a8c696"
            } else {
                "#385a31"
            }
            .to_owned(),
            font_family: "IBM Plex Sans".to_owned(),
            mono_family: "IBM Plex Mono".to_owned(),
        }
    }
}

/// Selected theme with every fallback resolved.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedTheme {
    /// Actual selection after fallback.
    pub selection: ThemeSelection,
    /// Resolved semantic tokens.
    pub tokens: ThemeTokens,
}

impl ResolvedTheme {
    /// CSS appended to Studio's checked-in shell stylesheet.
    #[must_use]
    pub fn css_overlay(&self) -> String {
        let t = &self.tokens;
        format!(
            r#"
/* GPUI Studio resolved editor theme: {name} / {mode:?} */
body {{ background-color: {background}; color: {text}; font-family: "{font}", system-ui; }}
button {{ color: {text}; border-color: {border}; font-family: "{font}", system-ui; }}
button:hover {{ border-color: {border_strong}; color: {text_strong}; }}
#studio-shell {{ background-color: {shell}; }}
#studio-topbar, #canvas-toolbar, #bottom-dock {{ background-color: {chrome}; border-color: {border}; }}
#project-rail, #inspector {{ background-color: {panel}; border-color: {border}; }}
#center-column, #canvas-stage, #canvas-viewport {{ background-color: {stage}; }}
#tool-switcher, #backend-switcher, #code-tabs, #annotation-draft, .metric-grid > div, .inspector-row {{ background-color: {recessed}; border-color: {border}; }}
#annotation-popover {{ background-color: {review_surface}; border-color: {review_border}; }}
#app-window, #component-dialog-card, #zoom-control, .session-badge {{ background-color: {surface}; }}
#app-window {{ border-color: {preview_border}; }}
#left-tabs, #dock-tabs, #inspector-heading {{ border-color: {border}; }}
h1, #window-brand h1, #inspector-heading strong, .inspector-row strong, .metric-grid strong {{ color: {text_strong}; }}
.rail-kicker, .inspector-kicker, #component-summary, #dock-tabs, #code-inspector > p {{ color: {text_muted}; }}
#studio-save, #selection-tag, #save-annotation {{ background-color: {accent}; }}
#studio-save {{ border-color: {accent_hover}; color: {accent_text}; }}
#studio-save:hover {{ background-color: {accent_hover}; }}
.status-dot {{ background-color: {success}; }}
#project-canvas {{ background-color: {preview}; border-color: {preview_border}; }}
#annotation-heading, #annotation-heading strong {{ color: {review_text}; }}
#console-lines, #project-path, #css-excerpt {{ font-family: "{mono}", monospace; }}
"#,
            name = self.selection.name,
            mode = self.selection.mode,
            background = t.background,
            chrome = t.chrome,
            shell = t.shell,
            panel = t.panel,
            stage = t.stage,
            surface = t.surface,
            recessed = t.recessed,
            border = t.border,
            border_strong = t.border_strong,
            text = t.text,
            text_strong = t.text_strong,
            text_muted = t.text_muted,
            accent = t.accent,
            accent_hover = t.accent_hover,
            accent_text = t.accent_text,
            success = t.success,
            preview = t.preview,
            preview_border = t.preview_border,
            review_surface = t.review_surface,
            review_border = t.review_border,
            review_text = t.review_text,
            font = t.font_family,
            mono = t.mono_family,
        )
    }
}

/// Loaded and precedence-merged theme definitions.
#[derive(Clone, Debug)]
pub struct ThemeCatalog {
    definitions: BTreeMap<String, ThemeDefinition>,
}

impl ThemeCatalog {
    /// Load bundled, user, then project themes; later locations override earlier definitions.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe paths, oversized files, malformed TOML, or invalid tokens.
    pub fn load(locations: &[ThemeLocation]) -> Result<Self, ThemeError> {
        let mut definitions = BTreeMap::new();
        for (name, source) in BUNDLED_THEMES {
            let definition = parse_definition(Path::new(name), source)?;
            merge_catalog_definition(&mut definitions, definition);
        }
        let mut file_count = 0_usize;
        for location in locations {
            let path = location.path();
            if !path.exists() {
                continue;
            }
            let metadata = fs::symlink_metadata(path).map_err(|source| ThemeError::Io {
                operation: "inspect theme directory",
                path: path.to_owned(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(ThemeError::UnsafePath(path.to_owned()));
            }
            let mut entries = fs::read_dir(path)
                .map_err(|source| ThemeError::Io {
                    operation: "read theme directory",
                    path: path.to_owned(),
                    source,
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| ThemeError::Io {
                    operation: "read theme entry",
                    path: path.to_owned(),
                    source,
                })?;
            entries.sort_by_key(fs::DirEntry::path);
            for entry in entries {
                let file = entry.path();
                if file.extension().and_then(|extension| extension.to_str()) != Some("toml") {
                    continue;
                }
                file_count = file_count.saturating_add(1);
                if file_count > MAX_THEME_FILES {
                    return Err(ThemeError::TooManyFiles);
                }
                let metadata = fs::symlink_metadata(&file).map_err(|source| ThemeError::Io {
                    operation: "inspect theme file",
                    path: file.clone(),
                    source,
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(ThemeError::UnsafePath(file));
                }
                if metadata.len() > MAX_THEME_BYTES {
                    return Err(ThemeError::TooLarge {
                        path: file,
                        found: metadata.len(),
                        maximum: MAX_THEME_BYTES,
                    });
                }
                let source = fs::read_to_string(&file).map_err(|source| ThemeError::Io {
                    operation: "read theme file",
                    path: file.clone(),
                    source,
                })?;
                let definition = parse_definition(&file, &source)?;
                merge_catalog_definition(&mut definitions, definition);
            }
        }
        if definitions.is_empty() {
            return Err(ThemeError::EmptyCatalog);
        }
        Ok(Self { definitions })
    }

    /// All authored light and dark variants in stable display order.
    #[must_use]
    pub fn available(&self) -> Vec<AvailableTheme> {
        self.definitions
            .values()
            .flat_map(|definition| {
                [ThemeMode::Light, ThemeMode::Dark]
                    .into_iter()
                    .filter(|mode| definition.has_mode(*mode))
                    .map(|mode| AvailableTheme {
                        name: definition.name.clone(),
                        mode,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Resolve a selection, falling back to the first available bundled variant.
    #[must_use]
    pub fn resolve(&self, selection: &ThemeSelection) -> ResolvedTheme {
        let requested_key = theme_key(&selection.name);
        let selected = self
            .definitions
            .get(&requested_key)
            .filter(|definition| definition.has_mode(selection.mode))
            .map(|definition| (definition, selection.mode))
            .or_else(|| {
                self.available().first().and_then(|available| {
                    self.definitions
                        .get(&theme_key(&available.name))
                        .map(|definition| (definition, available.mode))
                })
            });
        let Some((definition, mode)) = selected else {
            return ResolvedTheme {
                selection: ThemeSelection::default(),
                tokens: ThemeTokens::defaults(ThemeMode::Dark),
            };
        };
        ResolvedTheme {
            selection: ThemeSelection {
                name: definition.name.clone(),
                mode,
            },
            tokens: ThemeTokens::from_definition(definition, mode),
        }
    }

    /// Select the next available theme variant, wrapping at the end.
    #[must_use]
    pub fn next(&self, selection: &ThemeSelection) -> ThemeSelection {
        let available = self.available();
        if available.is_empty() {
            return ThemeSelection::default();
        }
        let current = available.iter().position(|candidate| {
            theme_key(&candidate.name) == theme_key(&selection.name)
                && candidate.mode == selection.mode
        });
        available
            .get(current.map_or(0, |index| (index + 1) % available.len()))
            .map_or_else(ThemeSelection::default, AvailableTheme::selection)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFont {
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    mono_family: Option<String>,
}

impl ThemeFont {
    fn merge(&mut self, higher: &Self) {
        if higher.family.is_some() {
            self.family.clone_from(&higher.family);
        }
        if higher.mono_family.is_some() {
            self.mono_family.clone_from(&higher.mono_family);
        }
    }

    fn validate(&self) -> bool {
        [&self.family, &self.mono_family]
            .into_iter()
            .flatten()
            .all(|font| {
                !font.is_empty()
                    && font.len() <= 128
                    && font.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, ' ' | '-' | '_' | '.')
                    })
            })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeVariant {
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    font: ThemeFont,
}

impl ThemeVariant {
    fn is_authored(&self) -> bool {
        !self.colors.is_empty() || self.font.family.is_some() || self.font.mono_family.is_some()
    }

    fn merge(&mut self, higher: &Self) {
        self.colors.extend(higher.colors.clone());
        self.font.merge(&higher.font);
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDefinition {
    name: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    font: ThemeFont,
    #[serde(default)]
    light: ThemeVariant,
    #[serde(default)]
    dark: ThemeVariant,
}

impl ThemeDefinition {
    fn variant(&self, mode: ThemeMode) -> &ThemeVariant {
        match mode {
            ThemeMode::Light => &self.light,
            ThemeMode::Dark => &self.dark,
        }
    }

    fn has_mode(&self, mode: ThemeMode) -> bool {
        self.variant(mode).is_authored()
            || (!self.colors.is_empty() && !self.light.is_authored() && !self.dark.is_authored())
    }

    fn merge(&mut self, higher: Self) {
        self.name = higher.name;
        self.colors.extend(higher.colors);
        self.font.merge(&higher.font);
        self.light.merge(&higher.light);
        self.dark.merge(&higher.dark);
    }

    fn validate(&self) -> Result<(), ThemeError> {
        if self.name.trim().is_empty() || self.name.len() > 128 || !self.font.validate() {
            return Err(ThemeError::Invalid("theme name or font is invalid"));
        }
        let colors = self
            .colors
            .values()
            .chain(self.light.colors.values())
            .chain(self.dark.colors.values());
        if !colors.into_iter().all(|color| valid_color(color))
            || !self.light.font.validate()
            || !self.dark.font.validate()
        {
            return Err(ThemeError::Invalid(
                "theme color or variant font is invalid",
            ));
        }
        if !self.has_mode(ThemeMode::Light) && !self.has_mode(ThemeMode::Dark) {
            return Err(ThemeError::Invalid("theme has no authored variant"));
        }
        Ok(())
    }
}

fn parse_definition(path: &Path, source: &str) -> Result<ThemeDefinition, ThemeError> {
    let definition =
        toml::from_str::<ThemeDefinition>(source).map_err(|source| ThemeError::Parse {
            path: path.to_owned(),
            source,
        })?;
    definition.validate()?;
    Ok(definition)
}

fn merge_catalog_definition(
    definitions: &mut BTreeMap<String, ThemeDefinition>,
    definition: ThemeDefinition,
) {
    let key = theme_key(&definition.name);
    if let Some(base) = definitions.get_mut(&key) {
        base.merge(definition);
    } else {
        definitions.insert(key, definition);
    }
}

fn theme_key(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn valid_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) struct ThemeWatcher {
    locations: Vec<ThemeLocation>,
    receiver: Receiver<notify::Result<Event>>,
    overflowed: Arc<AtomicBool>,
    _watcher: RecommendedWatcher,
}

impl ThemeWatcher {
    pub(crate) fn new(locations: Vec<ThemeLocation>) -> Result<Self, ThemeError> {
        let (sender, receiver) = sync_channel(THEME_EVENT_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let callback_overflowed = overflowed.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            if sender.try_send(event).is_err() {
                callback_overflowed.store(true, Ordering::Release);
            }
        })?;
        for location in &locations {
            fs::create_dir_all(location.path()).map_err(|source| ThemeError::Io {
                operation: "create theme directory",
                path: location.path().to_owned(),
                source,
            })?;
            watcher.watch(location.path(), RecursiveMode::NonRecursive)?;
        }
        Ok(Self {
            locations,
            receiver,
            overflowed,
            _watcher: watcher,
        })
    }

    pub(crate) fn poll(&self) -> Result<Option<ThemeCatalog>, ThemeError> {
        let mut changed = self.overflowed.swap(false, Ordering::AcqRel);
        loop {
            match self.receiver.try_recv() {
                Ok(Ok(event)) => {
                    changed |= event.paths.iter().any(|path| {
                        path.extension().and_then(|extension| extension.to_str()) == Some("toml")
                    });
                }
                Ok(Err(error)) => return Err(ThemeError::Watch(error)),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        changed
            .then(|| ThemeCatalog::load(&self.locations))
            .transpose()
    }
}

/// Invalid theme catalog, definition, or watch operation.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    /// Filesystem operation failed.
    #[error("{operation} `{}`", path.display())]
    Io {
        /// Operation being attempted.
        operation: &'static str,
        /// Affected path.
        path: PathBuf,
        /// Operating-system error.
        #[source]
        source: std::io::Error,
    },
    /// A theme path was a symlink or unexpected file type.
    #[error("unsafe theme path `{}`", .0.display())]
    UnsafePath(PathBuf),
    /// A theme file exceeded the bounded local input size.
    #[error("theme `{}` is {found} bytes; maximum is {maximum}", path.display())]
    TooLarge {
        /// Oversized theme path.
        path: PathBuf,
        /// Observed size.
        found: u64,
        /// Maximum size.
        maximum: u64,
    },
    /// Too many theme files were discovered.
    #[error("theme catalog exceeds the 64-file bound")]
    TooManyFiles,
    /// TOML parsing failed.
    #[error("parse theme `{}`", path.display())]
    Parse {
        /// Theme source path.
        path: PathBuf,
        /// TOML error.
        #[source]
        source: toml::de::Error,
    },
    /// Theme data violated a semantic invariant.
    #[error("invalid theme: {0}")]
    Invalid(&'static str),
    /// No theme survived loading and validation.
    #[error("theme catalog is empty")]
    EmptyCatalog,
    /// Filesystem watcher setup or delivery failed.
    #[error("theme watcher")]
    Watch(#[from] notify::Error),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{ThemeCatalog, ThemeLocation, ThemeMode, ThemeSelection};

    #[test]
    fn project_theme_overrides_user_and_bundled_tokens() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let user = root.path().join("user");
        let project = root.path().join("project");
        fs::create_dir_all(&user)?;
        fs::create_dir_all(&project)?;
        fs::write(
            user.join("foundry.toml"),
            "name='Foundry'\n[dark.colors]\naccent='#112233'\ntext='#445566'\n",
        )?;
        fs::write(
            project.join("foundry.toml"),
            "name='Foundry'\n[dark.colors]\naccent='#abcdef'\n",
        )?;

        let catalog =
            ThemeCatalog::load(&[ThemeLocation::User(user), ThemeLocation::Project(project)])?;
        let resolved = catalog.resolve(&ThemeSelection {
            name: "Foundry".to_owned(),
            mode: ThemeMode::Dark,
        });

        assert_eq!(resolved.tokens.accent, "#abcdef");
        assert_eq!(resolved.tokens.text, "#445566");
        Ok(())
    }

    #[test]
    fn catalog_cycles_only_authored_variants() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = ThemeCatalog::load(&[])?;
        let available = catalog.available();

        assert!(
            available
                .iter()
                .any(|theme| theme.name == "Foundry" && theme.mode == ThemeMode::Dark)
        );
        assert!(
            available
                .iter()
                .any(|theme| theme.name == "Paper" && theme.mode == ThemeMode::Light)
        );
        assert_ne!(
            catalog.next(&ThemeSelection::default()),
            ThemeSelection::default()
        );
        Ok(())
    }
}
