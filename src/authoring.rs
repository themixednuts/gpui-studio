use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, FontWeight, InteractiveElement as _,
    IntoElement as _, MouseButton, ParentElement as _, Render, SemanticElementExt as _,
    SemanticRole, SemanticText, SharedString, Window, div, ease_out_quint, prelude::*, px, rgb,
    rgba,
};
use gpui_mcp::Automation;
use serde::{Deserialize, Deserializer, Serialize};
use tempfile::NamedTempFile;

const COMPONENT_LIBRARY_VERSION: u16 = 8;
const LEGACY_COMPONENT_LIBRARY_VERSIONS: [u16; 7] = [1, 2, 3, 4, 5, 6, 7];
const MAX_COMPONENT_LIBRARY_BYTES: u64 = 512 * 1024;
const MAX_COMPONENTS: usize = 256;
const MAX_NODES_PER_COMPONENT: usize = 2_048;

/// Runtime callback shared by native component nodes and Studio state logic.
pub type ComponentActionHandler = Rc<dyn Fn(&str, &str, &str)>;

/// One pointer gesture on a rendered component node, forwarded to the editor
/// so mode-dependent behavior (select, run action, annotate, context menu)
/// lives in one place instead of inside the render tree.
#[derive(Clone, Debug)]
pub enum ComponentPointerGesture {
    /// Primary click; carries the node's resolved click action when one exists.
    Click {
        /// Resolved click action symbol, if the node has one.
        action: Option<String>,
    },
    /// Context-menu request at a window-relative position.
    ContextMenu {
        /// Window-relative x in logical pixels.
        x: f32,
        /// Window-relative y in logical pixels.
        y: f32,
    },
    /// Pointer is hovering this node (deepest node under the cursor).
    Hover,
    /// Primary-button press on the deepest node under the pointer. Select-on-
    /// press (design-tool convention) is immune to the release-time hit-test
    /// fall-through that otherwise collapses clicks to the root container.
    Select,
}

/// Editor callback receiving `(component_id, node_id, gesture)` for every
/// pointer gesture on a rendered component node. Returns whether the gesture
/// changed editor state and therefore needs a repaint, letting hover tracking
/// run on every mouse move without redrawing unchanged frames.
pub type ComponentPointerHandler = Rc<dyn Fn(&str, &str, ComponentPointerGesture) -> bool>;

/// Render-only drop-placement preview injected while a drag hovers the
/// canvas: a spacer at `(parent, index)` makes siblings reflow out of the way
/// exactly as they will after the drop commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropPreviewSpec {
    /// Stored-graph id of the container receiving the drop.
    pub parent: String,
    /// Visual-order insertion index within that container.
    pub index: usize,
    /// Whether the container flows horizontally (row-like) rather than
    /// vertically.
    pub horizontal: bool,
}

/// Reserved id for the render-only drop preview spacer node.
const DROP_PREVIEW_NODE_ID: &str = "__drop-preview";

/// Native drag payload emitted when the user grabs a rendered canvas node to
/// move it. The editor resolves the id (including instance-expansion prefixes)
/// and reuses the same drop machinery as tree and palette drags.
#[derive(Clone, Debug)]
pub struct ComponentNodeDrag {
    /// Component whose graph the node belongs to.
    pub component_id: String,
    /// Rendered node id (may be an instance-expansion prefixed id).
    pub node_id: String,
}

/// Compact pill shown under the cursor while dragging a canvas node.
struct NodeDragGhost {
    label: SharedString,
}

impl Render for NodeDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(rgb(0x1c_20_30))
            .border_1()
            .border_color(rgb(0x6e_7b_ff))
            .text_size(px(12.0))
            .text_color(rgb(0xe7_e9_ee))
            .child(self.label.clone())
    }
}

/// Editable source projection selected for the canonical component document.
///
/// This never selects a different runtime or component type: HTML/CSS and GPUI are two ways of
/// reading and writing the same typed component graph.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthoringBackend {
    /// Pure HTML/CSS plus typed RON bindings projection.
    #[default]
    Html,
    /// GPUI builder-chain projection.
    Gpui,
}

/// Built-in, offline component recipes installed into the same editable document graph as user
/// components. Presets contain complete structure, styling, props, semantics, and supported actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentPreset {
    /// Primary action button.
    Button,
    /// Segmented group of mutually selectable buttons.
    ButtonGroup,
    /// Content card with an action footer.
    Card,
    /// Compact status badge.
    Badge,
    /// Alert message with a dismiss action contract.
    Alert,
    /// Application command toolbar.
    Toolbar,
    /// Initials-based avatar.
    Avatar,
    /// Empty collection state with a recovery action.
    EmptyState,
    /// Semantic application titlebar.
    Titlebar,
    /// Stateful tab list and panel composition.
    Tabs,
    /// Accessible modal surface with close contract.
    Dialog,
    /// Select-style trigger with a listbox of options.
    Dropdown,
    /// Nested-menu-ready action collection.
    DropdownMenu,
    /// Dialog-like side sheet with a close contract.
    Drawer,
    /// Bounded semantic scroll region with editable content.
    Scrollable,
    /// Two-pane layout separated by a slider-like handle.
    Resizable,
    /// Contextual description linked to a trigger.
    Tooltip,
}

impl ComponentPreset {
    /// Stable preset order used by Studio's component catalog.
    pub const ALL: [Self; 17] = [
        Self::Button,
        Self::ButtonGroup,
        Self::Card,
        Self::Badge,
        Self::Alert,
        Self::Toolbar,
        Self::Avatar,
        Self::EmptyState,
        Self::Titlebar,
        Self::Tabs,
        Self::Dialog,
        Self::Dropdown,
        Self::DropdownMenu,
        Self::Drawer,
        Self::Scrollable,
        Self::Resizable,
        Self::Tooltip,
    ];

    /// Human-readable component name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Button => "Button",
            Self::ButtonGroup => "Button Group",
            Self::Card => "Card",
            Self::Badge => "Badge",
            Self::Alert => "Alert",
            Self::Toolbar => "Toolbar",
            Self::Avatar => "Avatar",
            Self::EmptyState => "Empty State",
            Self::Titlebar => "Titlebar",
            Self::Tabs => "Tabs",
            Self::Dialog => "Dialog",
            Self::Dropdown => "Dropdown",
            Self::DropdownMenu => "Dropdown Menu",
            Self::Drawer => "Drawer",
            Self::Scrollable => "Scrollable",
            Self::Resizable => "Resizable",
            Self::Tooltip => "Tooltip",
        }
    }

    /// Compact catalog description.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Button => "Accessible primary action",
            Self::ButtonGroup => "Stateful segmented action group",
            Self::Card => "Structured content and footer action",
            Self::Badge => "Compact semantic status",
            Self::Alert => "Message with dismiss action",
            Self::Toolbar => "Title and command group",
            Self::Avatar => "Initials identity primitive",
            Self::EmptyState => "Recovery-oriented empty state",
            Self::Titlebar => "Editable window chrome content",
            Self::Tabs => "Stateful tab list and panels",
            Self::Dialog => "Accessible modal action surface",
            Self::Dropdown => "Select trigger and option list",
            Self::DropdownMenu => "Menu-aim-ready command popup",
            Self::Drawer => "Dismissible dialog-like side sheet",
            Self::Scrollable => "Bounded semantic scroll container",
            Self::Resizable => "Adjustable two-pane composition",
            Self::Tooltip => "Accessible contextual description",
        }
    }
}

/// A persistent collection of canonical components with HTML and GPUI projections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeComponentLibrary {
    /// Versioned library schema.
    pub version: u16,
    /// Component selected for the live preview.
    pub active_component: String,
    /// Bounded component definitions.
    pub components: Vec<NativeComponent>,
    /// Project-local semantic design tokens available offline to every component.
    #[serde(default = "default_design_tokens")]
    pub tokens: Vec<DesignToken>,
}

impl Default for NativeComponentLibrary {
    fn default() -> Self {
        let component = NativeComponent::starter("component-1", "Root");
        Self {
            version: COMPONENT_LIBRARY_VERSION,
            active_component: component.id.clone(),
            components: vec![component],
            tokens: default_design_tokens(),
        }
    }
}

