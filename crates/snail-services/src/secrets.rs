//! Secrets in the OS keychain (plan.md E2.8 / §1.5). OAuth refresh tokens, iCloud app-specific
//! passwords and the PGP passphrase go here, never into SQLite or JSON.
//!
//! One trait so tests use an in-memory backend, and the "no keyring daemon" Linux case produces a
//! clear error rather than a crash or a silent plaintext fallback.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Result, anyhow};

/// The bundle id, used as the keychain service name.
pub const SERVICE: &str = "dev.snail.app";

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// The real store, over the OS keychain.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self {
            service: SERVICE.to_string(),
        }
    }

    pub fn with_service(service: &str) -> Self {
        Self {
            service: service.to_string(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(|error| explain(error, key))
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(explain(error, key)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.entry(key)?
            .set_password(value)
            .map_err(|error| explain(error, key))
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(explain(error, key)),
        }
    }
}

/// A test double, and the shape E3's account flows use in tests.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

/// Turn a keyring error into something a user can act on (E3.6, E3.8).
fn explain(error: keyring::Error, key: &str) -> anyhow::Error {
    match error {
        keyring::Error::PlatformFailure(inner) => anyhow!(
            "no OS keychain is available ({inner}); on Linux install a Secret Service such as \
             gnome-keyring or KWallet, or choose the encrypted-file fallback (E3.8)"
        ),
        other => anyhow!("keychain error for {key}: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_store_round_trips_and_deletes() {
        let store = MemoryStore::new();
        assert_eq!(store.get("gmail:refresh").unwrap(), None);
        store.set("gmail:refresh", "secret-token").unwrap();
        assert_eq!(
            store.get("gmail:refresh").unwrap().as_deref(),
            Some("secret-token")
        );
        store.delete("gmail:refresh").unwrap();
        assert_eq!(store.get("gmail:refresh").unwrap(), None);
    }

    #[test]
    fn keys_are_independent() {
        let store = MemoryStore::new();
        store.set("a", "1").unwrap();
        store.set("b", "2").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("1"));
        assert_eq!(store.get("b").unwrap().as_deref(), Some("2"));
    }
}
