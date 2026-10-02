//! The infinite canvas: camera, hit testing, direct manipulation, overlays.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::{
    AnyElement, BorderStyle, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Entity,
    Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas,
    div, fill, outline, point, prelude::*, px, quad, size,
};

use super::paint::{LayoutMap, Painter};
use super::{
    AlignBottom, AlignHCenter, AlignLeft, AlignRight, AlignTop, AlignVCenter, ArrowTool,
    BringForward, CANVAS_CONTEXT, CommentTool, ConnectorTool, CopySelection, CreateComponent,
    CutSelection, DeleteSelection, DetachInstance, DistributeHorizontal, DistributeVertical,
    DuplicateSelection, EllipseTool, EnterSelection, EscapeSelection, ExportSelection, FrameTool,
    GroupSelection, HandTool, LineTool, NudgeDown, NudgeDownBig, NudgeLeft, NudgeLeftBig,
    NudgeRight, NudgeRightBig, NudgeUp, NudgeUpBig, PasteClipboard, PencilTool, PlaceImage,
    RectangleTool, RenameSelection, RightTab, SelectAllSiblings, SelectTool, SendBackward,
    StartPresenting, Studio, TextTool, ToggleAutoLayout, ToggleHidden, ToggleLocked,
    UngroupSelection, ZoomToSelection,
};
use crate::editor::{ImagePlacement, InsertTarget, Tool};
use crate::geometry::{Align, Axis, Edges, Guide, Rect, guides, snap};
use crate::model::connection::{Anchor, distance_to_polyline, midpoint, route};
use crate::model::html::escape_text;
use crate::model::style::{Display, Position, fmt_num};
use crate::model::{ArrowHeads, Endpoint, NodeId, NodeKind};
use crate::presets;

const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 16.0;
const DRAG_THRESHOLD: f32 = 3.0;
const HANDLE: f32 = 8.0;
/// Snapping reach in screen pixels.
const SNAP_DISTANCE: f32 = 5.0;

/// Canvas camera: screen offset of the document origin and scale.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Camera {
    /// Canvas-local pixels of the document origin.
    pub pan: Point<Pixels>,
    /// Scale factor.
    pub zoom: f32,
}

/// A resize handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    const ALL: [Self; 8] = [
        Self::NW,
        Self::N,
        Self::NE,
        Self::E,
        Self::SE,
        Self::S,
        Self::SW,
        Self::W,
    ];

    /// Unit position on the rectangle.
    fn anchor(self) -> (f32, f32) {
        match self {
            Self::NW => (0.0, 0.0),
            Self::N => (0.5, 0.0),
            Self::NE => (1.0, 0.0),
            Self::E => (1.0, 0.5),
            Self::SE => (1.0, 1.0),
            Self::S => (0.5, 1.0),
            Self::SW => (0.0, 1.0),
            Self::W => (0.0, 0.5),
        }
    }

    fn cursor(self) -> CursorStyle {
        match self {
            Self::N | Self::S => CursorStyle::ResizeUpDown,
            Self::E | Self::W => CursorStyle::ResizeLeftRight,
            Self::NE | Self::SW => CursorStyle::ResizeUpRightDownLeft,
            Self::NW | Self::SE => CursorStyle::ResizeUpLeftDownRight,
        }
    }
}

/// What a press landed on.
#[derive(Clone, Copy, Debug)]
enum Press {
    Empty,
    ArtboardBackground(NodeId),
    Node(NodeId),
}

/// A rectangle in document pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DocRect {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

#[derive(Clone, Debug)]
struct DropTarget {
    parent: NodeId,
    index: usize,
    line: Bounds<Pixels>,
}

#[derive(Clone, Debug)]
enum Drag {
    Press {
        start: Point<Pixels>,
        press: Press,
    },
    Pan {
        last: Point<Pixels>,
    },
    Marquee {
        start: Point<Pixels>,
        current: Point<Pixels>,
        artboard: Option<NodeId>,
        base: Vec<NodeId>,
    },
    MoveArtboards {
        start: Point<Pixels>,
        origins: Vec<(NodeId, f32, f32)>,
        rect: Option<Rect>,
    },
    MoveAbsolute {
        start: Point<Pixels>,
        origins: Vec<(NodeId, f32, f32)>,
        rect: Option<Rect>,
    },
    Reorder {
        id: NodeId,
        target: Option<DropTarget>,
    },
    Resize {
        id: NodeId,
        handle: Handle,
        start: Point<Pixels>,
        rect: DocRect,
        insets: (f32, f32),
    },
    Draw {
        tool: Tool,
        start: Point<Pixels>,
        current: Point<Pixels>,
        parent: Option<NodeId>,
    },
    /// Pencil, line, or arrow; points in document coordinates.
    Stroke {
        tool: Tool,
        points: Vec<(f32, f32)>,
        parent: Option<NodeId>,
    },
    Connect {
        start: Point<Pixels>,
        current: Point<Pixels>,
        from: Endpoint,
    },
}

/// Canvas UI state.
pub(crate) struct CanvasState {
    /// Camera.
    pub camera: Camera,
    /// Element bounds from the most recent frame (window coordinates).
    pub layout: LayoutMap,
    /// Canvas element bounds (window coordinates).
    pub bounds: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    hover: Option<NodeId>,
    needs_fit: bool,
    space_held: bool,
    gesture: u64,
    /// What the last frame showed; a change schedules one follow-up frame so
    /// overlays built from last frame's bounds catch up.
    rendered: Option<(u64, f32, Point<Pixels>, Vec<NodeId>)>,
    text_edit: Option<(NodeId, Entity<InputState>, gpui_kit::Subscription)>,
    /// Smart guides for the current gesture (document pixels).
    guides: Vec<Guide>,
}

impl CanvasState {
    /// Fresh canvas that fits the page on first layout.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            camera: Camera {
                pan: point(px(80.0), px(80.0)),
                zoom: 0.5,
            },
            layout: LayoutMap::default(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            hover: None,
            needs_fit: true,
            space_held: false,
            gesture: 0,
            rendered: None,
            text_edit: None,
            guides: Vec::new(),
        }
    }

    /// Restore a saved camera instead of fitting the page.
    pub(crate) fn restore(&mut self, zoom: f32, pan: Point<Pixels>) {
        if zoom.is_finite() && zoom > 0.0 {
            self.camera = Camera {
                pan,
                zoom: zoom.clamp(MIN_ZOOM, MAX_ZOOM),
            };
            self.needs_fit = false;
        }
    }

    /// Fit the page into view on the next frame.
    pub(crate) fn needs_fit_page(&mut self) {
        self.needs_fit = true;
    }

    /// The text layer being edited in place.
    #[cfg(test)]
    pub(crate) fn text_edit_target(&self) -> Option<NodeId> {
        self.text_edit.as_ref().map(|(id, _, _)| *id)
    }

    /// Abort any gesture.
    pub(crate) fn cancel_drag(&mut self) {
        self.drag = None;
    }

    fn origin(&self) -> Point<Pixels> {
        self.bounds.get().origin
    }

    /// Window point → document point.
    fn to_doc(&self, p: Point<Pixels>) -> (f32, f32) {
        let local = p - self.origin() - self.camera.pan;
        (
            local.x.as_f32() / self.camera.zoom,
            local.y.as_f32() / self.camera.zoom,
        )
    }

    /// Document point → canvas-local point.
    fn doc_to_local(&self, x: f32, y: f32) -> Point<Pixels> {
        self.camera.pan + point(px(x * self.camera.zoom), px(y * self.camera.zoom))
    }

    /// A node's last laid-out bounds in document pixels.
    pub(crate) fn doc_bounds(&self, id: NodeId) -> Option<Bounds<f32>> {
        let b = *self.layout.borrow().get(&id)?;
        let (x, y) = self.to_doc(b.origin);
        Some(Bounds {
            origin: point(x, y),
            size: size(
                b.size.width.as_f32() / self.camera.zoom,
                b.size.height.as_f32() / self.camera.zoom,
            ),
        })
    }

    /// A node's last laid-out rectangle in document pixels.
    pub(crate) fn doc_rect(&self, id: NodeId) -> Option<Rect> {
        let b = self.doc_bounds(id)?;
        Some(Rect::new(
            b.origin.x,
            b.origin.y,
            b.size.width,
            b.size.height,
        ))
    }

    /// Every laid-out layer in document pixels.
    pub(crate) fn measured(&self) -> std::collections::HashMap<NodeId, Rect> {
        let ids: Vec<NodeId> = self.layout.borrow().keys().copied().collect();
        ids.into_iter()
            .filter_map(|id| Some((id, self.doc_rect(id)?)))
            .collect()
    }

    fn window_bounds(&self, id: NodeId) -> Option<Bounds<Pixels>> {
        self.layout.borrow().get(&id).copied()
    }

    fn zoom_at(&mut self, factor: f32, anchor_local: Point<Pixels>) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = new / old;
        let pan = self.camera.pan;
        self.camera.pan = point(
            anchor_local.x - (anchor_local.x - pan.x) * ratio,
            anchor_local.y - (anchor_local.y - pan.y) * ratio,
        );
        self.camera.zoom = new;
    }

    fn fit(&mut self, rect: DocRect) {
        let viewport = self.bounds.get().size;
        let (vw, vh) = (viewport.width.as_f32(), viewport.height.as_f32());
        if vw < 50.0 || vh < 50.0 || rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let margin = 64.0;
        let zoom = ((vw - margin * 2.0) / rect.w)
            .min((vh - margin * 2.0) / rect.h)
            .clamp(MIN_ZOOM, 2.0);
        self.camera.zoom = zoom;
        self.camera.pan = point(
            px((vw - rect.w * zoom) / 2.0 - rect.x * zoom),
            px((vh - rect.h * zoom) / 2.0 - rect.y * zoom),
        );
    }
}

fn union(rects: impl IntoIterator<Item = DocRect>) -> Option<DocRect> {
    let mut out: Option<(f32, f32, f32, f32)> = None;
    for r in rects {
        out = Some(match out {
            None => (r.x, r.y, r.x + r.w, r.y + r.h),
            Some((a, b, c, d)) => (a.min(r.x), b.min(r.y), c.max(r.x + r.w), d.max(r.y + r.h)),
        });
    }
    out.map(|(a, b, c, d)| DocRect {
        x: a,
        y: b,
        w: c - a,
        h: d - b,
    })
}

fn window_union(rects: impl IntoIterator<Item = Bounds<Pixels>>) -> Option<Bounds<Pixels>> {
    rects.into_iter().reduce(|a, b| a.union(&b))
}

fn rect_from_points(a: Point<Pixels>, b: Point<Pixels>) -> Bounds<Pixels> {
    Bounds::from_corners(
        point(a.x.min(b.x), a.y.min(b.y)),
        point(a.x.max(b.x), a.y.max(b.y)),
    )
}

fn distance(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    let d = a - b;
    (d.x.as_f32().powi(2) + d.y.as_f32().powi(2)).sqrt()
}

impl Studio {
    /// Layer under the pointer on the canvas.
    pub(super) fn canvas_hover(&self) -> Option<NodeId> {
        self.canvas.hover
    }

    // ---- geometry ----------------------------------------------------------------

