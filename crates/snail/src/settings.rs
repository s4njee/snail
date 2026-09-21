//! The theme preference, persisted as versioned JSON (plan.md E1.12 / §1.5). A placeholder for the
//! full settings store (E2.7): one file, atomic temp+rename, warn-and-default on a bad value.

use std::path::PathBuf;

use gpui_kit::*;
use serde::Deserialize;

use crate::style::{self, ThemePref};

/// Where the theme section lives, remembered as a global so a later save needs no plumbing.
struct ThemeFile(PathBuf);
impl Global for ThemeFile {}

/// The live preference.
pub struct ThemeState {
    pub pref: ThemePref,
}
impl Global for ThemeState {}

#[derive(Deserialize)]
struct Section {
    #[serde(default)]
    mode: ThemePref,
}

/// Read the saved preference, falling back to `System` on a missing or bad file.
pub fn load(paths: &snail_core::paths::Paths) -> ThemePref {
    let file = paths.settings_file("theme");
    std::fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str::<Section>(&raw).ok())
        .map(|section| section.mode)
        .unwrap_or_default()
}

/// Install the initial preference and remember where to persist it.
pub fn install(file: PathBuf, pref: ThemePref, cx: &mut App) {
    cx.set_global(ThemeFile(file));
    cx.set_global(ThemeState { pref });
    style::apply(pref, cx);
}

pub fn pref(cx: &App) -> ThemePref {
    cx.global::<ThemeState>().pref
}

/// Change the preference: persist it, then apply it live.
pub fn set(pref: ThemePref, cx: &mut App) {
    cx.global_mut::<ThemeState>().pref = pref;
    let file = cx.global::<ThemeFile>().0.clone();
    save(&file, pref);
    style::apply(pref, cx);
}

/// Atomic write: a crash mid-write leaves the old file intact (§1.5).
fn save(file: &PathBuf, pref: ThemePref) {
    let body = serde_json::json!({ "version": 1, "mode": pref }).to_string();
    let temp = file.with_extension("json.tmp");
    if std::fs::write(&temp, body).is_ok() {
        let _ = std::fs::rename(&temp, file);
    } else {
        log::warn!("could not write {}", file.display());
    }
}
