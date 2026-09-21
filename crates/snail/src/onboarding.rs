//! First run: connect a Gmail account (plan.md E3.1, E14.3). Shown in place of the mailbox until
//! an account exists, and again when the user reconnects one.
//!
//! Two steps, because Google requires both. First the **OAuth client** — a personal Snail uses
//! the owner's own Google Cloud "Desktop app" client, chosen as the JSON file the console
//! downloads (or pasted as ID and secret). Then **sign-in**, in the system browser over the
//! loopback redirect. Snail keeps the client and the resulting refresh token in the OS keychain
//! and never sees the Google password.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_services::auth::GoogleOAuth;
use snail_ui::text::TextRole;

use crate::style;
use crate::sync_model::{SetupCollectionKind, SyncHub};

const CONSOLE_URL: &str = "https://console.cloud.google.com/apis/credentials";
const GMAIL_API_URL: &str = "https://console.cloud.google.com/apis/library/gmail.googleapis.com";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Reading the keychain for a saved client.
    Loading,
    /// No client yet: choose or paste one.
    Client,
    /// Ready to sign in.
    SignIn,
    /// The browser is open; waiting for Google to redirect back.
    Waiting,
    /// OAuth and discovery succeeded; no message or event has been pulled yet.
    Choose,
}

pub struct Onboarding {
    hub: Entity<SyncHub>,
    step: Step,
    client: Option<GoogleOAuth>,
    /// Show the paste fields instead of the file button.
    paste: bool,
    client_id: Entity<InputState>,
    client_secret: Entity<InputState>,
    error: Option<String>,
    cancel: Option<Arc<AtomicBool>>,
    /// Bumped per sign-in attempt, so a superseded attempt's late result is ignored.
    attempt: u64,
    connected_address: Option<String>,
}