    /// Artboard rectangle from the document (no layout needed).
    fn artboard_rect(&self, root: NodeId) -> Option<DocRect> {
        let artboard = self.editor.doc.artboard_of(root)?;
        let computed = self.editor.doc.computed(self.editor.doc.get(root)?);
        let laid_out = self.canvas.doc_bounds(root);
        Some(DocRect {
            x: artboard.x,
            y: artboard.y,
            w: computed
                .width
                .px()
                .or(laid_out.map(|b| b.size.width))
                .unwrap_or(400.0),
            h: computed
                .height
                .px()
                .or(laid_out.map(|b| b.size.height))
                .unwrap_or(300.0),
        })
    }

    fn page_artboards(&self) -> Vec<NodeId> {
        self.editor
            .doc
            .pages
            .get(self.editor.page)
            .map(|p| p.artboards.iter().map(|a| a.root).collect())
            .unwrap_or_default()
    }

    /// Path from artboard root to the deepest element under a window point.
    fn hit_path(&self, p: Point<Pixels>, exclude: Option<NodeId>) -> Vec<NodeId> {
        for root in self.page_artboards().into_iter().rev() {
            if self
                .canvas
                .window_bounds(root)
                .is_some_and(|b| b.contains(&p))
            {
                let mut path = vec![root];
                self.descend(root, p, exclude, &mut path);
                return path;
            }
        }
        Vec::new()
    }

    fn descend(
        &self,
        id: NodeId,
        p: Point<Pixels>,
        exclude: Option<NodeId>,
        path: &mut Vec<NodeId>,
    ) {
        let doc = &self.editor.doc;
        if doc.is_text_layer(id) {
            return;
        }
        for child in doc.children(id).iter().rev() {
            if Some(*child) == exclude {
                continue;
            }
            let Some(node) = doc.get(*child) else {
                continue;
            };
            if node.hidden || node.is_text() {
                continue;
            }
            let mut sub = Vec::new();
            self.descend(*child, p, exclude, &mut sub);
            let inside = self
                .canvas
                .window_bounds(*child)
                .is_some_and(|b| b.contains(&p));
            if inside || !sub.is_empty() {
                path.push(*child);
                path.extend(sub);
                return;
            }
        }
    }

    /// Paper/Figma click target: the deepest node whose parent is an artboard
    /// or an ancestor of the current selection; Cmd/Ctrl picks the deepest.
    fn choose_target(&self, path: &[NodeId], modifiers: Modifiers) -> Option<NodeId> {
        let doc = &self.editor.doc;
        let pick = if modifiers.secondary() {
            path.last().copied()
        } else {
            let mut entered: BTreeSet<NodeId> = path.first().copied().into_iter().collect();
            for selected in &self.editor.selection {
                entered.extend(doc.ancestors(*selected));
            }
            path.iter()
                .skip(1)
                .rev()
                .find(|id| doc.parent(**id).is_some_and(|p| entered.contains(&p)))
                .copied()
                .or_else(|| path.get(1).copied())
                .or_else(|| path.first().copied())
        }?;
        // Locked layers pass clicks to their nearest unlocked ancestor.
        let mut pick = pick;
        while doc.get(pick).is_some_and(|n| n.locked) {
            pick = doc.parent(pick)?;
        }
        Some(pick)
    }

    fn is_container(&self, id: NodeId) -> bool {
        let doc = &self.editor.doc;
        doc.get(id).is_some_and(|node| {
            matches!(node.kind, NodeKind::Element { .. })
                && !doc.is_text_layer(id)
                && !matches!(
                    node.tag(),
                    "img" | "input" | "hr" | "br" | "textarea" | "select"
                )
        })
    }

    fn label_hit(&self, p: Point<Pixels>) -> Option<NodeId> {
        let origin = self.canvas.origin();
        for root in self.page_artboards().into_iter().rev() {
            let rect = self.artboard_rect(root)?;
            let local = self.canvas.doc_to_local(rect.x, rect.y);
            let name_width = (self.editor.doc.display_name(root).chars().count() as f32 * 6.5
                + 8.0)
                .min(rect.w * self.canvas.camera.zoom)
                .max(40.0);
            let label = Bounds {
                origin: origin + local - point(px(0.0), px(20.0)),
                size: size(px(name_width), px(18.0)),
            };
            if label.contains(&p) {
                return Some(root);
            }
        }
        None
    }

    fn handle_hit(&self, p: Point<Pixels>) -> Option<(NodeId, Handle)> {
        let [id] = self.editor.selection.as_slice() else {
            return None;
        };
        let b = self.canvas.window_bounds(*id)?;
        Handle::ALL.into_iter().find_map(|handle| {
            let (ax, ay) = handle.anchor();
            let c = point(
                b.origin.x + b.size.width * ax,
                b.origin.y + b.size.height * ay,
            );
            let r = Bounds {
                origin: c - point(px(HANDLE), px(HANDLE)),
                size: size(px(HANDLE * 2.0), px(HANDLE * 2.0)),
            };
            r.contains(&p).then_some((*id, handle))
        })
    }

    // ---- mouse ---------------------------------------------------------------------

