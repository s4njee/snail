//! OpenPGP key management: import existing keys, inspect identity/fingerprint/expiry, and unlock a
//! secret key for this session with optional OS-keychain persistence. Key generation is
//! intentionally absent in v1.

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::*;
use snail_core::pgp::KeySummary;
use snail_ui::text::TextRole;

use crate::mail_model::MailModel;
use crate::settings::{self, PassphrasePolicy};
use crate::style;

pub fn open(mail: MailModel, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(620.0), px(520.0)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(520.0), px(380.0))),
            window_decorations: Some(WindowDecorations::Client),
            ..TitleBar::window_options()
        },
        move |window, cx| {
            let view = cx.new(|cx| KeyManager::new(mail, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        },
    );
}

struct KeyManager {
    mail: MailModel,
    keys: Vec<KeySummary>,
    selected_secret: Option<usize>,
    passphrase: Entity<InputState>,
    wkd_address: Entity<InputState>,
    wkd_enabled: bool,
    remember: bool,
    status: Option<String>,
    focus: FocusHandle,
}

impl KeyManager {
    fn new(mail: MailModel, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let passphrase = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Passphrase")
                .masked(true)
        });
        let wkd_address =
            cx.new(|cx| InputState::new(window, cx).placeholder("person@example.com"));
        let keys = mail.pgp_keys();
        let pgp = settings::pgp(cx);
        Self {
            mail,
            keys,
            selected_secret: None,
            passphrase,
            wkd_address,
            wkd_enabled: pgp.wkd_discovery,
            remember: pgp.passphrase_policy == PassphrasePolicy::Keychain,
            status: None,
            focus: cx.focus_handle(),
        }
    }

    fn choose_key(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Import an existing OpenPGP key")
            .add_filter("OpenPGP key", &["asc", "pgp", "gpg", "key"])
            .pick_file()
        else {
            return;
        };
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.import_pgp_key(&path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.keys = this.mail.pgp_keys();
                this.status = Some(match result {
                    Ok(key) => format!("Imported {}", key.fingerprint),
                    Err(error) => format!("Import failed: {error:#}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn unlock(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_secret else {
            return;
        };
        let Some(key) = self.keys.get(index) else {
            return;
        };
        let passphrase = self.passphrase.read(cx).value().to_string();
        self.status = Some(
            match self
                .mail
                .unlock_pgp_key(&key.fingerprint, passphrase, self.remember)
            {
                Ok(()) if self.remember => {
                    "Unlocked for this session and saved in the OS keychain".into()
                }
                Ok(()) => "Unlocked for this session".into(),
                Err(error) => format!("Could not store passphrase: {error:#}"),
            },
        );
        self.passphrase
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
    }

    fn lookup_wkd(&mut self, cx: &mut Context<Self>) {
        let address = self.wkd_address.read(cx).value().trim().to_string();
        if address.is_empty() {
            return;
        }
        self.status = Some("Looking up WKD…".into());
        let mail = self.mail.clone();
        let task = cx
            .background_executor()
            .spawn(async move { mail.discover_pgp_key(&address) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.keys = this.mail.pgp_keys();
                this.status = Some(match result {
                    Ok(Some(key)) => format!("Discovered {} with WKD", key.fingerprint),
                    Ok(None) => "WKD has no key for that address".into(),
                    Err(error) => format!("WKD lookup failed: {error:#}"),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for KeyManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);
        let button = |label: &str| {
            div()
                .px_3()
                .py_1()
                .rounded(px(palette.radii.button))
                .border_1()
                .border_color(style::color(palette.colors.border_strong))
                .text_size(px(12.0))
                .text_color(style::color(palette.colors.secondary))
                .hover(|this| this.bg(style::color(palette.colors.sunken)))
                .child(label.to_string())
        };
        let selected = self.selected_secret;
        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .bg(style::color(palette.colors.canvas))
            .when(self.wkd_enabled, |this| this.child(
                div()
                    .h(px(palette.metrics.titlebar_h))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .window_control_area(WindowControlArea::Drag)
                    .bg(style::color(palette.colors.chrome))
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_hairline))
                    .child(style::text("OpenPGP keys", TextRole::ListHeaderTitle, cx))
                    .child(
                        button("Import key…").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| this.choose_key(cx)),
                        ),
                    ),
            ))
            .when(!self.wkd_enabled, |this| {
                this.child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .child(style::text(
                            "WKD discovery is off. Enable it in Settings → PGP to look up keys.",
                            TextRole::ReadingMeta,
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .child(style::text("Find with WKD", TextRole::ComposeLabel, cx))
                    .child(div().flex_1().child(Input::new(&self.wkd_address)))
                    .child(button("Look up").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.lookup_wkd(cx)),
                    )),
            )
            .child(
                div()
                    .id("pgp-key-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .when(self.keys.is_empty(), |this| {
                        this.child(style::text(
                            "No keys imported. Snail uses existing keys and does not generate new ones in v1.",
                            TextRole::ReadingMeta,
                            cx,
                        ))
                    })
                    .children(self.keys.clone().into_iter().enumerate().map(|(index, key)| {
                        let title = key.user_ids.first().cloned().unwrap_or_else(|| "Unnamed key".into());
                        let expiry = key.expires_at.map_or_else(
                            || "does not expire".into(),
                            |epoch| format!("expires {epoch}"),
                        );
                        div()
                            .p_3()
                            .rounded(px(palette.radii.card))
                            .border_1()
                            .border_color(style::color(if selected == Some(index) {
                                palette.colors.accent
                            } else {
                                palette.colors.border_soft
                            }))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(style::text(title, TextRole::ReadingSenderName, cx))
                                    .child(style::text(
                                        key.fingerprint.clone(),
                                        TextRole::ReadingMeta,
                                        cx,
                                    ))
                                    .child(style::text(
                                        format!(
                                            "{} · {expiry}{}",
                                            if key.secret { "secret + public" } else { "public" },
                                            if key.revoked { " · revoked" } else { "" }
                                        ),
                                        TextRole::ReadingMeta,
                                        cx,
                                    )),
                            )
                            .when(key.secret, |this| {
                                this.child(
                                    button(if selected == Some(index) { "Selected" } else { "Unlock…" })
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.selected_secret = Some(index);
                                                this.status = None;
                                                cx.notify();
                                            }),
                                        ),
                                )
                            })
                    })),
            )
            .when(self.selected_secret.is_some(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .p_4()
                        .border_t_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.passphrase)))
                        .child(
                            button(if self.remember { "✓ Remember in keychain" } else { "Remember in keychain" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.remember = !this.remember;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(button("Unlock").on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| this.unlock(window, cx)),
                        )),
                )
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .flex_none()
                        .px_4()
                        .pb_3()
                        .child(style::text(status, TextRole::ReadingMeta, cx)),
                )
            })
    }
}
