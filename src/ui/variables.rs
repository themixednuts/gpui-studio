//! The Variables tab: project-wide design tokens (CSS custom properties).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::*, px,
};

use super::Studio;
use super::paint::hsla;
use crate::model::Color;
use crate::model::variables::{VariableKind, kind_of, resolve};

/// Kinds offered by the "new variable" menu: (label, name prefix, default value).
const NEW_KINDS: [(&str, &str, &str); 3] = [
    ("Color", "color", "#0d99ff"),
    ("Number", "space", "16px"),
    ("Text", "font", "Geist, sans-serif"),
];

impl Studio {
    /// Start editing a variable's name or value in place.
    pub(super) fn begin_variable_edit(
        &mut self,
        name: &str,
        value_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = if value_field {
            self.editor
                .doc
                .variables
                .get(name)
                .cloned()
                .unwrap_or_default()
        } else {
            name.to_owned()
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let target = name.to_owned();
        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    return;
                }
                let Some((name, value_field, _)) = this.variable_edit.take() else {
                    return;
                };
                if name != target {
                    this.variable_edit = Some((name, value_field, input.clone()));
                    return;
                }
                let text = input.read(cx).value().trim().to_string();
                if value_field {
                    if this.editor.doc.variables.get(&name) != Some(&text) {
                        this.apply(window, cx, |e| e.set_variable(&name, &text));
                    }
                } else if text != name && !text.is_empty() {
                    this.apply(window, cx, |e| e.rename_variable(&name, &text));
                }
                this.editor.history.seal();
                cx.notify();
            },
        );
        self._subscriptions.push(subscription);
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.variable_edit = Some((name.to_owned(), value_field, input));
        cx.notify();
    }

    fn add_variable(
        &mut self,
        prefix: &str,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let vars = &self.editor.doc.variables;
        let name = (1..)
            .map(|n| format!("{prefix}-{n}"))
            .find(|n| !vars.contains_key(n))
            .unwrap_or_else(|| prefix.to_owned());
        if let Some(name) = self.apply(window, cx, |e| e.set_variable(&name, value)) {
            self.editor.history.seal();
            self.begin_variable_edit(&name, false, window, cx);
        }
    }

    pub(super) fn render_variables(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let vars = &self.editor.doc.variables;
        let entity = cx.entity();
        let header = h_flex()
            .px_3()
            .h(px(32.0))
            .justify_between()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .child("Variables")
            .child(
                Button::new("add-variable")
                    .icon(Icon::new(Lucide::Plus))
                    .ghost()
                    .xsmall()
                    .tooltip("New variable")
                    .dropdown_menu(move |mut menu, _, _| {
                        for (label, prefix, value) in NEW_KINDS {
                            let entity = entity.clone();
                            menu = menu.item(PopupMenuItem::new(label).on_click(
                                move |_, window, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.add_variable(prefix, value, window, cx);
                                    });
                                },
                            ));
                        }
                        menu
                    }),
            );
        let mut panel = v_flex().pb_4().child(header);
        if vars.is_empty() {
            return panel
                .child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Colors, spacing, and fonts shared by every artboard. Layers use them as var(--name)."),
                )
                .into_any_element();
        }
        for (group, kind) in [
            ("Colors", VariableKind::Color),
            ("Numbers", VariableKind::Number),
            ("Other", VariableKind::Other),
        ] {
            let names: Vec<&String> = vars
                .iter()
                .filter(|(_, v)| kind_of(v, vars) == kind)
                .map(|(n, _)| n)
                .collect();
            if names.is_empty() {
                continue;
            }
            panel = panel.child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(group),
            );
            for name in names {
                panel = panel.child(self.render_variable_row(name, kind, cx));
            }
        }
        panel.into_any_element()
    }

    fn render_variable_row(
        &self,
        name: &str,
        kind: VariableKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let vars = &self.editor.doc.variables;
        let value = vars.get(name).cloned().unwrap_or_default();
        let resolved = resolve(&value, vars).unwrap_or_else(|| value.clone());
        let usage = self.editor.doc.variable_usage(name);
        let editing = self
            .variable_edit
            .as_ref()
            .filter(|(n, _, _)| n == name)
            .map(|(_, value_field, input)| (*value_field, input.clone()));
        let group: SharedString = format!("var-{name}").into();
        let swatch = if kind == VariableKind::Color {
            div()
                .flex_shrink_0()
                .size(px(14.0))
                .rounded(px(3.0))
                .border_1()
                .border_color(theme.border)
                .bg(Color::parse_loose(&resolved).map_or(gpui_kit::transparent_black(), hsla))
                .into_any_element()
        } else {
            div()
                .flex_shrink_0()
                .size(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(Lucide::Variable)
                        .xsmall()
                        .text_color(theme.muted_foreground),
                )
                .into_any_element()
        };
        let field = |value_field: bool, text: String| -> AnyElement {
            match &editing {
                Some((v, input)) if *v == value_field => div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(input).xsmall())
                    .into_any_element(),
                _ => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(value_field, |d| d.text_color(theme.muted_foreground))
                    .child(text)
                    .into_any_element(),
            }
        };
        let name_owned = name.to_owned();
        let name_for_value = name.to_owned();
        let name_for_delete = name.to_owned();
        h_flex()
            .id(SharedString::from(format!("variable-row-{name}")))
            .group(group.clone())
            .h(px(28.0))
            .mx_1p5()
            .px_1p5()
            .gap_2()
            .rounded(px(6.0))
            .hover(|this| this.bg(theme.secondary))
            .text_xs()
            .child(swatch)
            .child(
                div()
                    .id(SharedString::from(format!("variable-name-{name}")))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .child(field(false, format!("--{name}")))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            this.begin_variable_edit(&name_owned, false, window, cx);
                        }
                    })),
            )
            .child(
                div()
                    .id(SharedString::from(format!("variable-value-{name}")))
                    .w(px(88.0))
                    .flex()
                    .child(field(true, value))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.begin_variable_edit(&name_for_value, true, window, cx);
                    })),
            )
            .child(
                div()
                    .w(px(18.0))
                    .text_color(theme.muted_foreground)
                    .child(if usage > 0 {
                        usage.to_string()
                    } else {
                        String::new()
                    }),
            )
            .child(
                div().invisible().group_hover(group, |s| s.visible()).child(
                    Button::new(SharedString::from(format!("variable-delete-{name}")))
                        .icon(Icon::new(Lucide::Trash))
                        .ghost()
                        .xsmall()
                        .tooltip(if usage > 0 {
                            "Delete (layers keep the value)"
                        } else {
                            "Delete"
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.apply(window, cx, |e| e.delete_variable(&name_for_delete));
                        })),
                ),
            )
            .into_any_element()
    }
}
