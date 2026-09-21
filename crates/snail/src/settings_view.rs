//! In-window Settings workspace (plan.md E14): a 236px navigation rail and card-and-row panes.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_ui::commands::{ShortcutPlatform, commands, display_binding};
use snail_ui::text::TextRole;

use crate::settings::{self, GeneralSettings, PassphrasePolicy, PgpSettings};
use crate::settings_model::{
    CollectionKind, SettingsAccount, SettingsCollection, SettingsModel, StorageSnapshot,
};
use crate::style::{self, ThemePref};
use crate::sync_model::{SyncHub, SyncStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Accounts,
    General,
    Signature,
    Pgp,
    Appearance,
    Keyboard,
    Advanced,
}

impl Pane {
    const ALL: [Self; 7] = [
        Self::Accounts,
        Self::General,
        Self::Signature,
        Self::Pgp,
        Self::Appearance,
        Self::Keyboard,
        Self::Advanced,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Accounts => "Accounts",
            Self::General => "General",
            Self::Signature => "Signature",
            Self::Pgp => "PGP",
            Self::Appearance => "Appearance",
            Self::Keyboard => "Keyboard",
            Self::Advanced => "Advanced",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Accounts => "Mail and calendars stay readable offline in the local store.",
            Self::General => "Shared mail and calendar behaviour.",
            Self::Signature => "A separate signature for each sending identity.",
            Self::Pgp => "Keys, passphrases and defaults for OpenPGP mail.",
            Self::Appearance => "Choose how Snail follows the desktop appearance.",
            Self::Keyboard => "Every binding comes from Snail’s command catalogue.",
            Self::Advanced => "Storage, recovery and diagnostics.",
        }
    }
}

pub enum SettingsEvent {
    AddGoogle,
    OpenPgpManager,
}

