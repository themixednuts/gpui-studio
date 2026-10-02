//! Prototype mode: the Prototype panel, flow arrows, and the click-through
//! presentation view.

use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, CursorStyle, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, SharedString, Styled as _, Window, div, prelude::*, px,
};

use super::Studio;
use super::paint::{LayoutMap, Painter};
use crate::model::NodeId;
use crate::model::prototype::{Link, LinkTarget, Transition};

/// Green used for prototype flows.
pub(crate) const FLOW_COLOR: gpui_kit::Hsla = gpui_kit::Hsla {
    h: 0.40,
    s: 0.79,
    l: 0.38,
    a: 1.0,
};

/// A running presentation.
pub(crate) struct Present {
    /// Artboard on screen.
    pub(crate) current: NodeId,
    history: Vec<NodeId>,
    layout: LayoutMap,
    transition: Transition,
    /// Bumps on every navigation to restart the transition animation.
    generation: usize,
    /// Set when a click misses every hotspot, to flash them briefly.
    flash: bool,
    hovering_link: bool,
    focus: FocusHandle,
}

impl Studio {
    /// Start presenting from the selected artboard (or the page's first).
    pub(crate) fn start_present(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = &self.editor.doc;
        let start = self
            .editor
            .primary()
            .and_then(|id| doc.artboard_of(id).map(|a| a.root))
            .or_else(|| {
                doc.pages
                    .get(self.editor.page)
                    .and_then(|p| p.artboards.first())
                    .map(|a| a.root)
            });
        let Some(start) = start else {
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.canvas.cancel_drag();
        self.present = Some(Present {
            current: start,
            history: Vec::new(),
            layout: LayoutMap::default(),
            transition: Transition::Instant,
            generation: 0,
            flash: false,
            hovering_link: false,
            focus,
        });
        cx.notify();
    }

    fn stop_present(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(present) = self.present.take() {
            self.editor.select([present.current]);
            self.reveal(present.current, cx);
        }
        self.canvas_focus.focus(window, cx);
        cx.notify();
    }

    fn present_go(&mut self, target: LinkTarget, transition: Transition, cx: &mut Context<Self>) {
        let Some(present) = &mut self.present else {
            return;
        };
        match target {
            LinkTarget::Artboard(board) if board != present.current => {
                present.history.push(present.current);
                present.current = board;
                present.transition = transition;
            }
            LinkTarget::Back => {
                let Some(previous) = present.history.pop() else {
                    return;
                };
                present.current = previous;
                present.transition = transition.reversed();
            }
            LinkTarget::Artboard(_) => return,
        }
        present.generation += 1;
        present.hovering_link = false;
        cx.notify();
    }

    /// Step through the page's artboards in order (arrow keys).
    fn present_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(present) = &self.present else {
            return;
        };
        let Some((page, index)) = self.editor.doc.artboard_index(present.current) else {
            return;
        };
        let boards = &self.editor.doc.pages[page].artboards;
        let next = index as isize + delta;
        if next < 0 || next as usize >= boards.len() {
            return;
        }
        let target = boards[next as usize].root;
        let transition = if delta > 0 {
            Transition::SlideLeft
        } else {
            Transition::SlideRight
        };
        self.present_go(LinkTarget::Artboard(target), transition, cx);
    }

    /// The deepest laid-out layer under a window point in the presentation.
    fn present_hit(&self, position: gpui_kit::Point<gpui_kit::Pixels>) -> Option<NodeId> {
        let present = self.present.as_ref()?;
        let doc = &self.editor.doc;
        let layout = present.layout.borrow();
        doc.descendants(present.current)
            .into_iter()
            .filter(|id| layout.get(id).is_some_and(|b| b.contains(&position)))
            .max_by_key(|id| doc.ancestors(*id).len())
    }

    fn present_link_at(&self, position: gpui_kit::Point<gpui_kit::Pixels>) -> Option<Link> {
        self.present_hit(position)
            .and_then(|id| self.editor.doc.link_at(id))
    }