    fn canvas_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.canvas_focus.focus(window, cx);
        self.commit_text_edit(cx);
        let p = event.position;
        self.canvas.gesture += 1;
        if self.comment_draft.is_some() {
            self.comment_draft = None;
        }
        if self.tool == Tool::Hand || self.canvas.space_held {
            self.canvas.drag = Some(Drag::Pan { last: p });
            cx.notify();
            return;
        }
        match self.tool {
            Tool::Comment => {
                let path = self.hit_path(p, None);
                if let Some(id) = path.last().copied()
                    && let Some(b) = self.canvas.window_bounds(id)
                {
                    let ax = ((p.x - b.origin.x).as_f32() / b.size.width.as_f32().max(1.0))
                        .clamp(0.0, 1.0);
                    let ay = ((p.y - b.origin.y).as_f32() / b.size.height.as_f32().max(1.0))
                        .clamp(0.0, 1.0);
                    self.open_comment_draft(id, (ax, ay), window, cx);
                }
            }
            tool if tool.draws_box() => {
                let path = self.hit_path(p, None);
                let parent = path.iter().rev().copied().find(|id| self.is_container(*id));
                self.canvas.drag = Some(Drag::Draw {
                    tool,
                    start: p,
                    current: p,
                    parent,
                });
            }
            Tool::Pencil | Tool::Line | Tool::Arrow => {
                let path = self.hit_path(p, None);
                let parent = path.iter().rev().copied().find(|id| self.is_container(*id));
                let start = self.canvas.to_doc(p);
                self.canvas.drag = Some(Drag::Stroke {
                    tool: self.tool,
                    points: vec![start, start],
                    parent,
                });
            }
            Tool::Connector => {
                let from = self.endpoint_at(p, event.modifiers);
                self.canvas.drag = Some(Drag::Connect {
                    start: p,
                    current: p,
                    from,
                });
            }
            Tool::Select => self.select_down(event, window, cx),
            _ => {}
        }
        cx.notify();
    }

    fn select_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let p = event.position;
        let shift = event.modifiers.shift;
        if self.handle_hit(p).is_none()
            && let Some(id) = self.connection_hit(p)
        {
            self.selected_connection = Some(id);
            self.editor.select([]);
            self.right_tab = super::RightTab::Design;
            self.inspector.invalidate();
            return;
        }
        self.selected_connection = None;
        if let Some((id, handle)) = self.handle_hit(p)
            && let Some(rect) = self.canvas.doc_bounds(id)
        {
            let computed = self
                .editor
                .doc
                .get(id)
                .map(|n| self.editor.doc.computed(n))
                .unwrap_or_default();
            let insets = (
                computed.inset[3].px().unwrap_or(0.0),
                computed.inset[0].px().unwrap_or(0.0),
            );
            self.canvas.drag = Some(Drag::Resize {
                id,
                handle,
                start: p,
                rect: DocRect {
                    x: rect.origin.x,
                    y: rect.origin.y,
                    w: rect.size.width,
                    h: rect.size.height,
                },
                insets,
            });
            return;
        }
        if let Some(root) = self.label_hit(p) {
            if shift {
                self.editor.toggle_selected(root);
            } else if !self.editor.selection.contains(&root) {
                self.editor.select([root]);
            }
            self.canvas.drag = Some(Drag::Press {
                start: p,
                press: Press::Node(root),
            });
            return;
        }
        let path = self.hit_path(p, None);
        match path.as_slice() {
            [] => {
                self.canvas.drag = Some(Drag::Press {
                    start: p,
                    press: Press::Empty,
                });
            }
            [root] => {
                if event.click_count >= 2 {
                    self.editor.select([*root]);
                    return;
                }
                self.canvas.drag = Some(Drag::Press {
                    start: p,
                    press: Press::ArtboardBackground(*root),
                });
            }
            _ => {
                if event.click_count >= 2 {
                    self.double_click(&path, window, cx);
                    return;
                }
                let Some(target) = self.choose_target(&path, event.modifiers) else {
                    return;
                };
                if shift {
                    self.editor.toggle_selected(target);
                } else if !self.editor.selection.contains(&target) {
                    self.editor.select([target]);
                }
                self.canvas.drag = Some(Drag::Press {
                    start: p,
                    press: Press::Node(target),
                });
            }
        }
    }

    fn double_click(&mut self, path: &[NodeId], window: &mut Window, cx: &mut Context<Self>) {
        // Drill one level below the current selection along the hit path.
        let next = self
            .editor
            .primary()
            .and_then(|selected| path.iter().position(|id| *id == selected))
            .and_then(|index| path.get(index + 1).copied());
        let target = next.or_else(|| self.choose_target(path, Modifiers::default()));
        let Some(target) = target else { return };
        // Double-clicking text always edits it, like Paper and Figma.
        let text = path.iter().copied().find(|id| {
            self.editor.doc.is_text_layer(*id) && self.editor.doc.is_ancestor_or_self(target, *id)
        });
        match text {
            Some(text) if !self.editor.doc.get(text).is_some_and(|n| n.locked) => {
                self.start_text_edit(text, window, cx);
            }
            _ => self.editor.select([target]),
        }
    }

    fn canvas_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.canvas.drag.is_some() {
            self.drag_move(event.position, event.modifiers, window, cx);
            return;
        }
        let hover = match self.tool {
            Tool::Select | Tool::Comment => {
                let path = self.hit_path(event.position, None);
                if self.tool == Tool::Comment {
                    path.last().copied()
                } else {
                    self.choose_target(&path, event.modifiers)
                }
            }
            Tool::Connector => {
                let path = self.hit_path(event.position, None);
                self.choose_target(&path, event.modifiers)
            }
            Tool::Hand => None,
            _ => self
                .hit_path(event.position, None)
                .into_iter()
                .rev()
                .find(|id| self.is_container(*id)),
        };
        if hover != self.canvas.hover {
            self.canvas.hover = hover;
            cx.notify();
        }
    }

    fn drag_move(
        &mut self,
        p: Point<Pixels>,
        modifiers: Modifiers,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.canvas.drag.clone() else {
            return;
        };
        let zoom = self.canvas.camera.zoom;
        let key = format!("gesture-{}", self.canvas.gesture);
        match drag {
            Drag::Press { start, press } => {
                if distance(start, p) < DRAG_THRESHOLD {
                    return;
                }
                self.canvas.drag = Some(self.begin_drag(start, press));
                self.drag_move(p, modifiers, _window, cx);
                return;
            }
            Drag::Pan { last } => {
                self.canvas.camera.pan += p - last;
                self.canvas.drag = Some(Drag::Pan { last: p });
            }
            Drag::Marquee {
                start,
                artboard,
                base,
                ..
            } => {
                self.canvas.drag = Some(Drag::Marquee {
                    start,
                    current: p,
                    artboard,
                    base: base.clone(),
                });
                let rect = rect_from_points(start, p);
                let candidates: Vec<NodeId> = match artboard {
                    Some(root) => self.editor.doc.children(root).to_vec(),
                    None => self.page_artboards(),
                };
                let mut picked = base;
                for id in candidates {
                    if self
                        .editor
                        .doc
                        .get(id)
                        .is_some_and(|n| n.hidden || n.locked || n.is_text())
                    {
                        continue;
                    }
                    if self
                        .canvas
                        .window_bounds(id)
                        .is_some_and(|b| b.intersects(&rect))
                        && !picked.contains(&id)
                    {
                        picked.push(id);
                    }
                }
                self.editor.select(picked);
                self.inspector.invalidate();
            }
            Drag::MoveArtboards {
                start,
                origins,
                rect,
            } => {
                let ids: Vec<NodeId> = origins.iter().map(|o| o.0).collect();
                let (dx, dy) = self.snapped_offset(start, p, rect, &ids, modifiers);
                for (id, x, y) in &origins {
                    let _ = self.editor.move_artboard(*id, x + dx, y + dy, Some(&key));
                }
                self.inspector.invalidate();
            }
            Drag::MoveAbsolute {
                start,
                origins,
                rect,
            } => {
                let ids: Vec<NodeId> = origins.iter().map(|o| o.0).collect();
                let (dx, dy) = self.snapped_offset(start, p, rect, &ids, modifiers);
                for (id, left, top) in &origins {
                    let _ = self.editor.set_styles(
                        &[*id],
                        &[
                            (
                                "left".into(),
                                Some(format!("{}px", fmt_num((left + dx).round()))),
                            ),
                            (
                                "top".into(),
                                Some(format!("{}px", fmt_num((top + dy).round()))),
                            ),
                        ],
                        Some(&key),
                    );
                }
                self.inspector.invalidate();
            }
            Drag::Reorder { id, .. } => {
                let target = self.drop_target(id, p);
                self.canvas.drag = Some(Drag::Reorder { id, target });
            }
            Drag::Resize {
                id,
                handle,
                start,
                rect,
                insets,
            } => {
                self.apply_resize(
                    id,
                    handle,
                    start,
                    rect,
                    insets,
                    p,
                    modifiers.shift,
                    !modifiers.secondary(),
                    &key,
                );
                self.inspector.invalidate();
            }
            Drag::Draw {
                tool,
                start,
                parent,
                ..
            } => {
                self.canvas.drag = Some(Drag::Draw {
                    tool,
                    start,
                    current: p,
                    parent,
                });
            }
            Drag::Stroke {
                tool,
                mut points,
                parent,
            } => {
                let mut point = self.canvas.to_doc(p);
                if tool == Tool::Pencil {
                    let last = points.last().copied().unwrap_or(point);
                    let step = 1.5 / zoom;
                    if ((point.0 - last.0).powi(2) + (point.1 - last.1).powi(2)).sqrt() >= step {
                        points.push(point);
                    }
                } else {
                    let start = points[0];
                    if modifiers.shift {
                        // Snap to 45° increments.
                        let (dx, dy) = (point.0 - start.0, point.1 - start.1);
                        let angle = (dy.atan2(dx) / std::f32::consts::FRAC_PI_4).round()
                            * std::f32::consts::FRAC_PI_4;
                        let length = (dx * dx + dy * dy).sqrt();
                        point = (
                            start.0 + length * angle.cos(),
                            start.1 + length * angle.sin(),
                        );
                    }
                    points = vec![start, point];
                }
                self.canvas.drag = Some(Drag::Stroke {
                    tool,
                    points,
                    parent,
                });
            }
            Drag::Connect { start, from, .. } => {
                let path = self.hit_path(p, None);
                self.canvas.hover = self.choose_target(&path, modifiers);
                self.canvas.drag = Some(Drag::Connect {
                    start,
                    current: p,
                    from,
                });
            }
        }
        cx.notify();
    }

    /// What a connector end attaches to at a window point.
    fn endpoint_at(&self, p: Point<Pixels>, modifiers: Modifiers) -> Endpoint {
        let path = self.hit_path(p, None);
        match self.choose_target(&path, modifiers) {
            Some(id) => Endpoint::Node(id),
            None => {
                let (x, y) = self.canvas.to_doc(p);
                Endpoint::Point {
                    x: x.round(),
                    y: y.round(),
                }
            }
        }
    }

    fn anchor(&self, endpoint: Endpoint) -> Option<Anchor> {
        match endpoint {
            Endpoint::Point { x, y } => Some(Anchor::Point(x, y)),
            Endpoint::Node(id) => {
                let b = self.canvas.doc_bounds(id)?;
                Some(Anchor::Rect(
                    b.origin.x,
                    b.origin.y,
                    b.size.width,
                    b.size.height,
                ))
            }
        }
    }

    /// Every connector on the current page as a window-space polyline.
    pub(crate) fn connection_paths(&self) -> Vec<(u64, Vec<Point<Pixels>>)> {
        let Some(page) = self.editor.doc.pages.get(self.editor.page) else {
            return Vec::new();
        };
        let origin = self.canvas.origin();
        page.connections
            .iter()
            .filter_map(|c| {
                let line = route(self.anchor(c.from)?, self.anchor(c.to)?, c.style);
                let points = line
                    .into_iter()
                    .map(|(x, y)| origin + self.canvas.doc_to_local(x, y))
                    .collect();
                Some((c.id, points))
            })
            .collect()
    }

    /// Prototype flows on the current page as window-space polylines
    /// (source layer → target artboard). Back links have no arrow.
    fn flow_paths(&self) -> Vec<Vec<Point<Pixels>>> {
        let origin = self.canvas.origin();
        self.editor
            .doc
            .page_links(self.editor.page)
            .into_iter()
            .filter_map(|link| {
                let crate::model::prototype::LinkTarget::Artboard(target) = link.target else {
                    return None;
                };
                let from = self.anchor(Endpoint::Node(link.source))?;
                let to = self.anchor(Endpoint::Node(target))?;
                let line = route(from, to, crate::model::ConnectorStyle::Curved);
                Some(
                    line.into_iter()
                        .map(|(x, y)| origin + self.canvas.doc_to_local(x, y))
                        .collect(),
                )
            })
            .collect()
    }

    fn connection_hit(&self, p: Point<Pixels>) -> Option<u64> {
        self.connection_paths()
            .into_iter()
            .map(|(id, points)| {
                let points: Vec<(f32, f32)> = points
                    .iter()
                    .map(|q| (q.x.as_f32(), q.y.as_f32()))
                    .collect();
                (
                    id,
                    distance_to_polyline(&points, (p.x.as_f32(), p.y.as_f32())),
                )
            })
            .filter(|(_, distance)| *distance <= 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    fn finish_connect(
        &mut self,
        start: Point<Pixels>,
        end: Point<Pixels>,
        from: Endpoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tool = Tool::Select;
        if distance(start, end) < DRAG_THRESHOLD {
            return;
        }
        let to = self.endpoint_at(end, Modifiers::default());
        if to == from {
            return;
        }
        if let Some(id) = self.apply(window, cx, |e| e.connect(from, to)) {
            self.editor.select([]);
            self.selected_connection = Some(id);
        }
    }

    fn finish_stroke(
        &mut self,
        tool: Tool,
        points: Vec<(f32, f32)>,
        parent: Option<NodeId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = points[0];
        let end = points[points.len() - 1];
        let length = ((end.0 - start.0).powi(2) + (end.1 - start.1).powi(2)).sqrt();
        if tool != Tool::Pencil && length < 3.0 {
            return;
        }
        let stroke = crate::shapes::Stroke {
            width: if tool == Tool::Pencil { 3.0 } else { 2.0 },
            ..crate::shapes::Stroke::default()
        };
        let origin = parent
            .and_then(|p| self.canvas.doc_bounds(p))
            .map(|b| (b.origin.x, b.origin.y));
        let build = |offset: (f32, f32)| -> Option<String> {
            let local: Vec<(f32, f32)> = points
                .iter()
                .map(|(x, y)| (x - offset.0, y - offset.1))
                .collect();
            match tool {
                Tool::Pencil => crate::shapes::freehand_svg(&local, &stroke),
                _ => Some(crate::shapes::line_svg(
                    local[0],
                    local[local.len() - 1],
                    &stroke,
                    tool == Tool::Arrow,
                )),
            }
        };
        match (parent, origin) {
            (Some(parent), Some(origin)) => {
                let Some(svg) = build(origin) else { return };
                self.apply(window, cx, |e| e.insert_vector(parent, &svg));
            }
            _ => {
                // Outside any artboard: wrap the drawing in a transparent artboard.
                let min_x = points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min) - 24.0;
                let min_y = points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min) - 24.0;
                let max_x = points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max) + 24.0;
                let max_y = points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max) + 24.0;
                let Some(svg) = build((min_x.round(), min_y.round())) else {
                    return;
                };
                self.apply(window, cx, |e| {
                    let board = e.create_artboard(
                        "Drawing",
                        ((max_x - min_x).round(), (max_y - min_y).round()),
                        Some((min_x.round(), min_y.round())),
                    )?;
                    e.set_styles(
                        &[board],
                        &[
                            ("background-color".into(), Some("transparent".into())),
                            ("overflow".into(), Some("visible".into())),
                        ],
                        None,
                    )?;
                    e.insert_vector(board, &svg)
                });
            }
        }
    }

    /// Rectangles a moving or resizing layer snaps to: other artboards for
    /// artboards; otherwise the parent and its other visible children.
    fn snap_targets(&self, moving: &[NodeId]) -> Vec<Rect> {
        let doc = &self.editor.doc;
        let Some(first) = moving.first() else {
            return Vec::new();
        };
        let others: Vec<NodeId> = if doc.is_artboard(*first) {
            self.page_artboards()
        } else {
            match doc.parent(*first) {
                Some(parent) => std::iter::once(parent)
                    .chain(doc.children(parent).iter().copied())
                    .collect(),
                None => Vec::new(),
            }
        };
        others
            .into_iter()
            .filter(|id| {
                !moving.contains(id) && doc.get(*id).is_some_and(|n| !n.hidden && !n.is_text())
            })
            .filter_map(|id| self.canvas.doc_rect(id))
            .collect()
    }

    /// Pointer offset in document pixels, snapped to nearby edges and centers
    /// (hold ⌘/Ctrl to move freely). Updates the visible guides.
    fn snapped_offset(
        &mut self,
        start: Point<Pixels>,
        p: Point<Pixels>,
        rect: Option<Rect>,
        ids: &[NodeId],
        modifiers: Modifiers,
    ) -> (f32, f32) {
        let zoom = self.canvas.camera.zoom;
        let mut dx = (p.x - start.x).as_f32() / zoom;
        let mut dy = (p.y - start.y).as_f32() / zoom;
        self.canvas.guides.clear();
        if let Some(rect) = rect
            && !modifiers.secondary()
        {
            let targets = self.snap_targets(ids);
            let s = snap(
                rect.offset(dx.round(), dy.round()),
                Edges::ALL,
                &targets,
                SNAP_DISTANCE / zoom,
            );
            dx = dx.round() + s.dx;
            dy = dy.round() + s.dy;
            self.canvas.guides = s.guides;
        }
        (dx, dy)
    }

    fn begin_drag(&self, start: Point<Pixels>, press: Press) -> Drag {
        let doc = &self.editor.doc;
        match press {
            Press::Empty => Drag::Marquee {
                start,
                current: start,
                artboard: None,
                base: Vec::new(),
            },
            Press::ArtboardBackground(root) => Drag::Marquee {
                start,
                current: start,
                artboard: Some(root),
                base: Vec::new(),
            },
            Press::Node(id) => {
                let selection = self.editor.selection.clone();
                if selection.iter().all(|s| doc.is_artboard(*s)) {
                    let origins = selection
                        .iter()
                        .filter_map(|s| doc.artboard_of(*s).map(|a| (*s, a.x, a.y)))
                        .collect();
                    let rect =
                        Rect::union_all(selection.iter().filter_map(|s| self.canvas.doc_rect(*s)));
                    return Drag::MoveArtboards {
                        start,
                        origins,
                        rect,
                    };
                }
                let absolute = |n: NodeId| {
                    doc.get(n).is_some_and(|node| {
                        self.editor.doc.computed(node).position == Position::Absolute
                    })
                };
                if absolute(id) {
                    let origins = selection
                        .iter()
                        .filter(|s| absolute(**s))
                        .filter_map(|s| {
                            let computed = doc.computed(doc.get(*s)?);
                            let (left, top) = match (computed.inset[3].px(), computed.inset[0].px())
                            {
                                (Some(l), Some(t)) => (l, t),
                                _ => {
                                    let parent = doc.parent(*s)?;
                                    let b = self.canvas.doc_bounds(*s)?;
                                    let pb = self.canvas.doc_bounds(parent)?;
                                    (b.origin.x - pb.origin.x, b.origin.y - pb.origin.y)
                                }
                            };
                            Some((*s, left, top))
                        })
                        .collect();
                    let rect = Rect::union_all(
                        selection
                            .iter()
                            .filter(|s| absolute(**s))
                            .filter_map(|s| self.canvas.doc_rect(*s)),
                    );
                    return Drag::MoveAbsolute {
                        start,
                        origins,
                        rect,
                    };
                }
                Drag::Reorder { id, target: None }
            }
        }
    }

    fn drop_target(&self, dragged: NodeId, p: Point<Pixels>) -> Option<DropTarget> {
        let doc = &self.editor.doc;
        let path = self.hit_path(p, Some(dragged));
        let container = path
            .iter()
            .rev()
            .copied()
            .find(|id| self.is_container(*id) && !doc.is_ancestor_or_self(dragged, *id))
            .or_else(|| doc.parent(dragged))?;
        let computed = doc.computed(doc.get(container)?);
        let horizontal = computed.display == Display::Flex && !computed.direction.is_column()
            || computed.display == Display::Grid;
        let all = doc.children(container).to_vec();
        let flow: Vec<NodeId> = all
            .iter()
            .copied()
            .filter(|c| {
                *c != dragged
                    && doc.get(*c).is_some_and(|n| {
                        !n.hidden
                            && !n.is_text()
                            && self.editor.doc.computed(n).position != Position::Absolute
                    })
            })
            .collect();
        let container_bounds = self.canvas.window_bounds(container)?;
        let line = |b: Bounds<Pixels>, after: bool| -> Bounds<Pixels> {
            if horizontal {
                let x = if after {
                    b.origin.x + b.size.width
                } else {
                    b.origin.x
                };
                Bounds {
                    origin: point(x - px(1.0), b.origin.y),
                    size: size(px(2.0), b.size.height),
                }
            } else {
                let y = if after {
                    b.origin.y + b.size.height
                } else {
                    b.origin.y
                };
                Bounds {
                    origin: point(b.origin.x, y - px(1.0)),
                    size: size(b.size.width, px(2.0)),
                }
            }
        };
        for child in &flow {
            let Some(b) = self.canvas.window_bounds(*child) else {
                continue;
            };
            let before = if horizontal {
                p.x < b.center().x
                    && (computed.display != Display::Grid || p.y < b.origin.y + b.size.height)
            } else {
                p.y < b.center().y
            };
            if before {
                let index = all.iter().position(|c| c == child).unwrap_or(all.len());
                return Some(DropTarget {
                    parent: container,
                    index,
                    line: line(b, false),
                });
            }
        }
        let line = match flow.last().and_then(|c| self.canvas.window_bounds(*c)) {
            Some(b) => line(b, true),
            None => Bounds {
                origin: container_bounds.origin + point(px(4.0), px(4.0)),
                size: size(container_bounds.size.width - px(8.0), px(2.0)),
            },
        };
        Some(DropTarget {
            parent: container,
            index: all.len(),
            line,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_resize(
        &mut self,
        id: NodeId,
        handle: Handle,
        start: Point<Pixels>,
        rect: DocRect,
        insets: (f32, f32),
        p: Point<Pixels>,
        keep_aspect: bool,
        snapping: bool,
        key: &str,
    ) {
        let zoom = self.canvas.camera.zoom;
        let dx = (p.x - start.x).as_f32() / zoom;
        let dy = (p.y - start.y).as_f32() / zoom;
        let (ax, ay) = handle.anchor();
        let mut x = rect.x;
        let mut y = rect.y;
        let mut w = rect.w;
        let mut h = rect.h;
        if ax == 1.0 {
            w += dx;
        } else if ax == 0.0 {
            x += dx;
            w -= dx;
        }
        if ay == 1.0 {
            h += dy;
        } else if ay == 0.0 {
            y += dy;
            h -= dy;
        }
        if keep_aspect && rect.w > 0.0 && rect.h > 0.0 && ax != 0.5 && ay != 0.5 {
            let scale = (w / rect.w).max(h / rect.h);
            let nw = rect.w * scale;
            let nh = rect.h * scale;
            if ax == 0.0 {
                x = rect.x + rect.w - nw;
            }
            if ay == 0.0 {
                y = rect.y + rect.h - nh;
            }
            w = nw;
            h = nh;
        }
        self.canvas.guides.clear();
        if snapping && !keep_aspect {
            let edges = Edges {
                x: [ax == 0.0, false, ax == 1.0],
                y: [ay == 0.0, false, ay == 1.0],
            };
            let targets = self.snap_targets(&[id]);
            let threshold = SNAP_DISTANCE / zoom;
            let s = snap(Rect::new(x, y, w, h), edges, &targets, threshold);
            if ax == 1.0 {
                w += s.dx;
            } else if ax == 0.0 {
                x += s.dx;
                w -= s.dx;
            }
            if ay == 1.0 {
                h += s.dy;
            } else if ay == 0.0 {
                y += s.dy;
                h -= s.dy;
            }
            self.canvas.guides = guides(Rect::new(x, y, w, h), edges, &targets);
        }
        w = w.max(1.0).round();
        h = h.max(1.0).round();
        let changes_width = ax != 0.5;
        let changes_height = ay != 0.5;
        let doc = &self.editor.doc;
        if doc.is_artboard(id) {
            let _ = self.editor.set_size(
                id,
                changes_width.then_some(w),
                changes_height.then_some(h),
                Some(key),
            );
            let _ = self.editor.move_artboard(id, x, y, Some(key));
            return;
        }
        let absolute = doc
            .get(id)
            .is_some_and(|n| self.editor.doc.computed(n).position == Position::Absolute);
        let mut changes = Vec::new();
        if changes_width {
            changes.push(("width".to_owned(), Some(format!("{}px", fmt_num(w)))));
        }
        if changes_height {
            changes.push(("height".to_owned(), Some(format!("{}px", fmt_num(h)))));
        }
        if absolute {
            changes.push((
                "left".to_owned(),
                Some(format!("{}px", fmt_num((insets.0 + x - rect.x).round()))),
            ));
            changes.push((
                "top".to_owned(),
                Some(format!("{}px", fmt_num((insets.1 + y - rect.y).round()))),
            ));
        }
        let _ = self.editor.set_styles(&[id], &changes, Some(key));
    }

    fn canvas_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.canvas.drag.take() else {
            return;
        };
        self.canvas.guides.clear();
        match drag {
            Drag::Press { press, .. } => match press {
                Press::Empty if !event.modifiers.shift => self.editor.select([]),
                Press::ArtboardBackground(root) => {
                    if event.modifiers.shift {
                        self.editor.toggle_selected(root);
                    } else {
                        self.editor.select([root]);
                    }
                }
                _ => {}
            },
            Drag::Reorder {
                id,
                target: Some(target),
                ..
            } => {
                let same_place = self.editor.doc.parent(id) == Some(target.parent) && {
                    let children = self.editor.doc.children(target.parent);
                    let old = children.iter().position(|c| *c == id);
                    old == Some(target.index) || old.map(|o| o + 1) == Some(target.index)
                };
                if !same_place {
                    self.apply(window, cx, |e| {
                        e.move_node(id, target.parent, Some(target.index))
                    });
                }
            }
            Drag::Draw {
                tool,
                start,
                current,
                parent,
            } => {
                self.finish_draw(tool, start, current, parent, window, cx);
            }
            Drag::Stroke {
                tool,
                points,
                parent,
            } => {
                self.finish_stroke(tool, points, parent, window, cx);
            }
            Drag::Connect {
                start,
                current,
                from,
            } => {
                self.canvas.hover = None;
                self.finish_connect(start, current, from, window, cx);
            }
            _ => {}
        }
        self.editor.history.seal();
        self.inspector.invalidate();
        cx.notify();
    }

    fn finish_draw(
        &mut self,
        tool: Tool,
        start: Point<Pixels>,
        current: Point<Pixels>,
        parent: Option<NodeId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (x0, y0) = self.canvas.to_doc(start);
        let (x1, y1) = self.canvas.to_doc(current);
        let (mut x, mut y) = (x0.min(x1).round(), y0.min(y1).round());
        let (mut w, mut h) = ((x1 - x0).abs().round(), (y1 - y0).abs().round());
        let clicked = w < 4.0 && h < 4.0;
        if clicked {
            (w, h) = match (tool, parent) {
                (Tool::Text, _) => (0.0, 0.0),
                (_, None) => (400.0, 300.0),
                _ => (100.0, 100.0),
            };
        }
        self.tool = Tool::Select;
        let Some(parent) = parent else {
            let name = match tool {
                Tool::Rectangle => "Rectangle",
                Tool::Ellipse => "Ellipse",
                Tool::Text => "Text",
                _ => "Frame",
            };
            if tool == Tool::Text {
                (w, h) = (w.max(240.0), h.max(64.0));
            }
            let created = self.apply(window, cx, |e| {
                let board = e.create_artboard(name, (w, h), Some((x, y)))?;
                if matches!(tool, Tool::Rectangle | Tool::Ellipse) {
                    e.set_styles(
                        &[board],
                        &[("background-color".into(), Some("#d9d9d9".into()))],
                        None,
                    )?;
                }
                if tool == Tool::Ellipse {
                    e.set_styles(
                        &[board],
                        &[("border-radius".into(), Some("50%".into()))],
                        None,
                    )?;
                }
                if tool == Tool::Text {
                    e.set_styles(
                        &[board],
                        &[
                            ("background-color".into(), Some("transparent".into())),
                            ("padding".into(), Some("8px".into())),
                        ],
                        None,
                    )?;
                    let ids = e.insert_html(
                        &format!("<p style=\"margin: 0; {}\">Text</p>", presets::text_style()),
                        InsertTarget {
                            parent: Some(board),
                            index: None,
                        },
                    )?;
                    return Ok(ids[0]);
                }
                Ok(board)
            });
            if tool == Tool::Text
                && let Some(text) = created
            {
                self.start_text_edit(text, window, cx);
            }
            return;
        };
        let parent_computed = self
            .editor
            .doc
            .get(parent)
            .map(|n| self.editor.doc.computed(n))
            .unwrap_or_default();
        let flow = matches!(parent_computed.display, Display::Flex | Display::Grid);
        let mut style = match tool {
            Tool::Rectangle => presets::rectangle_style(w, h),
            Tool::Ellipse => format!("{}; border-radius: 50%", presets::rectangle_style(w, h)),
            Tool::Text => format!("margin: 0; {}", presets::text_style()),
            _ => presets::frame_style(w, h),
        };
        if !flow {
            if let Some(pb) = self.canvas.doc_bounds(parent) {
                x -= pb.origin.x;
                y -= pb.origin.y;
            }
            style = format!(
                "position: absolute; left: {}px; top: {}px; {style}",
                fmt_num(x),
                fmt_num(y)
            );
        }
        let (tag, name, content) = match tool {
            Tool::Rectangle => ("div", "Rectangle", ""),
            Tool::Ellipse => ("div", "Ellipse", ""),
            Tool::Text => ("p", "", "Text"),
            _ => ("div", "Frame", ""),
        };
        let name_attr = if name.is_empty() {
            String::new()
        } else {
            format!(" data-name=\"{name}\"")
        };
        let html = format!("<{tag}{name_attr} style=\"{style}\">{content}</{tag}>");
        let index = if flow {
            self.drop_target(NodeId(u64::MAX), current)
                .filter(|t| t.parent == parent)
                .map(|t| t.index)
        } else {
            None
        };
        let parent_static = parent_computed.position == Position::Static;
        let created = self.apply(window, cx, |e| {
            if !flow && parent_static && !e.doc.is_artboard(parent) {
                e.set_styles(
                    &[parent],
                    &[("position".into(), Some("relative".into()))],
                    None,
                )?;
            }
            let ids = e.insert_html(
                &html,
                InsertTarget {
                    parent: Some(parent),
                    index,
                },
            )?;
            Ok(ids[0])
        });
        if let (Tool::Text, Some(id)) = (tool, created) {
            self.start_text_edit(id, window, cx);
        }
    }

    fn canvas_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(window.line_height());
        if event.modifiers.secondary() || event.modifiers.control {
            let factor = (-delta.y.as_f32() * 0.01).exp();
            let local = event.position - self.canvas.origin();
            self.canvas.zoom_at(factor, local);
        } else if event.modifiers.shift && delta.x.as_f32().abs() < 0.01 {
            self.canvas.camera.pan.x += delta.y;
        } else {
            self.canvas.camera.pan += delta;
        }
        self.canvas.needs_fit = false;
        cx.notify();
    }

    // ---- zoom ------------------------------------------------------------------------

    pub(super) fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        let size = self.canvas.bounds.get().size;
        self.canvas
            .zoom_at(factor, point(size.width / 2.0, size.height / 2.0));
        self.canvas.needs_fit = false;
        cx.notify();
    }

    pub(super) fn zoom_reset(&mut self, cx: &mut Context<Self>) {
        let zoom = self.canvas.camera.zoom;
        self.zoom_by(1.0 / zoom, cx);
    }

    pub(super) fn zoom_fit(&mut self, cx: &mut Context<Self>) {
        let rects: Vec<DocRect> = self
            .page_artboards()
            .into_iter()
            .filter_map(|r| self.artboard_rect(r))
            .collect();
        if let Some(rect) = union(rects) {
            self.canvas.fit(rect);
        }
        self.canvas.needs_fit = false;
        cx.notify();
    }

    pub(super) fn zoom_selection(&mut self, cx: &mut Context<Self>) {
        let rects: Vec<DocRect> = self
            .editor
            .selection
            .iter()
            .filter_map(|id| self.canvas.doc_bounds(*id))
            .map(|b| DocRect {
                x: b.origin.x,
                y: b.origin.y,
                w: b.size.width,
                h: b.size.height,
            })
            .collect();
        match union(rects) {
            Some(rect) => {
                self.canvas.fit(rect);
                self.canvas.needs_fit = false;
                cx.notify();
            }
            None => self.zoom_fit(cx),
        }
    }

    /// Center the view on a node without changing zoom.
    pub(super) fn reveal(&mut self, id: NodeId, cx: &mut Context<Self>) {
        let rect = self
            .canvas
            .doc_bounds(id)
            .map(|b| DocRect {
                x: b.origin.x,
                y: b.origin.y,
                w: b.size.width,
                h: b.size.height,
            })
            .or_else(|| self.artboard_rect(id));
        let Some(rect) = rect else { return };
        let viewport = self.canvas.bounds.get().size;
        let local = self.canvas.doc_to_local(rect.x, rect.y);
        let zoom = self.canvas.camera.zoom;
        let visible = local.x.as_f32() >= 0.0
            && local.y.as_f32() >= 0.0
            && local.x.as_f32() + rect.w * zoom <= viewport.width.as_f32()
            && local.y.as_f32() + rect.h * zoom <= viewport.height.as_f32();
        if !visible {
            self.canvas.camera.pan = point(
                viewport.width / 2.0 - px((rect.x + rect.w / 2.0) * zoom),
                viewport.height / 2.0 - px((rect.y + rect.h / 2.0) * zoom),
            );
            cx.notify();
        }
    }

    // ---- text editing --------------------------------------------------------------

    pub(super) fn start_text_edit(
        &mut self,
        id: NodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_text_edit(cx);
        let text = self.editor.doc.text_content(id).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    this.commit_text_edit(cx);
                    this.canvas_focus.focus(window, cx);
                }
                InputEvent::Blur => this.commit_text_edit(cx),
                _ => {}
            },
        );
        // Focus after the current mouse event: GPUI focuses the canvas on
        // mouse-down once our handler returns, which would blur the editor.
        let focus_input = input.clone();
        cx.defer_in(window, move |_, window, cx| {
            focus_input.update(cx, |state, cx| {
                state.focus(window, cx);
                state.select_all(window, cx);
            });
        });
        self.editor.select([id]);
        self.canvas.text_edit = Some((id, input, subscription));
        cx.notify();
    }

    fn commit_text_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, input, _subscription)) = self.canvas.text_edit.take() else {
            return;
        };
        let value = input.read(cx).value().to_string();
        if self.editor.doc.text_content(id).as_deref() != Some(value.as_str()) {
            // Keep inline formatting when the text did not change.
            let _ = self.editor.set_text(id, &value, None);
        }
        self.inspector.invalidate();
        cx.notify();
    }

    fn cancel_text_edit(&mut self, cx: &mut Context<Self>) {
        self.canvas.text_edit = None;
        cx.notify();
    }

    // ---- comments ------------------------------------------------------------------

    fn open_comment_draft(
        &mut self,
        id: NodeId,
        anchor: (f32, f32),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Add a comment for your team or agent…")
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let body = input.read(cx).value().to_string();
                    this.submit_comment(body, window, cx);
                }
            },
        );
        self._subscriptions.push(subscription);
        let focus_input = input.clone();
        cx.defer_in(window, move |_, window, cx| {
            focus_input.update(cx, |state, cx| state.focus(window, cx));
        });
        self.comment_draft = Some((id, anchor, input));
        cx.notify();
    }

    pub(super) fn submit_comment(
        &mut self,
        body: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((id, anchor, _)) = self.comment_draft.take() else {
            return;
        };
        if body.trim().is_empty() {
            cx.notify();
            return;
        }
        let Some(comments) = self.editor.comments.as_mut() else {
            return;
        };
        match comments.add(&id.to_string(), anchor, body.trim(), "you") {
            Ok(_) => {
                self.right_tab = RightTab::Comments;
                self.tool = Tool::Select;
            }
            Err(error) => {
                window.push_notification(
                    gpui_kit::component::notification::Notification::error(format!("{error:#}")),
                    cx,
                );
            }
        }
        self.canvas_focus.focus(window, cx);
        cx.notify();
    }

    // ---- clipboard -----------------------------------------------------------------

    fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        if self.editor.selection.is_empty() {
            return false;
        }
        let html = self
            .editor
            .selection
            .iter()
            .map(|id| self.editor.html_of(*id, false))
            .collect::<Vec<_>>()
            .join("");
        cx.write_to_clipboard(ClipboardItem::new_string(html));
        true
    }

    /// Where an image dropped at `p` (or pasted, when `None`) goes.
    fn image_placement(&self, p: Option<Point<Pixels>>) -> ImagePlacement {
        let doc = &self.editor.doc;
        let Some(p) = p else {
            let target = self.editor.insertion_target();
            return ImagePlacement {
                parent: target.parent,
                at: target.parent.is_some().then_some((24.0, 24.0)),
                index: target.index,
            };
        };
        let (x, y) = self.canvas.to_doc(p);
        let container = self
            .hit_path(p, None)
            .into_iter()
            .rev()
            .find(|id| self.is_container(*id) && doc.get(*id).is_some_and(|n| !n.locked));
        let Some(parent) = container else {
            return ImagePlacement {
                parent: None,
                at: Some((x, y)),
                index: None,
            };
        };
        let flow = doc.get(parent).is_some_and(|n| {
            matches!(
                self.editor.doc.computed(n).display,
                Display::Flex | Display::Grid
            )
        });
        let origin = self
            .canvas
            .doc_bounds(parent)
            .map_or((0.0, 0.0), |b| (b.origin.x, b.origin.y));
        ImagePlacement {
            parent: Some(parent),
            at: Some((x - origin.0, y - origin.1)),
            index: if flow {
                self.drop_target(NodeId(u64::MAX), p)
                    .filter(|t| t.parent == parent)
                    .map(|t| t.index)
            } else {
                None
            },
        }
    }

    /// Import image files (name, bytes), cascading multiple images.
    pub(super) fn import_images(
        &mut self,
        images: Vec<(String, Vec<u8>)>,
        p: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let base = self.image_placement(p);
        let mut placed = Vec::new();
        for (index, (name, bytes)) in images.into_iter().enumerate() {
            let offset = index as f32 * 24.0;
            let placement = ImagePlacement {
                at: base.at.map(|(x, y)| (x + offset, y + offset)),
                index: base.index.map(|i| i + index),
                ..base
            };
            if let Some(id) = self.apply(window, cx, |e| e.import_image(&bytes, &name, placement)) {
                placed.push(id);
            }
        }
        if !placed.is_empty() {
            self.editor.select(placed);
            self.tool = Tool::Select;
            self.canvas_focus.focus(window, cx);
        }
    }

    /// Read dropped or copied files that are images.
    pub(super) fn read_image_files(paths: &[std::path::PathBuf]) -> Vec<(String, Vec<u8>)> {
        paths
            .iter()
            .filter(|p| crate::assets::is_image_path(p))
            .filter(|p| {
                std::fs::metadata(p).is_ok_and(|m| m.len() <= crate::assets::MAX_IMAGE_BYTES as u64)
            })
            .filter_map(|p| {
                let name = p.file_name()?.to_str()?.to_owned();
                Some((name, std::fs::read(p).ok()?))
            })
            .collect()
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut images = Vec::new();
        for entry in item.entries() {
            match entry {
                gpui_kit::ClipboardEntry::Image(image) => {
                    let ext = match image.format {
                        gpui_kit::ImageFormat::Png => "png",
                        gpui_kit::ImageFormat::Jpeg => "jpg",
                        gpui_kit::ImageFormat::Webp => "webp",
                        gpui_kit::ImageFormat::Gif => "gif",
                        gpui_kit::ImageFormat::Svg => "svg",
                        gpui_kit::ImageFormat::Bmp => "bmp",
                        _ => continue,
                    };
                    images.push((format!("Pasted image.{ext}"), image.bytes.clone()));
                }
                gpui_kit::ClipboardEntry::ExternalPaths(paths) => {
                    images.extend(Self::read_image_files(paths.paths()));
                }
                gpui_kit::ClipboardEntry::String(_) => {}
            }
        }
        if !images.is_empty() {
            self.import_images(images, None, window, cx);
            return;
        }
        let Some(text) = item.text() else {
            return;
        };
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        let target = self.editor.insertion_target();
        let html = if text.starts_with('<') {
            text
        } else {
            format!(
                "<p style=\"margin: 0; {}\">{}</p>",
                presets::text_style(),
                escape_text(&text)
            )
        };
        if target.parent.is_none() && !html.starts_with("<p") {
            self.apply(window, cx, |e| e.insert_html(&html, target));
        } else if target.parent.is_none() {
            // Loose text needs a frame.
            self.apply(window, cx, |e| {
                let board = e.create_artboard("Text", (320.0, 120.0), None)?;
                e.insert_html(
                    &html,
                    InsertTarget {
                        parent: Some(board),
                        index: None,
                    },
                )
            });
        } else {
            self.apply(window, cx, |e| e.insert_html(&html, target));
        }
    }

    // ---- actions -------------------------------------------------------------------

    fn nudge(&mut self, dx: f32, dy: f32, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.editor.selection.clone();
        if selection.is_empty() {
            return;
        }
        let movable = selection.iter().any(|id| {
            self.editor.doc.is_artboard(*id)
                || self
                    .editor
                    .doc
                    .get(*id)
                    .is_some_and(|n| self.editor.doc.computed(n).position == Position::Absolute)
        });
        if movable {
            self.apply(window, cx, |e| e.nudge(&selection, dx, dy));
        } else if let Some(id) = self.editor.primary() {
            // Flow children reorder with the arrow keys.
            let delta = if dx < 0.0 || dy < 0.0 { -1 } else { 1 };
            self.apply(window, cx, |e| e.reorder(id, delta));
        }
    }

    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.canvas.text_edit.is_some() {
            self.cancel_text_edit(cx);
            return;
        }
        if self.comment_draft.take().is_some() || self.canvas.drag.take().is_some() {
            cx.notify();
            return;
        }
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
        } else if let Some(id) = self.editor.primary() {
            match self.editor.doc.parent(id) {
                Some(parent) => self.editor.select([parent]),
                None => self.editor.select([]),
            }
        }
        self.inspector.invalidate();
        cx.notify();
    }

    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.editor.primary() else {
            return;
        };
        if self.editor.doc.is_text_layer(id) {
            self.start_text_edit(id, window, cx);
            return;
        }
        let children: Vec<NodeId> = self
            .editor
            .doc
            .children(id)
            .iter()
            .copied()
            .filter(|c| self.editor.doc.get(*c).is_some_and(|n| !n.is_text()))
            .collect();
        if !children.is_empty() {
            self.editor.select(children);
            self.inspector.invalidate();
            cx.notify();
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let siblings = match self
            .editor
            .primary()
            .and_then(|id| self.editor.doc.parent(id))
        {
            Some(parent) => self
                .editor
                .doc
                .children(parent)
                .iter()
                .copied()
                .filter(|c| self.editor.doc.get(*c).is_some_and(|n| !n.is_text()))
                .collect(),
            None => self.page_artboards(),
        };
        self.editor.select(siblings);
        self.inspector.invalidate();
        cx.notify();
    }

    // ---- render --------------------------------------------------------------------

    pub(super) fn render_canvas(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.canvas.needs_fit && self.canvas.bounds.get().size.width > px(50.0) {
            let rects: Vec<DocRect> = self
                .page_artboards()
                .into_iter()
                .filter_map(|r| self.artboard_rect(r))
                .collect();
            if let Some(rect) = union(rects) {
                self.canvas.fit(rect);
            }
            self.canvas.needs_fit = false;
        }
        let theme = cx.theme().clone();
        let dark = theme.is_dark();
        let canvas_bg: Hsla = if dark {
            gpui_kit::rgb(0x1e1e1e).into()
        } else {
            gpui_kit::rgb(0xf5f5f4).into()
        };
        let accent: Hsla = gpui_kit::rgb(0x0d99ff).into();
        let label_color = theme.muted_foreground;
        let rendered = Some((
            self.editor.revision,
            self.canvas.camera.zoom,
            self.canvas.camera.pan,
            self.editor.selection.clone(),
        ));
        if self.canvas.rendered != rendered {
            self.canvas.rendered = rendered;
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        // Overlays read the previous frame's bounds, so build them first.
        let size_label = match self.editor.selection.as_slice() {
            [id] if self.canvas.text_edit.is_none() => {
                self.canvas.window_bounds(*id).and_then(|b| {
                    let d = self.canvas.doc_bounds(*id)?;
                    let local = b.origin - self.canvas.origin();
                    Some(
                        div()
                            .absolute()
                            .left(local.x + b.size.width / 2.0 - px(40.0))
                            .top(local.y + b.size.height + px(8.0))
                            .w(px(80.0))
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .bg(accent)
                                    .text_color(gpui_kit::white())
                                    .text_size(px(10.0))
                                    .child({
                                        // Prefer declared sizes; measured ones carry pixel snapping.
                                        let c = self
                                            .editor
                                            .doc
                                            .get(*id)
                                            .map(|n| self.editor.doc.computed(n))
                                            .unwrap_or_default();
                                        let w = c.width.px().unwrap_or(d.size.width).round();
                                        let h = c.height.px().unwrap_or(d.size.height).round();
                                        format!("{} × {}", fmt_num(w), fmt_num(h))
                                    }),
                            )
                            .into_any_element(),
                    )
                })
            }
            _ => None,
        };

        let text_editor = self.canvas.text_edit.as_ref().and_then(|(id, input, _)| {
            let b = self.canvas.window_bounds(*id)?;
            let local = b.origin - self.canvas.origin();
            Some(
                div()
                    .absolute()
                    .left(local.x)
                    .top(local.y + b.size.height / 2.0 - px(14.0))
                    .w(b.size.width.max(px(160.0)))
                    .shadow_md()
                    .child(Input::new(input).small())
                    .into_any_element(),
            )
        });

        let export_stage = self.render_export_stage(window, cx);
        let pins = self.render_comment_pins(cx);
        let connection_labels = self.render_connection_labels(cx);
        let draft = self.render_comment_draft(cx);

        // Bounds persist across frames (each prepaint overwrites them), so
        // overlays and hit tests always see a complete map; drop entries for
        // layers that are gone or hidden. Last frame's bounds also feed
        // measurement-dependent layout such as auto-fill grids.
        {
            let doc = &self.editor.doc;
            self.canvas.layout.borrow_mut().retain(|id, _| {
                doc.get(*id).is_some_and(|n| {
                    !n.hidden && self.editor.doc.computed(n).display != Display::None
                })
            });
        }
        let measured = self.canvas.layout.borrow().clone();
        let zoom = self.canvas.camera.zoom;
        let asset_dir = self
            .editor
            .project
            .as_ref()
            .map(|p| p.root().join("artboards"))
            .unwrap_or_else(std::env::temp_dir);
        let cache_dir = self
            .editor
            .project
            .as_ref()
            .map(|p| p.studio_dir().join("cache"))
            .unwrap_or_else(|| std::env::temp_dir().join("gpui-studio-cache"));
        let editing = self.canvas.text_edit.as_ref().map(|(id, _, _)| *id);
        let painter = Painter {
            doc: &self.editor.doc,
            zoom,
            asset_dir: &asset_dir,
            cache_dir: &cache_dir,
            layout: &self.canvas.layout,
            fonts: &self.fonts,
            editing,
            unsettled: std::cell::Cell::new(false),
            measured: &measured,
            texts: None,
        };
        let mut artboards = Vec::new();
        let mut labels = Vec::new();
        for root in self.page_artboards() {
            let Some(artboard) = self.editor.doc.artboard_of(root) else {
                continue;
            };
            let local = self.canvas.doc_to_local(artboard.x, artboard.y);
            if let Some(element) = painter.artboard(root) {
                artboards.push(
                    div()
                        .absolute()
                        .left(local.x)
                        .top(local.y)
                        .shadow(vec![gpui_kit::BoxShadow {
                            color: gpui_kit::hsla(0.0, 0.0, 0.0, 0.08),
                            offset: point(px(0.0), px(1.0)),
                            blur_radius: px(3.0),
                            spread_radius: px(0.0),
                            inset: false,
                        }])
                        .child(element)
                        .into_any_element(),
                );
            }
            let selected = self.editor.selection.contains(&root);
            labels.push(
                div()
                    .absolute()
                    .left(local.x)
                    .top(local.y - px(20.0))
                    .h(px(18.0))
                    .max_w(px(self
                        .artboard_rect(root)
                        .map_or(200.0, |r| r.w * zoom)
                        .max(40.0)))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .text_color(if selected { accent } else { label_color })
                    .child(self.editor.doc.display_name(root))
                    .into_any_element(),
            );
        }

        if painter.unsettled.get() {
            // Measurements this layout needs arrive with this frame.
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        // Overlay data captured for the paint pass.
        let layout = self.canvas.layout.clone();
        let selection = self.editor.selection.clone();
        let hover = self.canvas.hover.filter(|h| !selection.contains(h));
        let drag = self.canvas.drag.clone();
        let editing_text = editing.is_some();
        let draw_tool_parent = match &drag {
            Some(Drag::Draw { parent, .. }) => *parent,
            _ => None,
        };
        let entity = cx.entity();
        let dragging = drag.is_some();
        let canvas_bounds = self.canvas.bounds.get();
        let selected_connection = self.selected_connection;
        let connections: Vec<(u64, Vec<Point<Pixels>>, Hsla, ArrowHeads)> = self
            .connection_paths()
            .into_iter()
            .filter_map(|(id, points)| {
                let connection = self.editor.doc.connection(id)?;
                let color = crate::model::Color::parse_loose(&connection.color)
                    .map_or(accent, super::paint::hsla);
                Some((id, points, color, connection.heads))
            })
            .collect();
        let origin = self.canvas.origin();
        let stroke_draft: Option<(Tool, Vec<Point<Pixels>>)> = match &drag {
            Some(Drag::Stroke { tool, points, .. }) => Some((
                *tool,
                points
                    .iter()
                    .map(|(x, y)| origin + self.canvas.doc_to_local(*x, *y))
                    .collect(),
            )),
            _ => None,
        };
        let stroke_width = px((if matches!(stroke_draft, Some((Tool::Pencil, _))) {
            3.0
        } else {
            2.0
        }) * zoom);
        let ink: Hsla = gpui_kit::rgb(0x17181c).into();
        let guide_color: Hsla = gpui_kit::rgb(0xf24822).into();
        let presences: Vec<Bounds<Pixels>> = self
            .presence_overlays()
            .into_iter()
            .flat_map(|(_, _, bounds)| bounds)
            .collect();
        let presence_tags = self.render_presence_tags(self.canvas.origin());
        let prototyping = self.right_tab == RightTab::Prototype;
        let flows = if prototyping {
            self.flow_paths()
        } else {
            Vec::new()
        };
        let flow_sources: Vec<Bounds<Pixels>> = if prototyping {
            self.editor
                .doc
                .page_links(self.editor.page)
                .iter()
                .filter_map(|l| self.canvas.window_bounds(l.source))
                .collect()
        } else {
            Vec::new()
        };
        let componentish: BTreeSet<NodeId> = self
            .editor
            .selection
            .iter()
            .chain(self.canvas.hover.iter())
            .copied()
            .filter(|id| {
                let doc = &self.editor.doc;
                doc.component_name(*id).is_some() || doc.instance_root(*id).is_some()
            })
            .collect();
        let guide_lines: Vec<Bounds<Pixels>> = self
            .canvas
            .guides
            .iter()
            .map(|g| {
                let (a, b) = match g.axis {
                    Axis::Vertical => ((g.at, g.from), (g.at, g.to)),
                    Axis::Horizontal => ((g.from, g.at), (g.to, g.at)),
                };
                let a = origin + self.canvas.doc_to_local(a.0, a.1);
                let b = origin + self.canvas.doc_to_local(b.0, b.1);
                match g.axis {
                    Axis::Vertical => Bounds {
                        origin: point(a.x - px(0.5), a.y),
                        size: size(px(1.0), b.y - a.y),
                    },
                    Axis::Horizontal => Bounds {
                        origin: point(a.x, a.y - px(0.5)),
                        size: size(b.x - a.x, px(1.0)),
                    },
                }
            })
            .collect();
        let overlay = canvas(
            move |_, _, _| {},
            move |_bounds, (), window, _cx| {
                let layout = layout.borrow();
                let outline_quad = |b: Bounds<Pixels>, width: f32, color: Hsla| {
                    quad(
                        b,
                        px(0.0),
                        gpui_kit::transparent_black(),
                        px(width),
                        color,
                        BorderStyle::Solid,
                    )
                };
                for (id, points, color, heads) in &connections {
                    let selected = selected_connection == Some(*id);
                    paint_polyline(
                        window,
                        points,
                        px(if selected { 3.0 } else { 2.0 }),
                        *color,
                        false,
                    );
                    if points.len() >= 2 {
                        let n = points.len();
                        if matches!(heads, ArrowHeads::End | ArrowHeads::Both) {
                            paint_arrowhead(window, points[n - 1], points[n - 2], *color);
                        }
                        if matches!(heads, ArrowHeads::Both) {
                            paint_arrowhead(window, points[0], points[1], *color);
                        }
                        if selected {
                            for end in [points[0], points[n - 1]] {
                                let r = Bounds {
                                    origin: end - point(px(4.0), px(4.0)),
                                    size: size(px(8.0), px(8.0)),
                                };
                                window.paint_quad(quad(
                                    r,
                                    px(4.0),
                                    gpui_kit::white(),
                                    px(1.5),
                                    *color,
                                    BorderStyle::Solid,
                                ));
                            }
                        }
                    }
                }
                for b in &presences {
                    window.paint_quad(outline_quad(*b, 2.0, super::chat::AGENT_COLOR));
                }
                let flow = super::present::FLOW_COLOR;
                for b in &flow_sources {
                    window.paint_quad(outline_quad(*b, 1.5, flow));
                }
                for points in &flows {
                    paint_polyline(window, points, px(2.0), flow, false);
                    if let [.., a, b] = points.as_slice() {
                        paint_arrowhead(window, *b, *a, flow);
                    }
                    if let Some(start) = points.first() {
                        window.paint_quad(quad(
                            Bounds {
                                origin: *start - point(px(4.0), px(4.0)),
                                size: size(px(8.0), px(8.0)),
                            },
                            px(4.0),
                            flow,
                            px(1.5),
                            gpui_kit::white(),
                            BorderStyle::Solid,
                        ));
                    }
                }
                for line in &guide_lines {
                    window.paint_quad(fill(*line, guide_color));
                }
                if let Some((tool, points)) = &stroke_draft {
                    paint_polyline(window, points, stroke_width, ink, false);
                    if *tool == Tool::Arrow && points.len() >= 2 {
                        paint_arrowhead(window, points[points.len() - 1], points[0], ink);
                    }
                }
                if let Some(Drag::Connect { start, current, .. }) = &drag {
                    paint_polyline(window, &[*start, *current], px(2.0), accent, true);
                    paint_arrowhead(window, *current, *start, accent);
                }
                let tint = |id: &NodeId| {
                    if componentish.contains(id) {
                        super::panels::COMPONENT_COLOR
                    } else {
                        accent
                    }
                };
                if let Some(hover) = hover.or(draw_tool_parent)
                    && let Some(b) = layout.get(&hover)
                {
                    window.paint_quad(outline_quad(*b, 1.0, tint(&hover)));
                }
                let selected: Vec<Bounds<Pixels>> = selection
                    .iter()
                    .filter_map(|id| layout.get(id).copied())
                    .collect();
                for id in &selection {
                    if let Some(b) = layout.get(id) {
                        window.paint_quad(outline_quad(*b, 1.5, tint(id)));
                    }
                }
                if selected.len() > 1
                    && let Some(union) = window_union(selected.iter().copied())
                {
                    window.paint_quad(outline(union, accent.opacity(0.6), BorderStyle::Dashed));
                }
                if let ([b], false) = (selected.as_slice(), editing_text) {
                    for handle in Handle::ALL {
                        let (ax, ay) = handle.anchor();
                        let c = point(
                            b.origin.x + b.size.width * ax,
                            b.origin.y + b.size.height * ay,
                        );
                        let r = Bounds {
                            origin: c - point(px(4.0), px(4.0)),
                            size: size(px(8.0), px(8.0)),
                        };
                        window.paint_quad(quad(
                            r,
                            px(1.0),
                            gpui_kit::white(),
                            px(1.0),
                            accent,
                            BorderStyle::Solid,
                        ));
                    }
                }
                match &drag {
                    Some(Drag::Marquee { start, current, .. }) => {
                        let r = rect_from_points(*start, *current);
                        window.paint_quad(fill(r, accent.opacity(0.08)));
                        window.paint_quad(outline(r, accent, BorderStyle::Solid));
                    }
                    Some(Drag::Draw {
                        start,
                        current,
                        tool,
                        ..
                    }) if *tool != Tool::Text => {
                        let r = rect_from_points(*start, *current);
                        window.paint_quad(fill(r, accent.opacity(0.06)));
                        window.paint_quad(outline(r, accent, BorderStyle::Solid));
                    }
                    Some(Drag::Reorder {
                        target: Some(target),
                        ..
                    }) => {
                        if let Some(b) = layout.get(&target.parent) {
                            window.paint_quad(outline(
                                *b,
                                accent.opacity(0.5),
                                BorderStyle::Dashed,
                            ));
                        }
                        window.paint_quad(fill(target.line, accent));
                    }
                    _ => {}
                }
                if dragging {
                    // Keep tracking a drag that leaves the canvas.
                    let move_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        // Inside the canvas the element's own handler runs.
                        if phase == DispatchPhase::Capture
                            && event.pressed_button.is_some()
                            && !canvas_bounds.contains(&event.position)
                        {
                            move_entity.update(cx, |this, cx| {
                                this.drag_move(event.position, event.modifiers, window, cx)
                            });
                        }
                    });
                    let up_entity = entity.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            up_entity
                                .update(cx, |this, cx| this.canvas_mouse_up(event, window, cx));
                        }
                    });
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let bounds_cell = self.canvas.bounds.clone();
        let probe = canvas(
            move |bounds, _, _| bounds_cell.set(bounds),
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let cursor = if self.canvas.space_held || self.tool == Tool::Hand {
            if matches!(self.canvas.drag, Some(Drag::Pan { .. })) {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            }
        } else {
            match self.tool {
                Tool::Frame | Tool::Rectangle => CursorStyle::Crosshair,
                Tool::Text => CursorStyle::IBeam,
                Tool::Comment => CursorStyle::PointingHand,
                _ => {
                    let p = window.mouse_position();
                    self.handle_hit(p)
                        .map_or(CursorStyle::Arrow, |(_, h)| h.cursor())
                }
            }
        };

        let canvas_element = div()
            .id("canvas")
            .key_context(CANVAS_CONTEXT)
            .track_focus(&self.canvas_focus)
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(canvas_bg)
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::canvas_mouse_down))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.canvas.drag = Some(Drag::Pan {
                        last: event.position,
                    });
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(Self::canvas_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::canvas_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::canvas_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::canvas_mouse_up))
            .on_scroll_wheel(cx.listener(Self::canvas_scroll))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "space"
                    && this.canvas.text_edit.is_none()
                    && !this.canvas.space_held
                {
                    this.canvas.space_held = true;
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if event.keystroke.key == "space" {
                    this.canvas.space_held = false;
                    cx.notify();
                }
            }))
            .on_drop(
                cx.listener(|this, paths: &gpui_kit::ExternalPaths, window, cx| {
                    let images = Self::read_image_files(paths.paths());
                    if images.is_empty() {
                        window.push_notification(
                            gpui_kit::component::notification::Notification::warning(
                                "Drop PNG, JPEG, GIF, WebP, SVG, or BMP images onto the canvas.",
                            ),
                            cx,
                        );
                        return;
                    }
                    let at = window.mouse_position();
                    this.import_images(images, Some(at), window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &SelectTool, _, cx| this.set_tool(Tool::Select, cx)))
            .on_action(cx.listener(|this, _: &FrameTool, _, cx| this.set_tool(Tool::Frame, cx)))
            .on_action(
                cx.listener(|this, _: &RectangleTool, _, cx| this.set_tool(Tool::Rectangle, cx)),
            )
            .on_action(cx.listener(|this, _: &TextTool, _, cx| this.set_tool(Tool::Text, cx)))
            .on_action(cx.listener(|this, _: &HandTool, _, cx| this.set_tool(Tool::Hand, cx)))
            .on_action(cx.listener(|this, _: &CommentTool, _, cx| this.set_tool(Tool::Comment, cx)))
            .on_action(cx.listener(|this, _: &EllipseTool, _, cx| this.set_tool(Tool::Ellipse, cx)))
            .on_action(cx.listener(|this, _: &PencilTool, _, cx| this.set_tool(Tool::Pencil, cx)))
            .on_action(cx.listener(|this, _: &LineTool, _, cx| this.set_tool(Tool::Line, cx)))
            .on_action(cx.listener(|this, _: &ArrowTool, _, cx| this.set_tool(Tool::Arrow, cx)))
            .on_action(
                cx.listener(|this, _: &ConnectorTool, _, cx| this.set_tool(Tool::Connector, cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteSelection, window, cx| {
                if let Some(id) = this.selected_connection.take() {
                    this.apply(window, cx, |e| e.delete_connection(id));
                    return;
                }
                let selection = this.editor.selection.clone();
                this.apply(window, cx, |e| e.delete(&selection));
            }))
            .on_action(cx.listener(|this, _: &DuplicateSelection, window, cx| {
                let selection = this.editor.selection.clone();
                this.apply(window, cx, |e| e.duplicate(&selection));
            }))
            .on_action(cx.listener(|this, _: &CopySelection, _, cx| {
                this.copy_selection(cx);
            }))
            .on_action(cx.listener(|this, _: &CutSelection, window, cx| {
                if this.copy_selection(cx) {
                    let selection = this.editor.selection.clone();
                    this.apply(window, cx, |e| e.delete(&selection));
                }
            }))
            .on_action(cx.listener(|this, _: &PasteClipboard, window, cx| this.paste(window, cx)))
            .on_action(cx.listener(|this, _: &GroupSelection, window, cx| {
                let selection = this.editor.selection.clone();
                this.apply(window, cx, |e| e.group(&selection));
            }))
            .on_action(cx.listener(|this, _: &UngroupSelection, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.apply(window, cx, |e| e.ungroup(id));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleAutoLayout, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.apply(window, cx, |e| e.add_auto_layout(id));
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAllSiblings, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &EscapeSelection, _, cx| this.escape(cx)))
            .on_action(cx.listener(|this, _: &EnterSelection, window, cx| this.enter(window, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeft, w, cx| this.nudge(-1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeRight, w, cx| this.nudge(1.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeUp, w, cx| this.nudge(0.0, -1.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeDown, w, cx| this.nudge(0.0, 1.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeLeftBig, w, cx| this.nudge(-10.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeRightBig, w, cx| this.nudge(10.0, 0.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeUpBig, w, cx| this.nudge(0.0, -10.0, w, cx)))
            .on_action(cx.listener(|this, _: &NudgeDownBig, w, cx| this.nudge(0.0, 10.0, w, cx)))
            .on_action(cx.listener(|this, _: &ZoomToSelection, _, cx| this.zoom_selection(cx)))
            .on_action(cx.listener(|this, _: &BringForward, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.apply(window, cx, |e| e.reorder(id, 1));
                }
            }))
            .on_action(cx.listener(|this, _: &SendBackward, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.apply(window, cx, |e| e.reorder(id, -1));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleHidden, window, cx| {
                for id in this.editor.selection.clone() {
                    let hidden = this.editor.doc.get(id).is_some_and(|n| n.hidden);
                    this.apply(window, cx, |e| e.set_hidden(id, !hidden));
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleLocked, window, cx| {
                for id in this.editor.selection.clone() {
                    let locked = this.editor.doc.get(id).is_some_and(|n| n.locked);
                    this.apply(window, cx, |e| e.set_locked(id, !locked));
                }
            }))
            .on_action(cx.listener(|this, _: &PlaceImage, w, cx| this.prompt_place_image(w, cx)))
            .on_action(cx.listener(|this, _: &StartPresenting, w, cx| this.start_present(w, cx)))
            .on_action(cx.listener(|this, _: &ExportSelection, _, cx| {
                if let Some(id) = this.editor.primary() {
                    let (format, scale) = (this.export_format, this.export_scale);
                    if let Some(path) = this.export_path(id, format, scale) {
                        this.start_image_export(
                            id,
                            format,
                            scale,
                            super::image_export::ExportTarget::File(path),
                            cx,
                        );
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &CreateComponent, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.apply(window, cx, |e| e.create_component(id, None));
                }
            }))
            .on_action(cx.listener(|this, _: &DetachInstance, window, cx| {
                for id in this.editor.selection.clone() {
                    if let Some(root) = this.editor.doc.instance_root(id) {
                        this.apply(window, cx, |e| e.detach_instance(root));
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &AlignLeft, w, cx| this.align(Align::Left, w, cx)))
            .on_action(
                cx.listener(|this, _: &AlignHCenter, w, cx| this.align(Align::HCenter, w, cx)),
            )
            .on_action(cx.listener(|this, _: &AlignRight, w, cx| this.align(Align::Right, w, cx)))
            .on_action(cx.listener(|this, _: &AlignTop, w, cx| this.align(Align::Top, w, cx)))
            .on_action(
                cx.listener(|this, _: &AlignVCenter, w, cx| this.align(Align::VCenter, w, cx)),
            )
            .on_action(cx.listener(|this, _: &AlignBottom, w, cx| this.align(Align::Bottom, w, cx)))
            .on_action(cx.listener(|this, _: &DistributeHorizontal, w, cx| {
                this.distribute(Axis::Horizontal, w, cx);
            }))
            .on_action(cx.listener(|this, _: &DistributeVertical, w, cx| {
                this.distribute(Axis::Vertical, w, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameSelection, window, cx| {
                if let Some(id) = this.editor.primary() {
                    this.begin_rename(id, window, cx);
                }
            }))
            .child(probe)
            .children(artboards)
            .children(export_stage)
            .children(labels)
            .child(overlay)
            .children(size_label)
            .children(connection_labels)
            .children(presence_tags)
            .children(pins)
            .when(self.editor.doc.artboards().next().is_none(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(label_color)
                        .child("Press F and drag to draw your first frame, or paste HTML."),
                )
            })
            .into_any_element();
        // In-place editors sit outside the canvas key context so its
        // single-letter shortcuts do not swallow typing.
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(canvas_element)
            .children(text_editor)
            .children(draft)
            .into_any_element()
    }

    fn render_connection_labels(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let origin = self.canvas.origin();
        let theme = cx.theme().clone();
        self.connection_paths()
            .into_iter()
            .filter_map(|(id, points)| {
                let connection = self.editor.doc.connection(id)?;
                if connection.label.is_empty() {
                    return None;
                }
                let line: Vec<(f32, f32)> = points
                    .iter()
                    .map(|p| (p.x.as_f32(), p.y.as_f32()))
                    .collect();
                let (x, y) = midpoint(&line);
                let selected = self.selected_connection == Some(id);
                Some(
                    div()
                        .id(SharedString::from(format!("connection-label-{id}")))
                        .absolute()
                        .left(px(x) - origin.x)
                        .top(px(y) - origin.y - px(11.0))
                        .h(px(22.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(px(11.0))
                        .bg(theme.popover)
                        .border_1()
                        .border_color(if selected {
                            theme.primary
                        } else {
                            theme.border
                        })
                        .text_size(px(11.0))
                        .whitespace_nowrap()
                        .child(connection.label.clone())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected_connection = Some(id);
                            this.editor.select([]);
                            this.inspector.invalidate();
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn render_comment_pins(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(comments) = &self.editor.comments else {
            return Vec::new();
        };
        let origin = self.canvas.origin();
        let mut pins = Vec::new();
        for (index, comment) in comments.items.iter().enumerate() {
            if !comment.status.is_active() && !self.show_done_comments {
                continue;
            }
            let Some(id) = NodeId::parse(&comment.node) else {
                continue;
            };
            let Some(b) = self.canvas.window_bounds(id) else {
                continue;
            };
            let local = b.origin - origin;
            let x = local.x + b.size.width * comment.x;
            let y = local.y + b.size.height * comment.y;
            let active = self.hovered_comment == Some(comment.id);
            let done = !comment.status.is_active();
            let comment_id = comment.id;
            pins.push(
                div()
                    .id(SharedString::from(format!("comment-pin-{}", comment.id)))
                    .absolute()
                    .left(x - px(12.0))
                    .top(y - px(24.0))
                    .size(px(24.0))
                    .rounded_tl(px(12.0))
                    .rounded_tr(px(12.0))
                    .rounded_br(px(12.0))
                    .bg(if done {
                        gpui_kit::rgb(0x9ca3af)
                    } else {
                        gpui_kit::rgb(0xff5a36)
                    })
                    .border_2()
                    .border_color(if active {
                        gpui_kit::rgb(0x111111)
                    } else {
                        gpui_kit::rgb(0xffffff)
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.0))
                    .font_weight(gpui_kit::FontWeight::BOLD)
                    .text_color(gpui_kit::white())
                    .child(format!("{}", index + 1))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.hovered_comment = Some(comment_id);
                        this.right_tab = RightTab::Comments;
                        this.editor.select([id]);
                        this.inspector.invalidate();
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        pins
    }

    fn render_comment_draft(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, anchor, input) = self.comment_draft.as_ref()?;
        let b = self.canvas.window_bounds(*id)?;
        let local = b.origin - self.canvas.origin();
        let x = local.x + b.size.width * anchor.0;
        let y = local.y + b.size.height * anchor.1;
        let theme = cx.theme().clone();
        let input_for_post = input.clone();
        Some(
            div()
                .id("comment-draft")
                .absolute()
                .left(x + px(8.0))
                .top(y - px(8.0))
                .w(px(280.0))
                .p_2()
                .rounded(px(10.0))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(input).small()))
                        .child(
                            gpui_kit::component::button::Button::new("post-comment")
                                .label("Post")
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let body = input_for_post.read(cx).value().to_string();
                                    this.submit_comment(body, window, cx);
                                })),
                        ),
                )
                .into_any_element(),
        )
    }
}

fn paint_polyline(
    window: &mut Window,
    points: &[Point<Pixels>],
    width: Pixels,
    color: Hsla,
    dashed: bool,
) {
    if points.len() < 2 {
        return;
    }
    let mut builder = gpui_kit::PathBuilder::stroke(width);
    if dashed {
        builder = builder.dash_array(&[px(6.0), px(4.0)]);
    }
    builder.move_to(points[0]);
    for p in &points[1..] {
        builder.line_to(*p);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn paint_arrowhead(window: &mut Window, tip: Point<Pixels>, from: Point<Pixels>, color: Hsla) {
    let (dx, dy) = ((tip.x - from.x).as_f32(), (tip.y - from.y).as_f32());
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.5 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let (size, half) = (11.0, 5.5);
    let base = (tip.x.as_f32() - ux * size, tip.y.as_f32() - uy * size);
    let left = point(px(base.0 - uy * half), px(base.1 + ux * half));
    let right = point(px(base.0 + uy * half), px(base.1 - ux * half));
    let mut builder = gpui_kit::PathBuilder::fill();
    builder.move_to(tip);
    builder.line_to(left);
    builder.line_to(right);
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