pub struct SettingsWorkspace {
    model: SettingsModel,
    sync: Entity<SyncHub>,
    pane: Pane,
    accounts: Arc<Vec<SettingsAccount>>,
    storage: StorageSnapshot,
    general: GeneralSettings,
    pgp: PgpSettings,
    selected_identity: Option<i64>,
    signature: Entity<TextareaState>,
    icloud_address: Entity<InputState>,
    icloud_password: Entity<InputState>,
    add_provider: bool,
    add_icloud: bool,
    remove_confirm: Option<i64>,
    clear_confirm: bool,
    busy: bool,
    notice: Option<String>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsWorkspace {}

impl SettingsWorkspace {
    pub fn new(
        model: SettingsModel,
        sync: Entity<SyncHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let accounts = Arc::new(model.accounts().unwrap_or_default());
        let first_identity = accounts
            .iter()
            .flat_map(|account| account.identities.iter())
            .next()
            .cloned();
        let signature = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Your signature")
                .default_value(
                    first_identity
                        .as_ref()
                        .map(|identity| identity.signature.clone())
                        .unwrap_or_default(),
                )
        });
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("name@icloud.com"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("xxxx-xxxx-xxxx-xxxx")
                .masked(true)
        });
        let subscriptions = vec![cx.subscribe(&signature, |this, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.notice = None;
                cx.notify();
            }
        })];
        Self {
            storage: model.storage().unwrap_or_default(),
            general: settings::general(cx),
            pgp: settings::pgp(cx),
            model,
            sync,
            pane: Pane::Accounts,
            accounts,
            selected_identity: first_identity.map(|identity| identity.id),
            signature,
            icloud_address: address,
            icloud_password: password,
            add_provider: false,
            add_icloud: false,
            remove_confirm: None,
            clear_confirm: false,
            busy: false,
            notice: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn reload(&mut self) {
        match self.model.accounts() {
            Ok(accounts) => self.accounts = Arc::new(accounts),
            Err(error) => self.error = Some(format!("Could not load accounts: {error:#}")),
        }
        if let Ok(storage) = self.model.storage() {
            self.storage = storage;
        }
    }

    fn save_general(&mut self, cx: &mut Context<Self>) {
        settings::set_general(self.general.clone(), cx);
        for account in self.accounts.iter() {
            let minutes = settings::account_sync_minutes(&account.address, cx);
            self.sync.update(cx, |hub, cx| {
                hub.set_account_cadence(account.id, minutes, cx)
            });
        }
        self.notice = Some("General settings saved".into());
        cx.notify();
    }

    fn save_pgp(&mut self, cx: &mut Context<Self>) {
        settings::set_pgp(self.pgp.clone(), cx);
        self.notice = Some("PGP defaults saved".into());
        cx.notify();
    }

    fn select_identity(
        &mut self,
        identity_id: i64,
        signature: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_identity = Some(identity_id);
        self.signature
            .update(cx, |state, cx| state.set_value(signature, window, cx));
        self.notice = None;
        self.error = None;
        cx.notify();
    }

    fn save_signature(&mut self, cx: &mut Context<Self>) {
        let Some(identity_id) = self.selected_identity else {
            return;
        };
        let signature = self.signature.read(cx).value().to_string();
        let address = self
            .accounts
            .iter()
            .flat_map(|account| account.identities.iter())
            .find(|identity| identity.id == identity_id)
            .map(|identity| identity.address.clone());
        match self.model.set_signature(identity_id, &signature) {
            Ok(()) => {
                if let Some(address) = address {
                    settings::set_signature(&address, signature, cx);
                }
                self.reload();
                self.notice = Some("Signature saved".into());
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Could not save signature: {error:#}")),
        }
        cx.notify();
    }

    fn toggle_collection(&mut self, collection: SettingsCollection, cx: &mut Context<Self>) {
        match self
            .model
            .set_collection_enabled(&collection, !collection.enabled)
        {
            Ok(()) => {
                self.reload();
                self.sync.update(cx, |hub, cx| hub.sync_now(cx));
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Could not update collection: {error:#}")),
        }
        cx.notify();
    }

    fn connect_icloud(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let address = self.icloud_address.read(cx).value().to_string();
        let password = self.icloud_password.read(cx).value().to_string();
        let model = self.model.clone();
        self.busy = true;
        self.error = None;
        let task = cx.background_executor().spawn(async move {
            model.connect_icloud(&address, &password, chrono::Utc::now().timestamp())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(account_id) => {
                        this.add_icloud = false;
                        this.add_provider = false;
                        this.reload();
                        this.sync
                            .update(cx, |hub, cx| hub.stage_account_setup(account_id, cx));
                        this.notice = Some(
                            "iCloud connected. Choose mailboxes and calendars, then press Sync now."
                                .into(),
                        );
                    }
                    Err(error) => this.error = Some(format!("Could not connect iCloud: {error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn remove_account(&mut self, account: SettingsAccount, cx: &mut Context<Self>) {
        if self.remove_confirm != Some(account.id) {
            self.remove_confirm = Some(account.id);
            self.notice = Some(format!(
                "Press Remove again to revoke access and delete {} locally.",
                account.address
            ));
            cx.notify();
            return;
        }
        if self.busy {
            return;
        }
        let model = self.model.clone();
        let account_id = account.id;
        self.busy = true;
        self.error = None;
        let task = cx
            .background_executor()
            .spawn(async move { model.remove_account(&account) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.remove_confirm = None;
                match result {
                    Ok(()) => {
                        this.sync
                            .update(cx, |hub, cx| hub.forget_account(account_id, cx));
                        this.reload();
                        this.notice = Some("Account removed and credentials wiped".into());
                    }
                    Err(error) => this.error = Some(format!("Could not remove account: {error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_maintenance(
        &mut self,
        label: &'static str,
        job: impl FnOnce(SettingsModel) -> anyhow::Result<()> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let model = self.model.clone();
        let task = cx.background_executor().spawn(async move { job(model) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(()) => {
                        this.reload();
                        this.notice = Some(label.into());
                    }
                    Err(error) => this.error = Some(format!("{label} failed: {error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .w(px(236.0))
            .flex_none()
            .h_full()
            .px(px(14.0))
            .py_5()
            .flex()
            .flex_col()
            .gap_1()
            .border_r_1()
            .border_color(style::color(palette.colors.border_soft))
            .bg(style::color(palette.colors.chrome))
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .child(style::text("Settings", TextRole::SectionLabel, cx)),
            )
            .children(Pane::ALL.map(|pane| {
                let selected = pane == self.pane;
                div()
                    .id(("settings-pane", pane as usize))
                    .h(px(34.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(palette.radii.button))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(style::color(palette.colors.card)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pane = pane;
                        this.notice = None;
                        this.error = None;
                        cx.notify();
                    }))
                    .child(style::text(
                        pane.label(),
                        if selected {
                            TextRole::SidebarItemSelected
                        } else {
                            TextRole::SidebarItem
                        },
                        cx,
                    ))
            }))
            .child(div().flex_1())
            .child(style::text(
                format!("Snail {}", env!("CARGO_PKG_VERSION")),
                TextRole::SidebarCount,
                cx,
            ))
            .child(style::text("GPUI · SQLite", TextRole::SidebarCount, cx))
            .into_any_element()
    }

    fn heading(&self, cx: &App) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .px(px(40.0))
            .pt(px(30.0))
            .pb(px(22.0))
            .flex_none()
            .border_b_1()
            .border_color(style::color(palette.colors.border_soft))
            .child(style::text(self.pane.label(), TextRole::CalendarTitle, cx))
            .child(div().mt_2().child(style::text(
                self.pane.description(),
                TextRole::ReadingMeta,
                cx,
            )))
            .into_any_element()
    }

    fn pane_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let content = match self.pane {
            Pane::Accounts => self.accounts_pane(cx),
            Pane::General => self.general_pane(cx),
            Pane::Signature => self.signature_pane(cx),
            Pane::Pgp => self.pgp_pane(cx),
            Pane::Appearance => self.appearance_pane(cx),
            Pane::Keyboard => self.keyboard_pane(cx),
            Pane::Advanced => self.advanced_pane(cx),
        };
        div()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .px(px(40.0))
            .py(px(26.0))
            .child(content)
            .into_any_element()
    }

    fn accounts_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let mut content = div().flex().flex_col().gap_4();
        for account in self.accounts.iter().cloned() {
            let title =
                account
                    .display_name
                    .clone()
                    .unwrap_or_else(|| match account.kind.as_str() {
                        "gmail" | "google" => "Google".into(),
                        "icloud" => "iCloud".into(),
                        _ => "Account".into(),
                    });
            let initial = if account.kind == "icloud" { "iC" } else { "G" };
            let status = self
                .sync
                .read(cx)
                .account_status(account.id)
                .cloned()
                .unwrap_or_else(|| self.sync.read(cx).status().clone());
            let (status_text, status_color) = account_status(&status, account.last_sync);
            let mut collections = div().px_4().py_3().flex().flex_wrap().gap_2();
            for collection in account.collections.clone() {
                let enabled = collection.enabled;
                let item = collection.clone();
                collections = collections.child(
                    div()
                        .id((
                            "settings-collection",
                            ((collection.kind as u64) << 32) | collection.id as u64,
                        ))
                        .h(px(30.0))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(palette.radii.button))
                        .border_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .cursor_pointer()
                        .opacity(if enabled { 1.0 } else { 0.55 })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.toggle_collection(item.clone(), cx)
                        }))
                        .child(
                            div()
                                .size(px(9.0))
                                .rounded(px(3.0))
                                .bg(collection_color(&collection, palette)),
                        )
                        .child(style::text(collection.name, TextRole::ButtonLabel, cx))
                        .child(toggle(enabled, palette)),
                );
            }
            let sync = self.sync.clone();
            let sync_account_id = account.id;
            let cadence = settings::account_sync_minutes(&account.address, cx);
            let card_account = account.clone();
            content = content.child(
                div()
                    .rounded(px(11.0))
                    .border_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .overflow_hidden()
                    .child(
                        div()
                            .px_4()
                            .py_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .bg(style::color(palette.colors.chrome))
                            .child(
                                div()
                                    .size(px(34.0))
                                    .rounded(px(9.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(style::color(palette.colors.accent_tint))
                                    .child(style::text(initial, TextRole::EventTime, cx)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(style::text(title, TextRole::EventTitle, cx))
                                    .child(style::text(
                                        format!("{} · {}", account.kind, account.address),
                                        TextRole::SidebarCount,
                                        cx,
                                    )),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().size(px(7.0)).rounded_full().bg(status_color))
                                    .child(style::text(status_text, TextRole::SidebarCount, cx)),
                            )
                            .child(button("Sync now", palette, cx).on_click(move |_, _, cx| {
                                sync.update(cx, |hub, cx| {
                                    hub.sync_account_now(sync_account_id, cx)
                                });
                            })),
                    )
                    .child(collections)
                    .child(
                        div()
                            .px_4()
                            .pb_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(style::text("Sync every", TextRole::SectionLabel, cx))
                            .children([5u32, 15, 60, 0].map(|minutes| {
                                let address = account.address.clone();
                                let sync = self.sync.clone();
                                let account_id = account.id;
                                let selected = cadence == minutes;
                                let label = match minutes {
                                    0 => "manual".into(),
                                    60 => "1 hour".into(),
                                    value => format!("{value} min"),
                                };
                                div()
                                    .id((
                                        "sync-cadence",
                                        ((account.id as u64) << 32) | minutes as u64,
                                    ))
                                    .px_3()
                                    .h(px(28.0))
                                    .flex()
                                    .items_center()
                                    .rounded(px(palette.radii.button))
                                    .cursor_pointer()
                                    .when(selected, |this| {
                                        this.bg(style::color(palette.colors.card))
                                    })
                                    .on_click(cx.listener(move |_this, _, _, cx| {
                                        settings::set_account_sync_minutes(&address, minutes, cx);
                                        sync.update(cx, |hub, cx| {
                                            hub.set_account_cadence(account_id, minutes, cx)
                                        });
                                        cx.notify();
                                    }))
                                    .child(style::text(label, TextRole::SidebarCount, cx))
                            }))
                            .child(div().flex_1())
                            .child(
                                button(
                                    if self.remove_confirm == Some(account.id) {
                                        "Confirm remove"
                                    } else {
                                        "Remove"
                                    },
                                    palette,
                                    cx,
                                )
                                .text_color(style::color(palette.colors.danger))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove_account(card_account.clone(), cx)
                                    },
                                )),
                            ),
                    ),
            );
        }
        content = content.child(
            div()
                .id("settings-add-account")
                .h(px(46.0))
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .rounded(px(11.0))
                .border_1()
                .border_dashed()
                .border_color(style::color(palette.colors.border_strong))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.add_provider = !this.add_provider;
                    this.add_icloud = false;
                    cx.notify();
                }))
                .child(style::text(
                    "+  Add Google or iCloud",
                    TextRole::ButtonLabel,
                    cx,
                )),
        );
        if self.add_provider {
            content =
                content.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(button("Google", palette, cx).on_click(
                            cx.listener(|_this, _, _, cx| cx.emit(SettingsEvent::AddGoogle)),
                        ))
                        .child(button("iCloud", palette, cx).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.add_icloud = true;
                                cx.notify();
                            },
                        ))),
                );
        }
        if self.add_icloud {
            content = content.child(self.icloud_form(cx));
        }
        content.into_any_element()
    }

    fn icloud_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border_soft))
            .child(style::text("Connect iCloud", TextRole::EventTitle, cx))
            .child(style::text(
                "At account.apple.com open Sign-In and Security → App-Specific Passwords. Two-factor authentication is required; Apple permits 25 active passwords. Changing your Apple Account password revokes every app password.",
                TextRole::ReadingMeta,
                cx,
            ))
            .child(Input::new(&self.icloud_address))
            .child(Input::new(&self.icloud_password))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .child(primary_button(if self.busy { "Connecting…" } else { "Connect iCloud" }, palette, cx).on_click(
                        cx.listener(|this, _, _, cx| this.connect_icloud(cx)),
                    )),
            )
            .into_any_element()
    }

    fn general_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let row = |title: &'static str, helper: Option<&'static str>, control: AnyElement| {
            div()
                .min_h(px(54.0))
                .px_4()
                .py_3()
                .flex()
                .items_center()
                .border_b_1()
                .border_color(style::color(palette.colors.border_soft))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .child(style::text(title, TextRole::EventTitle, cx))
                        .when_some(helper, |this, helper| {
                            this.child(style::text(helper, TextRole::ReadingMeta, cx))
                        }),
                )
                .child(div().flex_1())
                .child(control)
        };
        div()
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border_soft))
            .overflow_hidden()
            .child(row(
                "Check for new mail and events",
                None,
                cycle_button(
                    interval_label(self.general.check_interval_minutes),
                    "general-interval",
                    cx.listener(|this, _, _, cx| {
                        this.general.check_interval_minutes =
                            match this.general.check_interval_minutes {
                                5 => 15,
                                15 => 60,
                                60 => 0,
                                _ => 5,
                            };
                        this.save_general(cx);
                    }),
                    palette,
                    cx,
                )
                .into_any_element(),
            ))
            .child(row(
                "Load remote images",
                Some("Off keeps senders from knowing you opened a message"),
                clickable_toggle(
                    "general-remote-images",
                    self.general.load_remote_images,
                    cx.listener(|this, _, _, cx| {
                        this.general.load_remote_images = !this.general.load_remote_images;
                        this.save_general(cx);
                    }),
                    palette,
                )
                .into_any_element(),
            ))
            .child(row(
                "Group messages by thread",
                None,
                clickable_toggle(
                    "general-threading",
                    self.general.group_by_thread,
                    cx.listener(|this, _, _, cx| {
                        this.general.group_by_thread = !this.general.group_by_thread;
                        this.save_general(cx);
                    }),
                    palette,
                )
                .into_any_element(),
            ))
            .child(
                div()
                    .min_h(px(54.0))
                    .px_4()
                    .py_3()
                    .flex()
                    .items_center()
                    .child(style::text("Undo send window", TextRole::EventTitle, cx))
                    .child(div().flex_1())
                    .child(cycle_button(
                        format!("{} seconds", self.general.undo_send_seconds),
                        "general-undo",
                        cx.listener(|this, _, _, cx| {
                            this.general.undo_send_seconds = match this.general.undo_send_seconds {
                                0 => 5,
                                5 => 10,
                                10 => 30,
                                _ => 0,
                            };
                            this.save_general(cx);
                        }),
                        palette,
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn signature_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let mut identities = div().flex().flex_wrap().gap_2();
        for identity in self
            .accounts
            .iter()
            .flat_map(|account| account.identities.iter())
            .cloned()
        {
            let selected = self.selected_identity == Some(identity.id);
            let signature = identity.signature.clone();
            identities = identities.child(
                div()
                    .id(("signature-identity", identity.id as usize))
                    .h(px(32.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(palette.radii.button))
                    .border_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .cursor_pointer()
                    .when(selected, |this| {
                        this.bg(style::color(palette.colors.accent_tint))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_identity(identity.id, signature.clone(), window, cx)
                    }))
                    .child(style::text(identity.address, TextRole::ButtonLabel, cx)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(identities)
            .child(
                div()
                    .h(px(180.0))
                    .rounded(px(palette.radii.card))
                    .border_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .child(
                        Textarea::new(&self.signature)
                            .size_full()
                            .font_family("Newsreader")
                            .text_size(px(15.0))
                            .line_height(px(25.5)),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    primary_button("Save signature", palette, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.save_signature(cx))),
                ),
            )
            .into_any_element()
    }

    fn pgp_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let keys = self.model.pgp_keys();
        let mut content = div().flex().flex_col().gap_4();
        let mut key_card = div()
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border_soft));
        if keys.is_empty() {
            key_card = key_card.child(div().p_4().child(style::text(
                "No OpenPGP keys imported",
                TextRole::ReadingMeta,
                cx,
            )));
        } else {
            for key in keys {
                key_card = key_card.child(
                    div()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(style::color(palette.colors.border_soft))
                        .child(style::text(
                            key.user_ids
                                .first()
                                .cloned()
                                .unwrap_or_else(|| "Unnamed key".into()),
                            TextRole::EventTitle,
                            cx,
                        ))
                        .child(style::text(key.fingerprint, TextRole::SidebarCount, cx)),
                );
            }
        }
        content =
            content
                .child(key_card)
                .child(button("Manage keys…", palette, cx).on_click(
                    cx.listener(|_this, _, _, cx| cx.emit(SettingsEvent::OpenPgpManager)),
                ))
                .child(settings_row(
                    "Passphrase policy",
                    cycle_button(
                        match self.pgp.passphrase_policy {
                            PassphrasePolicy::Session => "Session only",
                            PassphrasePolicy::Keychain => "Remember in keychain",
                        },
                        "pgp-passphrase-policy",
                        cx.listener(|this, _, _, cx| {
                            this.pgp.passphrase_policy = match this.pgp.passphrase_policy {
                                PassphrasePolicy::Session => PassphrasePolicy::Keychain,
                                PassphrasePolicy::Keychain => PassphrasePolicy::Session,
                            };
                            this.save_pgp(cx);
                        }),
                        palette,
                        cx,
                    )
                    .into_any_element(),
                    palette,
                    cx,
                ))
                .child(settings_row(
                    "Sign messages by default",
                    clickable_toggle(
                        "pgp-default-sign",
                        self.pgp.sign_by_default,
                        cx.listener(|this, _, _, cx| {
                            this.pgp.sign_by_default = !this.pgp.sign_by_default;
                            this.save_pgp(cx);
                        }),
                        palette,
                    )
                    .into_any_element(),
                    palette,
                    cx,
                ))
                .child(settings_row(
                    "Encrypt when every recipient has a key",
                    clickable_toggle(
                        "pgp-default-encrypt",
                        self.pgp.encrypt_by_default,
                        cx.listener(|this, _, _, cx| {
                            this.pgp.encrypt_by_default = !this.pgp.encrypt_by_default;
                            this.save_pgp(cx);
                        }),
                        palette,
                    )
                    .into_any_element(),
                    palette,
                    cx,
                ))
                .child(settings_row(
                    "Discover keys through WKD",
                    clickable_toggle(
                        "pgp-wkd",
                        self.pgp.wkd_discovery,
                        cx.listener(|this, _, _, cx| {
                            this.pgp.wkd_discovery = !this.pgp.wkd_discovery;
                            this.save_pgp(cx);
                        }),
                        palette,
                    )
                    .into_any_element(),
                    palette,
                    cx,
                ));
        content.into_any_element()
    }

    fn appearance_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        div()
            .flex()
            .gap_3()
            .children(
                [
                    (ThemePref::System, "System"),
                    (ThemePref::Light, "Light"),
                    (ThemePref::Dark, "Dark"),
                ]
                .map(|(preference, label)| {
                    let selected = settings::pref(cx) == preference;
                    div()
                        .id(("appearance", preference as usize))
                        .w(px(150.0))
                        .h(px(92.0))
                        .p_4()
                        .rounded(px(palette.radii.card))
                        .border_1()
                        .border_color(style::color(if selected {
                            palette.colors.accent
                        } else {
                            palette.colors.border_soft
                        }))
                        .cursor_pointer()
                        .on_click(cx.listener(move |_this, _, _, cx| {
                            settings::set(preference, cx);
                            cx.notify();
                        }))
                        .child(style::text(label, TextRole::EventTitle, cx))
                        .child(style::text(
                            if selected { "Selected" } else { "Choose" },
                            TextRole::ReadingMeta,
                            cx,
                        ))
                }),
            )
            .into_any_element()
    }

    fn keyboard_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let mut table = div()
            .rounded(px(palette.radii.card))
            .border_1()
            .border_color(style::color(palette.colors.border_soft))
            .overflow_hidden();
        for command in commands() {
            table = table.child(
                div()
                    .h(px(42.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(style::color(palette.colors.border_soft))
                    .child(style::text(command.title, TextRole::EventTitle, cx))
                    .child(div().flex_1())
                    .child(style::text(
                        command.context.label(),
                        TextRole::SidebarCount,
                        cx,
                    ))
                    .child(
                        div()
                            .ml_4()
                            .px_2()
                            .py_1()
                            .rounded(px(5.0))
                            .bg(style::color(palette.colors.sunken))
                            .child(style::text(
                                display_binding(
                                    command.default_binding,
                                    if cfg!(target_os = "macos") {
                                        ShortcutPlatform::Mac
                                    } else {
                                        ShortcutPlatform::Other
                                    },
                                ),
                                TextRole::EventTime,
                                cx,
                            )),
                    ),
            );
        }
        table.into_any_element()
    }

    fn advanced_pane(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = style::palette(cx);
        let mut content = div().flex().flex_col().gap_4().child(
            div()
                .p_4()
                .rounded(px(palette.radii.card))
                .border_1()
                .border_color(style::color(palette.colors.border_soft))
                .child(style::text(
                    format!(
                        "{} database · {} cache · {} messages · {} events · {} tasks",
                        bytes_label(self.storage.database_bytes),
                        bytes_label(self.storage.cache_bytes),
                        self.storage.messages,
                        self.storage.events,
                        self.storage.tasks,
                    ),
                    TextRole::EventTitle,
                    cx,
                ))
                .child(style::text(
                    self.storage.database_path.display().to_string(),
                    TextRole::SidebarCount,
                    cx,
                ))
                .child(style::text(
                    self.storage.cache_path.display().to_string(),
                    TextRole::SidebarCount,
                    cx,
                )),
        );
        content = content
            .child(advanced_action(
                "Clear cache",
                "Message bodies and images will be fetched again when needed.",
                if self.clear_confirm {
                    "Confirm clear"
                } else {
                    "Clear cache"
                },
                cx.listener(|this, _, _, cx| {
                    if !this.clear_confirm {
                        this.clear_confirm = true;
                        this.notice = Some("Press Confirm clear to remove the local cache.".into());
                        cx.notify();
                    } else {
                        this.clear_confirm = false;
                        this.run_maintenance("Cache cleared", |model| model.clear_cache(), cx);
                    }
                }),
                palette,
                cx,
            ))
            .child(advanced_action(
                "Re-index search",
                "Rebuild the full-text index from locally stored message metadata.",
                "Re-index",
                cx.listener(|this, _, _, cx| {
                    this.run_maintenance("Search index rebuilt", |model| model.rebuild_search(), cx)
                }),
                palette,
                cx,
            ));
        for account in self.accounts.iter().cloned() {
            let id = account.id;
            content = content.child(advanced_action(
                "Force full resync",
                &format!(
                    "Discard cursors and rebuild {} from its providers.",
                    account.address
                ),
                "Force resync",
                cx.listener(move |this, _, _, cx| {
                    this.run_maintenance(
                        "Full resync queued",
                        move |model| model.force_full_resync(id, chrono::Utc::now().timestamp()),
                        cx,
                    );
                    this.sync.update(cx, |hub, cx| hub.sync_now(cx));
                }),
                palette,
                cx,
            ));
        }
        let log = self.storage.log_path.clone();
        content
            .child(advanced_action(
                "Open log",
                "Show Snail’s rotating diagnostic log in the default app.",
                "Open log",
                move |_, _, cx| cx.open_with_system(&log),
                palette,
                cx,
            ))
            .into_any_element()
    }
}

