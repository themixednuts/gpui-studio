//! The Design panel: position, layout, fill, stroke, typography, effects, and
//! raw CSS for the selection. Every control writes through the editor.

use std::collections::HashMap;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div, prelude::*, px,
};

use gpui_kit::Focusable as _;

use super::Studio;
use super::paint::hsla;
use crate::model::style::{
    Align, Computed, Display, Length, LineHeight, Overflow, Position, Shadow, TextAlign, fmt_num,
};
use crate::model::{Color, NodeId, NodeKind};
use crate::presets::ARTBOARD_PRESETS;

/// Text-entry properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Field {
    Name,
    X,
    Y,
    W,
    H,
    Gap,
    PadX,
    PadY,
    Radius,
    Opacity,
    Fill,
    Stroke,
    StrokeWidth,
    TextColor,
    FontSize,
    LineHeight,
    LetterSpacing,
    Text,
    ShadowX,
    ShadowY,
    ShadowBlur,
    ShadowSpread,
    ShadowColor,
    GridColumns,
    Src,
    Href,
    Css,
}

impl Field {
    const ALL: [Self; 27] = [
        Self::Name,
        Self::X,
        Self::Y,
        Self::W,
        Self::H,
        Self::Gap,
        Self::PadX,
        Self::PadY,
        Self::Radius,
        Self::Opacity,
        Self::Fill,
        Self::Stroke,
        Self::StrokeWidth,
        Self::TextColor,
        Self::FontSize,
        Self::LineHeight,
        Self::LetterSpacing,
        Self::Text,
        Self::ShadowX,
        Self::ShadowY,
        Self::ShadowBlur,
        Self::ShadowSpread,
        Self::ShadowColor,
        Self::GridColumns,
        Self::Src,
        Self::Href,
        Self::Css,
    ];
}

/// Revision, selection, and measured sizes the inputs last showed.
type SyncKey = (u64, Vec<NodeId>, Vec<(i32, i32)>);

/// Inspector input state.
pub(crate) struct Inspector {
    fields: HashMap<Field, Entity<InputState>>,
    synced: Option<SyncKey>,
    _subscriptions: Vec<Subscription>,
}

impl Inspector {
    /// Create every input once and route edits back to the Studio.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Studio>) -> Self {
        let mut fields = HashMap::new();
        let mut subscriptions = Vec::new();
        for field in Field::ALL {
            let input = cx.new(|cx| InputState::new(window, cx));
            let subscription = cx.subscribe_in(
                &input,
                window,
                move |this: &mut Studio, input, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value().to_string();
                        this.apply_field(field, &value, window, cx);
                    }
                    InputEvent::PressEnter { .. } if field != Field::Css => {
                        this.editor.history.seal();
                        this.canvas_focus.focus(window, cx);
                    }
                    InputEvent::Blur => {
                        this.editor.history.seal();
                        this.inspector.invalidate();
                        cx.notify();
                    }
                    _ => {}
                },
            );
            fields.insert(field, input);
            subscriptions.push(subscription);
        }
        Self {
            fields,
            synced: None,
            _subscriptions: subscriptions,
        }
    }

    /// Force the next render to reload field values.
    pub(crate) fn invalidate(&mut self) {
        self.synced = None;
    }

    fn input(&self, field: Field) -> Entity<InputState> {
        self.fields[&field].clone()
    }
}

fn hex(color: Option<Color>) -> String {
    color.map_or_else(String::new, Color::to_css)
}

fn px_text(value: f32) -> String {
    fmt_num(value)
}

