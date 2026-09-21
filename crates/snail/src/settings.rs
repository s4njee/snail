//! The theme preference, persisted through the shared settings store (plan.md E1.12 / E2.7).

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
