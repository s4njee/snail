//! Versioned JSON settings, one file per section (plan.md E2.7 / §1.5). Writes are atomic
//! (temp + rename) so saving a compose draft never rewrites the theme, and a crash mid-write leaves
//! the old file intact. A bad value logs a warning and falls back to the default; it never fails
//! startup.

use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::atomic_file::atomic_write;
use crate::paths::Paths;

/// Bumped when the envelope changes shape; a mismatch falls back to defaults.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct SettingsStore {
    dir: PathBuf,
}

impl SettingsStore {
    pub fn new(paths: &Paths) -> Self {
        Self {
            dir: paths.config.clone(),
        }
    }

    pub fn file(&self, section: &str) -> PathBuf {
        self.dir.join(format!("{section}.json"))
    }

    /// Read a section, falling back to `T::default()` on a missing file, bad JSON, a version
    /// mismatch, or a value that no longer deserializes.
    pub fn load<T: DeserializeOwned + Default>(&self, section: &str) -> T {
        let path = self.file(section);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return T::default();
        };
        let Ok(envelope) = serde_json::from_str::<serde_json::Value>(&raw) else {
            log::warn!("settings/{section}: not JSON, using defaults");
            return T::default();
        };
        let version = envelope
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if version != VERSION as u64 {
            log::warn!("settings/{section}: version {version} != {VERSION}, using defaults");
            return T::default();
        }
        match envelope
            .get("data")
            .cloned()
            .and_then(|data| serde_json::from_value::<T>(data).ok())
        {
            Some(value) => value,
            None => {
                log::warn!("settings/{section}: value did not deserialize, using defaults");
                T::default()
            }
        }
    }

    /// Write a section atomically.
    pub fn save<T: Serialize>(&self, section: &str, value: &T) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let body = serde_json::json!({ "version": VERSION, "data": value }).to_string();
        let path = self.file(section);
        atomic_write(&path, body.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Section {
        #[serde(default)]
        mode: String,
    }

    fn store() -> (SettingsStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "snail-settings-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (SettingsStore { dir: dir.clone() }, dir)
    }

    #[test]
    fn round_trips() {
        let (store, dir) = store();
        store
            .save(
                "theme",
                &Section {
                    mode: "dark".into(),
                },
            )
            .unwrap();
        assert_eq!(store.load::<Section>("theme").mode, "dark");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_json_and_unknown_versions_fall_back_to_defaults() {
        let (store, dir) = store();
        std::fs::write(store.file("theme"), "{ not json").unwrap();
        assert_eq!(store.load::<Section>("theme"), Section::default());
        std::fs::write(
            store.file("theme"),
            r#"{"version": 99, "data": {"mode": "dark"}}"#,
        )
        .unwrap();
        assert_eq!(store.load::<Section>("theme"), Section::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_the_default() {
        let (store, dir) = store();
        assert_eq!(store.load::<Section>("mail"), Section::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let (store, dir) = store();
        store
            .save(
                "theme",
                &Section {
                    mode: "light".into(),
                },
            )
            .unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