    pub(crate) fn render_present(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(present) = &self.present else {
            return div().into_any_element();
        };
        let doc = &self.editor.doc;
        if !doc.is_artboard(present.current) {
            self.present = None;
            return div().into_any_element();
        }
        let current = present.current;
        let viewport = window.viewport_size();
        let (w, h) = self
            .canvas
            .doc_rect(current)
            .map(|r| (r.w, r.h))
            .or_else(|| {
                let c = doc.computed(doc.get(current)?);
                Some((c.width.px()?, c.height.px().unwrap_or(800.0)))
            })
            .unwrap_or((800.0, 600.0));
        let zoom = ((viewport.width.as_f32() - 96.0) / w)
            .min((viewport.height.as_f32() - 160.0) / h)
            .clamp(0.1, 1.0);
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
        let measured = present.layout.borrow().clone();
        let painter = Painter {
            doc,
            zoom,
            asset_dir: &asset_dir,
            cache_dir: &cache_dir,
            layout: &present.layout,
            fonts: &self.fonts,
            editing: None,
            unsettled: std::cell::Cell::new(false),
            measured: &measured,
        };
        let screen = painter.artboard(current);
        if painter.unsettled.get() {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let transition = present.transition;
        let generation = present.generation;
        let width = w * zoom;
        let stage = div()
            .relative()
            .shadow_lg()
            .children(screen)
            .with_animation(
                SharedString::from(format!("present-{generation}")),
                Animation::new(Duration::from_millis(
                    if transition == Transition::Instant {
                        1
                    } else {
                        260
                    },
                ))
                .with_easing(gpui_kit::ease_out_quint()),
                move |el, delta| match transition {
                    Transition::Instant => el,
                    Transition::Dissolve => el.opacity(delta),
                    Transition::SlideLeft => {
                        el.left(px((1.0 - delta) * width * 0.35)).opacity(delta)
                    }
                    Transition::SlideRight => {
                        el.left(px(-(1.0 - delta) * width * 0.35)).opacity(delta)
                    }
                },
            );
        // Hotspots flash briefly when a click hits nothing clickable. Bounds
        // are window coordinates, so they are painted directly.
        let hotspots: Vec<gpui_kit::Bounds<gpui_kit::Pixels>> = if present.flash {
            let layout = present.layout.borrow();
            doc.descendants(current)
                .into_iter()
                .filter(|id| doc.link_of(*id).is_some())
                .filter_map(|id| layout.get(&id).copied())
                .collect()
        } else {
            Vec::new()
        };
        let hotspot_layer = gpui_kit::canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                for b in &hotspots {
                    window.paint_quad(gpui_kit::quad(
                        *b,
                        px(4.0),
                        FLOW_COLOR.opacity(0.18),
                        px(2.0),
                        FLOW_COLOR,
                        gpui_kit::BorderStyle::Solid,
                    ));
                }
            },
        )
        .absolute()
        .size_full();
        let name = doc.display_name(current);
        let can_back = !present.history.is_empty();
        let hovering = present.hovering_link;
        let focus = present.focus.clone();
        // The bar is always dark, so its buttons don't use the theme colors.
        let bar_button = |id: &'static str, icon: Lucide, tooltip: &'static str, enabled: bool| {
            div()
                .id(id)
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .text_color(gpui_kit::rgb(if enabled { 0xe4e4e7 } else { 0x52525b }))
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|s| s.bg(gpui_kit::rgb(0x27272a)))
                })
                .child(Icon::new(icon).small())
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip).build(window, cx)
                })
        };
        v_flex()
            .id("present")
            .relative()
            .key_context("Present")
            .track_focus(&focus)
            .size_full()
            .bg(gpui_kit::rgb(0x18181b))
            .text_color(gpui_kit::rgb(0xe4e4e7))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.stop_present(window, cx),
                    "left" | "backspace" => {
                        if this.present.as_ref().is_some_and(|p| !p.history.is_empty()) {
                            this.present_go(LinkTarget::Back, Transition::SlideLeft, cx);
                        } else {
                            this.present_step(-1, cx);
                        }
                    }
                    "right" | "space" => this.present_step(1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .h(px(44.0))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(gpui_kit::rgb(0x27272a))
                    .child(
                        bar_button("present-back", Lucide::ArrowLeft, "Back (←)", can_back)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.present_go(LinkTarget::Back, Transition::SlideLeft, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(gpui_kit::rgb(0x71717a))
                            .child("Click hotspots · ← → step · Esc exit"),
                    )
                    .child(
                        bar_button("present-exit", Lucide::X, "Exit (Esc)", true).on_click(
                            cx.listener(|this, _, window, cx| this.stop_present(window, cx)),
                        ),
                    ),
            )
            .child(
                div()
                    .id("present-stage")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .when(hovering, |this| this.cursor(CursorStyle::PointingHand))
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        let hovering = this.present_link_at(event.position).is_some();
                        if let Some(present) = &mut this.present
                            && present.hovering_link != hovering
                        {
                            present.hovering_link = hovering;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            match this.present_link_at(event.position) {
                                Some(link) => this.present_go(link.target, link.transition, cx),
                                None => {
                                    if let Some(present) = &mut this.present {
                                        present.flash = true;
                                        cx.notify();
                                        cx.spawn(async move |this, cx| {
                                            cx.background_executor()
                                                .timer(Duration::from_millis(650))
                                                .await;
                                            let _ = this.update(cx, |this, cx| {
                                                if let Some(present) = &mut this.present {
                                                    present.flash = false;
                                                    cx.notify();
                                                }
                                            });
                                        })
                                        .detach();
                                    }
                                }
                            }
                        }),
                    )
                    .child(stage),
            )
            .child(hotspot_layer)
            .into_any_element()
    }

    /// The Prototype tab: the selection's click link and the presentation.
    pub(crate) fn render_prototype_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = &self.editor.doc;
        let links = doc.page_links(self.editor.page).len();
        let mut panel = v_flex().pb_8().child(
            v_flex()
                .px_3()
                .py_2p5()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    Button::new("present-start")
                        .icon(Icon::new(Lucide::Play))
                        .label("Present")
                        .small()
                        .primary()
                        .w_full()
                        .on_click(cx.listener(|this, _, window, cx| this.start_present(window, cx))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(match links {
                            0 => "No links on this page yet. Select a layer and choose where a click goes.".to_owned(),
                            1 => "1 link on this page.".to_owned(),
                            n => format!("{n} links on this page."),
                        }),
                ),
        );
        let Some(id) = self
            .editor
            .primary()
            .filter(|id| doc.get(*id).is_some_and(|n| !n.is_text()))
        else {
            return panel.into_any_element();
        };
        let link = doc.link_of(id);
        let own_board = doc.artboard_of(id).map(|a| a.root);
        let boards: Vec<(NodeId, String)> = doc
            .pages
            .get(self.editor.page)
            .map(|p| {
                p.artboards
                    .iter()
                    .filter(|a| Some(a.root) != own_board)
                    .map(|a| (a.root, doc.display_name(a.root)))
                    .collect()
            })
            .unwrap_or_default();
        let transition = link.map(|l| l.transition).unwrap_or_default();
        let current = match link.map(|l| l.target) {
            None => "None".to_owned(),
            Some(LinkTarget::Back) => "Back".to_owned(),
            Some(LinkTarget::Artboard(board)) => doc.display_name(board),
        };
        let entity = cx.entity();
        let target_menu = Button::new("link-target")
            .small()
            .outline()
            .w_full()
            .label(current)
            .dropdown_menu(move |mut menu, _, _| {
                let set = |target: Option<LinkTarget>| {
                    let entity = entity.clone();
                    move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                        entity.update(cx, |this, cx| {
                            this.apply(window, cx, |e| e.set_link(id, target, transition));
                        });
                    }
                };
                menu = menu
                    .item(PopupMenuItem::new("None").on_click(set(None)))
                    .item(PopupMenuItem::new("Back").on_click(set(Some(LinkTarget::Back))))
                    .separator();
                for (board, name) in &boards {
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .on_click(set(Some(LinkTarget::Artboard(*board)))),
                    );
                }
                menu.max_h(px(360.0)).scrollable(true)
            });
        let mut transitions = h_flex()
            .gap_0p5()
            .p_0p5()
            .rounded(px(7.0))
            .bg(theme.secondary);
        for option in Transition::ALL {
            let selected = option == transition;
            transitions = transitions.child(
                div()
                    .id(SharedString::from(format!(
                        "transition-{}",
                        option.as_str()
                    )))
                    .flex_1()
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |this| this.bg(theme.background).shadow_xs())
                    .when(!selected, |this| this.text_color(theme.muted_foreground))
                    .child(option.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let target = this.editor.doc.link_of(id).map(|l| l.target);
                        if target.is_some() {
                            this.apply(window, cx, |e| e.set_link(id, target, option));
                        }
                    })),
            );
        }
        panel = panel.child(
            v_flex()
                .px_3()
                .py_2p5()
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("On click · {}", doc.display_name(id))),
                )
                .child(target_menu)
                .when(link.is_some(), |this| this.child(transitions)),
        );
        panel.into_any_element()
    }
}