impl NativeComponentLibrary {
    /// Load a project-local library or return a portable starter library when absent.
    ///
    /// # Errors
    ///
    /// Returns an error for oversized, invalid, unsupported, or unsafe component data.
    pub fn load(project_root: &Path) -> Result<Self, NativeComponentError> {
        let path = library_path(project_root);
        let path = if path.exists() {
            path
        } else {
            legacy_library_path(project_root)
        };
        if !path.exists() {
            return Ok(Self::default());
        }
        let metadata = fs::metadata(&path).map_err(|source| NativeComponentError::Io {
            operation: "inspect component library",
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_COMPONENT_LIBRARY_BYTES {
            return Err(NativeComponentError::TooLarge {
                found: metadata.len(),
                maximum: MAX_COMPONENT_LIBRARY_BYTES,
            });
        }
        let source = fs::read_to_string(&path).map_err(|source| NativeComponentError::Io {
            operation: "read component library",
            path,
            source,
        })?;
        let mut library: Self = ron::from_str(&source)?;
        library.migrate_legacy_document()?;
        library.normalize_root_name();
        library.validate()?;
        Ok(library)
    }

    /// Return the selected component, falling back to the first valid definition.
    #[must_use]
    pub fn active(&self) -> Option<&NativeComponent> {
        self.components
            .iter()
            .find(|component| component.id == self.active_component)
            .or_else(|| self.components.first())
    }

    /// Look up one component definition by its stable id.
    #[must_use]
    pub fn component(&self, id: &str) -> Option<&NativeComponent> {
        self.components.iter().find(|component| component.id == id)
    }

    /// Whether inserting an instance of `candidate` into `host` would create a
    /// composition cycle (directly or transitively), which must be rejected.
    #[must_use]
    pub fn would_cycle(&self, host: &str, candidate: &str) -> bool {
        if host == candidate {
            return true;
        }
        // `candidate` cannot contain `host`, else host→candidate→…→host loops.
        let mut stack = vec![candidate.to_owned()];
        let mut seen = BTreeSet::new();
        while let Some(current) = stack.pop() {
            if current == host {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            if let Some(component) = self.component(&current) {
                component.root.collect_instance_refs(&mut stack);
            }
        }
        false
    }

    /// Component ids that place an instance of `id` somewhere in their graph.
    #[must_use]
    pub fn referencing_components(&self, id: &str) -> Vec<String> {
        let mut referencing = Vec::new();
        for component in &self.components {
            if component.id == id {
                continue;
            }
            let mut refs = Vec::new();
            component.root.collect_instance_refs(&mut refs);
            if refs.iter().any(|referenced| referenced == id)
                && !referencing.contains(&component.id)
            {
                referencing.push(component.id.clone());
            }
        }
        referencing
    }

    /// Remove a component definition. Fails for the root (first) component,
    /// the last remaining component, unknown ids, and components still
    /// referenced by other components' instances.
    ///
    /// # Errors
    ///
    /// Returns a user-readable message describing why the component could
    /// not be removed.
    pub fn remove_component(&mut self, id: &str) -> Result<(), String> {
        let Some(component) = self.component(id) else {
            return Err("component was not found".to_owned());
        };
        let name = component.name.clone();
        if self.components.first().is_some_and(|root| root.id == id) {
            return Err("the root component cannot be deleted".to_owned());
        }
        if self.components.len() <= 1 {
            return Err("at least one component must remain".to_owned());
        }
        let referencing = self.referencing_components(id);
        if !referencing.is_empty() {
            let names = referencing
                .iter()
                .map(|referencing_id| {
                    self.component(referencing_id).map_or_else(
                        || referencing_id.clone(),
                        |component| component.name.clone(),
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!("{name} is still used by: {names}"));
        }
        self.components.retain(|component| component.id != id);
        if self.active_component == id
            && let Some(first) = self.components.first()
        {
            self.active_component = first.id.clone();
        }
        Ok(())
    }

    fn next_component_number(&self) -> usize {
        let mut number = self.components.len().saturating_add(1);
        loop {
            let id = format!("component-{number}");
            if self.components.iter().all(|component| component.id != id) {
                return number;
            }
            number = number.checked_add(1).unwrap_or(1);
        }
    }

    /// Add and select a new editable starter component entirely at runtime.
    pub fn create_component(&mut self) -> &NativeComponent {
        let number = self.next_component_number();
        self.create_named_component(format!("Component {number}"))
    }

    /// Add and select a named editable starter component entirely at runtime.
    pub fn create_named_component(&mut self, name: impl Into<String>) -> &NativeComponent {
        self.create_named_component_with_props(name, Vec::new())
    }

    /// Add a canonical component with typed props; projection choice is deliberately absent.
    pub fn create_named_component_with_props(
        &mut self,
        name: impl Into<String>,
        props: Vec<ComponentProp>,
    ) -> &NativeComponent {
        let number = self.next_component_number();
        let id = format!("component-{number}");
        let mut component = NativeComponent::starter(&id, &name.into());
        component.props = props;
        self.components.push(component);
        self.active_component = id;
        // The vector is non-empty after the push.
        &self.components[self.components.len() - 1]
    }

    /// Add a platform-neutral semantic titlebar component to the same canonical graph.
    pub fn create_titlebar_component_with_props(
        &mut self,
        name: impl Into<String>,
        props: Vec<ComponentProp>,
    ) -> &NativeComponent {
        let number = self.next_component_number();
        let id = format!("component-{number}");
        let mut component = NativeComponent::titlebar(&id, &name.into());
        for prop in props {
            if let Some(existing) = component
                .props
                .iter_mut()
                .find(|existing| existing.name == prop.name)
            {
                *existing = prop;
            } else {
                component.props.push(prop);
            }
        }
        self.components.push(component);
        self.active_component = id;
        &self.components[self.components.len() - 1]
    }

    /// Install and select one complete built-in preset under a project-local name.
    pub fn create_preset_component(
        &mut self,
        preset: ComponentPreset,
        name: impl Into<String>,
    ) -> &NativeComponent {
        self.create_preset_component_with_props(preset, name, Vec::new())
    }

    /// Install a preset and merge additional typed props by name.
    pub fn create_preset_component_with_props(
        &mut self,
        preset: ComponentPreset,
        name: impl Into<String>,
        props: Vec<ComponentProp>,
    ) -> &NativeComponent {
        let number = self.next_component_number();
        let id = format!("component-{number}");
        let mut component = NativeComponent::preset(&id, &name.into(), preset);
        for prop in props {
            if let Some(existing) = component
                .props
                .iter_mut()
                .find(|existing| existing.name == prop.name)
            {
                *existing = prop;
            } else {
                component.props.push(prop);
            }
        }
        self.components.push(component);
        self.active_component = id;
        &self.components[self.components.len() - 1]
    }

    /// Persist the complete library through a flushed project-local staged file.
    ///
    /// # Errors
    ///
    /// Returns an error when validation, serialization, staging, or replacement fails.
    pub fn save(&self, project_root: &Path) -> Result<(), NativeComponentError> {
        self.validate()?;
        let directory = project_root.join(".gpui-studio");
        fs::create_dir_all(&directory).map_err(|source| NativeComponentError::Io {
            operation: "create component directory",
            path: directory.clone(),
            source,
        })?;
        let destination = library_path(project_root);
        let source = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;

        // Windows real-time antivirus and search indexers briefly lock a freshly
        // staged temp file or the destination during the atomic replace, which
        // surfaces as a transient sharing/permission error. The validated,
        // serialized bytes never change between attempts, so retry only the
        // filesystem operations with a short backoff before giving up.
        Self::with_transient_retry(|| {
            Self::stage_and_replace(&directory, &destination, source.as_bytes())
        })
    }

    /// Run a fallible filesystem operation, retrying it a bounded number of
    /// times with an escalating short backoff. Deterministic inputs (validation,
    /// serialization) must happen before this so every attempt is equivalent.
    fn with_transient_retry<T>(
        mut operation: impl FnMut() -> Result<T, NativeComponentError>,
    ) -> Result<T, NativeComponentError> {
        const RETRY_BACKOFF_MS: [u64; 4] = [2, 5, 12, 25];
        let mut attempt = 0;
        loop {
            match operation() {
                Ok(value) => return Ok(value),
                Err(error) => {
                    let Some(&backoff) = RETRY_BACKOFF_MS.get(attempt) else {
                        return Err(error);
                    };
                    std::thread::sleep(std::time::Duration::from_millis(backoff));
                    attempt += 1;
                }
            }
        }
    }

    /// Stage the serialized library to a sibling temp file and atomically
    /// replace the destination. Separated from [`Self::save`] so the caller can
    /// retry the filesystem operations on a transient lock without re-validating
    /// or re-serializing.
    fn stage_and_replace(
        directory: &Path,
        destination: &Path,
        bytes: &[u8],
    ) -> Result<(), NativeComponentError> {
        let mut temporary =
            NamedTempFile::new_in(directory).map_err(|source| NativeComponentError::Io {
                operation: "stage component library",
                path: directory.to_owned(),
                source,
            })?;
        temporary
            .write_all(bytes)
            .and_then(|()| temporary.flush())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|source| NativeComponentError::Io {
                operation: "flush component library",
                path: temporary.path().to_owned(),
                source,
            })?;
        temporary
            .persist(destination)
            .map_err(|source| NativeComponentError::Persist {
                path: destination.to_owned(),
                source,
            })?;
        Ok(())
    }

    fn validate(&self) -> Result<(), NativeComponentError> {
        if self.version != COMPONENT_LIBRARY_VERSION {
            return Err(NativeComponentError::UnsupportedVersion {
                found: self.version,
                supported: COMPONENT_LIBRARY_VERSION,
            });
        }
        if self.components.is_empty() || self.components.len() > MAX_COMPONENTS {
            return Err(NativeComponentError::Invalid(
                "component count is outside the supported bound",
            ));
        }
        let mut identifiers = std::collections::BTreeSet::new();
        for component in &self.components {
            validate_identifier(&component.id)?;
            if !identifiers.insert(component.id.as_str()) {
                return Err(NativeComponentError::Invalid(
                    "component identifiers must be unique",
                ));
            }
            if component.name.is_empty() || component.name.len() > 256 {
                return Err(NativeComponentError::Invalid("component name is invalid"));
            }
            let mut node_count = 0;
            let mut node_ids = BTreeSet::new();
            component.root.validate(&mut node_count, &mut node_ids)?;
            let mut prop_names = BTreeSet::new();
            for prop in &component.props {
                validate_identifier(&prop.name)?;
                if !prop_names.insert(prop.name.as_str())
                    || prop.value_type.is_empty()
                    || prop.value_type.len() > 128
                    || prop
                        .default
                        .as_ref()
                        .is_some_and(|value| value.len() > 16 * 1024)
                {
                    return Err(NativeComponentError::Invalid(
                        "component property schema is invalid",
                    ));
                }
            }
            let mut logic_ids = std::collections::BTreeSet::new();
            let mut state_names = std::collections::BTreeSet::new();
            for state in &component.states {
                validate_identifier(&state.name)?;
                if !state_names.insert(state.name.as_str())
                    || state.value_type.is_empty()
                    || state.value_type.len() > 128
                    || state.default.len() > 16 * 1024
                {
                    return Err(NativeComponentError::Invalid(
                        "component state schema is invalid",
                    ));
                }
            }
            for logic in &component.logic {
                validate_identifier(&logic.id)?;
                validate_identifier(&logic.source_node)?;
                validate_identifier(&logic.action)?;
                if !logic_ids.insert(logic.id.as_str())
                    || !component.root.contains_node(&logic.source_node)
                    || logic.guard.as_deref().is_some_and(|guard| {
                        guard.len() > 4_096
                            || component_logic_guard_parts(guard)
                                .is_none_or(|(state, _, _)| !state_names.contains(state))
                    })
                    || logic.target_state.is_some() != logic.value.is_some()
                    || logic
                        .target_state
                        .as_ref()
                        .is_some_and(|state| !state_names.contains(state.as_str()))
                    || logic
                        .value
                        .as_ref()
                        .is_some_and(|value| value.len() > 16 * 1024)
                {
                    return Err(NativeComponentError::Invalid(
                        "component logic graph is invalid",
                    ));
                }
            }
            let mut variant_ids = std::collections::BTreeSet::new();
            for variant in &component.variants {
                validate_identifier(&variant.id)?;
                if variant.name.is_empty()
                    || variant.name.len() > 128
                    || !variant_ids.insert(variant.id.as_str())
                    || variant.overrides.len() > MAX_NODES_PER_COMPONENT
                    || variant.overrides.iter().any(|overrides| {
                        !component.root.contains_node(&overrides.node_id)
                            || overrides
                                .text
                                .as_ref()
                                .and_then(Option::as_ref)
                                .is_some_and(|text| text.len() > 16 * 1024)
                    })
                {
                    return Err(NativeComponentError::Invalid(
                        "component variant schema is invalid",
                    ));
                }
            }
            let mut slot_names = std::collections::BTreeSet::new();
            for slot in &component.slots {
                validate_identifier(&slot.name)?;
                validate_identifier(&slot.node_id)?;
                if !slot_names.insert(slot.name.as_str())
                    || !component.root.contains_node(&slot.node_id)
                    || slot.accepted_kinds.len() > 32
                {
                    return Err(NativeComponentError::Invalid(
                        "component slot schema is invalid",
                    ));
                }
            }
        }
        let mut token_paths = std::collections::BTreeSet::new();
        if self.tokens.len() > 2_048 {
            return Err(NativeComponentError::Invalid(
                "design token count is invalid",
            ));
        }
        for token in &self.tokens {
            validate_identifier(&token.path)?;
            if !token_paths.insert(token.path.as_str())
                || token.value.is_empty()
                || token.value.len() > 4_096
                || token.description.len() > 4_096
            {
                return Err(NativeComponentError::Invalid(
                    "design token schema is invalid",
                ));
            }
        }
        if !identifiers.contains(self.active_component.as_str()) {
            return Err(NativeComponentError::Invalid(
                "active component does not exist",
            ));
        }
        Ok(())
    }

    fn migrate_legacy_document(&mut self) -> Result<(), NativeComponentError> {
        if self.version != COMPONENT_LIBRARY_VERSION
            && !LEGACY_COMPONENT_LIBRARY_VERSIONS.contains(&self.version)
        {
            return Err(NativeComponentError::UnsupportedVersion {
                found: self.version,
                supported: COMPONENT_LIBRARY_VERSION,
            });
        }
        for component in &mut self.components {
            component.root.migrate_legacy_backgrounds();
            component.normalize_legacy_vocabulary();
        }
        self.version = COMPONENT_LIBRARY_VERSION;
        Ok(())
    }

    /// Rename the root (first) component from its default "Component N"
    /// starter name to "Root" once, leaving a user-chosen name untouched.
    fn normalize_root_name(&mut self) {
        if let Some(root) = self.components.first_mut()
            && is_default_component_name(&root.name)
        {
            root.name = "Root".to_owned();
        }
    }
}

/// Whether `name` is the default "Component N" starter name assigned to a
/// newly created component, eligible for the root's one-time rename to
/// "Root".
fn is_default_component_name(name: &str) -> bool {
    name.strip_prefix("Component ").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

/// One user-authored component that maps directly to GPUI elements.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeComponent {
    /// Stable component identifier.
    pub id: String,
    /// Human-readable component name.
    pub name: String,
    /// Root of the typed GPUI element tree.
    pub root: NativeNode,
    /// Typed public component properties shared by every projection.
    #[serde(default)]
    pub props: Vec<ComponentProp>,
    /// State/event/action graph shared by every projection.
    #[serde(default)]
    pub logic: Vec<ComponentLogic>,
    /// Typed local state used by logic edges and variants.
    #[serde(default)]
    pub states: Vec<ComponentState>,
    /// Named visual variants over the canonical base graph.
    #[serde(default)]
    pub variants: Vec<ComponentVariant>,
    /// Explicit insertion points for composed children.
    #[serde(default)]
    pub slots: Vec<ComponentSlot>,
}

impl NativeComponent {
    fn starter(id: &str, name: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            root: NativeNode {
                id: "root".to_owned(),
                kind: NativeNodeKind::Column,
                semantic_role: None,
                state: NativeSemanticState::default(),
                layout: NativeLayout {
                    width: NativeSize::Fill,
                    height: NativeSize::Fill,
                    gap: 14,
                    padding: 28,
                    align: NativeAlign::Start,
                    justify: NativeAlign::Center,
                    ..NativeLayout::default()
                },
                appearance: NativeAppearance {
                    background: Some(0xe5_e3_d8),
                    foreground: 0x24_26_1f,
                    border: None,
                    radius: 0,
                },
                typography: NativeTypography::default(),
                text: None,
                action: None,
                instance_of: None,
                children: vec![
                    NativeNode::text("eyebrow", "UNIFIED COMPONENT DOCUMENT", 0x9b_68_26),
                    NativeNode::text("title", name, 0x1d_1f_1a),
                    NativeNode::text(
                        "description",
                        "Edit the same component as HTML/CSS or GPUI; both projections stay in sync.",
                        0x62_65_5b,
                    ),
                    NativeNode {
                        id: "action".to_owned(),
                        kind: NativeNodeKind::Button,
                        semantic_role: None,
                        state: NativeSemanticState::default(),
                        layout: NativeLayout {
                            width: NativeSize::Hug,
                            height: NativeSize::Fixed(36),
                            gap: 0,
                            padding: 10,
                            align: NativeAlign::Center,
                            justify: NativeAlign::Center,
                            ..NativeLayout::default()
                        },
                        appearance: NativeAppearance {
                            background: Some(0xc9_8a_36),
                            foreground: 0x21_1b_12,
                            border: Some(0xe3_aa_54),
                            radius: 4,
                        },
                        typography: NativeTypography {
                            weight: 600,
                            ..NativeTypography::default()
                        },
                        text: Some("Component action".to_owned()),
                        action: Some("component_action".to_owned()),
                        instance_of: None,
                        children: Vec::new(),
                    },
                ],
            },
            props: Vec::new(),
            logic: Vec::new(),
            states: Vec::new(),
            variants: Vec::new(),
            slots: Vec::new(),
        }
    }

    fn titlebar(id: &str, name: &str) -> Self {
        let action = |id: &str, text: &str, action: &str| NativeNode {
            id: id.to_owned(),
            kind: NativeNodeKind::Button,
            semantic_role: None,
            state: NativeSemanticState::default(),
            layout: NativeLayout {
                width: NativeSize::Fixed(28),
                height: NativeSize::Fixed(28),
                gap: 0,
                padding: 0,
                align: NativeAlign::Center,
                justify: NativeAlign::Center,
                ..NativeLayout::default()
            },
            appearance: NativeAppearance {
                background: Some(0x27_2a_33),
                foreground: 0xd7_db_e3,
                border: Some(0x38_3c_47),
                radius: 6,
            },
            typography: NativeTypography::default(),
            text: Some(text.to_owned()),
            action: Some(action.to_owned()),
            instance_of: None,
            children: Vec::new(),
        };
        let mut title = NativeNode::text("window-title", name, 0xe7_e9_ee);
        title.layout.width = NativeSize::Fill;
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            root: NativeNode {
                id: "root".to_owned(),
                kind: NativeNodeKind::Titlebar,
                semantic_role: None,
                state: NativeSemanticState::default(),
                layout: NativeLayout {
                    width: NativeSize::Fill,
                    height: NativeSize::Fixed(44),
                    gap: 8,
                    padding: 8,
                    align: NativeAlign::Center,
                    justify: NativeAlign::Start,
                    ..NativeLayout::default()
                },
                appearance: NativeAppearance {
                    background: Some(0x15_17_1d),
                    foreground: 0xe7_e9_ee,
                    border: Some(0x29_2d_36),
                    radius: 0,
                },
                typography: NativeTypography::default(),
                text: None,
                action: None,
                instance_of: None,
                children: vec![
                    title,
                    action("window-minimize", "−", "window_minimize"),
                    action("window-maximize", "□", "window_maximize"),
                    action("window-close", "×", "window_close"),
                ],
            },
            props: vec![ComponentProp {
                name: "title".to_owned(),
                value_type: "String".to_owned(),
                default: Some(format!("{name:?}")),
            }],
            logic: Vec::new(),
            states: Vec::new(),
            variants: Vec::new(),
            slots: Vec::new(),
        }
    }

    fn preset(id: &str, name: &str, preset: ComponentPreset) -> Self {
        let stage = |child: NativeNode| {
            let mut root = NativeNode::container("root", NativeNodeKind::Column);
            root.layout.width = NativeSize::Fill;
            root.layout.height = NativeSize::Fill;
            root.layout.align = NativeAlign::Center;
            root.layout.justify = NativeAlign::Center;
            root.layout.padding = 24;
            root.appearance.background = Some(0x0f_11_16_u32);
            root.children.push(child);
            root
        };
        let prop = |name: &str, value_type: &str, default: &str| ComponentProp {
            name: name.to_owned(),
            value_type: value_type.to_owned(),
            default: Some(default.to_owned()),
        };

        let (root, props) = match preset {
            ComponentPreset::Button => {
                let mut button = NativeNode::button("button", "Continue", "press");
                button.layout.height = NativeSize::Fixed(40);
                button.layout.padding = 16;
                button.appearance.background = Some(0x6e_7b_ff);
                button.appearance.foreground = 0x0b_0d_17;
                button.appearance.border = Some(0x83_8e_ff);
                button.appearance.radius = 8;
                button.typography.weight = 650;
                (stage(button), vec![prop("label", "String", "\"Continue\"")])
            }
            ComponentPreset::ButtonGroup => {
                let mut group = NativeNode::container("button-group", NativeNodeKind::Row);
                group.semantic_role = Some(NativeSemanticRole::Group);
                group.layout.height = NativeSize::Fixed(40);
                group.layout.padding = 3;
                group.layout.align = NativeAlign::Center;
                group.appearance.background = Some(0x15_18_20);
                group.appearance.border = Some(0x2f_35_42);
                group.appearance.radius = 9;
                for (node_id, label, action) in [
                    ("segment-left", "Left", "select_left"),
                    ("segment-center", "Center", "select_center"),
                    ("segment-right", "Right", "select_right"),
                ] {
                    let selected = node_id == "segment-center";
                    let mut segment = NativeNode::button(node_id, label, action);
                    segment.state.selected = Some(selected);
                    segment.layout.width = NativeSize::Fixed(92);
                    segment.layout.height = NativeSize::Fixed(32);
                    segment.layout.padding = 10;
                    segment.appearance.background = selected.then_some(0x2d_34_43);
                    segment.appearance.foreground = if selected { 0xf0_f2_f7 } else { 0x9a_a2_b1 };
                    segment.appearance.radius = 6;
                    segment.typography.size = 12;
                    segment.typography.weight = 600;
                    group.children.push(segment);
                }
                (stage(group), vec![prop("selected", "String", "\"center\"")])
            }
            ComponentPreset::Badge => {
                let mut badge = NativeNode::container("badge", NativeNodeKind::Row);
                badge.layout.height = NativeSize::Fixed(26);
                badge.layout.padding = 9;
                badge.layout.align = NativeAlign::Center;
                badge.appearance.background = Some(0x1c_24_3d);
                badge.appearance.foreground = 0xa9_b3_ff;
                badge.appearance.border = Some(0x32_3d_68);
                badge.appearance.radius = 13;
                let mut label = NativeNode::text("label", "Stable", 0xa9_b3_ff);
                label.typography.size = 12;
                label.typography.line_height = 16;
                label.typography.weight = 600;
                badge.children.push(label);
                (stage(badge), vec![prop("label", "String", "\"Stable\"")])
            }
            ComponentPreset::Avatar => {
                let mut avatar = NativeNode::container("avatar", NativeNodeKind::Row);
                avatar.layout.width = NativeSize::Fixed(44);
                avatar.layout.height = NativeSize::Fixed(44);
                avatar.layout.align = NativeAlign::Center;
                avatar.layout.justify = NativeAlign::Center;
                avatar.appearance.background = Some(0x29_2f_52);
                avatar.appearance.foreground = 0xc8_ce_ff;
                avatar.appearance.border = Some(0x41_49_75);
                avatar.appearance.radius = 22;
                let mut initials = NativeNode::text("initials", "JS", 0xc8_ce_ff);
                initials.typography.weight = 650;
                avatar.children.push(initials);
                (stage(avatar), vec![prop("initials", "String", "\"JS\"")])
            }
            ComponentPreset::Card => {
                let mut card = NativeNode::container("card", NativeNodeKind::Column);
                card.layout.width = NativeSize::Fixed(360);
                card.layout.gap = 12;
                card.layout.padding = 20;
                card.appearance.background = Some(0x17_1a_23);
                card.appearance.foreground = 0xe7_e9_ee;
                card.appearance.border = Some(0x2c_31_3d);
                card.appearance.radius = 12;
                let mut title = NativeNode::text("title", "Project ready", 0xf0_f2_f7);
                title.typography.size = 18;
                title.typography.line_height = 24;
                title.typography.weight = 650;
                let mut body = NativeNode::text(
                    "description",
                    "Your component graph is live and ready to edit.",
                    0x8f_96_a5,
                );
                body.typography.line_height = 21;
                let mut action = NativeNode::button("action", "Open project", "open_project");
                action.layout.height = NativeSize::Fixed(36);
                action.layout.padding = 12;
                action.appearance.background = Some(0x6e_7b_ff);
                action.appearance.foreground = 0x0b_0d_17;
                action.appearance.radius = 7;
                action.typography.weight = 600;
                card.children.extend([title, body, action]);
                (
                    stage(card),
                    vec![
                        prop("title", "String", "\"Project ready\""),
                        prop(
                            "description",
                            "String",
                            "\"Your component graph is live and ready to edit.\"",
                        ),
                    ],
                )
            }
            ComponentPreset::Alert => {
                let mut alert = NativeNode::container("alert", NativeNodeKind::Row);
                alert.layout.width = NativeSize::Fixed(420);
                alert.layout.gap = 12;
                alert.layout.padding = 14;
                alert.layout.align = NativeAlign::Center;
                alert.appearance.background = Some(0x26_20_15);
                alert.appearance.foreground = 0xf2_d4_9b;
                alert.appearance.border = Some(0x5d_47_25);
                alert.appearance.radius = 9;
                let mut copy = NativeNode::container("copy", NativeNodeKind::Column);
                copy.layout.width = NativeSize::Fill;
                copy.layout.gap = 2;
                let mut title = NativeNode::text("title", "Unsaved changes", 0xff_e0_a4);
                title.typography.weight = 650;
                let mut message = NativeNode::text(
                    "message",
                    "Export when you are ready to persist the project.",
                    0xc4_a9_76,
                );
                message.typography.size = 12;
                message.typography.line_height = 17;
                copy.children.extend([title, message]);
                let mut dismiss = NativeNode::button("dismiss", "×", "dismiss");
                dismiss.layout.width = NativeSize::Fixed(28);
                dismiss.layout.height = NativeSize::Fixed(28);
                dismiss.appearance.background = Some(0x35_2c_1e);
                dismiss.appearance.foreground = 0xf2_d4_9b;
                dismiss.appearance.radius = 6;
                alert.children.extend([copy, dismiss]);
                (
                    stage(alert),
                    vec![prop("message", "String", "\"Unsaved changes\"")],
                )
            }
            ComponentPreset::Toolbar => {
                let mut toolbar = NativeNode::container("toolbar", NativeNodeKind::Row);
                toolbar.layout.width = NativeSize::Fixed(620);
                toolbar.layout.height = NativeSize::Fixed(48);
                toolbar.layout.gap = 8;
                toolbar.layout.padding = 8;
                toolbar.layout.align = NativeAlign::Center;
                toolbar.appearance.background = Some(0x15_18_20);
                toolbar.appearance.foreground = 0xe7_e9_ee;
                toolbar.appearance.border = Some(0x2a_2f_3a);
                toolbar.appearance.radius = 10;
                let mut title = NativeNode::text("title", "Document", 0xe7_e9_ee);
                title.layout.width = NativeSize::Fill;
                title.typography.weight = 600;
                toolbar.children.push(title);
                for (node_id, label, action) in [
                    ("undo", "Undo", "undo"),
                    ("redo", "Redo", "redo"),
                    ("save", "Save", "save"),
                ] {
                    let mut command = NativeNode::button(node_id, label, action);
                    command.layout.height = NativeSize::Fixed(32);
                    command.layout.padding = 10;
                    command.appearance.background = Some(0x22_26_31);
                    command.appearance.foreground = 0xc5_ca_d5;
                    command.appearance.border = Some(0x33_38_45);
                    command.appearance.radius = 6;
                    command.typography.size = 12;
                    toolbar.children.push(command);
                }
                (
                    stage(toolbar),
                    vec![prop("title", "String", "\"Document\"")],
                )
            }
            ComponentPreset::EmptyState => {
                let mut empty = NativeNode::container("empty-state", NativeNodeKind::Column);
                empty.layout.width = NativeSize::Fixed(420);
                empty.layout.gap = 10;
                empty.layout.padding = 28;
                empty.layout.align = NativeAlign::Center;
                empty.layout.justify = NativeAlign::Center;
                empty.appearance.background = Some(0x15_18_20);
                empty.appearance.foreground = 0xe7_e9_ee;
                empty.appearance.border = Some(0x2a_2f_3a);
                empty.appearance.radius = 12;
                let mut glyph = NativeNode::text("glyph", "◇", 0x7f_89_ff);
                glyph.typography.size = 28;
                glyph.typography.line_height = 32;
                let mut title = NativeNode::text("title", "No components yet", 0xf0_f2_f7);
                title.typography.size = 18;
                title.typography.line_height = 24;
                title.typography.weight = 650;
                let mut copy = NativeNode::text(
                    "description",
                    "Add a preset or create a component from your selection.",
                    0x8f_96_a5,
                );
                copy.typography.line_height = 21;
                let mut action = NativeNode::button("action", "Add component", "add_component");
                action.layout.height = NativeSize::Fixed(36);
                action.layout.padding = 12;
                action.appearance.background = Some(0x6e_7b_ff);
                action.appearance.foreground = 0x0b_0d_17;
                action.appearance.radius = 7;
                action.typography.weight = 600;
                empty.children.extend([glyph, title, copy, action]);
                (stage(empty), Vec::new())
            }
            ComponentPreset::Tabs => {
                let mut tabs = NativeNode::container("tabs", NativeNodeKind::Column);
                tabs.layout.width = NativeSize::Fixed(480);
                tabs.layout.gap = 12;
                let mut list = NativeNode::container("tab-list", NativeNodeKind::Row);
                list.semantic_role = Some(NativeSemanticRole::TabList);
                list.layout.gap = 4;
                list.layout.padding = 4;
                list.appearance.background = Some(0x15_18_20);
                list.appearance.border = Some(0x2a_2f_3a);
                list.appearance.radius = 9;
                for (node_id, label, action) in [
                    ("tab-overview", "Overview", "select_overview"),
                    ("tab-activity", "Activity", "select_activity"),
                    ("tab-settings", "Settings", "select_settings"),
                ] {
                    let mut tab = NativeNode::button(node_id, label, action);
                    tab.semantic_role = Some(NativeSemanticRole::Tab);
                    tab.state.selected = Some(node_id == "tab-overview");
                    tab.layout.height = NativeSize::Fixed(34);
                    tab.layout.padding = 12;
                    tab.appearance.background = (node_id == "tab-overview").then_some(0x2a_2f_3c);
                    tab.appearance.foreground = 0xe7_e9_ee;
                    tab.appearance.radius = 6;
                    list.children.push(tab);
                }
                let mut panel = NativeNode::container("tab-panel", NativeNodeKind::Column);
                panel.semantic_role = Some(NativeSemanticRole::TabPanel);
                panel.layout.padding = 18;
                panel.layout.gap = 6;
                panel.appearance.background = Some(0x17_1a_23);
                panel.appearance.border = Some(0x2c_31_3d);
                panel.appearance.radius = 10;
                panel
                    .children
                    .push(NativeNode::text("panel-title", "Overview", 0xf0_f2_f7));
                panel.children.push(NativeNode::text(
                    "panel-copy",
                    "Edit tab state, panels, and actions from one component graph.",
                    0x8f_96_a5,
                ));
                tabs.children.extend([list, panel]);
                (
                    stage(tabs),
                    vec![prop("selected", "String", "\"overview\"")],
                )
            }
            ComponentPreset::Dialog => {
                let mut dialog = NativeNode::container("dialog", NativeNodeKind::Column);
                dialog.semantic_role = Some(NativeSemanticRole::Dialog);
                dialog.layout.width = NativeSize::Fixed(420);
                dialog.layout.gap = 12;
                dialog.layout.padding = 20;
                dialog.appearance.background = Some(0x17_1a_23);
                dialog.appearance.foreground = 0xe7_e9_ee;
                dialog.appearance.border = Some(0x35_3b_49);
                dialog.appearance.radius = 12;
                let mut title = NativeNode::text("title", "Confirm export", 0xf0_f2_f7);
                title.typography.size = 18;
                title.typography.weight = 650;
                let body = NativeNode::text(
                    "body",
                    "This writes the current canonical project to disk.",
                    0x9a_a1_af,
                );
                let mut actions = NativeNode::container("actions", NativeNodeKind::Row);
                actions.layout.gap = 8;
                actions.layout.justify = NativeAlign::End;
                actions
                    .children
                    .push(NativeNode::button("cancel", "Cancel", "close"));
                let mut confirm = NativeNode::button("confirm", "Export", "confirm");
                confirm.appearance.background = Some(0x6e_7b_ff);
                confirm.appearance.foreground = 0x0b_0d_17;
                confirm.appearance.radius = 7;
                actions.children.push(confirm);
                dialog.children.extend([title, body, actions]);
                (
                    stage(dialog),
                    vec![
                        prop("open", "bool", "true"),
                        prop("title", "String", "\"Confirm export\""),
                    ],
                )
            }
            ComponentPreset::Dropdown => {
                let mut dropdown = NativeNode::container("dropdown", NativeNodeKind::Column);
                dropdown.layout.width = NativeSize::Fixed(320);
                dropdown.layout.gap = 6;

                let mut trigger =
                    NativeNode::button("dropdown-trigger", "Production        ▾", "toggle_options");
                trigger.state.expanded = Some(false);
                trigger.layout.width = NativeSize::Fill;
                trigger.layout.height = NativeSize::Fixed(42);
                trigger.layout.padding = 12;
                trigger.layout.justify = NativeAlign::Start;
                trigger.appearance.background = Some(0x17_1a_23);
                trigger.appearance.foreground = 0xe7_e9_ee;
                trigger.appearance.border = Some(0x35_3b_49);
                trigger.appearance.radius = 8;
                trigger.typography.weight = 600;

                let mut options = NativeNode::container("option-list", NativeNodeKind::Column);
                options.semantic_role = Some(NativeSemanticRole::Listbox);
                options.layout.width = NativeSize::Fill;
                options.layout.height = NativeSize::Fixed(0);
                options.layout.overflow = NativeOverflow::Hidden;
                options.layout.opacity_percent = 0;
                options.appearance.background = Some(0x17_1a_23);
                options.appearance.border = Some(0x2c_31_3d);
                options.appearance.radius = 9;
                for (node_id, label, action) in [
                    ("option-production", "Production", "select_production"),
                    ("option-staging", "Staging", "select_staging"),
                    ("option-development", "Development", "select_development"),
                ] {
                    let selected = node_id == "option-production";
                    let mut option = NativeNode::button(node_id, label, action);
                    option.semantic_role = Some(NativeSemanticRole::Option);
                    option.state.selected = Some(selected);
                    option.layout.width = NativeSize::Fill;
                    option.layout.height = NativeSize::Fixed(36);
                    option.layout.padding = 10;
                    option.layout.justify = NativeAlign::Start;
                    option.appearance.background = selected.then_some(0x28_2f_3d);
                    option.appearance.foreground = if selected { 0xf0_f2_f7 } else { 0xa6_ad_ba };
                    option.appearance.radius = 6;
                    options.children.push(option);
                }
                dropdown.children.extend([trigger, options]);
                (
                    stage(dropdown),
                    vec![
                        prop("open", "bool", "false"),
                        prop("selected", "String", "\"production\""),
                    ],
                )
            }
            ComponentPreset::DropdownMenu => {
                let mut menu = NativeNode::container("menu", NativeNodeKind::Column);
                menu.semantic_role = Some(NativeSemanticRole::Menu);
                menu.layout.width = NativeSize::Fixed(220);
                menu.layout.gap = 2;
                menu.layout.padding = 5;
                menu.appearance.background = Some(0x17_1a_23);
                menu.appearance.foreground = 0xe7_e9_ee;
                menu.appearance.border = Some(0x2c_31_3d);
                menu.appearance.radius = 10;
                for (node_id, label, action) in [
                    ("duplicate", "Duplicate", "duplicate"),
                    ("rename", "Rename", "rename"),
                    ("delete", "Delete", "delete"),
                ] {
                    let mut item = NativeNode::button(node_id, label, action);
                    item.semantic_role = Some(NativeSemanticRole::MenuItem);
                    item.layout.height = NativeSize::Fixed(34);
                    item.layout.padding = 9;
                    item.layout.justify = NativeAlign::Start;
                    item.appearance.foreground = if node_id == "delete" {
                        0xff_9a_a8
                    } else {
                        0xc5_ca_d4
                    };
                    item.appearance.radius = 6;
                    menu.children.push(item);
                }
                (stage(menu), vec![prop("open", "bool", "true")])
            }
            ComponentPreset::Drawer => {
                let mut shell = NativeNode::container("drawer-shell", NativeNodeKind::Stack);
                shell.layout.width = NativeSize::Fixed(680);
                shell.layout.height = NativeSize::Fixed(420);
                shell.layout.overflow = NativeOverflow::Hidden;
                shell.appearance.background = Some(0x08_0a_0f);
                shell.appearance.border = Some(0x2a_2f_3a);
                shell.appearance.radius = 12;

                let mut scrim = NativeNode::container("drawer-scrim", NativeNodeKind::Column);
                scrim.layout.width = NativeSize::Fill;
                scrim.layout.height = NativeSize::Fill;
                scrim.layout.position = NativePosition::Absolute;
                scrim.layout.offsets = NativeOffsets {
                    top: Some(0),
                    right: Some(0),
                    bottom: Some(0),
                    left: Some(0),
                };
                scrim.layout.opacity_percent = 72;
                scrim.appearance.background = Some(0x05_06_09);

                let mut drawer = NativeNode::container("drawer", NativeNodeKind::Column);
                drawer.semantic_role = Some(NativeSemanticRole::Dialog);
                drawer.layout.width = NativeSize::Fixed(360);
                drawer.layout.height = NativeSize::Fill;
                drawer.layout.gap = 16;
                drawer.layout.padding = 20;
                drawer.layout.position = NativePosition::Absolute;
                drawer.layout.offsets = NativeOffsets {
                    top: Some(0),
                    right: Some(0),
                    bottom: Some(0),
                    left: None,
                };
                drawer.layout.z_index = 1;
                drawer.appearance.background = Some(0x17_1a_23);
                drawer.appearance.foreground = 0xe7_e9_ee;
                drawer.appearance.border = Some(0x35_3b_49);

                let mut header = NativeNode::container("drawer-header", NativeNodeKind::Row);
                header.layout.width = NativeSize::Fill;
                header.layout.align = NativeAlign::Center;
                header.layout.gap = 12;
                let mut title = NativeNode::text("drawer-title", "Inspector details", 0xf0_f2_f7);
                title.layout.width = NativeSize::Fill;
                title.typography.size = 18;
                title.typography.weight = 650;
                let mut close = NativeNode::button("drawer-close", "×", "close_drawer");
                close.layout.width = NativeSize::Fixed(32);
                close.layout.height = NativeSize::Fixed(32);
                close.appearance.background = Some(0x22_26_31);
                close.appearance.foreground = 0xc9_cf_da;
                close.appearance.radius = 7;
                header.children.extend([title, close]);

                let mut content = NativeNode::container("drawer-content", NativeNodeKind::Column);
                content.layout.width = NativeSize::Fill;
                content.layout.gap = 10;
                content.children.push(NativeNode::text(
                    "drawer-copy",
                    "Use a drawer for focused supporting work without leaving the current canvas.",
                    0x9a_a1_af,
                ));
                content.children.push(NativeNode::text(
                    "drawer-meta",
                    "Position, content, and close behavior remain editable in one graph.",
                    0x72_7a_89,
                ));
                drawer.children.extend([header, content]);
                shell.children.extend([scrim, drawer]);
                (
                    stage(shell),
                    vec![
                        prop("open", "bool", "true"),
                        prop("title", "String", "\"Inspector details\""),
                    ],
                )
            }
            ComponentPreset::Scrollable => {
                let mut frame = NativeNode::container("scrollable", NativeNodeKind::Column);
                frame.layout.width = NativeSize::Fixed(440);
                frame.layout.height = NativeSize::Fixed(320);
                frame.layout.gap = 12;
                frame.layout.padding = 16;
                frame.appearance.background = Some(0x15_18_20);
                frame.appearance.foreground = 0xe7_e9_ee;
                frame.appearance.border = Some(0x2f_35_42);
                frame.appearance.radius = 12;
                let mut title = NativeNode::text("scroll-title", "Recent activity", 0xf0_f2_f7);
                title.typography.size = 17;
                title.typography.weight = 650;
                let mut viewport = NativeNode::container("scroll-viewport", NativeNodeKind::Column);
                viewport.semantic_role = Some(NativeSemanticRole::ScrollArea);
                viewport.layout.width = NativeSize::Fill;
                viewport.layout.height = NativeSize::Fill;
                viewport.layout.gap = 8;
                viewport.layout.padding = 8;
                viewport.layout.overflow = NativeOverflow::Scroll;
                viewport.appearance.background = Some(0x0f_11_16_u32);
                viewport.appearance.border = Some(0x29_2e_39);
                viewport.appearance.radius = 8;
                for (index, label) in [
                    "Component published",
                    "Design tokens synced",
                    "Preview refreshed",
                    "Variant applied",
                    "Export completed",
                    "Review comment resolved",
                    "Workspace saved",
                ]
                .into_iter()
                .enumerate()
                {
                    let mut row = NativeNode::container(
                        &format!("scroll-row-{}", index + 1),
                        NativeNodeKind::Row,
                    );
                    row.layout.width = NativeSize::Fill;
                    row.layout.height = NativeSize::Fixed(44);
                    row.layout.padding = 10;
                    row.layout.align = NativeAlign::Center;
                    row.layout.shrink = false;
                    row.appearance.background = Some(0x19_1d_26);
                    row.appearance.border = Some(0x29_2e_39);
                    row.appearance.radius = 7;
                    row.children.push(NativeNode::text(
                        &format!("scroll-label-{}", index + 1),
                        label,
                        0xb5_bc_c8,
                    ));
                    viewport.children.push(row);
                }
                frame.children.extend([title, viewport]);
                (
                    stage(frame),
                    vec![prop("title", "String", "\"Recent activity\"")],
                )
            }
            ComponentPreset::Resizable => {
                let mut resizable = NativeNode::container("resizable", NativeNodeKind::Row);
                resizable.semantic_role = Some(NativeSemanticRole::Group);
                resizable.layout.width = NativeSize::Fixed(600);
                resizable.layout.height = NativeSize::Fixed(320);
                resizable.layout.align = NativeAlign::Center;
                resizable.layout.overflow = NativeOverflow::Hidden;
                resizable.appearance.background = Some(0x0f_11_16_u32);
                resizable.appearance.border = Some(0x2f_35_42);
                resizable.appearance.radius = 12;

                let mut primary = NativeNode::container("primary-pane", NativeNodeKind::Column);
                primary.layout.width = NativeSize::Fixed(294);
                primary.layout.height = NativeSize::Fill;
                primary.layout.gap = 8;
                primary.layout.padding = 20;
                primary.appearance.background = Some(0x17_1a_23);
                let mut primary_title =
                    NativeNode::text("primary-title", "Primary pane", 0xf0_f2_f7);
                primary_title.typography.size = 17;
                primary_title.typography.weight = 650;
                primary.children.extend([
                    primary_title,
                    NativeNode::text(
                        "primary-copy",
                        "Main content adapts as the split changes.",
                        0x91_99_a8,
                    ),
                ]);

                let mut handle = NativeNode::button("resize-handle", "⋮", "cycle_split");
                handle.semantic_role = Some(NativeSemanticRole::Slider);
                handle.layout.width = NativeSize::Fixed(12);
                handle.layout.height = NativeSize::Fill;
                handle.layout.shrink = false;
                handle.appearance.background = Some(0x2b_31_3d);
                handle.appearance.foreground = 0x8d_96_a7;

                let mut secondary = NativeNode::container("secondary-pane", NativeNodeKind::Column);
                secondary.layout.width = NativeSize::Fixed(294);
                secondary.layout.height = NativeSize::Fill;
                secondary.layout.gap = 8;
                secondary.layout.padding = 20;
                secondary.appearance.background = Some(0x14_17_1f);
                let mut secondary_title =
                    NativeNode::text("secondary-title", "Secondary pane", 0xe2_e5_ec);
                secondary_title.typography.size = 17;
                secondary_title.typography.weight = 650;
                secondary.children.extend([
                    secondary_title,
                    NativeNode::text(
                        "secondary-copy",
                        "Supporting content keeps a bounded minimum width.",
                        0x84_8c_9b,
                    ),
                ]);
                resizable.children.extend([primary, handle, secondary]);
                (stage(resizable), vec![prop("split", "u16", "50")])
            }
            ComponentPreset::Tooltip => {
                let mut tooltip =
                    NativeNode::text("tooltip", "Changes are saved locally", 0xe7_e9_ee);
                tooltip.semantic_role = Some(NativeSemanticRole::Tooltip);
                tooltip.layout.padding = 8;
                tooltip.appearance.background = Some(0x22_26_31);
                tooltip.appearance.border = Some(0x3a_40_4e);
                tooltip.appearance.radius = 7;
                tooltip.typography.size = 12;
                tooltip.typography.line_height = 17;
                (
                    stage(tooltip),
                    vec![prop("content", "String", "\"Changes are saved locally\"")],
                )
            }
            ComponentPreset::Titlebar => return Self::titlebar(id, name),
        };
        let mut component = Self {
            id: id.to_owned(),
            name: name.to_owned(),
            root,
            props,
            logic: Vec::new(),
            states: Vec::new(),
            variants: Vec::new(),
            slots: Vec::new(),
        };
        match preset {
            ComponentPreset::Alert => {
                if let Some(alert) = component.root.find_mut("alert") {
                    alert.semantic_role = Some(NativeSemanticRole::Alert);
                }
            }
            ComponentPreset::Toolbar => {
                if let Some(toolbar) = component.root.find_mut("toolbar") {
                    toolbar.semantic_role = Some(NativeSemanticRole::Toolbar);
                }
            }
            ComponentPreset::ButtonGroup => {
                component.states.push(ComponentState {
                    name: "selected".to_owned(),
                    value_type: "String".to_owned(),
                    default: "center".to_owned(),
                });
                for (id, source, value) in [
                    ("select-left", "segment-left", "left"),
                    ("select-center", "segment-center", "center"),
                    ("select-right", "segment-right", "right"),
                ] {
                    component.logic.push(ComponentLogic {
                        id: id.to_owned(),
                        source_node: source.to_owned(),
                        event: ComponentEvent::Click,
                        action: format!("select_{value}"),
                        guard: None,
                        target_state: Some("selected".to_owned()),
                        value: Some(value.to_owned()),
                    });
                }
                component.slots.push(ComponentSlot {
                    name: "segments".to_owned(),
                    node_id: "button-group".to_owned(),
                    multiple: true,
                    accepted_kinds: vec![NativeNodeKind::Button],
                });
                let inactive = NativeAppearance {
                    background: None,
                    foreground: 0x9a_a2_b1,
                    border: None,
                    radius: 6,
                };
                let active = NativeAppearance {
                    background: Some(0x2d_34_43),
                    foreground: 0xf0_f2_f7,
                    ..inactive
                };
                for (value, active_node, inactive_nodes, label) in [
                    (
                        "left",
                        "segment-left",
                        ["segment-center", "segment-right"],
                        "Left selected",
                    ),
                    (
                        "right",
                        "segment-right",
                        ["segment-left", "segment-center"],
                        "Right selected",
                    ),
                ] {
                    let mut overrides = inactive_nodes
                        .into_iter()
                        .map(|node_id| ComponentVariantOverride {
                            node_id: node_id.to_owned(),
                            appearance: Some(inactive),
                            state: Some(NativeSemanticState {
                                selected: Some(false),
                                ..NativeSemanticState::default()
                            }),
                            ..ComponentVariantOverride::default()
                        })
                        .collect::<Vec<_>>();
                    overrides.push(ComponentVariantOverride {
                        node_id: active_node.to_owned(),
                        appearance: Some(active),
                        state: Some(NativeSemanticState {
                            selected: Some(true),
                            ..NativeSemanticState::default()
                        }),
                        ..ComponentVariantOverride::default()
                    });
                    component.variants.push(ComponentVariant {
                        id: format!("selected-{value}"),
                        name: label.to_owned(),
                        overrides,
                    });
                }
            }
            ComponentPreset::Tabs => {
                component.states.push(ComponentState {
                    name: "selected".to_owned(),
                    value_type: "String".to_owned(),
                    default: "overview".to_owned(),
                });
                for (id, source, value) in [
                    ("select-overview", "tab-overview", "overview"),
                    ("select-activity", "tab-activity", "activity"),
                    ("select-settings", "tab-settings", "settings"),
                ] {
                    component.logic.push(ComponentLogic {
                        id: id.to_owned(),
                        source_node: source.to_owned(),
                        event: ComponentEvent::Click,
                        action: format!("select_{value}"),
                        guard: None,
                        target_state: Some("selected".to_owned()),
                        value: Some(value.to_owned()),
                    });
                }
                component.slots.push(ComponentSlot {
                    name: "panel".to_owned(),
                    node_id: "tab-panel".to_owned(),
                    multiple: true,
                    accepted_kinds: Vec::new(),
                });
                let inactive = NativeAppearance {
                    background: None,
                    foreground: 0xe7_e9_ee,
                    border: None,
                    radius: 6,
                };
                let active = NativeAppearance {
                    background: Some(0x2a_2f_3c),
                    ..inactive
                };
                for (value, active_node, title, copy) in [
                    (
                        "activity",
                        "tab-activity",
                        "Activity",
                        "Recent component events and state transitions.",
                    ),
                    (
                        "settings",
                        "tab-settings",
                        "Settings",
                        "Adjust component behavior without changing its backing projection.",
                    ),
                ] {
                    component.variants.push(ComponentVariant {
                        id: format!("selected-{value}"),
                        name: title.to_owned(),
                        overrides: vec![
                            ComponentVariantOverride {
                                node_id: "tab-overview".to_owned(),
                                appearance: Some(inactive),
                                state: Some(NativeSemanticState {
                                    selected: Some(false),
                                    ..NativeSemanticState::default()
                                }),
                                ..ComponentVariantOverride::default()
                            },
                            ComponentVariantOverride {
                                node_id: active_node.to_owned(),
                                appearance: Some(active),
                                state: Some(NativeSemanticState {
                                    selected: Some(true),
                                    ..NativeSemanticState::default()
                                }),
                                ..ComponentVariantOverride::default()
                            },
                            ComponentVariantOverride {
                                node_id: "panel-title".to_owned(),
                                text: Some(Some(title.to_owned())),
                                ..ComponentVariantOverride::default()
                            },
                            ComponentVariantOverride {
                                node_id: "panel-copy".to_owned(),
                                text: Some(Some(copy.to_owned())),
                                ..ComponentVariantOverride::default()
                            },
                        ],
                    });
                }
            }
            ComponentPreset::Dialog => {
                component.states.push(ComponentState {
                    name: "open".to_owned(),
                    value_type: "bool".to_owned(),
                    default: "true".to_owned(),
                });
                component.logic.push(ComponentLogic {
                    id: "close-dialog".to_owned(),
                    source_node: "cancel".to_owned(),
                    event: ComponentEvent::Click,
                    action: "close".to_owned(),
                    guard: None,
                    target_state: Some("open".to_owned()),
                    value: Some("false".to_owned()),
                });
                component.slots.push(ComponentSlot {
                    name: "content".to_owned(),
                    node_id: "dialog".to_owned(),
                    multiple: true,
                    accepted_kinds: Vec::new(),
                });
            }
            ComponentPreset::Dropdown => {
                component.states.extend([
                    ComponentState {
                        name: "open".to_owned(),
                        value_type: "bool".to_owned(),
                        default: "false".to_owned(),
                    },
                    ComponentState {
                        name: "selected".to_owned(),
                        value_type: "String".to_owned(),
                        default: "production".to_owned(),
                    },
                ]);
                for (id, guard, value) in [
                    ("open-options", "open == false", "true"),
                    ("close-options", "open == true", "false"),
                ] {
                    component.logic.push(ComponentLogic {
                        id: id.to_owned(),
                        source_node: "dropdown-trigger".to_owned(),
                        event: ComponentEvent::Click,
                        action: "toggle_options".to_owned(),
                        guard: Some(guard.to_owned()),
                        target_state: Some("open".to_owned()),
                        value: Some(value.to_owned()),
                    });
                }
                for (source, value) in [
                    ("option-production", "production"),
                    ("option-staging", "staging"),
                    ("option-development", "development"),
                ] {
                    let action = format!("select_{value}");
                    component.logic.extend([
                        ComponentLogic {
                            id: format!("select-{value}"),
                            source_node: source.to_owned(),
                            event: ComponentEvent::Click,
                            action: action.clone(),
                            guard: None,
                            target_state: Some("selected".to_owned()),
                            value: Some(value.to_owned()),
                        },
                        ComponentLogic {
                            id: format!("close-after-{value}"),
                            source_node: source.to_owned(),
                            event: ComponentEvent::Click,
                            action,
                            guard: None,
                            target_state: Some("open".to_owned()),
                            value: Some("false".to_owned()),
                        },
                    ]);
                }
                component.slots.push(ComponentSlot {
                    name: "options".to_owned(),
                    node_id: "option-list".to_owned(),
                    multiple: true,
                    accepted_kinds: vec![NativeNodeKind::Button],
                });
                let closed_options = NativeLayout {
                    width: NativeSize::Fill,
                    height: NativeSize::Fixed(0),
                    overflow: NativeOverflow::Hidden,
                    opacity_percent: 0,
                    ..NativeLayout::default()
                };
                let open_options = NativeLayout {
                    width: NativeSize::Fill,
                    height: NativeSize::Hug,
                    gap: 2,
                    padding: 5,
                    overflow: NativeOverflow::Hidden,
                    ..NativeLayout::default()
                };
                for (value, label, layout, expanded) in [
                    ("false", "Closed", closed_options, false),
                    ("true", "Open", open_options, true),
                ] {
                    component.variants.push(ComponentVariant {
                        id: format!("open-{value}"),
                        name: label.to_owned(),
                        overrides: vec![
                            ComponentVariantOverride {
                                node_id: "dropdown-trigger".to_owned(),
                                state: Some(NativeSemanticState {
                                    expanded: Some(expanded),
                                    ..NativeSemanticState::default()
                                }),
                                ..ComponentVariantOverride::default()
                            },
                            ComponentVariantOverride {
                                node_id: "option-list".to_owned(),
                                layout: Some(layout),
                                ..ComponentVariantOverride::default()
                            },
                        ],
                    });
                }
                let inactive = NativeAppearance {
                    background: None,
                    foreground: 0xa6_ad_ba,
                    border: None,
                    radius: 6,
                };
                let active = NativeAppearance {
                    background: Some(0x28_2f_3d),
                    foreground: 0xf0_f2_f7,
                    ..inactive
                };
                for (value, active_node, inactive_nodes, trigger_text, label) in [
                    (
                        "staging",
                        "option-staging",
                        ["option-production", "option-development"],
                        "Staging           ▾",
                        "Staging selected",
                    ),
                    (
                        "development",
                        "option-development",
                        ["option-production", "option-staging"],
                        "Development       ▾",
                        "Development selected",
                    ),
                ] {
                    let mut overrides = inactive_nodes
                        .into_iter()
                        .map(|node_id| ComponentVariantOverride {
                            node_id: node_id.to_owned(),
                            appearance: Some(inactive),
                            state: Some(NativeSemanticState {
                                selected: Some(false),
                                ..NativeSemanticState::default()
                            }),
                            ..ComponentVariantOverride::default()
                        })
                        .collect::<Vec<_>>();
                    overrides.extend([
                        ComponentVariantOverride {
                            node_id: active_node.to_owned(),
                            appearance: Some(active),
                            state: Some(NativeSemanticState {
                                selected: Some(true),
                                ..NativeSemanticState::default()
                            }),
                            ..ComponentVariantOverride::default()
                        },
                        ComponentVariantOverride {
                            node_id: "dropdown-trigger".to_owned(),
                            text: Some(Some(trigger_text.to_owned())),
                            ..ComponentVariantOverride::default()
                        },
                    ]);
                    component.variants.push(ComponentVariant {
                        id: format!("selected-{value}"),
                        name: label.to_owned(),
                        overrides,
                    });
                }
            }
            ComponentPreset::DropdownMenu => {
                component.states.push(ComponentState {
                    name: "open".to_owned(),
                    value_type: "bool".to_owned(),
                    default: "true".to_owned(),
                });
            }
            ComponentPreset::Drawer => {
                component.states.extend([
                    ComponentState {
                        name: "open".to_owned(),
                        value_type: "bool".to_owned(),
                        default: "true".to_owned(),
                    },
                    ComponentState {
                        name: "side".to_owned(),
                        value_type: "String".to_owned(),
                        default: "right".to_owned(),
                    },
                ]);
                component.logic.push(ComponentLogic {
                    id: "close-drawer".to_owned(),
                    source_node: "drawer-close".to_owned(),
                    event: ComponentEvent::Click,
                    action: "close_drawer".to_owned(),
                    guard: None,
                    target_state: Some("open".to_owned()),
                    value: Some("false".to_owned()),
                });
                component.slots.push(ComponentSlot {
                    name: "content".to_owned(),
                    node_id: "drawer-content".to_owned(),
                    multiple: true,
                    accepted_kinds: Vec::new(),
                });
                component.variants.push(ComponentVariant {
                    id: "side-left".to_owned(),
                    name: "Left side".to_owned(),
                    overrides: vec![ComponentVariantOverride {
                        node_id: "drawer".to_owned(),
                        layout: Some(NativeLayout {
                            width: NativeSize::Fixed(360),
                            height: NativeSize::Fill,
                            gap: 16,
                            padding: 20,
                            position: NativePosition::Absolute,
                            offsets: NativeOffsets {
                                top: Some(0),
                                right: None,
                                bottom: Some(0),
                                left: Some(0),
                            },
                            z_index: 1,
                            ..NativeLayout::default()
                        }),
                        ..ComponentVariantOverride::default()
                    }],
                });
            }
            ComponentPreset::Scrollable => {
                component.states.push(ComponentState {
                    name: "density".to_owned(),
                    value_type: "String".to_owned(),
                    default: "comfortable".to_owned(),
                });
                component.variants.push(ComponentVariant {
                    id: "density-compact".to_owned(),
                    name: "Compact rows".to_owned(),
                    overrides: (1..=7)
                        .map(|index| ComponentVariantOverride {
                            node_id: format!("scroll-row-{index}"),
                            layout: Some(NativeLayout {
                                width: NativeSize::Fill,
                                height: NativeSize::Fixed(34),
                                padding: 8,
                                align: NativeAlign::Center,
                                shrink: false,
                                ..NativeLayout::default()
                            }),
                            ..ComponentVariantOverride::default()
                        })
                        .collect(),
                });
                component.slots.push(ComponentSlot {
                    name: "content".to_owned(),
                    node_id: "scroll-viewport".to_owned(),
                    multiple: true,
                    accepted_kinds: Vec::new(),
                });
            }
            ComponentPreset::Resizable => {
                component.states.push(ComponentState {
                    name: "split".to_owned(),
                    value_type: "u16".to_owned(),
                    default: "50".to_owned(),
                });
                for (id, guard, value) in [
                    ("split-to-35", "split == 50", "35"),
                    ("split-to-65", "split == 35", "65"),
                    ("split-to-50", "split == 65", "50"),
                ] {
                    component.logic.push(ComponentLogic {
                        id: id.to_owned(),
                        source_node: "resize-handle".to_owned(),
                        event: ComponentEvent::Click,
                        action: "cycle_split".to_owned(),
                        guard: Some(guard.to_owned()),
                        target_state: Some("split".to_owned()),
                        value: Some(value.to_owned()),
                    });
                }
                for (value, primary, secondary, label) in [
                    ("35", 204, 384, "Primary 35%"),
                    ("65", 384, 204, "Primary 65%"),
                ] {
                    component.variants.push(ComponentVariant {
                        id: format!("split-{value}"),
                        name: label.to_owned(),
                        overrides: vec![
                            ComponentVariantOverride {
                                node_id: "primary-pane".to_owned(),
                                layout: Some(NativeLayout {
                                    width: NativeSize::Fixed(primary),
                                    height: NativeSize::Fill,
                                    gap: 8,
                                    padding: 20,
                                    ..NativeLayout::default()
                                }),
                                ..ComponentVariantOverride::default()
                            },
                            ComponentVariantOverride {
                                node_id: "secondary-pane".to_owned(),
                                layout: Some(NativeLayout {
                                    width: NativeSize::Fixed(secondary),
                                    height: NativeSize::Fill,
                                    gap: 8,
                                    padding: 20,
                                    ..NativeLayout::default()
                                }),
                                ..ComponentVariantOverride::default()
                            },
                        ],
                    });
                }
                component.slots.extend([
                    ComponentSlot {
                        name: "primary".to_owned(),
                        node_id: "primary-pane".to_owned(),
                        multiple: true,
                        accepted_kinds: Vec::new(),
                    },
                    ComponentSlot {
                        name: "secondary".to_owned(),
                        node_id: "secondary-pane".to_owned(),
                        multiple: true,
                        accepted_kinds: Vec::new(),
                    },
                ]);
            }
            ComponentPreset::Button
            | ComponentPreset::Card
            | ComponentPreset::Badge
            | ComponentPreset::Avatar
            | ComponentPreset::EmptyState
            | ComponentPreset::Tooltip
            | ComponentPreset::Titlebar => {}
        }
        component
    }

    fn normalize_legacy_vocabulary(&mut self) {
        let previous_name = self.name.clone();
        if let Some(number) = self.name.strip_prefix("Native card ") {
            self.name = format!("Component {number}");
        }
        self.root
            .normalize_legacy_vocabulary(&previous_name, &self.name);
        if self.root.kind == NativeNodeKind::Titlebar
            && let Some(title) = self
                .root
                .children
                .iter_mut()
                .find(|child| child.id == "window-title")
            && title.layout.width == NativeSize::Hug
        {
            title.layout.width = NativeSize::Fill;
        }
        for logic in &mut self.logic {
            if logic.action == "native_action" {
                logic.action = "component_action".to_owned();
            }
        }
    }

    /// Render this definition directly as a native GPUI element tree.
    ///
    /// `library` resolves component instances into id-prefixed copies of the
    /// referenced component graphs before painting.
    #[must_use]
    pub fn render(&self, library: &NativeComponentLibrary, automation: &Automation) -> AnyElement {
        let actions = self.resolved_click_actions();
        let mut root = self.root.clone();
        self.expand_instances(&mut root, library);
        root.render(&self.id, automation, None, &actions, false, true)
    }

    /// Render the same canonical tree with live state/variant resolution and
    /// a pointer callback. Every node forwards clicks, hovers, and
    /// context-menu gestures; the editor decides what each gesture means for
    /// the active mode (select, run logic, annotate, open a menu). An active
    /// `drop_preview` injects a spacer so the layout previews the drop.
    #[must_use]
    pub fn render_interactive(
        &self,
        library: &NativeComponentLibrary,
        automation: &Automation,
        state: &BTreeMap<String, String>,
        handler: Option<ComponentPointerHandler>,
        drop_preview: Option<&DropPreviewSpec>,
        draggable: bool,
    ) -> AnyElement {
        if state.get("open").is_some_and(|value| value == "false")
            && self.root.has_semantic_role(NativeSemanticRole::Dialog)
        {
            return div().into_any_element();
        }
        let actions = self.resolved_click_actions();
        let mut root = self.resolved_root(state);
        self.expand_instances(&mut root, library);
        if let Some(preview) = drop_preview
            && let Some(parent) = root.find_mut(&preview.parent)
        {
            let index = preview.index.min(parent.children.len());
            parent
                .children
                .insert(index, NativeNode::drop_preview_spacer(preview.horizontal));
        }
        root.render(&self.id, automation, handler, &actions, draggable, true)
    }

    /// Replace every [`NativeNodeKind::Instance`] node in `root` with an
    /// id-prefixed copy of the referenced component graph. Missing references
    /// and composition cycles resolve to a visible placeholder instead of
    /// looping. This mutates a render-only clone; the stored graph is untouched.
    fn expand_instances(&self, root: &mut NativeNode, library: &NativeComponentLibrary) {
        let mut ancestry = vec![self.id.clone()];
        expand_instances_in(root, library, &mut ancestry);
    }

    fn resolved_root(&self, state: &BTreeMap<String, String>) -> NativeNode {
        let mut root = self.root.clone();
        for (name, value) in state {
            let qualified = format!("{name}-{value}");
            for variant in self
                .variants
                .iter()
                .filter(|variant| variant.id == *value || variant.id == qualified)
            {
                for override_value in &variant.overrides {
                    let Some(node) = root.find_mut(&override_value.node_id) else {
                        continue;
                    };
                    if let Some(layout) = override_value.layout {
                        node.layout = layout;
                    }
                    if let Some(appearance) = override_value.appearance {
                        node.appearance = appearance;
                    }
                    if let Some(typography) = &override_value.typography {
                        node.typography.clone_from(typography);
                    }
                    if let Some(text) = &override_value.text {
                        node.text.clone_from(text);
                    }
                    if let Some(state) = override_value.state {
                        node.state = state;
                    }
                }
            }
        }
        root
    }

    /// Find one node by its component-local stable identifier.
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&NativeNode> {
        self.root.find(id)
    }

    /// Find one editable node by its component-local stable identifier.
    #[must_use]
    pub fn node_mut(&mut self, id: &str) -> Option<&mut NativeNode> {
        self.root.find_mut(id)
    }

    /// Return the parent node id and child index of `id`, or `None` for the
    /// root (which has no parent) or an unknown id.
    #[must_use]
    pub fn parent_and_index(&self, id: &str) -> Option<(String, usize)> {
        self.root.parent_and_index(id)
    }

    /// Whether `descendant` lies inside the subtree rooted at `ancestor`.
    #[must_use]
    pub fn node_contains(&self, ancestor: &str, descendant: &str) -> bool {
        self.root
            .find(ancestor)
            .is_some_and(|node| node.find(descendant).is_some())
    }

    /// Whether a node with `id` may contain children in the stored graph.
    #[must_use]
    pub fn node_accepts_children(&self, id: &str) -> bool {
        self.node(id).is_some_and(|node| {
            !matches!(
                node.kind,
                NativeNodeKind::Text | NativeNodeKind::Button | NativeNodeKind::Instance
            )
        })
    }

    /// Allocate a component-local node id with `prefix` that is not already used.
    #[must_use]
    pub fn unique_node_id(&self, prefix: &str) -> String {
        for index in 1..=u32::MAX {
            let candidate = format!("{prefix}-{index}");
            if self.root.find(&candidate).is_none() {
                return candidate;
            }
        }
        format!("{prefix}-node")
    }

    /// Return a compact builder-chain view of the native tree used by the inspector.
    #[must_use]
    pub fn gpui_excerpt(&self) -> String {
        self.root
            .gpui_excerpt(&self.id, &self.resolved_click_actions())
    }

    /// Pure HTML projection of this component graph. No HTMLSwap-only attributes are emitted.
    #[must_use]
    pub fn html_projection(&self) -> String {
        let mut output = String::new();
        self.root.write_html(&mut output, 0);
        output
    }

    /// CSS projection of layout and appearance stored in the component graph.
    #[must_use]
    pub fn css_projection(&self) -> String {
        let mut output = String::new();
        self.root.write_css(&mut output);
        output
    }

    /// Typed RON event bindings projected from the component's logic graph.
    #[must_use]
    pub fn bindings_projection(&self) -> String {
        let mut bindings = Vec::new();
        let explicit_click_sources = self
            .logic
            .iter()
            .filter(|logic| logic.event == ComponentEvent::Click)
            .map(|logic| logic.source_node.clone())
            .collect::<BTreeSet<_>>();
        self.root
            .collect_bindings(&mut bindings, &explicit_click_sources);
        bindings.extend(self.logic.iter().map(|logic| {
            format!(
                "        Event(target: Id({:?}), event: {}, handler: {:?}),",
                logic.source_node,
                logic.event.ron_name(),
                logic.action
            )
        }));
        format!(
            "(\n    version: 1,\n    bindings: [\n{}\n    ],\n)",
            bindings.join("\n")
        )
    }

    fn resolved_click_actions(&self) -> BTreeMap<String, String> {
        self.logic
            .iter()
            .filter(|logic| logic.event == ComponentEvent::Click)
            .fold(BTreeMap::new(), |mut actions, logic| {
                actions
                    .entry(logic.source_node.clone())
                    .or_insert_with(|| logic.action.clone());
                actions
            })
    }
}

/// One typed component property in the canonical graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentProp {
    /// Stable property name.
    pub name: String,
    /// Portable source type such as `String` or `bool`.
    pub value_type: String,
    /// Optional serialized default value.
    pub default: Option<String>,
}

/// A semantic event supported by both HTML bindings and GPUI event handlers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ComponentEvent {
    /// Pointer or keyboard activation.
    Click,
    /// Text input mutation.
    Input,
    /// Committed value change.
    Change,
    /// Focus entered the node.
    Focus,
    /// Focus left the node.
    Blur,
}

