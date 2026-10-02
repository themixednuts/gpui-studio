//! One open project: canvas, side panels, and floating tools.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _, h_flex,
};
use gpui_kit::{
    AnyElement, App, Context, CursorStyle, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div, point, prelude::*, px,
};

use super::canvas::CanvasState;
use super::inspector::Inspector;
use super::paint::FontBook;
use crate::editor::{Editor, Tool};
use crate::export::CodeFormat;
use crate::geometry::{Align, Axis};
use crate::model::NodeId;
use crate::workspace::{PanelLayout, ViewState};

/// Which tab the left sidebar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftTab {
    Layers,
    Assets,
    Variables,
}

/// Which tab the right sidebar shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RightTab {
    Design,
    Prototype,
    Code,
    Comments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanelSide {
    Left,
    Right,
}

/// An open project's editor.
pub(crate) struct Studio {
    pub(crate) editor: Editor,
    pub(crate) tool: Tool,
    pub(crate) canvas: CanvasState,
    pub(crate) canvas_focus: FocusHandle,
    pub(crate) fonts: Rc<FontBook>,
    pub(crate) inspector: Inspector,
    pub(crate) left_tab: LeftTab,
    pub(crate) right_tab: RightTab,
    pub(crate) code_format: CodeFormat,
    pub(crate) collapsed: BTreeSet<NodeId>,
    pub(crate) rename: Option<(NodeId, Entity<InputState>)>,
    pub(crate) page_rename: Option<(usize, Entity<InputState>)>,
    pub(crate) comment_draft: Option<(NodeId, (f32, f32), Entity<InputState>)>,
    pub(crate) hovered_comment: Option<u64>,
    pub(crate) show_done_comments: bool,
    /// Connector selected on the canvas (separate from layer selection).
    pub(crate) selected_connection: Option<u64>,
    pub(crate) panels: PanelLayout,
    /// Variable being edited in the Variables tab: (name, editing the value?, input).
    pub(crate) variable_edit: Option<(String, bool, Entity<InputState>)>,
    /// Format and scale the Export section uses.
    pub(crate) export_format: crate::export_image::ImageFormat,
    pub(crate) export_scale: f32,
    /// An image export waiting for its 1× layout.
    pub(crate) export_job: Option<super::image_export::ExportJob>,
    /// Whether the chat panel is open.
    pub(crate) chat_open: bool,
    /// The chat composer.
    pub(crate) chat_input: Entity<InputState>,
    /// A running presentation.
    pub(crate) present: Option<super::present::Present>,
    /// Pending drop target while dragging a row in the layer list.
    pub(crate) layer_drop: Option<(NodeId, crate::editor::DropPosition)>,
    panel_drag: Option<(PanelSide, f32, f32)>,
    shown_status: String,
    pub(crate) _subscriptions: Vec<Subscription>,
}

impl Focusable for Studio {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.canvas_focus.clone()
    }
}

impl Studio {
    pub(crate) fn new(
        mut editor: Editor,
        view: Option<ViewState>,
        panels: PanelLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fonts = Rc::new(FontBook::new(cx.text_system().all_font_names()));
        let inspector = Inspector::new(window, cx);
        let canvas_focus = cx.focus_handle();
        let mut canvas = CanvasState::new();
        if let Some(view) = view {
            editor.page = view.page.min(editor.doc.pages.len().saturating_sub(1));
            canvas.restore(view.zoom, point(px(view.pan_x), px(view.pan_y)));
        }
        let (chat_input, chat_subscription) = super::chat::chat_input(window, cx);
        let studio = Self {
            editor,
            chat_open: false,
            chat_input,
            tool: Tool::Select,
            canvas,
            canvas_focus,
            fonts,
            inspector,
            left_tab: LeftTab::Layers,
            right_tab: RightTab::Design,
            code_format: CodeFormat::Html,
            collapsed: BTreeSet::new(),
            rename: None,
            page_rename: None,
            comment_draft: None,
            hovered_comment: None,
            show_done_comments: false,
            selected_connection: None,
            panels: panels.clamped(),
            layer_drop: None,
            present: None,
            export_job: None,
            export_format: crate::export_image::ImageFormat::Png,
            export_scale: 2.0,
            variable_edit: None,
            panel_drag: None,
            shown_status: String::new(),
            _subscriptions: vec![chat_subscription],
        };
        studio.start_background_loop(window, cx);
        studio
    }

