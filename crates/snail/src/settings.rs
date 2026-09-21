//! The theme preference, persisted through the shared settings store (plan.md E1.12 / E2.7).

use gpui_kit::*;
use serde::{Deserialize, Serialize};

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