impl ComponentEvent {
    const fn ron_name(self) -> &'static str {
        match self {
            Self::Click => "Click",
            Self::Input => "Input",
            Self::Change => "Change",
            Self::Focus => "Focus",
            Self::Blur => "Blur",
        }
    }
}

/// One event-to-action edge in a component's shared logic graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentLogic {
    /// Stable edge identifier.
    pub id: String,
    /// Component-local source node.
    pub source_node: String,
    /// Semantic event emitted by the source node.
    pub event: ComponentEvent,
    /// Registered action symbol invoked by the edge.
    pub action: String,
    /// Optional portable boolean guard expression.
    pub guard: Option<String>,
    /// Optional state name updated after the action succeeds.
    #[serde(default)]
    pub target_state: Option<String>,
    /// Optional portable serialized value assigned to `target_state`.
    #[serde(default)]
    pub value: Option<String>,
}

/// Typed local state exposed to the structured Logic editor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentState {
    /// Stable state name.
    pub name: String,
    /// Portable scalar or record type.
    pub value_type: String,
    /// Serialized initial value.
    pub default: String,
}

/// Named visual state over the component's canonical base tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentVariant {
    /// Stable variant ID such as `hover` or `size-large`.
    pub id: String,
    /// Human-readable variant label.
    pub name: String,
    /// Sparse ordered node overrides.
    pub overrides: Vec<ComponentVariantOverride>,
}

