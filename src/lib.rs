//! Offline-first native Studio for revisioned HTML-backed GPUI projects.
//!
//! The checked-in `ui/app.html`, `ui/app.css`, and `ui/app.bindings.ron`
//! bundle remains authoritative. Studio previews complete candidate bundles in
//! memory and persists them only through an explicit, conflict-checked save.

mod annotations;
mod app;
mod authoring;
mod documents;
mod editor;
mod floating;
mod graph;
mod handoff;
mod history;
mod layers;
mod logic_layout;
mod output;
mod project;
mod resizable;
mod settings;
mod spatial;
mod theme;

pub use annotations::{
    Annotation, AnnotationCounts, AnnotationError, AnnotationStatus, AnnotationStore,
    ElementSelection, NormalizedAnchor, ResolvedAnnotation,
};
pub use app::{StudioConfig, run};
pub use authoring::{
    AuthoringBackend, AuthoringProjection, ComponentActionHandler, ComponentDefinition,
    ComponentDocumentError, ComponentEvent, ComponentLibrary, ComponentLogic, ComponentNode,
    ComponentNodeDrag, ComponentPointerGesture, ComponentPointerHandler, ComponentPreset,
    ComponentProp, ComponentSlot, ComponentState, ComponentVariant, ComponentVariantOverride,
    DesignToken, DesignTokenKind, DropPreviewSpec, NativeAlign, NativeAppearance, NativeComponent,
    NativeComponentError, NativeComponentLibrary, NativeConstraint, NativeEdges, NativeLayout,
    NativeNode, NativeNodeKind, NativeOffsets, NativeOverflow, NativePosition, NativeSemanticRole,
    NativeSemanticState, NativeSize, NativeTypography, NativeWrap,
};
pub use editor::{
    CanvasSettings, DockTab, ElementTransform, HorizontalConstraint, InspectorTab,
    OutputDecorations, PreviewLayout, ViewportPreset, available_canvas,
    available_canvas_with_rails,
};
pub use floating::{FloatingPlacement, FloatingSide, MenuAim, SafeCorridor, SurfaceRect};
pub use graph::{
    ComponentGraph, ComponentTransaction, GraphChange, GraphCommand, GraphError, NodePatch,
    OrderKey,
};
pub use handoff::{AnnotationHandoff, AnnotationHandoffError, AnnotationHandoffStore};
pub use history::{ChangeOrigin, RevisionHistory};
pub use layers::{LayerKind, LayerRow, LayerTree};
pub use logic_layout::{
    LogicEdge, LogicLayoutOptions, LogicNode, LogicPlacement, layout_logic_graph,
};
pub use project::{ProjectStore, ProjectStoreError};
pub use settings::{StudioMode, WorkspaceSettings, WorkspaceSettingsError};
pub use spatial::{
    Axis, DropPlacement, Guide, GuideKind, LayoutMode, PlacementEngine, SelectionBounds, SnapResult,
};
pub use theme::{
    AvailableTheme, ResolvedTheme, ThemeCatalog, ThemeError, ThemeLocation, ThemeMode,
    ThemeSelection, ThemeTokens,
};
