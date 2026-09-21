//! The theme preference, persisted through the shared settings store (plan.md E1.12 / E2.7).

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use snail_core::paths::Paths;
use snail_core::settings::SettingsStore;

use crate::style::{self, ThemePref};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Section {
    #[serde(default)]
    mode: ThemePref,
}

/// The settings store, remembered so a later save needs no plumbing.
struct ThemeSettings(SettingsStore);
impl Global for ThemeSettings {}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct ReaderSection {
    #[serde(default)]
    prefer_plain: bool,
    #[serde(default)]
    always_load_images_from: BTreeSet<String>,
}

/// The live preference.
pub struct ThemeState {
    pub pref: ThemePref,
}
impl Global for ThemeState {}

pub fn load(paths: &Paths) -> ThemePref {
    SettingsStore::new(paths).load::<Section>("theme").mode
}

pub fn install(store: SettingsStore, pref: ThemePref, cx: &mut App) {
    cx.set_global(ThemeSettings(store));
    cx.set_global(ThemeState { pref });
    style::apply(pref, cx);
}

pub fn pref(cx: &App) -> ThemePref {
    cx.global::<ThemeState>().pref
}

/// Change the preference: persist it, then apply it live.
pub fn set(pref: ThemePref, cx: &mut App) {
    cx.global_mut::<ThemeState>().pref = pref;
    let store = cx.global::<ThemeSettings>().0.clone();
    if let Err(error) = store.save("theme", &Section { mode: pref }) {
        log::warn!("could not save the theme preference: {error}");
    }
    style::apply(pref, cx);
}

pub fn body_preference(cx: &App) -> snail_core::mime::BodyPreference {
    if cx
        .global::<ThemeSettings>()
        .0
        .load::<ReaderSection>("reader")
        .prefer_plain
    {
        snail_core::mime::BodyPreference::Plain
    } else {
        snail_core::mime::BodyPreference::Html
    }
}

pub fn toggle_body_preference(cx: &mut App) -> snail_core::mime::BodyPreference {
    let store = cx.global::<ThemeSettings>().0.clone();
    let mut reader = store.load::<ReaderSection>("reader");
    reader.prefer_plain = !reader.prefer_plain;
    if let Err(error) = store.save("reader", &reader) {
        log::warn!("could not save the reading preference: {error}");
    }
    if reader.prefer_plain {
        snail_core::mime::BodyPreference::Plain
    } else {
        snail_core::mime::BodyPreference::Html
    }
}

/// Mail settings. `signature` is retained as a migration fallback for stores written before the
/// per-account E14 settings surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MailSection {
    #[serde(default)]
    signature: String,
    #[serde(default)]
    signatures: BTreeMap<String, String>,
    #[serde(default = "default_undo_seconds")]
    undo_send_seconds: u64,
}

impl Default for MailSection {
    fn default() -> Self {
        Self {
            signature: String::new(),
            signatures: BTreeMap::new(),
            undo_send_seconds: default_undo_seconds(),
        }
    }
}

fn default_undo_seconds() -> u64 {
    10
}

pub fn signature_for(address: Option<&str>, cx: &App) -> Option<String> {
    let mail = cx.global::<ThemeSettings>().0.load::<MailSection>("mail");
    let value = address
        .map(sender_key)
        .and_then(|address| mail.signatures.get(&address).cloned())
        .unwrap_or(mail.signature);
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub fn set_signature(address: &str, signature: String, cx: &mut App) {
    let store = cx.global::<ThemeSettings>().0.clone();
    let mut mail = store.load::<MailSection>("mail");
    mail.signatures.insert(sender_key(address), signature);
    if let Err(error) = store.save("mail", &mail) {
        log::warn!("could not save the account signature: {error}");
    }
}

pub fn undo_send_seconds(cx: &App) -> u64 {
    cx.global::<ThemeSettings>()
        .0
        .load::<MailSection>("mail")
        .undo_send_seconds
        .clamp(0, 300)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default = "default_check_minutes")]
    pub check_interval_minutes: u32,
    #[serde(default)]
    pub load_remote_images: bool,
    #[serde(default = "default_true")]
    pub group_by_thread: bool,
    #[serde(default = "default_undo_seconds")]
    pub undo_send_seconds: u64,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            check_interval_minutes: default_check_minutes(),
            load_remote_images: false,
            group_by_thread: true,
            undo_send_seconds: default_undo_seconds(),
        }
    }
}