/// Sparse node values changed by a component variant.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentVariantOverride {
    /// Target component-local node ID.
    pub node_id: String,
    /// Optional layout replacement.
    pub layout: Option<NativeLayout>,
    /// Optional appearance replacement.
    pub appearance: Option<NativeAppearance>,
    /// Optional typography replacement.
    pub typography: Option<NativeTypography>,
    /// Optional text replacement, including explicit clearing.
    pub text: Option<Option<String>>,
    /// Optional accessibility-state replacement.
    pub state: Option<NativeSemanticState>,
}

/// Explicit insertion point for composed component content.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentSlot {
    /// Stable public slot name.
    pub name: String,
    /// Container node receiving slotted children.
    pub node_id: String,
    /// Whether more than one child is accepted.
    pub multiple: bool,
    /// Empty means any canonical node kind.
    #[serde(default)]
    pub accepted_kinds: Vec<NativeNodeKind>,
}

/// Project-local semantic design token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignToken {
    /// Dot-delimited semantic path such as `color.surface`.
    pub path: String,
    /// Typed token category.
    pub kind: DesignTokenKind,
    /// Portable CSS/RON-compatible value.
    pub value: String,
    /// Human-readable usage guidance.
    pub description: String,
}

/// Supported design token categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DesignTokenKind {
    /// CSS-compatible color.
    Color,
    /// Logical-pixel length or unitless number.
    Number,
    /// Font family, size, weight, or line-height token.
    Typography,
    /// General bounded string.
    String,
}

