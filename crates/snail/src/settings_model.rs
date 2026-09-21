//! Background-capable model for Settings. Views never reach SQLite, the keychain, or disk paths
//! directly; every destructive or potentially blocking operation comes through this boundary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use snail_core::calendar::CalendarProvider;
use snail_core::paths::Paths;
use snail_core::store::Store;
use snail_services::auth::AccountStore;
use snail_services::secrets::KeyringStore;

use crate::mail_model::MailModel;

#[derive(Clone)]
pub struct SettingsModel {
    store: Arc<Store>,
    accounts: Arc<AccountStore>,
    paths: Paths,
    mail: MailModel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsCollection {
    pub id: i64,
    pub kind: CollectionKind,
    pub name: String,
    pub color: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    Mailbox,
    Calendar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsIdentity {
    pub id: i64,
    pub address: String,
    pub display_name: Option<String>,
    pub signature: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsAccount {
    pub id: i64,
    pub kind: String,
    pub address: String,
    pub display_name: Option<String>,
    pub last_sync: Option<i64>,
    pub collections: Vec<SettingsCollection>,
    pub identities: Vec<SettingsIdentity>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageSnapshot {
    pub database_bytes: u64,
    pub cache_bytes: u64,
    pub messages: i64,
    pub events: i64,
    pub tasks: i64,
    pub database_path: PathBuf,
    pub cache_path: PathBuf,
    pub log_path: PathBuf,
}

impl SettingsModel {
    pub fn new(store: Arc<Store>, paths: Paths, mail: MailModel) -> Self {
        Self {
            accounts: Arc::new(AccountStore::new(
                store.clone(),
                Arc::new(KeyringStore::new()),
            )),
            store,
            paths,
            mail,
        }
    }

    pub fn accounts(&self) -> Result<Vec<SettingsAccount>> {
        let mailboxes = self.store.mailboxes()?;
        self.store
            .accounts()?
            .into_iter()
            .filter(|account| account.kind != "local")
            .map(|account| {
                let mut collections = Vec::new();
                for mailbox in mailboxes
                    .iter()
                    .filter(|mailbox| mailbox.account_id == account.id)
                {
                    collections.push(SettingsCollection {
                        id: mailbox.id,
                        kind: CollectionKind::Mailbox,
                        name: mailbox.name.clone(),
                        color: None,
                        enabled: self.store.mailbox_sync_enabled(mailbox.id)?,
                    });
                }
                for calendar in self.store.calendars_for_account(account.id)? {
                    collections.push(SettingsCollection {
                        id: calendar.id,
                        kind: CollectionKind::Calendar,
                        name: calendar
                            .info
                            .name
                            .clone()
                            .unwrap_or_else(|| "Calendar".into()),
                        color: calendar.info.color.clone(),
                        enabled: calendar.visible,
                    });
                }
                let mut identity_rows = self.store.identities(account.id)?;
                if identity_rows.is_empty() {
                    self.store
                        .insert_identity(&snail_core::store::NewIdentity {
                            account_id: account.id,
                            address: account.address.clone(),
                            display_name: account.display_name.clone(),
                            signature: None,
                            is_default: true,
                        })?;
                    identity_rows = self.store.identities(account.id)?;
                }
                let identities = identity_rows
                    .into_iter()
                    .map(|identity| SettingsIdentity {
                        id: identity.id,
                        address: identity.address,
                        display_name: identity.display_name,
                        signature: identity.signature.unwrap_or_default(),
                        is_default: identity.is_default,
                    })
                    .collect();
                let last_sync = self.store.with_db(|conn| {
                    Ok(conn.query_row(
                        "SELECT max(last_sync) FROM sync_state WHERE account_id = ?1",
                        [account.id],
                        |row| row.get::<_, Option<i64>>(0),
                    )?)
                })?;
                Ok(SettingsAccount {
                    id: account.id,
                    kind: account.kind,
                    address: account.address,
                    display_name: account.display_name,
                    last_sync,
                    collections,
                    identities,
                })
            })
            .collect()
    }

    pub fn set_collection_enabled(
        &self,
        collection: &SettingsCollection,
        enabled: bool,
    ) -> Result<()> {
        match collection.kind {
            CollectionKind::Mailbox => self.store.set_mailbox_sync_enabled(collection.id, enabled),
            CollectionKind::Calendar => self.store.set_calendar_visible(collection.id, enabled),
        }
    }

    pub fn set_signature(&self, identity_id: i64, signature: &str) -> Result<()> {
        self.store.set_identity_signature(identity_id, signature)
    }

    pub fn pgp_keys(&self) -> Vec<snail_core::pgp::KeySummary> {
        self.mail.pgp_keys()
    }

    pub fn storage(&self) -> Result<StorageSnapshot> {
        let stats = self.store.stats()?;
        Ok(StorageSnapshot {
            database_bytes: std::fs::metadata(self.paths.store_db())
                .map(|metadata| metadata.len())
                .unwrap_or(0),
            cache_bytes: directory_size(&self.paths.cache).unwrap_or(0),
            messages: stats.messages,
            events: stats.events,
            tasks: stats.tasks,
            database_path: self.paths.store_db(),
            cache_path: self.paths.cache.clone(),
            log_path: self.paths.log_file(),
        })
    }

    pub fn clear_cache(&self) -> Result<()> {
        self.store.clear_cache()
    }

    pub fn rebuild_search(&self) -> Result<()> {
        self.store.rebuild_search_index()
    }

    pub fn force_full_resync(&self, account_id: i64, now: i64) -> Result<()> {
        self.store.force_full_resync(account_id, now)
    }

    pub fn remove_account(&self, account: &SettingsAccount) -> Result<()> {
        self.accounts
            .remove_account(&account.kind, &account.address, account.id)
    }

    /// iCloud uses one app-specific password for CalDAV, IMAP and SMTP. CalDAV discovery is the
    /// credential check available today; only after it succeeds do we persist the password and
    /// account. Canonical mailboxes are staged disabled/enabled choices before any mail pull.
    pub fn connect_icloud(&self, address: &str, password: &str, now: i64) -> Result<i64> {
        let address = address.trim();
        let password = password.trim();
        anyhow::ensure!(
            address.contains('@'),
            "Enter the full iCloud email address."
        );
        anyhow::ensure!(!password.is_empty(), "Enter an app-specific password.");
        let client = snail_core::providers::caldav::CalDavClient::new(address, password)?;
        let discovery = client.discover_calendars(None).map_err(|error| {
            if error
                .downcast_ref::<snail_core::providers::caldav::CalDavError>()
                .is_some_and(|error| {
                    error.kind
                        == snail_core::providers::caldav::CalDavErrorKind::CredentialRevoked
                })
            {
                anyhow::anyhow!(
                    "iCloud rejected that app-specific password. Generate a new one at account.apple.com; changing your Apple Account password revokes every existing app password."
                )
            } else {
                error
            }
        })?;
        let mailboxes = snail_services::icloud_imap::discover_mailboxes(address, password)
            .map_err(|error| {
                if error
                    .downcast_ref::<snail_services::auth::AuthError>()
                    .is_some()
                {
                    anyhow::anyhow!(
                        "iCloud rejected that app-specific password. Generate a new one at account.apple.com."
                    )
                } else {
                    error
                }
            })?;
        anyhow::ensure!(
            !mailboxes.is_empty(),
            "iCloud connected but returned no selectable mailboxes."
        );
        let account_id = self.accounts.add_account("icloud", address, None, now)?;
        self.accounts.set_icloud_app_password(address, password)?;
        self.store
            .apply_calendar_discovery(account_id, "caldav", &discovery, now)?;
        for mailbox in mailboxes {
            self.store
                .ensure_mailbox(account_id, &mailbox.name, &mailbox.kind)?;
        }
        Ok(account_id)
    }
}

fn directory_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut size = 0u64;
    for entry in std::fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
        size = size.saturating_add(directory_size(&entry?.path())?);
    }
    Ok(size)
}