impl Render for SettingsWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = style::palette(cx);
        let notice = self.notice.clone();
        let error = self.error.clone();
        div()
            .size_full()
            .flex()
            .bg(style::color(palette.colors.card))
            .child(self.nav(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.heading(cx))
                    .child(self.pane_body(cx))
                    .when_some(notice, |this, notice| {
                        this.child(
                            div()
                                .absolute()
                                .bottom(px(18.0))
                                .right(px(24.0))
                                .px_4()
                                .py_2()
                                .rounded(px(palette.radii.button))
                                .bg(style::color(palette.colors.ink))
                                .text_color(style::color(palette.colors.canvas))
                                .child(notice),
                        )
                    })
                    .when_some(error, |this, error| {
                        this.child(
                            div()
                                .absolute()
                                .bottom(px(18.0))
                                .right(px(24.0))
                                .max_w(px(520.0))
                                .px_4()
                                .py_2()
                                .rounded(px(palette.radii.button))
                                .bg(style::color(palette.colors.danger))
                                .text_color(rgb(0xffffff))
                                .child(error),
                        )
                    }),
            )
    }
}

fn button(label: &'static str, palette: &snail_ui::theme::Theme, cx: &App) -> Stateful<Div> {
    div()
        .id(label)
        .h(px(30.0))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(palette.radii.button))
        .border_1()
        .border_color(style::color(palette.colors.border_soft))
        .bg(style::color(palette.colors.card))
        .cursor_pointer()
        .child(style::text(label, TextRole::ButtonLabel, cx))
}