fn default_design_tokens() -> Vec<DesignToken> {
    [
        (
            "color.canvas",
            DesignTokenKind::Color,
            "#0f1116",
            "Application canvas",
        ),
        (
            "color.surface",
            DesignTokenKind::Color,
            "#171a23",
            "Primary surface",
        ),
        (
            "color.text",
            DesignTokenKind::Color,
            "#e7e9ee",
            "Primary text",
        ),
        (
            "color.muted",
            DesignTokenKind::Color,
            "#8f96a5",
            "Secondary text",
        ),
        (
            "color.accent",
            DesignTokenKind::Color,
            "#6e7bff",
            "Interactive accent",
        ),
        ("space.1", DesignTokenKind::Number, "4", "Compact spacing"),
        ("space.2", DesignTokenKind::Number, "8", "Control spacing"),
        ("space.3", DesignTokenKind::Number, "12", "Content spacing"),
        (
            "radius.control",
            DesignTokenKind::Number,
            "8",
            "Control radius",
        ),
        (
            "font.body",
            DesignTokenKind::Typography,
            "Geist",
            "Default UI family",
        ),
    ]
    .into_iter()
    .map(|(path, kind, value, description)| DesignToken {
        path: path.to_owned(),
        kind,
        value: value.to_owned(),
        description: description.to_owned(),
    })
    .collect()
}

/// A typed native GPUI element created and editable without recompiling Studio.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeNode {
    /// Stable component-local node identifier.
    pub id: String,
    /// GPUI element behavior.
    pub kind: NativeNodeKind,
    /// Optional stronger accessibility semantics independent of visual layout.
    #[serde(default)]
    pub semantic_role: Option<NativeSemanticRole>,
    /// Dynamic accessibility state authored independently of visual layout.
    #[serde(default)]
    pub state: NativeSemanticState,
    /// Native flex layout values.
    pub layout: NativeLayout,
    /// Native paint values.
    pub appearance: NativeAppearance,
    /// Editable typography shared by the HTML and GPUI projections.
    #[serde(default)]
    pub typography: NativeTypography,
    /// Optional text content.
    pub text: Option<String>,
    /// Optional application action symbol surfaced to MCP metadata.
    pub action: Option<String>,
    /// Referenced component id when [`NativeNode::kind`] is
    /// [`NativeNodeKind::Instance`]. Ignored for every other kind.
    #[serde(default)]
    pub instance_of: Option<String>,
    /// Ordered child nodes.
    pub children: Vec<NativeNode>,
}

