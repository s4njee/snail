//! The compose window (plan.md E7.1): a **second window**, not a modal, so it survives navigation.
//! Built on gpui-kit's `Input`/`Textarea`, which E0.2 validated. Drafts autosave to the store
//! (E7.6) and Send is held for an undo window before it is queued (E7.10).

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::compose::{self, Draft};
use snail_core::mime::Recipient;
use snail_ui::text::TextRole;

use crate::mail_model::MailModel;
use crate::style;

/// Default undo-send window (E7.10); a settings control lands in E14.4.
const UNDO_SECONDS: u64 = 10;
const AUTOSAVE: Duration = Duration::from_millis(1000);

/// Open a compose window with the given draft (E7.1).
pub fn open(mail: MailModel, draft: Draft, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(620.0), px(520.0)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(520.0), px(420.0))),
            window_decorations: Some(WindowDecorations::Client),
            ..TitleBar::window_options()
        },
        move |window, cx| {
            let view = cx.new(|cx| Compose::new(mail, draft, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        },
    )
    .ok();
}

struct Compose {
    mail: MailModel,
    draft: Draft,
    to: Entity<InputState>,
    cc: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    focus: FocusHandle,
    /// The saved draft row, reused by every autosave (E7.6).
    draft_id: Option<i64>,
    dirty: bool,
    last_edit: Option<Instant>,
    saved_at: Option<Instant>,
    /// Set while the undo-send window is open (E7.10).
    send_at: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl Compose {
    fn new(mail: MailModel, draft: Draft, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let to = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("To")
                .default_value(join_addresses(&draft.to))
        });
        let cc = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Cc")
                .default_value(join_addresses(&draft.cc))
        });
        let subject = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Subject")
                .default_value(draft.subject.clone())
        });
        let body = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Write your message…")
                .default_value(draft.body_text.clone())
        });

        let mut subscriptions = Vec::new();
        for state in [to.clone(), cc.clone(), subject.clone()] {
            subscriptions.push(cx.subscribe(&state, |this, _state, event, cx| {
                if matches!(event, InputEvent::Change) {
                    this.mark_dirty(cx);
                }
            }));
        }
        subscriptions.push(cx.subscribe(&body, |this, _state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.mark_dirty(cx);
            }
        }));

        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            mail,
            draft,
            to,
            cc,
            subject,
            body,
            focus,
            draft_id: None,
            dirty: false,
            last_edit: None,
            saved_at: None,
            send_at: None,
            _subscriptions: subscriptions,
        }
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        cx.notify();
    }

    /// Read the fields back into a draft.
    fn collect(&self, cx: &App) -> Draft {
        let mut draft = self.draft.clone();
        draft.to = parse_addresses(&self.to.read(cx).value().to_string());
        draft.cc = parse_addresses(&self.cc.read(cx).value().to_string());
        draft.subject = self.subject.read(cx).value().to_string();
        draft.body_text = self.body.read(cx).value().to_string();
        draft
    }

    /// Autosave to the Drafts mailbox (E7.6).
    fn autosave(&mut self, cx: &mut Context<Self>) {
        let draft = self.collect(cx);
        let raw = compose::build_raw(&draft).unwrap_or_default();
        let to_json = serde_json::to_string(&draft.to).unwrap_or_else(|_| "[]".into());
        if let Some(id) = self
            .mail
            .save_draft(self.draft_id, &draft.subject, &draft.body_text, &to_json, &raw)
        {
            self.draft_id = Some(id);
        }
        self.dirty = false;
        self.saved_at = Some(Instant::now());
    }

    /// Start the undo-send window: save the draft, then hold the send (E7.10).
    fn begin_send(&mut self, cx: &mut Context<Self>) {
        if self.collect(cx).to.is_empty() {
            log::warn!("not sending with no recipients");
            return;
        }
        self.autosave(cx);
        self.send_at = Some(Instant::now() + Duration::from_secs(UNDO_SECONDS));
        cx.notify();
    }

    /// The undo window expired: queue the send and remove the draft.
    fn dispatch(&mut self, cx: &mut Context<Self>) {
        let draft = self.collect(cx);
        if let Ok(raw) = compose::build_raw(&draft) {
            match self.mail.enqueue_send(&raw) {
                Some(hash) => log::info!("send queued ({hash})"),
                None => log::warn!("no account to send from"),
            }
        }
        if let Some(id) = self.draft_id.take() {
            self.mail.delete_message(id);
        }
        self.send_at = None;
    }

    fn undo_send(&mut self, cx: &mut Context<Self>) {
        self.send_at = None;
        cx.notify();
    }
}