    /// Display name of the project.
    pub(crate) fn name(&self) -> String {
        self.editor
            .project
            .as_ref()
            .map_or_else(|| "Untitled".to_owned(), |p| p.name.clone())
    }

    /// Camera and page, for the workspace file.
    pub(crate) fn view_state(&self) -> ViewState {
        ViewState {
            page: self.editor.page,
            zoom: self.canvas.camera.zoom,
            pan_x: self.canvas.camera.pan.x.as_f32(),
            pan_y: self.canvas.camera.pan.y.as_f32(),
        }
    }

    fn start_background_loop(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let alive = this.update_in(cx, |this, window, cx| {
                    let saved = this.editor.autosave();
                    let reloaded = this.editor.sync_external();
                    if reloaded {
                        this.inspector.invalidate();
                    }
                    if saved || reloaded {
                        cx.notify();
                    }
                    this.flush_status(window, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn flush_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.status != self.shown_status {
            self.shown_status = self.editor.status.clone();
            if self.shown_status.contains("failed") {
                window.push_notification(Notification::error(self.shown_status.clone()), cx);
            }
        }
    }

    /// Run an editor operation from the UI, reporting failures.
    pub(crate) fn apply<R>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        op: impl FnOnce(&mut Editor) -> anyhow::Result<R>,
    ) -> Option<R> {
        self.editor.measured = self.canvas.measured();
        let result = op(&mut self.editor);
        self.inspector.invalidate();
        cx.notify();
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                window.push_notification(Notification::warning(format!("{error:#}")), cx);
                None
            }
        }
    }

    /// Align the selection (to each other, or one layer to its parent).
    pub(crate) fn align(&mut self, align: Align, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.editor.selection.clone();
        self.apply(window, cx, |e| e.align(&selection, align));
    }

    /// Space the selection evenly.
    pub(crate) fn distribute(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.editor.selection.clone();
        self.apply(window, cx, |e| e.distribute(&selection, axis));
    }