impl NativeNode {
    fn container(id: &str, kind: NativeNodeKind) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            semantic_role: None,
            state: NativeSemanticState::default(),
            layout: NativeLayout::default(),
            appearance: NativeAppearance::default(),
            typography: NativeTypography::default(),
            text: None,
            action: None,
            instance_of: None,
            children: Vec::new(),
        }
    }

    /// Build a component-instance leaf referencing another component by id.
    fn instance(id: &str, component_id: &str) -> Self {
        let mut node = Self::container(id, NativeNodeKind::Instance);
        node.instance_of = Some(component_id.to_owned());
        node
    }

    /// Build a visible authored container (Row/Column/Grid/Stack/Titlebar) with
    /// sensible defaults for direct insertion from the editor.
    #[must_use]
    pub fn authored_container(id: &str, kind: NativeNodeKind) -> Self {
        let mut node = Self::container(id, kind);
        node.layout.width = NativeSize::Fixed(200);
        node.layout.height = NativeSize::Fixed(120);
        node.layout.gap = 8;
        node.layout.padding = 12;
        node.appearance.border = Some(0x3a_3f_4b);
        node.appearance.radius = 6;
        node
    }

    /// Build an authored text node with placeholder copy.
    #[must_use]
    pub fn authored_text(id: &str) -> Self {
        Self::text(id, "Text", 0xe7_e9_ee)
    }

    /// Build an authored button node with a placeholder label.
    #[must_use]
    pub fn authored_button(id: &str) -> Self {
        let mut node = Self::button(id, "Button", "component_action");
        node.layout.width = NativeSize::Hug;
        node.layout.height = NativeSize::Fixed(34);
        node.layout.padding = 12;
        node.appearance.background = Some(0xc9_8a_36);
        node.appearance.foreground = 0x21_1b_12;
        node.appearance.radius = 6;
        node.typography.weight = 600;
        node
    }

    /// Build a component-instance node referencing `component_id`.
    #[must_use]
    pub fn authored_instance(id: &str, component_id: &str) -> Self {
        Self::instance(id, component_id)
    }

    /// Render-only spacer previewing an imminent drop; sized along the
    /// container's main axis so siblings reflow exactly as they will after
    /// the drop commits.
    fn drop_preview_spacer(horizontal: bool) -> Self {
        let mut node = Self::container(DROP_PREVIEW_NODE_ID, NativeNodeKind::Column);
        if horizontal {
            node.layout.width = NativeSize::Fixed(48);
            node.layout.height = NativeSize::Fill;
            node.layout.min_height = Some(32);
        } else {
            node.layout.width = NativeSize::Fill;
            node.layout.height = NativeSize::Fixed(44);
        }
        node.layout.shrink = false;
        node
    }

    fn button(id: &str, text: &str, action: &str) -> Self {
        let mut node = Self::container(id, NativeNodeKind::Button);
        node.layout.align = NativeAlign::Center;
        node.layout.justify = NativeAlign::Center;
        node.text = Some(text.to_owned());
        node.action = Some(action.to_owned());
        node
    }

    fn text(id: &str, text: &str, foreground: u32) -> Self {
        Self {
            id: id.to_owned(),
            kind: NativeNodeKind::Text,
            semantic_role: None,
            state: NativeSemanticState::default(),
            layout: NativeLayout::default(),
            appearance: NativeAppearance {
                foreground,
                ..NativeAppearance::default()
            },
            typography: NativeTypography::default(),
            text: Some(text.to_owned()),
            action: None,
            instance_of: None,
            children: Vec::new(),
        }
    }

    fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }

    fn find_mut(&mut self, id: &str) -> Option<&mut Self> {
        if self.id == id {
            return Some(self);
        }
        self.children
            .iter_mut()
            .find_map(|child| child.find_mut(id))
    }

    fn has_semantic_role(&self, role: NativeSemanticRole) -> bool {
        self.semantic_role == Some(role)
            || self
                .children
                .iter()
                .any(|child| child.has_semantic_role(role))
    }

    fn parent_and_index(&self, id: &str) -> Option<(String, usize)> {
        for (index, child) in self.children.iter().enumerate() {
            if child.id == id {
                return Some((self.id.clone(), index));
            }
            if let Some(found) = child.parent_and_index(id) {
                return Some(found);
            }
        }
        None
    }

    /// Push the referenced component id of every instance node in this subtree.
    fn collect_instance_refs(&self, refs: &mut Vec<String>) {
        if self.kind == NativeNodeKind::Instance
            && let Some(referenced) = &self.instance_of
        {
            refs.push(referenced.clone());
        }
        for child in &self.children {
            child.collect_instance_refs(refs);
        }
    }

    /// Rewrite this subtree's ids with `prefix` so multiple instances of the
    /// same component keep unique runtime ids after expansion.
    fn prefix_ids(&mut self, prefix: &str) {
        self.id = format!("{prefix}{}", self.id);
        for child in &mut self.children {
            child.prefix_ids(prefix);
        }
    }

    /// Animated placeholder painted where a hovered drag will drop. Grows
    /// along the container's main axis so the surrounding layout shift eases
    /// in rather than snapping.
    fn render_drop_preview(layout: NativeLayout) -> AnyElement {
        let horizontal = matches!(layout.width, NativeSize::Fixed(_));
        let main = f32::from(
            match if horizontal {
                layout.width
            } else {
                layout.height
            } {
                NativeSize::Fixed(value) => value,
                NativeSize::Hug | NativeSize::Fill => 44,
            },
        );
        let base = div()
            .flex_none()
            .rounded(px(6.0))
            .bg(rgba(0x6e_7b_ff_2e))
            .border_1()
            .border_color(rgb(0x6e_7b_ff));
        let base = if horizontal {
            base.min_h(px(32.0)).h_full()
        } else {
            base.w_full()
        };
        base.with_animation(
            "drop-preview-grow",
            Animation::new(std::time::Duration::from_millis(140)).with_easing(ease_out_quint()),
            move |element, delta| {
                let size = px(main * delta);
                if horizontal {
                    element.w(size)
                } else {
                    element.h(size)
                }
            },
        )
        .into_any_element()
    }

    fn render(
        &self,
        component_id: &str,
        automation: &Automation,
        handler: Option<ComponentPointerHandler>,
        resolved_actions: &BTreeMap<String, String>,
        draggable: bool,
        is_root: bool,
    ) -> AnyElement {
        if self.id == DROP_PREVIEW_NODE_ID {
            return Self::render_drop_preview(self.layout);
        }
        let mut element = match self.kind {
            NativeNodeKind::Grid => div().grid(),
            NativeNodeKind::Stack => div().relative(),
            _ => div().flex(),
        }
        .min_w_0()
        .min_h_0();
        element = match self.kind {
            NativeNodeKind::Column => element.flex_col(),
            NativeNodeKind::Row | NativeNodeKind::Titlebar => element.flex_row(),
            NativeNodeKind::Grid
            | NativeNodeKind::Stack
            | NativeNodeKind::Text
            | NativeNodeKind::Button
            | NativeNodeKind::Instance => element,
        };
        element = apply_size(element, self.layout.width, true);
        element = apply_size(element, self.layout.height, false);
        element = element
            .gap(px(f32::from(self.layout.gap)))
            .p(px(f32::from(self.layout.padding)))
            .text_color(rgb(self.appearance.foreground))
            .font_family(self.typography.family.clone())
            .text_size(px(f32::from(self.typography.size)))
            .font_weight(FontWeight(f32::from(self.typography.weight)))
            .line_height(px(f32::from(self.typography.line_height)))
            .rounded(px(f32::from(self.appearance.radius)));
        if let Some(background) = self.appearance.background {
            element = element.bg(rgb(background));
        }
        if let Some(border) = self.appearance.border {
            element = element.border_1().border_color(rgb(border));
        }
        element = apply_alignment(element, self.layout.align, false);
        element = apply_alignment(element, self.layout.justify, true);
        element = apply_advanced_layout(element, self.layout);
        if let Some(text) = &self.text {
            element = element.child(text.clone());
        }
        element = self.children.iter().fold(element, |parent, child| {
            parent.child(child.render(
                component_id,
                automation,
                handler.clone(),
                resolved_actions,
                draggable,
                false,
            ))
        });

        let runtime_id = format!("component/{component_id}/{}", self.id);
        // A stable GPUI id gives the element the `element_state` GPUI requires to
        // process native pointer listeners — without it, click-to-select, hover,
        // right-click, and grab-to-move on canvas nodes silently do nothing (only
        // the semantic/MCP path fires).
        let mut element = element.id(SharedString::from(runtime_id.clone()));
        let role = self.semantic_role.map_or_else(
            || match self.kind {
                NativeNodeKind::Text => SemanticRole::Text,
                NativeNodeKind::Button => SemanticRole::Button,
                NativeNodeKind::Column
                | NativeNodeKind::Row
                | NativeNodeKind::Grid
                | NativeNodeKind::Stack
                | NativeNodeKind::Instance => SemanticRole::Group,
                NativeNodeKind::Titlebar => SemanticRole::Toolbar,
            },
            NativeSemanticRole::semantic_role,
        );
        element = element
            .semantic_role(role)
            .semantic_enabled(!self.state.disabled)
            .semantic_metadata("document_model", "component")
            .semantic_metadata("available_projections", "html,gpui")
            .semantic_metadata("component_id", component_id)
            .semantic_metadata("gpui_kind", format!("{:?}", self.kind).to_lowercase())
            .semantic_metadata("authored_id", self.id.clone());
        if let Some(selected) = self.state.selected {
            element = element.semantic_selected(selected);
        }
        if let Some(checked) = self.state.checked {
            element = element.semantic_checked(checked);
        }
        if let Some(expanded) = self.state.expanded {
            element = element.semantic_expanded(expanded);
        }
        if let Some(referenced) = &self.instance_of {
            element = element.semantic_metadata("instance_of", referenced.clone());
        }
        if self.kind == NativeNodeKind::Titlebar {
            element = element.semantic_metadata("semantic_component", "titlebar");
        }
        if let Some(text) = &self.text {
            element = element
                .accessible_name(text.clone())
                .semantic_text(SemanticText {
                    text: text.clone(),
                    ..SemanticText::default()
                });
        }
        let resolved_action = resolved_actions
            .get(&self.id)
            .or(self.action.as_ref())
            .cloned();
        if let Some(action) = &resolved_action {
            element = element.semantic_metadata("action", action.clone());
        }
        if let Some(handler) = handler {
            let node_id = self.id.clone();
            let component = component_id.to_owned();
            // The cursor is owned by the canvas SURFACE (see render_canvas), not
            // per node, so it stays constant across the whole canvas in select/
            // move mode instead of flickering between arrow and grab as the
            // pointer crosses element boundaries. Nodes therefore set NO cursor
            // in draggable (Design) mode and let the surface's grab cursor show
            // through their (cursor-less) hitboxes. Only Test mode marks an
            // actionable node with the pointing hand to signal it is clickable.
            if !draggable && resolved_action.is_some() {
                element = element.cursor_pointer();
            }
            {
                // Select-on-press: the deepest node under the pointer fires
                // first (bubble order = children before ancestors), selects,
                // and stops propagation so an ancestor/root can't override the
                // selection on the same press. This is robust against the tiny
                // hand-movement that makes GPUI's release-time click fall
                // through to the giant root hitbox. Selection is press-based
                // only in select-capable modes; Test/Compare keep click/release
                // semantics (their handler ignores `Select`). We still don't
                // stop propagation of the underlying event GPUI uses to arm the
                // drag, because GPUI records `pending_mouse_down` for this
                // element before our listener runs.
                let down_handler = handler.clone();
                let down_component = component.clone();
                let down_node = node_id.clone();
                let down_automation = automation.clone();
                element
                    .interactivity()
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        down_automation.log("info", &format!("[native] press node={down_node}"));
                        if down_handler(
                            &down_component,
                            &down_node,
                            ComponentPointerGesture::Select,
                        ) {
                            // This node consumed the press and needs a repaint —
                            // stop propagation so an ancestor/root can't
                            // re-select over the top of it on the same press.
                            cx.stop_propagation();
                            window.refresh();
                        }
                    });
            }
            {
                let handler = handler.clone();
                let component = component.clone();
                let node_id = node_id.clone();
                let action = resolved_action.clone();
                element.interactivity().on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    if handler(
                        &component,
                        &node_id,
                        ComponentPointerGesture::Click {
                            action: action.clone(),
                        },
                    ) {
                        window.refresh();
                    }
                });
            }
            {
                let handler = handler.clone();
                let component = component.clone();
                let node_id = node_id.clone();
                element.interactivity().on_mouse_down(
                    MouseButton::Right,
                    move |event, window, cx| {
                        cx.stop_propagation();
                        if handler(
                            &component,
                            &node_id,
                            ComponentPointerGesture::ContextMenu {
                                x: f32::from(event.position.x),
                                y: f32::from(event.position.y),
                            },
                        ) {
                            window.refresh();
                        }
                    },
                );
            }
            {
                let handler = handler.clone();
                let component = component.clone();
                let node_id = node_id.clone();
                element.interactivity().on_mouse_move(move |_, window, cx| {
                    cx.stop_propagation();
                    if handler(&component, &node_id, ComponentPointerGesture::Hover) {
                        window.refresh();
                    }
                });
            }
            // Grab-to-move: draggable in select (Design) mode for every node
            // except the root, which has no parent to move within.
            if draggable && !is_root {
                let payload = ComponentNodeDrag {
                    component_id: component.clone(),
                    node_id: node_id.clone(),
                };
                let label = SharedString::from(
                    self.text
                        .clone()
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| node_id.clone()),
                );
                let drag_handler = handler.clone();
                let drag_component = component.clone();
                let drag_node_id = node_id.clone();
                let drag_automation = automation.clone();
                element
                    .interactivity()
                    .on_drag(payload, move |_, _, _, cx| {
                        // Grabbing a node auto-selects it, so "drag-and-hold" both
                        // selects and starts the move in one gesture.
                        let _ = drag_handler(
                            &drag_component,
                            &drag_node_id,
                            ComponentPointerGesture::Select,
                        );
                        drag_automation.log(
                            "info",
                            &format!("[move] grab start node={drag_node_id} (auto-selected)"),
                        );
                        let label = label.clone();
                        cx.new(|_| NodeDragGhost { label })
                    });
            }
        }
        element.into_any_element()
    }

    fn gpui_excerpt(
        &self,
        component_id: &str,
        resolved_actions: &BTreeMap<String, String>,
    ) -> String {
        let mut lines = vec!["div()".to_owned()];
        lines.push(match self.kind {
            NativeNodeKind::Grid => "    .grid()".to_owned(),
            NativeNodeKind::Stack => "    .relative()".to_owned(),
            _ => "    .flex()".to_owned(),
        });
        match self.kind {
            NativeNodeKind::Column => lines.push("    .flex_col()".to_owned()),
            NativeNodeKind::Row | NativeNodeKind::Titlebar => {
                lines.push("    .flex_row()".to_owned());
            }
            NativeNodeKind::Grid | NativeNodeKind::Stack => {}
            NativeNodeKind::Text | NativeNodeKind::Button | NativeNodeKind::Instance => {}
        }
        match self.layout.width {
            NativeSize::Fill => lines.push("    .w_full().flex_grow()".to_owned()),
            NativeSize::Hug => {}
            NativeSize::Fixed(value) => lines.push(format!("    .w(px({value}.))")),
        }
        match self.layout.height {
            NativeSize::Fill => lines.push("    .h_full().flex_grow()".to_owned()),
            NativeSize::Hug => {}
            NativeSize::Fixed(value) => lines.push(format!("    .h(px({value}.))")),
        }
        if self.layout.gap > 0 {
            lines.push(format!("    .gap(px({}.))", self.layout.gap));
        }
        if self.layout.padding > 0 {
            lines.push(format!("    .p(px({}.))", self.layout.padding));
        }
        match self.layout.wrap {
            NativeWrap::NoWrap => {}
            NativeWrap::Wrap => lines.push("    .flex_wrap()".to_owned()),
            NativeWrap::WrapReverse => lines.push("    .flex_wrap_reverse()".to_owned()),
        }
        if self.layout.grow {
            lines.push("    .flex_grow()".to_owned());
        }
        if !self.layout.shrink {
            lines.push("    .flex_shrink_0()".to_owned());
        }
        if let Some(value) = self.layout.basis {
            lines.push(format!("    .flex_basis(px({value}.))"));
        }
        if self.layout.grid_columns > 0 {
            lines.push(format!("    .grid_cols({})", self.layout.grid_columns));
        }
        if self.layout.grid_rows > 0 {
            lines.push(format!("    .grid_rows({})", self.layout.grid_rows));
        }
        if self.layout.opacity_percent < 100 {
            lines.push(format!(
                "    .opacity({:.2})",
                f32::from(self.layout.opacity_percent) / 100.0
            ));
        }
        if let Some(background) = self.appearance.background {
            lines.push(format!("    .bg(rgb(0x{background:06x}))"));
        }
        if self.appearance.radius > 0 {
            lines.push(format!("    .rounded(px({}.))", self.appearance.radius));
        }
        lines.push(format!(
            "    .font_family({:?}).text_size(px({}.)).font_weight(FontWeight({}.))",
            self.typography.family, self.typography.size, self.typography.weight
        ));
        lines.push(format!(
            "    .line_height(px({}.))",
            self.typography.line_height
        ));
        if let Some(text) = &self.text {
            lines.push(format!("    .child({text:?})"));
        }
        for child in &self.children {
            let child = child
                .gpui_excerpt(component_id, resolved_actions)
                .lines()
                .map(|line| format!("        {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            lines.push(format!("    .child(\n{child}\n    )"));
        }
        let role = self.semantic_role.map_or_else(
            || match self.kind {
                NativeNodeKind::Text => SemanticRole::Text,
                NativeNodeKind::Button => SemanticRole::Button,
                NativeNodeKind::Column
                | NativeNodeKind::Row
                | NativeNodeKind::Grid
                | NativeNodeKind::Stack
                | NativeNodeKind::Instance => SemanticRole::Group,
                NativeNodeKind::Titlebar => SemanticRole::Toolbar,
            },
            NativeSemanticRole::semantic_role,
        );
        lines.push(format!("    .id(\"component/{component_id}/{}\")", self.id));
        lines.push(format!("    .semantic_role(SemanticRole::{role:?})"));
        if let Some(action) = resolved_actions.get(&self.id).or(self.action.as_ref()) {
            lines.push(format!("    .semantic_metadata(\"action\", {action:?})"));
        }
        lines.join("\n")
    }

    fn write_html(&self, output: &mut String, depth: usize) {
        let indent = "  ".repeat(depth);
        let (tag, close_tag) = match self.kind {
            NativeNodeKind::Button => ("button", "button"),
            NativeNodeKind::Text => ("span", "span"),
            NativeNodeKind::Column
            | NativeNodeKind::Row
            | NativeNodeKind::Grid
            | NativeNodeKind::Stack
            | NativeNodeKind::Instance => ("div", "div"),
            NativeNodeKind::Titlebar => ("header", "header"),
        };
        output.push_str(&format!("{indent}<{tag} id=\"{}\"", self.id));
        if let Some(role) = self.semantic_role {
            output.push_str(&format!(" role=\"{}\"", role.aria_name()));
        } else if self.kind == NativeNodeKind::Titlebar {
            output.push_str(" role=\"toolbar\" aria-label=\"Window titlebar\"");
        }
        if let Some(selected) = self.state.selected {
            output.push_str(&format!(" aria-selected=\"{selected}\""));
        }
        if let Some(checked) = self.state.checked {
            output.push_str(&format!(" aria-checked=\"{checked}\""));
        }
        if let Some(expanded) = self.state.expanded {
            output.push_str(&format!(" aria-expanded=\"{expanded}\""));
        }
        if self.state.disabled {
            output.push_str(" aria-disabled=\"true\" disabled");
        }
        output.push('>');
        if let Some(text) = &self.text {
            output.push_str(&escape_html(text));
        }
        if self.children.is_empty() {
            output.push_str(&format!("</{close_tag}>\n"));
            return;
        }
        output.push('\n');
        for child in &self.children {
            child.write_html(output, depth + 1);
        }
        output.push_str(&format!("{indent}</{close_tag}>\n"));
    }

    fn write_css(&self, output: &mut String) {
        output.push_str(&format!("#{} {{\n", self.id));
        output.push_str(match self.kind {
            NativeNodeKind::Grid => "  display: grid;\n",
            NativeNodeKind::Stack => "  display: block;\n  position: relative;\n",
            _ => "  display: flex;\n",
        });
        match self.kind {
            NativeNodeKind::Column => output.push_str("  flex-direction: column;\n"),
            NativeNodeKind::Row | NativeNodeKind::Titlebar => {
                output.push_str("  flex-direction: row;\n");
            }
            NativeNodeKind::Grid
            | NativeNodeKind::Stack
            | NativeNodeKind::Text
            | NativeNodeKind::Button
            | NativeNodeKind::Instance => {}
        }
        write_size_css(output, "width", self.layout.width);
        write_size_css(output, "height", self.layout.height);
        if self.layout.gap > 0 {
            output.push_str(&format!("  gap: {}px;\n", self.layout.gap));
        }
        if self.layout.padding > 0 {
            output.push_str(&format!("  padding: {}px;\n", self.layout.padding));
        }
        if let Some(edges) = self.layout.padding_edges {
            output.push_str(&format!(
                "  padding: {}px {}px {}px {}px;\n",
                edges.top, edges.right, edges.bottom, edges.left
            ));
        }
        output.push_str(&format!(
            "  margin: {}px {}px {}px {}px;\n",
            self.layout.margin.top,
            self.layout.margin.right,
            self.layout.margin.bottom,
            self.layout.margin.left
        ));
        output.push_str(&format!(
            "  align-items: {};\n  justify-content: {};\n",
            alignment_css(self.layout.align),
            alignment_css(self.layout.justify)
        ));
        output.push_str(&format!(
            "  flex-wrap: {};\n  flex-grow: {};\n  flex-shrink: {};\n",
            match self.layout.wrap {
                NativeWrap::NoWrap => "nowrap",
                NativeWrap::Wrap => "wrap",
                NativeWrap::WrapReverse => "wrap-reverse",
            },
            u8::from(self.layout.grow),
            u8::from(self.layout.shrink)
        ));
        write_optional_px(output, "flex-basis", self.layout.basis);
        write_optional_px(output, "min-width", self.layout.min_width);
        write_optional_px(output, "max-width", self.layout.max_width);
        write_optional_px(output, "min-height", self.layout.min_height);
        write_optional_px(output, "max-height", self.layout.max_height);
        if self.layout.position == NativePosition::Absolute {
            output.push_str("  position: absolute;\n");
            write_optional_signed_px(output, "top", self.layout.offsets.top);
            write_optional_signed_px(output, "right", self.layout.offsets.right);
            write_optional_signed_px(output, "bottom", self.layout.offsets.bottom);
            write_optional_signed_px(output, "left", self.layout.offsets.left);
        }
        if self.layout.grid_columns > 0 {
            output.push_str(&format!(
                "  grid-template-columns: repeat({}, minmax(0, 1fr));\n",
                self.layout.grid_columns
            ));
        }
        if self.layout.grid_rows > 0 {
            output.push_str(&format!(
                "  grid-template-rows: repeat({}, minmax(0, 1fr));\n",
                self.layout.grid_rows
            ));
        }
        if let Some(start) = self.layout.column_start {
            output.push_str(&format!("  grid-column-start: {start};\n"));
        }
        if self.layout.column_span > 1 {
            output.push_str(&format!(
                "  grid-column-end: span {};\n",
                self.layout.column_span
            ));
        }
        if let Some(start) = self.layout.row_start {
            output.push_str(&format!("  grid-row-start: {start};\n"));
        }
        if self.layout.row_span > 1 {
            output.push_str(&format!("  grid-row-end: span {};\n", self.layout.row_span));
        }
        output.push_str(&format!(
            "  overflow: {};\n  opacity: {:.2};\n  z-index: {};\n",
            match self.layout.overflow {
                NativeOverflow::Visible => "visible",
                NativeOverflow::Hidden => "hidden",
                NativeOverflow::Scroll => "auto",
            },
            f32::from(self.layout.opacity_percent) / 100.0,
            self.layout.z_index
        ));
        if self.layout.rotation_degrees != 0 {
            output.push_str(&format!(
                "  transform: rotate({}deg);\n",
                self.layout.rotation_degrees
            ));
        }
        output.push_str(&format!(
            "  --studio-horizontal-constraint: {};\n  --studio-vertical-constraint: {};\n",
            constraint_css(self.layout.horizontal_constraint),
            constraint_css(self.layout.vertical_constraint)
        ));
        output.push_str(&format!("  color: #{:06x};\n", self.appearance.foreground));
        output.push_str(&format!("  font-family: {:?};\n", self.typography.family));
        output.push_str(&format!("  font-size: {}px;\n", self.typography.size));
        output.push_str(&format!("  font-weight: {};\n", self.typography.weight));
        output.push_str(&format!(
            "  line-height: {}px;\n",
            self.typography.line_height
        ));
        if let Some(background) = self.appearance.background {
            output.push_str(&format!("  background-color: #{background:06x};\n"));
        }
        if let Some(border) = self.appearance.border {
            output.push_str(&format!("  border: 1px solid #{border:06x};\n"));
        }
        if self.appearance.radius > 0 {
            output.push_str(&format!("  border-radius: {}px;\n", self.appearance.radius));
        }
        output.push_str("}\n");
        for child in &self.children {
            child.write_css(output);
        }
    }

    fn collect_bindings(
        &self,
        output: &mut Vec<String>,
        explicit_click_sources: &BTreeSet<String>,
    ) {
        if let Some(action) = &self.action
            && !explicit_click_sources.contains(&self.id)
        {
            output.push(format!(
                "        Event(target: Id({:?}), event: Click, handler: {:?}),",
                self.id, action
            ));
        }
        for child in &self.children {
            child.collect_bindings(output, explicit_click_sources);
        }
    }

    fn contains_node(&self, id: &str) -> bool {
        self.id == id || self.children.iter().any(|child| child.contains_node(id))
    }

    fn migrate_legacy_backgrounds(&mut self) {
        if self.kind == NativeNodeKind::Text && self.appearance.background == Some(0) {
            self.appearance.background = None;
        }
        for child in &mut self.children {
            child.migrate_legacy_backgrounds();
        }
    }

    fn normalize_legacy_vocabulary(&mut self, previous_name: &str, current_name: &str) {
        if let Some(text) = &mut self.text {
            match text.as_str() {
                "DIRECT GPUI COMPONENT" => *text = "UNIFIED COMPONENT DOCUMENT".to_owned(),
                "This tree renders as gpui::div elements without HTML or CSS translation." => {
                    *text = "Edit the same component as HTML/CSS or GPUI; both projections stay in sync."
                        .to_owned();
                }
                "Native action" => *text = "Component action".to_owned(),
                value if value == previous_name && previous_name != current_name => {
                    *text = current_name.to_owned();
                }
                _ => {}
            }
        }
        if self.action.as_deref() == Some("native_action") {
            self.action = Some("component_action".to_owned());
        }
        for child in &mut self.children {
            child.normalize_legacy_vocabulary(previous_name, current_name);
        }
    }

    fn validate<'a>(
        &'a self,
        count: &mut usize,
        identifiers: &mut BTreeSet<&'a str>,
    ) -> Result<(), NativeComponentError> {
        *count = count.saturating_add(1);
        if *count > MAX_NODES_PER_COMPONENT {
            return Err(NativeComponentError::Invalid(
                "component exceeds the native node bound",
            ));
        }
        validate_identifier(&self.id)?;
        if !identifiers.insert(self.id.as_str()) {
            return Err(NativeComponentError::Invalid(
                "node identifiers must be unique within a component",
            ));
        }
        if self
            .text
            .as_ref()
            .is_some_and(|text| text.len() > 16 * 1024)
            || self
                .action
                .as_ref()
                .is_some_and(|action| validate_identifier(action).is_err())
            || self.layout.gap > 1_024
            || self.layout.padding > 1_024
            || self
                .layout
                .padding_edges
                .is_some_and(|edges| !valid_edges(edges))
            || !valid_edges(self.layout.margin)
            || self.layout.grid_columns > 1_024
            || self.layout.grid_rows > 1_024
            || !(1..=1_024).contains(&self.layout.column_span)
            || !(1..=1_024).contains(&self.layout.row_span)
            || self.layout.opacity_percent > 100
            || !(-3_600..=3_600).contains(&self.layout.rotation_degrees)
            || invalid_min_max(self.layout.min_width, self.layout.max_width)
            || invalid_min_max(self.layout.min_height, self.layout.max_height)
            || (matches!(
                self.kind,
                NativeNodeKind::Text | NativeNodeKind::Button | NativeNodeKind::Instance
            ) && !self.children.is_empty())
            || (matches!(self.kind, NativeNodeKind::Instance) && self.instance_of.is_none())
            || (!matches!(self.kind, NativeNodeKind::Instance) && self.instance_of.is_some())
            || self.typography.family.is_empty()
            || self.typography.family.len() > 128
            || !(6..=512).contains(&self.typography.size)
            || !(100..=900).contains(&self.typography.weight)
            || !(6..=1_024).contains(&self.typography.line_height)
        {
            return Err(NativeComponentError::Invalid(
                "native node content or layout is invalid",
            ));
        }
        for child in &self.children {
            child.validate(count, identifiers)?;
        }
        Ok(())
    }
}

/// Accessible semantics that can be applied without changing visual backing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeSemanticRole {
    /// Alert or status message.
    Alert,
    /// Modal or non-modal dialog surface.
    Dialog,
    /// Related controls or content.
    Group,
    /// Select-style option collection.
    Listbox,
    /// Menu container.
    Menu,
    /// Action inside a menu.
    MenuItem,
    /// Selectable item inside a listbox.
    Option,
    /// Semantic scroll region.
    ScrollArea,
    /// Slider or separator handle exposing an adjustable value.
    Slider,
    /// Tab-list container.
    TabList,
    /// One selectable tab.
    Tab,
    /// Content controlled by a tab.
    TabPanel,
    /// Contextual description.
    Tooltip,
    /// Application toolbar.
    Toolbar,
}

/// Accessibility state projected to both ARIA and the GPUI MCP semantic tree.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSemanticState {
    /// Whether a choice, tab, or option is selected.
    pub selected: Option<bool>,
    /// Whether a checkbox-like control is checked.
    pub checked: Option<bool>,
    /// Whether a disclosure or popup trigger is expanded.
    pub expanded: Option<bool>,
    /// Whether input is disabled.
    pub disabled: bool,
}

impl NativeSemanticRole {
    const fn semantic_role(self) -> SemanticRole {
        match self {
            Self::Alert => SemanticRole::Alert,
            Self::Dialog => SemanticRole::Dialog,
            Self::Group => SemanticRole::Group,
            Self::Listbox => SemanticRole::List,
            Self::Menu => SemanticRole::Menu,
            Self::MenuItem => SemanticRole::MenuItem,
            Self::Option => SemanticRole::Option,
            Self::ScrollArea => SemanticRole::ScrollArea,
            Self::Slider => SemanticRole::Slider,
            Self::TabList => SemanticRole::TabList,
            Self::Tab => SemanticRole::Tab,
            Self::TabPanel => SemanticRole::Group,
            Self::Tooltip => SemanticRole::Tooltip,
            Self::Toolbar => SemanticRole::Toolbar,
        }
    }

    const fn aria_name(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::Dialog => "dialog",
            Self::Group => "group",
            Self::Listbox => "listbox",
            Self::Menu => "menu",
            Self::MenuItem => "menuitem",
            Self::Option => "option",
            Self::ScrollArea => "region",
            Self::Slider => "slider",
            Self::TabList => "tablist",
            Self::Tab => "tab",
            Self::TabPanel => "tabpanel",
            Self::Tooltip => "tooltip",
            Self::Toolbar => "toolbar",
        }
    }
}