impl Render for Compose {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = Instant::now();
        if let Some(deadline) = self.send_at {
            if now >= deadline {
                self.dispatch(cx);
                window.remove_window();
                return div().into_any_element();
            }
            cx.on_next_frame(window, |_this, _window, cx| cx.notify());
        } else if self.dirty {
            if self
                .last_edit
                .map_or(true, |edited| now.duration_since(edited) >= AUTOSAVE)
            {
                self.autosave(cx);
            } else {
                cx.on_next_frame(window, |_this, _window, cx| cx.notify());
            }
        }

        let palette = style::palette(cx);
        let field = |label: &str, input: AnyElement| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_5()
                .py_2()
                .border_b_1()
                .border_color(style::color(palette.colors.border_soft))
                .child(
                    div()
                        .w(px(52.0))
                        .flex_none()
                        .child(style::text(label.to_string(), TextRole::ComposeLabel, cx)),
                )
                .child(div().flex_1().min_w_0().child(input))
        };

        let sending = self.send_at.is_some();
        let remaining = self
            .send_at
            .map(|deadline| deadline.saturating_duration_since(now).as_secs() + 1)
            .unwrap_or(0);

        let header_action = if sending {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(style::text(
                    format!("Sending in {remaining}s"),
                    TextRole::ButtonLabel,
                    cx,
                ))
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(palette.radii.button))
                        .border_1()
                        .border_color(style::color(palette.colors.border_strong))
                        .text_color(style::color(palette.colors.secondary))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.undo_send(cx)),
                        )
                        .child(style::text("Undo", TextRole::ButtonLabel, cx)),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(palette.radii.button))
                        .border_1()
                        .border_color(style::color(palette.colors.border_strong))
                        .text_color(style::color(palette.colors.secondary))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.remove_window())
                        .child(style::text("Cancel", TextRole::ButtonLabel, cx)),
                )
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .rounded(px(palette.radii.button))
                        .bg(style::color(palette.colors.accent))
                        .text_color(rgb(0xffffff))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _window, cx| this.begin_send(cx)),
                        )
                        .child(style::text("Send", TextRole::ButtonLabelFilled, cx)),
                )
                .into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key == "d" && modifiers.platform && modifiers.shift {
                    this.begin_send(cx);
                }
                let _ = window;
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(palette.metrics.titlebar_h))
                    .px_4()
                    .bg(style::color(palette.colors.chrome))
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_hairline))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .when(cfg!(target_os = "macos"), |this| this.pl(px(78.0)))
                            .child(style::text(
                                if self.draft.to.is_empty() {
                                    "New message".to_string()
                                } else {
                                    "Message".to_string()
                                },
                                TextRole::ListHeaderTitle,
                                cx,
                            ))
                            .when(self.saved_at.is_some() && !sending, |this| {
                                this.child(style::text("Draft saved", TextRole::ReadingMeta, cx))
                            }),
                    )
                    .child(header_action),
            )
            .child(field("From", {
                let name = self
                    .draft
                    .from_addr
                    .clone()
                    .unwrap_or_else(|| "choose account".into());
                style::text(name, TextRole::ComposeSubject, cx).into_any_element()
            }))
            .child(field("To", Input::new(&self.to).into_any_element()))
            .child(field("Cc", Input::new(&self.cc).into_any_element()))
            .child(field(
                "Subject",
                Input::new(&self.subject).into_any_element(),
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_5()
                    .py_3()
                    .child(Textarea::new(&self.body).size_full()),
            )
            .into_any_element()
    }
}

fn join_addresses(recipients: &[Recipient]) -> String {
    recipients
        .iter()
        .map(|recipient| recipient.address.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Tolerant address parsing for the To/Cc fields (E7.2's tokens land later): split on `,`/`;`.
fn parse_addresses(input: &str) -> Vec<Recipient> {
    input
        .split([',', ';'])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| Recipient {
            name: None,
            address: token.to_string(),
        })
        .collect()
}