fn parse_number(value: &str) -> Option<f32> {
    let value = value
        .trim()
        .trim_end_matches("px")
        .trim_end_matches('%')
        .trim();
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn primary_shadow(c: &Computed) -> Shadow {
    c.shadows.first().copied().unwrap_or(Shadow {
        x: 0.0,
        y: 4.0,
        blur: 12.0,
        spread: 0.0,
        color: Color::rgba(0, 0, 0, 40),
        inset: false,
    })
}

impl Studio {
    fn computed(&self, id: NodeId) -> Computed {
        self.editor
            .doc
            .get(id)
            .map(|n| n.style.computed())
            .unwrap_or_default()
    }

    fn parent_computed(&self, id: NodeId) -> Option<Computed> {
        self.editor.doc.parent(id).map(|p| self.computed(p))
    }

    fn field_value(&self, field: Field, id: NodeId) -> String {
        let doc = &self.editor.doc;
        let c = self.computed(id);
        let bounds = self.canvas.doc_bounds(id);
        let node = doc.get(id);
        match field {
            Field::Name => node.and_then(|n| n.name.clone()).unwrap_or_default(),
            Field::X | Field::Y => {
                if let Some(artboard) = doc.artboard_of(id).filter(|a| a.root == id) {
                    return px_text(if field == Field::X {
                        artboard.x
                    } else {
                        artboard.y
                    });
                }
                let inset = if field == Field::X {
                    c.inset[3]
                } else {
                    c.inset[0]
                };
                if let Some(value) = inset.px() {
                    return px_text(value);
                }
                let root = doc.root_of(id);
                match (bounds, self.canvas.doc_bounds(root)) {
                    (Some(b), Some(r)) => px_text(if field == Field::X {
                        (b.origin.x - r.origin.x).round()
                    } else {
                        (b.origin.y - r.origin.y).round()
                    }),
                    _ => String::new(),
                }
            }
            Field::W => c
                .width
                .px()
                .or(bounds.map(|b| b.size.width.round()))
                .map(px_text)
                .unwrap_or_default(),
            Field::H => c
                .height
                .px()
                .or(bounds.map(|b| b.size.height.round()))
                .map(px_text)
                .unwrap_or_default(),
            Field::Gap => px_text(c.row_gap.max(c.column_gap)),
            Field::PadX => px_text(c.padding[3].max(c.padding[1])),
            Field::PadY => px_text(c.padding[0].max(c.padding[2])),
            Field::Radius => px_text(c.radius[0]),
            Field::Opacity => px_text((c.opacity * 100.0).round()),
            Field::Fill => hex(c.background),
            Field::Stroke => hex(c.border_color),
            Field::StrokeWidth => {
                let has_border = c
                    .border_style
                    .as_deref()
                    .is_some_and(|s| !matches!(s, "none" | "hidden"));
                px_text(if has_border { c.border_width[0] } else { 0.0 })
            }
            Field::TextColor => hex(c.color),
            Field::FontSize => c.font_size.map(px_text).unwrap_or_default(),
            Field::LineHeight => match c.line_height {
                Some(LineHeight::Px(v)) => format!("{}px", px_text(v)),
                Some(LineHeight::Relative(v)) => px_text(v),
                None => String::new(),
            },
            Field::LetterSpacing => c.letter_spacing.map(px_text).unwrap_or_default(),
            Field::Text => doc.text_content(id).unwrap_or_default(),
            Field::ShadowX => c.shadows.first().map(|s| px_text(s.x)).unwrap_or_default(),
            Field::ShadowY => c.shadows.first().map(|s| px_text(s.y)).unwrap_or_default(),
            Field::ShadowBlur => c
                .shadows
                .first()
                .map(|s| px_text(s.blur))
                .unwrap_or_default(),
            Field::ShadowSpread => c
                .shadows
                .first()
                .map(|s| px_text(s.spread))
                .unwrap_or_default(),
            Field::ShadowColor => c
                .shadows
                .first()
                .map(|s| s.color.to_css())
                .unwrap_or_default(),
            Field::GridColumns => c.grid_columns.map(|n| n.to_string()).unwrap_or_default(),
            Field::Src => node.and_then(|n| n.attr("src")).unwrap_or("").to_owned(),
            Field::Href => node.and_then(|n| n.attr("href")).unwrap_or("").to_owned(),
            Field::Css => node.map(|n| n.style.to_css()).unwrap_or_default(),
        }
    }

    /// Refresh inputs from the document unless the user is typing in them.
    pub(super) fn sync_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.editor.selection.clone();
        let sizes: Vec<(i32, i32)> = selection
            .iter()
            .filter_map(|id| self.canvas.doc_bounds(*id))
            .map(|b| (b.size.width.round() as i32, b.size.height.round() as i32))
            .collect();
        let key = (self.editor.revision, selection.clone(), sizes);
        if self.inspector.synced.as_ref() == Some(&key) {
            return;
        }
        self.inspector.synced = Some(key);
        let Some(id) = selection.first().copied() else {
            return;
        };
        for field in Field::ALL {
            let input = self.inspector.input(field);
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let value = self.field_value(field, id);
            if input.read(cx).value().as_ref() != value {
                input.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
    }

    fn style_change(
        &mut self,
        changes: Vec<(&str, Option<String>)>,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ids = self.editor.selection.clone();
        if ids.is_empty() {
            return;
        }
        let changes: Vec<(String, Option<String>)> = changes
            .into_iter()
            .map(|(p, v)| (p.to_owned(), v))
            .collect();
        let result = self.editor.set_styles(&ids, &changes, Some(key));
        if let Err(error) = result {
            self.editor.status = format!("{error:#}");
        }
        let _ = window;
        cx.notify();
    }

    fn apply_field(
        &mut self,
        field: Field,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.editor.primary() else {
            return;
        };
        let key = format!("field-{field:?}-{id}");
        let number = parse_number(value);
        let px_value = |n: f32| Some(format!("{}px", fmt_num(n)));
        match field {
            Field::Name => {
                let _ = self.editor.rename(id, value);
            }
            Field::X | Field::Y => {
                let Some(n) = number else { return };
                if let Some(artboard) = self
                    .editor
                    .doc
                    .artboard_of(id)
                    .filter(|a| a.root == id)
                    .cloned()
                {
                    let (x, y) = if field == Field::X {
                        (n, artboard.y)
                    } else {
                        (artboard.x, n)
                    };
                    let _ = self.editor.move_artboard(id, x, y, Some(&key));
                } else if self.computed(id).position == Position::Absolute {
                    let property = if field == Field::X { "left" } else { "top" };
                    self.style_change(vec![(property, px_value(n))], &key, window, cx);
                }
            }
            Field::W | Field::H => {
                let Some(n) = number.filter(|n| *n >= 0.0) else {
                    return;
                };
                let property = if field == Field::W { "width" } else { "height" };
                self.style_change(vec![(property, px_value(n))], &key, window, cx);
            }
            Field::Gap => {
                self.style_change(vec![("gap", number.and_then(px_value))], &key, window, cx)
            }
            Field::PadX => {
                let v = number.and_then(px_value);
                self.style_change(
                    vec![("padding-left", v.clone()), ("padding-right", v)],
                    &key,
                    window,
                    cx,
                );
            }
            Field::PadY => {
                let v = number.and_then(px_value);
                self.style_change(
                    vec![("padding-top", v.clone()), ("padding-bottom", v)],
                    &key,
                    window,
                    cx,
                );
            }
            Field::Radius => self.style_change(
                vec![("border-radius", number.and_then(px_value))],
                &key,
                window,
                cx,
            ),
            Field::Opacity => {
                let v = number.map(|n| fmt_num((n / 100.0).clamp(0.0, 1.0)));
                self.style_change(vec![("opacity", v.filter(|v| v != "1"))], &key, window, cx);
            }
            Field::Fill | Field::Stroke | Field::TextColor | Field::ShadowColor => {
                let color = if value.trim().is_empty() {
                    None
                } else {
                    match Color::parse_loose(value) {
                        Some(color) => Some(color.to_css()),
                        None => return,
                    }
                };
                match field {
                    Field::Fill => {
                        self.style_change(vec![("background-color", color)], &key, window, cx)
                    }
                    Field::Stroke => {
                        let mut changes = vec![("border-color", color.clone())];
                        if color.is_some() && self.computed(id).border_style.is_none() {
                            changes.push(("border-style", Some("solid".into())));
                            changes.push(("border-width", Some("1px".into())));
                        }
                        self.style_change(changes, &key, window, cx);
                    }
                    Field::TextColor => self.style_change(vec![("color", color)], &key, window, cx),
                    _ => {
                        let mut shadow = primary_shadow(&self.computed(id));
                        if let Some(color) = color.as_deref().and_then(Color::parse) {
                            shadow.color = color;
                            self.style_change(
                                vec![("box-shadow", Some(shadow.to_css()))],
                                &key,
                                window,
                                cx,
                            );
                        }
                    }
                }
            }
            Field::StrokeWidth => {
                let width = number.unwrap_or(0.0);
                if width <= 0.0 {
                    self.style_change(
                        vec![
                            ("border-style", Some("none".into())),
                            ("border-width", None),
                        ],
                        &key,
                        window,
                        cx,
                    );
                } else {
                    let c = self.computed(id);
                    let mut changes = vec![("border-width", px_value(width))];
                    if c.border_style.as_deref().is_none_or(|s| s == "none") {
                        changes.push(("border-style", Some("solid".into())));
                    }
                    if c.border_color.is_none() {
                        changes.push(("border-color", Some("#d4d4d4".into())));
                    }
                    self.style_change(changes, &key, window, cx);
                }
            }
            Field::FontSize => self.style_change(
                vec![("font-size", number.and_then(px_value))],
                &key,
                window,
                cx,
            ),
            Field::LineHeight => {
                let trimmed = value.trim();
                let v = if trimmed.is_empty() {
                    None
                } else if trimmed.ends_with("px") || trimmed.ends_with('%') {
                    Some(trimmed.to_owned())
                } else {
                    number.map(fmt_num)
                };
                self.style_change(vec![("line-height", v)], &key, window, cx);
            }
            Field::LetterSpacing => self.style_change(
                vec![("letter-spacing", number.and_then(px_value))],
                &key,
                window,
                cx,
            ),
            Field::Text => {
                let _ = self.editor.set_text(id, value, Some(&key));
            }
            Field::ShadowX | Field::ShadowY | Field::ShadowBlur | Field::ShadowSpread => {
                let Some(n) = number else { return };
                let mut shadow = primary_shadow(&self.computed(id));
                match field {
                    Field::ShadowX => shadow.x = n,
                    Field::ShadowY => shadow.y = n,
                    Field::ShadowBlur => shadow.blur = n.max(0.0),
                    _ => shadow.spread = n,
                }
                self.style_change(
                    vec![("box-shadow", Some(shadow.to_css()))],
                    &key,
                    window,
                    cx,
                );
            }
            Field::GridColumns => {
                let v = number.map(|n| format!("repeat({}, 1fr)", n.max(1.0).round()));
                self.style_change(vec![("grid-template-columns", v)], &key, window, cx);
            }
            Field::Src | Field::Href => {
                let name = if field == Field::Src { "src" } else { "href" };
                let v = (!value.trim().is_empty()).then(|| value.trim().to_owned());
                let _ = self.editor.set_attributes(id, &[(name.to_owned(), v)]);
            }
            Field::Css => {
                let _ = self.editor.set_style_text(id, value, Some(&key));
            }
        }
        cx.notify();
    }

    // ---- building blocks -----------------------------------------------------------

    fn section(&self, title: &str, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
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
                    .child(title.to_owned()),
            )
    }

    fn labeled(&self, label: &'static str, field: Field, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                div()
                    .w(px(16.0))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.inspector.input(field)).xsmall()),
            )
            .into_any_element()
    }

    fn color_row(&self, field: Field, color: Option<Color>, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        h_flex()
            .gap_2()
            .child(
                div()
                    .size(px(20.0))
                    .flex_shrink_0()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(color.map_or(gpui_kit::transparent_black(), hsla)),
            )
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.inspector.input(field)).xsmall()),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn segment(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        icon: Option<Lucide>,
        selected: bool,
        tooltip: &'static str,
        cx: &mut Context<Self>,
        on: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        let id: SharedString = id.into();
        let mut button = Button::new(id)
            .ghost()
            .xsmall()
            .selected(selected)
            .tooltip(tooltip);
        button = match icon {
            Some(icon) => button.icon(Icon::new(icon)),
            None => button.label(label),
        };
        button.on_click(cx.listener(move |this, _, window, cx| {
            on(this, window, cx);
            this.inspector.invalidate();
            cx.notify();
        }))
    }

    fn set(
        &mut self,
        changes: Vec<(&'static str, Option<&str>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changes = changes
            .into_iter()
            .map(|(p, v)| (p, v.map(ToOwned::to_owned)))
            .collect();
        self.style_change(changes, "toggle", window, cx);
        self.editor.history.seal();
    }

    // ---- sections ------------------------------------------------------------------

    fn render_header(&self, id: NodeId, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let doc = &self.editor.doc;
        let tag = doc.get(id).map(|n| n.tag().to_owned()).unwrap_or_default();
        let is_artboard = doc.is_artboard(id);
        let tags = [
            "div", "section", "header", "main", "footer", "nav", "article", "aside", "p", "span",
            "h1", "h2", "h3", "button", "a", "label", "ul", "li",
        ];
        let entity = cx.entity();
        v_flex()
            .px_3()
            .py_2p5()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.inspector.input(Field::Name)).xsmall()),
                    )
                    .child(
                        Button::new("tag-menu")
                            .xsmall()
                            .outline()
                            .label(if is_artboard {
                                format!("Artboard · {tag}")
                            } else {
                                format!("<{tag}>")
                            })
                            .dropdown_menu(move |mut menu, _, _| {
                                for tag in tags {
                                    let entity = entity.clone();
                                    menu =
                                        menu.item(PopupMenuItem::new(format!("<{tag}>")).on_click(
                                            move |_, window, cx| {
                                                entity.update(cx, |this, cx| {
                                                    this.apply(window, cx, |e| e.set_tag(id, tag));
                                                });
                                            },
                                        ));
                                }
                                menu.max_h(px(320.0)).scrollable(true)
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_frame_section(&self, id: NodeId, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let doc = &self.editor.doc;
        let is_artboard = doc.is_artboard(id);
        let parent = self.parent_computed(id);
        let absolute = c.position == Position::Absolute;
        let mut section = self.section(if is_artboard { "Artboard" } else { "Frame" }, cx);
        if is_artboard {
            let entity = cx.entity();
            section = section.child(
                Button::new("artboard-preset")
                    .xsmall()
                    .outline()
                    .w_full()
                    .label("Resize to preset")
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for preset in ARTBOARD_PRESETS {
                            let entity = entity.clone();
                            let preset = *preset;
                            menu = menu.item(
                                PopupMenuItem::new(format!(
                                    "{} — {}×{}",
                                    preset.name, preset.width, preset.height
                                ))
                                .on_click(move |_, window, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.apply(window, cx, |e| {
                                            e.set_size(
                                                id,
                                                Some(preset.width),
                                                Some(preset.height),
                                                None,
                                            )
                                        });
                                    });
                                }),
                            );
                        }
                        menu
                    }),
            );
        }
        section = section
            .child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("X", Field::X, cx))
                    .child(self.labeled("Y", Field::Y, cx)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("W", Field::W, cx))
                    .child(self.labeled("H", Field::H, cx)),
            );
        if !is_artboard {
            // Sizing modes, like Paper/Figma: Fixed, Hug, Fill per axis.
            let parent_row = parent
                .as_ref()
                .is_some_and(|p| p.display == Display::Flex && !p.direction.is_column());
            let parent_col = parent
                .as_ref()
                .is_some_and(|p| p.display == Display::Flex && p.direction.is_column());
            let mode = |len: Length, fills_by_grow: bool| -> &'static str {
                if fills_by_grow && c.grow > 0.0 {
                    "Fill"
                } else {
                    match len {
                        Length::Px(_) => "Fixed",
                        Length::Percent(p) if (p - 100.0).abs() < 0.01 => "Fill",
                        Length::Percent(_) => "Fixed",
                        _ => "Hug",
                    }
                }
            };
            let width_mode = mode(c.width, parent_row);
            let height_mode = mode(c.height, parent_col);
            let mut row = h_flex().gap_1();
            for (axis, current) in [("W", width_mode), ("H", height_mode)] {
                let mut group = h_flex()
                    .flex_1()
                    .gap_0p5()
                    .child(div().w(px(16.0)).text_xs().child(axis));
                for option in ["Fixed", "Hug", "Fill"] {
                    let is_width = axis == "W";
                    group = group.child(self.segment(
                        format!("size-{axis}-{option}"),
                        option,
                        None,
                        current == option,
                        match option {
                            "Fixed" => "Fixed size",
                            "Hug" => "Hug contents",
                            _ => "Fill container",
                        },
                        cx,
                        move |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let property = if is_width { "width" } else { "height" };
                            let grows = if is_width { parent_row } else { parent_col };
                            match option {
                                "Fixed" => {
                                    let size = this.canvas.doc_bounds(id).map(|b| {
                                        if is_width {
                                            b.size.width
                                        } else {
                                            b.size.height
                                        }
                                    });
                                    let value = size.map(|s| format!("{}px", fmt_num(s.round())));
                                    this.set(
                                        vec![(property, value.as_deref()), ("flex-grow", None)],
                                        window,
                                        cx,
                                    );
                                }
                                "Hug" => this.set(
                                    vec![(property, None), ("flex-grow", None)],
                                    window,
                                    cx,
                                ),
                                _ if grows => this.set(
                                    vec![(property, None), ("flex-grow", Some("1"))],
                                    window,
                                    cx,
                                ),
                                _ => this.set(vec![(property, Some("100%"))], window, cx),
                            }
                        },
                    ));
                }
                row = row.child(group);
            }
            section = section.child(row).child(
                h_flex()
                    .gap_1()
                    .child(self.segment(
                        "pos-flow",
                        "In flow",
                        None,
                        !absolute,
                        "Positioned by the parent's layout",
                        cx,
                        |this, window, cx| {
                            this.set(
                                vec![
                                    ("position", None),
                                    ("left", None),
                                    ("top", None),
                                    ("right", None),
                                    ("bottom", None),
                                ],
                                window,
                                cx,
                            );
                        },
                    ))
                    .child(self.segment(
                        "pos-abs",
                        "Absolute",
                        None,
                        absolute,
                        "Free position inside the parent",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let offset = this.editor.doc.parent(id).and_then(|p| {
                                let b = this.canvas.doc_bounds(id)?;
                                let pb = this.canvas.doc_bounds(p)?;
                                Some((b.origin.x - pb.origin.x, b.origin.y - pb.origin.y))
                            });
                            let (left, top) = offset.unwrap_or((0.0, 0.0));
                            let left = format!("{}px", fmt_num(left.round()));
                            let top = format!("{}px", fmt_num(top.round()));
                            this.set(
                                vec![
                                    ("position", Some("absolute")),
                                    ("left", Some(&left)),
                                    ("top", Some(&top)),
                                ],
                                window,
                                cx,
                            );
                            if let Some(parent) = this.editor.doc.parent(id)
                                && this.computed(parent).position == Position::Static
                                && !this.editor.doc.is_artboard(parent)
                            {
                                let _ = this.editor.set_styles(
                                    &[parent],
                                    &[("position".into(), Some("relative".into()))],
                                    Some("toggle"),
                                );
                            }
                        },
                    )),
            );
        }
        section
            .child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("R", Field::Radius, cx))
                    .child(self.labeled("%", Field::Opacity, cx)),
            )
            .child(h_flex().gap_1().child(self.segment(
                "clip",
                "Clip content",
                None,
                c.overflow != Overflow::Visible,
                "overflow: hidden",
                cx,
                |this, window, cx| {
                    let Some(id) = this.editor.primary() else {
                        return;
                    };
                    let clipped = this.computed(id).overflow != Overflow::Visible;
                    this.set(
                        vec![("overflow", if clipped { None } else { Some("hidden") })],
                        window,
                        cx,
                    );
                },
            )))
            .into_any_element()
    }

    fn render_layout_section(&self, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let display = c.display;
        let mut section = self.section("Layout", cx).child(
            h_flex()
                .gap_1()
                .child(self.segment(
                    "display-block",
                    "Stack",
                    None,
                    matches!(display, Display::Block | Display::Inline),
                    "Block flow",
                    cx,
                    |this, window, cx| {
                        this.set(
                            vec![
                                ("display", None),
                                ("flex-direction", None),
                                ("grid-template-columns", None),
                            ],
                            window,
                            cx,
                        );
                    },
                ))
                .child(self.segment(
                    "display-flex",
                    "Auto layout",
                    None,
                    display == Display::Flex,
                    "Flexbox (Shift+A)",
                    cx,
                    |this, window, cx| {
                        this.set(
                            vec![("display", Some("flex")), ("grid-template-columns", None)],
                            window,
                            cx,
                        );
                    },
                ))
                .child(self.segment(
                    "display-grid",
                    "Grid",
                    None,
                    display == Display::Grid,
                    "CSS grid",
                    cx,
                    |this, window, cx| {
                        this.set(
                            vec![
                                ("display", Some("grid")),
                                ("grid-template-columns", Some("repeat(2, 1fr)")),
                            ],
                            window,
                            cx,
                        );
                    },
                )),
        );
        if display == Display::Flex {
            let column = c.direction.is_column();
            section = section.child(
                h_flex()
                    .gap_1()
                    .child(self.segment(
                        "dir-row",
                        "",
                        Some(Lucide::ArrowRight),
                        !column,
                        "Horizontal",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("flex-direction", None)], window, cx);
                        },
                    ))
                    .child(self.segment(
                        "dir-col",
                        "",
                        Some(Lucide::ArrowDown),
                        column,
                        "Vertical",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("flex-direction", Some("column"))], window, cx);
                        },
                    ))
                    .child(self.segment(
                        "wrap",
                        "Wrap",
                        None,
                        c.wrap,
                        "Wrap",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let wrap = this.computed(id).wrap;
                            this.set(
                                vec![("flex-wrap", if wrap { None } else { Some("wrap") })],
                                window,
                                cx,
                            );
                        },
                    ))
                    .child(self.segment(
                        "space-between",
                        "Space between",
                        None,
                        c.justify == Align::SpaceBetween,
                        "justify-content: space-between",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let between = this.computed(id).justify == Align::SpaceBetween;
                            this.set(
                                vec![(
                                    "justify-content",
                                    if between { None } else { Some("space-between") },
                                )],
                                window,
                                cx,
                            );
                        },
                    )),
            );
            // 3×3 alignment grid: main axis × cross axis.
            let mut grid = div()
                .grid()
                .grid_cols(3)
                .gap_0p5()
                .w(px(84.0))
                .p_1()
                .rounded(px(6.0))
                .bg(cx.theme().secondary);
            let keyword = |a: Align| match a {
                Align::Center => 1,
                Align::End => 2,
                _ => 0,
            };
            let (main, cross) = (keyword(c.justify), keyword(c.align_items));
            for row in 0..3 {
                for col in 0..3 {
                    let (m, x) = if column { (row, col) } else { (col, row) };
                    let selected = m == main && x == cross && c.justify != Align::SpaceBetween;
                    let names = ["flex-start", "center", "flex-end"];
                    grid = grid.child(
                        div()
                            .id(SharedString::from(format!("align-{row}-{col}")))
                            .size(px(24.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().background))
                            .child(
                                div()
                                    .size(px(if selected { 8.0 } else { 4.0 }))
                                    .rounded(px(2.0))
                                    .bg(if selected {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground.opacity(0.5)
                                    }),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set(
                                    vec![
                                        ("justify-content", Some(names[m])),
                                        ("align-items", Some(names[x])),
                                    ],
                                    window,
                                    cx,
                                );
                                this.inspector.invalidate();
                            })),
                    );
                }
            }
            section = section.child(
                h_flex().gap_3().items_start().child(grid).child(
                    v_flex()
                        .flex_1()
                        .gap_2()
                        .child(self.labeled("↔", Field::Gap, cx))
                        .child(self.labeled("⇥", Field::PadX, cx))
                        .child(self.labeled("⤓", Field::PadY, cx)),
                ),
            );
        } else if display == Display::Grid {
            section = section
                .child(
                    h_flex()
                        .gap_2()
                        .child(self.labeled("#", Field::GridColumns, cx))
                        .child(self.labeled("↔", Field::Gap, cx)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(self.labeled("⇥", Field::PadX, cx))
                        .child(self.labeled("⤓", Field::PadY, cx)),
                );
        } else {
            section = section.child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("⇥", Field::PadX, cx))
                    .child(self.labeled("⤓", Field::PadY, cx)),
            );
        }
        section.into_any_element()
    }

    fn render_fill_section(&self, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let has_fill = c.background.is_some_and(|b| b.a > 0);
        self.section("Fill", cx)
            .child(self.color_row(Field::Fill, c.background, cx))
            .child(h_flex().gap_1().child(self.segment(
                "fill-toggle",
                if has_fill { "Remove fill" } else { "Add fill" },
                None,
                false,
                "background-color",
                cx,
                move |this, window, cx| {
                    this.set(
                        vec![(
                            "background-color",
                            if has_fill { None } else { Some("#ffffff") },
                        )],
                        window,
                        cx,
                    );
                },
            )))
            .into_any_element()
    }

    fn render_stroke_section(&self, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let dashed = matches!(c.border_style.as_deref(), Some("dashed" | "dotted"));
        self.section("Stroke", cx)
            .child(self.color_row(Field::Stroke, c.border_color, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("W", Field::StrokeWidth, cx))
                    .child(self.segment(
                        "stroke-solid",
                        "Solid",
                        None,
                        !dashed,
                        "Solid stroke",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("border-style", Some("solid"))], window, cx);
                        },
                    ))
                    .child(self.segment(
                        "stroke-dashed",
                        "Dashed",
                        None,
                        dashed,
                        "Dashed stroke",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("border-style", Some("dashed"))], window, cx);
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_text_section(&self, id: NodeId, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let weight_entity = entity.clone();
        let mut families: Vec<String> = vec!["Geist".into(), "Geist Mono".into()];
        for family in self.fonts.families() {
            if families.len() >= 40 {
                break;
            }
            if !families.contains(family) && !family.starts_with('.') {
                families.push(family.clone());
            }
        }
        let family_label = c.font_family.as_deref().map_or_else(
            || "Geist".to_owned(),
            |f| {
                f.split(',')
                    .next()
                    .unwrap_or(f)
                    .trim()
                    .trim_matches(['"', '\''])
                    .to_owned()
            },
        );
        let weight = c.font_weight.unwrap_or(400);
        let align = c.text_align;
        let is_text = self.editor.doc.is_text_layer(id);
        let mut section = self.section("Typography", cx);
        if is_text {
            section = section.child(Input::new(&self.inspector.input(Field::Text)).xsmall());
        }
        section
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("font-family")
                            .xsmall()
                            .outline()
                            .flex_1()
                            .label(family_label)
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                for family in families.clone() {
                                    let entity = entity.clone();
                                    menu = menu.item(PopupMenuItem::new(family.clone()).on_click(
                                        move |_, window, cx| {
                                            let value = if family.contains(' ') {
                                                format!("\"{family}\", sans-serif")
                                            } else {
                                                format!("{family}, sans-serif")
                                            };
                                            entity.update(cx, |this, cx| {
                                                this.set(
                                                    vec![("font-family", Some(&value))],
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ));
                                }
                                menu.max_h(px(360.0)).scrollable(true)
                            }),
                    )
                    .child(
                        Button::new("font-weight")
                            .xsmall()
                            .outline()
                            .label(weight.to_string())
                            .dropdown_caret(true)
                            .dropdown_menu(move |mut menu, _, _| {
                                for (value, name) in [
                                    (300, "Light"),
                                    (400, "Regular"),
                                    (500, "Medium"),
                                    (600, "Semibold"),
                                    (700, "Bold"),
                                    (800, "Extra bold"),
                                ] {
                                    let entity = weight_entity.clone();
                                    menu = menu.item(
                                        PopupMenuItem::new(format!("{value} {name}"))
                                            .checked(value == weight)
                                            .on_click(move |_, window, cx| {
                                                let value = value.to_string();
                                                entity.update(cx, |this, cx| {
                                                    this.set(
                                                        vec![("font-weight", Some(&value))],
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                    );
                                }
                                menu
                            }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(self.labeled("S", Field::FontSize, cx))
                    .child(self.labeled("LH", Field::LineHeight, cx)),
            )
            .child(self.color_row(Field::TextColor, c.color, cx))
            .child(
                h_flex()
                    .gap_1()
                    .child(self.segment(
                        "align-left",
                        "",
                        Some(Lucide::TextAlignStart),
                        matches!(align, TextAlign::Left | TextAlign::Inherit),
                        "Align left",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("text-align", None)], window, cx);
                        },
                    ))
                    .child(self.segment(
                        "align-center",
                        "",
                        Some(Lucide::TextAlignCenter),
                        align == TextAlign::Center,
                        "Align center",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("text-align", Some("center"))], window, cx);
                        },
                    ))
                    .child(self.segment(
                        "align-right",
                        "",
                        Some(Lucide::TextAlignEnd),
                        align == TextAlign::Right,
                        "Align right",
                        cx,
                        |this, window, cx| {
                            this.set(vec![("text-align", Some("right"))], window, cx);
                        },
                    ))
                    .child(div().w(px(8.0)))
                    .child(self.segment(
                        "italic",
                        "",
                        Some(Lucide::Italic),
                        c.italic == Some(true),
                        "Italic",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let on = this.computed(id).italic == Some(true);
                            this.set(
                                vec![("font-style", if on { None } else { Some("italic") })],
                                window,
                                cx,
                            );
                        },
                    ))
                    .child(self.segment(
                        "underline",
                        "",
                        Some(Lucide::Underline),
                        c.underline == Some(true),
                        "Underline",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let on = this.computed(id).underline == Some(true);
                            this.set(
                                vec![(
                                    "text-decoration",
                                    if on { None } else { Some("underline") },
                                )],
                                window,
                                cx,
                            );
                        },
                    ))
                    .child(self.segment(
                        "strike",
                        "",
                        Some(Lucide::Strikethrough),
                        c.strikethrough == Some(true),
                        "Strikethrough",
                        cx,
                        |this, window, cx| {
                            let Some(id) = this.editor.primary() else {
                                return;
                            };
                            let on = this.computed(id).strikethrough == Some(true);
                            this.set(
                                vec![(
                                    "text-decoration",
                                    if on { None } else { Some("line-through") },
                                )],
                                window,
                                cx,
                            );
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_effects_section(&self, c: &Computed, cx: &mut Context<Self>) -> AnyElement {
        let has_shadow = !c.shadows.is_empty();
        let mut section = self
            .section("Shadow", cx)
            .child(h_flex().gap_1().child(self.segment(
                "shadow-toggle",
                if has_shadow {
                    "Remove shadow"
                } else {
                    "Add drop shadow"
                },
                None,
                false,
                "box-shadow",
                cx,
                move |this, window, cx| {
                    let value = Shadow {
                        x: 0.0,
                        y: 4.0,
                        blur: 12.0,
                        spread: 0.0,
                        color: Color::rgba(0, 0, 0, 40),
                        inset: false,
                    }
                    .to_css();
                    this.set(
                        vec![("box-shadow", if has_shadow { None } else { Some(&value) })],
                        window,
                        cx,
                    );
                },
            )));
        if has_shadow {
            section = section
                .child(
                    h_flex()
                        .gap_2()
                        .child(self.labeled("X", Field::ShadowX, cx))
                        .child(self.labeled("Y", Field::ShadowY, cx)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(self.labeled("B", Field::ShadowBlur, cx))
                        .child(self.labeled("S", Field::ShadowSpread, cx)),
                )
                .child(self.color_row(Field::ShadowColor, c.shadows.first().map(|s| s.color), cx));
        }
        section.into_any_element()
    }

    fn render_attributes_section(&self, id: NodeId, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tag = self.editor.doc.get(id)?.tag().to_owned();
        let field = match tag.as_str() {
            "img" => Field::Src,
            "a" => Field::Href,
            _ => return None,
        };
        let label = if field == Field::Src {
            "Image source (relative to artboards/)"
        } else {
            "Link"
        };
        Some(
            self.section(label, cx)
                .child(Input::new(&self.inspector.input(field)).xsmall())
                .into_any_element(),
        )
    }

    fn render_css_section(&self, cx: &mut Context<Self>) -> AnyElement {
        self.section("CSS", cx)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Inline style of this layer. Any CSS property can be edited here."),
            )
            .child(Input::new(&self.inspector.input(Field::Css)).xsmall())
            .into_any_element()
    }

    pub(super) fn render_inspector(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = window;
        let theme = cx.theme().clone();
        let Some(id) = self.editor.primary() else {
            let page = self.editor.doc.pages.get(self.editor.page);
            let count = page.map_or(0, |p| p.artboards.len());
            return v_flex()
                .p_4()
                .gap_3()
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(page.map_or_else(String::new, |p| p.name.clone())),
                )
                .child(format!(
                    "{count} artboard(s). Select a layer to edit its design."
                ))
                .child(
                    v_flex()
                        .gap_1()
                        .text_xs()
                        .child("V select · F frame · R rectangle · T text · C comment")
                        .child("Space-drag or middle-drag to pan · Ctrl/⌘ + scroll to zoom")
                        .child("Shift+A auto layout · ⌘G group · ⌘D duplicate")
                        .child("Paste HTML anywhere to import it as layers"),
                )
                .into_any_element();
        };
        if self.editor.selection.len() > 1 {
            // Multi-selection edits apply to every selected layer.
        }
        let c = self.computed(id);
        let is_text = self.editor.doc.is_text_layer(id);
        let is_svg = matches!(
            self.editor.doc.get(id).map(|n| &n.kind),
            Some(NodeKind::Svg(_))
        );
        let mut panel = v_flex()
            .pb_8()
            .child(self.render_header(id, cx))
            .child(self.render_frame_section(id, &c, cx));
        if !is_text && !is_svg {
            panel = panel.child(self.render_layout_section(&c, cx));
        }
        panel = panel
            .child(self.render_fill_section(&c, cx))
            .child(self.render_stroke_section(&c, cx));
        if !is_svg {
            panel = panel.child(self.render_text_section(id, &c, cx));
        }
        panel = panel.child(self.render_effects_section(&c, cx));
        if let Some(attrs) = self.render_attributes_section(id, cx) {
            panel = panel.child(attrs);
        }
        panel = panel.child(self.render_css_section(cx));
        div()
            .id("inspector-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(panel)
            .into_any_element()
    }
}