const fn valid_edges(edges: NativeEdges) -> bool {
    edges.top <= 1_024 && edges.right <= 1_024 && edges.bottom <= 1_024 && edges.left <= 1_024
}

const fn invalid_min_max(minimum: Option<u16>, maximum: Option<u16>) -> bool {
    matches!((minimum, maximum), (Some(minimum), Some(maximum)) if minimum > maximum)
}

fn write_size_css(output: &mut String, property: &str, size: NativeSize) {
    match size {
        NativeSize::Fill => output.push_str(&format!("  {property}: 100%;\n")),
        NativeSize::Hug => {}
        NativeSize::Fixed(value) => output.push_str(&format!("  {property}: {value}px;\n")),
    }
}

fn write_optional_px(output: &mut String, property: &str, value: Option<u16>) {
    if let Some(value) = value {
        output.push_str(&format!("  {property}: {value}px;\n"));
    }
}

fn write_optional_signed_px(output: &mut String, property: &str, value: Option<i16>) {
    if let Some(value) = value {
        output.push_str(&format!("  {property}: {value}px;\n"));
    }
}

const fn alignment_css(alignment: NativeAlign) -> &'static str {
    match alignment {
        NativeAlign::Start => "flex-start",
        NativeAlign::Center => "center",
        NativeAlign::End => "flex-end",
    }
}

const fn constraint_css(constraint: NativeConstraint) -> &'static str {
    match constraint {
        NativeConstraint::Start => "start",
        NativeConstraint::Center => "center",
        NativeConstraint::End => "end",
        NativeConstraint::Scale => "scale",
        NativeConstraint::Stretch => "stretch",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Depth-first expansion of instance nodes into id-prefixed copies of the
/// referenced component graphs. `ancestry` holds the component ids currently
/// being expanded so cycles resolve to a placeholder rather than recursing
/// forever.
fn expand_instances_in(
    node: &mut NativeNode,
    library: &NativeComponentLibrary,
    ancestry: &mut Vec<String>,
) {
    if node.kind == NativeNodeKind::Instance {
        let referenced = node.instance_of.clone();
        node.children.clear();
        match referenced {
            Some(component_id) if ancestry.contains(&component_id) => {
                node.children = vec![NativeNode::text(
                    &format!("{}--cycle", node.id),
                    &format!("↻ {component_id} (cycle)"),
                    0xc2_6a_4a,
                )];
            }
            Some(component_id) => {
                if let Some(master) = library.component(&component_id) {
                    let mut expanded = master.root.clone();
                    expanded.prefix_ids(&format!("{}--", node.id));
                    ancestry.push(component_id);
                    expand_instances_in(&mut expanded, library, ancestry);
                    ancestry.pop();
                    node.children = vec![expanded];
                } else {
                    node.children = vec![NativeNode::text(
                        &format!("{}--missing", node.id),
                        &format!("Missing component {component_id}"),
                        0xc2_6a_4a,
                    )];
                }
            }
            None => {}
        }
        return;
    }
    for child in &mut node.children {
        expand_instances_in(child, library, ancestry);
    }
}

/// Neutral public names for the canonical document. The legacy `Native*` aliases remain readable
/// so existing version-1/2 project files migrate without breaking offline workspaces.
pub type AuthoringProjection = AuthoringBackend;
/// Project-local canonical component library.
pub type ComponentLibrary = NativeComponentLibrary;
/// One canonical component definition.
pub type ComponentDefinition = NativeComponent;
/// One node in a canonical component graph.
pub type ComponentNode = NativeNode;
/// Invalid or unpersistable canonical component data.
pub type ComponentDocumentError = NativeComponentError;

/// Native GPUI node behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeNodeKind {
    /// Vertical flex container.
    Column,
    /// Horizontal flex container.
    Row,
    /// CSS-grid-compatible container.
    Grid,
    /// Freeform overlay container for positioned children.
    Stack,
    /// Document-authored window titlebar; native OS decorations remain a separate output policy.
    Titlebar,
    /// Semantic text element.
    Text,
    /// Semantic clickable element.
    Button,
    /// A reusable instance of another component in the same library.
    ///
    /// The referenced component id lives in [`NativeNode::instance_of`]. At
    /// render and semantic time an instance is resolved into an id-prefixed copy
    /// of the referenced component's graph; in the stored graph it is a leaf.
    Instance,
}

/// Native flex layout values for one node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeLayout {
    /// Horizontal sizing policy.
    pub width: NativeSize,
    /// Vertical sizing policy.
    pub height: NativeSize,
    /// GPUI flex gap in logical pixels.
    pub gap: u16,
    /// Uniform padding in logical pixels.
    pub padding: u16,
    /// Cross-axis alignment.
    pub align: NativeAlign,
    /// Main-axis justification.
    pub justify: NativeAlign,
    /// Flex wrapping policy.
    #[serde(default)]
    pub wrap: NativeWrap,
    /// Whether this node grows in remaining Flexbox space.
    #[serde(default)]
    pub grow: bool,
    /// Whether this node may shrink below its basis.
    #[serde(default = "default_true")]
    pub shrink: bool,
    /// Optional Flexbox basis in logical pixels.
    #[serde(default)]
    pub basis: Option<u16>,
    /// Optional minimum width.
    #[serde(default)]
    pub min_width: Option<u16>,
    /// Optional maximum width.
    #[serde(default)]
    pub max_width: Option<u16>,
    /// Optional minimum height.
    #[serde(default)]
    pub min_height: Option<u16>,
    /// Optional maximum height.
    #[serde(default)]
    pub max_height: Option<u16>,
    /// Per-edge padding that overrides `padding` when present.
    #[serde(default)]
    pub padding_edges: Option<NativeEdges>,
    /// Per-edge outer margin.
    #[serde(default)]
    pub margin: NativeEdges,
    /// Flow or absolute positioning.
    #[serde(default)]
    pub position: NativePosition,
    /// Optional absolute offsets.
    #[serde(default)]
    pub offsets: NativeOffsets,
    /// Explicit grid column count for grid containers.
    #[serde(default)]
    pub grid_columns: u16,
    /// Explicit grid row count for grid containers.
    #[serde(default)]
    pub grid_rows: u16,
    /// One-based grid column start for grid items.
    #[serde(default)]
    pub column_start: Option<i16>,
    /// Grid column span.
    #[serde(default = "default_grid_span")]
    pub column_span: u16,
    /// One-based grid row start for grid items.
    #[serde(default)]
    pub row_start: Option<i16>,
    /// Grid row span.
    #[serde(default = "default_grid_span")]
    pub row_span: u16,
    /// Overflow behavior.
    #[serde(default)]
    pub overflow: NativeOverflow,
    /// Paint order within a freeform parent.
    #[serde(default)]
    pub z_index: i16,
    /// Opacity from zero through 100.
    #[serde(default = "default_opacity")]
    pub opacity_percent: u8,
    /// Clockwise rotation stored by the visual editor and CSS projection.
    #[serde(default)]
    pub rotation_degrees: i16,
    /// Horizontal resize anchor.
    #[serde(default)]
    pub horizontal_constraint: NativeConstraint,
    /// Vertical resize anchor.
    #[serde(default)]
    pub vertical_constraint: NativeConstraint,
}

impl Default for NativeLayout {
    fn default() -> Self {
        Self {
            width: NativeSize::Hug,
            height: NativeSize::Hug,
            gap: 0,
            padding: 0,
            align: NativeAlign::Start,
            justify: NativeAlign::Start,
            wrap: NativeWrap::NoWrap,
            grow: false,
            shrink: true,
            basis: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            padding_edges: None,
            margin: NativeEdges::default(),
            position: NativePosition::Flow,
            offsets: NativeOffsets::default(),
            grid_columns: 0,
            grid_rows: 0,
            column_start: None,
            column_span: 1,
            row_start: None,
            row_span: 1,
            overflow: NativeOverflow::Visible,
            z_index: 0,
            opacity_percent: 100,
            rotation_degrees: 0,
            horizontal_constraint: NativeConstraint::Start,
            vertical_constraint: NativeConstraint::Start,
        }
    }
}

const fn default_true() -> bool {
    true
}
const fn default_grid_span() -> u16 {
    1
}
const fn default_opacity() -> u8 {
    100
}

/// Flex wrapping behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeWrap {
    /// Keep children on one line.
    #[default]
    NoWrap,
    /// Wrap onto additional lines.
    Wrap,
    /// Wrap additional lines in reverse cross-axis order.
    WrapReverse,
}

/// Overflow behavior shared by native GPUI and CSS projections.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeOverflow {
    /// Paint overflowing content.
    #[default]
    Visible,
    /// Clip overflowing content.
    Hidden,
    /// Expose a scroll container.
    Scroll,
}

/// Layout participation mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativePosition {
    /// Participate in parent Flexbox or Grid flow.
    #[default]
    Flow,
    /// Use explicit offsets inside a freeform parent.
    Absolute,
}

/// Resize anchoring constraint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeConstraint {
    /// Preserve the leading-edge distance.
    #[default]
    Start,
    /// Preserve the center offset.
    Center,
    /// Preserve the trailing-edge distance.
    End,
    /// Scale position and size with the parent.
    Scale,
    /// Preserve both edge distances and stretch.
    Stretch,
}

/// Four logical-pixel edges in CSS order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeEdges {
    /// Top edge.
    pub top: u16,
    /// Right edge.
    pub right: u16,
    /// Bottom edge.
    pub bottom: u16,
    /// Left edge.
    pub left: u16,
}

/// Optional signed offsets for absolutely positioned nodes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeOffsets {
    /// Top offset.
    pub top: Option<i16>,
    /// Right offset.
    pub right: Option<i16>,
    /// Bottom offset.
    pub bottom: Option<i16>,
    /// Left offset.
    pub left: Option<i16>,
}

/// Native dimension policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeSize {
    /// Fill the available axis and participate in flex growth.
    Fill,
    /// Size to content.
    Hug,
    /// Use a fixed logical-pixel size.
    Fixed(u16),
}

/// Start, center, or end flex alignment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeAlign {
    /// Align to flex start.
    Start,
    /// Center on the axis.
    Center,
    /// Align to flex end.
    End,
}

/// Native paint values for one node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeAppearance {
    /// Optional RGB background color encoded as `0xRRGGBB`; `None` is transparent.
    #[serde(default, deserialize_with = "deserialize_optional_color")]
    pub background: Option<u32>,
    /// RGB foreground color encoded as `0xRRGGBB`.
    pub foreground: u32,
    /// Optional RGB border color encoded as `0xRRGGBB`.
    pub border: Option<u32>,
    /// Uniform corner radius in logical pixels.
    pub radius: u16,
}

impl Default for NativeAppearance {
    fn default() -> Self {
        Self {
            background: None,
            foreground: 0xff_ff_ff,
            border: None,
            radius: 0,
        }
    }
}

/// Native text metrics inherited by a node and its children.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTypography {
    /// Installed font family name.
    pub family: String,
    /// Font size in logical pixels.
    pub size: u16,
    /// CSS-compatible numeric font weight from 100 through 900.
    pub weight: u16,
    /// Explicit line box height in logical pixels.
    pub line_height: u16,
}

impl Default for NativeTypography {
    fn default() -> Self {
        Self {
            family: "Geist".to_owned(),
            size: 14,
            weight: 400,
            line_height: 20,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OptionalColor {
    Legacy(u32),
    Current(Option<u32>),
}

fn deserialize_optional_color<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    OptionalColor::deserialize(deserializer).map(|color| match color {
        OptionalColor::Legacy(color) => Some(color),
        OptionalColor::Current(color) => color,
    })
}

fn apply_size(element: gpui::Div, size: NativeSize, horizontal: bool) -> gpui::Div {
    match (size, horizontal) {
        (NativeSize::Fill, true) => element.w_full().flex_1().min_w_0(),
        (NativeSize::Fill, false) => element.h_full().flex_1().min_h_0(),
        (NativeSize::Hug, _) => element,
        (NativeSize::Fixed(value), true) => element.w(px(f32::from(value))),
        (NativeSize::Fixed(value), false) => element.h(px(f32::from(value))),
    }
}

fn apply_alignment(element: gpui::Div, alignment: NativeAlign, justify: bool) -> gpui::Div {
    match (alignment, justify) {
        (NativeAlign::Start, true) => element.justify_start(),
        (NativeAlign::Center, true) => element.justify_center(),
        (NativeAlign::End, true) => element.justify_end(),
        (NativeAlign::Start, false) => element.items_start(),
        (NativeAlign::Center, false) => element.items_center(),
        (NativeAlign::End, false) => element.items_end(),
    }
}

fn apply_advanced_layout(mut element: gpui::Div, layout: NativeLayout) -> gpui::Div {
    element = match layout.wrap {
        NativeWrap::NoWrap => element.flex_nowrap(),
        NativeWrap::Wrap => element.flex_wrap(),
        NativeWrap::WrapReverse => element.flex_wrap_reverse(),
    };
    if layout.grow {
        element = element.flex_grow();
    }
    if !layout.shrink {
        element = element.flex_shrink_0();
    }
    if let Some(basis) = layout.basis {
        element = element.flex_basis(px(f32::from(basis)));
    }
    if let Some(value) = layout.min_width {
        element = element.min_w(px(f32::from(value)));
    }
    if let Some(value) = layout.max_width {
        element = element.max_w(px(f32::from(value)));
    }
    if let Some(value) = layout.min_height {
        element = element.min_h(px(f32::from(value)));
    }
    if let Some(value) = layout.max_height {
        element = element.max_h(px(f32::from(value)));
    }
    if let Some(edges) = layout.padding_edges {
        element = element
            .pt(px(f32::from(edges.top)))
            .pr(px(f32::from(edges.right)))
            .pb(px(f32::from(edges.bottom)))
            .pl(px(f32::from(edges.left)));
    }
    element = element
        .mt(px(f32::from(layout.margin.top)))
        .mr(px(f32::from(layout.margin.right)))
        .mb(px(f32::from(layout.margin.bottom)))
        .ml(px(f32::from(layout.margin.left)))
        .opacity(f32::from(layout.opacity_percent) / 100.0);
    if layout.position == NativePosition::Absolute {
        element = element.absolute();
        if let Some(value) = layout.offsets.top {
            element = element.top(px(f32::from(value)));
        }
        if let Some(value) = layout.offsets.right {
            element = element.right(px(f32::from(value)));
        }
        if let Some(value) = layout.offsets.bottom {
            element = element.bottom(px(f32::from(value)));
        }
        if let Some(value) = layout.offsets.left {
            element = element.left(px(f32::from(value)));
        }
    }
    if layout.grid_columns > 0 {
        element = element.grid_cols(layout.grid_columns);
    }
    if layout.grid_rows > 0 {
        element = element.grid_rows(layout.grid_rows);
    }
    if let Some(value) = layout.column_start {
        element = element.col_start(value);
    }
    if layout.column_span > 1 {
        element = element.col_span(layout.column_span);
    }
    if let Some(value) = layout.row_start {
        element = element.row_start(value);
    }
    if layout.row_span > 1 {
        element = element.row_span(layout.row_span);
    }
    match layout.overflow {
        NativeOverflow::Visible => element,
        NativeOverflow::Hidden => element.overflow_hidden(),
        NativeOverflow::Scroll => {
            element.style().overflow.x = Some(gpui::Overflow::Scroll);
            element.style().overflow.y = Some(gpui::Overflow::Scroll);
            element
        }
    }
}

fn component_logic_guard_parts(guard: &str) -> Option<(&str, &str, bool)> {
    let guard = guard.trim();
    let (name, expected, equality) = match (guard.split_once("=="), guard.split_once("!=")) {
        (Some((name, expected)), None) => (name, expected, true),
        (None, Some((name, expected))) => (name, expected, false),
        _ => return None,
    };
    let name = name.trim();
    let expected = expected.trim().trim_matches(['\'', '"']);
    (!name.is_empty() && validate_identifier(name).is_ok()).then_some((name, expected, equality))
}

pub(crate) fn component_logic_guard_matches(
    guard: Option<&str>,
    state: &BTreeMap<String, String>,
) -> bool {
    let Some(guard) = guard else {
        return true;
    };
    let Some((name, expected, equality)) = component_logic_guard_parts(guard) else {
        return false;
    };
    state
        .get(name)
        .is_some_and(|current| (current == expected) == equality)
}

fn validate_identifier(value: &str) -> Result<(), NativeComponentError> {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        });
    if value.len() > 128 || !valid {
        return Err(NativeComponentError::Invalid(
            "component and node identifiers must use portable identifier characters",
        ));
    }
    Ok(())
}

fn library_path(project_root: &Path) -> PathBuf {
    project_root.join(".gpui-studio").join("components.ron")
}

fn legacy_library_path(project_root: &Path) -> PathBuf {
    project_root
        .join(".gpui-studio")
        .join("native-components.ron")
}