fn default_check_minutes() -> u32 {
    5
}

fn default_true() -> bool {
    true
}

pub fn general(cx: &App) -> GeneralSettings {
    let store = &cx.global::<ThemeSettings>().0;
    let mut general = store.load::<GeneralSettings>("general");
    // Preserve an older undo-send choice until the General pane is saved once.
    let legacy = store.load::<MailSection>("mail").undo_send_seconds;
    if !store.file("general").exists() {
        general.undo_send_seconds = legacy;
    }
    general
}

pub fn set_general(value: GeneralSettings, cx: &mut App) {
    let store = cx.global::<ThemeSettings>().0.clone();
    if let Err(error) = store.save("general", &value) {
        log::warn!("could not save general settings: {error}");
    }
    let mut mail = store.load::<MailSection>("mail");
    mail.undo_send_seconds = value.undo_send_seconds;
    if let Err(error) = store.save("mail", &mail) {
        log::warn!("could not update undo-send compatibility setting: {error}");
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct AccountSyncSection {
    #[serde(default)]
    cadence_minutes: BTreeMap<String, u32>,
}

pub fn account_sync_minutes(address: &str, cx: &App) -> u32 {
    cx.global::<ThemeSettings>()
        .0
        .load::<AccountSyncSection>("account-sync")
        .cadence_minutes
        .get(&sender_key(address))
        .copied()
        .unwrap_or_else(|| general(cx).check_interval_minutes)
}

pub fn set_account_sync_minutes(address: &str, minutes: u32, cx: &mut App) {
    let store = cx.global::<ThemeSettings>().0.clone();
    let mut section = store.load::<AccountSyncSection>("account-sync");
    section.cadence_minutes.insert(sender_key(address), minutes);
    if let Err(error) = store.save("account-sync", &section) {
        log::warn!("could not save account sync cadence: {error}");
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PassphrasePolicy {
    #[default]
    Session,
    Keychain,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PgpSettings {
    #[serde(default)]
    pub passphrase_policy: PassphrasePolicy,
    #[serde(default)]
    pub sign_by_default: bool,
    #[serde(default)]
    pub encrypt_by_default: bool,
    #[serde(default)]
    pub wkd_discovery: bool,
}

impl Default for PgpSettings {
    fn default() -> Self {
        Self {
            passphrase_policy: PassphrasePolicy::Session,
            sign_by_default: false,
            encrypt_by_default: false,
            wkd_discovery: false,
        }
    }
}

pub fn pgp(cx: &App) -> PgpSettings {
    cx.global::<ThemeSettings>().0.load::<PgpSettings>("pgp")
}

pub fn set_pgp(value: PgpSettings, cx: &mut App) {
    let store = cx.global::<ThemeSettings>().0.clone();
    if let Err(error) = store.save("pgp", &value) {
        log::warn!("could not save PGP settings: {error}");
    }
}

fn sender_key(sender: &str) -> String {
    sender.trim().to_ascii_lowercase()
}

pub fn always_load_images_from(sender: &str, cx: &App) -> bool {
    let sender = sender_key(sender);
    !sender.is_empty()
        && cx
            .global::<ThemeSettings>()
            .0
            .load::<ReaderSection>("reader")
            .always_load_images_from
            .contains(&sender)
}

pub fn allow_images_from(sender: &str, cx: &mut App) {
    let sender = sender_key(sender);
    if sender.is_empty() {
        return;
    }
    let store = cx.global::<ThemeSettings>().0.clone();
    let mut reader = store.load::<ReaderSection>("reader");
    reader.always_load_images_from.insert(sender);
    if let Err(error) = store.save("reader", &reader) {
        log::warn!("could not save the remote-image allowance: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::{ReaderSection, sender_key};

    #[test]
    fn sender_keys_are_case_insensitive_and_trimmed() {
        assert_eq!(sender_key(" News@Example.COM "), "news@example.com");
    }

    #[test]
    fn reader_settings_round_trip() {
        let value = ReaderSection {
            prefer_plain: true,
            always_load_images_from: ["news@example.com".into()].into_iter().collect(),
        };
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(serde_json::from_str::<ReaderSection>(&json).unwrap(), value);
    }
}