    /// Pick image files and place them into the selection (or as artboards).
    pub(crate) fn prompt_place_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Place image".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    let images = Self::read_image_files(&paths);
                    this.import_images(images, None, window, cx);
                });
            }
        })
        .detach();
    }

    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.tool = tool;
        self.canvas.cancel_drag();
        cx.notify();
    }

    pub(crate) fn insert_component(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.editor.insertion_target();
        self.apply(window, cx, |e| e.insert_component(key, target));
        self.canvas_focus.focus(window, cx);
    }

    /// Show or hide both sidebars.
    pub(crate) fn toggle_panels(&mut self, cx: &mut Context<Self>) {
        self.panels.panels_visible = !self.panels.panels_visible;
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let current = self.tool;
        let tool = |id: &'static str, icon: Lucide, tool: Tool, cx: &mut Context<Self>| {
            Button::new(id)
                .icon(Icon::new(icon))
                .ghost()
                .small()
                .selected(current == tool)
                .tooltip(tool.label())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_tool(tool, cx);
                    this.canvas_focus.focus(window, cx);
                }))
        };
        let divider = || div().w(px(1.0)).h(px(18.0)).mx_1().bg(theme.border);
        let insert = cx.entity();
        h_flex()
            .id("toolbar")
            .occlude()
            .gap_0p5()
            .p_1()
            .rounded(px(12.0))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .child(tool("tool-select", Lucide::MousePointer2, Tool::Select, cx))
            .child(tool("tool-hand", Lucide::Hand, Tool::Hand, cx))
            .child(divider())
            .child(tool("tool-frame", Lucide::Frame, Tool::Frame, cx))
            .child(tool("tool-rect", Lucide::Square, Tool::Rectangle, cx))
            .child(tool("tool-ellipse", Lucide::Circle, Tool::Ellipse, cx))
            .child(tool("tool-text", Lucide::Type, Tool::Text, cx))
            .child(divider())
            .child(tool("tool-pencil", Lucide::Pencil, Tool::Pencil, cx))
            .child(tool("tool-line", Lucide::Slash, Tool::Line, cx))
            .child(tool("tool-arrow", Lucide::MoveUpRight, Tool::Arrow, cx))
            .child(tool("tool-connector", Lucide::Spline, Tool::Connector, cx))
            .child(divider())
            .child(
                Button::new("tool-image")
                    .icon(Icon::new(Lucide::Image))
                    .ghost()
                    .small()
                    .tooltip("Place image… (⇧⌘K)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.prompt_place_image(window, cx)),
                    ),
            )
            .child(tool(
                "tool-comment",
                Lucide::MessageSquare,
                Tool::Comment,
                cx,
            ))
            .child(
                Button::new("insert-menu")
                    .icon(Icon::new(Lucide::Component))
                    .ghost()
                    .small()
                    .tooltip("Insert component")
                    .dropdown_menu_with_anchor(
                        gpui_kit::Anchor::BottomCenter,
                        move |mut menu, _, _| {
                            for component in crate::presets::COMPONENTS {
                                let entity = insert.clone();
                                menu = menu.item(PopupMenuItem::new(component.name).on_click(
                                    move |_, window, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.insert_component(component.key, window, cx);
                                        });
                                    },
                                ));
                            }
                            menu.max_h(px(420.0)).scrollable(true)
                        },
                    ),
            )
            .into_any_element()
    }

    fn render_view_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let zoom = format!("{:.0}%", self.canvas.camera.zoom * 100.0);
        let entity = cx.entity();
        h_flex()
            .id("view-controls")
            .occlude()
            .gap_0p5()
            .p_1()
            .rounded(px(10.0))
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_md()
            .child(
                Button::new("present")
                    .icon(Icon::new(Lucide::Play))
                    .ghost()
                    .xsmall()
                    .tooltip("Present (⌥⌘↵)")
                    .on_click(cx.listener(|this, _, window, cx| this.start_present(window, cx))),
            )
            .child(
                Button::new("undo")
                    .icon(Icon::new(Lucide::Undo2))
                    .ghost()
                    .xsmall()
                    .disabled(!self.editor.history.can_undo())
                    .tooltip("Undo")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor.undo();
                        this.inspector.invalidate();
                        cx.notify();
                    })),
            )
            .child(
                Button::new("redo")
                    .icon(Icon::new(Lucide::Redo2))
                    .ghost()
                    .xsmall()
                    .disabled(!self.editor.history.can_redo())
                    .tooltip("Redo")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.editor.redo();
                        this.inspector.invalidate();
                        cx.notify();
                    })),
            )
            .child(div().w(px(1.0)).h(px(14.0)).mx_0p5().bg(theme.border))
            .child(
                Button::new("zoom-out")
                    .icon(Icon::new(Lucide::Minus))
                    .ghost()
                    .xsmall()
                    .tooltip("Zoom out")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0.8, cx))),
            )
            .child(
                Button::new("zoom-menu")
                    .ghost()
                    .xsmall()
                    .label(zoom)
                    .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomRight, move |menu, _, _| {
                        let reset = entity.clone();
                        let fit = entity.clone();
                        let selection = entity.clone();
                        menu.item(
                            PopupMenuItem::new("Zoom to 100%").on_click(move |_, _, cx| {
                                reset.update(cx, |this, cx| this.zoom_reset(cx));
                            }),
                        )
                        .item(PopupMenuItem::new("Zoom to fit").on_click(move |_, _, cx| {
                            fit.update(cx, |this, cx| this.zoom_fit(cx));
                        }))
                        .item(
                            PopupMenuItem::new("Zoom to selection").on_click(move |_, _, cx| {
                                selection.update(cx, |this, cx| this.zoom_selection(cx));
                            }),
                        )
                    }),
            )
            .child(
                Button::new("zoom-in")
                    .icon(Icon::new(Lucide::Plus))
                    .ghost()
                    .xsmall()
                    .tooltip("Zoom in")
                    .on_click(cx.listener(|this, _, _, cx| this.zoom_by(1.25, cx))),
            )
            .into_any_element()
    }

    fn resize_handle(&self, side: PanelSide, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let active = self.panel_drag.is_some_and(|(s, _, _)| s == side);
        div()
            .id(match side {
                PanelSide::Left => "resize-left",
                PanelSide::Right => "resize-right",
            })
            .w(px(5.0))
            .h_full()
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeLeftRight)
            .flex()
            .justify_center()
            .child(
                div()
                    .w(px(1.0))
                    .h_full()
                    .bg(if active { theme.primary } else { theme.border }),
            )
            .hover(|this| this.bg(theme.primary.opacity(0.15)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let width = match side {
                        PanelSide::Left => this.panels.left_width,
                        PanelSide::Right => this.panels.right_width,
                    };
                    this.panel_drag = Some((side, event.position.x.as_f32(), width));
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.present.is_some() {
            return self.render_present(window, cx);
        }
        self.sync_inspector(window, cx);
        if self.layer_drop.is_some() && !cx.has_active_drag() {
            self.layer_drop = None;
        }
        let visible = self.panels.panels_visible;
        let left = visible.then(|| self.render_left_panel(window, cx));
        let right = visible.then(|| self.render_right_panel(window, cx));
        let canvas = self.render_canvas(window, cx);
        let toolbar = self.render_toolbar(cx);
        let chat = self.render_chat(cx);
        let view_controls = self.render_view_controls(cx);
        let left_handle = visible.then(|| self.resize_handle(PanelSide::Left, cx));
        let right_handle = visible.then(|| self.resize_handle(PanelSide::Right, cx));
        let (left_width, right_width) = (self.panels.left_width, self.panels.right_width);
        h_flex()
            .id("studio-body")
            .size_full()
            .items_stretch()
            .when(self.panel_drag.is_some(), |this| {
                this.cursor(CursorStyle::ResizeLeftRight)
            })
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some((side, start, width)) = this.panel_drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.panel_drag = None;
                    cx.notify();
                    return;
                }
                let delta = event.position.x.as_f32() - start;
                match side {
                    PanelSide::Left => this.panels.left_width = width + delta,
                    PanelSide::Right => this.panels.right_width = width - delta,
                }
                this.panels = this.panels.clamped();
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.panel_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .children(left.map(|panel| {
                div()
                    .w(px(left_width))
                    .flex_shrink_0()
                    .h_full()
                    .child(panel)
            }))
            .children(left_handle)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(canvas)
                    .children(chat.map(|panel| {
                        div()
                            .absolute()
                            .top(px(12.0))
                            .right(px(12.0))
                            .bottom(px(64.0))
                            .child(panel)
                    }))
                    .child(
                        div()
                            .absolute()
                            .bottom(px(16.0))
                            .left_0()
                            .right_0()
                            .flex()
                            .justify_center()
                            .child(toolbar),
                    )
                    .child(
                        div()
                            .absolute()
                            .bottom(px(16.0))
                            .right(px(16.0))
                            .child(view_controls),
                    ),
            )
            .children(right_handle)
            .children(right.map(|panel| {
                div()
                    .w(px(right_width))
                    .flex_shrink_0()
                    .h_full()
                    .child(panel)
            }))
            .into_any_element()
    }
}

/// Label for a tab, used by the workspace.
pub(crate) fn tab_label(studio: &Studio) -> SharedString {
    studio.name().into()
}