fn primary_button(
    label: &'static str,
    palette: &snail_ui::theme::Theme,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(label)
        .h(px(34.0))
        .px_4()
        .flex()
        .items_center()
        .rounded(px(palette.radii.button))
        .bg(style::color(palette.colors.accent))
        .cursor_pointer()
        .child(style::text(label, TextRole::ButtonLabelFilled, cx))
}

fn toggle(on: bool, palette: &snail_ui::theme::Theme) -> Div {
    div()
        .w(px(28.0))
        .h(px(17.0))
        .p(px(2.0))
        .flex()
        .when(on, |this| this.justify_end())
        .rounded(px(9.0))
        .bg(style::color(if on {
            palette.colors.accent
        } else {
            palette.colors.border_strong
        }))
        .child(
            div()
                .size(px(13.0))
                .rounded_full()
                .bg(style::color(palette.colors.card)),
        )
}

fn clickable_toggle(
    id: &'static str,
    on: bool,
    listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    palette: &snail_ui::theme::Theme,
) -> Stateful<Div> {
    toggle(on, palette)
        .id(id)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, listener)
}

fn cycle_button(
    label: impl Into<SharedString>,
    id: &'static str,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: &snail_ui::theme::Theme,
    cx: &App,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(30.0))
        .px_3()
        .flex()
        .items_center()
        .rounded(px(palette.radii.button))
        .border_1()
        .border_color(style::color(palette.colors.border_soft))
        .cursor_pointer()
        .on_click(listener)
        .child(style::text(label, TextRole::ButtonLabel, cx))
}

