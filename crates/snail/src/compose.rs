//! The compose window (plan.md E7.1–E7.10): recipient tokens (7.2), contacts autocomplete (7.3),
//! attachments (7.7), a minimal rich-text mode (7.8), draft autosave (7.6) and undo send (7.10).

use std::time::{Duration, Instant};

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_core::compose::{self, Attachment, Draft};
use snail_core::mime::Recipient;
use snail_core::store::ContactRow;
use snail_ui::text::TextRole;

use crate::commands::RunCommand;
use crate::mail_model::MailModel;
use crate::style;
use snail_ui::commands::CommandId;

const AUTOSAVE: Duration = Duration::from_millis(1000);
const MAX_SUGGESTIONS: u32 = 5;

pub fn open(mail: MailModel, mut draft: Draft, cx: &mut App) {
    let sender = draft
        .from_addr
        .clone()
        .or_else(|| mail.first_account_address());
    if draft.from_addr.is_none() {
        draft.from_addr = sender.clone();
    }
    if let Some(signature) = mail
        .signature_for(sender.as_deref())
        .or_else(|| crate::settings::signature_for(sender.as_deref(), cx))
    {
        let marker = format!("-- \n{}", signature.trim());
        if !draft.body_text.contains(&marker) {
            draft.body_text = compose::with_signature(&draft.body_text, &signature);
        }
    }
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
    to_tokens: Vec<Recipient>,
    cc_tokens: Vec<Recipient>,
    to_input: Entity<InputState>,
    cc_input: Entity<InputState>,
    subject: Entity<InputState>,
    body: Entity<TextareaState>,
    suggestions: Vec<ContactRow>,
    attachments: Vec<Attachment>,
    rich: bool,
    focus: FocusHandle,
    _focus_lost: Subscription,
    draft_id: Option<i64>,
    dirty: bool,
    last_edit: Option<Instant>,
    saved_at: Option<Instant>,
    send_at: Option<Instant>,
    pgp_sign: bool,
    pgp_encrypt: bool,
    pgp_error: Option<String>,
    pgp_missing: Vec<String>,
    pgp_preflight_pending: bool,
    pgp_preflight_generation: u64,
    preparing_pgp: bool,
    prepared_raw: Option<Vec<u8>>,
    _subscriptions: Vec<Subscription>,
}