/// Invalid or unpersistable native component data.
#[derive(Debug, thiserror::Error)]
pub enum NativeComponentError {
    /// The library exceeds the bounded local input size.
    #[error("native component library is {found} bytes; maximum is {maximum}")]
    TooLarge {
        /// Observed bytes.
        found: u64,
        /// Maximum bytes.
        maximum: u64,
    },
    /// RON parsing failed.
    #[error("parse native component library")]
    Parse(#[from] ron::error::SpannedError),
    /// RON serialization failed.
    #[error("serialize native component library")]
    Serialize(#[from] ron::Error),
    /// The schema version is unsupported.
    #[error("native component version {found} is unsupported; expected {supported}")]
    UnsupportedVersion {
        /// Parsed version.
        found: u16,
        /// Current version.
        supported: u16,
    },
    /// Component data violates a bounded semantic invariant.
    #[error("invalid native component library: {0}")]
    Invalid(&'static str),
    /// A filesystem operation failed.
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
    /// Atomic replacement failed.
    #[error("replace native component library `{}`", path.display())]
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
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use tempfile::TempDir;

    use super::{
        COMPONENT_LIBRARY_VERSION, ComponentEvent, ComponentLogic, ComponentPreset,
        NativeComponentError, NativeComponentLibrary, NativeNode, NativeNodeKind,
        NativeSemanticRole, NativeSize,
    };

    fn node_ids(root: &NativeNode) -> Vec<String> {
        let mut ids = vec![root.id.clone()];
        for child in &root.children {
            ids.extend(node_ids(child));
        }
        ids
    }

    #[test]
    fn instance_nodes_resolve_into_prefixed_copies_and_guard_cycles() {
        let mut library = NativeComponentLibrary::default();
        library.create_named_component("Leaf");
        let host_id = library.components[0].id.clone();
        let leaf_id = library.components[1].id.clone();

        // Place an instance of the leaf inside the host root.
        library.components[0]
            .root
            .children
            .push(NativeNode::authored_instance("inst", &leaf_id));

        let host = library.components[0].clone();
        let mut resolved = host.root.clone();
        host.expand_instances(&mut resolved, &library);

        let instance = resolved
            .children
            .iter()
            .find(|node| node.kind == NativeNodeKind::Instance);
        assert!(
            instance.is_some(),
            "instance node should survive resolution"
        );
        if let Some(instance) = instance {
            assert_eq!(instance.instance_of.as_deref(), Some(leaf_id.as_str()));
            assert_eq!(instance.children.len(), 1, "instance expands its master");
            assert_eq!(
                instance.children[0].id, "inst--root",
                "expanded ids are prefixed by the instance id"
            );
        }

        // Direct and transitive cycles are rejected.
        assert!(library.would_cycle(&host_id, &host_id));
        assert!(library.would_cycle(&leaf_id, &host_id));
        assert!(!library.would_cycle(&host_id, &leaf_id));
    }

    #[test]
    fn remove_component_refuses_the_root() {
        let mut library = NativeComponentLibrary::default();
        let root_id = library.components[0].id.clone();
        library.create_named_component("Leaf");

        let result = library.remove_component(&root_id);
        assert!(result.is_err());
        if let Err(error) = result {
            assert!(
                error.contains("root component cannot be deleted"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn remove_component_refuses_when_still_referenced() {
        let mut library = NativeComponentLibrary::default();
        let leaf_id = library.create_named_component("Leaf").id.clone();
        let host_name = library.components[0].name.clone();
        library.components[0]
            .root
            .children
            .push(NativeNode::authored_instance("inst", &leaf_id));

        let result = library.remove_component(&leaf_id);
        assert!(result.is_err());
        if let Err(error) = result {
            assert!(
                error.contains(&host_name),
                "error should name the referencing component: {error}"
            );
        }
    }

    #[test]
    fn remove_component_succeeds_for_unreferenced_non_root() {
        let mut library = NativeComponentLibrary::default();
        let root_id = library.components[0].id.clone();
        let leaf_id = library.create_named_component("Leaf").id.clone();

        let result = library.remove_component(&leaf_id);
        assert!(result.is_ok());
        assert!(library.component(&leaf_id).is_none());
        assert_eq!(library.components.len(), 1);
        assert_eq!(library.active_component, root_id);
    }

    #[test]
    fn normalize_root_name_renames_default_component_one_but_not_custom_names() {
        let mut library = NativeComponentLibrary::default();
        library.components[0].name = "Component 1".to_owned();
        library.normalize_root_name();
        assert_eq!(library.components[0].name, "Root");

        library.components[0].name = "My Custom Root".to_owned();
        library.normalize_root_name();
        assert_eq!(library.components[0].name, "My Custom Root");
    }

    #[test]
    fn semantic_titlebar_projects_to_html_gpui_and_window_actions() {
        let mut library = NativeComponentLibrary::default();
        let component = library.create_titlebar_component_with_props("AppTitlebar", Vec::new());

        let html = component.html_projection();
        let gpui = component.gpui_excerpt();
        let bindings = component.bindings_projection();
        assert!(html.contains("<header id=\"root\" role=\"toolbar\""));
        assert!(gpui.contains(".flex_row()"));
        assert!(bindings.contains("window_minimize"));
        assert!(bindings.contains("window_maximize"));
        assert!(bindings.contains("window_close"));
        assert_eq!(component.root.children[0].layout.width, NativeSize::Fill);
    }

    #[test]
    fn every_builtin_preset_is_a_valid_adjustable_dual_projection() {
        let mut library = NativeComponentLibrary::default();
        for preset in ComponentPreset::ALL {
            let component = library.create_preset_component(preset, preset.label());
            let ids = node_ids(&component.root);
            assert_eq!(
                ids.len(),
                ids.iter().collect::<BTreeSet<_>>().len(),
                "{} contains duplicate node ids",
                preset.label()
            );
            assert!(component.html_projection().contains("id=\"root\""));
            assert!(component.css_projection().contains("font-size:"));
            assert!(component.css_projection().contains("line-height:"));
            assert!(component.gpui_excerpt().contains(".font_family("));
            if matches!(
                preset,
                ComponentPreset::Button
                    | ComponentPreset::ButtonGroup
                    | ComponentPreset::Card
                    | ComponentPreset::Alert
                    | ComponentPreset::Toolbar
                    | ComponentPreset::EmptyState
                    | ComponentPreset::Titlebar
                    | ComponentPreset::Tabs
                    | ComponentPreset::Dialog
                    | ComponentPreset::Dropdown
                    | ComponentPreset::DropdownMenu
                    | ComponentPreset::Drawer
                    | ComponentPreset::Resizable
            ) {
                assert!(component.bindings_projection().contains("Event("));
            }
            let html = component.html_projection();
            let css = component.css_projection();
            match preset {
                ComponentPreset::ButtonGroup => {
                    assert_eq!(
                        ids,
                        [
                            "root",
                            "button-group",
                            "segment-left",
                            "segment-center",
                            "segment-right",
                        ]
                        .map(str::to_owned)
                    );
                    assert!(html.contains("id=\"button-group\" role=\"group\""));
                    assert!(html.contains("aria-selected=\"true\""));
                    assert_eq!(component.logic.len(), 3);
                    assert_eq!(component.variants.len(), 2);
                    let resolved = component.resolved_root(&BTreeMap::from([(
                        "selected".to_owned(),
                        "left".to_owned(),
                    )]));
                    assert_eq!(
                        resolved
                            .find("segment-left")
                            .and_then(|node| node.state.selected),
                        Some(true)
                    );
                    assert_eq!(
                        resolved
                            .find("segment-center")
                            .and_then(|node| node.state.selected),
                        Some(false)
                    );
                }
                ComponentPreset::Dropdown => {
                    assert_eq!(
                        ids,
                        [
                            "root",
                            "dropdown",
                            "dropdown-trigger",
                            "option-list",
                            "option-production",
                            "option-staging",
                            "option-development",
                        ]
                        .map(str::to_owned)
                    );
                    assert!(html.contains("id=\"dropdown-trigger\" aria-expanded=\"false\""));
                    assert!(html.contains("id=\"option-list\" role=\"listbox\""));
                    assert!(html.contains("role=\"option\""));
                    assert_eq!(component.states.len(), 2);
                    assert_eq!(component.logic.len(), 8);
                    assert_eq!(component.slots.len(), 1);
                    let resolved = component.resolved_root(&BTreeMap::from([
                        ("open".to_owned(), "true".to_owned()),
                        ("selected".to_owned(), "staging".to_owned()),
                    ]));
                    assert_eq!(
                        resolved
                            .find("dropdown-trigger")
                            .and_then(|node| node.state.expanded),
                        Some(true)
                    );
                    assert_eq!(
                        resolved
                            .find("dropdown-trigger")
                            .and_then(|node| node.text.as_deref()),
                        Some("Staging           ▾")
                    );
                    assert_eq!(
                        resolved.find("option-list").map(|node| node.layout.height),
                        Some(NativeSize::Hug)
                    );
                    assert_eq!(
                        resolved
                            .find("option-staging")
                            .and_then(|node| node.state.selected),
                        Some(true)
                    );
                }
                ComponentPreset::Drawer => {
                    assert_eq!(
                        ids,
                        [
                            "root",
                            "drawer-shell",
                            "drawer-scrim",
                            "drawer",
                            "drawer-header",
                            "drawer-title",
                            "drawer-close",
                            "drawer-content",
                            "drawer-copy",
                            "drawer-meta",
                        ]
                        .map(str::to_owned)
                    );
                    assert!(html.contains("id=\"drawer\" role=\"dialog\""));
                    assert_eq!(
                        component.node("drawer").and_then(|node| node.semantic_role),
                        Some(NativeSemanticRole::Dialog)
                    );
                    assert!(component.logic.iter().any(|logic| {
                        logic.source_node == "drawer-close"
                            && logic.target_state.as_deref() == Some("open")
                            && logic.value.as_deref() == Some("false")
                    }));
                }
                ComponentPreset::Scrollable => {
                    assert_eq!(
                        ids,
                        [
                            "root",
                            "scrollable",
                            "scroll-title",
                            "scroll-viewport",
                            "scroll-row-1",
                            "scroll-label-1",
                            "scroll-row-2",
                            "scroll-label-2",
                            "scroll-row-3",
                            "scroll-label-3",
                            "scroll-row-4",
                            "scroll-label-4",
                            "scroll-row-5",
                            "scroll-label-5",
                            "scroll-row-6",
                            "scroll-label-6",
                            "scroll-row-7",
                            "scroll-label-7",
                        ]
                        .map(str::to_owned)
                    );
                    assert!(html.contains("id=\"scroll-viewport\" role=\"region\""));
                    assert!(css.contains("#scroll-viewport"));
                    assert!(css.contains("overflow: auto;"));
                }
                ComponentPreset::Resizable => {
                    assert_eq!(
                        ids,
                        [
                            "root",
                            "resizable",
                            "primary-pane",
                            "primary-title",
                            "primary-copy",
                            "resize-handle",
                            "secondary-pane",
                            "secondary-title",
                            "secondary-copy",
                        ]
                        .map(str::to_owned)
                    );
                    assert!(html.contains("id=\"resize-handle\" role=\"slider\""));
                    assert!(html.contains("id=\"primary-pane\""));
                    assert!(html.contains("id=\"secondary-pane\""));
                    assert_eq!(
                        component
                            .node("resize-handle")
                            .and_then(|node| node.semantic_role),
                        Some(NativeSemanticRole::Slider)
                    );
                    assert_eq!(component.slots.len(), 2);
                    let resolved = component
                        .resolved_root(&BTreeMap::from([("split".to_owned(), "35".to_owned())]));
                    assert_eq!(
                        resolved.find("primary-pane").map(|node| node.layout.width),
                        Some(NativeSize::Fixed(204))
                    );
                    assert_eq!(
                        resolved
                            .find("secondary-pane")
                            .map(|node| node.layout.width),
                        Some(NativeSize::Fixed(384))
                    );
                }
                ComponentPreset::Button
                | ComponentPreset::Card
                | ComponentPreset::Badge
                | ComponentPreset::Alert
                | ComponentPreset::Toolbar
                | ComponentPreset::Avatar
                | ComponentPreset::EmptyState
                | ComponentPreset::Titlebar
                | ComponentPreset::Tabs
                | ComponentPreset::Dialog
                | ComponentPreset::DropdownMenu
                | ComponentPreset::Tooltip => {}
            }
        }
        assert!(library.validate().is_ok());
    }

    #[test]
    fn component_creation_skips_ids_that_survive_sparse_library_history() {
        let mut library = NativeComponentLibrary::default();
        let removed = library.create_named_component("Removed").id.clone();
        assert_eq!(library.create_named_component("Survivor").id, "component-3");
        assert!(library.remove_component(&removed).is_ok());

        let created = library.create_preset_component(ComponentPreset::Dropdown, "Dropdown");
        assert_eq!(created.id, "component-4");
        assert!(library.validate().is_ok());
    }

    #[test]
    fn validation_rejects_duplicate_node_ids_and_inert_state_transitions() {
        let mut duplicate = NativeComponentLibrary::default();
        duplicate.components[0]
            .root
            .children
            .push(NativeNode::text("title", "Duplicate", 0));
        assert!(matches!(
            duplicate.validate(),
            Err(NativeComponentError::Invalid(
                "node identifiers must be unique within a component"
            ))
        ));

        let mut incomplete = NativeComponentLibrary::default();
        incomplete.components[0].states.push(super::ComponentState {
            name: "open".to_owned(),
            value_type: "bool".to_owned(),
            default: "false".to_owned(),
        });
        incomplete.components[0].logic.push(ComponentLogic {
            id: "open".to_owned(),
            source_node: "action".to_owned(),
            event: ComponentEvent::Click,
            action: "open".to_owned(),
            guard: None,
            target_state: Some("open".to_owned()),
            value: None,
        });
        assert!(matches!(
            incomplete.validate(),
            Err(NativeComponentError::Invalid(
                "component logic graph is invalid"
            ))
        ));

        let mut invalid_guard = NativeComponentLibrary::default();
        invalid_guard.components[0]
            .states
            .push(super::ComponentState {
                name: "open".to_owned(),
                value_type: "bool".to_owned(),
                default: "false".to_owned(),
            });
        invalid_guard.components[0].logic.push(ComponentLogic {
            id: "open".to_owned(),
            source_node: "action".to_owned(),
            event: ComponentEvent::Click,
            action: "open".to_owned(),
            guard: Some("missing == false".to_owned()),
            target_state: Some("open".to_owned()),
            value: Some("true".to_owned()),
        });
        assert!(matches!(
            invalid_guard.validate(),
            Err(NativeComponentError::Invalid(
                "component logic graph is invalid"
            ))
        ));
    }

    #[test]
    fn user_components_round_trip_as_project_local_ron() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let mut library = NativeComponentLibrary::default();
        let created_id = library.create_component().id.clone();

        library.save(root.path())?;
        let reloaded = NativeComponentLibrary::load(root.path())?;

        assert_eq!(reloaded, library);
        assert_eq!(reloaded.active_component, created_id);
        Ok(())
    }

    #[test]
    fn rapid_repeated_saves_all_persist_the_latest_library()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let mut library = NativeComponentLibrary::default();
        // Back-to-back saves are the workload that surfaced transient Windows
        // replace failures; every one must land and the final read must match.
        for _ in 0..12 {
            library.create_component();
            library.save(root.path())?;
        }
        let reloaded = NativeComponentLibrary::load(root.path())?;
        assert_eq!(reloaded, library);
        Ok(())
    }

    #[test]
    fn transient_retry_recovers_before_exhausting_attempts() {
        let attempts = std::cell::Cell::new(0);
        let result = NativeComponentLibrary::with_transient_retry(|| {
            let seen = attempts.get() + 1;
            attempts.set(seen);
            if seen < 3 {
                Err(NativeComponentError::Invalid("transient"))
            } else {
                Ok(seen)
            }
        });
        assert!(matches!(result, Ok(3)));
        assert_eq!(attempts.get(), 3);
    }

    #[test]
    fn transient_retry_surfaces_the_error_after_exhausting_attempts() {
        let attempts = std::cell::Cell::new(0);
        let result = NativeComponentLibrary::with_transient_retry(|| {
            attempts.set(attempts.get() + 1);
            Err::<(), _>(NativeComponentError::Invalid("always fails"))
        });
        assert!(result.is_err());
        // One initial attempt plus one per backoff step; the operation is never
        // retried unbounded.
        assert_eq!(attempts.get(), 5);
    }

    #[test]
    fn v1_text_backgrounds_migrate_to_transparency() -> Result<(), Box<dyn std::error::Error>> {
        let root = TempDir::new()?;
        let directory = root.path().join(".gpui-studio");
        fs::create_dir_all(&directory)?;
        fs::write(
            directory.join("native-components.ron"),
            r#"(
                version: 1,
                active_component: "legacy",
                components: [(
                    id: "legacy",
                    name: "Legacy",
                    root: (
                        id: "root",
                        kind: Text,
                        layout: (
                            width: Hug,
                            height: Hug,
                            gap: 0,
                            padding: 0,
                            align: Start,
                            justify: Start,
                        ),
                        appearance: (
                            background: 0,
                            foreground: 16777215,
                            border: None,
                            radius: 0,
                        ),
                        text: Some("Legacy"),
                        action: None,
                        children: [],
                    ),
                )],
            )"#,
        )?;

        let library = NativeComponentLibrary::load(root.path())?;

        assert_eq!(library.version, COMPONENT_LIBRARY_VERSION);
        assert_eq!(library.components[0].root.appearance.background, None);
        Ok(())
    }

    #[test]
    fn current_documents_normalize_stale_native_only_vocabulary()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut library = NativeComponentLibrary::default();
        let component = &mut library.components[0];
        component.name = "Native card 1".to_owned();
        component.root.children[0].text = Some("DIRECT GPUI COMPONENT".to_owned());
        component.root.children[1].text = Some("Native card 1".to_owned());
        component.root.children[2].text = Some(
            "This tree renders as gpui::div elements without HTML or CSS translation.".to_owned(),
        );
        component.root.children[3].text = Some("Native action".to_owned());
        component.root.children[3].action = Some("native_action".to_owned());

        library.migrate_legacy_document()?;

        let component = &library.components[0];
        assert_eq!(component.name, "Component 1");
        assert_eq!(
            component.root.children[0].text.as_deref(),
            Some("UNIFIED COMPONENT DOCUMENT")
        );
        assert_eq!(
            component.root.children[1].text.as_deref(),
            Some("Component 1")
        );
        assert_eq!(
            component.root.children[2].text.as_deref(),
            Some("Edit the same component as HTML/CSS or GPUI; both projections stay in sync.")
        );
        assert_eq!(
            component.root.children[3].action.as_deref(),
            Some("component_action")
        );

        library.create_titlebar_component_with_props("Titlebar", Vec::new());
        library.components[1].root.children[0].layout.width = NativeSize::Hug;
        library.migrate_legacy_document()?;
        assert_eq!(
            library.components[1].root.children[0].layout.width,
            NativeSize::Fill
        );
        Ok(())
    }

    #[test]
    fn one_component_graph_projects_to_pure_html_css_ron_and_gpui() {
        let library = NativeComponentLibrary::default();
        let component = &library.components[0];

        let html = component.html_projection();
        assert!(html.contains("<div id=\"root\">"));
        assert!(html.contains("<button id=\"action\">Component action</button>"));
        assert!(!html.contains("hs-"));
        assert!(!html.contains("htmlswap"));

        let css = component.css_projection();
        assert!(css.contains("#root"));
        assert!(css.contains("display: flex"));

        let bindings = component.bindings_projection();
        assert!(bindings.contains("Id(\"action\")"));
        assert!(bindings.contains("handler: \"component_action\""));

        let gpui = component.gpui_excerpt();
        assert!(gpui.starts_with("div()\n    .flex()"));
    }

    #[test]
    fn tabs_resolve_live_state_into_visual_and_semantic_variants() {
        let mut library = NativeComponentLibrary::default();
        let component = library.create_preset_component(ComponentPreset::Tabs, "Tabs");
        let state = BTreeMap::from([("selected".to_owned(), "activity".to_owned())]);

        let resolved = component.resolved_root(&state);

        assert_eq!(
            resolved
                .find("tab-overview")
                .and_then(|node| node.state.selected),
            Some(false)
        );
        assert_eq!(
            resolved
                .find("tab-activity")
                .and_then(|node| node.state.selected),
            Some(true)
        );
        assert_eq!(
            resolved
                .find("panel-title")
                .and_then(|node| node.text.as_deref()),
            Some("Activity")
        );

        let bindings = component.bindings_projection();
        assert_eq!(bindings.matches("Id(\"tab-activity\")").count(), 1);
        assert!(bindings.contains("handler: \"select_activity\""));
        let gpui = component.gpui_excerpt();
        assert!(gpui.contains(&format!(".id(\"component/{}/tab-activity\")", component.id)));
        assert!(gpui.contains(".semantic_role(SemanticRole::Tab)"));
        assert!(gpui.contains(".semantic_metadata(\"action\", \"select_activity\")"));
    }

    #[test]
    fn semantic_state_and_every_nested_id_exist_in_both_source_projections() {
        let mut library = NativeComponentLibrary::default();
        let component = library.create_preset_component(ComponentPreset::Tabs, "Tabs");

        let html = component.html_projection();
        let gpui = component.gpui_excerpt();

        for id in [
            "root",
            "tabs",
            "tab-list",
            "tab-overview",
            "tab-activity",
            "tab-settings",
            "tab-panel",
            "panel-title",
            "panel-copy",
        ] {
            assert!(html.contains(&format!("id=\"{id}\"")), "HTML omitted {id}");
            assert!(
                gpui.contains(&format!("component/{}/{id}", component.id)),
                "GPUI omitted {id}"
            );
        }
        assert!(html.contains("role=\"tab\" aria-selected=\"true\""));
        assert!(html.contains("role=\"tab\" aria-selected=\"false\""));
    }
}
