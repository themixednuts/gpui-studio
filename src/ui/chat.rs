//! The collaboration panel: chat with your MCP agent, its presence on the
//! canvas, and the controls that keep the person in charge.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::*, px,
};

use super::Studio;
use crate::collab::{Actor, Collab};

/// Color agents are drawn in.
pub(crate) const AGENT_COLOR: Hsla = Hsla {
    h: 0.07,
    s: 0.95,
    l: 0.53,
    a: 1.0,
};

/// Create the chat input and its Enter-to-send subscription.
pub(crate) fn chat_input(
    window: &mut Window,
    cx: &mut Context<Studio>,
) -> (Entity<InputState>, Subscription) {
    let input =
        cx.new(|cx| InputState::new(window, cx).placeholder("Message your agent… (Enter to send)"));
    let subscription =
        cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary, shift } = event
                && !secondary
                && !shift
            {
                this.send_chat(window, cx);
            }
        });
    (input, subscription)
}

impl Studio {
    /// Send the composer's text as the person's message, with the current
    /// selection attached as context.
    pub(crate) fn send_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chat_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let selection = self.editor.selection.clone();
        if self
            .editor
            .collab
            .post(Actor::Person, &text, selection, None)
            .is_some()
        {
            self.chat_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.editor.status = "Sent to your agent".to_owned();
        }
        cx.notify();
    }

    /// Show or hide the chat panel.
    pub(crate) fn toggle_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.chat_open = !self.chat_open;
        if self.chat_open {
            self.chat_input
                .update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.canvas_focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Agent presences to draw on the canvas: (name, status, window bounds).
    pub(crate) fn presence_overlays(&self) -> Vec<(String, String, Vec<gpui_kit::Bounds<Pixels>>)> {
        self.editor
            .collab
            .presence()
            .iter()
            .filter(|(_, p)| Collab::is_active(p))
            .map(|(name, p)| {
                let bounds = p
                    .nodes
                    .iter()
                    .filter_map(|id| self.canvas.layout.borrow().get(id).copied())
                    .collect();
                (name.clone(), p.status.clone(), bounds)
            })
            .collect()
    }

    /// Name tags above each agent's focus, positioned in canvas space.
    pub(crate) fn render_presence_tags(&self, origin: Point<Pixels>) -> Vec<AnyElement> {
        self.presence_overlays()
            .into_iter()
            .filter_map(|(name, status, bounds)| {
                let first = bounds.first()?;
                let local = first.origin - origin;
                let label = if status.is_empty() {
                    name.clone()
                } else {
                    format!("{name} · {status}")
                };
                Some(
                    div()
                        .id(SharedString::from(format!("agent-presence-{name}")))
                        .role(gpui_kit::Role::Status)
                        .aria_label(label.clone())
                        .absolute()
                        .left(local.x)
                        .top(local.y - px(22.0))
                        .max_w(px(320.0))
                        .h(px(20.0))
                        .px_1p5()
                        .flex()
                        .items_center()
                        .gap_1()
                        .rounded(px(5.0))
                        .bg(AGENT_COLOR)
                        .text_color(gpui_kit::white())
                        .text_xs()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(Icon::new(Lucide::Sparkles).xsmall())
                        .child(label)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The floating chat panel.
    pub(crate) fn render_chat(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.chat_open {
            return None;
        }
        let theme = cx.theme().clone();
        let collab = &self.editor.collab;
        let agent = collab.agent_name().to_owned();
        let presence = collab
            .presence()
            .get(&agent)
            .filter(|p| Collab::is_active(p));
        let mut log = v_flex().gap_2().p_3();
        // Messages and agent activity, interleaved by time.
        let mut entries: Vec<(u64, AnyElement)> = Vec::new();
        for message in collab.messages() {
            let mine = message.author == Actor::Person;
            let layers: Vec<String> = message
                .nodes
                .iter()
                .filter(|id| self.editor.doc.contains(**id))
                .take(3)
                .map(|id| self.editor.doc.display_name(*id))
                .collect();
            let bubble = v_flex()
                .max_w(px(250.0))
                .px_2p5()
                .py_1p5()
                .gap_0p5()
                .rounded(px(10.0))
                .text_sm()
                .when(mine, |b| {
                    b.bg(theme.primary).text_color(theme.primary_foreground)
                })
                .when(!mine, |b| b.bg(theme.secondary))
                .child(message.text.clone())
                .when(!layers.is_empty(), |b| {
                    b.child(
                        div()
                            .text_xs()
                            .opacity(0.75)
                            .child(format!("on {}", layers.join(", "))),
                    )
                });
            let row = v_flex()
                .id(SharedString::from(format!("chat-message-{}", message.id)))
                .role(gpui_kit::Role::ListItem)
                .aria_label(format!("{}: {}", message.author.name(), message.text))
                .w_full()
                .gap_0p5()
                .when(mine, |r| r.items_end())
                .when(!mine, |r| {
                    r.items_start().child(
                        div()
                            .text_xs()
                            .text_color(AGENT_COLOR)
                            .font_weight(FontWeight::MEDIUM)
                            .child(message.author.name().to_owned()),
                    )
                })
                .child(bubble);
            entries.push((message.at, row.into_any_element()));
        }
        for entry in collab.activity() {
            entries.push((
                entry.at,
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(Lucide::Pencil).xsmall())
                    .child(format!("{} · {}", entry.actor.name(), entry.summary))
                    .into_any_element(),
            ));
        }
        entries.sort_by_key(|(at, _)| *at);
        let empty = entries.is_empty();
        for (_, element) in entries {
            log = log.child(element);
        }
        if empty {
            log = log.child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Ask your agent for changes here. Messages reach any MCP agent connected to Studio (it calls read_messages); your selection is attached as context. You can keep working while it edits."),
            );
        }
        let unread = collab.unread_for_agent();
        let paused = collab.paused;
        let follow = collab.follow;
        Some(
            v_flex()
                .id("chat-panel")
                .occlude()
                .w(px(340.0))
                .h_full()
                .rounded(px(12.0))
                .bg(theme.popover)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .overflow_hidden()
                .child(
                    h_flex()
                        .h(px(40.0))
                        .px_3()
                        .gap_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .size(px(8.0))
                                .rounded_full()
                                .bg(if presence.is_some() {
                                    AGENT_COLOR
                                } else {
                                    theme.muted_foreground
                                }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(agent.clone()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .truncate()
                                        .text_color(theme.muted_foreground)
                                        .child(match presence {
                                            Some(p) if !p.status.is_empty() => p.status.clone(),
                                            Some(_) => "Working".to_owned(),
                                            None if unread > 0 => {
                                                format!("{unread} waiting for the agent")
                                            }
                                            None => "Idle".to_owned(),
                                        }),
                                ),
                        )
                        .child(
                            Button::new("chat-follow")
                                .icon(Icon::new(Lucide::Eye))
                                .ghost()
                                .xsmall()
                                .selected(follow)
                                .tooltip(if follow {
                                    "Following the agent's selection"
                                } else {
                                    "Follow the agent's selection"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.editor.collab.follow = !this.editor.collab.follow;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("chat-pause")
                                .icon(Icon::new(if paused { Lucide::Play } else { Lucide::Lock }))
                                .ghost()
                                .xsmall()
                                .selected(paused)
                                .tooltip(if paused {
                                    "Agent edits paused — click to allow"
                                } else {
                                    "Pause agent edits"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.editor.collab.paused = !this.editor.collab.paused;
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("chat-close")
                                .icon(Icon::new(Lucide::X))
                                .ghost()
                                .xsmall()
                                .tooltip("Close (⌘J)")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.toggle_chat(window, cx)),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("chat-log")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .child(log),
                )
                .child(
                    v_flex()
                        .p_2()
                        .gap_1()
                        .border_t_1()
                        .border_color(theme.border)
                        .when(!self.editor.selection.is_empty(), |this| {
                            let names: Vec<String> = self
                                .editor
                                .selection
                                .iter()
                                .take(3)
                                .map(|id| self.editor.doc.display_name(*id))
                                .collect();
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("Attached: {}", names.join(", "))),
                            )
                        })
                        .child(Input::new(&self.chat_input).small()),
                )
                .into_any_element(),
        )
    }
}
