//! The compose window (plan.md E7.1): a **second window**, not a modal, so it survives navigation.
//! Built on gpui-kit's `Input`/`Textarea`, which E0.2 validated.

use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::compose::{self, Draft};
use snail_core::mime::Recipient;
use snail_ui::text::TextRole;

use crate::mail_model::MailModel;
use crate::style;

/// Open a compose window with the given draft (E7.1). `cx` is an `App`, so this can be called from
/// any listener.
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
        }
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut draft = self.draft.clone();
        draft.to = parse_addresses(&self.to.read(cx).value().to_string());
        draft.cc = parse_addresses(&self.cc.read(cx).value().to_string());
        draft.subject = self.subject.read(cx).value().to_string();
        draft.body_text = self.body.read(cx).value().to_string();

        if draft.to.is_empty() {
            log::warn!("refusing to send with no recipients");
            return;
        }
        match compose::build_raw(&draft) {
            Ok(raw) => {
                // Parked in `pending_op` (E7.10); the send worker dispatches it.
                match self.mail.enqueue_send(&raw) {
                    Some(hash) => log::info!("send queued ({hash})"),
                    None => log::warn!("no account to send from"),
                }
                window.remove_window();
            }
            Err(error) => log::error!("could not build the message: {error:#}"),
        }
    }
}

impl Render for Compose {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Cmd/Ctrl+Shift+D sends, per the handoff (E7.1/E15.4).
                let modifiers = event.keystroke.modifiers;
                if event.keystroke.key == "d" && modifiers.platform && modifiers.shift {
                    this.send(window, cx);
                }
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
                            .child(style::text("New message", TextRole::ListHeaderTitle, cx)),
                    )
                    .child(
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
                                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                                        window.remove_window()
                                    })
                                    .child(style::text("Cancel", TextRole::ButtonLabel, cx)),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .rounded(px(palette.radii.button))
                                    .bg(style::color(palette.colors.accent))
                                    .text_color(rgb(0xffffff))
                                    .on_mouse_down(MouseButton::Left, cx.listener(
                                        |this, _, window, cx| this.send(window, cx),
                                    ))
                                    .child(style::text("Send", TextRole::ButtonLabelFilled, cx)),
                            ),
                    ),
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
            .child(field("Subject", Input::new(&self.subject).into_any_element()))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_5()
                    .py_3()
                    .child(Textarea::new(&self.body).size_full()),
            )
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