impl Onboarding {
    pub fn new(hub: Entity<SyncHub>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let client_id = cx.new(|cx| {
            InputState::new(window, cx).placeholder("1234-abc.apps.googleusercontent.com")
        });
        let client_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Client secret")
                .masked(true)
        });
        let load = hub.update(cx, |hub, cx| hub.load_client(cx));
        cx.spawn(async move |this, cx| {
            let client = load.await;
            this.update(cx, |this, cx| {
                let ready = client.is_some();
                this.client = client;
                if ready {
                    // A client is already configured, so there is nothing to ask: go straight to
                    // Google's consent page. The waiting screen offers cancel and retry.
                    this.sign_in(cx);
                } else {
                    this.step = Step::Client;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        Self {
            hub,
            step: Step::Loading,
            client: None,
            paste: false,
            client_id,
            client_secret,
            error: None,
            cancel: None,
            attempt: 0,
            connected_address: None,
        }
    }

    /// "Choose client JSON…": the file Google Cloud Console's "Download JSON" gives you.
    fn choose_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Choose your Google OAuth client JSON")
            .add_filter("JSON", &["json"])
            .pick_file()
        else {
            return;
        };
        let parsed = std::fs::read_to_string(&path)
            .map_err(|error| format!("Could not read {}: {error}", path.display()))
            .and_then(|json| {
                GoogleOAuth::from_client_json(&json).map_err(|error| error.to_string())
            });
        self.use_client(parsed, cx);
    }

    fn save_pasted(&mut self, cx: &mut Context<Self>) {
        let id = self.client_id.read(cx).value().to_string();
        let secret = self.client_secret.read(cx).value().to_string();
        let parsed = GoogleOAuth::checked(&id, &secret).map_err(|error| error.to_string());
        self.use_client(parsed, cx);
    }

    fn use_client(&mut self, parsed: Result<GoogleOAuth, String>, cx: &mut Context<Self>) {
        let client = match parsed {
            Ok(client) => client,
            Err(error) => {
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        self.error = None;
        let save = self
            .hub
            .update(cx, |hub, cx| hub.save_client(client.clone(), cx));
        cx.spawn(async move |this, cx| {
            let saved = save.await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(()) => {
                        this.client = Some(client);
                        this.step = Step::SignIn;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            self.step = Step::Client;
            cx.notify();
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.error = None;
        self.step = Step::Waiting;
        self.attempt += 1;
        let attempt = self.attempt;
        let connect = self
            .hub
            .update(cx, |hub, cx| hub.connect(client, cancel, cx));
        cx.spawn(async move |this, cx| {
            let result = connect.await;
            this.update(cx, |this, cx| {
                if this.attempt != attempt {
                    return;
                }
                this.cancel = None;
                match result {
                    Ok(address) => {
                        this.connected_address = Some(address);
                        this.step = Step::Choose;
                    }
                    Err(error) => {
                        // A cancel is the user's own choice; say nothing.
                        this.error = (error != "Sign-in was cancelled.").then_some(error);
                        this.step = Step::SignIn;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The tab was closed or lost: abandon the waiting listener and start a fresh sign-in.
    fn restart_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.sign_in(cx);
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        cx.notify();
    }

    fn change_client(&mut self, cx: &mut Context<Self>) {
        self.step = Step::Client;
        self.error = None;
        cx.notify();
    }

    fn back_to_mail(&mut self, cx: &mut Context<Self>) {
        self.cancel_sign_in(cx);
        self.hub.update(cx, |hub, cx| hub.cancel_reconnect(cx));
    }

    fn finish_setup(&mut self, cx: &mut Context<Self>) {
        self.hub.update(cx, |hub, cx| hub.finish_setup(cx));
    }
}

fn open_url(url: &str) {
    if let Err(error) = snail_services::auth::open_in_browser(url) {
        log::warn!("could not open {url}: {error:#}");
    }
}

/// Where the secrets end up, in the words each platform uses.
fn keychain_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else {
        "your Secret Service keyring"
    }
}

fn primary_button(label: &'static str, cx: &App) -> Stateful<Div> {
    let palette = style::palette(cx);
    div()
        .id(label)
        .px_4()
        .py(px(7.0))
        .rounded(px(palette.radii.button))
        .bg(style::color(palette.colors.accent))
        .text_color(rgb(0xffffff))
        .cursor_pointer()
        .hover(|this| this.opacity(0.92))
        .child(style::text(label, TextRole::ButtonLabelFilled, cx))
}

fn secondary_button(label: &'static str, cx: &App) -> Stateful<Div> {
    let palette = style::palette(cx);
    div()
        .id(label)
        .px_4()
        .py(px(7.0))
        .rounded(px(palette.radii.button))
        .border_1()
        .border_color(style::color(palette.colors.border_strong))
        .text_color(style::color(palette.colors.secondary))
        .cursor_pointer()
        .hover(|this| this.bg(style::color(palette.colors.sunken)))
        .child(style::text(label, TextRole::ButtonLabel, cx))
}

fn link(label: &'static str, url: &'static str, cx: &App) -> Stateful<Div> {
    let palette = style::palette(cx);
    div()
        .id(label)
        .cursor_pointer()
        .text_color(style::color(palette.colors.accent))
        .text_size(px(12.5))
        .child(label)
        .on_click(move |_, _, _| open_url(url))
}

/// One numbered instruction.
fn instruction(number: usize, body: AnyElement, cx: &App) -> Div {
    let palette = style::palette(cx);
    div()
        .flex()
        .gap_3()
        .items_start()
        .child(
            div()
                .flex_none()
                .size(px(20.0))
                .rounded_full()
                .bg(style::color(palette.colors.accent_tint))
                .flex()
                .items_center()
                .justify_center()
                .child(style::text(number.to_string(), TextRole::SidebarCount, cx)),
        )
        .child(div().flex_1().min_w_0().pt(px(1.0)).child(body))
}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);
        let reconnecting = self.hub.read(cx).has_accounts();

        let (label, title, body): (&str, &str, String) = match self.step {
            Step::Loading => ("Gmail", "Connect Gmail", "Checking the keychain…".into()),
            Step::Client => (
                "Step 1 of 3",
                "Connect your Google Cloud client",
                "Snail talks to Gmail through your own OAuth client, so nothing sits between you \
                 and Google. It takes a few minutes, once."
                    .into(),
            ),
            Step::SignIn => (
                "Step 2 of 3",
                if reconnecting {
                    "Sign in to Gmail again"
                } else {
                    "Sign in with Google"
                },
                format!(
                    "Your browser opens Google's sign-in page. Snail never sees your password — it \
                     keeps only a refresh token, in {}. Google will warn that the app is not \
                     verified: it is your own client, so choose Advanced, then continue.",
                    keychain_name()
                ),
            ),
            Step::Waiting => (
                "Step 2 of 3",
                "Waiting for Google…",
                "Finish signing in in your browser, then come back here. Snail will discover \
                 your mailboxes and calendars before downloading their contents."
                    .into(),
            ),
            Step::Choose => (
                "Step 3 of 3",
                "Choose what to sync",
                format!(
                    "{} is connected. Turn off anything you do not want stored locally, then \
                     start the first sync.",
                    self.connected_address
                        .as_deref()
                        .unwrap_or("Your Google account")
                ),
            ),
        };

        let mut content = div().flex().flex_col().gap_4();
        match self.step {
            Step::Loading => {}
            Step::Client => {
                let steps = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(instruction(
                        1,
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(style::text(
                                "In Google Cloud Console, pick or create a project and enable the \
                                 Gmail API.",
                                TextRole::ReadingMeta,
                                cx,
                            ))
                            .child(link("Enable the Gmail API ↗", GMAIL_API_URL, cx))
                            .into_any_element(),
                        cx,
                    ))
                    .child(instruction(
                        2,
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(style::text(
                                "Under Credentials, create an OAuth client ID of type \"Desktop \
                                 app\". On the consent screen choose External, then Publish the \
                                 app — left in Testing, Google signs you out every 7 days.",
                                TextRole::ReadingMeta,
                                cx,
                            ))
                            .child(link("Open Credentials ↗", CONSOLE_URL, cx))
                            .into_any_element(),
                        cx,
                    ))
                    .child(instruction(
                        3,
                        style::text(
                            "Download the client's JSON and choose it below.",
                            TextRole::ReadingMeta,
                            cx,
                        )
                        .into_any_element(),
                        cx,
                    ));
                content = content.child(steps);
                if self.paste {
                    content = content.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(style::text("Client ID", TextRole::ComposeLabel, cx))
                            .child(Input::new(&self.client_id))
                            .child(style::text("Client secret", TextRole::ComposeLabel, cx))
                            .child(Input::new(&self.client_secret))
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .pt_1()
                                    .child(primary_button("Save client", cx).on_click(
                                        cx.listener(|this, _, _, cx| this.save_pasted(cx)),
                                    ))
                                    .child(secondary_button("Choose JSON instead", cx).on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.paste = false;
                                            cx.notify();
                                        }),
                                    )),
                            ),
                    );
                } else {
                    content = content.child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary_button("Choose client JSON…", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_file(cx))),
                            )
                            .child(secondary_button("Paste ID and secret", cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.paste = true;
                                    this.error = None;
                                    cx.notify();
                                }),
                            )),
                    );
                }
            }
            Step::SignIn => {
                let client_id = self
                    .client
                    .as_ref()
                    .map(|client| client.client_id.clone())
                    .unwrap_or_default();
                content = content
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary_button("Sign in with Google", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.sign_in(cx))),
                            )
                            .child(
                                secondary_button("Use a different client", cx)
                                    .on_click(cx.listener(|this, _, _, cx| this.change_client(cx))),
                            ),
                    )
                    .child(
                        div().min_w_0().child(
                            style::text(format!("Client {client_id}"), TextRole::SidebarCount, cx)
                                .truncate(),
                        ),
                    );
            }
            Step::Waiting => {
                content = content.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            primary_button("Open the browser again", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.restart_sign_in(cx))),
                        )
                        .child(
                            secondary_button("Cancel", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx))),
                        ),
                );
            }
            Step::Choose => {
                let collections = self.hub.read(cx).setup_collections();
                let mut choices = div()
                    .rounded(px(palette.radii.card))
                    .border_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .overflow_hidden();
                for collection in collections {
                    let item = collection.clone();
                    let hub = self.hub.clone();
                    choices = choices.child(
                        div()
                            .id((
                                "setup-collection",
                                ((collection.kind as u64) << 32) | collection.id as u64,
                            ))
                            .h(px(42.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .border_b_1()
                            .border_color(style::color(palette.colors.border_soft))
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                hub.update(cx, |hub, cx| {
                                    hub.set_setup_collection(&item, !item.enabled, cx)
                                });
                            })
                            .child(style::text(
                                match collection.kind {
                                    SetupCollectionKind::Mailbox => "MAIL",
                                    SetupCollectionKind::Calendar => "CALENDAR",
                                },
                                TextRole::SectionLabel,
                                cx,
                            ))
                            .child(div().w(px(16.0)))
                            .child(style::text(collection.name, TextRole::EventTitle, cx))
                            .child(div().flex_1())
                            .child(if collection.enabled { "On" } else { "Off" }),
                    );
                }
                content = content.child(choices).child(
                    primary_button("Start syncing", cx)
                        .on_click(cx.listener(|this, _, _, cx| this.finish_setup(cx))),
                );
            }
        }

        let error = self.error.clone().map(|error| {
            div()
                .px_3()
                .py_2()
                .rounded(px(palette.radii.button))
                .bg(style::color(palette.colors.sunken))
                .border_1()
                .border_color(style::color(palette.colors.border_strong))
                .child(style::text(error, TextRole::ReadingMeta, cx))
        });

        let card = div()
            .w(px(520.0))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_4()
            .p(px(28.0))
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border))
            .bg(style::color(palette.colors.card))
            .child(style::text(
                label.to_uppercase(),
                TextRole::SectionLabel,
                cx,
            ))
            .child(style::text(title, TextRole::ReadingSubject, cx))
            .child(style::text(body, TextRole::ReadingMeta, cx))
            .when_some(error, |this, error| this.child(error))
            .child(content);

        div()
            .id("onboarding")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .p_6()
            .bg(style::color(palette.colors.canvas))
            .child(card)
            .when(reconnecting, |this| {
                this.child(
                    div()
                        .id("back-to-mail")
                        .cursor_pointer()
                        .child(style::text("Back to mail", TextRole::ButtonLabel, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.back_to_mail(cx))),
                )
            })
    }
}
