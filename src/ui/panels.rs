//! Left sidebar (pages, layers, assets) and right sidebar (design, code, comments).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, ClickEvent, ClipboardItem, Context, DragMoveEvent, FontWeight,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};

use super::{LeftTab, RightTab, Studio};
use crate::comments::CommentStatus;
use crate::editor::DropPosition;
use crate::export::{CodeFormat, export};
use crate::model::style::Display;
use crate::model::{NodeId, NodeKind};
use crate::presets::{ARTBOARD_PRESETS, COMPONENTS};

/// A layer row being dragged in the layer list.
#[derive(Clone)]
pub(crate) struct DraggedLayer {
    id: NodeId,
    name: SharedString,
}

impl Render for DraggedLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(px(6.0))
            .bg(theme.background)
            .border_1()
            .border_color(theme.primary)
            .shadow_md()
            .text_xs()
            .child(self.name.clone())
    }
}

/// Purple used for components and instances, as in other design tools.
pub(crate) const COMPONENT_COLOR: gpui_kit::Hsla = gpui_kit::Hsla {
    h: 0.736,
    s: 1.0,
    l: 0.65,
    a: 1.0,
};

struct LayerRow {
    id: NodeId,
    depth: usize,
    has_children: bool,
}

impl Studio {
    fn layer_rows(&self) -> Vec<LayerRow> {
        let mut rows = Vec::new();
        let Some(page) = self.editor.doc.pages.get(self.editor.page) else {
            return rows;
        };
        for artboard in &page.artboards {
            self.push_rows(artboard.root, 0, &mut rows);
        }
        rows
    }

    fn visible_children(&self, id: NodeId) -> Vec<NodeId> {
        let doc = &self.editor.doc;
        if doc.is_text_layer(id) {
            return Vec::new();
        }
        doc.children(id)
            .iter()
            .copied()
            .filter(|c| doc.get(*c).is_some_and(|n| !n.is_text()))
            .collect()
    }

    fn push_rows(&self, id: NodeId, depth: usize, rows: &mut Vec<LayerRow>) {
        let children = self.visible_children(id);
        rows.push(LayerRow {
            id,
            depth,
            has_children: !children.is_empty(),
        });
        if !self.collapsed.contains(&id) {
            // Document order: matches how HTML flows top to bottom.
            for child in children {
                self.push_rows(child, depth + 1, rows);
            }
        }
    }

    fn layer_icon(&self, id: NodeId) -> Lucide {
        let doc = &self.editor.doc;
        let Some(node) = doc.get(id) else {
            return Lucide::Square;
        };
        if doc.component_name(id).is_some() {
            return Lucide::Component;
        }
        if node.attr(crate::model::components::INSTANCE_ATTR).is_some() {
            return Lucide::Diamond;
        }
        if doc.is_artboard(id) {
            return Lucide::Frame;
        }
        match &node.kind {
            NodeKind::Svg(_) => Lucide::Shapes,
            NodeKind::Text(_) => Lucide::Type,
            NodeKind::Element { tag } => {
                if tag == "img" {
                    return Lucide::Image;
                }
                if doc.is_text_layer(id) {
                    return Lucide::Type;
                }
                let computed = self.editor.doc.computed(node);
                match computed.display {
                    Display::Flex if computed.direction.is_column() => Lucide::Rows3,
                    Display::Flex => Lucide::Columns3,
                    Display::Grid => Lucide::Grid2x2,
                    _ if node.children.is_empty() => Lucide::Square,
                    _ => Lucide::SquareDashed,
                }
            }
        }
    }