impl Compose {
    fn new(mail: MailModel, draft: Draft, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let to_input = cx.new(|cx| InputState::new(window, cx).placeholder("To"));
        let cc_input = cx.new(|cx| InputState::new(window, cx).placeholder("Cc"));
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
        subscriptions.push(cx.subscribe_in(
            &to_input,
            window,
            |this, _state, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_recipient_change(true, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &cc_input,
            window,
            |this, _state, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_recipient_change(false, window, cx);
                }
            },
        ));
        subscriptions.push(cx.subscribe(&subject, |this, _state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.mark_dirty(cx);
            }
        }));
        subscriptions.push(cx.subscribe(&body, |this, _state, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.mark_dirty(cx);
            }
        }));

        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let restore_focus = focus.clone();
        let focus_lost = cx.on_focus_lost(window, move |_this, window, cx| {
            if window.is_window_active() {
                window.focus(&restore_focus, cx);
            }
        });
        let rich = draft.body_html.is_some();
        let to_tokens = draft.to.clone();
        let cc_tokens = draft.cc.clone();
        let pgp = crate::settings::pgp(cx);
        let mut this = Self {
            mail,
            draft,
            to_tokens,
            cc_tokens,
            to_input,
            cc_input,
            subject,
            body,
            suggestions: Vec::new(),
            attachments: Vec::new(),
            rich,
            focus,
            _focus_lost: focus_lost,
            draft_id: None,
            dirty: false,
            last_edit: None,
            saved_at: None,
            send_at: None,
            pgp_sign: pgp.sign_by_default,
            pgp_encrypt: pgp.encrypt_by_default,
            pgp_error: None,
            pgp_missing: Vec::new(),
            pgp_preflight_pending: false,
            pgp_preflight_generation: 0,
            preparing_pgp: false,
            prepared_raw: None,
            _subscriptions: subscriptions,
        };
        if this.pgp_encrypt {
            this.refresh_pgp_preflight(cx);
        }
        this
    }

    fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.last_edit = Some(Instant::now());
        cx.notify();
    }

    /// On a `To`/`Cc` change: a trailing `,`/`;` commits a token, otherwise refresh suggestions.
    fn on_recipient_change(&mut self, to: bool, window: &mut Window, cx: &mut Context<Self>) {
        let input = if to {
            self.to_input.clone()
        } else {
            self.cc_input.clone()
        };
        let value = input.read(cx).value().to_string();
        if value.ends_with([',', ';']) {
            let tokens = if to {
                &mut self.to_tokens
            } else {
                &mut self.cc_tokens
            };
            for part in value.split([',', ';']) {
                let part = part.trim();
                if !part.is_empty() {
                    tokens.push(Recipient {
                        name: None,
                        address: part.to_string(),
                    });
                }
            }
            input.update(cx, |state, cx| state.set_value("", window, cx));
            self.suggestions.clear();
        } else if to {
            self.suggestions = self.mail.suggest_contacts(value.trim(), MAX_SUGGESTIONS);
        }
        self.mark_dirty(cx);
        self.refresh_pgp_preflight(cx);
    }

    fn commit_suggestion(
        &mut self,
        contact: ContactRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.to_tokens.push(Recipient {
            name: contact.name,
            address: contact.address,
        });
        self.suggestions.clear();
        let input = self.to_input.clone();
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.mark_dirty(cx);
        self.refresh_pgp_preflight(cx);
    }

    fn remove_token(&mut self, to: bool, index: usize, cx: &mut Context<Self>) {
        let tokens = if to {
            &mut self.to_tokens
        } else {
            &mut self.cc_tokens
        };
        if index < tokens.len() {
            tokens.remove(index);
        }
        self.mark_dirty(cx);
        self.refresh_pgp_preflight(cx);
    }

    fn collect(&self, cx: &App) -> Draft {
        let mut draft = self.draft.clone();
        let mut to = self.to_tokens.clone();
        let trailing = self.to_input.read(cx).value().to_string();
        if trailing.contains('@') {
            to.push(Recipient {
                name: None,
                address: trailing.trim().to_string(),
            });
        }
        draft.to = to;
        draft.cc = self.cc_tokens.clone();
        draft.subject = self.subject.read(cx).value().to_string();
        draft.body_text = self.body.read(cx).value().to_string();
        draft.body_html = self.rich.then(|| self.body.read(cx).value().to_string());
        draft.attachments = self.attachments.clone();
        draft
    }

    fn autosave(&mut self, cx: &mut Context<Self>) {
        let draft = self.collect(cx);
        let raw = compose::build_raw(&draft).unwrap_or_default();
        let to_json = serde_json::to_string(&draft.to).unwrap_or_else(|_| "[]".into());
        if let Some(id) = self.mail.save_draft(
            self.draft_id,
            &draft.subject,
            &draft.body_text,
            &to_json,
            &raw,
        ) {
            self.draft_id = Some(id);
        }
        self.dirty = false;
        self.saved_at = Some(Instant::now());
    }

    fn begin_send(&mut self, cx: &mut Context<Self>) {
        let draft = self.collect(cx);
        if draft.to.is_empty() {
            log::warn!("not sending with no recipients");
            return;
        }
        self.autosave(cx);
        let Ok(raw) = compose::build_raw(&draft) else {
            self.pgp_error = Some("Could not build this message".into());
            return;
        };
        let recipients = draft
            .to
            .iter()
            .chain(&draft.cc)
            .chain(&draft.bcc)
            .map(|recipient| recipient.address.clone())
            .collect::<Vec<_>>();
        if self.pgp_encrypt && self.pgp_preflight_pending {
            self.pgp_error = Some("Still checking recipient encryption keys…".into());
            cx.notify();
            return;
        }
        if self.pgp_encrypt && !self.pgp_missing.is_empty() {
            self.pgp_error = Some(format!(
                "Can't encrypt — key missing for {}",
                self.pgp_missing.join(", ")
            ));
            cx.notify();
            return;
        }
        self.pgp_error = None;
        if !self.pgp_sign && !self.pgp_encrypt {
            self.prepared_raw = Some(raw);
            let seconds = crate::settings::undo_send_seconds(cx);
            self.send_at = Some(Instant::now() + Duration::from_secs(seconds));
            cx.notify();
            return;
        }

        self.preparing_pgp = true;
        let mail = self.mail.clone();
        let sign = self.pgp_sign;
        let encrypt = self.pgp_encrypt;
        let task = cx
            .background_executor()
            .spawn(async move { mail.protect_outbound(&raw, &recipients, sign, encrypt) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.preparing_pgp = false;
                match result {
                    Ok(raw) => {
                        this.prepared_raw = Some(raw);
                        let seconds = crate::settings::undo_send_seconds(cx);
                        this.send_at = Some(Instant::now() + Duration::from_secs(seconds));
                    }
                    Err(error) => {
                        this.pgp_error = Some(format!("Could not secure message: {error:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn dispatch(&mut self, cx: &mut Context<Self>) {
        let raw = self
            .prepared_raw
            .take()
            .or_else(|| compose::build_raw(&self.collect(cx)).ok());
        if let Some(raw) = raw {
            match self.mail.enqueue_send(&raw) {
                Some(hash) => {
                    log::info!("send queued ({hash})");
                    // The undo window has already passed; send now rather than at the next poll.
                    if let Some(hub) = crate::sync_model::SyncHub::global(cx) {
                        hub.update(cx, |hub, cx| hub.sync_now(cx));
                    }
                }
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

    fn attach(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = rfd::FileDialog::new().pick_files() else {
            return;
        };
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else {
                log::warn!("could not read {}", path.display());
                continue;
            };
            let filename = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "attachment".into());
            if bytes.len() > Attachment::WARN_BYTES {
                log::warn!("{filename} is over the 20 MB provider limit");
            }
            self.attachments.push(Attachment {
                content_type: content_type_for(&filename).to_string(),
                filename,
                bytes,
            });
        }
        self.mark_dirty(cx);
    }

    /// The minimal format bar (E7.8): switch to rich mode and insert a tag at the caret.
    fn format(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.rich = true;
        let body = self.body.clone();
        let snippet = format!("<{tag}></{tag}>");
        body.update(cx, |state, cx| state.insert(snippet, window, cx));
        self.mark_dirty(cx);
    }

    /// Certificate validation/key selection can be expensive, so even the live preflight runs on
    /// the background executor. Its generation guard drops stale results while recipients change.
    fn refresh_pgp_preflight(&mut self, cx: &mut Context<Self>) {
        self.pgp_preflight_generation += 1;
        let generation = self.pgp_preflight_generation;
        if !self.pgp_encrypt {
            self.pgp_missing.clear();
            self.pgp_preflight_pending = false;
            return;
        }
        let recipients = self
            .to_tokens
            .iter()
            .chain(&self.cc_tokens)
            .chain(&self.draft.bcc)
            .map(|recipient| recipient.address.clone())
            .collect::<Vec<_>>();
        let mail = self.mail.clone();
        let discover = crate::settings::pgp(cx).wkd_discovery;
        self.pgp_preflight_pending = true;
        let task = cx.background_executor().spawn(async move {
            let mut missing = mail.missing_pgp_encryption_keys(&recipients);
            if discover {
                for address in &missing {
                    if let Err(error) = mail.discover_pgp_key(address) {
                        log::warn!("WKD lookup for {address} failed: {error:#}");
                    }
                }
                missing = mail.missing_pgp_encryption_keys(&recipients);
            }
            missing
        });
        cx.spawn(async move |this, cx| {
            let missing = task.await;
            this.update(cx, |this, cx| {
                if this.pgp_preflight_generation != generation {
                    return;
                }
                this.pgp_missing = missing;
                this.pgp_preflight_pending = false;
                this.pgp_error = if this.pgp_missing.is_empty() {
                    None
                } else {
                    Some(format!(
                        "Can't encrypt — key missing for {}",
                        this.pgp_missing.join(", ")
                    ))
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let sending = self.send_at.is_some();
        let remaining = self
            .send_at
            .map(|deadline| deadline.saturating_duration_since(now).as_secs() + 1)
            .unwrap_or(0);

        let header_action = if self.preparing_pgp {
            style::text("Securing message…", TextRole::ButtonLabel, cx).into_any_element()
        } else if sending {
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
                            cx.listener(|this, _, _w, cx| this.undo_send(cx)),
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
                            cx.listener(|this, _, _w, cx| this.begin_send(cx)),
                        )
                        .child(style::text("Send", TextRole::ButtonLabelFilled, cx)),
                )
                .into_any_element()
        };

        // Recipient tokens (7.2): pills, then the input for the next address.
        let tokens = |this: &Self,
                      to: bool,
                      list: &[Recipient],
                      input: &Entity<InputState>,
                      cx: &mut Context<Self>| {
            let palette = style::palette(cx);
            let mut row = div().flex().flex_wrap().items_center().gap_1();
            for (index, recipient) in list.iter().enumerate() {
                let valid = recipient.address.contains('@');
                let color = if valid {
                    palette.colors.accent_text
                } else {
                    palette.colors.danger
                };
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .rounded(px(palette.radii.token))
                        .bg(style::color(palette.colors.accent_tint))
                        .text_size(px(12.0))
                        .text_color(style::color(color))
                        .child(
                            recipient
                                .name
                                .clone()
                                .unwrap_or_else(|| recipient.address.clone()),
                        )
                        .child(
                            div()
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _w, cx| {
                                        this.remove_token(to, index, cx);
                                    }),
                                )
                                .child("×"),
                        ),
                );
            }
            let _ = this;
            let _ = input;
            row.child(div().flex_1().min_w_0().child(Input::new(input)))
        };

        let field = |label: &str, content: AnyElement, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_5()
                .py_2()
                .border_b_1()
                .border_color(style::color(palette.colors.border_soft))
                .child(div().w(px(52.0)).flex_none().child(style::text(
                    label.to_string(),
                    TextRole::ComposeLabel,
                    cx,
                )))
                .child(div().flex_1().min_w_0().child(content))
        };

        let suggestions = if self.suggestions.is_empty() {
            None
        } else {
            Some(
                div()
                    .flex()
                    .flex_col()
                    .px_5()
                    .py_1()
                    .bg(style::color(palette.colors.card))
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .children(self.suggestions.clone().into_iter().enumerate().map(
                        |(index, contact)| {
                            let address = contact.address.clone();
                            div()
                                .px_2()
                                .py_1()
                                .rounded(px(palette.radii.control))
                                .text_size(px(12.0))
                                .text_color(style::color(palette.colors.secondary))
                                .hover(|this| {
                                    this.bg(style::color(snail_ui::theme::LIGHT.accent_tint))
                                })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        if let Some(contact) = this.suggestions.get(index).cloned()
                                        {
                                            this.commit_suggestion(contact, window, cx);
                                        }
                                    }),
                                )
                                .child(match &contact.name {
                                    Some(name) => format!("{name} <{address}>"),
                                    None => address,
                                })
                        },
                    )),
            )
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(style::color(palette.colors.canvas))
            .text_color(style::color(palette.colors.ink))
            .key_context("Global Compose")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, action: &RunCommand, window, cx| {
                match action.0 {
                    CommandId::Send => this.begin_send(cx),
                    CommandId::Cancel => window.remove_window(),
                    CommandId::Quit => cx.quit(),
                    _ => return,
                }
                cx.stop_propagation();
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
                                if self.to_tokens.is_empty() {
                                    "New message"
                                } else {
                                    "Message"
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
            .child(field(
                "From",
                style::text(
                    self.draft
                        .from_addr
                        .clone()
                        .unwrap_or_else(|| "choose account".into()),
                    TextRole::ComposeSubject,
                    cx,
                )
                .into_any_element(),
                cx,
            ))
            .when_some(self.pgp_error.clone(), |this, error| {
                this.child(
                    div()
                        .px_5()
                        .py_2()
                        .bg(style::color(palette.colors.sunken))
                        .child(style::text(error, TextRole::ReadingMeta, cx)),
                )
            })
            .child(field(
                "To",
                tokens(self, true, &self.to_tokens.clone(), &self.to_input, cx).into_any_element(),
                cx,
            ))
            .when_some(suggestions, |this, suggestions| this.child(suggestions))
            .child(field(
                "Cc",
                tokens(self, false, &self.cc_tokens.clone(), &self.cc_input, cx).into_any_element(),
                cx,
            ))
            .child(field(
                "Subject",
                Input::new(&self.subject).into_any_element(),
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_5()
                    .py_3()
                    .child(Textarea::new(&self.body).size_full()),
            )
            .child(
                // Format bar (7.8) and attachments (7.7).
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_2()
                    .bg(style::color(palette.colors.sunken))
                    .border_t_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .children(
                        [("B", "b"), ("I", "i"), ("List", "ul"), ("Link", "a")]
                            .into_iter()
                            .map(|(label, tag)| {
                                let tag = tag.to_string();
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(palette.radii.control))
                                    .text_size(px(12.0))
                                    .text_color(style::color(palette.colors.muted))
                                    .hover(|this| this.bg(style::color(palette.colors.card)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.format(&tag, window, cx)
                                        }),
                                    )
                                    .child(label)
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(palette.radii.control))
                            .text_size(px(12.0))
                            .text_color(style::color(if self.pgp_sign {
                                palette.colors.accent
                            } else {
                                palette.colors.muted
                            }))
                            .hover(|this| this.bg(style::color(palette.colors.card)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _w, cx| {
                                    this.pgp_sign = !this.pgp_sign;
                                    this.pgp_error = None;
                                    cx.notify();
                                }),
                            )
                            .child(if self.pgp_sign { "✓ Sign" } else { "Sign" }),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(palette.radii.control))
                            .text_size(px(12.0))
                            .text_color(style::color(if self.pgp_encrypt {
                                palette.colors.accent
                            } else {
                                palette.colors.muted
                            }))
                            .hover(|this| this.bg(style::color(palette.colors.card)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _w, cx| {
                                    this.pgp_encrypt = !this.pgp_encrypt;
                                    this.pgp_error = None;
                                    this.refresh_pgp_preflight(cx);
                                    cx.notify();
                                }),
                            )
                            .child(if self.pgp_encrypt {
                                "✓ Encrypt"
                            } else {
                                "Encrypt"
                            }),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(palette.radii.control))
                            .text_size(px(12.0))
                            .text_color(style::color(palette.colors.muted))
                            .hover(|this| this.bg(style::color(palette.colors.card)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _w, cx| this.attach(cx)),
                            )
                            .child("Attach…"),
                    )
                    .children(self.attachments.clone().into_iter().enumerate().map(
                        |(index, attachment)| {
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .rounded(px(palette.radii.chip))
                                .bg(style::color(palette.colors.chrome))
                                .text_size(px(11.0))
                                .text_color(style::color(
                                    if attachment.bytes.len() > Attachment::WARN_BYTES {
                                        palette.colors.danger
                                    } else {
                                        palette.colors.secondary
                                    },
                                ))
                                .child(format!(
                                    "{} · {}",
                                    attachment.filename,
                                    human_size(attachment.bytes.len())
                                ))
                                .child(
                                    div()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _w, cx| {
                                                if index < this.attachments.len() {
                                                    this.attachments.remove(index);
                                                }
                                                this.mark_dirty(cx);
                                            }),
                                        )
                                        .child("×"),
                                )
                        },
                    )),
            )
            .into_any_element()
    }
}

fn content_type_for(filename: &str) -> &'static str {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        "doc" | "docx" => "application/msword",
        _ => "application/octet-stream",
    }
}

fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let size = bytes as f64;
    if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.0} KB", size / KB)
    } else {
        format!("{bytes} B")
    }
}