fn settings_row(
    title: &'static str,
    control: AnyElement,
    palette: &snail_ui::theme::Theme,
    cx: &App,
) -> AnyElement {
    div()
        .min_h(px(52.0))
        .px_4()
        .flex()
        .items_center()
        .rounded(px(palette.radii.card))
        .border_1()
        .border_color(style::color(palette.colors.border_soft))
        .child(style::text(title, TextRole::EventTitle, cx))
        .child(div().flex_1())
        .child(control)
        .into_any_element()
}

fn advanced_action(
    title: &'static str,
    helper: &str,
    action: &'static str,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    palette: &snail_ui::theme::Theme,
    cx: &App,
) -> AnyElement {
    div()
        .min_h(px(62.0))
        .px_4()
        .flex()
        .items_center()
        .rounded(px(palette.radii.card))
        .border_1()
        .border_color(style::color(palette.colors.border_soft))
        .child(
            div()
                .flex()
                .flex_col()
                .child(style::text(title, TextRole::EventTitle, cx))
                .child(style::text(helper.to_string(), TextRole::ReadingMeta, cx)),
        )
        .child(div().flex_1())
        .child(button(action, palette, cx).on_click(listener))
        .into_any_element()
}

fn account_status(status: &SyncStatus, last_sync: Option<i64>) -> (String, Hsla) {
    match status {
        SyncStatus::Syncing { .. } => ("syncing".into(), rgb(0xa16207).into()),
        SyncStatus::Offline(_) => ("offline".into(), rgb(0x8a837b).into()),
        SyncStatus::NeedsSignIn { .. } | SyncStatus::NoClient => {
            ("needs attention".into(), rgb(0xc2410c).into())
        }
        _ => {
            let text = last_sync
                .map(|at| format!("synced {}", relative_time(at)))
                .unwrap_or_else(|| "ready".into());
            (text, rgb(0x0f766e).into())
        }
    }
}

fn relative_time(timestamp: i64) -> String {
    let elapsed = chrono::Utc::now().timestamp().saturating_sub(timestamp);
    if elapsed < 60 {
        "just now".into()
    } else if elapsed < 3600 {
        format!("{} min ago", elapsed / 60)
    } else {
        format!("{} hr ago", elapsed / 3600)
    }
}

fn collection_color(collection: &SettingsCollection, palette: &snail_ui::theme::Theme) -> Hsla {
    if collection.kind == CollectionKind::Mailbox {
        return style::color(palette.colors.muted);
    }
    collection
        .color
        .as_deref()
        .and_then(hex_color)
        .unwrap_or_else(|| style::color(palette.colors.accent))
}

fn hex_color(value: &str) -> Option<Hsla> {
    let value = value.trim().trim_start_matches('#');
    (value.len() >= 6)
        .then(|| u32::from_str_radix(&value[..6], 16).ok())
        .flatten()
        .map(|value| rgb(value).into())
}

fn interval_label(minutes: u32) -> String {
    match minutes {
        0 => "Manual".into(),
        60 => "Every hour".into(),
        value => format!("Every {value} minutes"),
    }
}

fn bytes_label(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}