    pub(super) fn begin_rename(&mut self, id: NodeId, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.editor.doc.display_name(id);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
                    && this.rename.as_ref().is_some_and(|(rid, _)| *rid == id)
                {
                    let value = input.read(cx).value().to_string();
                    this.rename = None;
                    if value != this.editor.doc.display_name(id) {
                        this.apply(window, cx, |e| e.rename(id, &value));
                    }
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.canvas_focus.focus(window, cx);
                    }
                    cx.notify();
                }
            },
        );
        self._subscriptions.push(subscription);
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.rename = Some((id, input));
        cx.notify();
    }

    fn begin_page_rename(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.editor.doc.pages.get(page).map(|p| p.name.clone()) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur)
                    && this.page_rename.as_ref().is_some_and(|(p, _)| *p == page)
                {
                    let value = input.read(cx).value().trim().to_string();
                    this.page_rename = None;
                    if !value.is_empty() {
                        this.apply(window, cx, |e| e.rename_page(page, &value));
                    }
                    cx.notify();
                }
            },
        );
        self._subscriptions.push(subscription);
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.page_rename = Some((page, input));
        cx.notify();
    }

    fn render_pages(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut list = v_flex().gap_px();
        for (index, page) in self.editor.doc.pages.iter().enumerate() {
            let current = index == self.editor.page;
            let renaming = self
                .page_rename
                .as_ref()
                .filter(|(p, _)| *p == index)
                .map(|(_, i)| i.clone());
            let can_delete = self.editor.doc.pages.len() > 1;
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("page-{index}")))
                    .group("page-row")
                    .h(px(26.0))
                    .px_2()
                    .gap_2()
                    .rounded(px(6.0))
                    .when(current, |this| this.bg(theme.secondary))
                    .hover(|this| this.bg(theme.secondary.opacity(0.7)))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .when(current, |this| this.bg(theme.primary)),
                    )
                    .child(match renaming {
                        Some(input) => div()
                            .flex_1()
                            .child(Input::new(&input).xsmall())
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .truncate()
                            .when(current, |this| this.font_weight(FontWeight::MEDIUM))
                            .child(page.name.clone())
                            .into_any_element(),
                    })
                    .when(can_delete, |this| {
                        this.child(
                            div()
                                .invisible()
                                .group_hover("page-row", |s| s.visible())
                                .child(
                                    Button::new(SharedString::from(format!("delete-page-{index}")))
                                        .icon(Icon::new(Lucide::Trash))
                                        .ghost()
                                        .xsmall()
                                        .tooltip("Delete page")
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.apply(window, cx, |e| e.delete_page(index));
                                            this.canvas.needs_fit_page();
                                        })),
                                ),
                        )
                    })
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.begin_page_rename(index, window, cx);
                            return;
                        }
                        if this.editor.page != index {
                            this.editor.page = index;
                            this.editor.select([]);
                            this.canvas.needs_fit_page();
                            this.inspector.invalidate();
                        }
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .px_2()
            .pb_2()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .px_2()
                    .h(px(28.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Pages"),
                    )
                    .child(
                        Button::new("add-page")
                            .icon(Icon::new(Lucide::Plus))
                            .ghost()
                            .xsmall()
                            .tooltip("Add page")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply(window, cx, |e| e.add_page());
                                this.canvas.needs_fit_page();
                            })),
                    ),
            )
            .child(list)
    }

    fn render_layers(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let rows = self.layer_rows();
        if rows.is_empty() {
            return div()
                .p_4()
                .text_color(theme.muted_foreground)
                .child("No layers on this page yet. Press F to draw a frame.")
                .into_any_element();
        }
        let mut list = v_flex().py_1().gap_px();
        for row in rows {
            let id = row.id;
            let node = self.editor.doc.get(id);
            let hidden = node.is_some_and(|n| n.hidden);
            let locked = node.is_some_and(|n| n.locked);
            let selected = self.editor.selection.contains(&id);
            let hovered = self.canvas_hover() == Some(id);
            let is_artboard = self.editor.doc.is_artboard(id);
            let collapsed = self.collapsed.contains(&id);
            let renaming = self
                .rename
                .as_ref()
                .filter(|(rid, _)| *rid == id)
                .map(|(_, input)| input.clone());
            let group: SharedString = format!("layer-{id}").into();
            let renaming_none = renaming.is_none();
            let componentish = self.editor.doc.component_name(id).is_some()
                || node.is_some_and(|n| n.attr(crate::model::components::INSTANCE_ATTR).is_some());
            let drop_here = self
                .layer_drop
                .filter(|(target, _)| *target == id)
                .map(|(_, position)| position);
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("layer-row-{id}")))
                    .relative()
                    .group(group.clone())
                    .h(px(26.0))
                    .mx_1p5()
                    .rounded(px(6.0))
                    .pl(px(4.0 + row.depth as f32 * 12.0))
                    .pr_1()
                    .gap_1()
                    .when(selected, |this| this.bg(theme.primary.opacity(0.14)))
                    .when(!selected && hovered, |this| this.bg(theme.secondary))
                    .hover(|this| {
                        this.bg(if selected {
                            theme.primary.opacity(0.18)
                        } else {
                            theme.secondary
                        })
                    })
                    .when(hidden, |this| this.opacity(0.5))
                    .child(
                        div()
                            .id(SharedString::from(format!("disclosure-{id}")))
                            .size(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(row.has_children, |this| {
                                this.child(
                                    Icon::new(if collapsed {
                                        Lucide::ChevronRight
                                    } else {
                                        Lucide::ChevronDown
                                    })
                                    .xsmall()
                                    .text_color(theme.muted_foreground),
                                )
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if !this.collapsed.remove(&id) {
                                            this.collapsed.insert(id);
                                        }
                                        cx.notify();
                                    },
                                ))
                            }),
                    )
                    .child(
                        Icon::new(self.layer_icon(id))
                            .xsmall()
                            .text_color(if componentish {
                                COMPONENT_COLOR
                            } else if is_artboard {
                                theme.primary
                            } else {
                                theme.muted_foreground
                            }),
                    )
                    .child(match renaming {
                        Some(input) => div()
                            .flex_1()
                            .child(Input::new(&input).xsmall())
                            .into_any_element(),
                        None => div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(componentish, |this| this.text_color(COMPONENT_COLOR))
                            .when(is_artboard, |this| this.font_weight(FontWeight::MEDIUM))
                            .child(self.editor.doc.display_name(id))
                            .into_any_element(),
                    })
                    .child(
                        h_flex()
                            .gap_0p5()
                            .when(!hidden && !locked, |this| {
                                this.invisible().group_hover(group.clone(), |s| s.visible())
                            })
                            .child(
                                Button::new(SharedString::from(format!("lock-{id}")))
                                    .icon(Icon::new(if locked {
                                        Lucide::Lock
                                    } else {
                                        Lucide::LockOpen
                                    }))
                                    .ghost()
                                    .xsmall()
                                    .tooltip(if locked { "Unlock" } else { "Lock" })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply(window, cx, |e| e.set_locked(id, !locked));
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("hide-{id}")))
                                    .icon(Icon::new(if hidden {
                                        Lucide::EyeOff
                                    } else {
                                        Lucide::Eye
                                    }))
                                    .ghost()
                                    .xsmall()
                                    .tooltip(if hidden { "Show" } else { "Hide" })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply(window, cx, |e| e.set_hidden(id, !hidden));
                                    })),
                            ),
                    )
                    .when(renaming_none, |this| {
                        this.on_drag(
                            DraggedLayer {
                                id,
                                name: self.editor.doc.display_name(id).into(),
                            },
                            |dragged, _, _, cx| cx.new(|_| dragged.clone()),
                        )
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedLayer>, _, cx| {
                            if !event.bounds.contains(&event.event.position) {
                                return;
                            }
                            let dragged = event.drag(cx).id;
                            let fraction = ((event.event.position.y - event.bounds.origin.y)
                                / event.bounds.size.height)
                                .clamp(0.0, 1.0);
                            let inside_ok =
                                this.editor
                                    .can_drop_layer(dragged, id, DropPosition::Inside);
                            let position = if inside_ok && (0.25..=0.75).contains(&fraction) {
                                DropPosition::Inside
                            } else if fraction < 0.5 {
                                DropPosition::Before
                            } else {
                                DropPosition::After
                            };
                            let next = this
                                .editor
                                .can_drop_layer(dragged, id, position)
                                .then_some((id, position));
                            if this.layer_drop != next {
                                this.layer_drop = next;
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(
                        cx.listener(move |this, dragged: &DraggedLayer, window, cx| {
                            let Some((target, position)) = this.layer_drop.take() else {
                                return;
                            };
                            let dragged = dragged.id;
                            this.apply(window, cx, |e| e.drop_layer(dragged, target, position));
                        }),
                    )
                    .when_some(drop_here, |this, position| {
                        let line = |top: bool| {
                            div()
                                .absolute()
                                .left(px(4.0 + row.depth as f32 * 12.0))
                                .right_0()
                                .h(px(2.0))
                                .rounded(px(1.0))
                                .bg(theme.primary)
                                .when(top, |l| l.top(px(-1.0)))
                                .when(!top, |l| l.bottom(px(-1.0)))
                        };
                        match position {
                            DropPosition::Before => this.child(line(true)),
                            DropPosition::After => this.child(line(false)),
                            DropPosition::Inside => this
                                .border_1()
                                .border_color(theme.primary)
                                .bg(theme.primary.opacity(0.1)),
                        }
                    })
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.begin_rename(id, window, cx);
                            return;
                        }
                        let modifiers = event.modifiers();
                        if modifiers.shift || modifiers.secondary() {
                            this.editor.toggle_selected(id);
                        } else {
                            this.editor.select([id]);
                        }
                        this.reveal(id, cx);
                        this.inspector.invalidate();
                        this.canvas_focus.focus(window, cx);
                        cx.notify();
                    })),
            );
        }
        list.into_any_element()
    }

    fn render_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(versions) = &self.editor.versions else {
            return div()
                .p_3()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Version history is available for saved projects.")
                .into_any_element();
        };
        let mut list = v_flex().gap_px().pb_4().child(
            h_flex()
                .px_3()
                .h(px(32.0))
                .justify_between()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Versions")
                .child(
                    Button::new("save-version")
                        .label("Save version")
                        .xsmall()
                        .outline()
                        .on_click(cx.listener(|this, _, window, cx| {
                            let count = this
                                .editor
                                .versions
                                .as_ref()
                                .map_or(0, |v| v.list().iter().filter(|v| !v.auto).count());
                            let name = format!("Version {}", count + 1);
                            this.apply(window, cx, |e| e.checkpoint(&name, "You", false));
                        })),
                ),
        );
        for version in versions.list() {
            let id = version.id;
            let when = crate::workspace::relative_time(version.id / 1000);
            let group: SharedString = format!("version-{id}").into();
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("version-row-{id}")))
                    .group(group.clone())
                    .mx_1p5()
                    .px_1p5()
                    .py_1()
                    .gap_2()
                    .rounded(px(6.0))
                    .hover(|this| this.bg(theme.secondary))
                    .child(div().size(px(8.0)).rounded_full().bg(if version.auto {
                        theme.border
                    } else {
                        theme.primary
                    }))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .when(!version.auto, |d| d.font_weight(FontWeight::MEDIUM))
                                    .child(version.name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{when} · {}", version.author)),
                            ),
                    )
                    .child(
                        h_flex()
                            .invisible()
                            .group_hover(group, |s| s.visible())
                            .child(
                                Button::new(SharedString::from(format!("restore-{id}")))
                                    .icon(Icon::new(Lucide::Undo2))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Restore this version")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply(window, cx, |e| e.restore_version(id));
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("delete-version-{id}")))
                                    .icon(Icon::new(Lucide::Trash))
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Delete version")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.apply(window, cx, |e| {
                                            e.versions.as_mut().map_or(Ok(()), |v| v.delete(id))
                                        });
                                    })),
                            ),
                    ),
            );
        }
        list.into_any_element()
    }

    fn render_assets(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let mut presets = div().flex().flex_wrap().gap_1p5();
        for (index, preset) in ARTBOARD_PRESETS.iter().enumerate() {
            let preset = *preset;
            presets = presets.child(
                Button::new(SharedString::from(format!("preset-{index}")))
                    .small()
                    .outline()
                    .label(format!(
                        "{} {}×{}",
                        preset.name, preset.width, preset.height
                    ))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(id) = this.apply(window, cx, |e| {
                            e.create_artboard(preset.name, (preset.width, preset.height), None)
                        }) {
                            this.reveal(id, cx);
                        }
                    })),
            );
        }
        let mut components = div().grid().grid_cols(2).gap_1p5();
        for component in COMPONENTS {
            let key = component.key;
            components = components.child(
                v_flex()
                    .id(SharedString::from(format!("component-{key}")))
                    .p_2()
                    .gap_0p5()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .hover(|this| {
                        this.bg(theme.secondary)
                            .border_color(theme.primary.opacity(0.5))
                    })
                    .cursor_pointer()
                    .child(div().font_weight(FontWeight::MEDIUM).child(component.name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(component.description),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.insert_component(key, window, cx)
                    })),
            );
        }
        let mains = self.editor.doc.components();
        let mut project = v_flex().gap_0p5();
        for main in &mains {
            let main = *main;
            let name = self
                .editor
                .doc
                .component_name(main)
                .unwrap_or("Component")
                .to_owned();
            let count = self.editor.doc.instances_of(main).len();
            project =
                project.child(
                    h_flex()
                        .id(SharedString::from(format!("project-component-{main}")))
                        .h(px(28.0))
                        .px_2()
                        .gap_2()
                        .rounded(px(6.0))
                        .hover(|this| this.bg(theme.secondary))
                        .cursor_pointer()
                        .child(
                            Icon::new(Lucide::Component)
                                .xsmall()
                                .text_color(COMPONENT_COLOR),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(
                            match count {
                                0 => String::new(),
                                1 => "1 instance".to_owned(),
                                n => format!("{n} instances"),
                            },
                        ))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Insert an instance")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let target = this.editor.insertion_target();
                            let target = if target
                                .parent
                                .is_some_and(|p| this.editor.doc.is_ancestor_or_self(main, p))
                            {
                                crate::editor::InsertTarget::default()
                            } else {
                                target
                            };
                            if let Some(id) =
                                this.apply(window, cx, |e| e.create_instance(main, target))
                            {
                                this.reveal(id, cx);
                            }
                        })),
                );
        }
        v_flex()
            .p_3()
            .gap_3()
            .when(!mains.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("This project"),
                )
                .child(project)
            })
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("New artboard"),
            )
            .child(presets)
            .child(
                h_flex().justify_between().child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Library"),
                ),
            )
            .child(components)
            .into_any_element()
    }

    pub(super) fn render_left_panel(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let tab = self.left_tab;
        let content = match tab {
            LeftTab::Layers => v_flex()
                .flex_1()
                .min_h_0()
                .child(self.render_pages(cx))
                .child(
                    div()
                        .id("layers-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .child(self.render_layers(cx)),
                )
                .into_any_element(),
            LeftTab::Assets => div()
                .id("assets-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(self.render_assets(cx))
                .into_any_element(),
            LeftTab::Variables => div()
                .id("variables-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(self.render_variables(cx))
                .into_any_element(),
            LeftTab::History => div()
                .id("history-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(self.render_history(cx))
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div().p_2().child(
                    TabBar::new("left-tabs")
                        .segmented()
                        .xsmall()
                        .w_full()
                        .selected_index(match tab {
                            LeftTab::Layers => 0,
                            LeftTab::Assets => 1,
                            LeftTab::Variables => 2,
                            LeftTab::History => 3,
                        })
                        .child(Tab::new().label("Layers"))
                        .child(Tab::new().label("Assets"))
                        .child(Tab::new().label("Variables"))
                        .child(Tab::new().label("History"))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.left_tab = match *index {
                                0 => LeftTab::Layers,
                                1 => LeftTab::Assets,
                                2 => LeftTab::Variables,
                                _ => LeftTab::History,
                            };
                            cx.notify();
                        })),
                ),
            )
            .child(content)
            .into_any_element()
    }

    fn render_code(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let target = self
            .editor
            .primary()
            .or_else(|| crate::agent::live_artboard(&self.editor));
        let Some(target) = target else {
            return div()
                .p_4()
                .text_color(theme.muted_foreground)
                .child("Select a layer to see its code.")
                .into_any_element();
        };
        let code = export(&self.editor.doc, target, self.code_format);
        let copy = code.clone();
        let format_index = CodeFormat::ALL
            .iter()
            .position(|f| *f == self.code_format)
            .unwrap_or(0);
        let mut tabs = TabBar::new("code-format")
            .underline()
            .small()
            .selected_index(format_index);
        for format in CodeFormat::ALL {
            tabs = tabs.child(Tab::new().label(format.label()));
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_2()
                    .justify_between()
                    .child(tabs.on_click(cx.listener(|this, index: &usize, _, cx| {
                        this.code_format = CodeFormat::ALL[*index];
                        cx.notify();
                    })))
                    .child(
                        Button::new("copy-code")
                            .icon(Icon::new(Lucide::Copy))
                            .ghost()
                            .xsmall()
                            .tooltip("Copy code")
                            .on_click(cx.listener(move |_, _, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                window.push_notification(
                                    gpui_kit::component::notification::Notification::success(
                                        "Code copied",
                                    ),
                                    cx,
                                );
                            })),
                    ),
            )
            .child(
                div()
                    .id("code-scroll")
                    .flex_1()
                    .min_h_0()
                    .m_2()
                    .p_2()
                    .rounded(px(8.0))
                    .bg(theme.secondary)
                    .overflow_y_scrollbar()
                    .font_family("Geist Mono")
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .child(div().whitespace_normal().child(code)),
            )
            .into_any_element()
    }

    fn render_comments(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let Some(comments) = &self.editor.comments else {
            return div()
                .p_4()
                .child("Comments need an open project.")
                .into_any_element();
        };
        let show_done = self.show_done_comments;
        let visible: Vec<_> = comments
            .items
            .iter()
            .enumerate()
            .filter(|(_, c)| show_done || c.status.is_active())
            .collect();
        let mut list = v_flex().gap_2().p_2();
        if visible.is_empty() {
            list = list.child(
                div()
                    .p_2()
                    .text_color(theme.muted_foreground)
                    .child("No open comments. Press C and click a layer to leave one — agents read them over MCP."),
            );
        }
        for (index, comment) in visible {
            let id = comment.id;
            let node = NodeId::parse(&comment.node).filter(|n| self.editor.doc.contains(*n));
            let node_name = node.map_or_else(
                || "Deleted layer".to_owned(),
                |n| self.editor.doc.display_name(n),
            );
            let status = comment.status;
            let highlighted = self.hovered_comment == Some(id);
            list = list.child(
                v_flex()
                    .id(SharedString::from(format!("comment-{id}")))
                    .p_2()
                    .gap_1()
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(if highlighted { theme.primary } else { theme.border })
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .size(px(18.0))
                                    .rounded_full()
                                    .bg(if status.is_active() { gpui_kit::rgb(0xff5a36) } else { gpui_kit::rgb(0x9ca3af) })
                                    .text_color(gpui_kit::white())
                                    .text_size(px(10.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(format!("{}", index + 1)),
                            )
                            .child(div().flex_1().truncate().font_weight(FontWeight::MEDIUM).child(node_name))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(status.label())),
                    )
                    .child(div().child(comment.body.clone()))
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(comment.author.clone()))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new(SharedString::from(format!("resolve-{id}")))
                                            .xsmall()
                                            .ghost()
                                            .label(if status.is_active() { "Resolve" } else { "Reopen" })
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let next = if status.is_active() { CommentStatus::Done } else { CommentStatus::Open };
                                                if let Some(comments) = this.editor.comments.as_mut()
                                                    && let Err(error) = comments.update(id, None, Some(next)) {
                                                        window.push_notification(
                                                            gpui_kit::component::notification::Notification::error(format!("{error:#}")),
                                                            cx,
                                                        );
                                                    }
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("delete-comment-{id}")))
                                            .xsmall()
                                            .ghost()
                                            .icon(Icon::new(Lucide::Trash))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if let Some(comments) = this.editor.comments.as_mut() {
                                                    let _ = comments.remove(id);
                                                }
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.hovered_comment = Some(id);
                        if let Some(node) = node {
                            this.editor.select([node]);
                            this.reveal(node, cx);
                            this.inspector.invalidate();
                        }
                        cx.notify();
                    })),
            );
        }
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_3()
                    .h(px(32.0))
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{} open", comments.active().count())),
                    )
                    .child(
                        Button::new("toggle-done")
                            .xsmall()
                            .ghost()
                            .selected(show_done)
                            .label("Show resolved")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_done_comments = !this.show_done_comments;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id("comments-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(list),
            )
            .into_any_element()
    }

    pub(super) fn render_right_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.right_tab;
        let content = match tab {
            RightTab::Design => self.render_inspector(window, cx),
            RightTab::Prototype => self.render_prototype_panel(cx),
            RightTab::Code => self.render_code(cx),
            RightTab::Comments => self.render_comments(cx),
        };
        let theme = cx.theme().clone();
        let open = self
            .editor
            .comments
            .as_ref()
            .map_or(0, |c| c.active().count());
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_l_1()
            .border_color(theme.border)
            .child(
                div().p_2().child(
                    TabBar::new("right-tabs")
                        .segmented()
                        .small()
                        .w_full()
                        .selected_index(match tab {
                            RightTab::Design => 0,
                            RightTab::Prototype => 1,
                            RightTab::Code => 2,
                            RightTab::Comments => 3,
                        })
                        .child(Tab::new().label("Design"))
                        .child(Tab::new().label("Prototype"))
                        .child(Tab::new().label("Code"))
                        .child(Tab::new().label(if open > 0 {
                            format!("Comments {open}")
                        } else {
                            "Comments".to_owned()
                        }))
                        .on_click(cx.listener(|this, index: &usize, _, cx| {
                            this.right_tab = match index {
                                0 => RightTab::Design,
                                1 => RightTab::Prototype,
                                2 => RightTab::Code,
                                _ => RightTab::Comments,
                            };
                            cx.notify();
                        })),
                ),
            )
            .child(content)
            .into_any_element()
    }
}
