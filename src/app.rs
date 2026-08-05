use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{
    AnyElement, App, AppContext as _, Application, AssetSource, Bounds, Context, FocusHandle,
    IntoElement, MouseButton, ParentElement as _, Render, SemanticElementExt as _, SemanticRole,
    SemanticValue, SharedString, Timer, Window, WindowBounds, WindowOptions, anchored, canvas,
    deferred, div, fill, point, prelude::*, px, rgb, rgba, size, svg,
};
use gpui_mcp::{
    AppId, ApplicationCommandDescriptor, ApplicationCommandRequest, ApplicationCommandResponse,
    ApplicationCommandResult, Automation, BridgeConfig, BridgeError, BridgeHandle, ContextResource,
    ContextResourceDescriptor, ContextResourceRequest, ContextResourceResponse, ErrorCode,
    NodeAction, Point as McpPoint, Rect, UiNode,
};
use gpui_mcp_html::{
    ComponentNode, ComponentRegistry, HandlerId, HookOutcome as ActionOutcome, HookRegistry,
    LiveHtmlSession, ProjectPaths, ProjectSnapshot, ProjectWatcher, SemanticNamespace,
    StateBindingId, StateValue,
};
use serde::Deserialize;

use crate::authoring::{
    NativeAlign, NativeConstraint, NativeNode, NativeNodeKind, NativeOverflow, NativePosition,
    NativeSize, NativeWrap, component_logic_guard_matches,
};
use crate::documents::{DocumentId, DocumentTabs};
use crate::output::persist_output_window_module;
use crate::resizable::{
    RESIZABLE_SPECS, Resizable, ResizeAxis, ResizeBounds, ResizeDrag, ResizePoint,
};
use crate::theme::ThemeWatcher;
use crate::{
    AnnotationHandoffStore, AnnotationStatus, AnnotationStore, AuthoringBackend, CanvasSettings,
    ChangeOrigin, ComponentEvent, ComponentGraph, ComponentLibrary, ComponentLogic,
    ComponentPointerGesture, ComponentPreset, ComponentProp, ComponentSlot, ComponentState,
    ComponentTransaction, ComponentVariant, ComponentVariantOverride, DesignToken, DesignTokenKind,
    DockTab, ElementSelection, GraphCommand, GraphError, HorizontalConstraint, InspectorTab,
    LayerKind, LayerRow, LayerTree, LayoutMode, NodePatch, NormalizedAnchor, OutputDecorations,
    PlacementEngine, ProjectStore, RevisionHistory, StudioMode, ThemeCatalog, ThemeLocation,
    ViewportPreset, WorkspaceSettings, available_canvas_with_rails,
};

const APP_ID: &str = "gpui-studio";
const TITLE: &str = "GPUI Studio";
const WINDOW_WIDTH: f32 = 1925.0;
const WINDOW_HEIGHT: f32 = 1048.0;

/// Process configuration for one local Studio window.
#[derive(Clone, Debug)]
pub struct StudioConfig {
    /// Repository containing Studio's dogfooded `ui/app.*` shell.
    pub studio_root: PathBuf,
    /// Pure-HTML GPUI project rendered inside the native canvas.
    pub project_root: PathBuf,
    /// Install the owner-restricted local MCP bridge.
    pub mcp_enabled: bool,
    /// Persisted editor-only workspace state.
    pub workspace: WorkspaceSettings,
    /// Ordered user then project theme layers.
    pub theme_locations: Vec<ThemeLocation>,
}

/// Start the native Studio application and block until its window closes.
pub fn run(config: StudioConfig) {
    Application::new()
        .with_assets(StudioAssets)
        .run(move |cx: &mut App| {
            gpui_mcp_html::init(cx);
            if let Err(error) = register_studio_fonts(cx) {
                eprintln!("could not register GPUI Studio fonts: {error:#}");
                cx.quit();
                return;
            }
            let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..WindowOptions::default()
                },
                {
                    let config = config.clone();
                    move |window, cx| {
                        window.set_window_title(TITLE);
                        let app = build_app(&config, window, cx).unwrap_or_else(|error| {
                            eprintln!("could not initialize GPUI Studio: {error:#}");
                            std::process::exit(1);
                        });
                        if app.bridge.is_none() {
                            eprintln!(
                                "GPUI Studio started without MCP; all project features remain local"
                            );
                        }
                        let keystroke_state = app.state.clone();
                        cx.observe_keystrokes(move |event, window, cx| {
                            keystroke_state.handle_global_keystroke(event, window, cx);
                        })
                        .detach();
                        let view = cx.new(|_| app);
                        let weak_view = view.downgrade();
                        window
                            .spawn(cx, async move |cx| {
                                loop {
                                    Timer::after(Duration::from_millis(50)).await;
                                    if weak_view
                                        .update(cx, |view, cx| {
                                            if view.poll() {
                                                cx.notify();
                                            }
                                        })
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            })
                            .detach();
                        cx.new(|cx| gpui_mcp_html::NativeRoot::new(view, window, cx))
                    }
                },
            );
            if let Err(error) = window {
                eprintln!("could not open GPUI Studio window: {error}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
}

struct StudioAssets;

impl AssetSource for StudioAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/select.svg" => Some(include_bytes!("../assets/icons/select.svg")),
            "icons/pan.svg" => Some(include_bytes!("../assets/icons/pan.svg")),
            "icons/play.svg" => Some(include_bytes!("../assets/icons/play.svg")),
            "icons/settings.svg" => Some(include_bytes!("../assets/icons/settings.svg")),
            "icons/comment.svg" => Some(include_bytes!("../assets/icons/comment.svg")),
            "icons/upload.svg" => Some(include_bytes!("../assets/icons/upload.svg")),
            "icons/layers.svg" => Some(include_bytes!("../assets/icons/layers.svg")),
            "icons/folder.svg" => Some(include_bytes!("../assets/icons/folder.svg")),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg")),
            "icons/close.svg" => Some(include_bytes!("../assets/icons/close.svg")),
            "icons/component.svg" => Some(include_bytes!("../assets/icons/component.svg")),
            "icons/frame.svg" => Some(include_bytes!("../assets/icons/frame.svg")),
            "icons/text.svg" => Some(include_bytes!("../assets/icons/text.svg")),
            "icons/duplicate.svg" => Some(include_bytes!("../assets/icons/duplicate.svg")),
            "icons/trash.svg" => Some(include_bytes!("../assets/icons/trash.svg")),
            "icons/row.svg" => Some(include_bytes!("../assets/icons/row.svg")),
            "icons/column.svg" => Some(include_bytes!("../assets/icons/column.svg")),
            "icons/button.svg" => Some(include_bytes!("../assets/icons/button.svg")),
            "icons/instance.svg" => Some(include_bytes!("../assets/icons/instance.svg")),
            "icons/chevron.svg" => Some(include_bytes!("../assets/icons/chevron.svg")),
            "icons/rotate.svg" => Some(include_bytes!("../assets/icons/rotate.svg")),
            "icons/monitor.svg" => Some(include_bytes!("../assets/icons/monitor.svg")),
            "icons/grid.svg" => Some(include_bytes!("../assets/icons/grid.svg")),
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        if path == "icons" {
            return Ok([
                "select.svg",
                "pan.svg",
                "comment.svg",
                "upload.svg",
                "layers.svg",
                "folder.svg",
                "plus.svg",
                "close.svg",
                "component.svg",
                "frame.svg",
                "text.svg",
                "row.svg",
                "column.svg",
                "button.svg",
                "instance.svg",
                "chevron.svg",
                "rotate.svg",
                "monitor.svg",
                "grid.svg",
            ]
            .into_iter()
            .map(SharedString::from)
            .collect());
        }
        Ok(Vec::new())
    }
}

fn register_studio_fonts(cx: &mut App) -> Result<()> {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/Geist[wght].ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/GeistMono[wght].ttf")),
        ])
        .context("register bundled Geist families")
}

struct StudioApp {
    shell: LiveHtmlSession,
    shell_watcher: ProjectWatcher,
    project_watcher: ProjectWatcher,
    theme_watcher: ThemeWatcher,
    applied_theme_revision: u64,
    project_reload_pending: bool,
    shell_reload_pending: bool,
    state: Rc<StudioState>,
    bridge: Option<BridgeHandle>,
}

impl StudioApp {
    fn poll(&mut self) -> bool {
        let mut changed = false;
        match self.project_watcher.poll() {
            Ok(Some(_)) => self.project_reload_pending = true,
            Ok(None) if self.project_reload_pending => {
                self.project_reload_pending = false;
                changed |= self.state.reload_external_if_changed();
            }
            Ok(None) => {}
            Err(error) => {
                self.state.set_status(format!("Project watcher: {error}"));
                changed = true;
            }
        }

        let mut reload_shell = self.applied_theme_revision != self.state.theme_revision();
        match self.shell_watcher.poll() {
            Ok(Some(_)) => self.shell_reload_pending = true,
            Ok(None) if self.shell_reload_pending => {
                self.shell_reload_pending = false;
                reload_shell = true;
            }
            Ok(None) => {}
            Err(error) => {
                self.state
                    .set_status(format!("Studio shell watcher: {error}"));
                changed = true;
            }
        }
        match self.theme_watcher.poll() {
            Ok(Some(catalog)) => {
                self.state.replace_theme_catalog(catalog);
                reload_shell = true;
            }
            Ok(None) => {}
            Err(error) => {
                self.state.set_status(format!("Theme watcher: {error}"));
                changed = true;
            }
        }
        if reload_shell {
            changed = true;
            self.reload_shell();
        }
        changed |= self.state.flush_component_edits_if_quiet();
        changed |= self.state.flush_annotation_if_quiet();
        changed | self.state.observe_runtime_revision()
    }

    fn reload_shell(&mut self) {
        let source = ProjectSnapshot::load(self.shell_watcher.paths())
            .map(ProjectSnapshot::into_document)
            .map(|mut source| {
                source
                    .css
                    .push_str(&self.state.resolved_theme().css_overlay());
                source
            });
        match source {
            Ok(source) => match self.shell.preview_source(self.shell.revision(), source) {
                Ok(preview) if preview.applied => {
                    self.applied_theme_revision = self.state.theme_revision();
                    self.state.set_status(format!(
                        "Studio shell and theme applied at revision {}",
                        preview.document.revision
                    ));
                }
                Ok(preview) => self.state.set_status(format!(
                    "Studio shell kept last-good preview: {}",
                    diagnostic_summary(&preview.diagnostics)
                )),
                Err(error) => self
                    .state
                    .set_status(format!("Studio shell conflict: {}", error.message)),
            },
            Err(error) => self.state.set_status(format!("Read Studio shell: {error}")),
        }
    }
}

impl Render for StudioApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.shell.render(window, cx)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StudioScrollSurface {
    Layers,
    Files,
    Inspector,
    States,
}

impl StudioScrollSurface {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "layers" => Some(Self::Layers),
            "files" => Some(Self::Files),
            "inspector" => Some(Self::Inspector),
            "states" => Some(Self::States),
            _ => None,
        }
    }

    fn semantic_id(self) -> &'static str {
        match self {
            Self::Layers => "layer-tree-scroll",
            Self::Files => "project-files-scroll",
            Self::Inspector => "inspector-panel-scroll",
            Self::States => "state-panel-scroll",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Layers => "Project layer tree",
            Self::Files => "Project files",
            Self::Inspector => "Selection inspector",
            Self::States => "Component states",
        }
    }
}

struct StudioScrollHandles {
    layers: gpui::ScrollHandle,
    files: gpui::ScrollHandle,
    inspector: gpui::ScrollHandle,
    states: gpui::ScrollHandle,
}

impl StudioScrollHandles {
    fn new() -> Self {
        Self {
            layers: gpui::ScrollHandle::new(),
            files: gpui::ScrollHandle::new(),
            inspector: gpui::ScrollHandle::new(),
            states: gpui::ScrollHandle::new(),
        }
    }

    fn handle(&self, surface: StudioScrollSurface) -> &gpui::ScrollHandle {
        match surface {
            StudioScrollSurface::Layers => &self.layers,
            StudioScrollSurface::Files => &self.files,
            StudioScrollSurface::Inspector => &self.inspector,
            StudioScrollSurface::States => &self.states,
        }
    }
}

struct StudioState {
    project: RefCell<ProjectStore>,
    target: RefCell<Option<LiveHtmlSession>>,
    history: RefCell<Option<RevisionHistory>>,
    status: RefCell<String>,
    mode: Cell<StudioMode>,
    backend: Cell<AuthoringBackend>,
    native_components: RefCell<ComponentLibrary>,
    component_runtime_state: RefCell<BTreeMap<String, BTreeMap<String, String>>>,
    component_graphs: RefCell<BTreeMap<String, ComponentGraph>>,
    component_edit_dirty: Cell<bool>,
    component_edit_quiet_ticks: Cell<u8>,
    inspector_prop_name: RefCell<String>,
    inspector_prop_type: RefCell<String>,
    inspector_prop_default: RefCell<String>,
    inspector_definition_drafts: RefCell<BTreeMap<DefinitionDraftField, String>>,
    inspector_slot_multiple: Cell<bool>,
    annotations: RefCell<AnnotationStore>,
    annotation_handoffs: RefCell<AnnotationHandoffStore>,
    annotation_draft: RefCell<String>,
    active_annotation: RefCell<Option<String>>,
    annotation_popover_open: Cell<bool>,
    annotation_dirty: Cell<bool>,
    annotation_quiet_ticks: Cell<u8>,
    left_files_open: Cell<bool>,
    component_dialog_open: Cell<bool>,
    document_tabs: RefCell<DocumentTabs>,
    component_name: RefCell<String>,
    component_props: RefCell<String>,
    component_authoring: Cell<AuthoringBackend>,
    component_source: Cell<ComponentSource>,
    component_preset: Cell<ComponentPreset>,
    selection: RefCell<ElementSelection>,
    multi_selection: RefCell<BTreeSet<String>>,
    multi_selection_anchor: RefCell<Option<String>>,
    selection_snapshot: RefCell<Option<SelectionSnapshot>>,
    project_tree_expanded: RefCell<BTreeSet<String>>,
    project_tree_initialized: Cell<bool>,
    project_tree_focus: RefCell<Option<String>>,
    project_tree_focus_handle: RefCell<Option<FocusHandle>>,
    component_nodes_expanded: RefCell<BTreeSet<String>>,
    component_tree_initialized_for: RefCell<Option<String>>,
    inspector_tab: Cell<InspectorTab>,
    dock_tab: Cell<DockTab>,
    dock_collapsed: Cell<bool>,
    context_menu: RefCell<Option<ContextMenuState>>,
    hovered_canvas_node: RefCell<Option<String>>,
    drag_preview: RefCell<Option<DragPreviewState>>,
    /// Canvas viewport pan offset (logical px), applied by the Move tool.
    canvas_pan: Cell<(f32, f32)>,
    /// Pointer position anchoring an in-progress Move-tool pan drag.
    pan_anchor: Cell<Option<(f32, f32)>>,
    /// Independent positions for the authored layers, files, inspector, and state surfaces.
    scroll_surfaces: StudioScrollHandles,
    /// Scroll position of the console event trace.
    console_scroll: gpui::ScrollHandle,
    /// Scroll position of the component palette list.
    palette_scroll: gpui::ScrollHandle,
    /// Whether the console is pinned to the latest entry (auto-scroll on).
    /// Cleared when the user scrolls up; restored on reaching the bottom or
    /// pressing the scroll-to-bottom control.
    console_pinned: Cell<bool>,
    /// Whether the Settings / Preferences overlay is open.
    settings_open: Cell<bool>,
    /// Whether the theme dropdown inside Settings is expanded.
    theme_dropdown_open: Cell<bool>,
    /// Whether the annotations / handoff-queue drawer is open.
    annotations_drawer_open: Cell<bool>,
    /// Scroll position of the annotations drawer list.
    drawer_scroll: gpui::ScrollHandle,
    resizable: Resizable,
    project_menu_open: Cell<bool>,
    viewport_menu_open: Cell<bool>,
    welcome_collapsed: Cell<bool>,
    header_collapsed: Cell<bool>,
    welcome_copy_collapsed: Cell<bool>,
    feature_cards_collapsed: Cell<bool>,
    welcome_lower_collapsed: Cell<bool>,
    component_tree_collapsed: Cell<bool>,
    horizontal_constraint: Cell<HorizontalConstraint>,
    canvas: Cell<CanvasSettings>,
    workspace: RefCell<WorkspaceSettings>,
    studio_root: PathBuf,
    themes: RefCell<ThemeCatalog>,
    theme_revision: Cell<u64>,
    automation: Automation,
}

#[derive(Clone, Debug)]
struct SelectionSnapshot {
    generation: u64,
    runtime_id: String,
    node: Option<UiNode>,
    project_canvas: Option<Rect>,
    app_window: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ComponentSource {
    Selection,
    Blank,
    PasteHtml,
    Titlebar,
    Preset,
}

/// An open right-click menu: window-relative position plus its target.
#[derive(Clone, Debug)]
struct ContextMenuState {
    x: f32,
    y: f32,
    target: ContextMenuTarget,
}

#[derive(Clone, Debug)]
enum ContextMenuTarget {
    /// A node in the active component graph.
    ComponentNode {
        component_id: String,
        node_id: String,
    },
    /// A layer of the legacy HTML project page.
    ProjectLayer,
    /// A component entry in the library palette.
    PaletteComponent { component_id: String },
}

/// One executable entry in the right-click menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContextMenuAction {
    OpenComponent,
    InsertFrame,
    InsertRow,
    InsertText,
    InsertButton,
    WrapInFrame,
    Duplicate,
    Delete,
    Annotate,
    DeleteComponent,
}

impl ContextMenuAction {
    const fn label(self) -> &'static str {
        match self {
            Self::OpenComponent => "Open component",
            Self::InsertFrame => "Insert frame",
            Self::InsertRow => "Insert row",
            Self::InsertText => "Insert text",
            Self::InsertButton => "Insert button",
            Self::WrapInFrame => "Wrap in frame",
            Self::Duplicate => "Duplicate",
            Self::Delete => "Delete",
            Self::Annotate => "Add annotation",
            Self::DeleteComponent => "Delete component",
        }
    }

    const fn slug(self) -> &'static str {
        match self {
            Self::OpenComponent => "open-component",
            Self::InsertFrame => "insert-frame",
            Self::InsertRow => "insert-row",
            Self::InsertText => "insert-text",
            Self::InsertButton => "insert-button",
            Self::WrapInFrame => "wrap-in-frame",
            Self::Duplicate => "duplicate",
            Self::Delete => "delete",
            Self::Annotate => "annotate",
            Self::DeleteComponent => "delete-component",
        }
    }

    /// Whether a separator renders above this entry.
    const fn leads_group(self) -> bool {
        matches!(
            self,
            Self::WrapInFrame | Self::Annotate | Self::InsertFrame | Self::DeleteComponent
        )
    }
}

/// Drag payload for a node picked up from the component layer tree.
#[derive(Clone, Debug)]
struct TreeNodeDrag {
    component_id: String,
    node_id: String,
    label: String,
}

/// Drag payload for a component picked up from the library palette.
#[derive(Clone, Debug)]
struct PaletteDrag {
    component_id: String,
    name: String,
}

/// Live placement preview while a drag hovers the canvas. `offset` is the
/// user's scroll/keyboard adjustment relative to the pointer-computed index;
/// in Stack containers child order is paint order, so the same adjustment
/// controls stacking.
#[derive(Clone, Debug, PartialEq)]
struct DragPreviewState {
    parent: String,
    base_index: usize,
    offset: i32,
    child_count: usize,
    horizontal: bool,
    /// Whether the target is a freeform Stack, where insertion order is paint
    /// order — i.e. the effective index is a true z-index.
    is_stack: bool,
    /// Last window-relative pointer position, so the order chip follows the
    /// cursor while dragging.
    pointer_x: f32,
    pointer_y: f32,
}

impl DragPreviewState {
    /// Insertion index after the scroll/keyboard adjustment, clamped to the
    /// container's real child range.
    fn effective_index(&self) -> usize {
        let base = i64::try_from(self.base_index).unwrap_or(i64::MAX);
        let adjusted = base.saturating_add(i64::from(self.offset)).max(0);
        usize::try_from(adjusted)
            .unwrap_or(usize::MAX)
            .min(self.child_count)
    }

    /// One-based drop position shown to the user; `(position, total_slots)`.
    fn position_label(&self) -> (usize, usize) {
        (
            self.effective_index().saturating_add(1),
            self.child_count.saturating_add(1),
        )
    }

    fn spec(&self) -> crate::DropPreviewSpec {
        crate::DropPreviewSpec {
            parent: self.parent.clone(),
            index: self.effective_index(),
            horizontal: self.horizontal,
        }
    }
}

struct EmptyDragGhost;

impl Render for EmptyDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().w(px(0.0)).h(px(0.0))
    }
}

/// Compact pill rendered under the cursor while dragging.
struct DragGhost {
    label: SharedString,
}

impl Render for DragGhost {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectorValueField {
    Width,
    Height,
    Gap,
    Padding,
    Margin,
    Basis,
    MinWidth,
    MaxWidth,
    MinHeight,
    MaxHeight,
    GridColumns,
    GridRows,
    ColumnStart,
    ColumnSpan,
    RowStart,
    RowSpan,
    OffsetLeft,
    OffsetTop,
    OffsetRight,
    OffsetBottom,
    ZIndex,
    Opacity,
    Rotation,
    Radius,
    Background,
    Foreground,
    Border,
    Text,
    FontFamily,
    FontSize,
    FontWeight,
    LineHeight,
    Action,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InspectorLayoutChoice {
    Wrap(NativeWrap),
    Position(NativePosition),
    Overflow(NativeOverflow),
    HorizontalConstraint(NativeConstraint),
    VerticalConstraint(NativeConstraint),
    Grow(bool),
    Shrink(bool),
}

#[derive(Clone, Copy, Debug)]
enum StudioInspectorDraftField {
    Name,
    Type,
    Default,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum DefinitionDraftField {
    StateName,
    StateType,
    StateDefault,
    VariantId,
    VariantName,
    SlotName,
    SlotNode,
    TokenPath,
    TokenKind,
    TokenValue,
    TokenDescription,
}

#[derive(Clone, Copy, Debug)]
enum InspectorSizeAxis {
    Width,
    Height,
}

#[derive(Clone, Copy, Debug)]
enum InspectorAlignmentAxis {
    Align,
    Justify,
}

struct StudioStateInit {
    project: ProjectStore,
    workspace: WorkspaceSettings,
    studio_root: PathBuf,
    native_components: ComponentLibrary,
    annotations: AnnotationStore,
    annotation_handoffs: AnnotationHandoffStore,
    themes: ThemeCatalog,
    automation: Automation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentGraphTarget {
    component_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentGraphMutation {
    component_id: String,
    transaction: ComponentTransaction,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateComponentCommand {
    name: String,
    preset: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectLayerCommand {
    runtime_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SuggestDropCommand {
    parent: String,
    layout: crate::LayoutMode,
    child_ids: Vec<String>,
    pointer: McpPoint,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapSelectionCommand {
    ids: Vec<String>,
    proposed_x: f32,
    proposed_y: f32,
    threshold: Option<f32>,
    grid: Option<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MarqueeSelectionCommand {
    rect: Rect,
    additive: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateAnnotationCommand {
    id: String,
    status: String,
}

impl StudioState {
    fn new(init: StudioStateInit) -> Self {
        let StudioStateInit {
            project,
            workspace,
            studio_root,
            native_components,
            annotations,
            annotation_handoffs,
            themes,
            automation,
        } = init;
        let revision = 1;
        let selection = initial_selection(workspace.backend, &native_components, revision);
        let component_graphs = native_components
            .components
            .iter()
            .map(|component| {
                (
                    component.id.clone(),
                    ComponentGraph::new(component.root.clone(), revision),
                )
            })
            .collect();
        let component_runtime_state = native_components
            .components
            .iter()
            .map(|component| {
                (
                    component.id.clone(),
                    component
                        .states
                        .iter()
                        .map(|state| (state.name.clone(), state.default.clone()))
                        .collect(),
                )
            })
            .collect();
        Self {
            project: RefCell::new(project),
            target: RefCell::new(None),
            history: RefCell::new(None),
            status: RefCell::new("Opening native project canvas…".to_owned()),
            mode: Cell::new(workspace.mode),
            backend: Cell::new(workspace.backend),
            native_components: RefCell::new(native_components),
            component_runtime_state: RefCell::new(component_runtime_state),
            component_graphs: RefCell::new(component_graphs),
            component_edit_dirty: Cell::new(false),
            component_edit_quiet_ticks: Cell::new(0),
            inspector_prop_name: RefCell::new(String::new()),
            inspector_prop_type: RefCell::new("String".to_owned()),
            inspector_prop_default: RefCell::new(String::new()),
            inspector_definition_drafts: RefCell::new(BTreeMap::from([
                (DefinitionDraftField::StateType, "bool".to_owned()),
                (DefinitionDraftField::StateDefault, "false".to_owned()),
                (DefinitionDraftField::TokenKind, "Color".to_owned()),
            ])),
            inspector_slot_multiple: Cell::new(false),
            annotations: RefCell::new(annotations),
            annotation_handoffs: RefCell::new(annotation_handoffs),
            annotation_draft: RefCell::new(String::new()),
            annotation_dirty: Cell::new(false),
            annotation_quiet_ticks: Cell::new(0),
            active_annotation: RefCell::new(None),
            annotation_popover_open: Cell::new(false),
            left_files_open: Cell::new(false),
            component_dialog_open: Cell::new(false),
            document_tabs: RefCell::new(DocumentTabs::new(selection.clone())),
            component_name: RefCell::new("NewComponent".to_owned()),
            component_props: RefCell::new(String::new()),
            component_authoring: Cell::new(AuthoringBackend::Html),
            component_source: Cell::new(ComponentSource::Selection),
            component_preset: Cell::new(ComponentPreset::Button),
            selection: RefCell::new(selection.clone()),
            multi_selection: RefCell::new(BTreeSet::from([selection.runtime_id.clone()])),
            multi_selection_anchor: RefCell::new(None),
            selection_snapshot: RefCell::new(None),
            project_tree_expanded: RefCell::new(BTreeSet::new()),
            project_tree_initialized: Cell::new(false),
            project_tree_focus: RefCell::new(None),
            project_tree_focus_handle: RefCell::new(None),
            component_nodes_expanded: RefCell::new(BTreeSet::new()),
            component_tree_initialized_for: RefCell::new(None),
            inspector_tab: Cell::new(InspectorTab::Layout),
            dock_tab: Cell::new(DockTab::Console),
            dock_collapsed: Cell::new(false),
            context_menu: RefCell::new(None),
            hovered_canvas_node: RefCell::new(None),
            drag_preview: RefCell::new(None),
            canvas_pan: Cell::new((0.0, 0.0)),
            pan_anchor: Cell::new(None),
            scroll_surfaces: StudioScrollHandles::new(),
            console_scroll: gpui::ScrollHandle::new(),
            palette_scroll: gpui::ScrollHandle::new(),
            console_pinned: Cell::new(true),
            settings_open: Cell::new(false),
            theme_dropdown_open: Cell::new(false),
            annotations_drawer_open: Cell::new(false),
            drawer_scroll: gpui::ScrollHandle::new(),
            resizable: Resizable::new(RESIZABLE_SPECS),
            project_menu_open: Cell::new(false),
            viewport_menu_open: Cell::new(false),
            welcome_collapsed: Cell::new(false),
            header_collapsed: Cell::new(false),
            welcome_copy_collapsed: Cell::new(false),
            feature_cards_collapsed: Cell::new(false),
            welcome_lower_collapsed: Cell::new(false),
            component_tree_collapsed: Cell::new(false),
            horizontal_constraint: Cell::new(HorizontalConstraint::Left),
            canvas: Cell::new(workspace.canvas),
            workspace: RefCell::new(workspace),
            studio_root,
            themes: RefCell::new(themes),
            theme_revision: Cell::new(1),
            automation,
        }
    }

    fn attach_target(&self, target: LiveHtmlSession) {
        let document = target.document();
        self.selection.borrow_mut().document_revision = document.revision;
        self.document_tabs
            .borrow_mut()
            .replace_active_selection(self.selection.borrow().clone());
        *self.history.borrow_mut() = Some(RevisionHistory::new(
            document.revision,
            document.source.clone(),
        ));
        *self.target.borrow_mut() = Some(target);
        self.set_status(format!(
            "Ready · project revision {} · native preview",
            document.revision
        ));
    }

    fn target(&self) -> Option<LiveHtmlSession> {
        self.target.borrow().clone()
    }

    fn set_status(&self, status: impl Into<String>) {
        *self.status.borrow_mut() = status.into();
    }

    /// Record a bracketed, categorized entry in the live automation trace.
    fn trace(&self, level: &str, message: &str) {
        self.automation.log(level, message);
    }

    fn status(&self) -> String {
        self.status.borrow().clone()
    }

    fn project_path(&self) -> String {
        let root = self.project.borrow().root().display().to_string();
        let studio_root = self.studio_root.display().to_string();
        let root = root.strip_prefix(r"\\?\").unwrap_or(&root);
        let studio_root = studio_root
            .strip_prefix(r"\\?\")
            .unwrap_or(&studio_root)
            .trim_end_matches(['\\', '/']);
        root.strip_prefix(studio_root)
            .map(|relative| relative.trim_start_matches(['\\', '/']))
            .filter(|relative| !relative.is_empty())
            .unwrap_or(root)
            .to_owned()
    }

    fn revision_label(&self) -> String {
        self.target().map_or_else(
            || "REV —".to_owned(),
            |target| format!("REV {}", target.revision()),
        )
    }

    fn dirty_label(&self) -> String {
        let Some(target) = self.target() else {
            return "NO DOCUMENT".to_owned();
        };
        if self.project.borrow().is_dirty(&target.document().source) {
            "UNSAVED PREVIEW".to_owned()
        } else {
            "SAVED".to_owned()
        }
    }

    fn mode_label(&self) -> String {
        format!("{:?}", self.mode.get()).to_uppercase()
    }

    fn backend_label(&self) -> String {
        match self.backend.get() {
            AuthoringBackend::Html => "HTML PROJECTION".to_owned(),
            AuthoringBackend::Gpui => "GPUI PROJECTION".to_owned(),
        }
    }

    fn backend_html_selected(&self) -> bool {
        self.backend.get() == AuthoringBackend::Html
    }

    fn backend_gpui_selected(&self) -> bool {
        self.backend.get() == AuthoringBackend::Gpui
    }

    fn mode_design_selected(&self) -> bool {
        self.mode.get() == StudioMode::Design
    }

    fn mode_test_selected(&self) -> bool {
        self.mode.get() == StudioMode::Test
    }

    fn mode_compare_selected(&self) -> bool {
        self.mode.get() == StudioMode::Compare
    }

    // Toolbar tool group (active editing tool within the Design view).
    fn tool_select_selected(&self) -> bool {
        self.mode.get() == StudioMode::Design
    }

    fn tool_move_selected(&self) -> bool {
        self.mode.get() == StudioMode::Move
    }

    fn tool_annotate_selected(&self) -> bool {
        self.mode.get() == StudioMode::Compare
    }

    // Toolbar view group: Design (any editing tool) vs Preview (run the app).
    fn view_design_selected(&self) -> bool {
        matches!(
            self.mode.get(),
            StudioMode::Design | StudioMode::Move | StudioMode::Compare
        )
    }

    fn view_preview_selected(&self) -> bool {
        self.mode.get() == StudioMode::Test
    }

    /// Switch to the Design view, restoring the Select tool. Used by the view
    /// group's "Design" button when leaving Preview.
    fn enter_design_view(&self) {
        if !self.view_design_selected() {
            self.set_mode(StudioMode::Design);
        }
    }

    /// Activate the Move (pan) tool.
    fn enter_move_tool(&self) {
        self.set_mode(StudioMode::Move);
    }

    fn component_summary(&self) -> String {
        let library = self.native_components.borrow();
        let name = library
            .active()
            .map_or("No component", |component| component.name.as_str());
        format!("{} · {} total", name, library.components.len())
    }

    fn active_component_name(&self) -> String {
        self.native_components.borrow().active().map_or_else(
            || "Component".to_owned(),
            |component| component.name.clone(),
        )
    }

    fn selection_label(&self) -> String {
        let selection = self.selection.borrow();
        format!("#{}", selection.authored_id)
    }

    fn selection_runtime_id(&self) -> String {
        self.selection.borrow().runtime_id.clone()
    }

    fn selection_heading(&self) -> String {
        self.selection
            .borrow()
            .authored_id
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut characters = part.chars();
                characters.next().map_or_else(String::new, |first| {
                    first.to_uppercase().collect::<String>() + characters.as_str()
                })
            })
            .collect()
    }

    fn selection_element_tag(&self) -> String {
        let snapshot = self.selection_snapshot();
        let tag = snapshot
            .node
            .as_ref()
            .and_then(|node| node.metadata.get("html_tag"))
            .or_else(|| {
                snapshot
                    .node
                    .as_ref()
                    .and_then(|node| node.metadata.get("gpui_kind"))
            })
            .map_or_else(|| "element".to_owned(), Clone::clone);
        format!("<{tag}>")
    }

    fn selection_snapshot(&self) -> SelectionSnapshot {
        let runtime_id = self.selection.borrow().runtime_id.clone();
        let generation = self.automation.semantic_generation();
        if let Some(snapshot) = self.selection_snapshot.borrow().as_ref()
            && snapshot.generation == generation
            && snapshot.runtime_id == runtime_id
        {
            return snapshot.clone();
        }

        let tree = self.automation.snapshot();
        let snapshot = SelectionSnapshot {
            generation: tree.generation,
            node: tree.nodes.get(&runtime_id).cloned(),
            project_canvas: tree
                .nodes
                .get("project-canvas")
                .and_then(|node| node.bounds),
            app_window: tree.nodes.get("app-window").and_then(|node| node.bounds),
            runtime_id,
        };
        self.selection.borrow_mut().captured_rect =
            snapshot.node.as_ref().and_then(|node| node.bounds);
        *self.selection_snapshot.borrow_mut() = Some(snapshot.clone());
        snapshot
    }

    fn current_selection_rect(&self) -> Option<Rect> {
        self.selection_snapshot().node.and_then(|node| node.bounds)
    }

    fn selection_rect_label(&self) -> String {
        self.current_selection_rect().map_or_else(
            || "not painted".to_owned(),
            |rect| {
                format!(
                    "x {:.0} · y {:.0} · w {:.0} · h {:.0}",
                    rect.x, rect.y, rect.width, rect.height
                )
            },
        )
    }

    fn selection_width_label(&self) -> String {
        self.current_selection_rect()
            .map_or_else(|| "—".to_owned(), |rect| format!("{:.0}", rect.width))
    }

    fn selection_height_label(&self) -> String {
        self.current_selection_rect()
            .map_or_else(|| "—".to_owned(), |rect| format!("{:.0}", rect.height))
    }

    fn selection_relative_rect(&self) -> Option<Rect> {
        let snapshot = self.selection_snapshot();
        let selection = snapshot.node?.bounds?;
        let viewport = snapshot.project_canvas.or(snapshot.app_window)?;
        Some(Rect {
            x: selection.x - viewport.x,
            y: selection.y - viewport.y,
            width: selection.width,
            height: selection.height,
        })
    }

    fn selection_x_label(&self) -> String {
        self.selection_relative_rect()
            .map_or_else(|| "—".to_owned(), |rect| format!("{:.0}", rect.x))
    }

    fn selection_y_label(&self) -> String {
        self.selection_relative_rect()
            .map_or_else(|| "—".to_owned(), |rect| format!("{:.0}", rect.y))
    }

    fn selection_rotation_label(&self) -> String {
        "0°".to_owned()
    }

    fn selection_grid_label(&self) -> String {
        let canvas = self.canvas.get();
        if canvas.snap_enabled {
            format!("{}px", canvas.snap_grid)
        } else {
            "off".to_owned()
        }
    }

    fn select_horizontal_constraint(&self, constraint: HorizontalConstraint) {
        self.horizontal_constraint.set(constraint);
        let resolved = self
            .selection_relative_rect()
            .map(|rect| constraint.resolve(838.0, 388.0, rect.x, rect.width));
        self.set_status(resolved.map_or_else(
            || format!("{} constraint selected", constraint.label()),
            |(x, width)| {
                format!(
                    "{} constraint selected · mobile reflow x {:.0}, width {:.0}",
                    constraint.label(),
                    x,
                    width
                )
            },
        ));
    }

    fn constraint_left_selected(&self) -> bool {
        self.horizontal_constraint.get() == HorizontalConstraint::Left
    }

    fn constraint_center_selected(&self) -> bool {
        self.horizontal_constraint.get() == HorizontalConstraint::Center
    }

    fn constraint_right_selected(&self) -> bool {
        self.horizontal_constraint.get() == HorizontalConstraint::Right
    }

    fn constraint_scale_selected(&self) -> bool {
        self.horizontal_constraint.get() == HorizontalConstraint::Scale
    }

    fn selection_size_label(&self) -> String {
        self.current_selection_rect().map_or_else(
            || "awaiting paint".to_owned(),
            |rect| format!("{:.0} × {:.0}", rect.width, rect.height),
        )
    }

    fn annotation_count_label(&self) -> String {
        format!("MCP · {} resources", studio_resource_uris().len())
    }

    fn annotation_title(&self) -> String {
        self.active_annotation.borrow().as_ref().map_or_else(
            || "New annotation".to_owned(),
            |id| format!("Annotation {}", annotation_number(id)),
        )
    }

    fn annotation_target_label(&self) -> String {
        let authored_id = self
            .active_annotation
            .borrow()
            .as_ref()
            .and_then(|id| {
                self.annotations
                    .borrow()
                    .annotations
                    .iter()
                    .find(|annotation| annotation.id == *id)
                    .map(|annotation| annotation.target.authored_id.clone())
            })
            .unwrap_or_else(|| self.selection.borrow().authored_id.clone());
        format!("on {}", authored_name(&authored_id))
    }

    fn annotation_status_label(&self) -> String {
        self.active_annotation.borrow().as_ref().map_or_else(
            || "Saved as you type".to_owned(),
            |id| {
                self.annotations
                    .borrow()
                    .annotations
                    .iter()
                    .find(|annotation| annotation.id == *id)
                    .map_or_else(
                        || "Saved as you type".to_owned(),
                        |annotation| format!("Saved · {:?} · Send to hand off", annotation.status),
                    )
            },
        )
    }

    fn send_annotations_label(&self) -> String {
        let count = self.annotations.borrow().counts().active;
        format!("Send {count}")
    }

    fn annotation_popover_visible(&self) -> bool {
        self.annotation_popover_open.get()
    }

    fn annotation_has_saved_comment(&self) -> bool {
        self.active_annotation.borrow().is_some()
    }

    fn send_annotations_visible(&self) -> bool {
        self.annotations.borrow().has_sendable()
    }

    fn layers_visible(&self) -> bool {
        !self.left_files_open.get()
    }

    fn files_visible(&self) -> bool {
        self.left_files_open.get()
    }

    fn show_layers(&self) {
        self.left_files_open.set(false);
    }

    fn show_files(&self) {
        self.left_files_open.set(true);
    }

    fn inspector_layout_visible(&self) -> bool {
        self.inspector_tab.get() == InspectorTab::Layout
    }

    fn inspector_style_visible(&self) -> bool {
        self.inspector_tab.get() == InspectorTab::Style
    }

    fn inspector_logic_visible(&self) -> bool {
        self.inspector_tab.get() == InspectorTab::Logic
    }

    fn dock_console_selected(&self) -> bool {
        self.dock_tab.get() == DockTab::Console
    }

    fn dock_states_selected(&self) -> bool {
        self.dock_tab.get() == DockTab::States
    }

    fn set_inspector_tab(&self, tab: InspectorTab) {
        self.inspector_tab.set(tab);
        self.set_status(format!(
            "{} inspector opened",
            format!("{tab:?}").to_lowercase()
        ));
    }

    fn selected_component_node(&self) -> Option<NativeNode> {
        let component_id = self.active_component_id()?;
        let node_id = self.selection.borrow().authored_id.clone();
        self.native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.id == component_id)
            .and_then(|component| component.node(&node_id))
            .cloned()
    }

    fn mutate_selected_component_node(
        &self,
        description: &str,
        update: impl FnOnce(&mut NativeNode),
    ) -> Result<(), String> {
        let component_id = self
            .active_component_id()
            .ok_or_else(|| "Open a component document to edit canonical values".to_owned())?;
        let node_id = self.selection.borrow().authored_id.clone();
        let mut updated = self
            .native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.id == component_id)
            .and_then(|component| component.node(&node_id))
            .cloned()
            .ok_or_else(|| "The selected component node no longer exists".to_owned())?;
        update(&mut updated);
        self.synchronize_component_graph(&component_id)
            .map_err(|error| error.message)?;
        let root = {
            let mut graphs = self.component_graphs.borrow_mut();
            let graph = graphs
                .get_mut(&component_id)
                .ok_or_else(|| "The component graph was not initialized".to_owned())?;
            graph
                .apply(ComponentTransaction {
                    expected_revision: graph.revision(),
                    actor: "studio.inspector".to_owned(),
                    commands: vec![GraphCommand::Patch {
                        id: node_id,
                        patch: NodePatch {
                            kind: Some(updated.kind),
                            semantic_role: Some(updated.semantic_role),
                            state: Some(updated.state),
                            layout: Some(updated.layout),
                            appearance: Some(updated.appearance),
                            typography: Some(updated.typography),
                            text: Some(updated.text),
                            action: Some(updated.action),
                        },
                    }],
                })
                .map_err(|error| error.to_string())?;
            graph.root().clone()
        };
        self.publish_component_graph(&component_id, root)
            .map_err(|error| error.message)?;
        self.set_status(format!("{description} · live graph updated"));
        Ok(())
    }

    /// Apply structural graph commands through the same revision-checked
    /// transaction engine the inspector uses, then publish the new root.
    fn apply_graph_commands(
        &self,
        component_id: &str,
        description: &str,
        commands: Vec<GraphCommand>,
    ) -> Result<(), String> {
        self.synchronize_component_graph(component_id)
            .map_err(|error| error.message)?;
        let root = {
            let mut graphs = self.component_graphs.borrow_mut();
            let graph = graphs
                .get_mut(component_id)
                .ok_or_else(|| "The component graph was not initialized".to_owned())?;
            graph
                .apply(ComponentTransaction {
                    expected_revision: graph.revision(),
                    actor: "studio.editor".to_owned(),
                    commands,
                })
                .map_err(|error| error.to_string())?;
            graph.root().clone()
        };
        self.publish_component_graph(component_id, root)
            .map_err(|error| error.message)?;
        self.set_status(format!("{description} · live graph updated"));
        self.trace("info", &format!("[graph] {description} · {component_id}"));
        Ok(())
    }

    /// Resolve where a newly inserted node should land relative to the current
    /// selection: inside the selection when it accepts children, otherwise as
    /// the following sibling; falling back to the component root.
    fn insertion_target(&self, component: &crate::NativeComponent) -> (String, usize) {
        let selected = self.selection.borrow().authored_id.clone();
        if component.node_accepts_children(&selected) {
            (selected, usize::MAX)
        } else if let Some((parent, index)) = component.parent_and_index(&selected) {
            (parent, index.saturating_add(1))
        } else {
            ("root".to_owned(), usize::MAX)
        }
    }

    /// Build a node with a unique id and insert it at `placement`, or near the
    /// current selection when no explicit placement is given.
    fn insert_authored_node(
        &self,
        prefix: &str,
        description: &str,
        placement: Option<(String, usize)>,
        make: impl FnOnce(&str) -> NativeNode,
    ) -> Result<String, String> {
        let component_id = self
            .active_component_id()
            .ok_or_else(|| "Open a component document to insert nodes".to_owned())?;
        let (parent, index, node) = {
            let library = self.native_components.borrow();
            let component = library
                .component(&component_id)
                .ok_or_else(|| "The active component no longer exists".to_owned())?;
            let id = component.unique_node_id(prefix);
            let (parent, index) = placement.unwrap_or_else(|| self.insertion_target(component));
            (parent, index, make(&id))
        };
        let new_id = node.id.clone();
        self.apply_graph_commands(
            &component_id,
            description,
            vec![GraphCommand::Insert {
                parent,
                index,
                node,
            }],
        )?;
        self.set_selection(ElementSelection::new(
            new_id.clone(),
            format!("component/{component_id}/{new_id}"),
            1,
            None,
        ));
        Ok(new_id)
    }

    fn insert_frame_node(&self) {
        self.insert_primitive_node(NativeNodeKind::Column);
    }

    fn insert_row_node(&self) {
        self.insert_primitive_node(NativeNodeKind::Row);
    }

    fn insert_text_node(&self) {
        self.insert_primitive_node(NativeNodeKind::Text);
    }

    fn insert_button_node(&self) {
        self.insert_primitive_node(NativeNodeKind::Button);
    }

    fn insert_primitive_node(&self, kind: NativeNodeKind) {
        let (prefix, label) = match kind {
            NativeNodeKind::Row => ("row", "Row"),
            NativeNodeKind::Column => ("column", "Column"),
            NativeNodeKind::Grid => ("grid", "Grid"),
            NativeNodeKind::Stack => ("stack", "Stack"),
            NativeNodeKind::Text => ("text", "Text"),
            NativeNodeKind::Button => ("button", "Button"),
            NativeNodeKind::Titlebar => ("titlebar", "Titlebar"),
            NativeNodeKind::Instance => ("instance", "Instance"),
        };
        let description = format!("Inserted {label}");
        let result = self.insert_authored_node(prefix, &description, None, |id| match kind {
            NativeNodeKind::Text => NativeNode::authored_text(id),
            NativeNodeKind::Button => NativeNode::authored_button(id),
            other => NativeNode::authored_container(id, other),
        });
        if let Err(error) = result {
            self.set_status(format!("Insert {label} failed: {error}"));
        }
    }

    /// Place an instance of `referenced_id` at an explicit `(parent, index)`
    /// or near the current selection, rejecting composition cycles.
    fn insert_component_instance_at(
        &self,
        referenced_id: &str,
        placement: Option<(String, usize)>,
    ) {
        let Some(active) = self.active_component_id() else {
            self.set_status("Open a component document to place an instance");
            return;
        };
        let (name, cycles) = {
            let library = self.native_components.borrow();
            let name = library
                .component(referenced_id)
                .map(|component| component.name.clone());
            (name, library.would_cycle(&active, referenced_id))
        };
        let Some(name) = name else {
            self.set_status("That component no longer exists");
            return;
        };
        if cycles {
            self.set_status(format!(
                "Cannot place {name} here: it would create a component cycle"
            ));
            self.trace("warn", &format!("[compose] refused cycle placing {name}"));
            return;
        }
        let referenced = referenced_id.to_owned();
        let description = format!("Placed {name} instance");
        if let Err(error) = self.insert_authored_node("instance", &description, placement, |id| {
            NativeNode::authored_instance(id, &referenced)
        }) {
            self.set_status(format!("Place {name} failed: {error}"));
        }
    }

    /// Where a drop **onto** a tree row should land: into the row when it can
    /// hold children, otherwise directly after it, correcting for same-parent
    /// index shift after the dragged node is removed.
    fn tree_drop_placement(
        &self,
        component_id: &str,
        dragged: Option<&str>,
        target_node: &str,
    ) -> Option<(String, usize)> {
        let library = self.native_components.borrow();
        let component = library.component(component_id)?;
        if component.node_accepts_children(target_node) {
            return Some((target_node.to_owned(), usize::MAX));
        }
        let (parent, target_index) = component.parent_and_index(target_node)?;
        let dragged_earlier = dragged
            .and_then(|id| component.parent_and_index(id))
            .is_some_and(|(dragged_parent, dragged_index)| {
                dragged_parent == parent && dragged_index < target_index
            });
        // After the dragged node leaves the parent, later siblings shift left.
        let index = if dragged_earlier {
            target_index
        } else {
            target_index.saturating_add(1)
        };
        Some((parent, index))
    }

    /// Whether `drag` may drop onto `target_node` without corrupting the tree.
    fn can_drop_tree_node(&self, drag: &TreeNodeDrag, target_node: &str) -> bool {
        if drag.node_id == "root" || drag.node_id == target_node {
            return false;
        }
        if self.active_component_id().as_deref() != Some(drag.component_id.as_str()) {
            return false;
        }
        let library = self.native_components.borrow();
        library
            .component(&drag.component_id)
            .is_some_and(|component| {
                component.node(target_node).is_some()
                    && !component.node_contains(&drag.node_id, target_node)
            })
    }

    /// Move a dragged tree node onto `target_node` through the transaction
    /// engine, keeping the moved node selected.
    fn drop_tree_node(&self, drag: &TreeNodeDrag, target_node: &str) {
        if !self.can_drop_tree_node(drag, target_node) {
            self.set_status("That move would break the component tree");
            return;
        }
        let Some((parent, index)) =
            self.tree_drop_placement(&drag.component_id, Some(&drag.node_id), target_node)
        else {
            self.set_status("The drop target no longer exists");
            return;
        };
        if let Err(error) = self.apply_graph_commands(
            &drag.component_id,
            &format!("Moved {}", drag.label),
            vec![GraphCommand::Move {
                id: drag.node_id.clone(),
                parent,
                index,
            }],
        ) {
            self.set_status(format!("Move failed: {error}"));
            return;
        }
        self.set_selection(ElementSelection::new(
            drag.node_id.clone(),
            format!("component/{}/{}", drag.component_id, drag.node_id),
            1,
            None,
        ));
    }

    /// Move a dragged tree node to a canvas position: the deepest container
    /// under the pointer at the visual-order index.
    fn drop_tree_node_at_canvas(&self, drag: &TreeNodeDrag, pointer: McpPoint) {
        let Some((parent, index)) = self.canvas_drop_placement(pointer) else {
            self.trace(
                "warn",
                &format!(
                    "[move] drop node={} at ({:.0},{:.0}) → no container under pointer",
                    drag.node_id, pointer.x, pointer.y
                ),
            );
            self.set_status("Drop over a container to move the node there");
            return;
        };
        self.trace(
            "info",
            &format!(
                "[move] drop node={} → parent={parent} index={index}",
                drag.node_id
            ),
        );
        if parent == drag.node_id || drag.node_id == "root" {
            self.set_status("A node cannot move into itself");
            return;
        }
        let invalid = self
            .native_components
            .borrow()
            .component(&drag.component_id)
            .is_none_or(|component| component.node_contains(&drag.node_id, &parent));
        if invalid {
            self.set_status("A node cannot move into its own subtree");
            return;
        }
        if let Err(error) = self.apply_graph_commands(
            &drag.component_id,
            &format!("Moved {}", drag.label),
            vec![GraphCommand::Move {
                id: drag.node_id.clone(),
                parent,
                index,
            }],
        ) {
            self.set_status(format!("Move failed: {error}"));
        }
    }

    /// Move a canvas node grabbed with the mouse to the drop position. The
    /// rendered id is resolved to its stored graph node (so grabbing inside an
    /// expanded instance moves the instance), then reused through the tree-move
    /// path.
    fn drop_component_node_at_canvas(&self, drag: &crate::ComponentNodeDrag, pointer: McpPoint) {
        let node_id = self.resolve_component_node(&drag.component_id, &drag.node_id);
        let moved = TreeNodeDrag {
            component_id: drag.component_id.clone(),
            node_id: node_id.clone(),
            label: node_id,
        };
        self.drop_tree_node_at_canvas(&moved, pointer);
    }

    /// Drop a palette component onto a tree row: instance lands in or after
    /// the row.
    fn drop_palette_on_node(&self, drag: &PaletteDrag, target_node: &str) {
        let Some(component_id) = self.active_component_id() else {
            return;
        };
        let placement = self.tree_drop_placement(&component_id, None, target_node);
        self.insert_component_instance_at(&drag.component_id, placement);
    }

    /// Resolve a canvas pointer position to a live drag target using the
    /// semantic bounds: the deepest stored container under the pointer wins,
    /// and the visual-order placement engine picks the insertion index.
    fn canvas_drag_target(&self, pointer: McpPoint) -> Option<DragPreviewState> {
        let component_id = self.active_component_id()?;
        let library = self.native_components.borrow();
        let component = library.component(&component_id)?;
        let tree = self.automation.snapshot();
        let prefix = format!("component/{component_id}/");
        let mut best: Option<(String, f32)> = None;
        for (runtime_id, node) in &tree.nodes {
            let Some(authored) = runtime_id.strip_prefix(&prefix) else {
                continue;
            };
            let Some(bounds) = node.bounds else {
                continue;
            };
            let inside = pointer.x >= bounds.x
                && pointer.x <= bounds.x + bounds.width
                && pointer.y >= bounds.y
                && pointer.y <= bounds.y + bounds.height;
            if !inside || !component.node_accepts_children(authored) {
                continue;
            }
            let area = bounds.width * bounds.height;
            if best.as_ref().is_none_or(|(_, smallest)| area < *smallest) {
                best = Some((authored.to_owned(), area));
            }
        }
        let (parent_authored, _) = best?;
        let parent_node = component.node(&parent_authored)?;
        let horizontal = matches!(
            parent_node.kind,
            NativeNodeKind::Row | NativeNodeKind::Titlebar
        );
        let layout = match parent_node.kind {
            NativeNodeKind::Row | NativeNodeKind::Titlebar => LayoutMode::Flex {
                axis: crate::Axis::Horizontal,
                reverse: false,
            },
            NativeNodeKind::Grid => LayoutMode::Grid {
                columns: usize::from(parent_node.layout.grid_columns.max(1)),
            },
            NativeNodeKind::Stack => LayoutMode::Freeform,
            _ => LayoutMode::Flex {
                axis: crate::Axis::Vertical,
                reverse: false,
            },
        };
        let child_ids = parent_node
            .children
            .iter()
            .map(|child| format!("{prefix}{}", child.id))
            .collect::<Vec<_>>();
        let engine = PlacementEngine::rebuild(
            tree.nodes
                .iter()
                .filter_map(|(id, node)| node.bounds.map(|bounds| (id.clone(), bounds))),
        );
        let placement = engine.drop_placement(
            format!("{prefix}{parent_authored}"),
            layout,
            &child_ids,
            pointer,
        );
        Some(DragPreviewState {
            parent: parent_authored,
            base_index: placement.index,
            offset: 0,
            child_count: child_ids.len(),
            horizontal,
            is_stack: parent_node.kind == NativeNodeKind::Stack,
            pointer_x: pointer.x,
            pointer_y: pointer.y,
        })
    }

    /// Resolve a canvas drop position to `(parent, index)`, honoring the live
    /// preview (including any scroll/keyboard order adjustment) when one is
    /// active so the drop commits exactly what the user saw.
    fn canvas_drop_placement(&self, pointer: McpPoint) -> Option<(String, usize)> {
        // Finalize the preview at the exact drop point (keeping any scroll
        // offset) so the drop lands under the pointer even for interpolated
        // semantic drags whose last preview frame stops short of the endpoint.
        self.update_drag_preview(pointer);
        if let Some(preview) = self.drag_preview.borrow().as_ref() {
            return Some((preview.parent.clone(), preview.effective_index()));
        }
        let target = self.canvas_drag_target(pointer)?;
        let index = target.effective_index();
        Some((target.parent, index))
    }

    /// Track a drag hovering the canvas: recompute the target container and
    /// index, preserving the user's order adjustment while the container is
    /// unchanged. Returns whether the preview changed.
    fn update_drag_preview(&self, pointer: McpPoint) -> bool {
        let next = self.canvas_drag_target(pointer);
        let mut current = self.drag_preview.borrow_mut();
        match (&mut *current, next) {
            (Some(current), Some(mut next)) => {
                if current.parent == next.parent {
                    next.offset = current.offset;
                }
                if *current == next {
                    false
                } else {
                    *current = next;
                    true
                }
            }
            (current @ Some(_), None) => {
                *current = None;
                true
            }
            (current @ None, Some(next)) => {
                *current = Some(next);
                true
            }
            (None, None) => false,
        }
    }

    /// Nudge the pending drop's order within its container (flex/grid order;
    /// stacking order in a Stack). Returns whether a preview was adjusted.
    fn adjust_drag_preview_order(&self, delta: i32) -> bool {
        let mut preview = self.drag_preview.borrow_mut();
        let Some(preview) = preview.as_mut() else {
            return false;
        };
        let before = preview.effective_index();
        preview.offset = preview.offset.saturating_add(delta);
        let after = preview.effective_index();
        self.set_status(format!(
            "Drop position {} of {} · scroll or [ ] to adjust",
            after + 1,
            preview.child_count + 1
        ));
        before != after
    }

    /// Current pixel size of a declared resizable surface.
    fn panel_size(&self, target: &str) -> f32 {
        self.resizable.size(target).unwrap_or_default()
    }

    /// Resize the surface identified by the active drag payload. The target's
    /// current bounds come from the last semantic frame, so all listeners use
    /// the active payload discriminator rather than their captured handle.
    fn resize_panel(&self, drag: &ResizeDrag, pointer_x: f32, pointer_y: f32) {
        let tree = self.automation.snapshot();
        let Some(bounds) = tree.nodes.get(&drag.target).and_then(|node| node.bounds) else {
            return;
        };
        self.resizable.resize(
            drag,
            ResizeBounds {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
            },
            ResizePoint {
                x: pointer_x,
                y: pointer_y,
            },
        );
    }

    /// Set a declared surface size through slider-like semantic automation.
    fn set_panel_size(&self, target: &str, size: f32) -> bool {
        self.resizable.set_size(target, size)
    }

    /// Reset a panel to its declared default size (double-click a resize handle).
    fn reset_panel(&self, target: &str) {
        if self.resizable.reset(target) {
            self.set_status(format!("Reset {target} panel to default size"));
        }
    }

    fn clear_drag_preview(&self) -> bool {
        self.drag_preview.borrow_mut().take().is_some()
    }

    fn delete_selected_node(&self) {
        let Some(component_id) = self.active_component_id() else {
            self.set_status("Open a component document to delete nodes");
            return;
        };
        let selected = self.selection.borrow().authored_id.clone();
        if selected == "root" {
            self.set_status("The component root cannot be deleted");
            return;
        }
        let parent = {
            let library = self.native_components.borrow();
            let Some(component) = library.component(&component_id) else {
                self.set_status("The active component no longer exists");
                return;
            };
            if component.node(&selected).is_none() {
                self.set_status("Nothing is selected to delete");
                return;
            }
            component
                .parent_and_index(&selected)
                .map_or_else(|| "root".to_owned(), |(parent, _)| parent)
        };
        if let Err(error) = self.apply_graph_commands(
            &component_id,
            "Deleted node",
            vec![GraphCommand::Remove {
                id: selected.clone(),
            }],
        ) {
            self.set_status(format!("Delete failed: {error}"));
            return;
        }
        self.set_selection(ElementSelection::new(
            parent.clone(),
            format!("component/{component_id}/{parent}"),
            1,
            None,
        ));
    }

    fn duplicate_selected_node(&self) {
        let Some(component_id) = self.active_component_id() else {
            self.set_status("Open a component document to duplicate nodes");
            return;
        };
        let selected = self.selection.borrow().authored_id.clone();
        let (parent, index, suffix, new_id) = {
            let library = self.native_components.borrow();
            let Some(component) = library.component(&component_id) else {
                self.set_status("The active component no longer exists");
                return;
            };
            let Some((parent, index)) = component.parent_and_index(&selected) else {
                self.set_status("Select a non-root node to duplicate");
                return;
            };
            let mut suffix = "-copy".to_owned();
            let mut counter = 1;
            while component.node(&format!("{selected}{suffix}")).is_some() {
                counter += 1;
                suffix = format!("-copy-{counter}");
            }
            let new_id = format!("{selected}{suffix}");
            (parent, index.saturating_add(1), suffix, new_id)
        };
        if let Err(error) = self.apply_graph_commands(
            &component_id,
            "Duplicated node",
            vec![GraphCommand::Duplicate {
                id: selected,
                parent,
                index,
                suffix,
            }],
        ) {
            self.set_status(format!("Duplicate failed: {error}"));
            return;
        }
        self.set_selection(ElementSelection::new(
            new_id.clone(),
            format!("component/{component_id}/{new_id}"),
            1,
            None,
        ));
    }

    fn flush_component_edits_if_quiet(&self) -> bool {
        if !self.component_edit_dirty.get() {
            return false;
        }
        let quiet_ticks = self.component_edit_quiet_ticks.get().saturating_add(1);
        self.component_edit_quiet_ticks.set(quiet_ticks);
        if quiet_ticks < 2 {
            return false;
        }
        let root = self.project.borrow().root().to_owned();
        match self.native_components.borrow().save(&root) {
            Ok(()) => {
                self.component_edit_dirty.set(false);
                self.component_edit_quiet_ticks.set(0);
                self.set_status("Inspector edit saved · runtime stayed live");
            }
            Err(error) => {
                self.component_edit_quiet_ticks.set(0);
                self.set_status(format!("Persist inspector edit failed: {error}"));
            }
        }
        true
    }

    fn inspector_width_value(&self) -> String {
        self.inspector_axis_value(InspectorSizeAxis::Width)
    }

    fn inspector_height_value(&self) -> String {
        self.inspector_axis_value(InspectorSizeAxis::Height)
    }

    fn inspector_axis_value(&self, axis: InspectorSizeAxis) -> String {
        let measured = self.current_selection_rect().map(|rect| match axis {
            InspectorSizeAxis::Width => rect.width,
            InspectorSizeAxis::Height => rect.height,
        });
        self.selected_component_node().map_or_else(
            || measured.map_or_else(|| "0".to_owned(), rounded_pixels),
            |node| {
                let size = match axis {
                    InspectorSizeAxis::Width => node.layout.width,
                    InspectorSizeAxis::Height => node.layout.height,
                };
                match size {
                    NativeSize::Fixed(value) => value.to_string(),
                    NativeSize::Fill | NativeSize::Hug => {
                        measured.map_or_else(|| "0".to_owned(), rounded_pixels)
                    }
                }
            },
        )
    }

    fn inspector_axis_intrinsic(&self, axis: InspectorSizeAxis) -> bool {
        self.selected_component_node().is_some_and(|node| {
            matches!(
                match axis {
                    InspectorSizeAxis::Width => node.layout.width,
                    InspectorSizeAxis::Height => node.layout.height,
                },
                NativeSize::Fill | NativeSize::Hug
            )
        })
    }

    fn inspector_gap_value(&self) -> String {
        self.selected_component_node()
            .map_or_else(|| "0".to_owned(), |node| node.layout.gap.to_string())
    }

    fn inspector_padding_value(&self) -> String {
        self.selected_component_node()
            .map_or_else(|| "0".to_owned(), |node| node.layout.padding.to_string())
    }

    fn inspector_radius_value(&self) -> String {
        self.selected_component_node()
            .map_or_else(|| "0".to_owned(), |node| node.appearance.radius.to_string())
    }

    fn inspector_background_value(&self) -> String {
        self.selected_component_node().map_or_else(
            || "transparent".to_owned(),
            |node| format_optional_color(node.appearance.background),
        )
    }

    fn inspector_foreground_value(&self) -> String {
        self.selected_component_node().map_or_else(
            || "#e7e9ee".to_owned(),
            |node| format!("#{:06x}", node.appearance.foreground),
        )
    }

    fn inspector_border_value(&self) -> String {
        self.selected_component_node().map_or_else(
            || "transparent".to_owned(),
            |node| format_optional_color(node.appearance.border),
        )
    }

    fn inspector_text_value(&self) -> String {
        self.selected_component_node()
            .and_then(|node| node.text)
            .unwrap_or_default()
    }

    fn inspector_font_value(&self) -> String {
        self.selected_component_node()
            .map_or_else(|| "Geist".to_owned(), |node| node.typography.family)
    }

    fn inspector_font_size_value(&self) -> String {
        self.selected_component_node()
            .map_or_else(|| "14".to_owned(), |node| node.typography.size.to_string())
    }

    fn inspector_font_weight_value(&self) -> String {
        self.selected_component_node().map_or_else(
            || "400".to_owned(),
            |node| node.typography.weight.to_string(),
        )
    }

    fn inspector_line_height_value(&self) -> String {
        self.selected_component_node().map_or_else(
            || "20".to_owned(),
            |node| node.typography.line_height.to_string(),
        )
    }

    fn inspector_action_value(&self) -> String {
        self.selected_component_node()
            .and_then(|node| node.action)
            .unwrap_or_default()
    }

    fn inspector_component_editable(&self) -> bool {
        self.selected_component_node().is_some()
    }

    fn inspector_page_notice_visible(&self) -> bool {
        !self.inspector_component_editable()
    }

    fn inspector_draft(&self, field: StudioInspectorDraftField) -> String {
        match field {
            StudioInspectorDraftField::Name => self.inspector_prop_name.borrow().clone(),
            StudioInspectorDraftField::Type => self.inspector_prop_type.borrow().clone(),
            StudioInspectorDraftField::Default => self.inspector_prop_default.borrow().clone(),
        }
    }

    fn set_inspector_draft(&self, field: StudioInspectorDraftField, value: String) {
        match field {
            StudioInspectorDraftField::Name => {
                *self.inspector_prop_name.borrow_mut() = value;
            }
            StudioInspectorDraftField::Type => {
                *self.inspector_prop_type.borrow_mut() = value;
            }
            StudioInspectorDraftField::Default => {
                *self.inspector_prop_default.borrow_mut() = value;
            }
        }
    }

    fn definition_draft(&self, field: DefinitionDraftField) -> String {
        self.inspector_definition_drafts
            .borrow()
            .get(&field)
            .cloned()
            .unwrap_or_default()
    }

    fn set_definition_draft(&self, field: DefinitionDraftField, value: String) {
        self.inspector_definition_drafts
            .borrow_mut()
            .insert(field, value);
    }

    fn mutate_component_definition(
        &self,
        description: &str,
        update: impl FnOnce(&mut crate::NativeComponent),
    ) -> Result<(), String> {
        let component_id = self
            .active_component_id()
            .ok_or_else(|| "Open a component document to edit its definition".to_owned())?;
        let mut candidate = self.native_components.borrow().clone();
        let component = candidate
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
            .ok_or_else(|| "The active component no longer exists".to_owned())?;
        update(component);
        candidate
            .save(self.project.borrow().root())
            .map_err(|error| error.to_string())?;
        *self.native_components.borrow_mut() = candidate;
        self.set_status(format!("{description} · component definition saved"));
        Ok(())
    }

    fn save_component_state(&self) {
        let name = self.definition_draft(DefinitionDraftField::StateName);
        let value_type = self.definition_draft(DefinitionDraftField::StateType);
        let default = self.definition_draft(DefinitionDraftField::StateDefault);
        if !is_portable_identifier(name.trim()) || !is_portable_type(value_type.trim()) {
            self.set_status("State requires a portable name and scalar type");
            return;
        }
        let state = ComponentState {
            name: name.trim().to_owned(),
            value_type: value_type.trim().to_owned(),
            default,
        };
        if let Err(error) = self.mutate_component_definition("State updated", |component| {
            if let Some(existing) = component
                .states
                .iter_mut()
                .find(|existing| existing.name == state.name)
            {
                *existing = state;
            } else {
                component.states.push(state);
            }
        }) {
            self.set_status(error);
        }
    }

    fn remove_component_state(&self) {
        let name = self.definition_draft(DefinitionDraftField::StateName);
        if let Err(error) = self.mutate_component_definition("State removed", |component| {
            component.states.retain(|state| state.name != name.trim());
            for logic in &mut component.logic {
                if logic.target_state.as_deref() == Some(name.trim()) {
                    logic.target_state = None;
                    logic.value = None;
                }
            }
        }) {
            self.set_status(error);
        }
    }

    fn save_component_variant(&self) {
        let id = self.definition_draft(DefinitionDraftField::VariantId);
        let name = self.definition_draft(DefinitionDraftField::VariantName);
        if !is_portable_identifier(id.trim()) || name.trim().is_empty() || name.len() > 128 {
            self.set_status("Variant requires a portable ID and a display name");
            return;
        }
        let id = id.trim().to_owned();
        let name = name.trim().to_owned();
        if let Err(error) = self.mutate_component_definition("Variant updated", |component| {
            if let Some(existing) = component
                .variants
                .iter_mut()
                .find(|variant| variant.id == id)
            {
                existing.name = name;
            } else {
                component.variants.push(ComponentVariant {
                    id,
                    name,
                    overrides: Vec::new(),
                });
            }
        }) {
            self.set_status(error);
        }
    }

    fn capture_variant_override(&self) {
        let variant_id = self.definition_draft(DefinitionDraftField::VariantId);
        let Some(node) = self.selected_component_node() else {
            self.set_status("Select a component node before capturing an override");
            return;
        };
        let variant_exists = self
            .native_components
            .borrow()
            .active()
            .is_some_and(|component| {
                component
                    .variants
                    .iter()
                    .any(|variant| variant.id == variant_id.trim())
            });
        if !variant_exists {
            self.set_status("Save the variant before capturing a node override");
            return;
        }
        if let Err(error) =
            self.mutate_component_definition("Variant override captured", |component| {
                let Some(variant) = component
                    .variants
                    .iter_mut()
                    .find(|variant| variant.id == variant_id.trim())
                else {
                    return;
                };
                let override_value = ComponentVariantOverride {
                    node_id: node.id.clone(),
                    layout: Some(node.layout),
                    appearance: Some(node.appearance),
                    typography: Some(node.typography.clone()),
                    text: Some(node.text.clone()),
                    state: Some(node.state),
                };
                if let Some(existing) = variant
                    .overrides
                    .iter_mut()
                    .find(|existing| existing.node_id == node.id)
                {
                    *existing = override_value;
                } else {
                    variant.overrides.push(override_value);
                }
            })
        {
            self.set_status(error);
        }
    }

    fn remove_component_variant(&self) {
        let id = self.definition_draft(DefinitionDraftField::VariantId);
        if let Err(error) = self.mutate_component_definition("Variant removed", |component| {
            component.variants.retain(|variant| variant.id != id.trim());
        }) {
            self.set_status(error);
        }
    }

    fn save_component_slot(&self) {
        let name = self.definition_draft(DefinitionDraftField::SlotName);
        let node_id = self.definition_draft(DefinitionDraftField::SlotNode);
        if !is_portable_identifier(name.trim()) || !is_portable_identifier(node_id.trim()) {
            self.set_status("Slot requires a portable name and container node ID");
            return;
        }
        let slot = ComponentSlot {
            name: name.trim().to_owned(),
            node_id: node_id.trim().to_owned(),
            multiple: self.inspector_slot_multiple.get(),
            accepted_kinds: Vec::new(),
        };
        if let Err(error) = self.mutate_component_definition("Slot updated", |component| {
            if let Some(existing) = component
                .slots
                .iter_mut()
                .find(|existing| existing.name == slot.name)
            {
                *existing = slot;
            } else {
                component.slots.push(slot);
            }
        }) {
            self.set_status(error);
        }
    }

    fn toggle_slot_multiple(&self) {
        self.inspector_slot_multiple
            .set(!self.inspector_slot_multiple.get());
    }

    fn remove_component_slot(&self) {
        let name = self.definition_draft(DefinitionDraftField::SlotName);
        if let Err(error) = self.mutate_component_definition("Slot removed", |component| {
            component.slots.retain(|slot| slot.name != name.trim());
        }) {
            self.set_status(error);
        }
    }

    fn save_design_token(&self) {
        let path = self.definition_draft(DefinitionDraftField::TokenPath);
        let kind = self.definition_draft(DefinitionDraftField::TokenKind);
        let value = self.definition_draft(DefinitionDraftField::TokenValue);
        let description = self.definition_draft(DefinitionDraftField::TokenDescription);
        let kind = match kind.trim().to_ascii_lowercase().as_str() {
            "color" => DesignTokenKind::Color,
            "number" => DesignTokenKind::Number,
            "typography" => DesignTokenKind::Typography,
            "string" => DesignTokenKind::String,
            _ => {
                self.set_status("Token kind must be Color, Number, Typography, or String");
                return;
            }
        };
        if !is_portable_identifier(path.trim()) || value.trim().is_empty() {
            self.set_status("Token requires a dot-delimited portable path and value");
            return;
        }
        let mut candidate = self.native_components.borrow().clone();
        let token = DesignToken {
            path: path.trim().to_owned(),
            kind,
            value,
            description,
        };
        if let Some(existing) = candidate
            .tokens
            .iter_mut()
            .find(|existing| existing.path == token.path)
        {
            *existing = token;
        } else {
            candidate.tokens.push(token);
        }
        match candidate.save(self.project.borrow().root()) {
            Ok(()) => {
                *self.native_components.borrow_mut() = candidate;
                self.set_status("Design token updated · available to every projection");
            }
            Err(error) => self.set_status(format!("Design token rejected: {error}")),
        }
    }

    fn remove_design_token(&self) {
        let path = self.definition_draft(DefinitionDraftField::TokenPath);
        let mut candidate = self.native_components.borrow().clone();
        candidate.tokens.retain(|token| token.path != path.trim());
        match candidate.save(self.project.borrow().root()) {
            Ok(()) => {
                *self.native_components.borrow_mut() = candidate;
                self.set_status("Design token removed");
            }
            Err(error) => self.set_status(format!("Remove design token rejected: {error}")),
        }
    }

    fn inspector_value(&self, field: InspectorValueField) -> String {
        match field {
            InspectorValueField::Width => self.inspector_width_value(),
            InspectorValueField::Height => self.inspector_height_value(),
            InspectorValueField::Gap => self.inspector_gap_value(),
            InspectorValueField::Padding => self.inspector_padding_value(),
            InspectorValueField::Margin => self.selected_component_node().map_or_else(
                || "0".to_owned(),
                |node| {
                    let edges = node.layout.margin;
                    if edges.top == edges.right
                        && edges.top == edges.bottom
                        && edges.top == edges.left
                    {
                        edges.top.to_string()
                    } else {
                        "mixed".to_owned()
                    }
                },
            ),
            InspectorValueField::Basis => optional_u16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.basis),
            ),
            InspectorValueField::MinWidth => optional_u16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.min_width),
            ),
            InspectorValueField::MaxWidth => optional_u16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.max_width),
            ),
            InspectorValueField::MinHeight => optional_u16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.min_height),
            ),
            InspectorValueField::MaxHeight => optional_u16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.max_height),
            ),
            InspectorValueField::GridColumns => self.selected_component_node().map_or_else(
                || "0".to_owned(),
                |node| node.layout.grid_columns.to_string(),
            ),
            InspectorValueField::GridRows => self
                .selected_component_node()
                .map_or_else(|| "0".to_owned(), |node| node.layout.grid_rows.to_string()),
            InspectorValueField::ColumnStart => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.column_start),
            ),
            InspectorValueField::ColumnSpan => self.selected_component_node().map_or_else(
                || "1".to_owned(),
                |node| node.layout.column_span.to_string(),
            ),
            InspectorValueField::RowStart => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.row_start),
            ),
            InspectorValueField::RowSpan => self
                .selected_component_node()
                .map_or_else(|| "1".to_owned(), |node| node.layout.row_span.to_string()),
            InspectorValueField::OffsetLeft => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.offsets.left),
            ),
            InspectorValueField::OffsetTop => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.offsets.top),
            ),
            InspectorValueField::OffsetRight => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.offsets.right),
            ),
            InspectorValueField::OffsetBottom => optional_i16_value(
                self.selected_component_node()
                    .and_then(|node| node.layout.offsets.bottom),
            ),
            InspectorValueField::ZIndex => self
                .selected_component_node()
                .map_or_else(|| "0".to_owned(), |node| node.layout.z_index.to_string()),
            InspectorValueField::Opacity => self.selected_component_node().map_or_else(
                || "100".to_owned(),
                |node| node.layout.opacity_percent.to_string(),
            ),
            InspectorValueField::Rotation => self.selected_component_node().map_or_else(
                || "0".to_owned(),
                |node| node.layout.rotation_degrees.to_string(),
            ),
            InspectorValueField::Radius => self.inspector_radius_value(),
            InspectorValueField::Background => self.inspector_background_value(),
            InspectorValueField::Foreground => self.inspector_foreground_value(),
            InspectorValueField::Border => self.inspector_border_value(),
            InspectorValueField::Text => self.inspector_text_value(),
            InspectorValueField::FontFamily => self.inspector_font_value(),
            InspectorValueField::FontSize => self.inspector_font_size_value(),
            InspectorValueField::FontWeight => self.inspector_font_weight_value(),
            InspectorValueField::LineHeight => self.inspector_line_height_value(),
            InspectorValueField::Action => self.inspector_action_value(),
        }
    }

    fn set_inspector_value(&self, field: InspectorValueField, value: String) -> Result<(), String> {
        let current = self.inspector_value(field);
        if current == value
            || (matches!(
                field,
                InspectorValueField::Background
                    | InspectorValueField::Foreground
                    | InspectorValueField::Border
            ) && current.eq_ignore_ascii_case(value.trim()))
        {
            return Ok(());
        }
        match field {
            InspectorValueField::Width | InspectorValueField::Height => {
                let value = parse_bounded_u16(&value, 1, 4_096, "size")?;
                self.mutate_selected_component_node("Size changed", |node| match field {
                    InspectorValueField::Width => node.layout.width = NativeSize::Fixed(value),
                    InspectorValueField::Height => node.layout.height = NativeSize::Fixed(value),
                    _ => {}
                })
            }
            InspectorValueField::Gap
            | InspectorValueField::Padding
            | InspectorValueField::Margin
            | InspectorValueField::Radius => {
                let value = parse_bounded_u16(&value, 0, 1_024, "spacing")?;
                self.mutate_selected_component_node("Spacing changed", |node| match field {
                    InspectorValueField::Gap => node.layout.gap = value,
                    InspectorValueField::Padding => node.layout.padding = value,
                    InspectorValueField::Margin => {
                        node.layout.margin.top = value;
                        node.layout.margin.right = value;
                        node.layout.margin.bottom = value;
                        node.layout.margin.left = value;
                    }
                    InspectorValueField::Radius => node.appearance.radius = value,
                    _ => {}
                })
            }
            InspectorValueField::Basis
            | InspectorValueField::MinWidth
            | InspectorValueField::MaxWidth
            | InspectorValueField::MinHeight
            | InspectorValueField::MaxHeight => {
                let value = parse_optional_bounded_u16(&value, 0, 4_096, "layout size")?;
                self.mutate_selected_component_node("Layout bound changed", |node| match field {
                    InspectorValueField::Basis => node.layout.basis = value,
                    InspectorValueField::MinWidth => node.layout.min_width = value,
                    InspectorValueField::MaxWidth => node.layout.max_width = value,
                    InspectorValueField::MinHeight => node.layout.min_height = value,
                    InspectorValueField::MaxHeight => node.layout.max_height = value,
                    _ => {}
                })
            }
            InspectorValueField::GridColumns | InspectorValueField::GridRows => {
                let value = parse_bounded_u16(&value, 0, 64, "grid track count")?;
                self.mutate_selected_component_node("Grid tracks changed", |node| match field {
                    InspectorValueField::GridColumns => node.layout.grid_columns = value,
                    InspectorValueField::GridRows => node.layout.grid_rows = value,
                    _ => {}
                })
            }
            InspectorValueField::ColumnSpan | InspectorValueField::RowSpan => {
                let value = parse_bounded_u16(&value, 1, 64, "grid span")?;
                self.mutate_selected_component_node("Grid span changed", |node| match field {
                    InspectorValueField::ColumnSpan => node.layout.column_span = value,
                    InspectorValueField::RowSpan => node.layout.row_span = value,
                    _ => {}
                })
            }
            InspectorValueField::ColumnStart
            | InspectorValueField::RowStart
            | InspectorValueField::OffsetLeft
            | InspectorValueField::OffsetTop
            | InspectorValueField::OffsetRight
            | InspectorValueField::OffsetBottom => {
                let value = parse_optional_bounded_i16(&value, -16_384, 16_384, "position")?;
                self.mutate_selected_component_node("Position changed", |node| match field {
                    InspectorValueField::ColumnStart => node.layout.column_start = value,
                    InspectorValueField::RowStart => node.layout.row_start = value,
                    InspectorValueField::OffsetLeft => node.layout.offsets.left = value,
                    InspectorValueField::OffsetTop => node.layout.offsets.top = value,
                    InspectorValueField::OffsetRight => node.layout.offsets.right = value,
                    InspectorValueField::OffsetBottom => node.layout.offsets.bottom = value,
                    _ => {}
                })
            }
            InspectorValueField::ZIndex | InspectorValueField::Rotation => {
                let value = parse_bounded_i16(&value, -32_767, 32_767, "transform")?;
                self.mutate_selected_component_node("Transform changed", |node| match field {
                    InspectorValueField::ZIndex => node.layout.z_index = value,
                    InspectorValueField::Rotation => node.layout.rotation_degrees = value,
                    _ => {}
                })
            }
            InspectorValueField::Opacity => {
                let value = parse_bounded_u16(&value, 0, 100, "opacity")? as u8;
                self.mutate_selected_component_node("Opacity changed", |node| {
                    node.layout.opacity_percent = value;
                })
            }
            InspectorValueField::Background
            | InspectorValueField::Foreground
            | InspectorValueField::Border => {
                let color = parse_optional_color(&value)?;
                if field == InspectorValueField::Foreground && color.is_none() {
                    return Err("Foreground requires a #RRGGBB color".to_owned());
                }
                self.mutate_selected_component_node("Color changed", |node| match field {
                    InspectorValueField::Background => node.appearance.background = color,
                    InspectorValueField::Foreground => {
                        if let Some(color) = color {
                            node.appearance.foreground = color;
                        }
                    }
                    InspectorValueField::Border => node.appearance.border = color,
                    _ => {}
                })
            }
            InspectorValueField::Text => {
                if value.len() > 16 * 1_024 {
                    return Err("Text exceeds the 16 KiB component bound".to_owned());
                }
                self.mutate_selected_component_node("Content changed", |node| {
                    node.text = (!value.is_empty()).then_some(value);
                })
            }
            InspectorValueField::FontFamily => {
                let family = value.trim();
                if family.is_empty() || family.len() > 128 {
                    return Err("Font family must contain 1–128 characters".to_owned());
                }
                self.mutate_selected_component_node("Font family changed", |node| {
                    family.clone_into(&mut node.typography.family);
                })
            }
            InspectorValueField::FontSize => {
                let value = parse_bounded_u16(&value, 6, 512, "font size")?;
                self.mutate_selected_component_node("Font size changed", |node| {
                    node.typography.size = value;
                })
            }
            InspectorValueField::FontWeight => {
                let value = parse_bounded_u16(&value, 100, 900, "font weight")?;
                if value % 50 != 0 {
                    return Err("Font weight must use increments of 50".to_owned());
                }
                self.mutate_selected_component_node("Font weight changed", |node| {
                    node.typography.weight = value;
                })
            }
            InspectorValueField::LineHeight => {
                let value = parse_bounded_u16(&value, 6, 1_024, "line height")?;
                self.mutate_selected_component_node("Line height changed", |node| {
                    node.typography.line_height = value;
                })
            }
            InspectorValueField::Action => {
                let action = value.trim();
                if !action.is_empty() && !is_portable_identifier(action) {
                    return Err("Action must use a portable identifier".to_owned());
                }
                self.mutate_selected_component_node("Action changed", |node| {
                    node.action = (!action.is_empty()).then(|| action.to_owned());
                })
            }
        }
    }

    fn set_inspector_size(&self, axis: InspectorSizeAxis, size: NativeSize) {
        let size = if size == NativeSize::Fixed(0) {
            let measured = self.current_selection_rect().map(|rect| match axis {
                InspectorSizeAxis::Width => rect.width,
                InspectorSizeAxis::Height => rect.height,
            });
            let pixels = measured.unwrap_or(100.0).round().clamp(1.0, 4_096.0);
            NativeSize::Fixed(format!("{pixels:.0}").parse::<u16>().unwrap_or(100))
        } else {
            size
        };
        let _ = self.mutate_selected_component_node("Sizing policy changed", |node| match axis {
            InspectorSizeAxis::Width => node.layout.width = size,
            InspectorSizeAxis::Height => node.layout.height = size,
        });
    }

    fn inspector_size_selected(&self, axis: InspectorSizeAxis, size: NativeSize) -> bool {
        self.selected_component_node().is_some_and(|node| {
            let current = match axis {
                InspectorSizeAxis::Width => node.layout.width,
                InspectorSizeAxis::Height => node.layout.height,
            };
            matches!(
                (current, size),
                (NativeSize::Fill, NativeSize::Fill)
                    | (NativeSize::Hug, NativeSize::Hug)
                    | (NativeSize::Fixed(_), NativeSize::Fixed(_))
            )
        })
    }

    fn set_inspector_alignment(&self, axis: InspectorAlignmentAxis, value: NativeAlign) {
        let _ = self.mutate_selected_component_node("Alignment changed", |node| match axis {
            InspectorAlignmentAxis::Align => node.layout.align = value,
            InspectorAlignmentAxis::Justify => node.layout.justify = value,
        });
    }

    fn inspector_alignment_selected(
        &self,
        axis: InspectorAlignmentAxis,
        value: NativeAlign,
    ) -> bool {
        self.selected_component_node().is_some_and(|node| {
            (match axis {
                InspectorAlignmentAxis::Align => node.layout.align,
                InspectorAlignmentAxis::Justify => node.layout.justify,
            }) == value
        })
    }

    fn set_inspector_layout_choice(&self, choice: InspectorLayoutChoice) {
        let _ =
            self.mutate_selected_component_node("Layout behavior changed", |node| match choice {
                InspectorLayoutChoice::Wrap(value) => node.layout.wrap = value,
                InspectorLayoutChoice::Position(value) => node.layout.position = value,
                InspectorLayoutChoice::Overflow(value) => node.layout.overflow = value,
                InspectorLayoutChoice::HorizontalConstraint(value) => {
                    node.layout.horizontal_constraint = value;
                }
                InspectorLayoutChoice::VerticalConstraint(value) => {
                    node.layout.vertical_constraint = value;
                }
                InspectorLayoutChoice::Grow(value) => node.layout.grow = value,
                InspectorLayoutChoice::Shrink(value) => node.layout.shrink = value,
            });
    }

    fn inspector_layout_choice_selected(&self, choice: InspectorLayoutChoice) -> bool {
        self.selected_component_node()
            .is_some_and(|node| match choice {
                InspectorLayoutChoice::Wrap(value) => node.layout.wrap == value,
                InspectorLayoutChoice::Position(value) => node.layout.position == value,
                InspectorLayoutChoice::Overflow(value) => node.layout.overflow == value,
                InspectorLayoutChoice::HorizontalConstraint(value) => {
                    node.layout.horizontal_constraint == value
                }
                InspectorLayoutChoice::VerticalConstraint(value) => {
                    node.layout.vertical_constraint == value
                }
                InspectorLayoutChoice::Grow(value) => node.layout.grow == value,
                InspectorLayoutChoice::Shrink(value) => node.layout.shrink == value,
            })
    }

    fn add_property(&self) {
        self.inspector_tab.set(InspectorTab::Logic);
        let name = self.inspector_prop_name.borrow().trim().to_owned();
        let value_type = self.inspector_prop_type.borrow().trim().to_owned();
        let default = self.inspector_prop_default.borrow().trim().to_owned();
        if !is_portable_identifier(&name) {
            self.set_status("Property name must use a portable identifier");
            return;
        }
        if !is_portable_type(&value_type) {
            self.set_status("Property type must be a portable Rust/HTML scalar type");
            return;
        }
        if default.len() > 16 * 1_024 {
            self.set_status("Property default exceeds the 16 KiB component bound");
            return;
        }
        let Some(component_id) = self.active_component_id() else {
            self.set_status("Open a component document to edit its property contract");
            return;
        };
        let mut library = self.native_components.borrow_mut();
        let Some(component) = library
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
        else {
            self.set_status("The active component no longer exists");
            return;
        };
        let property = ComponentProp {
            name: name.clone(),
            value_type,
            default: (!default.is_empty()).then_some(default),
        };
        let updated = if let Some(existing) = component
            .props
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            *existing = property;
            true
        } else {
            component.props.push(property);
            false
        };
        drop(library);
        self.component_edit_dirty.set(true);
        self.component_edit_quiet_ticks.set(0);
        self.set_status(if updated {
            "Property contract updated · live graph updated"
        } else {
            "Property added · live graph updated"
        });
    }

    fn remove_property(&self) {
        self.inspector_tab.set(InspectorTab::Logic);
        let name = self.inspector_prop_name.borrow().trim().to_owned();
        if name.is_empty() {
            self.set_status("Enter the property name to remove");
            return;
        }
        let Some(component_id) = self.active_component_id() else {
            self.set_status("Open a component document to edit its property contract");
            return;
        };
        let mut library = self.native_components.borrow_mut();
        let Some(component) = library
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
        else {
            self.set_status("The active component no longer exists");
            return;
        };
        let original_len = component.props.len();
        component.props.retain(|property| property.name != name);
        if component.props.len() == original_len {
            self.set_status("No property with that name exists");
            return;
        }
        drop(library);
        self.component_edit_dirty.set(true);
        self.component_edit_quiet_ticks.set(0);
        self.set_status("Property removed · live graph updated");
    }

    fn add_logic(&self) {
        self.inspector_tab.set(InspectorTab::Logic);
        let added = {
            let mut library = self.native_components.borrow_mut();
            let active_id = library.active_component.clone();
            let Some(component) = library
                .components
                .iter_mut()
                .find(|component| component.id == active_id)
            else {
                self.set_status("No component is available for an interaction");
                return;
            };
            if component.logic.iter().any(|logic| logic.id == "root-click") {
                false
            } else {
                component.logic.push(ComponentLogic {
                    id: "root-click".to_owned(),
                    source_node: "root".to_owned(),
                    event: ComponentEvent::Click,
                    action: "component_action".to_owned(),
                    guard: None,
                    target_state: None,
                    value: None,
                });
                true
            }
        };
        if !added {
            self.set_status("Root click interaction already exists");
            return;
        }
        self.component_edit_dirty.set(true);
        self.component_edit_quiet_ticks.set(0);
        self.set_status("Added root click → component_action · live graph updated");
    }

    fn console_visible(&self) -> bool {
        self.dock_tab.get() == DockTab::Console && !self.dock_collapsed.get()
    }

    fn states_visible(&self) -> bool {
        self.dock_tab.get() == DockTab::States && !self.dock_collapsed.get()
    }

    fn set_dock_tab(&self, tab: DockTab) {
        self.dock_tab.set(tab);
        self.dock_collapsed.set(false);
    }

    fn toggle_dock(&self) {
        self.dock_collapsed.set(!self.dock_collapsed.get());
    }

    fn dock_toggle_label(&self) -> String {
        if self.dock_collapsed.get() {
            "Expand".to_owned()
        } else {
            "Collapse".to_owned()
        }
    }

    fn toggle_project_menu(&self) {
        self.project_menu_open.set(!self.project_menu_open.get());
        self.viewport_menu_open.set(false);
    }

    fn project_menu_visible(&self) -> bool {
        self.project_menu_open.get()
    }

    fn project_open_help(&self) {
        self.project_menu_open.set(false);
        self.set_status(
            "Open any local project with gpui-studio --project <path>; no network required",
        );
    }

    fn toggle_viewport_menu(&self) {
        self.viewport_menu_open.set(!self.viewport_menu_open.get());
        self.project_menu_open.set(false);
    }

    fn viewport_menu_visible(&self) -> bool {
        self.viewport_menu_open.get()
    }

    fn viewport_label(&self) -> String {
        let canvas = self.canvas.get();
        let layout = canvas.layout(856.0, 572.0);
        format!(
            "{} · {:.0}×{:.0}",
            canvas.preset.label(),
            layout.viewport_width,
            layout.viewport_height
        )
    }

    fn select_viewport(&self, preset: ViewportPreset) {
        let mut canvas = self.canvas.get();
        canvas.preset = preset;
        self.commit_canvas(canvas, format!("{} viewport selected", preset.label()));
        self.viewport_menu_open.set(false);
    }

    fn decorations_label(&self) -> String {
        match self.canvas.get().decorations {
            OutputDecorations::Native => "Frame · Native".to_owned(),
            OutputDecorations::Custom => "Frame · None".to_owned(),
        }
    }

    fn toggle_output_decorations(&self) {
        let mut canvas = self.canvas.get();
        canvas.decorations = match canvas.decorations {
            OutputDecorations::Native => OutputDecorations::Custom,
            OutputDecorations::Custom => OutputDecorations::Native,
        };
        self.commit_canvas(
            canvas,
            format!(
                "{} · output policy and embedded shell preview updated",
                canvas.decorations.label()
            ),
        );
    }

    fn zoom_label(&self, window: &Window) -> String {
        let layout = self.preview_layout(window);
        format!("{:.0}%", layout.effective_zoom_percent)
    }

    fn zoom_by(&self, delta: i16) {
        let mut canvas = self.canvas.get();
        canvas.zoom_by(delta);
        self.commit_canvas(canvas, format!("Canvas zoom {}%", canvas.zoom_percent));
    }

    fn fit_canvas(&self, window: &Window) {
        let mut canvas = self.canvas.get();
        let (available_width, available_height) = self.canvas_room(window);
        canvas.zoom_percent = canvas.fit_zoom(available_width, available_height);
        self.commit_canvas(canvas, format!("Canvas fitted at {}%", canvas.zoom_percent));
    }

    fn rotate_viewport(&self) {
        let mut canvas = self.canvas.get();
        canvas.rotate_clockwise();
        self.commit_canvas(
            canvas,
            format!("Viewport rotated {}°", u16::from(canvas.quarter_turns) * 90),
        );
    }

    fn toggle_snap(&self) {
        let mut canvas = self.canvas.get();
        canvas.snap_enabled = !canvas.snap_enabled;
        self.commit_canvas(
            canvas,
            if canvas.snap_enabled {
                format!("Snapping enabled · {}px grid", canvas.snap_grid)
            } else {
                "Snapping disabled".to_owned()
            },
        );
    }

    fn snap_label(&self) -> String {
        let canvas = self.canvas.get();
        if canvas.snap_enabled {
            format!("Snap {}", canvas.snap_grid)
        } else {
            "Snap off".to_owned()
        }
    }

    fn commit_canvas(&self, canvas: CanvasSettings, status: String) {
        let previous = self.canvas.get();
        let project_root = self.project.borrow().root().to_owned();
        if canvas.decorations != previous.decorations
            && let Err(error) = persist_output_window_module(&project_root, canvas.decorations)
        {
            self.set_status(format!("Update output window policy failed: {error}"));
            return;
        }
        if let Err(error) = self.update_workspace(|workspace| workspace.canvas = canvas) {
            if canvas.decorations != previous.decorations {
                let _ = persist_output_window_module(&project_root, previous.decorations);
            }
            self.set_status(format!("Update canvas settings failed: {error}"));
            return;
        }
        self.canvas.set(canvas);
        self.set_status(status);
    }

    fn canvas_room(&self, window: &Window) -> (f32, f32) {
        let size = window.viewport_size();
        available_canvas_with_rails(
            size.width.into(),
            size.height.into(),
            self.dock_collapsed.get(),
            self.panel_size("project-rail"),
            self.panel_size("inspector"),
        )
    }

    fn preview_layout(&self, window: &Window) -> crate::PreviewLayout {
        let (available_width, available_height) = self.canvas_room(window);
        self.canvas.get().layout(available_width, available_height)
    }

    fn render_app_frame(&self, children: Vec<AnyElement>, window: &Window) -> AnyElement {
        let layout = self.preview_layout(window);
        let mut frame = div()
            .relative()
            .flex()
            .flex_col()
            .w(px(layout.frame_width))
            .h(px(layout.frame_height))
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x0b_0d_12))
            .border_1()
            .border_color(rgb(0x23_26_2f))
            .rounded(px(
                if self.canvas.get().decorations == OutputDecorations::Native {
                    9.0
                } else {
                    13.0
                },
            ));
        if layout.native_decorator_height > 0.0 {
            frame = frame.child(render_native_decorator(layout.native_decorator_height));
        }
        children
            .into_iter()
            .fold(frame, gpui::ParentElement::child)
            .into_any_element()
    }

    fn render_bottom_dock(&self, children: Vec<AnyElement>, window: &Window) -> AnyElement {
        let size = window.viewport_size();
        let width: f32 = size.width.into();
        let height: f32 = size.height.into();
        let height = if self.dock_collapsed.get() {
            36.0
        } else if height <= 720.0 {
            150.0
        } else if width <= 1_120.0 {
            170.0
        } else {
            206.0
        };
        children
            .into_iter()
            .fold(
                div()
                    .flex()
                    .flex_col()
                    .h(px(height))
                    .min_h(px(36.0))
                    .bg(rgb(0x11_13_19))
                    .border_t_1()
                    .border_color(rgb(0x1e_22_2b)),
                gpui::ParentElement::child,
            )
            .into_any_element()
    }

    fn toggle_welcome_layer(&self) {
        self.select_project_element("welcome-canvas");
        self.welcome_collapsed.set(!self.welcome_collapsed.get());
    }

    fn welcome_children_visible(&self) -> bool {
        !self.welcome_collapsed.get()
    }

    fn welcome_disclosure(&self) -> String {
        disclosure_symbol(self.welcome_collapsed.get())
    }

    fn toggle_header_layer(&self) {
        self.select_project_element("welcome-header");
        self.header_collapsed.set(!self.header_collapsed.get());
    }

    fn header_children_visible(&self) -> bool {
        !self.header_collapsed.get()
    }

    fn header_disclosure(&self) -> String {
        disclosure_symbol(self.header_collapsed.get())
    }

    fn toggle_welcome_copy_layer(&self) {
        self.select_project_element("welcome-copy");
        self.welcome_copy_collapsed
            .set(!self.welcome_copy_collapsed.get());
    }

    fn welcome_copy_children_visible(&self) -> bool {
        !self.welcome_copy_collapsed.get()
    }

    fn welcome_copy_disclosure(&self) -> String {
        disclosure_symbol(self.welcome_copy_collapsed.get())
    }

    fn toggle_feature_cards_layer(&self) {
        self.select_project_element("feature-grid");
        self.feature_cards_collapsed
            .set(!self.feature_cards_collapsed.get());
    }

    fn feature_cards_children_visible(&self) -> bool {
        !self.feature_cards_collapsed.get()
    }

    fn feature_cards_disclosure(&self) -> String {
        disclosure_symbol(self.feature_cards_collapsed.get())
    }

    fn toggle_welcome_lower_layer(&self) {
        self.select_project_element("welcome-lower");
        self.welcome_lower_collapsed
            .set(!self.welcome_lower_collapsed.get());
    }

    fn welcome_lower_children_visible(&self) -> bool {
        !self.welcome_lower_collapsed.get()
    }

    fn welcome_lower_disclosure(&self) -> String {
        disclosure_symbol(self.welcome_lower_collapsed.get())
    }

    fn layer_welcome_selected(&self) -> bool {
        self.selection.borrow().authored_id == "welcome-canvas"
    }

    fn layer_runtime_badge_selected(&self) -> bool {
        self.selection.borrow().authored_id == "preview-badge"
    }

    fn layer_hero_title_selected(&self) -> bool {
        self.selection.borrow().authored_id == "hero-title"
    }

    fn layer_header_selected(&self) -> bool {
        self.selection.borrow().authored_id == "welcome-header"
    }

    fn layer_welcome_copy_selected(&self) -> bool {
        self.selection.borrow().authored_id == "welcome-copy"
    }

    fn layer_feature_cards_selected(&self) -> bool {
        self.selection.borrow().authored_id == "feature-grid"
    }

    fn layer_runtime_building_card_selected(&self) -> bool {
        self.selection.borrow().authored_id == "runtime-building-card"
    }

    fn layer_local_ai_card_selected(&self) -> bool {
        self.selection.borrow().authored_id == "local-ai-card"
    }

    fn layer_portable_source_card_selected(&self) -> bool {
        self.selection.borrow().authored_id == "portable-source-card"
    }

    fn layer_component_selected(&self) -> bool {
        self.selection.borrow().authored_id == "app-titlebar-component"
    }

    /// The component id whose graph the canvas is currently editing. Main maps
    /// to the project's root component so the whole editor operates on one
    /// unified component graph model.
    fn active_component_id(&self) -> Option<String> {
        match self.document_tabs.borrow().active_id() {
            DocumentId::Main => self.root_component_id(),
            DocumentId::Component(id) => Some(id.clone()),
        }
    }

    /// The project's root ("App") component — the first definition in the
    /// library. This is what the permanent Main tab renders and edits.
    fn root_component_id(&self) -> Option<String> {
        self.native_components
            .borrow()
            .components
            .first()
            .map(|component| component.id.clone())
    }

    /// Whether a component graph is currently active and editable. Always true
    /// while the library is non-empty, since Main now edits the root component.
    fn editing_component_graph(&self) -> bool {
        self.active_component_id().is_some()
    }

    fn set_selection(&self, selection: ElementSelection) {
        self.document_tabs
            .borrow_mut()
            .replace_active_selection(selection.clone());
        *self.selection.borrow_mut() = selection;
        self.selection_snapshot.borrow_mut().take();
    }

    fn select_project_layer(
        &self,
        model: &LayerTree,
        row: &LayerRow,
        extend_range: bool,
        toggle: bool,
    ) {
        let runtime_id = row.runtime_id.clone();
        if extend_range {
            let anchor = self
                .multi_selection_anchor
                .borrow()
                .clone()
                .unwrap_or_else(|| runtime_id.clone());
            let range = model.range(&self.project_tree_expanded.borrow(), &anchor, &runtime_id);
            if !range.is_empty() {
                *self.multi_selection.borrow_mut() = range;
            }
        } else if toggle {
            let mut selection = self.multi_selection.borrow_mut();
            if !selection.remove(&runtime_id) {
                selection.insert(runtime_id.clone());
            }
            *self.multi_selection_anchor.borrow_mut() = Some(runtime_id.clone());
        } else {
            *self.multi_selection.borrow_mut() = BTreeSet::from([runtime_id.clone()]);
            *self.multi_selection_anchor.borrow_mut() = Some(runtime_id.clone());
        }
        *self.project_tree_focus.borrow_mut() = Some(runtime_id.clone());
        let tree = self.automation.snapshot();
        let revision = self.target().map_or_else(
            || self.selection.borrow().document_revision,
            |target| target.revision(),
        );
        self.set_selection(ElementSelection::new(
            row.authored_id.clone(),
            runtime_id.clone(),
            revision,
            tree.nodes.get(&runtime_id).and_then(|node| node.bounds),
        ));
        self.set_status(format!(
            "Selected {} · {} layer{}",
            row.label,
            self.multi_selection.borrow().len(),
            if self.multi_selection.borrow().len() == 1 {
                ""
            } else {
                "s"
            }
        ));
    }

    fn toggle_project_layer(&self, runtime_id: &str) {
        let mut expanded = self.project_tree_expanded.borrow_mut();
        if !expanded.remove(runtime_id) {
            expanded.insert(runtime_id.to_owned());
        }
    }

    fn open_component_instance(&self, runtime_id: &str) {
        let tree = self.automation.snapshot();
        let Some(component_id) = tree
            .nodes
            .get(runtime_id)
            .and_then(|node| node.metadata.get("component_id"))
        else {
            self.set_status("Component instance has no canonical component ID");
            return;
        };
        self.open_component_document(component_id);
    }

    fn activate_document(&self, id: &DocumentId) -> bool {
        let current_selection = self.selection.borrow().clone();
        let selection = self
            .document_tabs
            .borrow_mut()
            .activate(id, current_selection);
        let Some(selection) = selection else {
            return false;
        };
        match id {
            DocumentId::Component(component_id) => {
                self.native_components
                    .borrow_mut()
                    .active_component
                    .clone_from(component_id);
            }
            DocumentId::Main => {
                // Main edits the root component; keep `active()` pointed at it so
                // the tree, inspector, and canvas all follow the same graph.
                if let Some(root) = self.root_component_id() {
                    self.native_components.borrow_mut().active_component = root;
                }
            }
        }
        *self.selection.borrow_mut() = selection;
        self.selection_snapshot.borrow_mut().take();
        true
    }

    fn open_component_document(&self, component_id: &str) {
        let exists = self
            .native_components
            .borrow()
            .components
            .iter()
            .any(|component| component.id == component_id);
        if !exists {
            self.set_status(format!(
                "Component document {component_id} no longer exists"
            ));
            return;
        }
        // The root component is edited through the permanent Main tab; opening it
        // as a separate document would duplicate the same graph.
        if self.root_component_id().as_deref() == Some(component_id) {
            self.focus_main_root();
            return;
        }
        component_id.clone_into(&mut self.native_components.borrow_mut().active_component);
        self.component_tree_collapsed.set(false);
        let current_selection = self.selection.borrow().clone();
        let selection = self
            .document_tabs
            .borrow_mut()
            .open_component(component_id, current_selection);
        *self.selection.borrow_mut() = selection;
    }

    fn close_component_document(&self, component_id: &str) {
        let id = DocumentId::Component(component_id.to_owned());
        let current_selection = self.selection.borrow().clone();
        let selection = self
            .document_tabs
            .borrow_mut()
            .close(&id, current_selection);
        let Some(selection) = selection else {
            return;
        };
        *self.selection.borrow_mut() = selection;
        if let Some(active_component) = self.active_component_id() {
            self.native_components.borrow_mut().active_component = active_component;
        }
        let name = self
            .native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.id == component_id)
            .map_or_else(
                || component_id.to_owned(),
                |component| component.name.clone(),
            );
        self.set_status(format!("Closed {name} · active document restored"));
    }

    /// Remove a component from the library: refuses the root component, the
    /// last remaining component, and components still referenced by other
    /// components' instances. On success, closes its open document tab,
    /// drops its runtime graph and state, and persists the library.
    fn delete_component(&self, component_id: &str) {
        match self.remove_component_checked(component_id) {
            Ok(name) => {
                self.set_status(format!("Deleted {name}"));
                self.trace("info", &format!("[library] deleted {name}"));
            }
            Err(error) => {
                self.trace("warn", &format!("[library] delete refused: {error}"));
                self.set_status(error);
            }
        }
    }

    /// Remove a component through the guarded library path (refuses the root,
    /// the last component, or one still referenced by others), tear down its
    /// per-component editor state, and persist. Shared by the palette delete UI
    /// and the `component.remove` MCP command. Returns the deleted name.
    fn remove_component_checked(&self, component_id: &str) -> Result<String, String> {
        let name = {
            let mut library = self.native_components.borrow_mut();
            let name = library.component(component_id).map_or_else(
                || component_id.to_owned(),
                |component| component.name.clone(),
            );
            library.remove_component(component_id).map(|()| name)?
        };
        self.close_component_document(component_id);
        self.component_graphs.borrow_mut().remove(component_id);
        self.component_runtime_state
            .borrow_mut()
            .remove(component_id);
        let root = self.project.borrow().root().to_owned();
        self.native_components
            .borrow()
            .save(&root)
            .map_err(|error| format!("Deleted {name} but saving failed: {error}"))?;
        Ok(name)
    }

    fn page_tree_visible(&self) -> bool {
        !self.editing_component_graph()
    }

    fn component_document_tree_visible(&self) -> bool {
        self.editing_component_graph()
    }

    fn component_root_selected(&self) -> bool {
        self.editing_component_graph() && self.selection.borrow().authored_id == "root"
    }

    fn toggle_component_root(&self) {
        self.select_component_root();
        self.component_tree_collapsed
            .set(!self.component_tree_collapsed.get());
    }

    fn component_tree_children_visible(&self) -> bool {
        self.editing_component_graph() && !self.component_tree_collapsed.get()
    }

    fn component_root_disclosure(&self) -> String {
        disclosure_symbol(self.component_tree_collapsed.get())
    }

    fn layer_runtime_details_selected(&self) -> bool {
        self.selection.borrow().authored_id == "runtime-details"
    }

    fn layer_welcome_lower_selected(&self) -> bool {
        self.selection.borrow().authored_id == "welcome-lower"
    }

    fn layer_dogfood_note_selected(&self) -> bool {
        self.selection.borrow().authored_id == "dogfood-note"
    }

    fn select_project_element(&self, authored_id: &str) {
        let revision = self.target().map_or(1, |target| target.revision());
        self.activate_document(&DocumentId::Main);
        self.set_selection(ElementSelection::new(
            authored_id,
            format!("project-canvas--{authored_id}"),
            revision,
            None,
        ));
        self.set_status(format!("Selected #{authored_id}"));
    }

    fn select_active_component(&self) {
        self.project_menu_open.set(false);
        let library = self.native_components.borrow();
        let Some(component) = library.active() else {
            self.set_status("No component document is available");
            return;
        };
        let id = component.id.clone();
        let name = component.name.clone();
        drop(library);
        self.open_component_document(&id);
        self.set_status(format!("Opened {name} in its component document tab"));
    }

    fn open_app_titlebar_component(&self) {
        let component = self
            .native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.name == "AppTitlebar")
            .map(|component| (component.id.clone(), component.name.clone()));
        let Some((id, name)) = component else {
            self.set_status("AppTitlebar component is missing from this project");
            return;
        };
        self.open_component_document(&id);
        self.set_status(format!("Opened {name} in its component document tab"));
    }

    fn select_component_instance(&self) {
        self.select_project_element("app-titlebar-component");
        self.set_status("Selected the AppTitlebar instance · double-click to open its component");
    }

    fn select_component_node(&self, component_id: &str, node_id: &str) {
        if self.active_component_id().as_deref() != Some(component_id) {
            self.open_component_document(component_id);
        }
        self.set_selection(ElementSelection::new(
            node_id,
            format!("component/{component_id}/{node_id}"),
            1,
            None,
        ));
        self.set_status(format!("Selected component node #{node_id}"));
        self.trace("info", &format!("[select] {component_id}/{node_id}"));
    }

    /// Activate the permanent Main tab and select the root component's root
    /// node, giving the inspector an editable target.
    fn focus_main_root(&self) {
        self.project_menu_open.set(false);
        self.activate_document(&DocumentId::Main);
        self.select_component_root();
    }

    fn select_component_root(&self) {
        let Some(component_id) = self.active_component_id() else {
            return;
        };
        self.set_selection(ElementSelection::new(
            "root",
            format!("component/{component_id}/root"),
            1,
            None,
        ));
        self.set_status("Selected component document root");
    }

    fn component_dialog_visible(&self) -> bool {
        self.component_dialog_open.get()
    }

    fn open_component_dialog(&self) {
        *self.component_name.borrow_mut() = self.selection_heading();
        *self.component_props.borrow_mut() = "title: String, active: bool".to_owned();
        self.component_authoring.set(self.backend.get());
        self.component_source.set(ComponentSource::Selection);
        self.component_dialog_open.set(true);
    }

    fn close_component_dialog(&self) {
        self.component_dialog_open.set(false);
    }

    fn component_html_selected(&self) -> bool {
        self.component_authoring.get() == AuthoringBackend::Html
    }

    fn component_gpui_selected(&self) -> bool {
        self.component_authoring.get() == AuthoringBackend::Gpui
    }

    fn select_component_html(&self) {
        self.component_authoring.set(AuthoringBackend::Html);
    }

    fn select_component_gpui(&self) {
        self.component_authoring.set(AuthoringBackend::Gpui);
    }

    fn component_selection_selected(&self) -> bool {
        self.component_source.get() == ComponentSource::Selection
    }

    fn component_blank_selected(&self) -> bool {
        self.component_source.get() == ComponentSource::Blank
    }

    fn component_paste_html_selected(&self) -> bool {
        self.component_source.get() == ComponentSource::PasteHtml
    }

    fn component_titlebar_selected(&self) -> bool {
        self.component_source.get() == ComponentSource::Titlebar
    }

    fn component_preset_selected(&self, preset: ComponentPreset) -> bool {
        self.component_source.get() == ComponentSource::Preset
            && self.component_preset.get() == preset
    }

    fn select_component_selection(&self) {
        self.component_source.set(ComponentSource::Selection);
    }

    fn select_component_blank(&self) {
        self.component_source.set(ComponentSource::Blank);
    }

    fn select_component_paste_html(&self) {
        self.component_source.set(ComponentSource::PasteHtml);
    }

    fn select_component_titlebar(&self) {
        self.component_source.set(ComponentSource::Titlebar);
        if self.component_name.borrow().trim().is_empty()
            || self.component_name.borrow().as_str() == self.selection_heading()
        {
            *self.component_name.borrow_mut() = "AppTitlebar".to_owned();
        }
    }

    fn select_component_preset(&self, preset: ComponentPreset) {
        self.component_source.set(ComponentSource::Preset);
        self.component_preset.set(preset);
        *self.component_name.borrow_mut() = preset.label().replace(' ', "");
        *self.component_props.borrow_mut() = String::new();
        self.set_status(format!(
            "{} preset selected · {}",
            preset.label(),
            preset.description()
        ));
    }

    fn component_stub(&self) -> String {
        let name = self
            .valid_component_name()
            .unwrap_or_else(|| "Component".to_owned());
        let props = self.component_props.borrow();
        let selection = self.selection_heading();
        let source_hint = match self.component_source.get() {
            ComponentSource::Selection => format!("  <!-- extracted from {selection} -->\n"),
            ComponentSource::Blank => String::new(),
            ComponentSource::PasteHtml => "  <!-- paste HTML on create -->\n".to_owned(),
            ComponentSource::Titlebar => {
                "  <!-- semantic titlebar; native decorations configured separately -->\n"
                    .to_owned()
            }
            ComponentSource::Preset => format!(
                "  <!-- complete {} preset: {} -->\n",
                self.component_preset.get().label(),
                self.component_preset.get().description()
            ),
        };
        match self.component_authoring.get() {
            AuthoringBackend::Html => {
                let (open, close) = if self.component_source.get() == ComponentSource::Titlebar {
                    (
                        format!(
                            "<header id=\"{}\" role=\"toolbar\" aria-label=\"Window titlebar\">",
                            component_id(&name)
                        ),
                        "</header>",
                    )
                } else {
                    (format!("<div class=\"{}\">", component_id(&name)), "</div>")
                };
                format!("{open}\n{source_hint}  …\n{close}")
            }
            AuthoringBackend::Gpui => {
                let body = match self.component_source.get() {
                    ComponentSource::Selection => format!("// from {selection}"),
                    ComponentSource::Titlebar => {
                        ".child(/* semantic title + window actions */)".to_owned()
                    }
                    ComponentSource::Preset => format!(
                        ".child(/* complete {} preset graph */)",
                        self.component_preset.get().label()
                    ),
                    ComponentSource::Blank | ComponentSource::PasteHtml => {
                        ".child(/* ... */)".to_owned()
                    }
                };
                format!(
                    "struct {name} {{\n    {}\n}}\n\nimpl Render for {name} {{\n    fn render(&mut self, cx) -> impl IntoElement {{\n        div().flex().gap_2()\n            {body}\n    }}\n}}",
                    if props.trim().is_empty() {
                        "// props"
                    } else {
                        props.trim()
                    }
                )
            }
        }
    }

    fn valid_component_name(&self) -> Option<String> {
        let name = self.component_name.borrow();
        let trimmed = name.trim();
        (!trimmed.is_empty() && trimmed.len() <= 256).then(|| trimmed.to_owned())
    }

    fn create_component_from_dialog(&self) {
        if self.valid_component_name().is_none() {
            self.set_status("Component name must contain 1–256 characters");
            return;
        }
        self.create_document_component();
    }

    fn theme_label(&self) -> String {
        self.workspace.borrow().theme.label()
    }

    fn theme_revision(&self) -> u64 {
        self.theme_revision.get()
    }

    fn resolved_theme(&self) -> crate::ResolvedTheme {
        self.themes.borrow().resolve(&self.workspace.borrow().theme)
    }

    fn cycle_theme(&self) {
        let selection = self.themes.borrow().next(&self.workspace.borrow().theme);
        if let Err(error) = self.update_workspace(|workspace| workspace.theme = selection.clone()) {
            self.set_status(format!("Change editor theme failed: {error}"));
            return;
        }
        self.theme_revision
            .set(self.theme_revision.get().saturating_add(1));
        self.set_status(format!("Editor theme changed to {}", selection.label()));
    }

    fn settings_visible(&self) -> bool {
        self.settings_open.get()
    }

    fn open_settings(&self) {
        self.settings_open.set(true);
    }

    fn close_settings(&self) {
        self.settings_open.set(false);
        self.theme_dropdown_open.set(false);
    }

    fn toggle_theme_dropdown(&self) {
        let next = !self.theme_dropdown_open.get();
        self.theme_dropdown_open.set(next);
    }

    fn annotations_drawer_visible(&self) -> bool {
        self.annotations_drawer_open.get()
    }

    fn toggle_annotations_drawer(&self) {
        let next = !self.annotations_drawer_open.get();
        self.annotations_drawer_open.set(next);
    }

    fn close_annotations_drawer(&self) {
        self.annotations_drawer_open.set(false);
    }

    /// Available theme variants paired with whether each is the active one.
    fn theme_options(&self) -> Vec<(crate::AvailableTheme, bool)> {
        let active = self.workspace.borrow().theme.clone();
        self.themes
            .borrow()
            .available()
            .into_iter()
            .map(|available| {
                let selected = available.name.eq_ignore_ascii_case(active.name.trim())
                    && available.mode == active.mode;
                (available, selected)
            })
            .collect()
    }

    /// Select a specific theme variant (from the Settings dropdown).
    fn select_theme(&self, name: &str, mode: crate::ThemeMode) {
        let selection = crate::ThemeSelection {
            name: name.to_owned(),
            mode,
        };
        if let Err(error) = self.update_workspace(|workspace| workspace.theme = selection.clone()) {
            self.set_status(format!("Change editor theme failed: {error}"));
            return;
        }
        self.theme_revision
            .set(self.theme_revision.get().saturating_add(1));
        self.theme_dropdown_open.set(false);
        self.set_status(format!("Editor theme changed to {}", selection.label()));
    }

    fn replace_theme_catalog(&self, catalog: ThemeCatalog) {
        let selection = catalog.resolve(&self.workspace.borrow().theme).selection;
        if let Err(error) = self.update_workspace(|workspace| workspace.theme = selection.clone()) {
            self.set_status(format!("Reload theme catalog failed: {error}"));
            return;
        }
        *self.themes.borrow_mut() = catalog;
        self.theme_revision
            .set(self.theme_revision.get().saturating_add(1));
        self.set_status(format!(
            "Theme catalog hot reloaded · {}",
            selection.label()
        ));
    }

    fn update_workspace(
        &self,
        update: impl FnOnce(&mut WorkspaceSettings),
    ) -> Result<(), crate::WorkspaceSettingsError> {
        let mut candidate = self.workspace.borrow().clone();
        update(&mut candidate);
        candidate.save(&self.studio_root)?;
        *self.workspace.borrow_mut() = candidate;
        Ok(())
    }

    fn commit_backend(
        &self,
        backend: AuthoringBackend,
    ) -> Result<(), crate::WorkspaceSettingsError> {
        self.update_workspace(|workspace| workspace.backend = backend)?;
        self.backend.set(backend);
        Ok(())
    }

    fn inspector_code_excerpt(&self) -> String {
        if self.editing_component_graph() {
            return self.native_components.borrow().active().map_or_else(
                String::new,
                |component| match self.backend.get() {
                    AuthoringBackend::Html => format!(
                        "{}\n<style>\n{}</style>\n\n{}",
                        component.html_projection(),
                        component.css_projection(),
                        component.bindings_projection()
                    ),
                    AuthoringBackend::Gpui => component.gpui_excerpt(),
                },
            );
        }
        match self.backend.get() {
            AuthoringBackend::Html => {
                let authored_id = self.selection.borrow().authored_id.clone();
                self.target().map_or_else(String::new, |target| {
                    let source = target.document().source.html;
                    html_element_excerpt(&source, &authored_id)
                        .unwrap_or_else(|| bounded_excerpt(&source, 920))
                })
            }
            AuthoringBackend::Gpui => {
                let selection = self.selection.borrow();
                self.automation
                    .snapshot()
                    .nodes
                    .get(&selection.runtime_id)
                    .map_or_else(String::new, gpui_element_excerpt)
            }
        }
    }

    fn set_mode(&self, mode: StudioMode) {
        if let Err(error) = self.update_workspace(|workspace| workspace.mode = mode) {
            self.set_status(format!("Change workspace mode failed: {error}"));
            return;
        }
        self.mode.set(mode);
        self.set_status(format!(
            "{} workspace selected",
            format!("{mode:?}").to_lowercase()
        ));
        self.trace("info", &format!("[mode] {mode:?}"));
    }

    /// Anchor a Move-tool pan drag at the current pointer.
    fn begin_pan(&self, x: f32, y: f32) {
        self.pan_anchor.set(Some((x, y)));
    }

    /// Advance the canvas pan offset by the pointer delta since the last
    /// sample. Returns whether the offset moved (and a repaint is needed).
    fn update_pan(&self, x: f32, y: f32) -> bool {
        let Some((anchor_x, anchor_y)) = self.pan_anchor.get() else {
            return false;
        };
        let (pan_x, pan_y) = self.canvas_pan.get();
        self.canvas_pan
            .set((pan_x + (x - anchor_x), pan_y + (y - anchor_y)));
        self.pan_anchor.set(Some((x, y)));
        true
    }

    /// Release a pan drag. Returns whether one was in progress.
    fn end_pan(&self) -> bool {
        self.pan_anchor.take().is_some()
    }

    fn set_backend(&self, backend: AuthoringBackend) {
        if let Err(error) = self.commit_backend(backend) {
            self.set_status(format!("Change authoring projection failed: {error}"));
            return;
        }
        self.set_status(match backend {
            AuthoringBackend::Html => {
                "HTML/CSS projection selected · canvas, selection, state, and logic unchanged"
            }
            AuthoringBackend::Gpui => {
                "GPUI projection selected · canvas, selection, state, and logic unchanged"
            }
        });
    }

    fn render_canvas(self: &Rc<Self>, window: &mut Window, cx: &mut App) -> gpui::AnyElement {
        if self.editing_component_graph() {
            let component = self.native_components.borrow().active().cloned();
            let Some(component) = component else {
                return "No component document".to_owned().into_any_element();
            };
            let state = self
                .component_runtime_state
                .borrow_mut()
                .entry(component.id.clone())
                .or_insert_with(|| {
                    component
                        .states
                        .iter()
                        .map(|state| (state.name.clone(), state.default.clone()))
                        .collect()
                })
                .clone();
            let pointer_state = self.clone();
            // The Move tool pans the whole canvas, so nodes must NOT capture
            // pointer events (their hover/stop-propagation would swallow the
            // pan drag). Rendering with no handler leaves them inert and lets
            // every mouse event reach the surface's pan listeners.
            let handler: Option<crate::ComponentPointerHandler> =
                if self.mode.get() == StudioMode::Move {
                    None
                } else {
                    Some(Rc::new(move |component_id, node_id, gesture| {
                        pointer_state.handle_component_pointer(component_id, node_id, gesture)
                    }))
                };
            let preview_spec = self
                .drag_preview
                .borrow()
                .as_ref()
                .map(DragPreviewState::spec);
            // In select (Design) mode canvas nodes are grab-to-move draggable.
            let draggable = self.mode.get() == StudioMode::Design;
            let rendered = component.render_interactive(
                &self.native_components.borrow(),
                &self.automation,
                &state,
                handler,
                preview_spec.as_ref(),
                draggable,
            );
            // The Move tool pans the canvas by offsetting the rendered content
            // within the surface; a relative wrapper shifts it without changing
            // layout so element bounds (and hit-testing) stay consistent.
            let (pan_x, pan_y) = self.canvas_pan.get();
            let rendered: AnyElement = if pan_x == 0.0 && pan_y == 0.0 {
                rendered
            } else {
                div()
                    .relative()
                    .left(px(pan_x))
                    .top(px(pan_y))
                    .size_full()
                    .child(rendered)
                    .into_any_element()
            };
            // The canvas accepts palette components, tree nodes, and its own
            // nodes (grab-to-move). While a drag hovers, the preview spacer
            // shows the landing spot; scroll (or [ ]) nudges the insertion order.
            let palette_drop_state = self.clone();
            let tree_drop_state = self.clone();
            let node_drop_state = self.clone();
            let can_drop_state = self.clone();
            let palette_move_state = self.clone();
            let tree_move_state = self.clone();
            let node_move_state = self.clone();
            let scroll_state = self.clone();
            let hover_clear_state = self.clone();
            let up_state = self.clone();
            let up_out_state = self.clone();
            let pan_down_state = self.clone();
            let pan_move_state = self.clone();
            let pan_up_state = self.clone();
            let pan_up_out_state = self.clone();
            let mut surface = div().size_full().min_w_0().min_h_0();
            // One stable cursor for the whole canvas per mode. Nodes set no
            // cursor, so this shows through their hitboxes and never flickers as
            // the pointer crosses element boundaries: grab/open-hand in the Move
            // (pan) tool, crosshair in Annotate, default arrow for Select and
            // Preview (Select uses the standard arrow like Figma's move tool).
            match self.mode.get() {
                StudioMode::Compare => {
                    surface = surface.cursor(gpui::CursorStyle::Crosshair);
                }
                StudioMode::Move => {
                    surface = surface.cursor(gpui::CursorStyle::OpenHand);
                }
                StudioMode::Design | StudioMode::Test | StudioMode::Source => {}
            }
            return surface
                .on_drag_move::<PaletteDrag>(move |event, window, _| {
                    let position = event.event.position;
                    if palette_move_state.update_drag_preview(McpPoint {
                        x: f32::from(position.x),
                        y: f32::from(position.y),
                    }) {
                        window.refresh();
                    }
                })
                .on_drag_move::<TreeNodeDrag>(move |event, window, _| {
                    let position = event.event.position;
                    if tree_move_state.update_drag_preview(McpPoint {
                        x: f32::from(position.x),
                        y: f32::from(position.y),
                    }) {
                        window.refresh();
                    }
                })
                .on_drag_move::<crate::ComponentNodeDrag>(move |event, window, _| {
                    let position = event.event.position;
                    if node_move_state.update_drag_preview(McpPoint {
                        x: f32::from(position.x),
                        y: f32::from(position.y),
                    }) {
                        window.refresh();
                    }
                })
                .on_scroll_wheel(move |event, window, cx| {
                    if scroll_state.drag_preview.borrow().is_none() {
                        return;
                    }
                    let delta = match event.delta {
                        gpui::ScrollDelta::Pixels(point) => f32::from(point.y),
                        gpui::ScrollDelta::Lines(point) => point.y,
                    };
                    let step = if delta > 0.0 {
                        -1
                    } else if delta < 0.0 {
                        1
                    } else {
                        return;
                    };
                    cx.stop_propagation();
                    if scroll_state.adjust_drag_preview_order(step) {
                        window.refresh();
                    }
                })
                .on_mouse_down(MouseButton::Left, move |event, _, _| {
                    // Move tool: anchor a pan drag. Nodes are inert in Move mode
                    // (no handler), so this fires anywhere on the canvas.
                    if pan_down_state.mode.get() == StudioMode::Move {
                        pan_down_state
                            .begin_pan(f32::from(event.position.x), f32::from(event.position.y));
                    }
                })
                .on_mouse_move(move |event, window, _| {
                    // Move tool: pan follows the pointer while dragging.
                    if pan_move_state.mode.get() == StudioMode::Move
                        && pan_move_state
                            .update_pan(f32::from(event.position.x), f32::from(event.position.y))
                    {
                        window.refresh();
                        return;
                    }
                    // Children stop propagation, so this only fires over empty
                    // canvas: clear any stale hover target.
                    if hover_clear_state
                        .hovered_canvas_node
                        .borrow_mut()
                        .take()
                        .is_some()
                    {
                        window.refresh();
                    }
                })
                .on_mouse_up(MouseButton::Left, move |_, window, _| {
                    let panned = pan_up_state.end_pan();
                    if up_state.clear_drag_preview() || panned {
                        window.refresh();
                    }
                })
                .on_mouse_up_out(MouseButton::Left, move |_, window, _| {
                    let panned = pan_up_out_state.end_pan();
                    if up_out_state.clear_drag_preview() || panned {
                        window.refresh();
                    }
                })
                .drag_over::<PaletteDrag>(|style, _, _, _| style.bg(rgba(0x6e_7b_ff_10)))
                .drag_over::<TreeNodeDrag>(|style, _, _, _| style.bg(rgba(0x6e_7b_ff_10)))
                .drag_over::<crate::ComponentNodeDrag>(|style, _, _, _| {
                    style.bg(rgba(0x6e_7b_ff_10))
                })
                .can_drop(move |payload, _, _| {
                    if let Some(drag) = payload.downcast_ref::<PaletteDrag>() {
                        can_drop_state.active_component_id().is_some_and(|active| {
                            !can_drop_state
                                .native_components
                                .borrow()
                                .would_cycle(&active, &drag.component_id)
                        })
                    } else {
                        payload.downcast_ref::<TreeNodeDrag>().is_some()
                            || payload.downcast_ref::<crate::ComponentNodeDrag>().is_some()
                    }
                })
                .on_drop::<PaletteDrag>(move |drag, window, _| {
                    let pointer = window.mouse_position();
                    let pointer = McpPoint {
                        x: f32::from(pointer.x),
                        y: f32::from(pointer.y),
                    };
                    let placement = palette_drop_state.canvas_drop_placement(pointer);
                    palette_drop_state.clear_drag_preview();
                    palette_drop_state.insert_component_instance_at(&drag.component_id, placement);
                    window.refresh();
                })
                .on_drop::<TreeNodeDrag>(move |drag, window, _| {
                    let pointer = window.mouse_position();
                    let pointer = McpPoint {
                        x: f32::from(pointer.x),
                        y: f32::from(pointer.y),
                    };
                    tree_drop_state.drop_tree_node_at_canvas(drag, pointer);
                    tree_drop_state.clear_drag_preview();
                    window.refresh();
                })
                .on_drop::<crate::ComponentNodeDrag>(move |drag, window, _| {
                    let pointer = window.mouse_position();
                    let pointer = McpPoint {
                        x: f32::from(pointer.x),
                        y: f32::from(pointer.y),
                    };
                    node_drop_state.drop_component_node_at_canvas(drag, pointer);
                    node_drop_state.clear_drag_preview();
                    window.refresh();
                })
                .child(rendered)
                .into_any_element();
        }
        let layout = self.preview_layout(window);
        self.target().map_or_else(
            || "No project document".to_owned().into_any_element(),
            |target| {
                target.render_for_viewport(
                    layout.viewport_width,
                    layout.viewport_height,
                    window,
                    cx,
                )
            },
        )
    }

    fn invoke_component_action(&self, component_id: &str, node_id: &str, action: &str) {
        let (name, defaults, logic) = {
            let library = self.native_components.borrow();
            let Some(component) = library
                .components
                .iter()
                .find(|component| component.id == component_id)
            else {
                self.set_status("Component action ignored because the document was removed");
                return;
            };
            (
                component.name.clone(),
                component
                    .states
                    .iter()
                    .map(|state| (state.name.clone(), state.default.clone()))
                    .collect::<BTreeMap<_, _>>(),
                component.logic.clone(),
            )
        };
        let current = self
            .component_runtime_state
            .borrow()
            .get(component_id)
            .cloned()
            .unwrap_or(defaults);
        let transitions = component_logic_transitions(&logic, &current, node_id, action);
        if !transitions.is_empty() {
            let mut runtime = self.component_runtime_state.borrow_mut();
            let values = runtime.entry(component_id.to_owned()).or_insert(current);
            for (state, value) in &transitions {
                values.insert(state.clone(), value.clone());
            }
        }
        self.automation.log(
            "info",
            &format!("component action: {component_id}/{node_id} → {action}"),
        );
        self.set_status(if transitions.is_empty() {
            format!("{name} · {action}")
        } else {
            format!("{name} · {action} · state updated live")
        });
    }

    /// Deepest (smallest-area) stored graph node whose live bounds contain the
    /// window-relative pointer, resolved across instance-expansion prefixes.
    fn canvas_node_at(&self, pointer: McpPoint) -> Option<String> {
        let component_id = self.active_component_id()?;
        let prefix = format!("component/{component_id}/");
        let tree = self.automation.snapshot();
        let mut best: Option<(String, f32)> = None;
        for (runtime_id, node) in &tree.nodes {
            let Some(authored) = runtime_id.strip_prefix(&prefix) else {
                continue;
            };
            let Some(bounds) = node.bounds else {
                continue;
            };
            let inside = pointer.x >= bounds.x
                && pointer.x <= bounds.x + bounds.width
                && pointer.y >= bounds.y
                && pointer.y <= bounds.y + bounds.height;
            if !inside {
                continue;
            }
            let area = bounds.width * bounds.height;
            if best.as_ref().is_none_or(|(_, smallest)| area < *smallest) {
                best = Some((authored.to_owned(), area));
            }
        }
        best.map(|(id, _)| self.resolve_component_node(&component_id, &id))
    }

    /// Update the hovered canvas node from a pointer position. Returns whether
    /// the highlight changed (so callers repaint only when needed).
    fn hover_canvas_at(&self, pointer: McpPoint) -> bool {
        let hovered = self.active_component_id().and_then(|component_id| {
            self.canvas_node_at(pointer)
                .map(|node| format!("component/{component_id}/{node}"))
        });
        let mut current = self.hovered_canvas_node.borrow_mut();
        if *current == hovered {
            false
        } else {
            *current = hovered;
            true
        }
    }

    /// Open a fresh annotation on the element under the pointer (annotate-mode
    /// canvas click). Falls back to a hint when the pointer is over empty space.
    fn annotate_canvas_at(&self, pointer: McpPoint) {
        let Some(component_id) = self.active_component_id() else {
            return;
        };
        let Some(node_id) = self.canvas_node_at(pointer) else {
            self.set_status("Hover an element, then click to annotate it");
            return;
        };
        self.begin_annotation_on(&component_id, &node_id);
    }

    /// Start a new annotation targeting a specific node: auto-save any pending
    /// draft for the previous target, then select this one and open an empty
    /// editor popover.
    fn begin_annotation_on(&self, component_id: &str, node_id: &str) {
        self.commit_annotation();
        self.select_component_node(component_id, node_id);
        self.active_annotation.borrow_mut().take();
        self.annotation_draft.borrow_mut().clear();
        self.annotation_dirty.set(false);
        self.annotation_quiet_ticks.set(0);
        self.annotation_popover_open.set(true);
        self.trace("info", &format!("[annotate] new draft on {node_id}"));
        self.set_status(format!("New annotation on #{node_id} · type a comment"));
    }

    /// Map a rendered node id back to one that exists in the stored graph.
    /// Clicks inside an expanded instance resolve to the instance node itself,
    /// because the expansion's prefixed ids are render-only.
    fn resolve_component_node(&self, component_id: &str, node_id: &str) -> String {
        let library = self.native_components.borrow();
        let Some(component) = library.component(component_id) else {
            return node_id.to_owned();
        };
        if component.node(node_id).is_some() {
            return node_id.to_owned();
        }
        let mut candidate = node_id;
        while let Some((prefix, _)) = candidate.rsplit_once("--") {
            candidate = prefix;
            if component.node(candidate).is_some() {
                return candidate.to_owned();
            }
        }
        node_id.to_owned()
    }

    /// Mode-dependent routing for every pointer gesture on a canvas node:
    /// Design/Source select, Test runs the node's click logic, Compare starts
    /// an annotation on the clicked element, hover feeds the target overlay,
    /// and right-click opens the menu. Returns whether a repaint is needed.
    fn handle_component_pointer(
        &self,
        component_id: &str,
        node_id: &str,
        gesture: ComponentPointerGesture,
    ) -> bool {
        let node_id = self.resolve_component_node(component_id, node_id);
        match gesture {
            ComponentPointerGesture::Select => {
                // Press-to-select only in select-capable modes. Test/Compare
                // deliberately ignore the press so their click/release handlers
                // (run action / start annotation) stay authoritative.
                match self.mode.get() {
                    StudioMode::Design | StudioMode::Source => {
                        self.select_component_node(component_id, &node_id);
                        true
                    }
                    // Move pans the viewport; Test/Compare use click/release.
                    StudioMode::Test | StudioMode::Compare | StudioMode::Move => false,
                }
            }
            ComponentPointerGesture::Click { action } => {
                match self.mode.get() {
                    StudioMode::Test => {
                        if let Some(action) = action {
                            self.invoke_component_action(component_id, &node_id, &action);
                        } else {
                            self.set_status(
                                "Test mode runs interactions · this element has no click action",
                            );
                        }
                    }
                    StudioMode::Compare => {
                        self.begin_annotation_on(component_id, &node_id);
                    }
                    StudioMode::Design | StudioMode::Source | StudioMode::Move => {
                        // Selection already happened on press (`Select`). Doing
                        // it again here would let a release that fell through to
                        // the root container overwrite the correct selection.
                        // Move ignores element clicks entirely (it pans).
                    }
                }
                true
            }
            ComponentPointerGesture::ContextMenu { x, y } => {
                self.select_component_node(component_id, &node_id);
                *self.context_menu.borrow_mut() = Some(ContextMenuState {
                    x,
                    y,
                    target: ContextMenuTarget::ComponentNode {
                        component_id: component_id.to_owned(),
                        node_id,
                    },
                });
                true
            }
            ComponentPointerGesture::Hover => {
                let runtime = format!("component/{component_id}/{node_id}");
                let mut hovered = self.hovered_canvas_node.borrow_mut();
                if hovered.as_deref() == Some(runtime.as_str()) {
                    false
                } else {
                    *hovered = Some(runtime);
                    true
                }
            }
        }
    }

    fn close_context_menu(&self) {
        self.context_menu.borrow_mut().take();
    }

    /// Entries for the open context menu, in display order.
    fn context_menu_actions(&self, target: &ContextMenuTarget) -> Vec<ContextMenuAction> {
        match target {
            ContextMenuTarget::ComponentNode {
                component_id,
                node_id,
            } => {
                let library = self.native_components.borrow();
                let node = library
                    .component(component_id)
                    .and_then(|component| component.node(node_id).cloned());
                let mut actions = Vec::new();
                if node
                    .as_ref()
                    .is_some_and(|node| node.kind == NativeNodeKind::Instance)
                {
                    actions.push(ContextMenuAction::OpenComponent);
                }
                actions.extend([
                    ContextMenuAction::InsertFrame,
                    ContextMenuAction::InsertRow,
                    ContextMenuAction::InsertText,
                    ContextMenuAction::InsertButton,
                    ContextMenuAction::WrapInFrame,
                    ContextMenuAction::Duplicate,
                    ContextMenuAction::Delete,
                    ContextMenuAction::Annotate,
                ]);
                actions
            }
            ContextMenuTarget::ProjectLayer => vec![ContextMenuAction::Annotate],
            ContextMenuTarget::PaletteComponent { .. } => {
                vec![
                    ContextMenuAction::OpenComponent,
                    ContextMenuAction::DeleteComponent,
                ]
            }
        }
    }

    fn run_context_menu_action(&self, action: ContextMenuAction) {
        let target = self
            .context_menu
            .borrow()
            .as_ref()
            .map(|menu| menu.target.clone());
        self.close_context_menu();
        match action {
            ContextMenuAction::OpenComponent => {
                let referenced = target.as_ref().and_then(|target| match target {
                    ContextMenuTarget::ComponentNode {
                        component_id,
                        node_id,
                    } => self
                        .native_components
                        .borrow()
                        .component(component_id)
                        .and_then(|component| component.node(node_id))
                        .and_then(|node| node.instance_of.clone()),
                    ContextMenuTarget::PaletteComponent { component_id } => {
                        Some(component_id.clone())
                    }
                    ContextMenuTarget::ProjectLayer => None,
                });
                if let Some(referenced) = referenced {
                    self.open_component_document(&referenced);
                } else {
                    self.set_status("This node is not a component instance");
                }
            }
            ContextMenuAction::InsertFrame => self.insert_primitive_node(NativeNodeKind::Column),
            ContextMenuAction::InsertRow => self.insert_primitive_node(NativeNodeKind::Row),
            ContextMenuAction::InsertText => self.insert_primitive_node(NativeNodeKind::Text),
            ContextMenuAction::InsertButton => self.insert_primitive_node(NativeNodeKind::Button),
            ContextMenuAction::WrapInFrame => self.wrap_selected_in_frame(),
            ContextMenuAction::Duplicate => self.duplicate_selected_node(),
            ContextMenuAction::Delete => self.delete_selected_node(),
            ContextMenuAction::Annotate => {
                self.set_mode(StudioMode::Compare);
                self.active_annotation.borrow_mut().take();
                self.annotation_draft.borrow_mut().clear();
                self.annotation_popover_open.set(true);
                self.set_status("New annotation on the selected element");
            }
            ContextMenuAction::DeleteComponent => {
                let component_id = target.as_ref().and_then(|target| match target {
                    ContextMenuTarget::PaletteComponent { component_id } => {
                        Some(component_id.clone())
                    }
                    ContextMenuTarget::ComponentNode { .. } | ContextMenuTarget::ProjectLayer => {
                        None
                    }
                });
                if let Some(component_id) = component_id {
                    self.delete_component(&component_id);
                }
            }
        }
    }

    /// Wrap the selected node in a new hug-sized frame via the transactional
    /// Group command, then select the new frame.
    fn wrap_selected_in_frame(&self) {
        let Some(component_id) = self.active_component_id() else {
            self.set_status("Open a component document to wrap nodes");
            return;
        };
        let selected = self.selection.borrow().authored_id.clone();
        if selected == "root" {
            self.set_status("The component root cannot be wrapped");
            return;
        }
        let group = {
            let library = self.native_components.borrow();
            let Some(component) = library.component(&component_id) else {
                self.set_status("The active component no longer exists");
                return;
            };
            if component.node(&selected).is_none() {
                self.set_status("Nothing is selected to wrap");
                return;
            }
            let mut group = NativeNode::authored_container(
                &component.unique_node_id("frame"),
                NativeNodeKind::Column,
            );
            group.layout.width = crate::NativeSize::Hug;
            group.layout.height = crate::NativeSize::Hug;
            group
        };
        let group_id = group.id.clone();
        if let Err(error) = self.apply_graph_commands(
            &component_id,
            "Wrapped in frame",
            vec![GraphCommand::Group {
                ids: vec![selected],
                group,
            }],
        ) {
            self.set_status(format!("Wrap failed: {error}"));
            return;
        }
        self.set_selection(ElementSelection::new(
            group_id.clone(),
            format!("component/{component_id}/{group_id}"),
            1,
            None,
        ));
    }

    fn render_selection_overlay(&self, children: Vec<AnyElement>) -> AnyElement {
        let snapshot = self.selection_snapshot();
        let tree = self.automation.snapshot();
        let selected_rects = self
            .multi_selection
            .borrow()
            .iter()
            .filter_map(|id| {
                tree.nodes
                    .get(id)
                    .and_then(|node| node.bounds)
                    .map(|bounds| (id.clone(), bounds))
            })
            .collect::<Vec<_>>();
        let group = crate::SelectionBounds::from_rects(selected_rects.clone());
        let selection = group
            .as_ref()
            .map(|selection| selection.bounds)
            .or_else(|| snapshot.node.and_then(|node| node.bounds));
        let app_window = snapshot.app_window;

        let mut overlay = div().relative().size_full();
        // Hover target feedback: in annotate mode the outline and chip show
        // exactly which element a click will comment on; in design mode a
        // subtle outline previews what a click will select.
        if let Some(app_window) = app_window
            && let Some(hovered) = self.hovered_canvas_node.borrow().clone()
            && self.multi_selection.borrow().len() <= 1
            && self.selection.borrow().runtime_id != hovered
            && let Some(rect) = tree.nodes.get(&hovered).and_then(|node| node.bounds)
        {
            let annotate = self.mode.get() == StudioMode::Compare;
            let name = hovered.rsplit('/').next().unwrap_or(&hovered).to_owned();
            let (border, chip_bg, chip_fg, chip_text) = if annotate {
                (
                    rgb(0xf3_a3_4d),
                    rgb(0xf3_a3_4d),
                    rgb(0x16_10_09),
                    format!("Annotate #{name}"),
                )
            } else {
                (
                    rgba(0x6e_7b_ff_90),
                    rgb(0x27_2c_45),
                    rgb(0xc9_ce_e2),
                    format!("#{name}"),
                )
            };
            overlay = overlay
                .child(
                    div()
                        .absolute()
                        .left(px(rect.x - app_window.x - 1.0))
                        .top(px(rect.y - app_window.y - 1.0))
                        .w(px(rect.width + 2.0))
                        .h(px(rect.height + 2.0))
                        .rounded(px(3.0))
                        .border_2()
                        .border_color(border),
                )
                .child(
                    div()
                        .absolute()
                        .left(px((rect.x - app_window.x - 1.0).max(0.0)))
                        .top(px((rect.y - app_window.y - 21.0).max(0.0)))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .bg(chip_bg)
                        .text_size(px(10.5))
                        .text_color(chip_fg)
                        .child(chip_text),
                );
        }
        // While a drag is live, ring the container that will receive the drop
        // and float a chip at the cursor showing the drop order / z-index.
        if let Some(app_window) = app_window
            && let Some(preview) = self.drag_preview.borrow().clone()
            && let Some(component_id) = self.active_component_id()
        {
            if let Some(rect) = tree
                .nodes
                .get(&format!("component/{component_id}/{}", preview.parent))
                .and_then(|node| node.bounds)
            {
                overlay = overlay.child(
                    div()
                        .absolute()
                        .left(px(rect.x - app_window.x - 2.0))
                        .top(px(rect.y - app_window.y - 2.0))
                        .w(px(rect.width + 4.0))
                        .h(px(rect.height + 4.0))
                        .rounded(px(5.0))
                        .border_2()
                        .border_color(rgba(0x6e_7b_ff_b0)),
                );
            }
            let (position, total) = preview.position_label();
            let (kind, glyph) = if preview.is_stack {
                ("z-index", "⧉")
            } else {
                ("order", "⋮")
            };
            let chip_left = (preview.pointer_x - app_window.x + 16.0).max(2.0);
            let chip_top = (preview.pointer_y - app_window.y + 18.0).max(2.0);
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(px(chip_left))
                    .top(px(chip_top))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(7.0))
                    .bg(rgb(0x14_16_1d))
                    .border_1()
                    .border_color(rgb(0x6e_7b_ff))
                    .text_size(px(11.5))
                    .text_color(rgb(0xe7_e9_ee))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(0x8a_93_ff))
                            .child(glyph),
                    )
                    .child(format!("{kind} {position} / {total}"))
                    .child(
                        div()
                            .text_size(px(9.5))
                            .text_color(rgb(0x67_6d_7a))
                            .child("scroll ↕"),
                    ),
            );
        }
        if let (Some(selection), Some(app_window)) = (selection, app_window) {
            let left = selection.x - app_window.x;
            let top = selection.y - app_window.y;
            if selection.width > 0.0 && selection.height > 0.0 {
                overlay = overlay
                    .children(
                        (group.as_ref().is_some_and(|group| group.ids.len() > 1))
                            .then(|| {
                                selected_rects.into_iter().map(|(_, rect)| {
                                    div()
                                        .absolute()
                                        .left(px(rect.x - app_window.x))
                                        .top(px(rect.y - app_window.y))
                                        .w(px(rect.width))
                                        .h(px(rect.height))
                                        .border_1()
                                        .border_color(rgba(0x6e_7b_ff_70))
                                })
                            })
                            .into_iter()
                            .flatten(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(left - 1.0))
                            .top(px(top - 1.0))
                            .w(px(selection.width + 2.0))
                            .h(px(selection.height + 2.0))
                            .rounded(px(3.0))
                            .border_2()
                            .border_color(rgb(0x6e_7b_ff))
                            .bg(rgba(0x6e_7b_ff_1f)),
                    )
                    .child(
                        children.into_iter().fold(
                            div()
                                .absolute()
                                .left(px(left.max(0.0)))
                                .top(px((top - 22.0).max(0.0))),
                            gpui::ParentElement::child,
                        ),
                    );
            }
        }
        overlay.into_any_element()
    }

    fn render_annotation_layer(self: &Rc<Self>, children: Vec<AnyElement>) -> AnyElement {
        let tree = self.automation.snapshot();
        let viewport = tree
            .nodes
            .get("canvas-viewport")
            .and_then(|node| node.bounds);
        let resolved = self.annotations.borrow().resolved_active(&tree);
        let mut layer = div().relative().size_full();

        if let Some(viewport) = viewport {
            // In annotate mode a full-canvas crosshair layer owns hover + click
            // so the cursor is constant and clicking anywhere comments on the
            // element beneath it. It renders first (below the badges/popover),
            // so existing annotation chips stay clickable to edit.
            if self.mode.get() == StudioMode::Compare
                && let Some(frame) = tree.nodes.get("app-window").and_then(|node| node.bounds)
            {
                let hover_state = self.clone();
                let click_state = self.clone();
                layer = layer.child(
                    div()
                        .id("annotate-capture")
                        .absolute()
                        .left(px(frame.x - viewport.x))
                        .top(px(frame.y - viewport.y))
                        .w(px(frame.width))
                        .h(px(frame.height))
                        .cursor(gpui::CursorStyle::Crosshair)
                        .on_mouse_move(move |event, window, cx| {
                            // Own hover in annotate mode; keep it from reaching
                            // the now-interactive nodes beneath.
                            cx.stop_propagation();
                            let pointer = McpPoint {
                                x: f32::from(event.position.x),
                                y: f32::from(event.position.y),
                            };
                            if hover_state.hover_canvas_at(pointer) {
                                window.refresh();
                            }
                        })
                        .on_click(move |event, window, cx| {
                            cx.stop_propagation();
                            let position = event.position();
                            click_state.annotate_canvas_at(McpPoint {
                                x: f32::from(position.x),
                                y: f32::from(position.y),
                            });
                            window.refresh();
                        }),
                );
            }
            let active_id = self.active_annotation.borrow().clone();
            for (position, annotation) in resolved.iter().enumerate() {
                let Some(rect) = annotation.current_rect else {
                    continue;
                };
                let id = annotation.annotation.id.clone();
                let is_active = active_id.as_deref() == Some(id.as_str());
                // Outline the exact annotated element so the review target is
                // unambiguous; the active annotation gets a stronger ring.
                let outline = div()
                    .absolute()
                    .left(px(rect.x - viewport.x - 2.0))
                    .top(px(rect.y - viewport.y - 2.0))
                    .w(px(rect.width + 4.0))
                    .h(px(rect.height + 4.0))
                    .rounded(px(4.0))
                    .border_2()
                    .border_color(if is_active {
                        rgb(0xf3_a3_4d)
                    } else {
                        rgba(0xf3_a3_4d_66)
                    });
                let outline = if is_active {
                    outline.bg(rgba(0xf3_a3_4d_14))
                } else {
                    outline
                };
                layer = layer.child(outline);
                // Number badges by their position in the active queue (1-based)
                // so the highest badge number equals the "Send N" count. The raw
                // id sequence would keep gaps as annotations resolve, making a
                // lone "#5" read as five open comments when only two remain.
                let number = SharedString::from((position + 1).to_string());
                let user_state = self.clone();
                let user_id = id.clone();
                let badge = div()
                    .id(SharedString::from(format!("studio-annotation-{id}")))
                    .absolute()
                    .left(px(rect.x - viewport.x + rect.width - 11.0))
                    .top(px(rect.y - viewport.y - 11.0))
                    .w(px(22.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(11.0))
                    .bg(rgb(0xf3_a3_4d))
                    .text_color(rgb(0x16_10_09))
                    .text_size(px(11.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xff_c0_6e)))
                    .child(number.clone())
                    .on_click(move |_, window, cx| {
                        // Stop the annotate-mode capture layer beneath from also
                        // starting a new annotation on this badge's element.
                        cx.stop_propagation();
                        user_state.activate_annotation(&user_id);
                        window.refresh();
                    })
                    .semantic_role(SemanticRole::Button)
                    .accessible_name(format!("Annotation {number}"))
                    .semantic_metadata("annotation_id", id);
                layer = layer.child(badge);
            }

            let target = self
                .active_annotation
                .borrow()
                .as_ref()
                .and_then(|id| {
                    resolved
                        .iter()
                        .find(|resolved| resolved.annotation.id == *id)
                        .and_then(|resolved| resolved.current_rect)
                })
                .or_else(|| {
                    let selection_id = self.selection.borrow().runtime_id.clone();
                    tree.nodes.get(&selection_id).and_then(|node| node.bounds)
                });
            let (left, top) = target.map_or((24.0, 24.0), |rect| {
                let right = rect.x - viewport.x + rect.width + 16.0;
                let left = rect.x - viewport.x;
                let popover_left = if right + 278.0 <= viewport.width {
                    right
                } else {
                    (left - 294.0).max(6.0)
                };
                (
                    popover_left,
                    (rect.y - viewport.y - 6.0)
                        .max(6.0)
                        .min((viewport.height - 230.0).max(6.0)),
                )
            });
            layer = layer.child(
                children
                    .into_iter()
                    .fold(
                        div().absolute().left(px(left)).top(px(top)).w(px(278.0)),
                        gpui::ParentElement::child,
                    )
                    // Keep clicks inside the editor from reaching the annotate
                    // capture layer beneath (which would start a new annotation).
                    .id("annotation-editor-surface")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            );
        }
        layer.into_any_element()
    }

    fn create_document_component(&self) {
        self.component_dialog_open.set(false);
        let root = self.project.borrow().root().to_owned();
        let requested_name = self
            .valid_component_name()
            .unwrap_or_else(|| "Component".to_owned());
        let props = parse_component_props(&self.component_props.borrow());
        let (id, name) = {
            let mut library = self.native_components.borrow_mut();
            let component = match self.component_source.get() {
                ComponentSource::Titlebar => {
                    library.create_titlebar_component_with_props(requested_name, props)
                }
                ComponentSource::Preset => library.create_preset_component_with_props(
                    self.component_preset.get(),
                    requested_name,
                    props,
                ),
                ComponentSource::Selection
                | ComponentSource::Blank
                | ComponentSource::PasteHtml => {
                    library.create_named_component_with_props(requested_name, props)
                }
            };
            (component.id.clone(), component.name.clone())
        };
        if let Err(error) = self.native_components.borrow().save(&root) {
            self.set_status(format!("Create component document failed: {error}"));
            return;
        }
        self.open_component_document(&id);
        self.set_status(format!(
            "Created {name} at runtime · one graph, HTML and GPUI projections · saved to .gpui-studio/components.ron"
        ));
        self.trace("info", &format!("[library] created {name}"));
    }

    /// Enter annotate mode; the annotation popover opens when an element on
    /// the canvas is clicked, anchored to that exact element.
    fn enter_annotate_mode(&self) {
        self.set_mode(StudioMode::Compare);
        self.annotation_popover_open.set(false);
        self.set_status("Annotate mode · click an element to comment on it");
    }

    /// Global editor shortcuts: V/H/C switch tools (matching the toolbar
    /// tooltips) and Escape dismisses overlays. Skipped while a text input has
    /// focus so typing never changes modes.
    fn handle_global_keystroke(
        &self,
        event: &gpui::KeystrokeEvent,
        window: &mut Window,
        _cx: &App,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers != gpui::Modifiers::default() {
            return;
        }
        if keystroke.key == "escape" {
            if self.context_menu.borrow().is_some()
                || self.annotation_popover_open.get()
                || self.drag_preview.borrow().is_some()
            {
                self.close_context_menu();
                self.annotation_popover_open.set(false);
                self.clear_drag_preview();
                window.refresh();
            }
            return;
        }
        // While a drop preview is live, [ and ] nudge the insertion order
        // (stacking order in a Stack container).
        if matches!(keystroke.key.as_str(), "[" | "]") && self.drag_preview.borrow().is_some() {
            let delta = if keystroke.key == "[" { -1 } else { 1 };
            if self.adjust_drag_preview_order(delta) {
                window.refresh();
            }
            return;
        }
        // Only text entry may swallow tool shortcuts: block V/M/C/P while a
        // node advertising text editing has keyboard focus.
        let typing = self.automation.snapshot().nodes.values().any(|node| {
            node.state.focused
                && node
                    .actions
                    .iter()
                    .any(|action| matches!(action, NodeAction::SetText | NodeAction::SetValue))
        });
        if typing {
            return;
        }
        match keystroke.key.as_str() {
            "v" => {
                self.set_mode(StudioMode::Design);
                window.refresh();
            }
            "m" => {
                self.enter_move_tool();
                window.refresh();
            }
            "c" => {
                self.enter_annotate_mode();
                window.refresh();
            }
            "p" => {
                self.set_mode(StudioMode::Test);
                window.refresh();
            }
            _ => {}
        }
    }

    fn activate_annotation(&self, id: &str) {
        // Save any pending draft before switching to the clicked annotation.
        self.commit_annotation();
        let annotation = self
            .annotations
            .borrow()
            .annotations
            .iter()
            .find(|annotation| annotation.id == id)
            .map(|annotation| (annotation.comment.clone(), annotation.target.clone()));
        let Some((comment, target)) = annotation else {
            self.set_status(format!("Annotation {id} no longer exists"));
            return;
        };
        *self.active_annotation.borrow_mut() = Some(id.to_owned());
        *self.annotation_draft.borrow_mut() = comment;
        self.annotation_dirty.set(false);
        self.annotation_quiet_ticks.set(0);
        self.annotation_popover_open.set(true);
        self.mode.set(StudioMode::Compare);
        // Reveal the exact annotated element: selecting it lights up the
        // selection overlay on the canvas and the matching layer-tree row.
        *self.multi_selection.borrow_mut() = BTreeSet::from([target.runtime_id.clone()]);
        *self.multi_selection_anchor.borrow_mut() = Some(target.runtime_id.clone());
        *self.project_tree_focus.borrow_mut() = Some(target.runtime_id.clone());
        self.set_selection(target);
    }

    fn close_annotation(&self) {
        self.commit_annotation();
        self.annotation_popover_open.set(false);
    }

    /// Auto-persist the current draft after a short quiet period, so typing a
    /// comment IS the save. Returns whether a repaint is needed.
    fn flush_annotation_if_quiet(&self) -> bool {
        if !self.annotation_dirty.get() {
            return false;
        }
        let ticks = self.annotation_quiet_ticks.get().saturating_add(1);
        self.annotation_quiet_ticks.set(ticks);
        if ticks < 2 {
            return false;
        }
        self.commit_annotation();
        true
    }

    /// Create or update the annotation for the current draft against the
    /// selected element. A blank draft is a no-op (nothing to persist), and an
    /// existing annotation is never emptied. Clears the dirty flag.
    fn commit_annotation(&self) {
        self.annotation_dirty.set(false);
        self.annotation_quiet_ticks.set(0);
        let comment = self.annotation_draft.borrow().trim().to_owned();
        if comment.is_empty() {
            return;
        }
        let tree = self.automation.snapshot();
        let mut selection = self.selection.borrow().clone();
        selection.captured_rect = tree
            .nodes
            .get(&selection.runtime_id)
            .and_then(|node| node.bounds);
        let root = self.project.borrow().root().to_owned();
        let mut candidate = self.annotations.borrow().clone();
        let active_id = self.active_annotation.borrow().clone();
        let id = match active_id {
            Some(id) => match candidate.update_comment(&id, &comment) {
                Ok(annotation) => annotation.id.clone(),
                Err(error) => {
                    self.set_status(format!("Update annotation rejected: {error}"));
                    return;
                }
            },
            None => match candidate.add(selection, &comment, NormalizedAnchor::default()) {
                Ok(annotation) => annotation.id.clone(),
                Err(error) => {
                    self.set_status(format!("Add annotation rejected: {error}"));
                    return;
                }
            },
        };
        if let Err(error) = candidate.save(&root) {
            self.set_status(format!("Persist annotation failed: {error}"));
            return;
        }
        *self.annotations.borrow_mut() = candidate;
        *self.active_annotation.borrow_mut() = Some(id.clone());
        self.set_status("Annotation saved · Send to hand off to the agent");
        self.trace("info", &format!("[annotate] saved {id}"));
    }

    /// Remove the active annotation by archiving it out of the review queue.
    fn delete_annotation(&self) {
        let id = self.active_annotation.borrow().clone();
        self.annotation_dirty.set(false);
        self.annotation_quiet_ticks.set(0);
        let Some(id) = id else {
            // Nothing persisted yet — just discard the draft and close.
            self.annotation_draft.borrow_mut().clear();
            self.annotation_popover_open.set(false);
            return;
        };
        let mut candidate = self.annotations.borrow().clone();
        if let Err(error) = candidate.remove(&id) {
            self.set_status(format!("Remove annotation rejected: {error}"));
            return;
        }
        let root = self.project.borrow().root().to_owned();
        if let Err(error) = candidate.save(&root) {
            self.set_status(format!("Persist annotation removal failed: {error}"));
            return;
        }
        *self.annotations.borrow_mut() = candidate;
        self.active_annotation.borrow_mut().take();
        self.annotation_draft.borrow_mut().clear();
        self.annotation_popover_open.set(false);
        self.trace("info", &format!("[annotate] removed {id}"));
        self.set_status(format!("Removed annotation {id}"));
    }

    fn send_annotations(&self) {
        // Commit any in-flight draft so it's included in the handoff.
        self.commit_annotation();
        let tree = self.automation.snapshot();
        let project_revision = self.target().map_or(1, |target| target.revision());
        let root = self.project.borrow().root().to_owned();
        let mut candidate = self.annotation_handoffs.borrow().clone();
        let (id, count) = match candidate.publish(
            &self.annotations.borrow(),
            project_revision,
            tree.generation.max(1),
        ) {
            Ok(handoff) => (handoff.id.clone(), handoff.annotation_ids.len()),
            Err(error) => {
                self.set_status(format!("Send annotations rejected: {error}"));
                return;
            }
        };
        if let Err(error) = candidate.save(&root) {
            self.set_status(format!("Persist MCP handoff failed: {error}"));
            return;
        }
        // Reflect the handoff in each open task's status so the queue shows what
        // has been sent. The MCP transport is pull-based — the app publishes the
        // handoff resource and the connected agent reads it; there is no push.
        let mut annotations = self.annotations.borrow().clone();
        let open_ids = annotations
            .active()
            .filter(|annotation| annotation.status == AnnotationStatus::Open)
            .map(|annotation| annotation.id.clone())
            .collect::<Vec<_>>();
        for open_id in &open_ids {
            let _ = annotations.transition(open_id, AnnotationStatus::InProgress);
        }
        if annotations.save(&root).is_ok() {
            *self.annotations.borrow_mut() = annotations;
        }
        *self.annotation_handoffs.borrow_mut() = candidate;
        self.trace(
            "info",
            &format!("[handoff] published {id} with {count} annotations"),
        );
        self.set_status(format!(
            "Handed off {count} annotations to the agent · resource gpui-studio://annotations/handoff/latest"
        ));
    }

    /// Transition one annotation's review status by id. Used by the MCP
    /// `annotation.update` command so the agent can mark work in progress or
    /// done as it addresses each comment.
    fn transition_annotation(&self, id: &str, next: AnnotationStatus) -> Result<(), BridgeError> {
        let mut candidate = self.annotations.borrow().clone();
        candidate
            .transition(id, next)
            .map_err(|error| BridgeError::new(ErrorCode::InvalidRequest, error.to_string()))?;
        let root = self.project.borrow().root().to_owned();
        candidate
            .save(&root)
            .map_err(|error| BridgeError::new(ErrorCode::Internal, error.to_string()))?;
        *self.annotations.borrow_mut() = candidate;
        if matches!(next, AnnotationStatus::Done | AnnotationStatus::Archived)
            && self.active_annotation.borrow().as_deref() == Some(id)
        {
            self.annotation_popover_open.set(false);
            self.active_annotation.borrow_mut().take();
            self.annotation_draft.borrow_mut().clear();
        }
        self.trace("info", &format!("[annotate] {id} → {next:?}"));
        self.set_status(format!("Review task {id} → {next:?}"));
        Ok(())
    }

    fn context_resource(
        &self,
        request: ContextResourceRequest,
    ) -> Result<ContextResourceResponse, BridgeError> {
        match request {
            ContextResourceRequest::List => Ok(ContextResourceResponse::List(
                studio_resource_uris()
                    .iter()
                    .filter_map(|uri| self.build_resource(uri).ok())
                    .map(|resource| resource.descriptor)
                    .collect(),
            )),
            ContextResourceRequest::Read { uri } => self
                .build_resource(&uri)
                .map(ContextResourceResponse::Resource),
        }
    }

    fn application_command(
        &self,
        request: ApplicationCommandRequest,
    ) -> Result<ApplicationCommandResponse, BridgeError> {
        match request {
            ApplicationCommandRequest::List => Ok(ApplicationCommandResponse::List(
                studio_application_commands(),
            )),
            ApplicationCommandRequest::Execute { name, arguments } => self
                .execute_application_command(&name, arguments)
                .map(ApplicationCommandResponse::Result),
        }
    }

    fn execute_application_command(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ApplicationCommandResult, BridgeError> {
        match name {
            "component.graph.get" => {
                let args: ComponentGraphTarget = decode_command_arguments(arguments)?;
                self.synchronize_component_graph(&args.component_id)?;
                let graphs = self.component_graphs.borrow();
                let graph = graphs.get(&args.component_id).ok_or_else(|| {
                    BridgeError::new(ErrorCode::NotFound, "component graph was not found")
                })?;
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: Some(graph.revision()),
                    output: serde_json::json!({
                        "component_id": args.component_id,
                        "root": graph.root(),
                    }),
                })
            }
            "component.graph.apply" => {
                let args: ComponentGraphMutation = decode_command_arguments(arguments)?;
                self.synchronize_component_graph(&args.component_id)?;
                let (change, root) = {
                    let mut graphs = self.component_graphs.borrow_mut();
                    let graph = graphs.get_mut(&args.component_id).ok_or_else(|| {
                        BridgeError::new(ErrorCode::NotFound, "component graph was not found")
                    })?;
                    let change = graph.apply(args.transaction).map_err(graph_bridge_error)?;
                    (change, graph.root().clone())
                };
                self.publish_component_graph(&args.component_id, root)?;
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: Some(change.revision),
                    output: serde_json::json!({ "change": change }),
                })
            }
            "component.graph.undo" | "component.graph.redo" => {
                let args: ComponentGraphTarget = decode_command_arguments(arguments)?;
                self.synchronize_component_graph(&args.component_id)?;
                let (revision, root) = {
                    let mut graphs = self.component_graphs.borrow_mut();
                    let graph = graphs.get_mut(&args.component_id).ok_or_else(|| {
                        BridgeError::new(ErrorCode::NotFound, "component graph was not found")
                    })?;
                    let revision = if name.ends_with("undo") {
                        graph.undo()
                    } else {
                        graph.redo()
                    }
                    .map_err(graph_bridge_error)?;
                    (revision, graph.root().clone())
                };
                self.publish_component_graph(&args.component_id, root)?;
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: Some(revision),
                    output: serde_json::json!({
                        "component_id": args.component_id,
                        "revision": revision,
                    }),
                })
            }
            "component.remove" => {
                let args: ComponentGraphTarget = decode_command_arguments(arguments)?;
                let removed = self
                    .remove_component_checked(&args.component_id)
                    .map_err(|error| BridgeError::new(ErrorCode::InvalidRequest, error))?;
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: None,
                    output: serde_json::json!({
                        "component_id": args.component_id,
                        "removed": removed,
                    }),
                })
            }
            "component.create" => {
                let args: CreateComponentCommand = decode_command_arguments(arguments)?;
                let name_value = args.name.trim();
                if name_value.is_empty() || name_value.len() > 128 {
                    return Err(BridgeError::new(
                        ErrorCode::InvalidRequest,
                        "component name must contain 1 through 128 bytes",
                    ));
                }
                let mut candidate = self.native_components.borrow().clone();
                let component = if let Some(preset) = args.preset.as_deref() {
                    let preset = parse_component_preset(preset)?;
                    candidate.create_preset_component(preset, name_value)
                } else {
                    candidate.create_named_component(name_value)
                }
                .clone();
                let project_root = self.project.borrow().root().to_owned();
                candidate.save(&project_root).map_err(|error| {
                    BridgeError::new(
                        ErrorCode::Internal,
                        format!("component could not be persisted: {error}"),
                    )
                })?;
                *self.native_components.borrow_mut() = candidate;
                self.component_graphs.borrow_mut().insert(
                    component.id.clone(),
                    ComponentGraph::new(component.root.clone(), 1),
                );
                self.open_component_document(&component.id);
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: Some(1),
                    output: serde_json::json!({ "component": component }),
                })
            }
            "selection.set" => {
                let args: SelectLayerCommand = decode_command_arguments(arguments)?;
                let tree = self.automation.snapshot();
                let model = LayerTree::from_semantics(&tree, "project-canvas");
                let mut expanded = self.project_tree_expanded.borrow().clone();
                if !self.project_tree_initialized.get() {
                    expanded.extend(model.expandable_ids());
                }
                let row = model
                    .visible_rows(&expanded)
                    .into_iter()
                    .find(|row| row.runtime_id == args.runtime_id)
                    .ok_or_else(|| {
                        BridgeError::new(ErrorCode::NotFound, "project layer was not found")
                    })?;
                self.select_project_layer(&model, &row, false, false);
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: self.target().map(|target| target.revision()),
                    output: serde_json::json!({
                        "selection": self.selection.borrow().clone(),
                    }),
                })
            }
            "selection.marquee" => {
                let args: MarqueeSelectionCommand = decode_command_arguments(arguments)?;
                let tree = self.automation.snapshot();
                let model = LayerTree::from_semantics(&tree, "project-canvas");
                let rows = model.visible_rows(&model.expandable_ids());
                let candidates = rows
                    .iter()
                    .filter_map(|row| {
                        tree.nodes
                            .get(&row.runtime_id)
                            .and_then(|node| node.bounds)
                            .map(|bounds| (row.runtime_id.clone(), bounds))
                    })
                    .collect::<Vec<_>>();
                let engine = crate::PlacementEngine::rebuild(candidates);
                let mut ids = engine.intersecting(args.rect);
                ids.sort();
                ids.dedup();
                if args.additive.unwrap_or(false) {
                    self.multi_selection
                        .borrow_mut()
                        .extend(ids.iter().cloned());
                } else {
                    *self.multi_selection.borrow_mut() = ids.iter().cloned().collect();
                }
                if let Some(row) = rows
                    .iter()
                    .find(|row| ids.first().is_some_and(|id| id == &row.runtime_id))
                {
                    self.select_project_layer(&model, row, false, false);
                    self.multi_selection
                        .borrow_mut()
                        .extend(ids.iter().cloned());
                }
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: self.target().map(|target| target.revision()),
                    output: serde_json::json!({
                        "selection_ids": self.multi_selection.borrow().clone(),
                        "count": self.multi_selection.borrow().len(),
                    }),
                })
            }
            "layout.suggest_drop" => {
                let args: SuggestDropCommand = decode_command_arguments(arguments)?;
                let tree = self.automation.snapshot();
                if !tree.nodes.contains_key(&args.parent) {
                    return Err(BridgeError::new(
                        ErrorCode::NotFound,
                        "drop parent was not found in the live semantic tree",
                    ));
                }
                if args.child_ids.iter().any(|id| !tree.nodes.contains_key(id)) {
                    return Err(BridgeError::new(
                        ErrorCode::NotFound,
                        "one or more drop children were not found",
                    ));
                }
                let engine = crate::PlacementEngine::rebuild(
                    tree.nodes
                        .values()
                        .filter_map(|node| node.bounds.map(|bounds| (node.id.clone(), bounds))),
                );
                let placement =
                    engine.drop_placement(args.parent, args.layout, &args.child_ids, args.pointer);
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: self.target().map(|target| target.revision()),
                    output: serde_json::json!({ "placement": placement }),
                })
            }
            "layout.snap" => {
                let args: SnapSelectionCommand = decode_command_arguments(arguments)?;
                if args.ids.is_empty() || args.ids.len() > 256 {
                    return Err(BridgeError::new(
                        ErrorCode::InvalidRequest,
                        "snap requires 1 through 256 selected IDs",
                    ));
                }
                let tree = self.automation.snapshot();
                let rects = args
                    .ids
                    .iter()
                    .filter_map(|id| {
                        tree.nodes
                            .get(id)
                            .and_then(|node| node.bounds)
                            .map(|bounds| (id.clone(), bounds))
                    })
                    .collect::<Vec<_>>();
                let moving = crate::SelectionBounds::from_rects(rects).ok_or_else(|| {
                    BridgeError::new(ErrorCode::NotFound, "selected live bounds were not found")
                })?;
                if moving.ids.len() != args.ids.iter().collect::<BTreeSet<_>>().len() {
                    return Err(BridgeError::new(
                        ErrorCode::NotFound,
                        "one or more selected live bounds were not found",
                    ));
                }
                let engine = crate::PlacementEngine::rebuild(
                    tree.nodes
                        .values()
                        .filter_map(|node| node.bounds.map(|bounds| (node.id.clone(), bounds))),
                );
                let result = engine.snap(
                    &moving,
                    args.proposed_x,
                    args.proposed_y,
                    args.threshold.unwrap_or(6.0).clamp(0.0, 64.0),
                    args.grid,
                );
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: self.target().map(|target| target.revision()),
                    output: serde_json::json!({ "snap": result }),
                })
            }
            "annotation.update" => {
                let args: UpdateAnnotationCommand = decode_command_arguments(arguments)?;
                let status = match args.status.as_str() {
                    "open" => AnnotationStatus::Open,
                    "in_progress" => AnnotationStatus::InProgress,
                    "done" => AnnotationStatus::Done,
                    "archived" => AnnotationStatus::Archived,
                    _ => {
                        return Err(BridgeError::new(
                            ErrorCode::InvalidRequest,
                            "status must be open, in_progress, done, or archived",
                        ));
                    }
                };
                self.transition_annotation(&args.id, status)?;
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: None,
                    output: serde_json::json!({ "id": args.id, "status": args.status }),
                })
            }
            "project.parity" => {
                let args: ComponentGraphTarget = decode_command_arguments(arguments)?;
                let library = self.native_components.borrow();
                let component = library
                    .components
                    .iter()
                    .find(|component| component.id == args.component_id)
                    .ok_or_else(|| {
                        BridgeError::new(ErrorCode::NotFound, "component was not found")
                    })?;
                let html = component.html_projection();
                let css = component.css_projection();
                let gpui = component.gpui_excerpt();
                let bindings = component.bindings_projection();
                let node_ids = component_node_ids(&component.root);
                let missing_html = node_ids
                    .iter()
                    .filter(|id| !html.contains(&format!("id=\"{id}\"")))
                    .cloned()
                    .collect::<Vec<_>>();
                let missing_gpui = node_ids
                    .iter()
                    .filter(|id| !gpui.contains(&format!("component/{}/{}", args.component_id, id)))
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(ApplicationCommandResult {
                    name: name.to_owned(),
                    revision: self
                        .component_graphs
                        .borrow()
                        .get(&args.component_id)
                        .map(ComponentGraph::revision),
                    output: serde_json::json!({
                        "component_id": args.component_id,
                        "parity": missing_html.is_empty() && missing_gpui.is_empty(),
                        "node_count": node_ids.len(),
                        "missing_html_ids": missing_html,
                        "missing_gpui_ids": missing_gpui,
                        "projections": {
                            "html": html,
                            "css": css,
                            "gpui": gpui,
                            "bindings_ron": bindings,
                        },
                    }),
                })
            }
            _ => Err(BridgeError::new(
                ErrorCode::NotFound,
                "Studio application command was not found",
            )),
        }
    }

    fn synchronize_component_graph(&self, component_id: &str) -> Result<(), BridgeError> {
        let root = self
            .native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.id == component_id)
            .map(|component| component.root.clone())
            .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, "component was not found"))?;
        let mut graphs = self.component_graphs.borrow_mut();
        let stale = graphs
            .get(component_id)
            .is_none_or(|graph| graph.root() != &root);
        if stale {
            let revision = graphs
                .get(component_id)
                .map_or(1, |graph| graph.revision().saturating_add(1));
            graphs.insert(component_id.to_owned(), ComponentGraph::new(root, revision));
        }
        Ok(())
    }

    fn publish_component_graph(
        &self,
        component_id: &str,
        root: NativeNode,
    ) -> Result<(), BridgeError> {
        let mut library = self.native_components.borrow_mut();
        let component = library
            .components
            .iter_mut()
            .find(|component| component.id == component_id)
            .ok_or_else(|| BridgeError::new(ErrorCode::NotFound, "component was not found"))?;
        component.root = root;
        self.component_edit_dirty.set(true);
        self.component_edit_quiet_ticks.set(0);
        self.component_tree_initialized_for.borrow_mut().take();
        self.selection_snapshot.borrow_mut().take();
        self.set_status(format!("Component {component_id} live graph published"));
        Ok(())
    }

    fn build_resource(&self, uri: &str) -> Result<ContextResource, BridgeError> {
        let tree = self.automation.snapshot();
        let (name, title, description, value) = match uri {
            "gpui-studio://project/manifest" => (
                "studio-project-manifest",
                "GPUI Studio project manifest",
                "Canonical component graph, active source projection, viewport, and project revision.",
                serde_json::json!({
                    "schema_version": 6,
                    "component_schema_version": self.native_components.borrow().version,
                    "authoring_projection": format!("{:?}", self.backend.get()).to_lowercase(),
                    "runtime_model": "one-component-graph",
                    "available_projections": ["html-css-ron", "gpui"],
                    "application_commands": studio_application_commands().into_iter().map(|command| command.name).collect::<Vec<_>>(),
                    "html_revision": self.target().map(|target| target.revision()),
                    "project_root": self.project.borrow().root(),
                    "components": self.native_components.borrow().components,
                    "design_tokens": self.native_components.borrow().tokens,
                    "graph_revisions": self.component_graphs.borrow().iter().map(|(id, graph)| (id.clone(), graph.revision())).collect::<BTreeMap<_, _>>(),
                    "canvas": self.canvas.get(),
                    "window_output": {
                        "native_decorations": self.canvas.get().decorations == OutputDecorations::Native,
                        "titlebar": "document-semantic-component",
                        "windows_macos_titlebar_appears_transparent": self.canvas.get().decorations.titlebar_appears_transparent(),
                        "linux_window_decorations": self.canvas.get().decorations.linux_policy(),
                    },
                    "editor_theme": self.resolved_theme(),
                }),
            ),
            "gpui-studio://selection" => {
                let selection = self.selection.borrow().clone();
                let current_rect = tree
                    .nodes
                    .get(&selection.runtime_id)
                    .and_then(|node| node.bounds);
                (
                    "studio-selection",
                    "Current Studio selection",
                    "Stable authored/runtime identity plus current semantic generation and bounds.",
                    serde_json::json!({
                        "schema_version": 1,
                        "selection": selection,
                        "current_rect": current_rect,
                        "semantic_generation": tree.generation,
                    }),
                )
            }
            "gpui-studio://tasks/active" | "gpui-studio://annotations/active" => {
                let annotations = self.annotations.borrow().resolved_active(&tree);
                (
                    "studio-active-annotations",
                    "Active Studio annotations",
                    "Open and in-progress spatial comments resolved against current GPUI bounds.",
                    serde_json::json!({
                        "schema_version": 2,
                        "counts": self.annotations.borrow().counts(),
                        "annotations": annotations,
                        "tasks": annotations,
                    }),
                )
            }
            "gpui-studio://tasks/history" | "gpui-studio://annotations/history" => {
                let annotations = self.annotations.borrow().resolved(&tree);
                (
                    "studio-annotation-history",
                    "Studio annotation history",
                    "Completed and archived spatial comments retained without polluting active AI context.",
                    serde_json::json!({
                        "schema_version": 2,
                        "counts": self.annotations.borrow().counts(),
                        "annotations": annotations,
                        "tasks": annotations,
                    }),
                )
            }
            "gpui-studio://annotations/handoff/latest" => self.annotation_handoff_resource(&tree),
            "gpui-studio://theme" => (
                "studio-editor-theme",
                "Studio editor theme",
                "Active editor-chrome theme, resolved tokens, and available offline variants.",
                serde_json::json!({
                    "schema_version": 1,
                    "resolved": self.resolved_theme(),
                    "available": self.themes.borrow().available(),
                }),
            ),
            _ => {
                return Err(BridgeError::new(
                    ErrorCode::NotFound,
                    "Studio context resource was not found",
                ));
            }
        };
        let text = serde_json::to_string_pretty(&value).map_err(|_| {
            BridgeError::new(
                ErrorCode::Internal,
                "Studio context resource could not be serialized",
            )
        })?;
        Ok(ContextResource {
            descriptor: ContextResourceDescriptor {
                uri: uri.to_owned(),
                name: name.to_owned(),
                title: Some(title.to_owned()),
                description: Some(description.to_owned()),
                mime_type: "application/json".to_owned(),
                size: Some(text.len() as u64),
            },
            text,
        })
    }

    fn annotation_handoff_resource(
        &self,
        tree: &gpui_mcp::UiTree,
    ) -> (&'static str, &'static str, &'static str, serde_json::Value) {
        let latest = self.annotation_handoffs.borrow().latest.clone();
        let annotations = latest.as_ref().map_or_else(Vec::new, |handoff| {
            self.annotations
                .borrow()
                .resolved_ids(&handoff.annotation_ids, tree)
        });
        (
            "studio-annotation-handoff",
            "Latest annotation handoff",
            "The latest user-sent batch for an MCP agent, including stable IDs, live bounds, and project context.",
            serde_json::json!({
                "schema_version": 1,
                "delivery": "mcp-resource-outbox",
                "handoff": latest,
                "annotations": annotations,
                "project": {
                    "root": self.project.borrow().root(),
                    "authoring_projection": format!("{:?}", self.backend.get()).to_lowercase(),
                    "runtime_model": "one-component-graph",
                    "html_revision": self.target().map(|target| target.revision()),
                    "canvas": self.canvas.get(),
                },
                "current_selection": self.selection.borrow().clone(),
                "instructions": "Address each unresolved annotation, preserve its stable target identity, and mark completed annotations done through the Studio UI or semantic actions.",
            }),
        )
    }

    fn save(&self) {
        let Some(target) = self.target() else {
            self.set_status("No active project session");
            return;
        };
        let document = target.document();
        let save_result = {
            let mut project = self.project.borrow_mut();
            project.save(&document.source)
        };
        match save_result {
            Ok(()) => {
                let root = self.project.borrow().root().to_owned();
                if let Err(error) = self.native_components.borrow().save(&root) {
                    self.set_status(format!(
                        "Project source saved, but component document export failed: {error}"
                    ));
                    return;
                }
                match persist_output_window_module(&root, self.canvas.get().decorations) {
                    Ok(path) => self.set_status(format!(
                        "Exported revision {} with runnable window policy at {}",
                        document.revision,
                        path.display()
                    )),
                    Err(error) => self.set_status(format!(
                        "Project source saved, but output window generation failed: {error}"
                    )),
                }
            }
            Err(error) => self.set_status(format!("Save rejected: {error}")),
        }
    }

    fn reload_external(&self) {
        let _ = self.reload_external_if_changed();
    }

    fn reload_external_if_changed(&self) -> bool {
        let Some(target) = self.target() else {
            self.set_status("No active project session");
            return true;
        };
        let disk = match self.project.borrow().read_disk() {
            Ok(disk) => disk,
            Err(error) => {
                self.set_status(format!("Read external edit: {error}"));
                return true;
            }
        };
        let active = target.document();
        let baseline = self.project.borrow().baseline().clone();
        if active.source != baseline {
            if disk != baseline {
                self.set_status(
                    "External edit conflicts with an unsaved preview; save, undo, or merge first",
                );
                return true;
            }
            return false;
        }
        if disk == active.source {
            self.project.borrow_mut().adopt_disk(disk);
            return false;
        }

        match target.preview_source(active.revision, disk.clone()) {
            Ok(preview) if preview.applied => {
                self.project.borrow_mut().adopt_disk(disk);
                if let Some(history) = self.history.borrow_mut().as_mut() {
                    history.observe(
                        preview.document.revision,
                        preview.document.source,
                        ChangeOrigin::File,
                    );
                }
                self.set_status(format!(
                    "External files hot reloaded at revision {}",
                    preview.document.revision
                ));
            }
            Ok(preview) => self.set_status(format!(
                "External edit rejected; keeping last-good preview: {}",
                diagnostic_summary(&preview.diagnostics)
            )),
            Err(error) => {
                self.set_status(format!("External reload conflict: {}", error.message));
            }
        }
        true
    }

    fn observe_runtime_revision(&self) -> bool {
        let Some(target) = self.target() else {
            return false;
        };
        let revision = target.revision();
        let mut history = self.history.borrow_mut();
        let Some(history) = history.as_mut() else {
            return false;
        };
        if history.observed_revision() == revision {
            return false;
        }
        let document = target.document();
        if history.observe(document.revision, document.source, ChangeOrigin::Mcp) {
            self.set_status(format!(
                "MCP preview applied in memory at revision {} · explicit save required",
                document.revision
            ));
            return true;
        }
        false
    }

    fn undo(&self) {
        self.move_history(true);
    }

    fn redo(&self) {
        self.move_history(false);
    }

    fn move_history(&self, undo: bool) {
        let Some(target) = self.target() else {
            self.set_status("No active project session");
            return;
        };
        let candidate = self.history.borrow().as_ref().and_then(|history| {
            if undo {
                history.undo_source()
            } else {
                history.redo_source()
            }
            .cloned()
        });
        let Some(candidate) = candidate else {
            self.set_status(if undo {
                "Nothing to undo"
            } else {
                "Nothing to redo"
            });
            return;
        };
        match target.preview_source(target.revision(), candidate) {
            Ok(preview) if preview.applied => {
                if let Some(history) = self.history.borrow_mut().as_mut() {
                    if undo {
                        history.commit_undo(preview.document.revision);
                    } else {
                        history.commit_redo(preview.document.revision);
                    }
                }
                self.set_status(format!(
                    "{} applied at revision {}",
                    if undo { "Undo" } else { "Redo" },
                    preview.document.revision
                ));
            }
            Ok(preview) => self.set_status(format!(
                "History candidate rejected: {}",
                diagnostic_summary(&preview.diagnostics)
            )),
            Err(error) => self.set_status(format!("History conflict: {}", error.message)),
        }
    }
}

fn build_app(config: &StudioConfig, window: &mut Window, cx: &mut App) -> Result<StudioApp> {
    let shell_paths = ProjectPaths::open(&config.studio_root).context("open Studio shell")?;
    let project = ProjectStore::open(&config.project_root).context("open canvas project")?;
    let mut native_components =
        ComponentLibrary::load(project.root()).context("open component document library")?;
    // Studio opens on the Main tab, which edits the project's root (first)
    // component, so make the active definition match that from the start.
    if let Some(root) = native_components.components.first() {
        native_components.active_component = root.id.clone();
    }
    let annotations = AnnotationStore::load(project.root()).context("open annotations")?;
    let annotation_handoffs =
        AnnotationHandoffStore::load(project.root()).context("open annotation handoff")?;
    let themes = ThemeCatalog::load(&config.theme_locations).context("load editor themes")?;
    let theme_watcher =
        ThemeWatcher::new(config.theme_locations.clone()).context("watch editor themes")?;

    let bridge = if config.mcp_enabled {
        Some(
            BridgeHandle::install(window, cx, BridgeConfig::new(AppId::new(APP_ID)?, TITLE))
                .context("install local MCP bridge")?,
        )
    } else {
        None
    };
    let automation = bridge
        .as_ref()
        .map_or_else(Automation::isolated, BridgeHandle::automation);
    let state = Rc::new(StudioState::new(StudioStateInit {
        project,
        workspace: config.workspace.clone(),
        studio_root: config.studio_root.clone(),
        native_components,
        annotations,
        annotation_handoffs,
        themes,
        automation: automation.clone(),
    }));
    let hooks = studio_hooks(&state)?;
    if let Some(bridge) = &bridge {
        let resource_state = state.clone();
        bridge
            .on_resource(move |request, _, _| resource_state.context_resource(request))
            .context("register Studio context resources")?;
        let command_state = state.clone();
        bridge
            .on_command(move |request, _, _| command_state.application_command(request))
            .context("register Studio application commands")?;
    }

    let project_source = state.project.borrow().baseline().clone();
    let project_components = project_components(&state)?;
    let project_session =
        LiveHtmlSession::compile(project_source, automation.clone(), hooks.clone())
            .context("compile project canvas")?
            .with_components(project_components)
            .embedded(SemanticNamespace::new("project-canvas")?);
    state.attach_target(project_session.clone());
    if let Some(bridge) = &bridge {
        project_session
            .serve_mcp(bridge)
            .context("register project MCP preview")?;
    }

    let components = studio_components(&state)?;
    let mut shell_source = ProjectSnapshot::load(&shell_paths)
        .context("read Studio shell")?
        .into_document();
    shell_source
        .css
        .push_str(&state.resolved_theme().css_overlay());
    let shell = LiveHtmlSession::compile(shell_source, automation, hooks)
        .context("compile Studio shell")?
        .with_components(components);
    let shell_watcher = ProjectWatcher::new(shell_paths).context("watch Studio shell")?;
    let project_watcher = ProjectWatcher::new(state.project.borrow().paths().clone())
        .context("watch canvas project")?;

    Ok(StudioApp {
        shell,
        shell_watcher,
        project_watcher,
        theme_watcher,
        applied_theme_revision: state.theme_revision(),
        project_reload_pending: false,
        shell_reload_pending: false,
        state,
        bridge,
    })
}

fn studio_components(state: &Rc<StudioState>) -> Result<ComponentRegistry> {
    let mut components = ComponentRegistry::new();
    components.register("studio-icon", move |node, _, _, _| {
        // Every named icon maps to its own `assets/icons/<name>.svg`; only a
        // genuinely unknown name falls back to the select cursor. Keep this list
        // in sync with `StudioAssets::load`.
        const ICONS: [&str; 23] = [
            "select",
            "pan",
            "play",
            "comment",
            "upload",
            "layers",
            "folder",
            "plus",
            "close",
            "component",
            "frame",
            "text",
            "row",
            "column",
            "button",
            "instance",
            "chevron",
            "rotate",
            "monitor",
            "grid",
            "duplicate",
            "trash",
            "settings",
        ];
        let name = node
            .attribute("name")
            .filter(|name| ICONS.contains(name))
            .unwrap_or("select");
        let file = format!("{name}.svg");
        let color = match node.attribute("tone") {
            Some("accent") => rgb(0x6e_7b_ff),
            Some("dim") => rgb(0x5f_66_75),
            _ => rgb(0xa9_af_bd),
        };
        let size = node
            .attribute("size")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| *value > 0.0 && *value <= 64.0)
            .unwrap_or(15.0);
        svg()
            .path(format!("icons/{file}"))
            .w(px(size))
            .h(px(size))
            .text_color(color)
            .into_any_element()
    })?;
    let canvas_state = state.clone();
    components.register("studio-canvas", move |_, _, window, cx| {
        canvas_state.render_canvas(window, cx)
    })?;
    let tabs_state = state.clone();
    components.register("studio-document-tabs", move |_, _, _, _| {
        render_document_tabs(&tabs_state)
    })?;
    let frame_state = state.clone();
    components.register("studio-app-frame", move |_, children, window, _| {
        frame_state.render_app_frame(children, window)
    })?;
    let dock_state = state.clone();
    components.register("studio-bottom-dock", move |_, children, window, _| {
        dock_state.render_bottom_dock(children, window)
    })?;
    components.register("studio-grid", move |_, _, _, _| {
        canvas(
            |_, _, _| {},
            |bounds, (), window, _| {
                for row in 0_u16..60 {
                    let y = bounds.origin.y + px(9.0 + f32::from(row) * 18.0);
                    if y >= bounds.bottom() {
                        break;
                    }
                    for column in 0_u16..100 {
                        let x = bounds.origin.x + px(9.0 + f32::from(column) * 18.0);
                        if x >= bounds.right() {
                            break;
                        }
                        window.paint_quad(fill(
                            Bounds::new(point(x, y), size(px(1.0), px(1.0))),
                            rgba(0x35_3a_46_80),
                        ));
                    }
                }
            },
        )
        .size_full()
        .into_any_element()
    })?;
    let floating_state = state.clone();
    components.register("studio-floating-surface", move |node, children, _, _| {
        let trigger_id = node.attribute("trigger").unwrap_or_default();
        let boundary_id = node.attribute("boundary").unwrap_or("studio-shell");
        let width = node
            .attribute("width")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(238.0)
            .clamp(80.0, 1_024.0);
        let height = node
            .attribute("height")
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(180.0)
            .clamp(40.0, 1_024.0);
        let preferred = match node.attribute("side") {
            Some("top") => crate::FloatingSide::Top,
            Some("right") => crate::FloatingSide::Right,
            Some("left") => crate::FloatingSide::Left,
            _ => crate::FloatingSide::Bottom,
        };
        let tree = floating_state.automation.snapshot();
        let boundary = tree
            .nodes
            .get(boundary_id)
            .and_then(|node| node.bounds)
            .unwrap_or(Rect {
                x: 0.0,
                y: 0.0,
                width: WINDOW_WIDTH,
                height: WINDOW_HEIGHT,
            });
        let anchor = tree
            .nodes
            .get(trigger_id)
            .and_then(|node| node.bounds)
            .unwrap_or(Rect {
                x: boundary.x + 12.0,
                y: boundary.y + 12.0,
                width: 1.0,
                height: 1.0,
            });
        let placement = crate::FloatingPlacement::resolve(
            crate::SurfaceRect {
                x: anchor.x,
                y: anchor.y,
                width: anchor.width,
                height: anchor.height,
            },
            (width, height),
            crate::SurfaceRect {
                x: boundary.x,
                y: boundary.y,
                width: boundary.width,
                height: boundary.height,
            },
            preferred,
            6.0,
            8.0,
        );
        children
            .into_iter()
            .fold(
                div()
                    .absolute()
                    .left(px(placement.rect.x - boundary.x))
                    .top(px(placement.rect.y - boundary.y))
                    .w(px(placement.rect.width))
                    .max_h(px(placement.available_height)),
                gpui::ParentElement::child,
            )
            .into_any_element()
    })?;
    let selection_state = state.clone();
    components.register("studio-selection-overlay", move |_, children, _, _| {
        selection_state.render_selection_overlay(children)
    })?;
    let annotation_state = state.clone();
    components.register("studio-annotation-layer", move |_, children, _, _| {
        annotation_state.render_annotation_layer(children)
    })?;
    let component_tree_state = state.clone();
    components.register("studio-component-tree", move |_, _, _, _| {
        render_component_tree(&component_tree_state)
    })?;
    let project_tree_state = state.clone();
    components.register("studio-project-tree", move |_, _, window, cx| {
        render_project_tree(&project_tree_state, window, cx)
    })?;
    let props_state = state.clone();
    components.register("studio-component-props", move |_, _, _, _| {
        render_component_props(&props_state)
    })?;
    let logic_state = state.clone();
    components.register("studio-component-logic", move |_, _, _, _| {
        render_component_logic(&logic_state)
    })?;
    let states_state = state.clone();
    components.register("studio-component-states", move |_, _, _, _| {
        render_component_states(&states_state)
    })?;
    let variants_state = state.clone();
    components.register("studio-component-variants", move |_, _, _, _| {
        render_component_variants(&variants_state)
    })?;
    let slots_state = state.clone();
    components.register("studio-component-slots", move |_, _, _, _| {
        render_component_slots(&slots_state)
    })?;
    let tokens_state = state.clone();
    components.register("studio-design-tokens", move |_, _, _, _| {
        render_design_tokens(&tokens_state)
    })?;
    let scroll_state = state.clone();
    components.register("studio-scroll-area", move |node, children, _, _| {
        render_studio_scroll_area(
            &scroll_state.scroll_surfaces,
            &scroll_state.automation,
            node,
            children,
        )
    })?;
    let palette_state = state.clone();
    components.register("studio-component-palette", move |_, _, _, _| {
        render_component_palette(&palette_state)
    })?;
    let resize_state = state.clone();
    components.register("studio-resize-handle", move |node, _, _, _| {
        render_resize_handle(&resize_state, node)
    })?;
    let console_log_state = state.clone();
    components.register("studio-console-log", move |_, _, _, _| {
        render_console_log(&console_log_state)
    })?;
    let state_panel_state = state.clone();
    components.register("studio-state-panel", move |_, _, _, _| {
        render_state_panel(&state_panel_state)
    })?;
    let context_menu_state = state.clone();
    components.register("studio-context-menu", move |_, _, _, _| {
        render_context_menu(&context_menu_state)
    })?;
    let settings_state = state.clone();
    components.register("studio-settings", move |_, _, _, _| {
        render_settings(&settings_state)
    })?;
    let drawer_state = state.clone();
    components.register("studio-drawer", move |_, _, _, _| {
        render_annotations_drawer(&drawer_state)
    })?;
    Ok(components)
}

/// Reusable modal: a dimmed full-window backdrop (click to dismiss) centering a
/// titled card. `body` is the modal content. Rendered above everything via
/// `deferred`. This is the studio's Modal component, shared by Settings and any
/// future confirmation/detail dialog.
fn render_modal(
    id: &str,
    title: &str,
    on_dismiss: impl Fn(&mut Window) + 'static,
    body: AnyElement,
) -> AnyElement {
    let dismiss = Rc::new(on_dismiss);
    let backdrop_dismiss = dismiss.clone();
    deferred(
        div()
            .id(SharedString::from(format!("{id}-backdrop")))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x05_07_0c_bb))
            .on_click(move |_, window, _| backdrop_dismiss(window))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-card")))
                    .flex()
                    .flex_col()
                    .w(px(360.0))
                    .rounded(px(12.0))
                    .bg(rgb(0x14_16_1d))
                    .border_1()
                    .border_color(rgb(0x2c_31_3d))
                    .shadow_lg()
                    // Clicks inside the card must not fall through to the
                    // backdrop's dismiss handler.
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(16.0))
                            .h(px(46.0))
                            .border_b_1()
                            .border_color(rgb(0x22_26_30))
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgb(0xe7_e9_ee))
                                    .child(title.to_owned()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("{id}-close")))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(6.0))
                                    .text_color(rgb(0x9aa2b6))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x22_26_30)))
                                    .child("×")
                                    .on_click(move |_, window, _| dismiss(window)),
                            ),
                    )
                    .child(body)
                    .semantic_role(SemanticRole::Dialog)
                    .accessible_name(title.to_owned()),
            ),
    )
    .into_any_element()
}

/// Settings / Preferences modal. Currently hosts the editor theme picker (via
/// the Dropdown component); future preferences slot in as further rows.
fn render_settings(state: &Rc<StudioState>) -> AnyElement {
    if !state.settings_visible() {
        return div().into_any_element();
    }
    let dismiss_state = state.clone();
    let body = div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(16.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(rgb(0xd6_da_e6))
                                .child("Editor theme"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(0x67_6d_7a))
                                .child("Applies to the Studio chrome"),
                        ),
                )
                .child(render_theme_dropdown(state)),
        )
        .into_any_element();
    render_modal(
        "settings",
        "Settings",
        move |_| dismiss_state.close_settings(),
        body,
    )
}

/// The theme picker Dropdown: a trigger showing the active theme and, when
/// open, an anchored list of every available variant. This is the studio's
/// reusable Dropdown component pattern (trigger + anchored option list).
fn render_theme_dropdown(state: &Rc<StudioState>) -> AnyElement {
    let options = state.theme_options();
    let active_label = options.iter().find(|(_, selected)| *selected).map_or_else(
        || "Default".to_owned(),
        |(theme, _)| theme_variant_label(theme),
    );
    let open = state.theme_dropdown_open.get();
    let toggle_state = state.clone();
    let mut trigger = div()
        .id("theme-dropdown-trigger")
        .relative()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .w(px(168.0))
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .bg(rgb(0x1a_1d_26))
        .border_1()
        .border_color(if open {
            rgb(0x55_60_a0)
        } else {
            rgb(0x2a_2f_3d)
        })
        .text_size(px(12.0))
        .text_color(rgb(0xd6_da_e6))
        .cursor_pointer()
        .hover(|style| style.border_color(rgb(0x3a_41_57)))
        .child(active_label)
        .child(
            div()
                .text_size(px(9.0))
                .text_color(rgb(0x8a_90a2))
                .child(if open { "▲" } else { "▼" }),
        )
        .on_click(move |_, window, _| {
            toggle_state.toggle_theme_dropdown();
            window.refresh();
        });

    if open {
        let mut list = div()
            .absolute()
            .top(px(34.0))
            .left(px(0.0))
            .w(px(168.0))
            .max_h(px(240.0))
            .flex()
            .flex_col()
            .py(px(4.0))
            .rounded(px(8.0))
            .bg(rgb(0x14_16_1d))
            .border_1()
            .border_color(rgb(0x2c_31_3d))
            .shadow_lg()
            .overflow_hidden();
        for (theme, selected) in options {
            let name = theme.name.clone();
            let mode = theme.mode;
            let pick_state = state.clone();
            let label = theme_variant_label(&theme);
            list = list.child(
                div()
                    .id(SharedString::from(format!(
                        "theme-option-{}-{mode:?}",
                        name
                    )))
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(28.0))
                    .px(px(10.0))
                    .text_size(px(12.0))
                    .text_color(if selected {
                        rgb(0xe7_e9_ee)
                    } else {
                        rgb(0xb3_bb_d0)
                    })
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0x22_26_30)))
                    .child(label.clone())
                    .when(selected, |row| {
                        row.child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(0x6e_7b_ff))
                                .child("✓"),
                        )
                    })
                    .on_click(move |_, window, _| {
                        pick_state.select_theme(&name, mode);
                        window.refresh();
                    })
                    .semantic_role(SemanticRole::Option)
                    .accessible_name(label)
                    .semantic_selected(selected),
            );
        }
        trigger = trigger.child(list);
    }
    trigger
        .semantic_role(SemanticRole::Combobox)
        .accessible_name(format!("Theme: {}", state.theme_label()))
        .semantic_expanded(open)
        .into_any_element()
}

fn theme_variant_label(theme: &crate::AvailableTheme) -> String {
    let mode = match theme.mode {
        crate::ThemeMode::Light => "Light",
        crate::ThemeMode::Dark => "Dark",
    };
    format!("{} · {mode}", theme.name)
}

/// Reusable right-side Drawer — the studio's Drawer component. A dimmed
/// full-window backdrop (click to dismiss) with a full-height panel sliding in
/// from the right. `body` is the drawer content (already scrollable if needed).
/// Rendered above everything via `deferred`, mounted over a full-window host.
fn render_drawer(
    id: &str,
    title: &str,
    on_dismiss: impl Fn(&mut Window) + 'static,
    body: AnyElement,
) -> AnyElement {
    let dismiss = Rc::new(on_dismiss);
    let backdrop_dismiss = dismiss.clone();
    deferred(
        div()
            .id(SharedString::from(format!("{id}-backdrop")))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .justify_end()
            .bg(rgba(0x05_07_0c_88))
            .on_click(move |_, window, _| backdrop_dismiss(window))
            .child(
                div()
                    .id(SharedString::from(format!("{id}-panel")))
                    .h_full()
                    .w(px(320.0))
                    .flex()
                    .flex_col()
                    .bg(rgb(0x12_14_1b))
                    .border_l_1()
                    .border_color(rgb(0x24_2832))
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(16.0))
                            .h(px(48.0))
                            .flex_none()
                            .border_b_1()
                            .border_color(rgb(0x22_26_30))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(gpui::FontWeight(600.0))
                                    .text_color(rgb(0xe7_e9_ee))
                                    .child(title.to_owned()),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("{id}-close")))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(6.0))
                                    .text_color(rgb(0x9aa2b6))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0x22_26_30)))
                                    .child("×")
                                    .on_click(move |_, window, _| dismiss(window)),
                            ),
                    )
                    .child(body)
                    .semantic_role(SemanticRole::Dialog)
                    .accessible_name(title.to_owned()),
            ),
    )
    .into_any_element()
}

/// Annotations / handoff-queue drawer: the reviewer-facing list of every
/// annotation with its status, and a Send action to hand the queue to the
/// agent. Uses the Drawer + Scrollable components.
fn render_annotations_drawer(state: &Rc<StudioState>) -> AnyElement {
    if !state.annotations_drawer_visible() {
        return div().into_any_element();
    }
    let items: Vec<(String, String, AnnotationStatus)> = state
        .annotations
        .borrow()
        .annotations
        .iter()
        .filter(|annotation| annotation.status != AnnotationStatus::Archived)
        .map(|annotation| {
            (
                annotation.target.authored_id.clone(),
                annotation.comment.clone(),
                annotation.status,
            )
        })
        .collect();

    let mut list = div().flex().flex_col().gap(px(8.0)).p(px(14.0)).w_full();
    if items.is_empty() {
        list = list.child(
            div()
                .text_size(px(12.0))
                .text_color(rgb(0x73_7b_8c))
                .child("No annotations yet — switch to the Annotate tool and click an element."),
        );
    }
    for (target, comment, status) in items {
        let (badge_label, badge_bg, badge_fg) = match status {
            AnnotationStatus::Open => ("Open", rgb(0x2b_31_60), rgb(0xc9_ce_ff)),
            AnnotationStatus::InProgress => ("In progress", rgb(0x5a_45_12), rgb(0xf3_d8_a3)),
            AnnotationStatus::Done => ("Done", rgb(0x1d_3a_28), rgb(0x9d_e8_b8)),
            AnnotationStatus::Archived => ("Archived", rgb(0x24_2832), rgb(0x8a_90a2)),
        };
        list = list.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgb(0x0f_1116))
                .border_1()
                .border_color(rgb(0x24_2832))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.0))
                                .text_color(rgb(0x8a_93_ff))
                                .child(format!("#{target}")),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(10.0))
                                .bg(badge_bg)
                                .text_size(px(10.0))
                                .text_color(badge_fg)
                                .child(badge_label),
                        ),
                )
                .child(div().text_size(px(12.0)).text_color(rgb(0xc4_c9_d4)).child(
                    if comment.trim().is_empty() {
                        "(empty)".to_owned()
                    } else {
                        comment
                    },
                )),
        );
    }

    let scroll = scroll_area(
        "annotations-drawer-scroll",
        "Annotations drawer",
        &state.drawer_scroll,
        list.into_any_element(),
    );

    let send_state = state.clone();
    let sendable = state.annotations.borrow().has_sendable();
    let footer = div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .px(px(14.0))
        .h(px(56.0))
        .border_t_1()
        .border_color(rgb(0x22_26_30))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(rgb(0x73_7b_8c))
                .child(state.send_annotations_label()),
        )
        .child(
            div()
                .id("annotations-drawer-send")
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(12.0))
                .h(px(30.0))
                .rounded(px(8.0))
                .bg(if sendable {
                    rgb(0x2c_32_63)
                } else {
                    rgb(0x1a_1d_26)
                })
                .border_1()
                .border_color(if sendable {
                    rgb(0x4e_58_a0)
                } else {
                    rgb(0x2a_2f_3d)
                })
                .text_size(px(12.0))
                .text_color(if sendable {
                    rgb(0xdf_e2_ff)
                } else {
                    rgb(0x5f_66_75)
                })
                .cursor_pointer()
                .child("Send to agent")
                .on_click(move |_, window, _| {
                    send_state.send_annotations();
                    window.refresh();
                })
                .semantic_role(SemanticRole::Button)
                .accessible_name("Send annotations to agent")
                .semantic_enabled(sendable),
        );

    let body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .child(scroll)
        .child(footer)
        .into_any_element();

    let dismiss_state = state.clone();
    render_drawer(
        "annotations-drawer",
        "Annotations",
        move |_| dismiss_state.close_annotations_drawer(),
        body,
    )
}

/// Right-click menu rendered above everything at the pointer position, with
/// window-edge flipping from GPUI's anchored container.
fn render_context_menu(state: &Rc<StudioState>) -> AnyElement {
    let Some(menu) = state.context_menu.borrow().clone() else {
        return div().into_any_element();
    };
    let actions = state.context_menu_actions(&menu.target);
    let mut surface = div()
        .flex()
        .flex_col()
        .w(px(188.0))
        .py(px(5.0))
        .rounded(px(9.0))
        .bg(rgb(0x14_16_1d))
        .border_1()
        .border_color(rgb(0x2c_31_3d))
        .shadow_lg();
    for (index, action) in actions.iter().copied().enumerate() {
        if index > 0 && action.leads_group() {
            surface = surface.child(div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgb(0x24_29_34)));
        }
        let danger = matches!(
            action,
            ContextMenuAction::Delete | ContextMenuAction::DeleteComponent
        );
        let click_state = state.clone();
        let row = div()
            .id(SharedString::from(format!(
                "context-menu-{}",
                action.slug()
            )))
            .flex()
            .h(px(28.0))
            .mx(px(5.0))
            .px(px(9.0))
            .items_center()
            .rounded(px(6.0))
            .text_size(px(12.5))
            .text_color(rgb(if danger { 0xe8_8a_97 } else { 0xc4_c9_d4 }))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(if danger { 0x3a_20_26 } else { 0x22_26_41 })))
            .child(action.label())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                click_state.run_context_menu_action(action);
                window.refresh();
            })
            .semantic_role(SemanticRole::MenuItem)
            .accessible_name(action.label());
        surface = surface.child(row);
    }
    let dismiss_state = state.clone();
    let menu_root = div()
        .occlude()
        .on_mouse_down_out(move |_, window, _| {
            dismiss_state.close_context_menu();
            window.refresh();
        })
        .child(surface)
        .id("studio-context-menu")
        .semantic_role(SemanticRole::Menu)
        .accessible_name("Context menu");
    deferred(
        anchored()
            .position(point(px(menu.x), px(menu.y)))
            .child(menu_root),
    )
    .with_priority(64)
    .into_any_element()
}

/// A draggable strip between panels that resolves all behavior from the target's
/// [`ResizableSpec`](crate::resizable::ResizableSpec) declaration.
fn render_resize_handle(state: &Rc<StudioState>, node: &ComponentNode) -> AnyElement {
    let Some(spec) = node
        .attribute("target")
        .and_then(|target| state.resizable.spec(target))
    else {
        return div().into_any_element();
    };
    let target = spec.target;
    let handle = div().id(SharedString::from(node.id().to_owned()));
    let handle = match spec.axis {
        ResizeAxis::Width => handle
            .w(px(6.0))
            .h_full()
            .flex_none()
            .cursor(gpui::CursorStyle::ResizeLeftRight),
        ResizeAxis::Height => handle
            .w_full()
            .h(px(6.0))
            .flex_none()
            .cursor(gpui::CursorStyle::ResizeUpDown),
    };
    let payload = spec.drag();
    let drag_state = state.clone();
    let click_state = state.clone();
    let keyboard_state = state.clone();
    handle
        .hover(|style| style.bg(rgba(0x6e_7b_ff_55)))
        .on_click(move |event, window, _| {
            if matches!(event, gpui::ClickEvent::Mouse(mouse) if mouse.up.click_count >= 2) {
                click_state.reset_panel(target);
                window.refresh();
            }
        })
        .on_drag(payload, |_, _, _, cx| cx.new(|_| EmptyDragGhost))
        .on_drag_move::<ResizeDrag>(move |event, window, cx| {
            // GPUI invokes every ResizeDrag listener. Always use the active
            // payload's target discriminator, never this listener's target.
            let drag = event.drag(cx).clone();
            let position = event.event.position;
            drag_state.resize_panel(&drag, f32::from(position.x), f32::from(position.y));
            window.refresh();
        })
        .focusable()
        .on_key_down(move |event, window, cx| {
            let current = keyboard_state.panel_size(target);
            let next = match event.keystroke.key.as_str() {
                "home" => spec.min,
                "end" => spec.max,
                "left" | "down" => current - 1.0,
                "right" | "up" => current + 1.0,
                _ => return,
            };
            if keyboard_state.set_panel_size(target, next) {
                window.refresh();
            }
            cx.stop_propagation();
        })
        .semantic_role(SemanticRole::Slider)
        .accessible_name(format!("Resize {target} panel · double-click to reset"))
        .accessible_description(format!(
            "{:?} from the {:?} edge; range {:.0} to {:.0} pixels; default {:.0} pixels",
            spec.axis, spec.edge, spec.min, spec.max, spec.default
        ))
        .semantic_value(SemanticValue {
            value: format!("{:.0}", state.panel_size(target)),
            min: Some(f64::from(spec.min)),
            max: Some(f64::from(spec.max)),
            step: Some(1.0),
            editable: true,
        })
        .into_any_element()
}

fn render_component_palette(state: &Rc<StudioState>) -> AnyElement {
    let active = state.active_component_id();
    let entries: Vec<(String, String, bool)> = {
        let library = state.native_components.borrow();
        library
            .components
            .iter()
            .map(|component| {
                let cycles = active
                    .as_deref()
                    .is_some_and(|host| library.would_cycle(host, &component.id));
                (component.id.clone(), component.name.clone(), cycles)
            })
            .collect()
    };
    let mut container = div().flex().w_full().flex_col().gap(px(2.0));
    if entries.is_empty() {
        container = container.child(
            div()
                .px(px(10.0))
                .py(px(8.0))
                .text_size(px(11.5))
                .text_color(rgb(0x73_7b_8c))
                .child("No components yet — create one to reuse it."),
        );
    }
    for (id, name, cycles) in entries {
        let is_self = active.as_deref() == Some(id.as_str());
        let disabled = cycles;
        let mut row = div()
            .flex()
            .w_full()
            .h(px(28.0))
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(12.5))
            .text_color(rgb(if disabled { 0x5c_63_72 } else { 0xc4_c9_d4 }))
            .child(
                svg()
                    .path("icons/component.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(if disabled { 0x4c_53_61 } else { 0x8a_93_ff })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name.clone()),
            );
        if is_self {
            row = row.child(
                div()
                    .text_size(px(10.0))
                    .text_color(rgb(0x73_7b_8c))
                    .child("self"),
            );
        }
        if disabled {
            container = container.child(
                row.id(SharedString::from(format!("component-palette/{id}")))
                    .semantic_role(SemanticRole::Button)
                    .accessible_name(format!("{name} (unavailable: would cycle)"))
                    .semantic_enabled(false)
                    .into_any_element(),
            );
            continue;
        }
        let click_state = state.clone();
        let click_id = id.clone();
        let context_state = state.clone();
        let context_id = id.clone();
        let row = row
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0x22_26_41)))
            .id(SharedString::from(format!("component-palette/{id}")))
            .on_click(move |_, window, _| {
                click_state.open_component_document(&click_id);
                window.refresh();
            })
            .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                *context_state.context_menu.borrow_mut() = Some(ContextMenuState {
                    x: f32::from(event.position.x),
                    y: f32::from(event.position.y),
                    target: ContextMenuTarget::PaletteComponent {
                        component_id: context_id.clone(),
                    },
                });
                window.refresh();
            })
            .on_drag(
                PaletteDrag {
                    component_id: id.clone(),
                    name: name.clone(),
                },
                |drag, _, _, cx| {
                    let label = SharedString::from(drag.name.clone());
                    cx.new(|_| DragGhost { label })
                },
            );
        container = container.child(
            row.semantic_role(SemanticRole::Button)
                .accessible_name(format!("Open {name}"))
                .semantic_metadata("component_id", id)
                .semantic_metadata("drag_to_place", "true")
                .into_any_element(),
        );
    }
    scroll_area(
        "component-palette-scroll",
        "Component library palette",
        &state.palette_scroll,
        container.into_any_element(),
    )
    .into_any_element()
}

fn render_component_props(state: &StudioState) -> AnyElement {
    let props = state
        .native_components
        .borrow()
        .active()
        .map(|component| component.props.clone())
        .unwrap_or_default();
    if props.is_empty() {
        return render_inspector_data_row("No public props", "Add one below");
    }
    props
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, prop| {
                let value = prop.default.as_deref().unwrap_or("required");
                rows.child(render_inspector_data_row(
                    &prop.name,
                    &format!("{} · {value}", prop.value_type),
                ))
            },
        )
        .into_any_element()
}

fn render_component_logic(state: &StudioState) -> AnyElement {
    fn collect(node: &NativeNode, actions: &mut Vec<(String, String)>) {
        if let Some(action) = &node.action {
            actions.push((node.id.clone(), action.clone()));
        }
        for child in &node.children {
            collect(child, actions);
        }
    }

    let library = state.native_components.borrow();
    let Some(component) = library.active() else {
        return render_inspector_data_row("No interactions", "Select a component");
    };
    let mut actions = Vec::new();
    collect(&component.root, &mut actions);
    actions.extend(component.logic.iter().map(|logic| {
        let result = logic.target_state.as_ref().map_or_else(
            || logic.action.clone(),
            |target| format!("{} → {target}", logic.action),
        );
        (logic.source_node.clone(), result)
    }));
    if actions.is_empty() {
        return render_inspector_data_row("No interactions", "Add one below");
    }
    let logic_nodes = actions
        .iter()
        .enumerate()
        .flat_map(|(index, (source, _))| {
            [
                crate::LogicNode {
                    id: format!("source:{source}"),
                    width: 112.0,
                    height: 36.0,
                },
                crate::LogicNode {
                    id: format!("action:{index}"),
                    width: 148.0,
                    height: 36.0,
                },
            ]
        })
        .collect::<Vec<_>>();
    let logic_edges = actions
        .iter()
        .enumerate()
        .map(|(index, (source, _))| crate::LogicEdge {
            id: format!("edge:{index}"),
            from: format!("source:{source}"),
            to: format!("action:{index}"),
        })
        .collect::<Vec<_>>();
    let placements = crate::layout_logic_graph(
        &logic_nodes,
        &logic_edges,
        crate::LogicLayoutOptions::default(),
    )
    .into_iter()
    .filter_map(|placement| {
        placement
            .id
            .strip_prefix("action:")
            .and_then(|index| index.parse::<usize>().ok())
            .map(|index| (placement.rank, placement.order, index))
    })
    .collect::<Vec<_>>();
    placements
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, (rank, _, index)| {
                let (node, action) = &actions[index];
                rows.child(render_inspector_data_row(
                    &format!("L{} · {node} · click", rank + 1),
                    action,
                ))
            },
        )
        .into_any_element()
}

fn render_component_states(state: &StudioState) -> AnyElement {
    let states = state
        .native_components
        .borrow()
        .active()
        .map(|component| component.states.clone())
        .unwrap_or_default();
    if states.is_empty() {
        return render_inspector_data_row("No local state", "Add one below");
    }
    states
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, item| {
                rows.child(render_inspector_data_row(
                    &item.name,
                    &format!("{} · {}", item.value_type, item.default),
                ))
            },
        )
        .into_any_element()
}

fn render_component_variants(state: &StudioState) -> AnyElement {
    let variants = state
        .native_components
        .borrow()
        .active()
        .map(|component| component.variants.clone())
        .unwrap_or_default();
    if variants.is_empty() {
        return render_inspector_data_row("No variants", "Base graph only");
    }
    variants
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, item| {
                rows.child(render_inspector_data_row(
                    &item.name,
                    &format!("{} · {} overrides", item.id, item.overrides.len()),
                ))
            },
        )
        .into_any_element()
}

fn render_component_slots(state: &StudioState) -> AnyElement {
    let slots = state
        .native_components
        .borrow()
        .active()
        .map(|component| component.slots.clone())
        .unwrap_or_default();
    if slots.is_empty() {
        return render_inspector_data_row("No slots", "Closed composition");
    }
    slots
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, item| {
                rows.child(render_inspector_data_row(
                    &item.name,
                    &format!(
                        "{} · {}",
                        item.node_id,
                        if item.multiple { "many" } else { "single" }
                    ),
                ))
            },
        )
        .into_any_element()
}

fn render_design_tokens(state: &StudioState) -> AnyElement {
    let tokens = state.native_components.borrow().tokens.clone();
    if tokens.is_empty() {
        return render_inspector_data_row("No tokens", "Add one below");
    }
    tokens
        .into_iter()
        .fold(
            div().flex().w_full().flex_col().gap(px(6.0)),
            |rows, item| {
                rows.child(render_inspector_data_row(
                    &item.path,
                    &format!("{:?} · {}", item.kind, item.value),
                ))
            },
        )
        .into_any_element()
}

fn render_inspector_data_row(label: &str, value: &str) -> AnyElement {
    div()
        .flex()
        .w_full()
        .min_w_0()
        .h(px(36.0))
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .bg(rgb(0x0f_1116_u32))
        .border_1()
        .border_color(rgb(0x2a_30_3b))
        .rounded(px(8.0))
        .font_family("Geist Mono")
        .text_size(px(11.0))
        .line_height(px(16.0))
        .text_color(rgb(0x8b_93_a2))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.to_owned()),
        )
        .child(
            div()
                .ml_auto()
                .min_w_0()
                .max_w(px(148.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(rgb(0xc7_cc_d6))
                .child(value.to_owned()),
        )
        .into_any_element()
}

/// Section heading matching the static `.inspector-kicker` styling used by
/// the surrounding HTML shell.
fn inspector_kicker(label: &str) -> AnyElement {
    div()
        .font_family("Geist Mono")
        .text_size(px(10.0))
        .line_height(px(15.0))
        .text_color(rgb(0x74_7c_8c))
        .child(label.to_owned())
        .into_any_element()
}

/// Registered native scroll viewport used by the authored shell surfaces. The
/// `surface` attribute selects one persistent handle; invalid values deliberately
/// fall back to the authored children so a typo cannot make content disappear.
fn render_studio_scroll_area(
    handles: &StudioScrollHandles,
    automation: &Automation,
    node: &ComponentNode,
    children: Vec<AnyElement>,
) -> AnyElement {
    let surface = node.attribute("surface").unwrap_or_default();
    let Some(surface) = StudioScrollSurface::parse(surface) else {
        let diagnostic = if surface.is_empty() {
            "<missing>"
        } else {
            surface
        };
        automation.log(
            "debug",
            &format!(
                "studio-scroll-area invalid surface \"{diagnostic}\"; preserving authored children"
            ),
        );
        return children
            .into_iter()
            .fold(div(), gpui::ParentElement::child)
            .into_any_element();
    };
    let content = children
        .into_iter()
        .fold(div().flex().w_full().flex_col(), gpui::ParentElement::child)
        .into_any_element();
    scroll_area(
        surface.semantic_id(),
        surface.label(),
        handles.handle(surface),
        content,
    )
    .into_any_element()
}

/// Reusable vertical scroll area — the studio's Scrollable component. Wraps
/// `content` in a clipped viewport that tracks `handle`, publishes one semantic
/// scroll node, and overlays a thin scrollbar only when the content overflows.
/// Returns a fillable `Div` so callers can add overlays such as the console's
/// jump-to-latest button. The host must provide a definite height.
fn scroll_area(
    id: &str,
    label: &str,
    handle: &gpui::ScrollHandle,
    content: AnyElement,
) -> gpui::Div {
    let max_h = f32::from(handle.max_offset().height);
    let offset_from_top = -f32::from(handle.offset().y);
    let viewport_h = f32::from(handle.bounds().size.height);
    let viewport = div()
        .id(SharedString::from(id.to_owned()))
        .size_full()
        .overflow_y_scroll()
        .track_scroll(handle)
        .child(content)
        .semantic_role(SemanticRole::ScrollArea)
        .accessible_name(label.to_owned())
        .semantic_metadata("scroll_offset_y", format!("{offset_from_top:.1}"))
        .semantic_metadata("scroll_max_y", format!("{max_h:.1}"));
    let mut area = div().relative().flex_1().min_h_0().child(viewport);
    if max_h > 1.0 && viewport_h > 1.0 {
        let content_h = viewport_h + max_h;
        let thumb_h = (viewport_h * viewport_h / content_h).clamp(28.0, viewport_h);
        let frac = (offset_from_top / max_h).clamp(0.0, 1.0);
        let thumb_top = frac * (viewport_h - thumb_h);
        area = area.child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(px(10.0))
                .child(
                    div()
                        .absolute()
                        .right(px(2.0))
                        .top(px(thumb_top))
                        .w(px(5.0))
                        .h(px(thumb_h))
                        .rounded(px(3.0))
                        .bg(rgba(0x6e_7b_ff_66)),
                ),
        );
    }
    area
}

fn scroll_console_to_latest(state: &StudioState, window: &mut Window) {
    state.console_pinned.set(true);
    let max = f32::from(state.console_scroll.max_offset().height);
    state
        .console_scroll
        .set_offset(gpui::point(px(0.0), px(-max)));
    window.refresh();
}

/// Live tracing console: the most recent automation log entries, oldest to
/// newest, each with a clock, level chip, and message.
fn render_console_log(state: &Rc<StudioState>) -> AnyElement {
    let entries = state.automation.logs(400, None);
    let mut container = div().flex().w_full().flex_col().gap(px(2.0));
    if entries.is_empty() {
        container = container.child(
            div()
                .flex()
                .items_center()
                .h(px(24.0))
                .px(px(8.0))
                .text_size(px(11.0))
                .text_color(rgb(0x5f_66_75))
                .child("No events yet — interact with the canvas"),
        );
    }
    for entry in &entries {
        let secs = (entry.timestamp_ms / 1000) % 86_400;
        let hours = secs / 3_600;
        let minutes = (secs % 3_600) / 60;
        let seconds = secs % 60;
        let level_color = match entry.level.as_str() {
            "error" => rgb(0xe8_62_6f),
            "warn" => rgb(0xf3_a3_4d),
            "debug" => rgb(0x67_6d_7a),
            _ => rgb(0x6e_7b_ff),
        };
        container = container.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .h(px(20.0))
                .flex_none()
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(0x67_6d_7a))
                        .child(format!("{hours:02}:{minutes:02}:{seconds:02}")),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(44.0))
                        .text_color(level_color)
                        .child(entry.level.to_uppercase()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(rgb(0xc4_c9_d4))
                        .child(entry.message.clone()),
                ),
        );
    }
    // Auto-scroll to the latest entry (bottom) while pinned. The pin releases
    // when the user scrolls up and is restored when they reach the bottom again
    // or press the scroll-to-bottom control.
    let handle = &state.console_scroll;
    let max_h = f32::from(handle.max_offset().height);
    let offset_from_top = -f32::from(handle.offset().y);
    let at_bottom = max_h <= 1.0 || offset_from_top >= max_h - 6.0;
    if at_bottom {
        state.console_pinned.set(true);
    }
    if state.console_pinned.get() {
        handle.set_offset(gpui::point(px(0.0), px(-max_h)));
    }
    let pinned = state.console_pinned.get();

    // Compose the reusable Scrollable component, then layer the console-specific
    // pin/unpin wheel handler and jump-to-latest control on top.
    let wheel_state = state.clone();
    let mut area = scroll_area(
        "console-scroll",
        "Live event trace",
        handle,
        container.into_any_element(),
    )
    .on_scroll_wheel(move |event, _, _| {
        // Native wheel deltas use GPUI's offset direction; a positive delta
        // moves toward older entries and releases the live pin.
        let delta_y = match event.delta {
            gpui::ScrollDelta::Pixels(point) => f32::from(point.y),
            gpui::ScrollDelta::Lines(point) => point.y,
        };
        if delta_y > 0.0 {
            wheel_state.console_pinned.set(false);
        }
    });

    // Scroll-to-bottom control, shown only while the pin is released.
    if !pinned {
        let jump_state = state.clone();
        area = area.child(
            div()
                .id("console-jump-latest")
                .absolute()
                .right(px(14.0))
                .bottom(px(10.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(9.0))
                .h(px(24.0))
                .rounded(px(12.0))
                .bg(rgb(0x2b_31_60))
                .border_1()
                .border_color(rgb(0x55_60_a0))
                .text_size(px(11.0))
                .text_color(rgb(0xe7_e9_ee))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(0x35_3c_72)))
                .child("↓ Latest")
                .on_click(move |_, window, _| {
                    scroll_console_to_latest(&jump_state, window);
                })
                .semantic_role(SemanticRole::Button)
                .accessible_name("Scroll to latest"),
        );
    }

    area.into_any_element()
}

/// One props/state/variants/logic row group, or a muted placeholder row when
/// the section has nothing to show.
fn render_state_panel_section(heading: &str, rows: Vec<AnyElement>) -> AnyElement {
    let mut section = div().flex().w_full().flex_col().gap(px(6.0));
    if rows.is_empty() {
        section = section.child(render_inspector_data_row("—", ""));
    } else {
        for row in rows {
            section = section.child(row);
        }
    }
    div()
        .flex()
        .w_full()
        .flex_col()
        .gap(px(8.0))
        .child(inspector_kicker(heading))
        .child(section)
        .into_any_element()
}

/// A boolean local-state value rendered as a clickable pill toggle, live both
/// on the canvas dock and to MCP automation.
fn render_state_toggle_row(
    state: &Rc<StudioState>,
    component_id: &str,
    name: &str,
    label: &str,
    value: bool,
) -> AnyElement {
    let click_state = state.clone();
    let click_component = component_id.to_owned();
    let click_name = name.to_owned();
    let pill = div()
        .id(SharedString::from(format!(
            "state-toggle-{component_id}-{name}"
        )))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(48.0))
        .h(px(20.0))
        .rounded(px(10.0))
        .cursor_pointer()
        .text_size(px(10.0))
        .font_family("Geist Mono")
        .bg(rgb(if value { 0x1f_3d_2b } else { 0x2a_30_3b }))
        .text_color(rgb(if value { 0x7d_e0_a3 } else { 0x8b_93_a2 }))
        .child(if value { "true" } else { "false" })
        .on_click(move |_, window, _| {
            let mut runtime = click_state.component_runtime_state.borrow_mut();
            runtime
                .entry(click_component.clone())
                .or_default()
                .insert(click_name.clone(), (!value).to_string());
            drop(runtime);
            window.refresh();
        });
    div()
        .flex()
        .w_full()
        .min_w_0()
        .h(px(36.0))
        .items_center()
        .gap(px(10.0))
        .px(px(10.0))
        .bg(rgb(0x0f_1116_u32))
        .border_1()
        .border_color(rgb(0x2a_30_3b))
        .rounded(px(8.0))
        .font_family("Geist Mono")
        .text_size(px(11.0))
        .line_height(px(16.0))
        .text_color(rgb(0x8b_93_a2))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.to_owned()),
        )
        .child(
            pill.semantic_role(SemanticRole::Switch)
                .accessible_name(label.to_owned())
                .semantic_checked(value),
        )
        .into_any_element()
}

/// Live state panel for the active component: props, local state (booleans
/// toggle live), variants, and logic edges.
fn render_state_panel(state: &Rc<StudioState>) -> AnyElement {
    let active = {
        let library = state.native_components.borrow();
        library.active().map(|component| {
            (
                component.id.clone(),
                component.props.clone(),
                component.states.clone(),
                component.variants.clone(),
                component.logic.clone(),
            )
        })
    };
    let Some((component_id, props, states, variants, logic)) = active else {
        return div()
            .flex()
            .items_center()
            .h(px(24.0))
            .px(px(4.0))
            .text_size(px(11.0))
            .text_color(rgb(0x5f_66_75))
            .child("No component selected")
            .into_any_element();
    };
    let runtime = state
        .component_runtime_state
        .borrow()
        .get(&component_id)
        .cloned()
        .unwrap_or_default();

    let prop_rows = props
        .iter()
        .map(|prop| {
            let value = prop.default.as_deref().unwrap_or("required");
            render_inspector_data_row(&format!("{}: {}", prop.name, prop.value_type), value)
        })
        .collect();

    let state_rows = states
        .iter()
        .map(|item| {
            let label = format!("{}: {}", item.name, item.value_type);
            let current = runtime
                .get(&item.name)
                .cloned()
                .unwrap_or_else(|| item.default.clone());
            if current == "true" || current == "false" {
                render_state_toggle_row(state, &component_id, &item.name, &label, current == "true")
            } else {
                render_inspector_data_row(&label, &current)
            }
        })
        .collect();

    let variant_rows = variants
        .iter()
        .map(|variant| {
            render_inspector_data_row(
                &format!("{} / {}", variant.id, variant.name),
                &format!("{} overrides", variant.overrides.len()),
            )
        })
        .collect();

    let logic_rows = logic
        .iter()
        .map(|edge| {
            let value = edge.target_state.as_ref().map_or_else(
                || edge.action.clone(),
                |target| format!("{} → {target}", edge.action),
            );
            render_inspector_data_row(&edge.source_node, &value)
        })
        .collect();

    div()
        .flex()
        .w_full()
        .flex_col()
        .gap(px(16.0))
        .child(render_state_panel_section("PROPS", prop_rows))
        .child(render_state_panel_section("STATE", state_rows))
        .child(render_state_panel_section("VARIANTS", variant_rows))
        .child(render_state_panel_section("LOGIC", logic_rows))
        .into_any_element()
}

fn render_native_decorator(height: f32) -> AnyElement {
    let scale = (height / 32.0).clamp(0.5, 1.0);
    let platform = if cfg!(target_os = "windows") {
        "Windows 11"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    };
    let mut decorator = div()
        .relative()
        .flex()
        .w_full()
        .h(px(height))
        .flex_none()
        .items_center()
        .border_b_1()
        .border_color(rgb(0x32_35_3c))
        .bg(rgb(0x20_20_22))
        .text_color(rgb(0xdc_dc_e0))
        .text_size(px(11.0 * scale));

    if cfg!(target_os = "macos") {
        decorator = decorator
            .gap(px(7.0 * scale))
            .px(px(11.0 * scale))
            .child(native_window_dot(0xff_5f_57, scale))
            .child(native_window_dot(0xfe_bc_2e, scale))
            .child(native_window_dot(0x28_c8_40, scale))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child("GPUI Preview"),
            );
    } else {
        decorator = decorator
            .pl(px(10.0 * scale))
            .child(
                div()
                    .w(px(14.0 * scale))
                    .h(px(14.0 * scale))
                    .rounded(px(3.0 * scale))
                    .bg(rgb(0x6e_7b_ff)),
            )
            .child(div().ml(px(7.0 * scale)).child("GPUI Preview"))
            .child(div().flex_1())
            .child(native_window_control("—", false, scale))
            .child(native_window_control("□", false, scale))
            .child(native_window_control("×", true, scale));
    }

    decorator
        .id("preview-native-decorator")
        .semantic_role(SemanticRole::Group)
        .accessible_name(format!("{platform} native window decoration preview"))
        .semantic_metadata("platform", platform)
        .semantic_metadata("output_policy", "native")
        .into_any_element()
}

fn native_window_dot(color: u32, scale: f32) -> AnyElement {
    div()
        .w(px(11.0 * scale))
        .h(px(11.0 * scale))
        .rounded(px(6.0 * scale))
        .bg(rgb(color))
        .into_any_element()
}

fn native_window_control(label: &'static str, close: bool, scale: f32) -> AnyElement {
    div()
        .flex()
        .w(px(42.0 * scale))
        .h_full()
        .items_center()
        .justify_center()
        .bg(rgb(if close { 0x3a_20_24 } else { 0x20_20_22 }))
        .text_color(rgb(if close { 0xff_c9_cd } else { 0xc8_c8_cc }))
        .child(label)
        .into_any_element()
}

fn render_document_tabs(state: &Rc<StudioState>) -> AnyElement {
    let (open, active) = {
        let tabs = state.document_tabs.borrow();
        (
            tabs.open_ids().cloned().collect::<Vec<_>>(),
            tabs.active_id().clone(),
        )
    };
    let mut strip = div()
        .flex()
        .h(px(32.0))
        .min_w_0()
        .items_center()
        .gap(px(3.0))
        .overflow_hidden();

    for id in open {
        strip = strip.child(render_document_tab(state, id, &active));
    }

    strip
        .id("studio-document-tabs-list")
        .semantic_role(SemanticRole::TabList)
        .accessible_name("Open editor documents")
        .into_any_element()
}

fn render_document_tab(state: &Rc<StudioState>, id: DocumentId, active: &DocumentId) -> AnyElement {
    let label = match &id {
        // Main renders the project's root component; show its name so the tab
        // reflects the unified graph model rather than an opaque "Main".
        DocumentId::Main => state
            .root_component_id()
            .and_then(|root| {
                state
                    .native_components
                    .borrow()
                    .components
                    .iter()
                    .find(|component| component.id == root)
                    .map(|component| component.name.clone())
            })
            .unwrap_or_else(|| "Main".to_owned()),
        DocumentId::Component(component_id) => state
            .native_components
            .borrow()
            .components
            .iter()
            .find(|component| component.id == *component_id)
            .map_or_else(|| component_id.clone(), |component| component.name.clone()),
    };
    let selected = &id == active;
    let tab_id = match &id {
        DocumentId::Main => "document-tab/main".to_owned(),
        DocumentId::Component(component_id) => format!("document-tab/{component_id}"),
    };
    let click_state = state.clone();
    let click_id = id.clone();
    let click_label = label.clone();
    let tab = div()
        .flex()
        .h_full()
        .min_w(px(if matches!(id, DocumentId::Main) {
            56.0
        } else {
            78.0
        }))
        .max_w(px(156.0))
        .items_center()
        .px(px(10.0))
        .overflow_hidden()
        .text_ellipsis()
        .whitespace_nowrap()
        .text_size(px(12.0))
        .text_color(rgb(if selected { 0xe7_e9_ee } else { 0x8b_91_a0 }))
        .cursor_pointer()
        .child(label.clone())
        .id(SharedString::from(tab_id))
        .on_click(move |_, window, _| {
            if click_state.activate_document(&click_id) {
                click_state.set_status(format!("Activated {click_label} document"));
                window.refresh();
            }
        })
        .semantic_role(SemanticRole::Tab)
        .accessible_name(label.clone())
        .semantic_selected(selected);
    let mut wrapper = div()
        .flex()
        .h(px(29.0))
        .min_w_0()
        .items_center()
        .overflow_hidden()
        .border_1()
        .border_color(rgb(if selected { 0x35_3a_56 } else { 0x20_24_2d }))
        .rounded(px(7.0))
        .bg(rgb(if selected { 0x21_24_32_u32 } else { 0x15_18_1f }))
        .child(tab);
    if let DocumentId::Component(component_id) = id {
        wrapper = wrapper.child(render_document_tab_close(state, &component_id, &label));
    }
    wrapper.into_any_element()
}

fn render_document_tab_close(
    state: &Rc<StudioState>,
    component_id: &str,
    label: &str,
) -> AnyElement {
    let close_state = state.clone();
    let close_component = component_id.to_owned();
    div()
        .flex()
        .w(px(25.0))
        .h_full()
        .flex_none()
        .items_center()
        .justify_center()
        .border_l_1()
        .border_color(rgb(0x29_2d_38))
        .cursor_pointer()
        .child(
            svg()
                .path("icons/close.svg")
                .w(px(10.0))
                .h(px(10.0))
                .text_color(rgb(0x74_7b_89)),
        )
        .id(SharedString::from(format!(
            "document-tab-close/{component_id}"
        )))
        .on_click(move |_, window, _| {
            close_state.close_component_document(&close_component);
            window.refresh();
        })
        .semantic_role(SemanticRole::Button)
        .accessible_name(format!("Close {label}"))
        .semantic_metadata("component_id", component_id)
        .into_any_element()
}

fn render_project_tree(state: &Rc<StudioState>, _window: &mut Window, cx: &mut App) -> AnyElement {
    if state.editing_component_graph() {
        return div().h(px(0.0)).into_any_element();
    }
    let semantic_tree = state.automation.snapshot();
    let model = LayerTree::from_semantics(&semantic_tree, "project-canvas");
    if !state.project_tree_initialized.replace(true) {
        state
            .project_tree_expanded
            .borrow_mut()
            .extend(model.expandable_ids());
    }
    let rows = model.visible_rows(&state.project_tree_expanded.borrow());
    let existing_focus_handle = { state.project_tree_focus_handle.borrow().clone() };
    let focus_handle = if let Some(handle) = existing_focus_handle {
        handle
    } else {
        let handle = cx.focus_handle();
        *state.project_tree_focus_handle.borrow_mut() = Some(handle.clone());
        handle
    };
    let mut tree = div()
        .id("project-layer-tree")
        .track_focus(&focus_handle)
        .key_context("ProjectLayers")
        .flex()
        .w_full()
        .flex_col();

    for (position, row) in rows.iter().enumerate() {
        let selected = state.multi_selection.borrow().contains(&row.runtime_id);
        let focused = state.project_tree_focus.borrow().as_deref() == Some(&row.runtime_id);
        let expanded = row.expandable
            && state
                .project_tree_expanded
                .borrow()
                .contains(&row.runtime_id);
        let icon = match row.kind {
            LayerKind::Component | LayerKind::Control => "component.svg",
            LayerKind::Text => "text.svg",
            LayerKind::Frame | LayerKind::Image => "frame.svg",
        };
        let icon_color = if row.kind == LayerKind::Component {
            0x6e_7b_ff
        } else {
            0x73_7b_8c
        };

        let disclosure = if row.expandable {
            let toggle_state = state.clone();
            let toggle_id = row.runtime_id.clone();
            div()
                .id(SharedString::from(format!(
                    "layer-disclosure-{}",
                    row.authored_id
                )))
                .flex()
                .w(px(14.0))
                .h(px(20.0))
                .flex_none()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .text_color(rgb(0x8b_92_a2))
                .cursor_pointer()
                .child(if expanded { "▾" } else { "▸" })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    toggle_state.toggle_project_layer(&toggle_id);
                    window.refresh();
                })
                .into_any_element()
        } else {
            div().w(px(14.0)).h(px(20.0)).flex_none().into_any_element()
        };

        let click_state = state.clone();
        let click_model = model.clone();
        let click_row = row.clone();
        let mut control = div()
            .id(SharedString::from(format!(
                "project-layer/{}",
                row.authored_id
            )))
            .flex()
            .w_full()
            .h(px(26.0))
            .flex_none()
            .items_center()
            .gap(px(6.0))
            .pl(px(8.0 + row.depth as f32 * 12.0))
            .pr(px(8.0))
            .text_size(px(12.5))
            .line_height(px(18.0))
            .text_color(rgb(if selected { 0xe7_e9_ee } else { 0xa9_af_bd }))
            .cursor_pointer()
            .child(disclosure)
            .child(
                svg()
                    .path(format!("icons/{icon}"))
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(icon_color)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(row.label.clone()),
            )
            .on_click(move |event, window, _| {
                let modifiers = event.modifiers();
                click_state.select_project_layer(
                    &click_model,
                    &click_row,
                    modifiers.shift,
                    modifiers.secondary(),
                );
                let double_click = matches!(
                    event,
                    gpui::ClickEvent::Mouse(mouse) if mouse.up.click_count >= 2
                );
                if double_click && click_row.kind == LayerKind::Component {
                    click_state.open_component_instance(&click_row.runtime_id);
                }
                window.refresh();
            });
        {
            let context_state = state.clone();
            let context_model = model.clone();
            let context_row = row.clone();
            control = control.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                cx.stop_propagation();
                context_state.select_project_layer(&context_model, &context_row, false, false);
                *context_state.context_menu.borrow_mut() = Some(ContextMenuState {
                    x: f32::from(event.position.x),
                    y: f32::from(event.position.y),
                    target: ContextMenuTarget::ProjectLayer,
                });
                window.refresh();
            });
        }
        if state
            .annotations
            .borrow()
            .latest_active_for_target(&row.runtime_id)
            .is_some()
        {
            control = control.child(annotation_dot());
        }
        if selected {
            control = control.bg(rgb(0x22_26_41));
        } else if focused {
            control = control.bg(rgb(0x1b_1f_2d));
        }

        control = control
            .semantic_role(SemanticRole::TreeItem)
            .accessible_name(row.label.clone())
            .semantic_selected(selected)
            .semantic_metadata("runtime_id", row.runtime_id.clone())
            .semantic_metadata("authored_id", row.authored_id.clone())
            .semantic_metadata("depth", row.depth.to_string())
            .semantic_metadata("position", (position + 1).to_string())
            .semantic_metadata("set_size", rows.len().to_string());
        if row.expandable {
            control = control.semantic_expanded(expanded);
        }
        tree = tree.child(control);
    }

    let keyboard_state = state.clone();
    let keyboard_model = model.clone();
    tree = tree.on_key_down(move |event, window, cx| {
        let key = event.keystroke.key.as_str();
        let current = keyboard_state.project_tree_focus.borrow().clone();
        let expanded = keyboard_state.project_tree_expanded.borrow().clone();
        let next = match key {
            "up" => keyboard_model.adjacent(&expanded, current.as_deref(), -1),
            "down" => keyboard_model.adjacent(&expanded, current.as_deref(), 1),
            _ => None,
        };
        if let Some(next) = next
            && let Some(row) = keyboard_model
                .visible_rows(&expanded)
                .into_iter()
                .find(|row| row.runtime_id == next)
        {
            keyboard_state.select_project_layer(&keyboard_model, &row, false, false);
            cx.stop_propagation();
            window.refresh();
            return;
        }
        let Some(current) = current else {
            return;
        };
        let rows = keyboard_model.visible_rows(&expanded);
        let Some(row) = rows.iter().find(|row| row.runtime_id == current) else {
            return;
        };
        match key {
            "right" if row.expandable && !expanded.contains(&current) => {
                keyboard_state
                    .project_tree_expanded
                    .borrow_mut()
                    .insert(current);
            }
            "left" if expanded.contains(&current) => {
                keyboard_state
                    .project_tree_expanded
                    .borrow_mut()
                    .remove(&current);
            }
            "left" => {
                if let Some(parent) = row.parent.as_ref()
                    && let Some(parent_row) = rows.iter().find(|row| &row.runtime_id == parent)
                {
                    keyboard_state.select_project_layer(&keyboard_model, parent_row, false, false);
                }
            }
            "enter" | "space" if row.kind == LayerKind::Component => {
                keyboard_state.open_component_instance(&row.runtime_id);
            }
            "enter" | "space" if row.expandable => {
                keyboard_state.toggle_project_layer(&row.runtime_id);
            }
            _ => return,
        }
        cx.stop_propagation();
        window.refresh();
    });

    tree.semantic_role(SemanticRole::Tree)
        .accessible_name("Project layers")
        .semantic_metadata(
            "selection_count",
            state.multi_selection.borrow().len().to_string(),
        )
        .into_any_element()
}

/// Small badge marking a tree row whose element has an active annotation.
fn annotation_dot() -> AnyElement {
    div()
        .w(px(6.0))
        .h(px(6.0))
        .flex_none()
        .rounded(px(3.0))
        .bg(rgb(0xf3_a3_4d))
        .into_any_element()
}

fn render_component_tree(state: &Rc<StudioState>) -> AnyElement {
    if !state.editing_component_graph() {
        return div().h(px(0.0)).into_any_element();
    }
    let component = state
        .native_components
        .borrow()
        .active()
        .map(|component| (component.id.clone(), component.root.clone()));
    let Some((component_id, root)) = component else {
        return div().h(px(0.0)).into_any_element();
    };
    if state.component_tree_initialized_for.borrow().as_deref() != Some(&component_id) {
        let mut expanded = BTreeSet::new();
        collect_expandable_component_nodes(&component_id, &root, &mut expanded);
        *state.component_nodes_expanded.borrow_mut() = expanded;
        *state.component_tree_initialized_for.borrow_mut() = Some(component_id.clone());
    }
    let mut rows = Vec::new();
    for child in &root.children {
        append_component_tree_row(&mut rows, state, &component_id, child, 2);
    }
    rows.into_iter()
        .fold(div().flex().w_full().flex_col(), gpui::ParentElement::child)
        .id(SharedString::from(format!("component-tree/{component_id}")))
        .semantic_role(SemanticRole::Tree)
        .accessible_name("Component layers")
        .semantic_metadata("component_id", component_id)
        .into_any_element()
}

fn collect_expandable_component_nodes(
    component_id: &str,
    node: &NativeNode,
    output: &mut BTreeSet<String>,
) {
    if !node.children.is_empty() {
        output.insert(format!("component/{component_id}/{}", node.id));
        for child in &node.children {
            collect_expandable_component_nodes(component_id, child, output);
        }
    }
}

fn append_component_tree_row(
    rows: &mut Vec<AnyElement>,
    state: &Rc<StudioState>,
    component_id: &str,
    node: &NativeNode,
    depth: u16,
) {
    let target_id = format!("component/{component_id}/{}", node.id);
    let selected = state.selection.borrow().runtime_id == target_id;
    let expanded =
        !node.children.is_empty() && state.component_nodes_expanded.borrow().contains(&target_id);
    let label = match node.kind {
        NativeNodeKind::Text => node
            .text
            .as_deref()
            .map(|text| text.chars().take(32).collect())
            .filter(|text: &String| !text.is_empty())
            .unwrap_or_else(|| authored_name(&node.id)),
        NativeNodeKind::Instance => node
            .instance_of
            .as_deref()
            .and_then(|referenced| {
                state
                    .native_components
                    .borrow()
                    .components
                    .iter()
                    .find(|component| component.id == referenced)
                    .map(|component| component.name.clone())
            })
            .unwrap_or_else(|| authored_name(&node.id)),
        _ => authored_name(&node.id),
    };
    let icon = match node.kind {
        NativeNodeKind::Text => "text.svg",
        NativeNodeKind::Button => "button.svg",
        NativeNodeKind::Instance => "instance.svg",
        NativeNodeKind::Row => "row.svg",
        NativeNodeKind::Column => "column.svg",
        NativeNodeKind::Grid => "grid.svg",
        NativeNodeKind::Stack | NativeNodeKind::Titlebar => "frame.svg",
    };
    let click_state = state.clone();
    let click_component = component_id.to_owned();
    let click_node = node.id.clone();
    let disclosure = if node.children.is_empty() {
        div().w(px(14.0)).h(px(20.0)).flex_none().into_any_element()
    } else {
        let disclosure_state = state.clone();
        let disclosure_id = target_id.clone();
        div()
            .id(SharedString::from(format!(
                "component-tree-disclosure-{component_id}-{}",
                node.id
            )))
            .flex()
            .w(px(14.0))
            .h(px(20.0))
            .flex_none()
            .items_center()
            .justify_center()
            .text_size(px(11.0))
            .text_color(rgb(0x8b_92_a2))
            .cursor_pointer()
            .child(if expanded { "▾" } else { "▸" })
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                let mut expanded = disclosure_state.component_nodes_expanded.borrow_mut();
                if !expanded.remove(&disclosure_id) {
                    expanded.insert(disclosure_id.clone());
                }
                window.refresh();
            })
            .into_any_element()
    };
    let mut row = div()
        .flex()
        .w_full()
        .h(px(26.0))
        .flex_none()
        .items_center()
        .gap(px(6.0))
        .pl(px(8.0 + f32::from(depth) * 12.0))
        .pr(px(8.0))
        .text_size(px(12.5))
        .text_color(rgb(if selected { 0xe7_e9_ee } else { 0xa9_af_bd }))
        .cursor_pointer()
        .child(disclosure)
        .child(
            svg()
                .path(format!("icons/{icon}"))
                .w(px(13.0))
                .h(px(13.0))
                .flex_none()
                .text_color(rgb(0x73_7b_8c)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label.clone()),
        )
        .id(SharedString::from(format!(
            "component-tree/{component_id}/{}",
            node.id
        )))
        .on_click(move |_, window, _| {
            click_state.select_component_node(&click_component, &click_node);
            window.refresh();
        });
    {
        let context_state = state.clone();
        let context_component = component_id.to_owned();
        let context_node = node.id.clone();
        row = row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
            cx.stop_propagation();
            context_state.select_component_node(&context_component, &context_node);
            *context_state.context_menu.borrow_mut() = Some(ContextMenuState {
                x: f32::from(event.position.x),
                y: f32::from(event.position.y),
                target: ContextMenuTarget::ComponentNode {
                    component_id: context_component.clone(),
                    node_id: context_node.clone(),
                },
            });
            window.refresh();
        });
    }
    if node.id != "root" {
        row = row.on_drag(
            TreeNodeDrag {
                component_id: component_id.to_owned(),
                node_id: node.id.clone(),
                label: label.clone(),
            },
            |drag, _, _, cx| {
                let label = SharedString::from(drag.label.clone());
                cx.new(|_| DragGhost { label })
            },
        );
    }
    {
        let can_drop_state = state.clone();
        let can_drop_target = node.id.clone();
        let tree_drop_state = state.clone();
        let tree_drop_target = node.id.clone();
        let palette_drop_state = state.clone();
        let palette_drop_target = node.id.clone();
        row = row
            .drag_over::<TreeNodeDrag>(|style, _, _, _| style.bg(rgb(0x1c_29_4a)))
            .drag_over::<PaletteDrag>(|style, _, _, _| style.bg(rgb(0x1c_29_4a)))
            .can_drop(move |payload, _, _| {
                if let Some(drag) = payload.downcast_ref::<TreeNodeDrag>() {
                    can_drop_state.can_drop_tree_node(drag, &can_drop_target)
                } else if let Some(drag) = payload.downcast_ref::<PaletteDrag>() {
                    can_drop_state.active_component_id().is_some_and(|active| {
                        !can_drop_state
                            .native_components
                            .borrow()
                            .would_cycle(&active, &drag.component_id)
                    })
                } else {
                    false
                }
            })
            .on_drop::<TreeNodeDrag>(move |drag, window, _| {
                tree_drop_state.drop_tree_node(drag, &tree_drop_target);
                window.refresh();
            })
            .on_drop::<PaletteDrag>(move |drag, window, _| {
                palette_drop_state.drop_palette_on_node(drag, &palette_drop_target);
                window.refresh();
            });
    }
    if state
        .annotations
        .borrow()
        .latest_active_for_target(&target_id)
        .is_some()
    {
        row = row.child(annotation_dot());
    }
    if selected {
        row = row.bg(rgb(0x22_26_41));
    }
    row = row
        .semantic_role(SemanticRole::TreeItem)
        .accessible_name(label)
        .semantic_selected(selected)
        .semantic_metadata("component_id", component_id)
        .semantic_metadata("component_node", node.id.clone())
        .semantic_metadata("target_runtime_id", target_id);
    if !node.children.is_empty() {
        row = row.semantic_expanded(expanded);
    }
    rows.push(row.into_any_element());
    if expanded {
        for child in &node.children {
            append_component_tree_row(rows, state, component_id, child, depth.saturating_add(1));
        }
    }
}

fn project_components(state: &Rc<StudioState>) -> Result<ComponentRegistry> {
    let component_state = state.clone();
    let mut components = ComponentRegistry::new();
    components.register("gpui-component", move |node, _, _, _| {
        let library = component_state.native_components.borrow();
        let requested = node.attribute("component").unwrap_or("active");
        let component = if requested == "active" {
            library.active()
        } else {
            library.components.iter().find(|component| {
                component.id == requested || component.name.eq_ignore_ascii_case(requested)
            })
        };
        component.map_or_else(
            || "Unknown component".to_owned().into_any_element(),
            |component| component.render(&library, &component_state.automation),
        )
    })?;
    Ok(components)
}

fn initial_selection(
    _backend: AuthoringBackend,
    native_components: &ComponentLibrary,
    revision: u64,
) -> ElementSelection {
    // Main now edits the root component graph, so seed its selection with that
    // component's root node instead of a welcome-page element.
    native_components.components.first().map_or_else(
        || ElementSelection::new("hero-title", "project-canvas--hero-title", revision, None),
        |root| {
            ElementSelection::new(
                "root",
                format!("component/{}/root", root.id),
                revision,
                None,
            )
        },
    )
}

fn disclosure_symbol(collapsed: bool) -> String {
    if collapsed { "▸" } else { "▾" }.to_owned()
}

fn studio_hooks(state: &Rc<StudioState>) -> Result<HookRegistry> {
    let mut hooks = HookRegistry::new();
    register_studio_text_states(&mut hooks, state)?;
    register_annotation_draft_state(&mut hooks, state)?;
    register_component_input_states(&mut hooks, state)?;
    register_inspector_states(&mut hooks, state)?;

    let inspector_state = state.clone();
    hooks.register_state(StateBindingId::new("css_excerpt"), move |_, _| {
        StateValue::Text(inspector_state.inspector_code_excerpt())
    })?;
    let zoom_state = state.clone();
    hooks.register_state(StateBindingId::new("zoom_label"), move |window, _| {
        StateValue::Text(zoom_state.zoom_label(window))
    })?;
    for spec in state.resizable.specs() {
        let size_state = state.clone();
        let target = spec.target;
        hooks.register_state(StateBindingId::new(spec.binding_id()), move |_, _| {
            StateValue::Number(f64::from(size_state.panel_size(target)))
        })?;
    }

    register_studio_events(&mut hooks, state)?;
    Ok(hooks)
}

fn register_studio_text_states(hooks: &mut HookRegistry, state: &Rc<StudioState>) -> Result<()> {
    register_text_state(hooks, "studio_status", state, StudioState::status)?;
    register_text_state(hooks, "project_path", state, StudioState::project_path)?;
    register_text_state(hooks, "revision_label", state, StudioState::revision_label)?;
    register_text_state(hooks, "dirty_label", state, StudioState::dirty_label)?;
    register_text_state(hooks, "mode_label", state, StudioState::mode_label)?;
    register_text_state(hooks, "backend_label", state, StudioState::backend_label)?;
    register_text_state(
        hooks,
        "component_summary",
        state,
        StudioState::component_summary,
    )?;
    register_text_state(
        hooks,
        "active_component_name",
        state,
        StudioState::active_component_name,
    )?;
    register_text_state(
        hooks,
        "selection_label",
        state,
        StudioState::selection_label,
    )?;
    register_text_state(
        hooks,
        "selection_runtime_id",
        state,
        StudioState::selection_runtime_id,
    )?;
    register_text_state(
        hooks,
        "selection_heading",
        state,
        StudioState::selection_heading,
    )?;
    register_text_state(
        hooks,
        "selection_element_tag",
        state,
        StudioState::selection_element_tag,
    )?;
    register_text_state(
        hooks,
        "selection_width",
        state,
        StudioState::selection_width_label,
    )?;
    register_text_state(
        hooks,
        "selection_height",
        state,
        StudioState::selection_height_label,
    )?;
    register_text_state(hooks, "selection_x", state, StudioState::selection_x_label)?;
    register_text_state(hooks, "selection_y", state, StudioState::selection_y_label)?;
    register_text_state(
        hooks,
        "selection_rotation",
        state,
        StudioState::selection_rotation_label,
    )?;
    register_text_state(
        hooks,
        "selection_grid",
        state,
        StudioState::selection_grid_label,
    )?;
    register_text_state(
        hooks,
        "selection_size",
        state,
        StudioState::selection_size_label,
    )?;
    register_text_state(
        hooks,
        "selection_rect",
        state,
        StudioState::selection_rect_label,
    )?;
    register_annotation_states(hooks, state)?;
    register_text_state(hooks, "theme_label", state, StudioState::theme_label)?;
    register_boolean_state(hooks, "layers_visible", state, StudioState::layers_visible)?;
    register_boolean_state(hooks, "files_visible", state, StudioState::files_visible)?;
    register_boolean_state(
        hooks,
        "backend_html_selected",
        state,
        StudioState::backend_html_selected,
    )?;
    register_boolean_state(
        hooks,
        "backend_gpui_selected",
        state,
        StudioState::backend_gpui_selected,
    )?;
    register_boolean_state(
        hooks,
        "mode_design_selected",
        state,
        StudioState::mode_design_selected,
    )?;
    register_boolean_state(
        hooks,
        "mode_test_selected",
        state,
        StudioState::mode_test_selected,
    )?;
    register_boolean_state(
        hooks,
        "mode_compare_selected",
        state,
        StudioState::mode_compare_selected,
    )?;
    register_boolean_state(
        hooks,
        "tool_select_selected",
        state,
        StudioState::tool_select_selected,
    )?;
    register_boolean_state(
        hooks,
        "tool_move_selected",
        state,
        StudioState::tool_move_selected,
    )?;
    register_boolean_state(
        hooks,
        "tool_annotate_selected",
        state,
        StudioState::tool_annotate_selected,
    )?;
    register_boolean_state(
        hooks,
        "view_design_selected",
        state,
        StudioState::view_design_selected,
    )?;
    register_boolean_state(
        hooks,
        "view_preview_selected",
        state,
        StudioState::view_preview_selected,
    )?;
    register_text_state(hooks, "viewport_label", state, StudioState::viewport_label)?;
    register_text_state(
        hooks,
        "decorations_label",
        state,
        StudioState::decorations_label,
    )?;
    register_text_state(hooks, "snap_label", state, StudioState::snap_label)?;
    register_text_state(
        hooks,
        "dock_toggle_label",
        state,
        StudioState::dock_toggle_label,
    )?;
    register_text_state(
        hooks,
        "welcome_disclosure",
        state,
        StudioState::welcome_disclosure,
    )?;
    register_text_state(
        hooks,
        "header_disclosure",
        state,
        StudioState::header_disclosure,
    )?;
    register_text_state(
        hooks,
        "welcome_copy_disclosure",
        state,
        StudioState::welcome_copy_disclosure,
    )?;
    register_text_state(
        hooks,
        "feature_cards_disclosure",
        state,
        StudioState::feature_cards_disclosure,
    )?;
    register_text_state(
        hooks,
        "welcome_lower_disclosure",
        state,
        StudioState::welcome_lower_disclosure,
    )?;
    register_text_state(
        hooks,
        "component_root_disclosure",
        state,
        StudioState::component_root_disclosure,
    )?;
    register_boolean_state(
        hooks,
        "project_menu_visible",
        state,
        StudioState::project_menu_visible,
    )?;
    register_boolean_state(
        hooks,
        "viewport_menu_visible",
        state,
        StudioState::viewport_menu_visible,
    )?;
    register_boolean_state(
        hooks,
        "inspector_layout_visible",
        state,
        StudioState::inspector_layout_visible,
    )?;
    register_boolean_state(
        hooks,
        "inspector_style_visible",
        state,
        StudioState::inspector_style_visible,
    )?;
    register_boolean_state(
        hooks,
        "inspector_logic_visible",
        state,
        StudioState::inspector_logic_visible,
    )?;
    register_boolean_state(
        hooks,
        "constraint_left_selected",
        state,
        StudioState::constraint_left_selected,
    )?;
    register_boolean_state(
        hooks,
        "constraint_center_selected",
        state,
        StudioState::constraint_center_selected,
    )?;
    register_boolean_state(
        hooks,
        "constraint_right_selected",
        state,
        StudioState::constraint_right_selected,
    )?;
    register_boolean_state(
        hooks,
        "constraint_scale_selected",
        state,
        StudioState::constraint_scale_selected,
    )?;
    register_boolean_state(
        hooks,
        "console_visible",
        state,
        StudioState::console_visible,
    )?;
    register_boolean_state(hooks, "states_visible", state, StudioState::states_visible)?;
    register_boolean_state(
        hooks,
        "dock_console_selected",
        state,
        StudioState::dock_console_selected,
    )?;
    register_boolean_state(
        hooks,
        "dock_states_selected",
        state,
        StudioState::dock_states_selected,
    )?;
    register_boolean_state(
        hooks,
        "welcome_children_visible",
        state,
        StudioState::welcome_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "header_children_visible",
        state,
        StudioState::header_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "welcome_copy_children_visible",
        state,
        StudioState::welcome_copy_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "feature_cards_children_visible",
        state,
        StudioState::feature_cards_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "welcome_lower_children_visible",
        state,
        StudioState::welcome_lower_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "layer_welcome_selected",
        state,
        StudioState::layer_welcome_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_runtime_badge_selected",
        state,
        StudioState::layer_runtime_badge_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_header_selected",
        state,
        StudioState::layer_header_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_welcome_copy_selected",
        state,
        StudioState::layer_welcome_copy_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_hero_title_selected",
        state,
        StudioState::layer_hero_title_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_feature_cards_selected",
        state,
        StudioState::layer_feature_cards_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_runtime_building_card_selected",
        state,
        StudioState::layer_runtime_building_card_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_local_ai_card_selected",
        state,
        StudioState::layer_local_ai_card_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_portable_source_card_selected",
        state,
        StudioState::layer_portable_source_card_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_component_selected",
        state,
        StudioState::layer_component_selected,
    )?;
    register_boolean_state(
        hooks,
        "page_tree_visible",
        state,
        StudioState::page_tree_visible,
    )?;
    register_boolean_state(
        hooks,
        "component_document_tree_visible",
        state,
        StudioState::component_document_tree_visible,
    )?;
    register_boolean_state(
        hooks,
        "component_tree_children_visible",
        state,
        StudioState::component_tree_children_visible,
    )?;
    register_boolean_state(
        hooks,
        "component_root_selected",
        state,
        StudioState::component_root_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_runtime_details_selected",
        state,
        StudioState::layer_runtime_details_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_welcome_lower_selected",
        state,
        StudioState::layer_welcome_lower_selected,
    )?;
    register_boolean_state(
        hooks,
        "layer_dogfood_note_selected",
        state,
        StudioState::layer_dogfood_note_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_dialog_visible",
        state,
        StudioState::component_dialog_visible,
    )?;
    register_text_state(hooks, "component_stub", state, StudioState::component_stub)?;
    register_boolean_state(
        hooks,
        "component_html_selected",
        state,
        StudioState::component_html_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_gpui_selected",
        state,
        StudioState::component_gpui_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_selection_selected",
        state,
        StudioState::component_selection_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_blank_selected",
        state,
        StudioState::component_blank_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_paste_html_selected",
        state,
        StudioState::component_paste_html_selected,
    )?;
    register_boolean_state(
        hooks,
        "component_titlebar_selected",
        state,
        StudioState::component_titlebar_selected,
    )?;

    Ok(())
}

fn register_annotation_states(hooks: &mut HookRegistry, state: &Rc<StudioState>) -> Result<()> {
    register_text_state(
        hooks,
        "annotation_count",
        state,
        StudioState::annotation_count_label,
    )?;
    register_text_state(
        hooks,
        "annotation_title",
        state,
        StudioState::annotation_title,
    )?;
    register_text_state(
        hooks,
        "annotation_target",
        state,
        StudioState::annotation_target_label,
    )?;
    register_text_state(
        hooks,
        "annotation_status",
        state,
        StudioState::annotation_status_label,
    )?;
    register_text_state(
        hooks,
        "send_annotations_label",
        state,
        StudioState::send_annotations_label,
    )?;
    register_boolean_state(
        hooks,
        "annotation_popover_visible",
        state,
        StudioState::annotation_popover_visible,
    )?;
    register_boolean_state(
        hooks,
        "annotation_has_saved_comment",
        state,
        StudioState::annotation_has_saved_comment,
    )?;
    register_boolean_state(
        hooks,
        "send_annotations_visible",
        state,
        StudioState::send_annotations_visible,
    )?;
    Ok(())
}

fn register_annotation_draft_state(
    hooks: &mut HookRegistry,
    state: &Rc<StudioState>,
) -> Result<()> {
    let draft_reader = state.clone();
    let draft_writer = state.clone();
    hooks.register_state_mut(
        StateBindingId::new("annotation_draft"),
        move |_, _| StateValue::Text(draft_reader.annotation_draft.borrow().clone()),
        move |value, window, _| {
            let StateValue::Text(value) = value else {
                return ActionOutcome::Rejected {
                    reason: "annotation draft requires text".to_owned(),
                };
            };
            *draft_writer.annotation_draft.borrow_mut() = value;
            // Typing is the save: mark dirty and let the quiet-period flush
            // auto-persist the annotation. No explicit save button.
            draft_writer.annotation_dirty.set(true);
            draft_writer.annotation_quiet_ticks.set(0);
            window.refresh();
            ActionOutcome::Handled
        },
    )?;
    Ok(())
}

fn register_component_input_states(
    hooks: &mut HookRegistry,
    state: &Rc<StudioState>,
) -> Result<()> {
    let name_reader = state.clone();
    let name_writer = state.clone();
    hooks.register_state_mut(
        StateBindingId::new("component_name"),
        move |_, _| StateValue::Text(name_reader.component_name.borrow().clone()),
        move |value, window, _| {
            let StateValue::Text(value) = value else {
                return ActionOutcome::Rejected {
                    reason: "component name requires text".to_owned(),
                };
            };
            *name_writer.component_name.borrow_mut() = value;
            window.refresh();
            ActionOutcome::Handled
        },
    )?;

    let props_reader = state.clone();
    let props_writer = state.clone();
    hooks.register_state_mut(
        StateBindingId::new("component_props"),
        move |_, _| StateValue::Text(props_reader.component_props.borrow().clone()),
        move |value, window, _| {
            let StateValue::Text(value) = value else {
                return ActionOutcome::Rejected {
                    reason: "component props require text".to_owned(),
                };
            };
            *props_writer.component_props.borrow_mut() = value;
            window.refresh();
            ActionOutcome::Handled
        },
    )?;

    for (slug, preset) in component_preset_bindings() {
        let reader = state.clone();
        hooks.register_state(
            StateBindingId::new(format!("preset_{slug}_selected")),
            move |_, _| StateValue::Boolean(reader.component_preset_selected(preset)),
        )?;
    }
    Ok(())
}

fn register_inspector_states(hooks: &mut HookRegistry, state: &Rc<StudioState>) -> Result<()> {
    for (id, field) in [
        ("inspector_prop_name", StudioInspectorDraftField::Name),
        ("inspector_prop_type", StudioInspectorDraftField::Type),
        ("inspector_prop_default", StudioInspectorDraftField::Default),
    ] {
        let reader = state.clone();
        let writer = state.clone();
        hooks.register_state_mut(
            StateBindingId::new(id),
            move |_, _| StateValue::Text(reader.inspector_draft(field)),
            move |value, window, _| {
                let StateValue::Text(value) = value else {
                    return ActionOutcome::Rejected {
                        reason: "inspector property draft requires text".to_owned(),
                    };
                };
                writer.set_inspector_draft(field, value);
                window.refresh();
                ActionOutcome::Handled
            },
        )?;
    }

    for (id, field) in [
        ("inspector_state_name", DefinitionDraftField::StateName),
        ("inspector_state_type", DefinitionDraftField::StateType),
        (
            "inspector_state_default",
            DefinitionDraftField::StateDefault,
        ),
        ("inspector_variant_id", DefinitionDraftField::VariantId),
        ("inspector_variant_name", DefinitionDraftField::VariantName),
        ("inspector_slot_name", DefinitionDraftField::SlotName),
        ("inspector_slot_node", DefinitionDraftField::SlotNode),
        ("inspector_token_path", DefinitionDraftField::TokenPath),
        ("inspector_token_kind", DefinitionDraftField::TokenKind),
        ("inspector_token_value", DefinitionDraftField::TokenValue),
        (
            "inspector_token_description",
            DefinitionDraftField::TokenDescription,
        ),
    ] {
        let reader = state.clone();
        let writer = state.clone();
        hooks.register_state_mut(
            StateBindingId::new(id),
            move |_, _| StateValue::Text(reader.definition_draft(field)),
            move |value, window, _| {
                let StateValue::Text(value) = value else {
                    return ActionOutcome::Rejected {
                        reason: "definition field requires text".to_owned(),
                    };
                };
                writer.set_definition_draft(field, value);
                window.refresh();
                ActionOutcome::Handled
            },
        )?;
    }

    for (id, field) in [
        ("inspector_width_value", InspectorValueField::Width),
        ("inspector_height_value", InspectorValueField::Height),
        ("inspector_gap_value", InspectorValueField::Gap),
        ("inspector_padding_value", InspectorValueField::Padding),
        ("inspector_margin_value", InspectorValueField::Margin),
        ("inspector_basis_value", InspectorValueField::Basis),
        ("inspector_min_width_value", InspectorValueField::MinWidth),
        ("inspector_max_width_value", InspectorValueField::MaxWidth),
        ("inspector_min_height_value", InspectorValueField::MinHeight),
        ("inspector_max_height_value", InspectorValueField::MaxHeight),
        (
            "inspector_grid_columns_value",
            InspectorValueField::GridColumns,
        ),
        ("inspector_grid_rows_value", InspectorValueField::GridRows),
        (
            "inspector_column_start_value",
            InspectorValueField::ColumnStart,
        ),
        (
            "inspector_column_span_value",
            InspectorValueField::ColumnSpan,
        ),
        ("inspector_row_start_value", InspectorValueField::RowStart),
        ("inspector_row_span_value", InspectorValueField::RowSpan),
        (
            "inspector_offset_left_value",
            InspectorValueField::OffsetLeft,
        ),
        ("inspector_offset_top_value", InspectorValueField::OffsetTop),
        (
            "inspector_offset_right_value",
            InspectorValueField::OffsetRight,
        ),
        (
            "inspector_offset_bottom_value",
            InspectorValueField::OffsetBottom,
        ),
        ("inspector_z_index_value", InspectorValueField::ZIndex),
        ("inspector_opacity_value", InspectorValueField::Opacity),
        ("inspector_rotation_value", InspectorValueField::Rotation),
        ("inspector_radius_value", InspectorValueField::Radius),
        (
            "inspector_background_value",
            InspectorValueField::Background,
        ),
        (
            "inspector_foreground_value",
            InspectorValueField::Foreground,
        ),
        ("inspector_border_value", InspectorValueField::Border),
        ("inspector_text_value", InspectorValueField::Text),
        ("inspector_font_value", InspectorValueField::FontFamily),
        ("inspector_font_size_value", InspectorValueField::FontSize),
        (
            "inspector_font_weight_value",
            InspectorValueField::FontWeight,
        ),
        (
            "inspector_line_height_value",
            InspectorValueField::LineHeight,
        ),
        ("inspector_action_value", InspectorValueField::Action),
    ] {
        let reader = state.clone();
        let writer = state.clone();
        hooks.register_state_mut(
            StateBindingId::new(id),
            move |_, _| StateValue::Text(reader.inspector_value(field)),
            move |value, window, _| {
                let StateValue::Text(value) = value else {
                    return ActionOutcome::Rejected {
                        reason: "inspector field requires text".to_owned(),
                    };
                };
                match writer.set_inspector_value(field, value) {
                    Ok(()) => {
                        window.refresh();
                        ActionOutcome::Handled
                    }
                    Err(reason) => ActionOutcome::Rejected { reason },
                }
            },
        )?;
    }

    register_boolean_state(
        hooks,
        "inspector_component_editable",
        state,
        StudioState::inspector_component_editable,
    )?;
    let slot_multiple = state.clone();
    hooks.register_state(StateBindingId::new("slot_multiple"), move |_, _| {
        StateValue::Boolean(slot_multiple.inspector_slot_multiple.get())
    })?;
    let slot_capacity = state.clone();
    hooks.register_state(StateBindingId::new("slot_capacity_label"), move |_, _| {
        StateValue::Text(
            if slot_capacity.inspector_slot_multiple.get() {
                "Multiple children"
            } else {
                "Single child"
            }
            .to_owned(),
        )
    })?;
    register_boolean_state(
        hooks,
        "inspector_page_notice_visible",
        state,
        StudioState::inspector_page_notice_visible,
    )?;
    for (id, axis) in [
        ("inspector_width_intrinsic", InspectorSizeAxis::Width),
        ("inspector_height_intrinsic", InspectorSizeAxis::Height),
    ] {
        let reader = state.clone();
        hooks.register_state(StateBindingId::new(id), move |_, _| {
            StateValue::Boolean(reader.inspector_axis_intrinsic(axis))
        })?;
    }
    for (id, axis, size) in [
        (
            "inspector_width_hug",
            InspectorSizeAxis::Width,
            NativeSize::Hug,
        ),
        (
            "inspector_width_fill",
            InspectorSizeAxis::Width,
            NativeSize::Fill,
        ),
        (
            "inspector_width_fixed",
            InspectorSizeAxis::Width,
            NativeSize::Fixed(0),
        ),
        (
            "inspector_height_hug",
            InspectorSizeAxis::Height,
            NativeSize::Hug,
        ),
        (
            "inspector_height_fill",
            InspectorSizeAxis::Height,
            NativeSize::Fill,
        ),
        (
            "inspector_height_fixed",
            InspectorSizeAxis::Height,
            NativeSize::Fixed(0),
        ),
    ] {
        let reader = state.clone();
        hooks.register_state(StateBindingId::new(id), move |_, _| {
            StateValue::Boolean(reader.inspector_size_selected(axis, size))
        })?;
    }
    for (id, axis, alignment) in [
        (
            "inspector_align_start",
            InspectorAlignmentAxis::Align,
            NativeAlign::Start,
        ),
        (
            "inspector_align_center",
            InspectorAlignmentAxis::Align,
            NativeAlign::Center,
        ),
        (
            "inspector_align_end",
            InspectorAlignmentAxis::Align,
            NativeAlign::End,
        ),
        (
            "inspector_justify_start",
            InspectorAlignmentAxis::Justify,
            NativeAlign::Start,
        ),
        (
            "inspector_justify_center",
            InspectorAlignmentAxis::Justify,
            NativeAlign::Center,
        ),
        (
            "inspector_justify_end",
            InspectorAlignmentAxis::Justify,
            NativeAlign::End,
        ),
    ] {
        let reader = state.clone();
        hooks.register_state(StateBindingId::new(id), move |_, _| {
            StateValue::Boolean(reader.inspector_alignment_selected(axis, alignment))
        })?;
    }
    for (id, choice) in inspector_layout_choice_bindings() {
        let reader = state.clone();
        hooks.register_state(StateBindingId::new(id), move |_, _| {
            StateValue::Boolean(reader.inspector_layout_choice_selected(choice))
        })?;
    }
    Ok(())
}

fn register_studio_events(hooks: &mut HookRegistry, state: &Rc<StudioState>) -> Result<()> {
    register_event(hooks, "save_project", state, StudioState::save)?;
    register_event(hooks, "reload_project", state, StudioState::reload_external)?;
    register_event(hooks, "undo", state, StudioState::undo)?;
    register_event(hooks, "redo", state, StudioState::redo)?;
    register_event(
        hooks,
        "delete_annotation",
        state,
        StudioState::delete_annotation,
    )?;
    register_event(
        hooks,
        "close_annotation",
        state,
        StudioState::close_annotation,
    )?;
    register_event(
        hooks,
        "send_annotations",
        state,
        StudioState::send_annotations,
    )?;
    register_event(hooks, "cycle_theme", state, StudioState::cycle_theme)?;
    register_event(hooks, "open_settings", state, StudioState::open_settings)?;
    register_event(
        hooks,
        "toggle_annotations_drawer",
        state,
        StudioState::toggle_annotations_drawer,
    )?;
    register_event(hooks, "show_layers", state, StudioState::show_layers)?;
    register_event(hooks, "show_files", state, StudioState::show_files)?;
    register_event(
        hooks,
        "toggle_project_menu",
        state,
        StudioState::toggle_project_menu,
    )?;
    register_event(
        hooks,
        "show_project_canvas",
        state,
        StudioState::focus_main_root,
    )?;
    register_event(
        hooks,
        "show_component_library",
        state,
        StudioState::select_active_component,
    )?;
    register_event(
        hooks,
        "project_open_help",
        state,
        StudioState::project_open_help,
    )?;
    register_event(
        hooks,
        "toggle_viewport_menu",
        state,
        StudioState::toggle_viewport_menu,
    )?;
    register_viewport_event(
        hooks,
        "viewport_responsive",
        state,
        ViewportPreset::Responsive,
    )?;
    register_viewport_event(hooks, "viewport_desktop", state, ViewportPreset::Desktop)?;
    register_viewport_event(hooks, "viewport_tablet", state, ViewportPreset::Tablet)?;
    register_viewport_event(hooks, "viewport_mobile", state, ViewportPreset::Mobile)?;
    register_event(
        hooks,
        "rotate_viewport",
        state,
        StudioState::rotate_viewport,
    )?;
    register_event(
        hooks,
        "toggle_output_decorations",
        state,
        StudioState::toggle_output_decorations,
    )?;
    register_event(hooks, "toggle_snap", state, StudioState::toggle_snap)?;
    register_zoom_event(hooks, "zoom_out", state, -10)?;
    register_fit_event(hooks, "zoom_fit", state)?;
    register_zoom_event(hooks, "zoom_in", state, 10)?;
    register_dock_event(hooks, "dock_console", state, DockTab::Console)?;
    register_dock_event(hooks, "dock_states", state, DockTab::States)?;
    register_event(hooks, "toggle_dock", state, StudioState::toggle_dock)?;
    register_inspector_event(hooks, "inspector_layout", state, InspectorTab::Layout)?;
    register_inspector_event(hooks, "inspector_style", state, InspectorTab::Style)?;
    register_inspector_event(hooks, "inspector_logic", state, InspectorTab::Logic)?;
    for (id, axis, size) in [
        (
            "inspector_width_hug",
            InspectorSizeAxis::Width,
            NativeSize::Hug,
        ),
        (
            "inspector_width_fill",
            InspectorSizeAxis::Width,
            NativeSize::Fill,
        ),
        (
            "inspector_width_fixed",
            InspectorSizeAxis::Width,
            NativeSize::Fixed(0),
        ),
        (
            "inspector_height_hug",
            InspectorSizeAxis::Height,
            NativeSize::Hug,
        ),
        (
            "inspector_height_fill",
            InspectorSizeAxis::Height,
            NativeSize::Fill,
        ),
        (
            "inspector_height_fixed",
            InspectorSizeAxis::Height,
            NativeSize::Fixed(0),
        ),
    ] {
        register_inspector_size_event(hooks, id, state, axis, size)?;
    }
    for (id, axis, alignment) in [
        (
            "inspector_align_start",
            InspectorAlignmentAxis::Align,
            NativeAlign::Start,
        ),
        (
            "inspector_align_center",
            InspectorAlignmentAxis::Align,
            NativeAlign::Center,
        ),
        (
            "inspector_align_end",
            InspectorAlignmentAxis::Align,
            NativeAlign::End,
        ),
        (
            "inspector_justify_start",
            InspectorAlignmentAxis::Justify,
            NativeAlign::Start,
        ),
        (
            "inspector_justify_center",
            InspectorAlignmentAxis::Justify,
            NativeAlign::Center,
        ),
        (
            "inspector_justify_end",
            InspectorAlignmentAxis::Justify,
            NativeAlign::End,
        ),
    ] {
        register_inspector_alignment_event(hooks, id, state, axis, alignment)?;
    }
    for (id, choice) in inspector_layout_choice_bindings() {
        register_inspector_layout_choice_event(hooks, id, state, choice)?;
    }
    register_constraint_event(hooks, "constraint_left", state, HorizontalConstraint::Left)?;
    register_constraint_event(
        hooks,
        "constraint_center",
        state,
        HorizontalConstraint::Center,
    )?;
    register_constraint_event(
        hooks,
        "constraint_right",
        state,
        HorizontalConstraint::Right,
    )?;
    register_constraint_event(
        hooks,
        "constraint_scale",
        state,
        HorizontalConstraint::Scale,
    )?;
    register_event(hooks, "add_property", state, StudioState::add_property)?;
    register_event(
        hooks,
        "save_component_state",
        state,
        StudioState::save_component_state,
    )?;
    register_event(
        hooks,
        "remove_component_state",
        state,
        StudioState::remove_component_state,
    )?;
    register_event(
        hooks,
        "save_component_variant",
        state,
        StudioState::save_component_variant,
    )?;
    register_event(
        hooks,
        "capture_variant_override",
        state,
        StudioState::capture_variant_override,
    )?;
    register_event(
        hooks,
        "remove_component_variant",
        state,
        StudioState::remove_component_variant,
    )?;
    register_event(
        hooks,
        "save_component_slot",
        state,
        StudioState::save_component_slot,
    )?;
    register_event(
        hooks,
        "toggle_slot_multiple",
        state,
        StudioState::toggle_slot_multiple,
    )?;
    register_event(
        hooks,
        "remove_component_slot",
        state,
        StudioState::remove_component_slot,
    )?;
    register_event(
        hooks,
        "save_design_token",
        state,
        StudioState::save_design_token,
    )?;
    register_event(
        hooks,
        "remove_design_token",
        state,
        StudioState::remove_design_token,
    )?;
    register_event(
        hooks,
        "remove_property",
        state,
        StudioState::remove_property,
    )?;
    register_event(hooks, "add_logic", state, StudioState::add_logic)?;
    register_event(
        hooks,
        "toggle_layer_welcome",
        state,
        StudioState::toggle_welcome_layer,
    )?;
    register_event(
        hooks,
        "toggle_layer_header",
        state,
        StudioState::toggle_header_layer,
    )?;
    register_event(
        hooks,
        "toggle_layer_welcome_copy",
        state,
        StudioState::toggle_welcome_copy_layer,
    )?;
    register_event(
        hooks,
        "toggle_layer_feature_cards",
        state,
        StudioState::toggle_feature_cards_layer,
    )?;
    register_event(
        hooks,
        "toggle_layer_welcome_lower",
        state,
        StudioState::toggle_welcome_lower_layer,
    )?;
    register_event(hooks, "select_welcome", state, StudioState::focus_main_root)?;
    register_selection_event(hooks, "select_runtime_badge", state, "preview-badge")?;
    register_selection_event(hooks, "select_hero_title", state, "hero-title")?;
    register_selection_event(
        hooks,
        "select_runtime_building_card",
        state,
        "runtime-building-card",
    )?;
    register_selection_event(hooks, "select_local_ai_card", state, "local-ai-card")?;
    register_selection_event(
        hooks,
        "select_portable_source_card",
        state,
        "portable-source-card",
    )?;
    register_event(
        hooks,
        "select_component_instance",
        state,
        StudioState::select_component_instance,
    )?;
    register_event(
        hooks,
        "open_component",
        state,
        StudioState::open_app_titlebar_component,
    )?;
    register_event(
        hooks,
        "toggle_component_root",
        state,
        StudioState::toggle_component_root,
    )?;
    register_selection_event(hooks, "select_runtime_details", state, "runtime-details")?;
    register_selection_event(hooks, "select_dogfood_note", state, "dogfood-note")?;
    register_event(
        hooks,
        "open_component_dialog",
        state,
        StudioState::open_component_dialog,
    )?;
    register_event(hooks, "insert_frame", state, StudioState::insert_frame_node)?;
    register_event(hooks, "insert_row", state, StudioState::insert_row_node)?;
    register_event(hooks, "insert_text", state, StudioState::insert_text_node)?;
    register_event(
        hooks,
        "insert_button",
        state,
        StudioState::insert_button_node,
    )?;
    register_event(
        hooks,
        "duplicate_node",
        state,
        StudioState::duplicate_selected_node,
    )?;
    register_event(
        hooks,
        "delete_node",
        state,
        StudioState::delete_selected_node,
    )?;
    register_event(
        hooks,
        "close_component_dialog",
        state,
        StudioState::close_component_dialog,
    )?;
    register_event(
        hooks,
        "select_component_html",
        state,
        StudioState::select_component_html,
    )?;
    register_event(
        hooks,
        "select_component_gpui",
        state,
        StudioState::select_component_gpui,
    )?;
    register_event(
        hooks,
        "select_component_selection",
        state,
        StudioState::select_component_selection,
    )?;
    register_event(
        hooks,
        "select_component_blank",
        state,
        StudioState::select_component_blank,
    )?;
    register_event(
        hooks,
        "select_component_paste_html",
        state,
        StudioState::select_component_paste_html,
    )?;
    register_event(
        hooks,
        "select_component_titlebar",
        state,
        StudioState::select_component_titlebar,
    )?;
    for (slug, preset) in component_preset_bindings() {
        let preset_state = state.clone();
        hooks.register_event(
            HandlerId::new(format!("select_preset_{slug}")),
            move |_, window, _| {
                preset_state.select_component_preset(preset);
                window.refresh();
                ActionOutcome::Handled
            },
        )?;
    }
    register_event(
        hooks,
        "create_component_from_dialog",
        state,
        StudioState::create_component_from_dialog,
    )?;
    register_backend_event(hooks, "backend_html", state, AuthoringBackend::Html)?;
    register_backend_event(hooks, "backend_gpui", state, AuthoringBackend::Gpui)?;
    register_mode_event(hooks, "mode_design", state, StudioMode::Design)?;
    register_mode_event(hooks, "mode_source", state, StudioMode::Source)?;
    register_mode_event(hooks, "mode_test", state, StudioMode::Test)?;
    register_event(
        hooks,
        "mode_compare",
        state,
        StudioState::enter_annotate_mode,
    )?;
    // Toolbar: tool group (Select / Move / Annotate) + view group (Design /
    // Preview). Select and Preview reuse the plain mode setters; Move and
    // Annotate route through their tool activators.
    register_mode_event(hooks, "tool_select", state, StudioMode::Design)?;
    register_event(hooks, "tool_move", state, StudioState::enter_move_tool)?;
    register_event(
        hooks,
        "tool_annotate",
        state,
        StudioState::enter_annotate_mode,
    )?;
    register_event(hooks, "view_design", state, StudioState::enter_design_view)?;
    register_mode_event(hooks, "view_preview", state, StudioMode::Test)?;
    Ok(())
}

fn register_text_state(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    read: fn(&StudioState) -> String,
) -> Result<()> {
    let state = state.clone();
    hooks.register_state(StateBindingId::new(id), move |_, _| {
        StateValue::Text(read(&state))
    })?;
    Ok(())
}

fn register_boolean_state(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    read: fn(&StudioState) -> bool,
) -> Result<()> {
    let state = state.clone();
    hooks.register_state(StateBindingId::new(id), move |_, _| {
        StateValue::Boolean(read(&state))
    })?;
    Ok(())
}

fn register_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    action: fn(&StudioState),
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        action(&state);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_selection_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    authored_id: &'static str,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.select_project_element(authored_id);
        state.project_menu_open.set(false);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_viewport_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    preset: ViewportPreset,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.select_viewport(preset);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_zoom_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    delta: i16,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.zoom_by(delta);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_fit_event(hooks: &mut HookRegistry, id: &str, state: &Rc<StudioState>) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.fit_canvas(window);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_dock_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    tab: DockTab,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_dock_tab(tab);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_inspector_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    tab: InspectorTab,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_inspector_tab(tab);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_inspector_size_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    axis: InspectorSizeAxis,
    size: NativeSize,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_inspector_size(axis, size);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_inspector_alignment_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    axis: InspectorAlignmentAxis,
    alignment: NativeAlign,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_inspector_alignment(axis, alignment);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_inspector_layout_choice_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    choice: InspectorLayoutChoice,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_inspector_layout_choice(choice);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_constraint_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    constraint: HorizontalConstraint,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.select_horizontal_constraint(constraint);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_mode_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    mode: StudioMode,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_mode(mode);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

fn register_backend_event(
    hooks: &mut HookRegistry,
    id: &str,
    state: &Rc<StudioState>,
    backend: AuthoringBackend,
) -> Result<()> {
    let state = state.clone();
    hooks.register_event(HandlerId::new(id), move |_, window, _| {
        state.set_backend(backend);
        window.refresh();
        ActionOutcome::Handled
    })?;
    Ok(())
}

const fn studio_resource_uris() -> [&'static str; 8] {
    [
        "gpui-studio://project/manifest",
        "gpui-studio://selection",
        "gpui-studio://tasks/active",
        "gpui-studio://tasks/history",
        "gpui-studio://annotations/active",
        "gpui-studio://annotations/history",
        "gpui-studio://annotations/handoff/latest",
        "gpui-studio://theme",
    ]
}

fn studio_application_commands() -> Vec<ApplicationCommandDescriptor> {
    let component_target = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["component_id"],
        "properties": { "component_id": { "type": "string", "minLength": 1, "maxLength": 128 } }
    });
    vec![
        ApplicationCommandDescriptor {
            name: "component.graph.get".to_owned(),
            title: "Read component graph".to_owned(),
            description: "Return the canonical editable component tree and its optimistic-concurrency revision.".to_owned(),
            input_schema: component_target.clone(),
            mutating: false,
        },
        ApplicationCommandDescriptor {
            name: "component.graph.apply".to_owned(),
            title: "Apply component graph transaction".to_owned(),
            description: "Atomically apply insert, remove, move, duplicate, group, ungroup, or typed patch commands. The nested transaction must include the revision returned by component.graph.get.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["component_id", "transaction"],
                "properties": {
                    "component_id": { "type": "string", "minLength": 1, "maxLength": 128 },
                    "transaction": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["expected_revision", "actor", "commands"],
                        "properties": {
                            "expected_revision": { "type": "integer", "minimum": 1 },
                            "actor": { "type": "string", "minLength": 1, "maxLength": 128 },
                            "commands": { "type": "array", "minItems": 1, "maxItems": 256, "items": { "type": "object" } }
                        }
                    }
                }
            }),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "component.graph.undo".to_owned(),
            title: "Undo component transaction".to_owned(),
            description: "Restore the previous canonical graph as a new revision.".to_owned(),
            input_schema: component_target.clone(),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "component.graph.redo".to_owned(),
            title: "Redo component transaction".to_owned(),
            description: "Restore the next canonical graph as a new revision.".to_owned(),
            input_schema: component_target.clone(),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "component.remove".to_owned(),
            title: "Remove component".to_owned(),
            description: "Delete a component from the library and persist. Refused for the root, the last remaining component, or one still referenced by other components.".to_owned(),
            input_schema: component_target.clone(),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "component.create".to_owned(),
            title: "Create component".to_owned(),
            description: "Create and persist a blank component or complete built-in preset, then open it in a component tab.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["name"],
                "properties": {
                    "name": { "type": "string", "minLength": 1, "maxLength": 128 },
                    "preset": { "type": ["string", "null"], "enum": [null, "button", "button_group", "card", "badge", "alert", "toolbar", "avatar", "empty_state", "titlebar", "tabs", "dialog", "dropdown", "dropdown_menu", "drawer", "scrollable", "resizable", "tooltip"] }
                }
            }),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "selection.set".to_owned(),
            title: "Select project layer".to_owned(),
            description: "Select one exact live project runtime ID and synchronize the canvas, Layers tree, inspector, annotations, and MCP context.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["runtime_id"],
                "properties": { "runtime_id": { "type": "string", "minLength": 1, "maxLength": 256 } }
            }),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "selection.marquee".to_owned(),
            title: "Marquee-select live layers".to_owned(),
            description: "Use the live spatial index to select every project layer intersecting an editor-space rectangle.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["rect"],
                "properties": {
                    "rect": {
                        "type": "object",
                        "additionalProperties": false,
                        "required": ["x", "y", "width", "height"],
                        "properties": {
                            "x": { "type": "number" }, "y": { "type": "number" },
                            "width": { "type": "number", "minimum": 0 },
                            "height": { "type": "number", "minimum": 0 }
                        }
                    },
                    "additive": { "type": ["boolean", "null"] }
                }
            }),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "layout.suggest_drop".to_owned(),
            title: "Suggest layout-aware drop".to_owned(),
            description: "Resolve a pointer drop to a Flexbox, Grid, or freeform insertion using the live R-tree and visual order.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["parent", "layout", "child_ids", "pointer"],
                "properties": {
                    "parent": { "type": "string", "minLength": 1, "maxLength": 256 },
                    "layout": { "type": "object" },
                    "child_ids": { "type": "array", "maxItems": 512, "items": { "type": "string", "minLength": 1, "maxLength": 256 } },
                    "pointer": {
                        "type": "object", "additionalProperties": false,
                        "required": ["x", "y"],
                        "properties": { "x": { "type": "number" }, "y": { "type": "number" } }
                    }
                }
            }),
            mutating: false,
        },
        ApplicationCommandDescriptor {
            name: "layout.snap".to_owned(),
            title: "Snap live selection".to_owned(),
            description: "Snap a multi-selection to nearby starts, centers, ends, and grid lines and return semantic guides.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["ids", "proposed_x", "proposed_y"],
                "properties": {
                    "ids": { "type": "array", "minItems": 1, "maxItems": 256, "items": { "type": "string", "minLength": 1, "maxLength": 256 } },
                    "proposed_x": { "type": "number" },
                    "proposed_y": { "type": "number" },
                    "threshold": { "type": ["number", "null"], "minimum": 0, "maximum": 64 },
                    "grid": { "type": ["number", "null"], "exclusiveMinimum": 0, "maximum": 1024 }
                }
            }),
            mutating: false,
        },
        ApplicationCommandDescriptor {
            name: "annotation.update".to_owned(),
            title: "Update review-task status".to_owned(),
            description: "Transition one spatial annotation's review status. Agents mark work `in_progress` when they start and `done` when the requested change is complete; `archived` removes it from the queue.".to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["id", "status"],
                "properties": {
                    "id": { "type": "string", "minLength": 1, "maxLength": 256 },
                    "status": { "type": "string", "enum": ["open", "in_progress", "done", "archived"] }
                }
            }),
            mutating: true,
        },
        ApplicationCommandDescriptor {
            name: "project.parity".to_owned(),
            title: "Diagnose projection parity".to_owned(),
            description: "Generate HTML, CSS, RON, and GPUI views from one canonical graph and report stable IDs missing from either executable projection.".to_owned(),
            input_schema: component_target,
            mutating: false,
        },
    ]
}

fn decode_command_arguments<T: for<'de> Deserialize<'de>>(
    arguments: serde_json::Value,
) -> Result<T, BridgeError> {
    serde_json::from_value(arguments).map_err(|error| {
        BridgeError::new(
            ErrorCode::InvalidRequest,
            format!("application command arguments are invalid: {error}"),
        )
    })
}

fn graph_bridge_error(error: GraphError) -> BridgeError {
    let code = match error {
        GraphError::Conflict { .. } => ErrorCode::StaleRevision,
        GraphError::MissingNode(_) => ErrorCode::NotFound,
        _ => ErrorCode::InvalidRequest,
    };
    BridgeError::new(code, error.to_string())
}

fn parse_component_preset(value: &str) -> Result<ComponentPreset, BridgeError> {
    component_preset_bindings()
        .into_iter()
        .find(|(slug, _)| *slug == value)
        .map(|(_, preset)| preset)
        .ok_or_else(|| {
            BridgeError::new(
                ErrorCode::InvalidRequest,
                "component preset is not supported",
            )
        })
}

fn component_logic_transitions(
    logic: &[ComponentLogic],
    current: &BTreeMap<String, String>,
    node_id: &str,
    action: &str,
) -> Vec<(String, String)> {
    logic
        .iter()
        .filter(|logic| logic.source_node == node_id)
        .filter(|logic| logic.event == ComponentEvent::Click)
        .filter(|logic| logic.action == action)
        .filter(|logic| component_logic_guard_matches(logic.guard.as_deref(), current))
        .filter_map(|logic| {
            logic
                .target_state
                .as_ref()
                .zip(logic.value.as_ref())
                .map(|(state, value)| (state.clone(), value.clone()))
        })
        .collect()
}

fn component_node_ids(root: &NativeNode) -> Vec<String> {
    let mut ids = vec![root.id.clone()];
    for child in &root.children {
        ids.extend(component_node_ids(child));
    }
    ids
}

fn annotation_number(id: &str) -> String {
    let number = id
        .strip_prefix("comment-")
        .unwrap_or(id)
        .trim_start_matches('0');
    if number.is_empty() {
        "0".to_owned()
    } else {
        number.to_owned()
    }
}

fn authored_name(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect()
}

fn rounded_pixels(value: f32) -> String {
    format!("{:.0}", value.max(0.0))
}

fn format_optional_color(color: Option<u32>) -> String {
    color.map_or_else(|| "transparent".to_owned(), |color| format!("#{color:06x}"))
}

fn parse_optional_color(value: &str) -> Result<Option<u32>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("transparent") || value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| "Color must use #RRGGBB or transparent".to_owned())?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Color must use exactly six hexadecimal digits".to_owned());
    }
    u32::from_str_radix(hex, 16)
        .map(Some)
        .map_err(|_| "Color is outside the RGB range".to_owned())
}

fn parse_bounded_u16(value: &str, minimum: u16, maximum: u16, field: &str) -> Result<u16, String> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| format!("{field} must be between {minimum} and {maximum}"))
}

fn optional_u16_value(value: Option<u16>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn optional_i16_value(value: Option<i16>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn parse_optional_bounded_u16(
    value: &str,
    minimum: u16,
    maximum: u16,
    field: &str,
) -> Result<Option<u16>, String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    parse_bounded_u16(value, minimum, maximum, field).map(Some)
}

fn parse_bounded_i16(value: &str, minimum: i16, maximum: i16, field: &str) -> Result<i16, String> {
    value
        .trim()
        .parse::<i16>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| format!("{field} must be between {minimum} and {maximum}"))
}

fn parse_optional_bounded_i16(
    value: &str,
    minimum: i16,
    maximum: i16,
    field: &str,
) -> Result<Option<i16>, String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    parse_bounded_i16(value, minimum, maximum, field).map(Some)
}

fn is_portable_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    value.len() <= 128
        && characters
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn is_portable_type(value: &str) -> bool {
    matches!(
        value.trim(),
        "String"
            | "bool"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "f32"
            | "f64"
    )
}

fn component_preset_bindings() -> [(&'static str, ComponentPreset); 17] {
    [
        ("button", ComponentPreset::Button),
        ("button_group", ComponentPreset::ButtonGroup),
        ("card", ComponentPreset::Card),
        ("badge", ComponentPreset::Badge),
        ("alert", ComponentPreset::Alert),
        ("toolbar", ComponentPreset::Toolbar),
        ("avatar", ComponentPreset::Avatar),
        ("empty_state", ComponentPreset::EmptyState),
        ("titlebar", ComponentPreset::Titlebar),
        ("tabs", ComponentPreset::Tabs),
        ("dialog", ComponentPreset::Dialog),
        ("dropdown", ComponentPreset::Dropdown),
        ("dropdown_menu", ComponentPreset::DropdownMenu),
        ("drawer", ComponentPreset::Drawer),
        ("scrollable", ComponentPreset::Scrollable),
        ("resizable", ComponentPreset::Resizable),
        ("tooltip", ComponentPreset::Tooltip),
    ]
}

fn inspector_layout_choice_bindings() -> [(&'static str, InspectorLayoutChoice); 22] {
    use InspectorLayoutChoice::{
        Grow, HorizontalConstraint, Overflow, Position, Shrink, VerticalConstraint, Wrap,
    };
    [
        ("inspector_wrap_none", Wrap(NativeWrap::NoWrap)),
        ("inspector_wrap_wrap", Wrap(NativeWrap::Wrap)),
        ("inspector_wrap_reverse", Wrap(NativeWrap::WrapReverse)),
        ("inspector_position_flow", Position(NativePosition::Flow)),
        (
            "inspector_position_absolute",
            Position(NativePosition::Absolute),
        ),
        (
            "inspector_overflow_visible",
            Overflow(NativeOverflow::Visible),
        ),
        (
            "inspector_overflow_hidden",
            Overflow(NativeOverflow::Hidden),
        ),
        (
            "inspector_overflow_scroll",
            Overflow(NativeOverflow::Scroll),
        ),
        (
            "inspector_constraint_x_start",
            HorizontalConstraint(NativeConstraint::Start),
        ),
        (
            "inspector_constraint_x_center",
            HorizontalConstraint(NativeConstraint::Center),
        ),
        (
            "inspector_constraint_x_end",
            HorizontalConstraint(NativeConstraint::End),
        ),
        (
            "inspector_constraint_x_scale",
            HorizontalConstraint(NativeConstraint::Scale),
        ),
        (
            "inspector_constraint_x_stretch",
            HorizontalConstraint(NativeConstraint::Stretch),
        ),
        (
            "inspector_constraint_y_start",
            VerticalConstraint(NativeConstraint::Start),
        ),
        (
            "inspector_constraint_y_center",
            VerticalConstraint(NativeConstraint::Center),
        ),
        (
            "inspector_constraint_y_end",
            VerticalConstraint(NativeConstraint::End),
        ),
        (
            "inspector_constraint_y_scale",
            VerticalConstraint(NativeConstraint::Scale),
        ),
        (
            "inspector_constraint_y_stretch",
            VerticalConstraint(NativeConstraint::Stretch),
        ),
        ("inspector_grow_off", Grow(false)),
        ("inspector_grow_on", Grow(true)),
        ("inspector_shrink_off", Shrink(false)),
        ("inspector_shrink_on", Shrink(true)),
    ]
}

fn component_id(name: &str) -> String {
    let mut id = String::with_capacity(name.len());
    let mut separated = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            id.push(character.to_ascii_lowercase());
            separated = false;
        } else if !separated && !id.is_empty() {
            id.push('-');
            separated = true;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() {
        "component".to_owned()
    } else {
        id
    }
}

fn parse_component_props(source: &str) -> Vec<ComponentProp> {
    source
        .split(',')
        .filter_map(|candidate| {
            let (name, value_type) = candidate.split_once(':')?;
            let name = name.trim();
            let value_type = value_type.trim();
            let mut characters = name.chars();
            let portable = characters
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
                && characters
                    .all(|character| character.is_ascii_alphanumeric() || character == '_');
            (portable && !value_type.is_empty()).then(|| ComponentProp {
                name: name.to_owned(),
                value_type: value_type.to_owned(),
                default: None,
            })
        })
        .collect()
}

fn gpui_element_excerpt(node: &UiNode) -> String {
    let mut lines = vec!["div()".to_owned(), format!("    .id({:?})", node.id)];
    if let Some(bounds) = node.bounds {
        lines.push(format!("    .w(px({:.0}.))", bounds.width));
        lines.push(format!("    .h(px({:.0}.))", bounds.height));
    }
    if let Some(text) = &node.text {
        lines.push(format!("    .child({:?})", text.text));
    } else if let Some(label) = &node.label {
        lines.push(format!("    .child({label:?})"));
    }
    lines.push("// Projection of the same selected component node".to_owned());
    lines.join("\n")
}

fn bounded_excerpt(source: &str, maximum: usize) -> String {
    let flattened = source.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.len() <= maximum {
        return flattened;
    }
    let mut end = maximum;
    while !flattened.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &flattened[..end])
}

fn html_element_excerpt(source: &str, id: &str) -> Option<String> {
    let id_position = [format!("id=\"{id}\""), format!("id='{id}'")]
        .into_iter()
        .filter_map(|needle| source.find(&needle))
        .min()?;
    let opening_start = source[..id_position].rfind('<')?;
    let opening_end = id_position + source[id_position..].find('>')? + 1;
    let opening = source[opening_start..opening_end].trim();
    if opening.ends_with("/>") {
        return Some(opening.to_owned());
    }
    let tag = opening
        .trim_start_matches('<')
        .split(|character: char| character.is_ascii_whitespace() || character == '>')
        .next()?;
    if tag.is_empty() || tag.starts_with(['!', '?', '/']) {
        return None;
    }
    let closing = format!("</{tag}>");
    let closing_start = opening_end + source[opening_end..].find(&closing)?;
    let inner = source[opening_end..closing_start].trim();
    if inner.is_empty() {
        return Some(format!("{opening}\n{closing}"));
    }
    if inner.contains('\n') || inner.contains('<') {
        return Some(
            source[opening_start..closing_start + closing.len()]
                .trim()
                .to_owned(),
        );
    }
    Some(format!("{opening}\n  {inner}\n{closing}"))
}

fn diagnostic_summary(diagnostics: &[gpui_mcp::LiveDocumentDiagnostic]) -> String {
    diagnostics.first().map_or_else(
        || "unknown validation failure".to_owned(),
        |diagnostic| diagnostic.message.clone(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use gpui::{Context, IntoElement, Render, Styled as _, TestAppContext, Window, px, size};
    use gpui_mcp::{Automation, LiveDocumentSource, NodeAction, Role};
    use gpui_mcp_html::{ComponentRegistry, HookRegistry, LiveHtmlSession, SemanticNamespace};

    use super::{
        DragPreviewState, StudioScrollHandles, component_logic_transitions,
        component_preset_bindings, html_element_excerpt, initial_selection, parse_component_preset,
        render_studio_scroll_area, studio_application_commands,
    };
    use crate::authoring::component_logic_guard_matches;
    use crate::{AuthoringBackend, ComponentPreset, NativeComponentLibrary};

    fn drag_preview(base_index: usize, offset: i32, child_count: usize) -> DragPreviewState {
        DragPreviewState {
            parent: "root".to_owned(),
            base_index,
            offset,
            child_count,
            horizontal: false,
            is_stack: true,
            pointer_x: 0.0,
            pointer_y: 0.0,
        }
    }

    #[test]
    fn drag_preview_index_and_position_respect_container_bounds() {
        // Middle of a 4-child container, nudged up two slots.
        let preview = drag_preview(2, 1, 4);
        assert_eq!(preview.effective_index(), 3);
        assert_eq!(preview.position_label(), (4, 5));

        // Scrolling below the first slot clamps to 0 (position 1).
        let low = drag_preview(1, -10, 4);
        assert_eq!(low.effective_index(), 0);
        assert_eq!(low.position_label(), (1, 5));

        // Scrolling past the last slot clamps to child_count (append).
        let high = drag_preview(2, 99, 4);
        assert_eq!(high.effective_index(), 4);
        assert_eq!(high.position_label(), (5, 5));
    }

    #[test]
    fn component_logic_guards_match_portable_state_comparisons() {
        let state = BTreeMap::from([
            ("open".to_owned(), "false".to_owned()),
            ("split".to_owned(), "50".to_owned()),
        ]);

        assert!(component_logic_guard_matches(None, &state));
        assert!(component_logic_guard_matches(Some("open == false"), &state));
        assert!(component_logic_guard_matches(Some("open != true"), &state));
        assert!(component_logic_guard_matches(Some("split == '50'"), &state));
        assert!(!component_logic_guard_matches(Some("open == true"), &state));
        assert!(!component_logic_guard_matches(Some("unsupported"), &state));
    }

    #[test]
    fn preset_registration_matches_catalog_schema_parser_and_ui() {
        let bindings = component_preset_bindings();
        assert_eq!(bindings.map(|(_, preset)| preset), ComponentPreset::ALL);

        let commands = studio_application_commands();
        let create = commands
            .iter()
            .find(|command| command.name == "component.create");
        assert!(create.is_some(), "component.create command is missing");
        if let Some(create) = create {
            let schema_values = create.input_schema["properties"]["preset"]["enum"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>();
            let expected = bindings.iter().map(|(slug, _)| *slug).collect::<Vec<_>>();
            assert_eq!(schema_values, expected);
        }

        let html = include_str!("../ui/app.html");
        let ron = include_str!("../ui/app.bindings.ron");
        for (slug, preset) in bindings {
            assert!(matches!(parse_component_preset(slug), Ok(parsed) if parsed == preset));
            let html_slug = slug.replace('_', "-");
            assert!(
                html.contains(&format!("id=\"preset-{html_slug}\"")),
                "HTML catalog omitted {slug}"
            );
            assert!(
                ron.contains(&format!("handler: \"select_preset_{slug}\"")),
                "UI event bindings omitted {slug}"
            );
            assert!(
                ron.contains(&format!("source: \"preset_{slug}_selected\"")),
                "UI state bindings omitted {slug}"
            );
        }
        assert!(parse_component_preset("not-a-preset").is_err());
    }

    #[test]
    fn new_preset_actions_use_the_shared_guarded_transition_machinery() {
        fn defaults(component: &crate::NativeComponent) -> BTreeMap<String, String> {
            component
                .states
                .iter()
                .map(|state| (state.name.clone(), state.default.clone()))
                .collect()
        }

        fn apply(state: &mut BTreeMap<String, String>, transitions: Vec<(String, String)>) {
            state.extend(transitions);
        }

        let mut library = NativeComponentLibrary::default();
        let dropdown = library
            .create_preset_component(ComponentPreset::Dropdown, "Runtime Dropdown")
            .clone();
        let mut dropdown_state = defaults(&dropdown);
        let opened = component_logic_transitions(
            &dropdown.logic,
            &dropdown_state,
            "dropdown-trigger",
            "toggle_options",
        );
        assert_eq!(opened, vec![("open".to_owned(), "true".to_owned())]);
        apply(&mut dropdown_state, opened);
        let closed = component_logic_transitions(
            &dropdown.logic,
            &dropdown_state,
            "dropdown-trigger",
            "toggle_options",
        );
        assert_eq!(closed, vec![("open".to_owned(), "false".to_owned())]);
        apply(&mut dropdown_state, closed);
        let selected = component_logic_transitions(
            &dropdown.logic,
            &dropdown_state,
            "option-staging",
            "select_staging",
        );
        apply(&mut dropdown_state, selected);
        assert_eq!(
            dropdown_state.get("selected").map(String::as_str),
            Some("staging")
        );
        assert_eq!(
            dropdown_state.get("open").map(String::as_str),
            Some("false")
        );

        for (preset, node, action, expected) in [
            (
                ComponentPreset::ButtonGroup,
                "segment-left",
                "select_left",
                ("selected", "left"),
            ),
            (
                ComponentPreset::Drawer,
                "drawer-close",
                "close_drawer",
                ("open", "false"),
            ),
            (
                ComponentPreset::Resizable,
                "resize-handle",
                "cycle_split",
                ("split", "35"),
            ),
        ] {
            let component = library
                .create_preset_component(preset, format!("Runtime {}", preset.label()))
                .clone();
            let state = defaults(&component);
            assert_eq!(
                component_logic_transitions(&component.logic, &state, node, action),
                vec![(expected.0.to_owned(), expected.1.to_owned())]
            );
        }
    }

    struct ResponsiveStudio {
        shell: LiveHtmlSession,
    }

    #[test]
    fn selected_inline_html_is_formatted_for_the_inspector() {
        assert_eq!(
            html_element_excerpt(
                "<main><h1 id=\"hero-title\">Build GPUI live.</h1></main>",
                "hero-title"
            )
            .as_deref(),
            Some("<h1 id=\"hero-title\">\n  Build GPUI live.\n</h1>")
        );
    }

    impl Render for ResponsiveStudio {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.render(window, cx)
        }
    }

    fn source(html: &str, css: &str, bindings_ron: &str) -> LiveDocumentSource {
        LiveDocumentSource {
            html: html.to_owned(),
            css: css.to_owned(),
            bindings_ron: bindings_ron.to_owned(),
        }
    }

    fn expect_ok<T, E: std::fmt::Debug>(result: Result<T, E>, context: &str) -> Option<T> {
        assert!(result.is_ok(), "{context}: {:?}", result.as_ref().err());
        result.ok()
    }

    #[test]
    fn initial_selection_is_independent_of_source_projection() {
        let library = NativeComponentLibrary::default();
        let root = library
            .components
            .first()
            .map(|component| component.id.clone())
            .unwrap_or_default();

        let html = initial_selection(AuthoringBackend::Html, &library, 7);
        assert_eq!(html.authored_id, "root");
        assert_eq!(html.runtime_id, format!("component/{root}/root"));
        assert_eq!(html.document_revision, 7);

        let gpui = initial_selection(AuthoringBackend::Gpui, &library, 7);
        assert_eq!(gpui.authored_id, "root");
        assert_eq!(gpui.runtime_id, format!("component/{root}/root"));
        assert_eq!(gpui.document_revision, 7);
        assert_eq!(html, gpui);
    }

    #[gpui::test]
    fn studio_scroll_surfaces_publish_unique_semantics_and_preserve_invalid_children(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_mcp_html::init);
        let automation = Automation::isolated();
        let scroll_automation = automation.clone();
        let handles = Rc::new(StudioScrollHandles::new());
        let scroll_handles = handles.clone();
        let mut components = ComponentRegistry::new();
        let Some(()) = expect_ok(
            components.register("studio-scroll-area", move |node, children, _, _| {
                render_studio_scroll_area(&scroll_handles, &scroll_automation, node, children)
            }),
            "Studio scroll area should register",
        ) else {
            return;
        };
        let Some(shell) = expect_ok(
            LiveHtmlSession::compile(
                source(
                    r#"<html><body><main id="scroll-test-root">
                        <studio-scroll-area surface="layers"><div id="layers-child">Layers</div></studio-scroll-area>
                        <studio-scroll-area surface="files"><div id="files-child">Files</div></studio-scroll-area>
                        <studio-scroll-area surface="inspector"><div id="inspector-child">Inspector</div></studio-scroll-area>
                        <studio-scroll-area surface="states"><div id="states-child">States</div></studio-scroll-area>
                        <studio-scroll-area surface="unknown"><div id="invalid-scroll-child">Preserved</div></studio-scroll-area>
                    </main></body></html>"#,
                    "html, body, #scroll-test-root { width: 100%; height: 100%; } #scroll-test-root { display: flex; } studio-scroll-area { width: 100px; height: 100px; display: flex; }",
                    "(version: 1, bindings: [])",
                ),
                automation.clone(),
                HookRegistry::new(),
            ),
            "scroll surface fixture should compile",
        )
        .map(|session| session.with_components(components)) else {
            return;
        };
        let (view, visual) = cx.add_window_view(|_, _| ResponsiveStudio { shell });
        view.update(visual, |_, cx| cx.notify());
        visual.run_until_parked();

        let tree = automation.snapshot();
        for id in [
            "layer-tree-scroll",
            "project-files-scroll",
            "inspector-panel-scroll",
            "state-panel-scroll",
        ] {
            let node = tree.nodes.get(id);
            assert!(node.is_some(), "missing semantic scroll surface {id}");
            let Some(node) = node else {
                continue;
            };
            assert_eq!(node.role, Role::ScrollArea);
            assert!(node.actions.contains(&NodeAction::Scroll));
        }
        assert!(tree.nodes.contains_key("invalid-scroll-child"));
        assert!(automation.logs(32, None).iter().any(|entry| {
            entry.level == "debug"
                && entry.message.contains("invalid surface \"unknown\"")
                && entry.message.contains("preserving authored children")
        }));
    }

    #[gpui::test]
    fn real_studio_and_embedded_canvas_flex_with_window_height(cx: &mut TestAppContext) {
        cx.update(gpui_mcp_html::init);
        let automation = Automation::isolated();
        let Some(namespace) = expect_ok(
            SemanticNamespace::new("project-canvas"),
            "project namespace should validate",
        ) else {
            return;
        };
        let Some(project) = expect_ok(
            LiveHtmlSession::compile(
                source(
                    include_str!("../examples/welcome/ui/app.html"),
                    include_str!("../examples/welcome/ui/app.css"),
                    include_str!("../examples/welcome/ui/app.bindings.ron"),
                ),
                automation.clone(),
                HookRegistry::new(),
            ),
            "welcome project should compile",
        )
        .map(|session| session.embedded(namespace)) else {
            return;
        };
        let mut components = ComponentRegistry::new();
        let scroll_automation = automation.clone();
        let scroll_handles = Rc::new(StudioScrollHandles::new());
        let Some(()) = expect_ok(
            components.register("studio-scroll-area", move |node, children, _, _| {
                render_studio_scroll_area(&scroll_handles, &scroll_automation, node, children)
            }),
            "Studio scroll area should register",
        ) else {
            return;
        };
        let Some(()) = expect_ok(
            components.register("studio-canvas", move |_, _, window, cx| {
                project.render(window, cx)
            }),
            "Studio canvas should register",
        ) else {
            return;
        };
        let Some(()) = expect_ok(
            components.register("studio-app-frame", |_, children, window, _| {
                let size = window.viewport_size();
                let (available_width, available_height) =
                    crate::available_canvas(size.width.into(), size.height.into(), false);
                let layout =
                    crate::CanvasSettings::default().layout(available_width, available_height);
                children
                    .into_iter()
                    .fold(
                        gpui::div()
                            .flex()
                            .flex_col()
                            .w(px(layout.frame_width))
                            .h(px(layout.frame_height)),
                        gpui::ParentElement::child,
                    )
                    .into_any_element()
            }),
            "Studio frame should register",
        ) else {
            return;
        };
        let Some(()) = expect_ok(
            components.register("studio-bottom-dock", |_, children, window, _| {
                let size = window.viewport_size();
                let width: f32 = size.width.into();
                let height: f32 = size.height.into();
                let dock_height = if height <= 720.0 {
                    150.0
                } else if width <= 1_120.0 {
                    170.0
                } else {
                    206.0
                };
                children
                    .into_iter()
                    .fold(gpui::div().h(px(dock_height)), gpui::ParentElement::child)
                    .into_any_element()
            }),
            "Studio dock should register",
        ) else {
            return;
        };
        let Some(shell) = expect_ok(
            LiveHtmlSession::compile(
                source(
                    include_str!("../ui/app.html"),
                    include_str!("../ui/app.css"),
                    "(version: 1, bindings: [])",
                ),
                automation.clone(),
                HookRegistry::new(),
            ),
            "Studio shell should compile",
        )
        .map(|session| session.with_components(components)) else {
            return;
        };
        let shell_document = shell.document();
        assert!(
            shell_document.diagnostics.is_empty(),
            "Studio shell should not rely on ignored HTML/CSS features: {:#?}",
            shell_document.diagnostics
        );
        let (view, visual) = cx.add_window_view(|_, _| ResponsiveStudio { shell });

        for (width, height) in [
            (1_440.0, 900.0),
            (1_280.0, 650.0),
            (1_024.0, 720.0),
            (860.0, 520.0),
            (1_280.0, 420.0),
            (1_280.0, 760.0),
        ] {
            visual.simulate_resize(size(px(width), px(height)));
            view.update(visual, |_, cx| cx.notify());
            visual.run_until_parked();
            let tree = automation.snapshot();
            let bounds = |id: &str| tree.nodes.get(id).and_then(|node| node.bounds);

            assert!(!tree.nodes.contains_key("layer-html-runtime"));
            for id in [
                "runtime-building-card",
                "local-ai-card",
                "portable-source-card",
            ] {
                assert_eq!(
                    tree.nodes[&format!("project-canvas--{id}")]
                        .parent
                        .as_deref(),
                    Some("project-canvas--feature-grid")
                );
            }

            assert_eq!(bounds("studio-shell").map(|rect| rect.height), Some(height));
            assert_eq!(bounds("studio-shell").map(|rect| rect.width), Some(width));
            assert_eq!(
                bounds("studio-workspace").map(|rect| rect.height),
                Some(height - 46.0)
            );
            assert_eq!(
                bounds("studio-workspace").map(|rect| rect.width),
                Some(width)
            );
            assert_eq!(
                bounds("project-canvas--html-root").map(|rect| rect.height),
                bounds("project-canvas").map(|rect| rect.height)
            );
            let project_canvas = bounds("project-canvas");
            assert!(
                project_canvas.is_some_and(|rect| {
                    rect.width > 100.0
                        && rect.height > 24.0
                        && rect.x + rect.width <= width
                        && rect.y + rect.height <= height
                }),
                "project canvas {project_canvas:?} must remain within {width}x{height}"
            );
            assert!(bounds("center-column").is_some_and(|rect| rect.width > 200.0));
            assert!(bounds("inspector").is_some_and(|rect| rect.x + rect.width <= width));
            assert_eq!(
                bounds("project-canvas--welcome-canvas").map(|rect| rect.height),
                bounds("project-canvas--html-root").map(|rect| rect.height)
            );
        }
    }
}
