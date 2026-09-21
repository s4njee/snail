//! The local store (plan.md E2): one `rusqlite` connection behind a `Mutex`, WAL, touched only from
//! the background executor. Every read is served from here, so offline is the normal case.

pub mod schema;

#[cfg(test)]
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::types::Value;
use rusqlite::{Connection, params};

use crate::cache::CacheStore;
use crate::paths::Paths;

/// Where a message came from, without either provider's shape leaking upward (E2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderRef {
    Gmail {
        id: String,
        thread_id: Option<String>,
        history_id: Option<i64>,
    },
    Imap {
        uid: u32,
        uidvalidity: u32,
        modseq: Option<i64>,
    },
}

/// Everything needed to insert a message row.
#[derive(Clone, Debug, Default)]
pub struct NewMessage {
    pub account_id: i64,
    pub mailbox_id: Option<i64>,
    pub thread_id: Option<i64>,
    pub provider: Option<ProviderRef>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub to_json: Option<String>,
    pub date: Option<i64>,
    pub preview: Option<String>,
    pub unread: bool,
    pub has_attachments: bool,
    pub body_hash: Option<String>,
    pub raw_hash: Option<String>,
    pub labels_json: Option<String>,
    /// The plain-text body, for the FTS index (E9.1). Derived from the cached raw message.
    pub body_text: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MessageRow {
    pub id: i64,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub date: Option<i64>,
    pub preview: Option<String>,
    pub unread: bool,
    pub body_hash: Option<String>,
}

/// A queued local mutation (E2.5). `idempotency_key` makes enqueueing safe to repeat.
#[derive(Clone, Debug)]
pub struct NewOp {
    pub account_id: i64,
    pub target_kind: String,
    pub target_id: Option<i64>,
    pub operation: String,
    pub payload_json: Option<String>,
    pub idempotency_key: String,
    pub now: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OpRow {
    pub id: i64,
    pub account_id: i64,
    pub target_kind: String,
    pub target_id: Option<i64>,
    pub operation: String,
    pub payload_json: Option<String>,
    pub idempotency_key: String,
    pub attempts: i64,
    pub state: String,
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpCounts {
    pub pending: i64,
    pub failed: i64,
    pub dead: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub messages: i64,
    pub events: i64,
    pub tasks: i64,
}

impl OpCounts {
    /// What the sync footer shows ("N changes pending").
    pub fn outstanding(&self) -> i64 {
        self.pending + self.failed
    }
}

/// A per (account, kind, collection) cursor, plus the first-class full-resync flag (E2.6).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncState {
    pub account_id: i64,
    pub kind: String,
    pub collection: String,
    pub cursor_json: Option<String>,
    pub history_id: Option<i64>,
    pub sync_token: Option<String>,
    pub uidvalidity: Option<i64>,
    pub highest_modseq: Option<i64>,
    pub ctag: Option<String>,
    pub full_resync_needed: bool,
    pub last_sync: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountRow {
    pub id: i64,
    pub kind: String,
    pub address: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalendarRow {
    pub id: i64,
    pub account_id: i64,
    pub provider: String,
    pub info: crate::calendar::CalendarInfo,
    pub visible: bool,
    pub event_count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRow {
    pub id: i64,
    pub calendar_id: i64,
    pub event: crate::calendar::CalendarEvent,
    pub server_ical: Option<String>,
    pub conflict_json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReminderKind {
    MinutesBefore(i64),
    SameDayMinute(u16),
    Absolute(i64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderRow {
    pub id: i64,
    pub event_id: i64,
    pub kind: ReminderKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DueReminder {
    pub reminder_id: i64,
    pub event_id: i64,
    pub occurrence_start: i64,
    pub due_utc: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttendeeRow {
    pub id: i64,
    pub event_id: i64,
    pub email: String,
    pub display_name: Option<String>,
    pub role: String,
    pub status: String,
    pub is_self: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRow {
    pub id: i64,
    pub account_id: i64,
    pub title: String,
    pub notes: Option<String>,
    pub due_utc: Option<i64>,
    pub done: bool,
    pub created_at: i64,
}

/// Send-as identities hang off an account (E3.4c), not the other way around.
#[derive(Clone, Debug, PartialEq)]
pub struct NewIdentity {
    pub account_id: i64,
    pub address: String,
    pub display_name: Option<String>,
    pub signature: Option<String>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IdentityRow {
    pub id: i64,
    pub account_id: i64,
    pub address: String,
    pub display_name: Option<String>,
    pub signature: Option<String>,
    pub is_default: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailboxRow {
    pub id: i64,
    pub account_id: i64,
    pub name: String,
    pub kind: String,
    pub unread: i64,
    pub total: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactRow {
    pub address: String,
    pub name: Option<String>,
    /// `2 * sent + received`, so people you write to rank above bulk senders (E7.3).
    pub score: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRow {
    pub thread_id: i64,
    /// The newest message in the thread, used as the selection target.
    pub newest_id: i64,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub last_date: Option<i64>,
    pub count: i64,
    pub unread: i64,
}

/// The criteria a search runs against (E9.1/E9.2). The `snail-ui` parser produces the same shape;
/// the bin maps one to the other, since neither crate depends on the other.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub terms: Vec<String>,
    pub phrases: Vec<String>,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub subject: Vec<String>,
    pub mailbox: Option<String>,
    pub unread: Option<bool>,
    pub has_attachment: bool,
    pub after: Option<i64>,
    pub before: Option<i64>,
}

/// One search result row, with its mailbox name so the list can group by mailbox (E9.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub id: i64,
    pub mailbox_id: Option<i64>,
    pub mailbox_name: Option<String>,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub date: Option<i64>,
    pub preview: Option<String>,
    pub unread: bool,
}

pub struct Store {
    conn: Mutex<Connection>,
    cache: CacheStore,
}

impl Store {
    /// Open (creating if needed) the store at the configured location.
    pub fn open(paths: &Paths) -> Result<Self> {
        paths.ensure()?;
        let mut conn = Connection::open(paths.store_db())
            .with_context(|| format!("open {}", paths.store_db().display()))?;
        configure(&conn)?;
        schema::migrate(&mut conn, None)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cache: CacheStore::new(paths.cache_files()),
        })
    }

    /// An in-memory store for tests.
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        schema::migrate(&mut conn, None)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cache: CacheStore::new(std::env::temp_dir().join("snail-cache-in-memory")),
        })
    }

    pub fn cache(&self) -> &CacheStore {
        &self.cache
    }

    /// The only way to touch SQLite. Runs on the caller's thread — the background executor.
    pub fn with_db<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        f(&conn)
    }

    /// A transaction, for an optimistic change and its queued op in one commit (E2.5).
    pub fn transaction<T>(&self, f: impl FnOnce(&rusqlite::Transaction) -> Result<T>) -> Result<T> {
        let mut conn = self.conn.lock().expect("store mutex poisoned");
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.with_db(schema::version)
    }

    pub fn insert_account(
        &self,
        kind: &str,
        address: &str,
        display_name: Option<&str>,
        now: i64,
    ) -> Result<i64> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO account (kind, address, display_name, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![kind, address, display_name, now],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn insert_message(&self, message: &NewMessage) -> Result<i64> {
        let (provider, gmail_id, gmail_thread_id, gmail_history_id, imap_uid, uidvalidity, modseq) =
            match &message.provider {
                Some(ProviderRef::Gmail {
                    id,
                    thread_id,
                    history_id,
                }) => (
                    "gmail",
                    Some(id.clone()),
                    thread_id.clone(),
                    *history_id,
                    None,
                    None,
                    None,
                ),
                Some(ProviderRef::Imap {
                    uid,
                    uidvalidity,
                    modseq,
                }) => (
                    "imap",
                    None,
                    None,
                    None,
                    Some(*uid as i64),
                    Some(*uidvalidity as i64),
                    *modseq,
                ),
                None => ("unknown", None, None, None, None, None, None),
            };
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO message (
                    account_id, mailbox_id, thread_id, provider,
                    gmail_id, gmail_thread_id, gmail_history_id,
                    imap_uid, imap_uidvalidity, imap_modseq,
                    message_id, in_reply_to, references_header,
                    subject, from_name, from_addr, to_json, date,
                    preview, unread, has_attachments, body_hash, raw_hash, labels_json, body_text
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25
                 )",
                params![
                    message.account_id,
                    message.mailbox_id,
                    message.thread_id,
                    provider,
                    gmail_id,
                    gmail_thread_id,
                    gmail_history_id,
                    imap_uid,
                    uidvalidity,
                    modseq,
                    message.message_id,
                    message.in_reply_to,
                    message.references,
                    message.subject,
                    message.from_name,
                    message.from_addr,
                    message.to_json,
                    message.date,
                    message.preview,
                    message.unread as i64,
                    message.has_attachments as i64,
                    message.body_hash,
                    message.raw_hash,
                    message.labels_json,
                    message.body_text,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Create a mailbox if it is not there and return its id (used by backfill/sync).
    pub fn ensure_mailbox(&self, account_id: i64, name: &str, kind: &str) -> Result<i64> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO mailbox (account_id, name, kind) VALUES (?1, ?2, ?3)",
                params![account_id, name, kind],
            )?;
            Ok(conn.query_row(
                "SELECT id FROM mailbox WHERE account_id = ?1 AND name = ?2",
                params![account_id, name],
                |row| row.get(0),
            )?)
        })
    }

    /// The thread a message belongs to, created on first sight (Gmail's `threadId` is authoritative).
    pub fn ensure_thread(
        &self,
        account_id: i64,
        provider_thread_id: Option<&str>,
        subject: Option<&str>,
    ) -> Result<i64> {
        self.with_db(|conn| {
            if let Some(provider_id) = provider_thread_id {
                use rusqlite::OptionalExtension as _;
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM thread WHERE account_id = ?1 AND provider_thread_id = ?2",
                        params![account_id, provider_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(id) = existing {
                    return Ok(id);
                }
                conn.execute(
                    "INSERT INTO thread (account_id, provider_thread_id, subject) VALUES (?1, ?2, ?3)",
                    params![account_id, provider_id, subject],
                )?;
                return Ok(conn.last_insert_rowid());
            }
            conn.execute(
                "INSERT INTO thread (account_id, subject) VALUES (?1, ?2)",
                params![account_id, subject],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn mailbox_id(&self, account_id: i64, name: &str) -> Result<Option<i64>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT id FROM mailbox WHERE account_id = ?1 AND name = ?2",
                    params![account_id, name],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }

    /// Every mailbox, for the sidebar (E5.4).
    pub fn mailboxes(&self) -> Result<Vec<MailboxRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, account_id, name, kind, unread, total FROM mailbox
                 ORDER BY account_id, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(MailboxRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    unread: row.get(4)?,
                    total: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Mailboxes enabled in Settings. Local/import accounts have no remote pull and remain
    /// visible regardless of the sync flag.
    pub fn enabled_mailboxes(&self) -> Result<Vec<MailboxRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT m.id, m.account_id, m.name, m.kind, m.unread, m.total
                 FROM mailbox m
                 JOIN account a ON a.id = m.account_id
                 WHERE m.sync_enabled = 1 OR a.kind = 'local'
                 ORDER BY m.account_id, m.id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(MailboxRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    unread: row.get(4)?,
                    total: row.get(5)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Provider mailbox kinds selected for an account's next pull.
    pub fn enabled_mailbox_kinds(&self, account_id: i64) -> Result<Vec<String>> {
        Ok(self
            .enabled_mailbox_filters(account_id)?
            .into_iter()
            .map(|(_, kind)| kind)
            .collect())
    }

    pub fn enabled_mailbox_filters(&self, account_id: i64) -> Result<Vec<(String, String)>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT name, kind FROM mailbox
                 WHERE account_id = ?1 AND sync_enabled = 1
                 ORDER BY id",
            )?;
            let rows = statement.query_map([account_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn mailbox_sync_enabled(&self, mailbox_id: i64) -> Result<bool> {
        self.with_db(|conn| {
            Ok(conn.query_row(
                "SELECT sync_enabled FROM mailbox WHERE id = ?1",
                [mailbox_id],
                |row| row.get::<_, i64>(0),
            )? != 0)
        })
    }

    pub fn set_mailbox_sync_enabled(&self, mailbox_id: i64, enabled: bool) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE mailbox SET sync_enabled = ?2 WHERE id = ?1",
                params![mailbox_id, enabled as i64],
            )?;
            Ok(())
        })
    }

    /// The original RFC822 bytes for a message, from the content-addressed cache. `None` means the
    /// cache file is gone and the caller re-fetches (E2.3).
    pub fn raw_bytes(&self, message_id: i64) -> Result<Option<Vec<u8>>> {
        let hash: Option<String> = self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT raw_hash FROM message WHERE id = ?1",
                    [message_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten())
        })?;
        match hash {
            Some(hash) => self.cache.get(&hash),
            None => Ok(None),
        }
    }

    /// Apply a triage action optimistically and queue the remote op in the **same transaction**
    /// (E8.4). Returns the op id for a later undo (E8.5).
    pub fn apply_triage(
        &self,
        message_id: i64,
        action: &crate::triage::TriageAction,
        key: &str,
        now: i64,
    ) -> Result<i64> {
        use crate::triage::{Mailbox, TriageAction, TriagePayload};
        self.transaction(|tx| {
            let (account_id, mailbox_name, unread, flagged): (i64, Option<String>, i64, i64) = tx
                .query_row(
                "SELECT m.account_id, b.name, m.unread, m.flagged
                     FROM message m LEFT JOIN mailbox b ON b.id = m.mailbox_id WHERE m.id = ?1",
                [message_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            let mailbox_before = mailbox_of(tx, message_id)?;
            let payload = TriagePayload {
                action: action.clone(),
                previous_mailbox: mailbox_name,
                previous_unread: unread != 0,
                previous_flagged: flagged != 0,
            };

            let target = match action {
                TriageAction::Archive => Some(Mailbox::Archive.name().to_string()),
                TriageAction::Trash => Some(Mailbox::Trash.name().to_string()),
                TriageAction::Move { mailbox } => Some(mailbox.clone()),
                _ => None,
            };
            if let Some(name) = &target {
                tx.execute(
                    "INSERT OR IGNORE INTO mailbox (account_id, name, kind) VALUES (?1, ?2, ?3)",
                    params![account_id, name, kind_for_name(name)],
                )?;
                let mailbox_id: i64 = tx.query_row(
                    "SELECT id FROM mailbox WHERE account_id = ?1 AND name = ?2",
                    params![account_id, name],
                    |row| row.get(0),
                )?;
                tx.execute(
                    "UPDATE message SET mailbox_id = ?2 WHERE id = ?1",
                    params![message_id, mailbox_id],
                )?;
            }
            match action {
                TriageAction::MarkRead(read) => {
                    tx.execute(
                        "UPDATE message SET unread = ?2 WHERE id = ?1",
                        params![message_id, if *read { 0 } else { 1 }],
                    )?;
                }
                TriageAction::MarkFlagged(flagged) => {
                    tx.execute(
                        "UPDATE message SET starred = ?2 WHERE id = ?1",
                        params![message_id, *flagged as i64],
                    )?;
                }
                _ => {}
            }

            recount_touched(tx, message_id, mailbox_before)?;
            tx.execute(
                "INSERT OR IGNORE INTO pending_op (
                    account_id, target_kind, target_id, operation, payload_json,
                    idempotency_key, attempts, state, created_at, updated_at
                 ) VALUES (?1, 'message', ?2, ?3, ?4, ?5, 0, 'pending', ?6, ?6)",
                params![
                    account_id,
                    message_id,
                    action.operation(),
                    payload.to_json(),
                    key,
                    now
                ],
            )?;
            Ok(tx.query_row(
                "SELECT id FROM pending_op WHERE idempotency_key = ?1",
                [key],
                |row| row.get(0),
            )?)
        })
    }

    /// Undo a triage op: restore the row, and either cancel the queued op (still pending) or queue
    /// its inverse (already dispatched). Returns true when an inverse was queued (E8.5).
    pub fn undo_triage(&self, op_id: i64, now: i64) -> Result<bool> {
        use crate::triage::{TriageAction, TriagePayload};
        self.transaction(|tx| {
            let (state, payload_json, target_id): (String, Option<String>, Option<i64>) = tx
                .query_row(
                    "SELECT state, payload_json, target_id FROM pending_op WHERE id = ?1",
                    [op_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            let Some(payload) = payload_json.as_deref().and_then(TriagePayload::from_json) else {
                return Ok(false);
            };
            let Some(message_id) = target_id else {
                return Ok(false);
            };
            let account_id = restore_triage_row(tx, message_id, &payload)?;

            let inverse = payload.action.toggle_inverse().or_else(|| {
                payload
                    .previous_mailbox
                    .clone()
                    .map(|mailbox| TriageAction::Move { mailbox })
            });
            if state == "pending" {
                // Still local: cancel it and we are done.
                tx.execute(
                    "UPDATE pending_op SET state = 'cancelled', updated_at = ?2 WHERE id = ?1",
                    params![op_id, now],
                )?;
                return Ok(false);
            }
            if let Some(inverse) = inverse {
                let inverse_payload = TriagePayload {
                    action: inverse.clone(),
                    previous_mailbox: None,
                    previous_unread: !payload.previous_unread,
                    previous_flagged: !payload.previous_flagged,
                };
                let key = format!("undo:{op_id}");
                tx.execute(
                    "INSERT OR IGNORE INTO pending_op (
                        account_id, target_kind, target_id, operation, payload_json,
                        idempotency_key, attempts, state, created_at, updated_at
                     ) VALUES (?1, 'message', ?2, ?3, ?4, ?5, 0, 'pending', ?6, ?6)",
                    params![
                        account_id,
                        message_id,
                        inverse.operation(),
                        inverse_payload.to_json(),
                        key,
                        now
                    ],
                )?;
                return Ok(true);
            }
            Ok(false)
        })
    }

    /// Roll back triage ops the provider rejected (failed or dead-lettered): put each row back
    /// where it was and mark the op `rolled_back` so it is neither retried nor double-applied.
    /// Returns how many were rolled back, for the visible "couldn't apply" notice (E8.4).
    pub fn rollback_failed_triage(&self, now: i64) -> Result<usize> {
        use crate::triage::TriagePayload;
        self.transaction(|tx| {
            let rows = {
                let mut statement = tx.prepare(
                    "SELECT id, payload_json, target_id FROM pending_op
                     WHERE state IN ('failed', 'dead')
                       AND operation IN ('archive', 'trash', 'mark_read', 'mark_flagged', 'move')",
                )?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            let mut rolled_back = 0;
            for (id, payload_json, target_id) in rows {
                let Some(payload) = payload_json.as_deref().and_then(TriagePayload::from_json)
                else {
                    continue;
                };
                let Some(message_id) = target_id else {
                    continue;
                };
                restore_triage_row(tx, message_id, &payload)?;
                tx.execute(
                    "UPDATE pending_op SET state = 'rolled_back', updated_at = ?2 WHERE id = ?1",
                    params![id, now],
                )?;
                rolled_back += 1;
            }
            Ok(rolled_back)
        })
    }

    /// Full-text plus filtered search (E9.1/E9.2/E9.5), newest first within each mailbox so the
    /// list can group by mailbox with headers (E9.4). Local only: this never touches a provider.
    pub fn search(&self, query: &SearchQuery, limit: u32) -> Result<Vec<SearchHit>> {
        self.with_db(|conn| {
            let mut sql = String::from(
                "SELECT m.id, m.mailbox_id, b.name, m.subject, m.from_name, m.from_addr, m.date,
                        m.preview, m.unread
                 FROM message m LEFT JOIN mailbox b ON b.id = m.mailbox_id
                 WHERE 1 = 1",
            );
            let mut values: Vec<Value> = Vec::new();

            if let Some(expression) = fts_match(query) {
                sql.push_str(
                    " AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?1)",
                );
                values.push(Value::Text(expression));
            }
            for value in &query.from {
                sql.push_str(
                    " AND (lower(COALESCE(m.from_name, '')) LIKE ? ESCAPE '\\'
                          OR lower(COALESCE(m.from_addr, '')) LIKE ? ESCAPE '\\')",
                );
                let pattern = like_pattern(&value.to_lowercase());
                values.push(Value::Text(pattern.clone()));
                values.push(Value::Text(pattern));
            }
            for value in &query.to {
                sql.push_str(
                    " AND (lower(COALESCE(m.to_json, '')) LIKE ? ESCAPE '\\'
                          OR lower(COALESCE(m.cc_json, '')) LIKE ? ESCAPE '\\')",
                );
                let pattern = like_pattern(&value.to_lowercase());
                values.push(Value::Text(pattern.clone()));
                values.push(Value::Text(pattern));
            }
            for value in &query.subject {
                sql.push_str(" AND lower(COALESCE(m.subject, '')) LIKE ? ESCAPE '\\'");
                values.push(Value::Text(like_pattern(&value.to_lowercase())));
            }
            if let Some(mailbox) = &query.mailbox {
                sql.push_str(" AND lower(COALESCE(b.name, '')) = lower(?)");
                values.push(Value::Text(mailbox.clone()));
            }
            match query.unread {
                Some(true) => sql.push_str(" AND m.unread = 1"),
                Some(false) => sql.push_str(" AND m.unread = 0"),
                None => {}
            }
            if query.has_attachment {
                sql.push_str(" AND m.has_attachments = 1");
            }
            if let Some(after) = query.after {
                sql.push_str(" AND m.date >= ?");
                values.push(Value::Integer(after));
            }
            if let Some(before) = query.before {
                sql.push_str(" AND m.date < ?");
                values.push(Value::Integer(before));
            }
            sql.push_str(
                " ORDER BY lower(COALESCE(b.name, '')) ASC, m.date DESC, m.id DESC LIMIT ?",
            );
            values.push(Value::Integer(limit as i64));

            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok(SearchHit {
                    id: row.get(0)?,
                    mailbox_id: row.get(1)?,
                    mailbox_name: row.get(2)?,
                    subject: row.get(3)?,
                    from_name: row.get(4)?,
                    from_addr: row.get(5)?,
                    date: row.get(6)?,
                    preview: row.get(7)?,
                    unread: row.get::<_, i64>(8)? != 0,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// One row per thread in a mailbox, newest first — the "group by thread" list shape (E8.3).
    pub fn thread_page(&self, mailbox_id: i64, limit: u32) -> Result<Vec<ThreadRow>> {
        // Group by the real thread where there is one, and by the message itself where there is
        // not (a Sent copy, an import): coalescing to `-id` keeps those visible as singletons
        // instead of dropping them from a join. The newest row supplies the summary fields.
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "WITH ranked AS (
                     SELECT id, COALESCE(thread_id, -id) AS grp, subject, from_name, from_addr,
                            date, unread,
                            COUNT(*) OVER (PARTITION BY COALESCE(thread_id, -id)) AS cnt,
                            SUM(unread) OVER (PARTITION BY COALESCE(thread_id, -id)) AS unread_sum,
                            MAX(date) OVER (PARTITION BY COALESCE(thread_id, -id)) AS last_date,
                            ROW_NUMBER() OVER (
                                PARTITION BY COALESCE(thread_id, -id) ORDER BY date DESC, id DESC
                            ) AS rank
                     FROM message WHERE mailbox_id = ?1
                 )
                 SELECT grp, id, subject, from_name, from_addr, last_date, cnt, unread_sum
                 FROM ranked WHERE rank = 1
                 ORDER BY last_date DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(params![mailbox_id, limit], |row| {
                Ok(ThreadRow {
                    thread_id: row.get(0)?,
                    newest_id: row.get(1)?,
                    subject: row.get(2)?,
                    from_name: row.get(3)?,
                    from_addr: row.get(4)?,
                    last_date: row.get(5)?,
                    count: row.get(6)?,
                    unread: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn thread_id_of(&self, message_id: i64) -> Result<Option<i64>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT thread_id FROM message WHERE id = ?1",
                    [message_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten())
        })
    }

    /// Every message in a thread, oldest first — the thread view (E8.2) and thread actions (E8.8).
    pub fn messages_in_thread(&self, thread_id: i64) -> Result<Vec<MessageRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, subject, from_name, from_addr, date, preview, unread, body_hash
                 FROM message WHERE thread_id = ?1 ORDER BY date ASC",
            )?;
            let rows = statement.query_map([thread_id], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    from_name: row.get(2)?,
                    from_addr: row.get(3)?,
                    date: row.get(4)?,
                    preview: row.get(5)?,
                    unread: row.get::<_, i64>(6)? != 0,
                    body_hash: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Insert or update a local draft (E7.6). Drafts are messages in the Drafts mailbox with
    /// provider `local`, so they show up in the mailbox like anything else. Returns the message id
    /// to reuse on the next autosave.
    pub fn save_draft(
        &self,
        draft_id: Option<i64>,
        account_id: i64,
        subject: &str,
        body_text: &str,
        to_json: &str,
        raw_hash: Option<&str>,
        now: i64,
    ) -> Result<i64> {
        let preview = crate::mime::collapse(body_text, 120);
        self.with_db(|conn| {
            if let Some(id) = draft_id {
                conn.execute(
                    "UPDATE message SET subject = ?2, preview = ?3, to_json = ?4, raw_hash = ?5,
                        date = ?6 WHERE id = ?1",
                    params![id, subject, preview, to_json, raw_hash, now],
                )?;
                return Ok(id);
            }
            conn.execute(
                "INSERT OR IGNORE INTO mailbox (account_id, name, kind) VALUES (?1, 'Drafts', 'drafts')",
                params![account_id],
            )?;
            let mailbox: i64 = conn.query_row(
                "SELECT id FROM mailbox WHERE account_id = ?1 AND name = 'Drafts'",
                params![account_id],
                |row| row.get(0),
            )?;
            conn.execute(
                "INSERT INTO message (
                    account_id, mailbox_id, provider, gmail_id, subject, preview, to_json,
                    raw_hash, date, unread
                 ) VALUES (?1, ?2, 'local', ?3, ?4, ?5, ?6, ?7, ?8, 0)",
                params![
                    account_id,
                    mailbox,
                    format!("draft-{now}"),
                    subject,
                    preview,
                    to_json,
                    raw_hash,
                    now
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Delete a message (a discarded draft, or a permanently expunged one).
    pub fn delete_message(&self, message_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute("DELETE FROM message WHERE id = ?1", [message_id])?;
            Ok(())
        })
    }

    /// The most recent draft for an account, for "resume the last draft" affordances.
    pub fn latest_draft(&self, account_id: i64) -> Result<Option<MessageRow>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT id, subject, from_name, from_addr, date, preview, unread, body_hash
                     FROM message WHERE account_id = ?1 AND provider = 'local'
                     ORDER BY date DESC LIMIT 1",
                    params![account_id],
                    |row| {
                        Ok(MessageRow {
                            id: row.get(0)?,
                            subject: row.get(1)?,
                            from_name: row.get(2)?,
                            from_addr: row.get(3)?,
                            date: row.get(4)?,
                            preview: row.get(5)?,
                            unread: row.get::<_, i64>(6)? != 0,
                            body_hash: row.get(7)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// A single message row by id, for the reading pane header.
    pub fn message(&self, message_id: i64) -> Result<Option<MessageRow>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT id, subject, from_name, from_addr, date, preview, unread, body_hash
                     FROM message WHERE id = ?1",
                    [message_id],
                    |row| {
                        Ok(MessageRow {
                            id: row.get(0)?,
                            subject: row.get(1)?,
                            from_name: row.get(2)?,
                            from_addr: row.get(3)?,
                            date: row.get(4)?,
                            preview: row.get(5)?,
                            unread: row.get::<_, i64>(6)? != 0,
                            body_hash: row.get(7)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Mark a message read/unread, optimistically (E5.9). The caller queues the `pending_op`.
    pub fn set_unread(&self, message_id: i64, unread: bool) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE message SET unread = ?2 WHERE id = ?1",
                params![message_id, unread as i64],
            )?;
            Ok(())
        })
    }

    /// Harvest From/To addresses from the messages already synced (E7.3). Idempotent: it rebuilds
    /// the counts from scratch, so it is safe to re-run.
    pub fn harvest_contacts(&self, account_id: i64) -> Result<usize> {
        self.with_db(|conn| {
            conn.execute(
                "DELETE FROM contact WHERE account_id = ?1",
                params![account_id],
            )?;
            let mut statement = conn.prepare(
                "SELECT from_name, from_addr, to_json, date FROM message WHERE account_id = ?1",
            )?;
            let rows = statement.query_map(params![account_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?;
            let mut contacts =
                std::collections::BTreeMap::<String, (Option<String>, i64, i64, i64)>::new();
            for row in rows {
                let (name, from, to_json, date) = row?;
                if let Some(address) = from.filter(|address| !address.is_empty()) {
                    let entry = contacts.entry(address).or_default();
                    entry.0 = entry.0.take().or(name);
                    entry.2 += 1;
                    entry.3 = entry.3.max(date.unwrap_or(0));
                }
                if let Some(to_json) = to_json {
                    if let Ok(recipients) =
                        serde_json::from_str::<Vec<crate::mime::Recipient>>(&to_json)
                    {
                        for recipient in recipients {
                            if recipient.address.is_empty() {
                                continue;
                            }
                            let entry = contacts.entry(recipient.address).or_default();
                            entry.0 = entry.0.take().or(recipient.name);
                            entry.1 += 1;
                            entry.3 = entry.3.max(date.unwrap_or(0));
                        }
                    }
                }
            }
            let mut count = 0;
            for (address, (name, sent, received, last_seen)) in contacts {
                conn.execute(
                    "INSERT INTO contact (account_id, address, name, sent, received, last_seen)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![account_id, address, name, sent, received, last_seen],
                )?;
                count += 1;
            }
            Ok(count)
        })
    }

    /// Address autocomplete: prefix matches on address or name, best score first (E7.3).
    pub fn suggest_contacts(
        &self,
        account_id: i64,
        prefix: &str,
        limit: u32,
    ) -> Result<Vec<ContactRow>> {
        let pattern = format!("{}%", prefix.trim().to_lowercase());
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT address, name, (sent * 2 + received) AS score
                 FROM contact
                 WHERE account_id = ?1
                   AND (lower(address) LIKE ?2 OR lower(COALESCE(name, '')) LIKE ?2)
                 ORDER BY score DESC, last_seen DESC LIMIT ?3",
            )?;
            let rows = statement.query_map(params![account_id, pattern, limit], |row| {
                Ok(ContactRow {
                    address: row.get(0)?,
                    name: row.get(1)?,
                    score: row.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Insert only if this provider id is new; used by backfill so a re-run converges (E4.23).
    pub fn insert_message_if_new(&self, message: &NewMessage) -> Result<bool> {
        match &message.provider {
            Some(ProviderRef::Gmail { id, .. })
                if self.gmail_message_exists(message.account_id, id)? =>
            {
                return Ok(false);
            }
            Some(ProviderRef::Imap {
                uid, uidvalidity, ..
            }) if self.imap_message_exists(message.account_id, *uid, *uidvalidity)? => {
                return Ok(false);
            }
            _ => {}
        }
        self.insert_message(message)?;
        Ok(true)
    }

    pub fn gmail_message_exists(&self, account_id: i64, gmail_id: &str) -> Result<bool> {
        self.with_db(|conn| {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM message WHERE account_id = ?1 AND provider = 'gmail' AND gmail_id = ?2",
                params![account_id, gmail_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    pub fn imap_message_exists(&self, account_id: i64, uid: u32, uidvalidity: u32) -> Result<bool> {
        self.with_db(|conn| {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM message
                 WHERE account_id = ?1 AND provider = 'imap' AND imap_uid = ?2 AND imap_uidvalidity = ?3",
                params![account_id, uid as i64, uidvalidity as i64],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    /// Mailbox page, newest first.
    pub fn page(&self, mailbox_id: i64, limit: u32, offset: u32) -> Result<Vec<MessageRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, subject, from_name, from_addr, date, preview, unread, body_hash
                 FROM message WHERE mailbox_id = ?1
                 ORDER BY date DESC LIMIT ?2 OFFSET ?3",
            )?;
            let rows = statement.query_map(params![mailbox_id, limit, offset], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    from_name: row.get(2)?,
                    from_addr: row.get(3)?,
                    date: row.get(4)?,
                    preview: row.get(5)?,
                    unread: row.get::<_, i64>(6)? != 0,
                    body_hash: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The unread count is a stored counter, not a `count(*)` — the sidebar reads this on every
    /// paint, and E5.9 decrements it optimistically when a message is read. 138 ms vs microseconds.
    pub fn unread_count(&self, mailbox_id: i64) -> Result<i64> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            let unread: Option<i64> = conn
                .query_row(
                    "SELECT unread FROM mailbox WHERE id = ?1",
                    [mailbox_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(unread.unwrap_or(0))
        })
    }

    /// Recompute a mailbox's `total`/`unread` counters from the message table. The recovery path
    /// after a bulk change, a fixture rebuild, or a full resync (E4.24).
    pub fn recount_mailbox(&self, mailbox_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE mailbox SET
                    total = (SELECT count(*) FROM message WHERE mailbox_id = ?1),
                    unread = (SELECT count(*) FROM message WHERE mailbox_id = ?1 AND unread = 1)
                 WHERE id = ?1",
                [mailbox_id],
            )?;
            Ok(())
        })
    }

    /// Recount every mailbox of one account after a sync batch (E5.9 reads the counters).
    pub fn recount_account(&self, account_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE mailbox SET
                    total = (SELECT count(*) FROM message WHERE message.mailbox_id = mailbox.id),
                    unread = (SELECT count(*) FROM message
                              WHERE message.mailbox_id = mailbox.id AND message.unread = 1)
                 WHERE account_id = ?1",
                [account_id],
            )?;
            Ok(())
        })
    }

    /// A Gmail message's local id and stored `labelIds`, for applying history records (E4.7).
    pub fn gmail_message_labels(
        &self,
        account_id: i64,
        gmail_id: &str,
    ) -> Result<Option<(i64, Vec<String>)>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            let row: Option<(i64, Option<String>)> = conn
                .query_row(
                    "SELECT id, labels_json FROM message
                     WHERE account_id = ?1 AND provider = 'gmail' AND gmail_id = ?2",
                    params![account_id, gmail_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(row.map(|(id, json)| {
                let labels = json
                    .as_deref()
                    .and_then(|json| serde_json::from_str(json).ok())
                    .unwrap_or_default();
                (id, labels)
            }))
        })
    }

    /// The Gmail id behind a local message, if it has one (E8.4 pushes triage by it).
    pub fn gmail_id_of(&self, message_id: i64) -> Result<Option<String>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT gmail_id FROM message WHERE id = ?1 AND provider = 'gmail'",
                    [message_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
    }

    /// Store a message's full `labelIds` and everything derived from them: its mailbox, unread and
    /// starred (E4.7/E8.6). The mailbox is resolved by the caller from the labels.
    pub fn set_gmail_labels(
        &self,
        message_id: i64,
        labels: &[String],
        mailbox_id: i64,
    ) -> Result<()> {
        let has = |name: &str| labels.iter().any(|label| label == name);
        self.with_db(|conn| {
            conn.execute(
                "UPDATE message SET labels_json = ?2, mailbox_id = ?3, unread = ?4, starred = ?5
                 WHERE id = ?1",
                params![
                    message_id,
                    serde_json::to_string(labels)?,
                    mailbox_id,
                    has("UNREAD") as i64,
                    has("STARRED") as i64,
                ],
            )?;
            Ok(())
        })
    }

    /// Remove a message Gmail deleted permanently (`messagesDeleted`, E4.7). True when a row went.
    pub fn delete_gmail_message(&self, account_id: i64, gmail_id: &str) -> Result<bool> {
        self.with_db(|conn| {
            Ok(conn.execute(
                "DELETE FROM message WHERE account_id = ?1 AND provider = 'gmail' AND gmail_id = ?2",
                params![account_id, gmail_id],
            )? > 0)
        })
    }

    /// Drop the local stand-in for a message we sent (E7.11 files one under a synthetic `sent-…`
    /// id) once the real copy arrives from the provider, so Sent does not show it twice.
    pub fn delete_sent_placeholder(
        &self,
        account_id: i64,
        message_id_header: &str,
    ) -> Result<usize> {
        self.with_db(|conn| {
            Ok(conn.execute(
                "DELETE FROM message
                 WHERE account_id = ?1 AND provider = 'gmail' AND gmail_id LIKE 'sent-%'
                   AND message_id = ?2",
                params![account_id, message_id_header],
            )?)
        })
    }

    /// Whether a local change to this message is still waiting to reach the provider. Incoming
    /// label changes skip such a message, or a stale server state would undo the user's action.
    pub fn has_outstanding_ops(&self, message_id: i64) -> Result<bool> {
        self.with_db(|conn| {
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM pending_op
                 WHERE target_id = ?1 AND target_kind = 'message' AND state IN ('pending', 'failed')",
                [message_id],
                |row| row.get(0),
            )?;
            Ok(count > 0)
        })
    }

    /// Re-derive every stored preview under the current rules (`mime::parse_raw`: the HTML's
    /// sanitized text, preheader padding stripped). Earlier ones could begin with the sender's CSS
    /// or JSON-LD — `div.preheader { display: none }…`. Runs once per store, recorded in `meta`;
    /// returns how many rows changed.
    pub fn repair_html_previews(&self) -> Result<usize> {
        const KEY: &str = "repair:previews-v2";
        let done: Option<String> = self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [KEY], |row| {
                    row.get(0)
                })
                .optional()?)
        })?;
        if done.is_some() {
            return Ok(0);
        }
        let suspects: Vec<(i64, String)> = self.with_db(|conn| {
            let mut statement =
                conn.prepare("SELECT id, raw_hash FROM message WHERE raw_hash IS NOT NULL")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut repaired = 0;
        for (id, hash) in suspects {
            let Some(raw) = self.cache().get(&hash)? else {
                continue;
            };
            let Ok(parsed) = crate::mime::parse_raw(&raw) else {
                continue;
            };
            let body_text = parsed.plain.as_deref().map(crate::mime::search_text);
            let changed = self.with_db(|conn| {
                Ok(conn.execute(
                    "UPDATE message SET preview = ?2, body_text = coalesce(?3, body_text)
                     WHERE id = ?1 AND (preview IS NOT ?2 OR body_text IS NOT coalesce(?3, body_text))",
                    params![id, parsed.preview, body_text],
                )?)
            })?;
            repaired += changed;
        }
        self.with_db(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, '1')",
                [KEY],
            )?;
            Ok(())
        })?;
        Ok(repaired)
    }

    /// The mailbox a message is filed in.
    pub fn message_mailbox(&self, message_id: i64) -> Result<Option<i64>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT mailbox_id FROM message WHERE id = ?1",
                    [message_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten())
        })
    }

    /// The newest message id, the watermark new-mail notifications count from.
    pub fn max_message_id(&self) -> Result<i64> {
        self.with_db(|conn| {
            Ok(
                conn.query_row("SELECT coalesce(max(id), 0) FROM message", [], |row| {
                    row.get(0)
                })?,
            )
        })
    }

    /// Unread Inbox messages stored after `after_id` and dated at or after `since`: what arrived in
    /// the last sync and is worth a notification (E16.8). Oldest first.
    pub fn new_unread_inbox(
        &self,
        after_id: i64,
        since: i64,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT m.id, m.subject, m.from_name, m.from_addr, m.date, m.preview, m.unread,
                        m.body_hash
                 FROM message m JOIN mailbox b ON b.id = m.mailbox_id
                 WHERE m.id > ?1 AND b.kind = 'inbox' AND m.unread = 1 AND coalesce(m.date, 0) >= ?2
                 ORDER BY m.id LIMIT ?3",
            )?;
            let rows = statement.query_map(params![after_id, since, limit], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    subject: row.get(1)?,
                    from_name: row.get(2)?,
                    from_addr: row.get(3)?,
                    date: row.get(4)?,
                    preview: row.get(5)?,
                    unread: row.get::<_, i64>(6)? != 0,
                    body_hash: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// When the oldest op still waiting to reach a provider was queued, so the sync loop can push
    /// it as soon as its undo window closes rather than at the next poll.
    pub fn oldest_outstanding_op_at(&self) -> Result<Option<i64>> {
        self.with_db(|conn| {
            Ok(conn.query_row(
                "SELECT min(created_at) FROM pending_op WHERE state IN ('pending', 'failed')",
                [],
                |row| row.get(0),
            )?)
        })
    }

    /// One account's queued ops created at or before `created_before`: the ones whose undo window
    /// has closed and so may be pushed to the provider (E8.5).
    pub fn settled_ops(
        &self,
        account_id: i64,
        created_before: i64,
        limit: u32,
    ) -> Result<Vec<OpRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, account_id, target_kind, target_id, operation, payload_json,
                        idempotency_key, attempts, state, last_error
                 FROM pending_op
                 WHERE account_id = ?1 AND state IN ('pending', 'failed') AND created_at <= ?2
                 ORDER BY created_at, id LIMIT ?3",
            )?;
            let rows = statement.query_map(params![account_id, created_before, limit], |row| {
                Ok(OpRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_id: row.get(3)?,
                    operation: row.get(4)?,
                    payload_json: row.get(5)?,
                    idempotency_key: row.get(6)?,
                    attempts: row.get(7)?,
                    state: row.get(8)?,
                    last_error: row.get(9)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Queue an operation. Repeating the same idempotency key is a no-op (E2.5).
    pub fn enqueue_op(&self, op: &NewOp) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO pending_op (
                    account_id, target_kind, target_id, operation, payload_json,
                    idempotency_key, attempts, state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 'pending', ?7, ?7)",
                params![
                    op.account_id,
                    op.target_kind,
                    op.target_id,
                    op.operation,
                    op.payload_json,
                    op.idempotency_key,
                    op.now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn pending_ops(&self, limit: u32) -> Result<Vec<OpRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, account_id, target_kind, target_id, operation, payload_json,
                        idempotency_key, attempts, state, last_error
                 FROM pending_op WHERE state IN ('pending', 'failed')
                 ORDER BY created_at LIMIT ?1",
            )?;
            let rows = statement.query_map([limit], |row| {
                Ok(OpRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_id: row.get(3)?,
                    operation: row.get(4)?,
                    payload_json: row.get(5)?,
                    idempotency_key: row.get(6)?,
                    attempts: row.get(7)?,
                    state: row.get(8)?,
                    last_error: row.get(9)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn op_counts(&self) -> Result<OpCounts> {
        self.with_db(|conn| {
            let count = |state: &str| -> rusqlite::Result<i64> {
                conn.query_row(
                    "SELECT count(*) FROM pending_op WHERE state = ?1",
                    [state],
                    |row| row.get(0),
                )
            };
            Ok(OpCounts {
                pending: count("pending")?,
                failed: count("failed")?,
                dead: count("dead")?,
            })
        })
    }

    /// Move failed and dead-lettered operations back to `pending` (E7.12's explicit retry).
    pub fn requeue_failed_ops(&self) -> Result<usize> {
        self.with_db(|conn| {
            Ok(conn.execute(
                "UPDATE pending_op SET state = 'pending', attempts = 0, last_error = NULL
                 WHERE state IN ('failed', 'dead')",
                [],
            )?)
        })
    }

    pub fn set_op_state(&self, id: i64, state: &str, error: Option<&str>, now: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE pending_op
                 SET state = ?2, last_error = ?3, attempts = attempts + 1, updated_at = ?4
                 WHERE id = ?1",
                params![id, state, error, now],
            )?;
            Ok(())
        })
    }

    pub fn set_sync_state(&self, state: &SyncState) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO sync_state (
                    account_id, kind, collection, cursor_json, history_id, sync_token,
                    uidvalidity, highest_modseq, ctag, full_resync_needed, last_sync
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(account_id, kind, collection) DO UPDATE SET
                    cursor_json = excluded.cursor_json,
                    history_id = excluded.history_id,
                    sync_token = excluded.sync_token,
                    uidvalidity = excluded.uidvalidity,
                    highest_modseq = excluded.highest_modseq,
                    ctag = excluded.ctag,
                    full_resync_needed = excluded.full_resync_needed,
                    last_sync = excluded.last_sync",
                params![
                    state.account_id,
                    state.kind,
                    state.collection,
                    state.cursor_json,
                    state.history_id,
                    state.sync_token,
                    state.uidvalidity,
                    state.highest_modseq,
                    state.ctag,
                    state.full_resync_needed as i64,
                    state.last_sync,
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_sync_state(
        &self,
        account_id: i64,
        kind: &str,
        collection: &str,
    ) -> Result<Option<SyncState>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT cursor_json, history_id, sync_token, uidvalidity, highest_modseq, ctag,
                        full_resync_needed, last_sync
                 FROM sync_state WHERE account_id = ?1 AND kind = ?2 AND collection = ?3",
            )?;
            let mut rows = statement.query(params![account_id, kind, collection])?;
            match rows.next()? {
                Some(row) => Ok(Some(SyncState {
                    account_id,
                    kind: kind.to_string(),
                    collection: collection.to_string(),
                    cursor_json: row.get(0)?,
                    history_id: row.get(1)?,
                    sync_token: row.get(2)?,
                    uidvalidity: row.get(3)?,
                    highest_modseq: row.get(4)?,
                    ctag: row.get(5)?,
                    full_resync_needed: row.get::<_, i64>(6)? != 0,
                    last_sync: row.get(7)?,
                })),
                None => Ok(None),
            }
        })
    }
    pub fn accounts(&self) -> Result<Vec<AccountRow>> {
        self.with_db(|conn| {
            let mut statement =
                conn.prepare("SELECT id, kind, address, display_name FROM account ORDER BY id")?;
            let rows = statement.query_map([], |row| {
                Ok(AccountRow {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    address: row.get(2)?,
                    display_name: row.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn account_by_address(&self, kind: &str, address: &str) -> Result<Option<AccountRow>> {
        self.with_db(|conn| {
            use rusqlite::OptionalExtension as _;
            let row = conn
                .query_row(
                    "SELECT id, kind, address, display_name FROM account
                     WHERE kind = ?1 AND address = ?2",
                    params![kind, address],
                    |row| {
                        Ok(AccountRow {
                            id: row.get(0)?,
                            kind: row.get(1)?,
                            address: row.get(2)?,
                            display_name: row.get(3)?,
                        })
                    },
                )
                .optional()?;
            Ok(row)
        })
    }

    pub fn insert_identity(&self, identity: &NewIdentity) -> Result<i64> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO identity (account_id, address, display_name, signature, is_default)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    identity.account_id,
                    identity.address,
                    identity.display_name,
                    identity.signature,
                    identity.is_default as i64,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn identities(&self, account_id: i64) -> Result<Vec<IdentityRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, account_id, address, display_name, signature, is_default
                 FROM identity WHERE account_id = ?1 ORDER BY is_default DESC, id",
            )?;
            let rows = statement.query_map([account_id], |row| {
                Ok(IdentityRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    address: row.get(2)?,
                    display_name: row.get(3)?,
                    signature: row.get(4)?,
                    is_default: row.get::<_, i64>(5)? != 0,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn set_identity_signature(&self, identity_id: i64, signature: &str) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE identity SET signature = ?2 WHERE id = ?1",
                params![identity_id, signature],
            )?;
            Ok(())
        })
    }

    /// Merge calendar-list discovery without deleting events when a subscription disappears.
    pub fn apply_calendar_discovery(
        &self,
        account_id: i64,
        provider: &str,
        batch: &crate::calendar::CalendarBatch,
        now: i64,
    ) -> Result<()> {
        self.transaction(|tx| {
            for calendar in &batch.calendars {
                tx.execute(
                    "INSERT INTO calendar (
                        account_id, provider, remote_id, name, color, selected, read_only,
                        href, access_role, subscribed, ctag, supports_sync_collection,
                        supports_scheduling
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, ?10, ?11, ?12)
                     ON CONFLICT(account_id, provider, remote_id) DO UPDATE SET
                        name = excluded.name, color = excluded.color, selected = excluded.selected,
                        read_only = excluded.read_only, href = excluded.href,
                        access_role = excluded.access_role, subscribed = 1, ctag = excluded.ctag,
                        supports_sync_collection = excluded.supports_sync_collection,
                        supports_scheduling = excluded.supports_scheduling",
                    params![
                        account_id,
                        provider,
                        calendar.remote_id,
                        calendar.name,
                        calendar.color,
                        calendar.selected as i64,
                        calendar.read_only as i64,
                        calendar.href,
                        calendar.access_role,
                        calendar.ctag,
                        calendar.supports_sync_collection as i64,
                        calendar.supports_scheduling as i64,
                    ],
                )?;
            }
            for remote_id in &batch.unsubscribed {
                tx.execute(
                    "UPDATE calendar SET subscribed = 0
                     WHERE account_id = ?1 AND provider = ?2 AND remote_id = ?3",
                    params![account_id, provider, remote_id],
                )?;
            }
            tx.execute(
                "INSERT INTO sync_state (account_id, kind, collection, sync_token, last_sync)
                 VALUES (?1, ?2, '', ?3, ?4)
                 ON CONFLICT(account_id, kind, collection) DO UPDATE SET
                    sync_token = excluded.sync_token, last_sync = excluded.last_sync,
                    full_resync_needed = 0",
                params![
                    account_id,
                    format!("calendar-list:{provider}"),
                    batch.next_sync_token,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn calendars_for_account(&self, account_id: i64) -> Result<Vec<CalendarRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, provider, remote_id, href, name, color, selected, read_only,
                        access_role, subscribed, ctag, supports_sync_collection,
                        supports_scheduling, visible,
                        (SELECT count(*) FROM event WHERE event.calendar_id = calendar.id)
                 FROM calendar WHERE account_id = ?1 ORDER BY id",
            )?;
            let rows = statement.query_map([account_id], |row| {
                Ok(CalendarRow {
                    id: row.get(0)?,
                    account_id,
                    provider: row.get(1)?,
                    info: crate::calendar::CalendarInfo {
                        remote_id: row.get(2)?,
                        href: row.get(3)?,
                        name: row.get(4)?,
                        color: row.get(5)?,
                        selected: row.get::<_, i64>(6)? != 0,
                        read_only: row.get::<_, i64>(7)? != 0,
                        access_role: row.get(8)?,
                        subscribed: row.get::<_, i64>(9)? != 0,
                        ctag: row.get(10)?,
                        supports_sync_collection: row.get::<_, i64>(11)? != 0,
                        supports_scheduling: row.get::<_, i64>(12)? != 0,
                    },
                    visible: row.get::<_, i64>(13)? != 0,
                    event_count: row.get(14)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Snail's visibility toggle is local presentation state, independent of Google's `selected`
    /// subscription metadata. Views re-query immediately after this update (E12.11).
    pub fn set_calendar_visible(&self, calendar_id: i64, visible: bool) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE calendar SET visible = ?2 WHERE id = ?1",
                params![calendar_id, visible as i64],
            )?;
            Ok(())
        })
    }

    /// Event masters and one-off events needed to expand a buffered visible range. Recurring
    /// masters are deliberately included even when their DTSTART lies before the requested range;
    /// recurrence expansion and timezone projection happen off the UI thread.
    pub fn calendar_events_for_range(
        &self,
        start_utc: i64,
        end_utc: i64,
        visible_only: bool,
    ) -> Result<Vec<EventRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT e.id, e.calendar_id, e.remote_id, e.href, e.etag, e.ical_uid,
                        e.recurring_event_id, e.original_start, e.summary, e.location,
                        e.description, e.start_utc, e.end_utc, e.all_day, e.tz, e.rrule,
                        e.rdate, e.exdate, e.status, e.sequence, e.raw_ical, e.server_ical,
                        e.conflict_json
                 FROM event e JOIN calendar c ON c.id = e.calendar_id
                 WHERE c.subscribed = 1 AND (?3 = 0 OR c.visible = 1)
                   AND (e.rrule IS NOT NULL OR e.rdate IS NOT NULL OR
                        (e.start_utc < ?2 AND COALESCE(e.end_utc, e.start_utc + 1) > ?1))
                 ORDER BY COALESCE(e.start_utc, 0), e.id",
            )?;
            let rows =
                statement.query_map(params![start_utc, end_utc, visible_only as i64], |row| {
                    Ok(EventRow {
                        id: row.get(0)?,
                        calendar_id: row.get(1)?,
                        event: crate::calendar::CalendarEvent {
                            remote_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                            href: row.get(3)?,
                            etag: row.get(4)?,
                            ical_uid: row.get(5)?,
                            recurring_event_id: row.get(6)?,
                            original_start: row.get(7)?,
                            summary: row.get(8)?,
                            location: row.get(9)?,
                            description: row.get(10)?,
                            start_utc: row.get(11)?,
                            end_utc: row.get(12)?,
                            all_day: row.get::<_, i64>(13)? != 0,
                            tz: row.get(14)?,
                            rrule: row.get(15)?,
                            rdate: row.get(16)?,
                            exdate: row.get(17)?,
                            status: row.get(18)?,
                            sequence: row.get(19)?,
                            attendees: Vec::new(),
                            reminders: Vec::new(),
                            raw_ical: row.get(20)?,
                        },
                        server_ical: row.get(21)?,
                        conflict_json: row.get(22)?,
                    })
                })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn cancelled_occurrences(&self, event_id: i64) -> Result<Vec<String>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT original_start FROM event_exception
                 WHERE event_id = ?1 AND is_cancelled = 1 ORDER BY original_start",
            )?;
            let rows = statement.query_map([event_id], |row| row.get(0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Per-collection cursor plus the exact href/etag inventory for CalDAV's ctag fallback.
    pub fn calendar_cursor(
        &self,
        account_id: i64,
        provider: &str,
        calendar: &CalendarRow,
    ) -> Result<crate::calendar::CalendarCursor> {
        let state = self.get_sync_state(
            account_id,
            &format!("calendar:{provider}"),
            &calendar.info.remote_id,
        )?;
        let etags = self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT href, etag FROM event
                 WHERE calendar_id = ?1 AND href IS NOT NULL AND etag IS NOT NULL",
            )?;
            let rows = statement.query_map([calendar.id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        Ok(crate::calendar::CalendarCursor {
            sync_token: state.as_ref().and_then(|state| state.sync_token.clone()),
            ctag: state.as_ref().and_then(|state| state.ctag.clone()),
            etags,
        })
    }

    /// Atomically apply a terminal provider batch and only then advance its cursor (E11.3).
    pub fn apply_calendar_batch(
        &self,
        account_id: i64,
        provider: &str,
        calendar: &CalendarRow,
        batch: &crate::calendar::CalendarBatch,
        now: i64,
    ) -> Result<()> {
        self.transaction(|tx| {
            if batch.full_resync_required {
                tx.execute(
                    "INSERT INTO sync_state (
                        account_id, kind, collection, full_resync_needed, last_sync
                     ) VALUES (?1, ?2, ?3, 1, ?4)
                     ON CONFLICT(account_id, kind, collection) DO UPDATE SET
                        full_resync_needed = 1, sync_token = NULL, last_sync = excluded.last_sync",
                    params![
                        account_id,
                        format!("calendar:{provider}"),
                        calendar.info.remote_id,
                        now
                    ],
                )?;
                return Ok(());
            }
            for change in &batch.changes {
                match change {
                    crate::calendar::EventChange::Upsert(event) => {
                        upsert_calendar_event(tx, calendar.id, event, true)?;
                    }
                    crate::calendar::EventChange::Delete(identity) => match identity {
                        crate::calendar::EventIdentity::Event(remote_id) => {
                            tx.execute(
                                "DELETE FROM event WHERE calendar_id = ?1 AND remote_id = ?2",
                                params![calendar.id, remote_id],
                            )?;
                        }
                        crate::calendar::EventIdentity::Instance {
                            recurring_event_id,
                            original_start,
                        } => {
                            tx.execute(
                                "DELETE FROM event WHERE calendar_id = ?1
                                 AND recurring_event_id = ?2 AND original_start = ?3",
                                params![calendar.id, recurring_event_id, original_start],
                            )?;
                        }
                    },
                    crate::calendar::EventChange::CancelInstance {
                        recurring_event_id,
                        original_start,
                    } => {
                        use rusqlite::OptionalExtension as _;
                        let master: Option<i64> = tx
                            .query_row(
                                "SELECT id FROM event
                                 WHERE calendar_id = ?1 AND remote_id = ?2
                                   AND recurring_event_id IS NULL",
                                params![calendar.id, recurring_event_id],
                                |row| row.get(0),
                            )
                            .optional()?;
                        if let Some(master) = master {
                            tx.execute(
                                "INSERT INTO event_exception (event_id, original_start, is_cancelled)
                                 VALUES (?1, ?2, 1)
                                 ON CONFLICT(event_id, original_start) DO UPDATE SET is_cancelled = 1",
                                params![master, original_start],
                            )?;
                        }
                    }
                }
            }
            tx.execute(
                "INSERT INTO sync_state (
                    account_id, kind, collection, sync_token, ctag,
                    full_resync_needed, last_sync
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6)
                 ON CONFLICT(account_id, kind, collection) DO UPDATE SET
                    sync_token = COALESCE(excluded.sync_token, sync_state.sync_token),
                    ctag = COALESCE(excluded.ctag, sync_state.ctag),
                    full_resync_needed = 0, last_sync = excluded.last_sync",
                params![
                    account_id,
                    format!("calendar:{provider}"),
                    calendar.info.remote_id,
                    batch.next_sync_token,
                    batch.next_ctag,
                    now,
                ],
            )?;
            Ok(())
        })
    }

    /// A Google 410 invalidates one calendar only. Clearing happens immediately before its fresh
    /// pull so another collection's events and token are untouched.
    pub fn clear_calendar_for_resync(&self, calendar_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute("DELETE FROM event WHERE calendar_id = ?1", [calendar_id])?;
            Ok(())
        })
    }

    /// Optimistic edit + queued provider write in one transaction (E11.21).
    pub fn queue_calendar_write(
        &self,
        account_id: i64,
        calendar_id: i64,
        event: &crate::calendar::CalendarEvent,
        write: &crate::calendar::CalendarWrite,
        idempotency_key: &str,
        now: i64,
    ) -> Result<i64> {
        self.transaction(|tx| {
            let event_id = upsert_calendar_event(tx, calendar_id, event, false)?;
            tx.execute(
                "INSERT OR IGNORE INTO pending_op (
                    account_id, target_kind, target_id, operation, payload_json,
                    idempotency_key, attempts, state, created_at, updated_at
                 ) VALUES (?1, 'event', ?2, ?3, ?4, ?5, 0, 'pending', ?6, ?6)",
                params![
                    account_id,
                    event_id,
                    match write {
                        crate::calendar::CalendarWrite::Put { .. } => "calendar_put",
                        crate::calendar::CalendarWrite::Delete { .. } => "calendar_delete",
                    },
                    serde_json::to_string(write)?,
                    idempotency_key,
                    now,
                ],
            )?;
            Ok(event_id)
        })
    }

    pub fn record_calendar_conflict(
        &self,
        event_id: i64,
        server: Option<&crate::calendar::CalendarEvent>,
    ) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE event SET conflict_json = ?2 WHERE id = ?1",
                params![event_id, server.map(serde_json::to_string).transpose()?],
            )?;
            Ok(())
        })
    }

    /// Replace an optimistic row with the provider's canonical response after a successful write.
    pub fn accept_calendar_write(
        &self,
        event_id: i64,
        event: &crate::calendar::CalendarEvent,
    ) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE event SET remote_id = ?2, href = ?3, etag = ?4, ical_uid = ?5,
                    recurring_event_id = ?6, original_start = ?7, summary = ?8, location = ?9,
                    description = ?10, start_utc = ?11, end_utc = ?12, all_day = ?13, tz = ?14,
                    rrule = ?15, rdate = ?16, exdate = ?17, status = ?18, sequence = ?19,
                    raw_ical = ?20, server_ical = ?20, conflict_json = NULL
                 WHERE id = ?1",
                params![
                    event_id,
                    event.remote_id,
                    event.href,
                    event.etag,
                    event.ical_uid,
                    event.recurring_event_id,
                    event.original_start,
                    event.summary,
                    event.location,
                    event.description,
                    event.start_utc,
                    event.end_utc,
                    event.all_day as i64,
                    event.tz,
                    event.rrule,
                    event.rdate,
                    event.exdate,
                    event.status,
                    event.sequence,
                    event.raw_ical,
                ],
            )?;
            Ok(())
        })
    }

    pub fn delete_calendar_event(&self, event_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute("DELETE FROM event WHERE id = ?1", [event_id])?;
            Ok(())
        })
    }

    pub fn calendar_by_id(&self, calendar_id: i64) -> Result<Option<CalendarRow>> {
        use rusqlite::OptionalExtension as _;
        self.with_db(|conn| {
            conn.query_row(
                "SELECT account_id, provider, remote_id, href, name, color, selected, read_only,
                        access_role, subscribed, ctag, supports_sync_collection,
                        supports_scheduling, visible,
                        (SELECT count(*) FROM event WHERE event.calendar_id = calendar.id)
                 FROM calendar WHERE id = ?1",
                [calendar_id],
                |row| {
                    Ok(CalendarRow {
                        id: calendar_id,
                        account_id: row.get(0)?,
                        provider: row.get(1)?,
                        info: crate::calendar::CalendarInfo {
                            remote_id: row.get(2)?,
                            href: row.get(3)?,
                            name: row.get(4)?,
                            color: row.get(5)?,
                            selected: row.get::<_, i64>(6)? != 0,
                            read_only: row.get::<_, i64>(7)? != 0,
                            access_role: row.get(8)?,
                            subscribed: row.get::<_, i64>(9)? != 0,
                            ctag: row.get(10)?,
                            supports_sync_collection: row.get::<_, i64>(11)? != 0,
                            supports_scheduling: row.get::<_, i64>(12)? != 0,
                        },
                        visible: row.get::<_, i64>(13)? != 0,
                        event_count: row.get(14)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn event_by_id(&self, event_id: i64) -> Result<Option<EventRow>> {
        use rusqlite::OptionalExtension as _;
        self.with_db(|conn| {
            conn.query_row(
                "SELECT calendar_id, remote_id, href, etag, ical_uid, recurring_event_id,
                        original_start, summary, location, description, start_utc, end_utc,
                        all_day, tz, rrule, rdate, exdate, status, sequence, raw_ical,
                        server_ical, conflict_json
                 FROM event WHERE id = ?1",
                [event_id],
                |row| {
                    Ok(EventRow {
                        id: event_id,
                        calendar_id: row.get(0)?,
                        event: crate::calendar::CalendarEvent {
                            remote_id: row.get(1)?,
                            href: row.get(2)?,
                            etag: row.get(3)?,
                            ical_uid: row.get(4)?,
                            recurring_event_id: row.get(5)?,
                            original_start: row.get(6)?,
                            summary: row.get(7)?,
                            location: row.get(8)?,
                            description: row.get(9)?,
                            start_utc: row.get(10)?,
                            end_utc: row.get(11)?,
                            all_day: row.get::<_, i64>(12)? != 0,
                            tz: row.get(13)?,
                            rrule: row.get(14)?,
                            rdate: row.get(15)?,
                            exdate: row.get(16)?,
                            status: row.get(17)?,
                            sequence: row.get(18)?,
                            attendees: Vec::new(),
                            reminders: Vec::new(),
                            raw_ical: row.get(19)?,
                        },
                        server_ical: row.get(20)?,
                        conflict_json: row.get(21)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn event_by_ical_uid(&self, uid: &str) -> Result<Option<EventRow>> {
        use rusqlite::OptionalExtension as _;
        let id = self.with_db(|conn| {
            conn.query_row(
                "SELECT id FROM event WHERE ical_uid = ?1 ORDER BY id LIMIT 1",
                [uid],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(Into::into)
        })?;
        id.map(|id| self.event_by_id(id))
            .transpose()
            .map(Option::flatten)
    }

    pub fn replace_event_reminders(&self, event_id: i64, reminders: &[ReminderKind]) -> Result<()> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM reminder WHERE event_id = ?1", [event_id])?;
            for reminder in reminders {
                let (minutes, absolute, same_day): (Option<i64>, Option<i64>, Option<i64>) =
                    match reminder {
                        ReminderKind::MinutesBefore(value) => (Some(*value), None, None),
                        ReminderKind::SameDayMinute(value) => (None, None, Some(*value as i64)),
                        ReminderKind::Absolute(value) => (None, Some(*value), None),
                    };
                tx.execute(
                    "INSERT INTO reminder (event_id, minutes_before, absolute_utc, same_day_minute)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![event_id, minutes, absolute, same_day],
                )?;
            }
            Ok(())
        })
    }

    pub fn reminders_for_event(&self, event_id: i64) -> Result<Vec<ReminderRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, minutes_before, absolute_utc, same_day_minute
                 FROM reminder WHERE event_id = ?1 ORDER BY id",
            )?;
            let rows = statement.query_map([event_id], |row| {
                let minutes: Option<i64> = row.get(1)?;
                let absolute: Option<i64> = row.get(2)?;
                let same_day: Option<i64> = row.get(3)?;
                let kind = if let Some(value) = minutes {
                    ReminderKind::MinutesBefore(value)
                } else if let Some(value) = same_day {
                    ReminderKind::SameDayMinute(value.clamp(0, 1439) as u16)
                } else {
                    ReminderKind::Absolute(absolute.unwrap_or_default())
                };
                Ok(ReminderRow {
                    id: row.get(0)?,
                    event_id,
                    kind,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn schedule_reminder_delivery(
        &self,
        reminder_id: i64,
        occurrence_start: i64,
        due_utc: i64,
    ) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO reminder_delivery (reminder_id, occurrence_start, due_utc)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(reminder_id, occurrence_start) DO UPDATE SET
                    due_utc = excluded.due_utc
                 WHERE reminder_delivery.fired_at IS NULL",
                params![reminder_id, occurrence_start, due_utc],
            )?;
            Ok(())
        })
    }

    pub fn due_reminders(
        &self,
        now: i64,
        late_after: i64,
        limit: usize,
    ) -> Result<Vec<DueReminder>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT d.reminder_id, r.event_id, d.occurrence_start, d.due_utc,
                        COALESCE(e.summary, 'Untitled event')
                 FROM reminder_delivery d
                 JOIN reminder r ON r.id = d.reminder_id
                 JOIN event e ON e.id = r.event_id
                 WHERE d.fired_at IS NULL AND d.due_utc <= ?1 AND d.due_utc >= ?2
                 ORDER BY d.due_utc LIMIT ?3",
            )?;
            let rows = statement.query_map(params![now, late_after, limit as i64], |row| {
                Ok(DueReminder {
                    reminder_id: row.get(0)?,
                    event_id: row.get(1)?,
                    occurrence_start: row.get(2)?,
                    due_utc: row.get(3)?,
                    title: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn mark_reminder_fired(
        &self,
        reminder_id: i64,
        occurrence_start: i64,
        fired_at: i64,
    ) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE reminder_delivery SET fired_at = ?3
                 WHERE reminder_id = ?1 AND occurrence_start = ?2 AND fired_at IS NULL",
                params![reminder_id, occurrence_start, fired_at],
            )?;
            Ok(())
        })
    }

    pub fn replace_event_attendees(&self, event_id: i64, attendees: &[AttendeeRow]) -> Result<()> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM attendee WHERE event_id = ?1", [event_id])?;
            for attendee in attendees {
                tx.execute(
                    "INSERT INTO attendee
                     (event_id, email, display_name, role, status, is_self)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        event_id,
                        attendee.email,
                        attendee.display_name,
                        attendee.role,
                        attendee.status,
                        attendee.is_self as i64,
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn attendees_for_event(&self, event_id: i64) -> Result<Vec<AttendeeRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, email, display_name, role, status, is_self
                 FROM attendee WHERE event_id = ?1 ORDER BY is_self DESC, email",
            )?;
            let rows = statement.query_map([event_id], |row| {
                Ok(AttendeeRow {
                    id: row.get(0)?,
                    event_id,
                    email: row.get(1)?,
                    display_name: row.get(2)?,
                    role: row.get(3)?,
                    status: row.get(4)?,
                    is_self: row.get::<_, i64>(5)? != 0,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn insert_task(
        &self,
        account_id: i64,
        title: &str,
        notes: Option<&str>,
        due_utc: Option<i64>,
        now: i64,
    ) -> Result<i64> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO task (account_id, title, notes, due_utc, done, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![account_id, title, notes, due_utc, now],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn tasks_between(&self, start_utc: i64, end_utc: i64) -> Result<Vec<TaskRow>> {
        self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, account_id, title, notes, due_utc, done, created_at
                 FROM task WHERE due_utc IS NULL OR (due_utc >= ?1 AND due_utc < ?2)
                 ORDER BY done, due_utc, created_at",
            )?;
            let rows = statement.query_map(params![start_utc, end_utc], |row| {
                Ok(TaskRow {
                    id: row.get(0)?,
                    account_id: row.get(1)?,
                    title: row.get(2)?,
                    notes: row.get(3)?,
                    due_utc: row.get(4)?,
                    done: row.get::<_, i64>(5)? != 0,
                    created_at: row.get(6)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn set_task_done(&self, task_id: i64, done: bool) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "UPDATE task SET done = ?2 WHERE id = ?1",
                params![task_id, done as i64],
            )?;
            Ok(())
        })
    }

    pub fn stats(&self) -> Result<StoreStats> {
        self.with_db(|conn| {
            Ok(StoreStats {
                messages: conn.query_row("SELECT count(*) FROM message", [], |row| row.get(0))?,
                events: conn.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?,
                tasks: conn.query_row("SELECT count(*) FROM task", [], |row| row.get(0))?,
            })
        })
    }

    pub fn clear_cache(&self) -> Result<()> {
        self.cache.clear()
    }

    pub fn rebuild_search_index(&self) -> Result<()> {
        self.with_db(|conn| {
            conn.execute(
                "INSERT INTO message_fts (message_fts) VALUES ('rebuild')",
                [],
            )?;
            Ok(())
        })
    }

    pub fn force_full_resync(&self, account_id: i64, now: i64) -> Result<()> {
        self.with_db(|conn| {
            let changed = conn.execute(
                "UPDATE sync_state SET full_resync_needed = 1, cursor_json = NULL,
                    history_id = NULL, sync_token = NULL, ctag = NULL, last_sync = ?2
                 WHERE account_id = ?1",
                params![account_id, now],
            )?;
            if changed == 0 {
                conn.execute(
                    "INSERT INTO sync_state
                        (account_id, kind, collection, full_resync_needed, last_sync)
                     VALUES (?1, 'account', '', 1, ?2)",
                    params![account_id, now],
                )?;
            }
            Ok(())
        })
    }

    /// Remove an account and everything that cascades from it (E14.9).
    pub fn delete_account(&self, account_id: i64) -> Result<()> {
        let hashes = self.with_db(|conn| {
            let mut statement = conn.prepare(
                "SELECT raw_hash FROM message WHERE account_id = ?1 AND raw_hash IS NOT NULL
                 UNION SELECT body_hash FROM message
                       WHERE account_id = ?1 AND body_hash IS NOT NULL",
            )?;
            let hashes = statement
                .query_map([account_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            conn.execute("DELETE FROM account WHERE id = ?1", [account_id])?;
            Ok(hashes)
        })?;
        for hash in hashes {
            let referenced = self.with_db(|conn| {
                let messages: i64 = conn.query_row(
                    "SELECT count(*) FROM message WHERE raw_hash = ?1 OR body_hash = ?1",
                    [&hash],
                    |row| row.get(0),
                )?;
                let operations: i64 = conn.query_row(
                    "SELECT count(*) FROM pending_op WHERE payload_json LIKE '%' || ?1 || '%'",
                    [&hash],
                    |row| row.get(0),
                )?;
                Ok(messages + operations)
            })?;
            if referenced == 0 {
                self.cache.remove(&hash)?;
            }
        }
        Ok(())
    }
}

fn upsert_calendar_event(
    tx: &rusqlite::Transaction,
    calendar_id: i64,
    event: &crate::calendar::CalendarEvent,
    from_server: bool,
) -> Result<i64> {
    use rusqlite::OptionalExtension as _;
    let existing: Option<i64> = if let (Some(series), Some(original)) =
        (&event.recurring_event_id, &event.original_start)
    {
        tx.query_row(
            "SELECT id FROM event WHERE calendar_id = ?1
             AND recurring_event_id = ?2 AND original_start = ?3",
            params![calendar_id, series, original],
            |row| row.get(0),
        )
        .optional()?
    } else {
        tx.query_row(
            "SELECT id FROM event WHERE calendar_id = ?1 AND remote_id = ?2",
            params![calendar_id, event.remote_id],
            |row| row.get(0),
        )
        .optional()?
    };
    if let Some(id) = existing {
        if from_server {
            let conflict: Option<String> = tx.query_row(
                "SELECT conflict_json FROM event WHERE id = ?1",
                [id],
                |row| row.get(0),
            )?;
            if conflict.is_some() {
                // A pull after HTTP 412 refreshes the server side of the conflict but must not
                // overwrite the optimistic local row the user still needs to resolve.
                tx.execute(
                    "UPDATE event SET server_ical = ?2, conflict_json = ?3 WHERE id = ?1",
                    params![id, event.raw_ical, serde_json::to_string(event)?],
                )?;
                replace_event_children_tx(tx, id, event)?;
                return Ok(id);
            }
        }
        tx.execute(
            "UPDATE event SET remote_id = ?2, href = ?3, etag = ?4, ical_uid = ?5,
                recurring_event_id = ?6, original_start = ?7, summary = ?8, location = ?9,
                description = ?10, start_utc = ?11, end_utc = ?12, all_day = ?13, tz = ?14,
                rrule = ?15, rdate = ?16, exdate = ?17, status = ?18, sequence = ?19,
                raw_ical = ?20,
                server_ical = CASE WHEN ?21 THEN ?20 ELSE server_ical END,
                conflict_json = CASE WHEN ?21 THEN NULL ELSE conflict_json END
             WHERE id = ?1",
            params![
                id,
                event.remote_id,
                event.href,
                event.etag,
                event.ical_uid,
                event.recurring_event_id,
                event.original_start,
                event.summary,
                event.location,
                event.description,
                event.start_utc,
                event.end_utc,
                event.all_day as i64,
                event.tz,
                event.rrule,
                event.rdate,
                event.exdate,
                event.status,
                event.sequence,
                event.raw_ical,
                from_server as i64,
            ],
        )?;
        replace_event_children_tx(tx, id, event)?;
        return Ok(id);
    }
    tx.execute(
        "INSERT INTO event (
            calendar_id, remote_id, href, etag, ical_uid, recurring_event_id, original_start,
            summary, location, description, start_utc, end_utc, all_day, tz, rrule, rdate,
            exdate, status, sequence, raw_ical, server_ical
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, CASE WHEN ?21 THEN ?20 ELSE NULL END
         )",
        params![
            calendar_id,
            event.remote_id,
            event.href,
            event.etag,
            event.ical_uid,
            event.recurring_event_id,
            event.original_start,
            event.summary,
            event.location,
            event.description,
            event.start_utc,
            event.end_utc,
            event.all_day as i64,
            event.tz,
            event.rrule,
            event.rdate,
            event.exdate,
            event.status,
            event.sequence,
            event.raw_ical,
            from_server as i64,
        ],
    )?;
    let id = tx.last_insert_rowid();
    replace_event_children_tx(tx, id, event)?;
    Ok(id)
}

fn replace_event_children_tx(
    tx: &rusqlite::Transaction,
    event_id: i64,
    event: &crate::calendar::CalendarEvent,
) -> Result<()> {
    tx.execute("DELETE FROM attendee WHERE event_id = ?1", [event_id])?;
    for attendee in &event.attendees {
        tx.execute(
            "INSERT INTO attendee (event_id, email, display_name, role, status, is_self)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                event_id,
                attendee.email,
                attendee.display_name,
                attendee.role,
                attendee.status,
                attendee.is_self as i64,
            ],
        )?;
    }
    tx.execute("DELETE FROM reminder WHERE event_id = ?1", [event_id])?;
    for reminder in &event.reminders {
        let (minutes, absolute, same_day): (Option<i64>, Option<i64>, Option<i64>) = match reminder
        {
            crate::calendar::CalendarReminder::MinutesBefore(value) => (Some(*value), None, None),
            crate::calendar::CalendarReminder::SameDayMinute(value) => {
                (None, None, Some(*value as i64))
            }
            crate::calendar::CalendarReminder::Absolute(value) => (None, Some(*value), None),
        };
        tx.execute(
            "INSERT INTO reminder (event_id, minutes_before, absolute_utc, same_day_minute)
             VALUES (?1, ?2, ?3, ?4)",
            params![event_id, minutes, absolute, same_day],
        )?;
    }
    Ok(())
}

/// Put a message back where a triage payload says it was, and return its account id (E8.4/E8.5).
/// Shared by undo and by the rollback of an op the provider rejected.
fn restore_triage_row(
    tx: &rusqlite::Transaction,
    message_id: i64,
    payload: &crate::triage::TriagePayload,
) -> Result<i64> {
    let account_id: i64 = tx.query_row(
        "SELECT account_id FROM message WHERE id = ?1",
        [message_id],
        |row| row.get(0),
    )?;
    let mailbox_before = mailbox_of(tx, message_id)?;
    if let Some(name) = &payload.previous_mailbox {
        tx.execute(
            "INSERT OR IGNORE INTO mailbox (account_id, name, kind) VALUES (?1, ?2, ?3)",
            params![account_id, name, kind_for_name(name)],
        )?;
        let mailbox_id: i64 = tx.query_row(
            "SELECT id FROM mailbox WHERE account_id = ?1 AND name = ?2",
            params![account_id, name],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE message SET mailbox_id = ?2 WHERE id = ?1",
            params![message_id, mailbox_id],
        )?;
    }
    tx.execute(
        "UPDATE message SET unread = ?2, starred = ?3 WHERE id = ?1",
        params![
            message_id,
            payload.previous_unread as i64,
            payload.previous_flagged as i64
        ],
    )?;
    recount_touched(tx, message_id, mailbox_before)?;
    Ok(account_id)
}

fn mailbox_of(tx: &rusqlite::Transaction, message_id: i64) -> Result<Option<i64>> {
    Ok(tx.query_row(
        "SELECT mailbox_id FROM message WHERE id = ?1",
        [message_id],
        |row| row.get(0),
    )?)
}

/// Keep the stored counters honest after a local change: the mailbox the message left and the one
/// it is in now. Without this the sidebar and the Dock badge lag until the next sync recounts.
fn recount_touched(tx: &rusqlite::Transaction, message_id: i64, before: Option<i64>) -> Result<()> {
    let after = mailbox_of(tx, message_id)?;
    let mut touched = vec![before, after];
    touched.dedup();
    for mailbox_id in touched.into_iter().flatten() {
        tx.execute(
            "UPDATE mailbox SET
                total = (SELECT count(*) FROM message WHERE mailbox_id = ?1),
                unread = (SELECT count(*) FROM message WHERE mailbox_id = ?1 AND unread = 1)
             WHERE id = ?1",
            [mailbox_id],
        )?;
    }
    Ok(())
}

/// Build the FTS5 MATCH expression (E9.1). Every term is quoted, so punctuation cannot be read as
/// query syntax; a multi-word fallback term becomes a phrase. Terms with nothing indexable are
/// dropped rather than handed to FTS5, which would reject them.
fn fts_match(query: &SearchQuery) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for value in query.phrases.iter().chain(query.terms.iter()) {
        if value.chars().any(char::is_alphanumeric) {
            parts.push(quote_fts(value));
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" AND "))
    }
}

fn quote_fts(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// A LIKE pattern matching `value` anywhere, escaping LIKE's metacharacters.
fn like_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for ch in value.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    pattern
}

fn kind_for_name(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "inbox" => "inbox",
        "sent" | "sent messages" => "sent",
        "drafts" => "drafts",
        "archive" | "all mail" => "archive",
        "trash" | "deleted messages" => "trash",
        _ => "other",
    }
}

fn configure(conn: &Connection) -> Result<()> {
    // WAL for a responsive reader alongside the writer; NORMAL is the right durability for a cache.
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
    Ok(())
}

/// The store is only reachable from the background executor, never a view (E2.4).
#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_account() -> Store {
        let store = Store::open_in_memory().unwrap();
        store
            .insert_account("gmail", "me@example.com", Some("Me"), 0)
            .unwrap();
        store
    }

    #[test]
    fn messages_round_trip_with_a_provider_ref() {
        let store = store_with_account();
        let id = store
            .insert_message(&NewMessage {
                account_id: 1,
                provider: Some(ProviderRef::Gmail {
                    id: "18f".into(),
                    thread_id: Some("t1".into()),
                    history_id: Some(42),
                }),
                subject: Some("Hello".into()),
                from_addr: Some("a@b.c".into()),
                date: Some(1000),
                unread: true,
                ..Default::default()
            })
            .unwrap();
        assert!(id > 0);

        let page = store.page(1, 10, 0).unwrap();
        // No mailbox assigned, so the page is empty; query directly instead.
        assert!(page.is_empty());

        let subject: String = store
            .with_db(|conn| {
                Ok(
                    conn.query_row("SELECT subject FROM message WHERE id = ?1", [id], |r| {
                        r.get(0)
                    })?,
                )
            })
            .unwrap();
        assert_eq!(subject, "Hello");
    }

    #[test]
    fn an_op_is_enqueued_once_per_idempotency_key() {
        let store = store_with_account();
        let op = NewOp {
            account_id: 1,
            target_kind: "message".into(),
            target_id: Some(7),
            operation: "archive".into(),
            payload_json: None,
            idempotency_key: "archive:7".into(),
            now: 0,
        };
        store.enqueue_op(&op).unwrap();
        store.enqueue_op(&op).unwrap();
        assert_eq!(store.pending_ops(10).unwrap().len(), 1);
        assert_eq!(store.op_counts().unwrap().outstanding(), 1);
    }

    #[test]
    fn an_optimistic_change_and_its_op_share_a_transaction() {
        let store = store_with_account();
        store
            .transaction(|tx| {
                tx.execute("UPDATE message SET unread = 0 WHERE id = 1", [])?;
                tx.execute(
                    "INSERT INTO pending_op (account_id, target_kind, target_id, operation,
                        idempotency_key, attempts, state, created_at, updated_at)
                     VALUES (1, 'message', 1, 'mark_read', 'read:1', 0, 'pending', 0, 0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(store.pending_ops(10).unwrap().len(), 1);
    }

    #[test]
    fn sync_state_round_trips_and_can_ask_for_a_full_resync() {
        let store = store_with_account();
        assert_eq!(store.get_sync_state(1, "gmail", "").unwrap(), None);
        store
            .set_sync_state(&SyncState {
                account_id: 1,
                kind: "gmail".into(),
                collection: String::new(),
                history_id: Some(999),
                full_resync_needed: false,
                ..Default::default()
            })
            .unwrap();
        let state = store.get_sync_state(1, "gmail", "").unwrap().unwrap();
        assert_eq!(state.history_id, Some(999));

        // A 404 history window invalidates the cursor; recovery is a first-class state (E2.6).
        store
            .set_sync_state(&SyncState {
                account_id: 1,
                kind: "gmail".into(),
                full_resync_needed: true,
                ..Default::default()
            })
            .unwrap();
        assert!(
            store
                .get_sync_state(1, "gmail", "")
                .unwrap()
                .unwrap()
                .full_resync_needed
        );
    }

    #[test]
    fn accounts_and_identities_round_trip() {
        let store = store_with_account();
        assert_eq!(store.accounts().unwrap().len(), 1);
        assert_eq!(
            store
                .account_by_address("gmail", "me@example.com")
                .unwrap()
                .unwrap()
                .id,
            1
        );
        store
            .insert_identity(&NewIdentity {
                account_id: 1,
                address: "alias@icloud.com".into(),
                display_name: Some("Me".into()),
                signature: None,
                is_default: false,
            })
            .unwrap();
        let identities = store.identities(1).unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].address, "alias@icloud.com");
    }

    #[test]
    fn deleting_an_account_cascades_to_its_identities_and_messages() {
        let store = store_with_account();
        store
            .insert_identity(&NewIdentity {
                account_id: 1,
                address: "a@b.c".into(),
                display_name: None,
                signature: None,
                is_default: true,
            })
            .unwrap();
        store.delete_account(1).unwrap();
        assert!(store.accounts().unwrap().is_empty());
        assert!(store.identities(1).unwrap().is_empty());
    }

    #[test]
    fn insert_message_if_new_is_idempotent_so_backfill_converges() {
        let store = store_with_account();
        let message = NewMessage {
            account_id: 1,
            provider: Some(ProviderRef::Gmail {
                id: "g1".into(),
                thread_id: None,
                history_id: None,
            }),
            subject: Some("x".into()),
            ..Default::default()
        };
        assert!(store.insert_message_if_new(&message).unwrap());
        assert!(
            !store.insert_message_if_new(&message).unwrap(),
            "re-run must not duplicate"
        );
    }

    #[test]
    fn drafts_are_saved_updated_and_deleted() {
        let store = store_with_account();
        let id = store
            .save_draft(None, 1, "Hello", "first line\nsecond", "[]", None, 10)
            .unwrap();
        let drafts = store
            .mailbox_id(1, "Drafts")
            .unwrap()
            .expect("Drafts created");
        let page = store.page(drafts, 10, 0).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].subject.as_deref(), Some("Hello"));
        assert_eq!(page[0].unread, false);

        // Autosave updates in place rather than inserting another.
        let again = store
            .save_draft(Some(id), 1, "Hello there", "edited", "[]", None, 20)
            .unwrap();
        assert_eq!(again, id);
        assert_eq!(store.page(drafts, 10, 0).unwrap().len(), 1);
        assert_eq!(store.latest_draft(1).unwrap().unwrap().id, id);

        store.delete_message(id).unwrap();
        assert!(store.page(drafts, 10, 0).unwrap().is_empty());
    }

    #[test]
    fn contacts_are_harvested_and_suggested() {
        let store = store_with_account();
        for (id, date) in [("g1", 100), ("g2", 200)] {
            store
                .insert_message(&NewMessage {
                    account_id: 1,
                    provider: Some(ProviderRef::Gmail {
                        id: id.into(),
                        thread_id: None,
                        history_id: None,
                    }),
                    from_name: Some("Maya".into()),
                    from_addr: Some("maya@example.com".into()),
                    to_json: Some(r#"[{"name":null,"address":"me@example.com"}]"#.into()),
                    date: Some(date),
                    ..Default::default()
                })
                .unwrap();
        }
        let count = store.harvest_contacts(1).unwrap();
        assert!(count >= 2, "maya and me");
        let suggestions = store.suggest_contacts(1, "may", 5).unwrap();
        assert_eq!(suggestions[0].address, "maya@example.com");
        assert_eq!(suggestions[0].name.as_deref(), Some("Maya"));
        assert_eq!(suggestions[0].score, 2, "received twice, never written to");
        // Prefix matches the name too.
        assert!(
            store
                .suggest_contacts(1, "me@", 5)
                .unwrap()
                .iter()
                .any(|c| c.address == "me@example.com")
        );
    }

    #[test]
    fn failed_ops_can_be_requeued_explicitly() {
        let store = store_with_account();
        store
            .enqueue_op(&NewOp {
                account_id: 1,
                target_kind: "message".into(),
                target_id: None,
                operation: "send".into(),
                payload_json: None,
                idempotency_key: "send:1".into(),
                now: 0,
            })
            .unwrap();
        let id = store.pending_ops(10).unwrap()[0].id;
        store.set_op_state(id, "failed", Some("smtp"), 0).unwrap();
        store.set_op_state(id, "dead", Some("smtp"), 0).unwrap();
        assert_eq!(store.op_counts().unwrap().dead, 1);
        assert_eq!(store.requeue_failed_ops().unwrap(), 1);
        assert_eq!(store.op_counts().unwrap().pending, 1);
    }

    #[test]
    fn archive_is_optimistic_and_undoable() {
        use crate::triage::TriageAction;
        let store = store_with_account();
        let inbox = store.ensure_mailbox(1, "Inbox", "inbox").unwrap();
        let message = store
            .insert_message(&NewMessage {
                account_id: 1,
                mailbox_id: Some(inbox),
                provider: Some(ProviderRef::Gmail {
                    id: "g1".into(),
                    thread_id: None,
                    history_id: None,
                }),
                subject: Some("x".into()),
                unread: true,
                ..Default::default()
            })
            .unwrap();

        let op = store
            .apply_triage(message, &TriageAction::Archive, "archive:1", 0)
            .unwrap();
        let archive = store.mailbox_id(1, "Archive").unwrap().unwrap();
        assert_eq!(
            store.page(archive, 10, 0).unwrap().len(),
            1,
            "moved locally"
        );
        assert_eq!(store.pending_ops(10).unwrap().len(), 1, "queued remotely");

        // Undo before dispatch: back to Inbox, op cancelled (not retried).
        assert!(!store.undo_triage(op, 1).unwrap());
        assert_eq!(store.page(inbox, 10, 0).unwrap().len(), 1);
        assert!(
            store.pending_ops(10).unwrap().is_empty(),
            "cancelled op is not pending"
        );
    }

    #[test]
    fn mark_read_undo_restores_the_flag() {
        use crate::triage::TriageAction;
        let store = store_with_account();
        let message = store
            .insert_message(&NewMessage {
                account_id: 1,
                provider: Some(ProviderRef::Gmail {
                    id: "g1".into(),
                    thread_id: None,
                    history_id: None,
                }),
                unread: true,
                ..Default::default()
            })
            .unwrap();
        let op = store
            .apply_triage(message, &TriageAction::MarkRead(true), "read:1", 0)
            .unwrap();
        assert!(!store.message(message).unwrap().unwrap().unread);
        store.set_op_state(op, "done", None, 1).unwrap();
        // Already dispatched: undo queues the inverse.
        assert!(store.undo_triage(op, 2).unwrap());
        assert!(
            store.message(message).unwrap().unwrap().unread,
            "flag restored"
        );
        assert_eq!(store.pending_ops(10).unwrap().len(), 1, "inverse queued");
    }

    #[test]
    fn thread_page_keeps_a_message_without_a_thread() {
        let store = store_with_account();
        let inbox = store.ensure_mailbox(1, "Inbox", "inbox").unwrap();
        let thread = store.ensure_thread(1, Some("t1"), None).unwrap();
        let threaded = store
            .insert_message(&NewMessage {
                account_id: 1,
                mailbox_id: Some(inbox),
                thread_id: Some(thread),
                provider: Some(ProviderRef::Gmail {
                    id: "a".into(),
                    thread_id: None,
                    history_id: None,
                }),
                subject: Some("hi".into()),
                date: Some(10),
                ..Default::default()
            })
            .unwrap();
        let solo = store
            .insert_message(&NewMessage {
                account_id: 1,
                mailbox_id: Some(inbox),
                thread_id: None,
                provider: Some(ProviderRef::Gmail {
                    id: "b".into(),
                    thread_id: None,
                    history_id: None,
                }),
                subject: Some("solo".into()),
                date: Some(20),
                ..Default::default()
            })
            .unwrap();

        let page = store.thread_page(inbox, 10).unwrap();
        assert_eq!(page.len(), 2, "the threadless message is its own thread");
        assert_eq!(page[0].newest_id, solo, "newest first");
        assert_eq!(
            page[0].thread_id, -solo,
            "keyed by -id so it never collides"
        );
        assert!(page.iter().any(|row| row.newest_id == threaded));
        // The threadless message is not dropped from the grouped list.
        assert!(page.iter().any(|row| row.newest_id == solo));
    }

    #[test]
    fn a_failed_triage_op_rolls_back_to_where_it_was() {
        use crate::triage::TriageAction;
        let store = store_with_account();
        let inbox = store.ensure_mailbox(1, "Inbox", "inbox").unwrap();
        let message = store
            .insert_message(&NewMessage {
                account_id: 1,
                mailbox_id: Some(inbox),
                provider: Some(ProviderRef::Gmail {
                    id: "g1".into(),
                    thread_id: None,
                    history_id: None,
                }),
                subject: Some("x".into()),
                unread: true,
                ..Default::default()
            })
            .unwrap();
        let op = store
            .apply_triage(message, &TriageAction::Archive, "archive:rb", 0)
            .unwrap();
        // The provider rejected it.
        store.set_op_state(op, "failed", Some("quota"), 1).unwrap();

        assert_eq!(store.rollback_failed_triage(2).unwrap(), 1);
        assert_eq!(store.page(inbox, 10, 0).unwrap().len(), 1, "back in Inbox");
        assert!(
            store.pending_ops(10).unwrap().is_empty(),
            "a rolled-back op is not retried"
        );
        // Rolling back again is a no-op.
        assert_eq!(store.rollback_failed_triage(3).unwrap(), 0);
    }

    #[test]
    fn search_finds_body_text_and_applies_every_filter() {
        let store = store_with_account();
        let inbox = store.ensure_mailbox(1, "Inbox", "inbox").unwrap();
        let archive = store.ensure_mailbox(1, "Archive", "archive").unwrap();
        let insert = |mailbox: i64,
                      id: &str,
                      subject: &str,
                      from: &str,
                      date: i64,
                      unread: bool,
                      body: &str,
                      has_attachments: bool| {
            store
                .insert_message(&NewMessage {
                    account_id: 1,
                    mailbox_id: Some(mailbox),
                    provider: Some(ProviderRef::Gmail {
                        id: id.into(),
                        thread_id: None,
                        history_id: None,
                    }),
                    subject: Some(subject.into()),
                    from_addr: Some(from.into()),
                    date: Some(date),
                    unread,
                    has_attachments,
                    body_text: Some(body.into()),
                    ..Default::default()
                })
                .unwrap()
        };
        let almanac = insert(
            inbox,
            "a",
            "Almanac geometry",
            "maya@example.com",
            1000,
            true,
            "the 6x7 grid maths checks out",
            false,
        );
        insert(
            archive,
            "b",
            "Receipt",
            "shop@example.com",
            2000,
            false,
            "your order shipped",
            true,
        );

        // Free text reaches the body; a phrase keeps its words together.
        let hits = store
            .search(
                &SearchQuery {
                    terms: vec!["almanac".into()],
                    ..Default::default()
                },
                50,
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, almanac);
        assert_eq!(hits[0].mailbox_name.as_deref(), Some("Inbox"));

        let hits = store
            .search(
                &SearchQuery {
                    phrases: vec!["grid maths".into()],
                    ..Default::default()
                },
                50,
            )
            .unwrap();
        assert_eq!(hits.len(), 1, "phrase spans the body");

        // Field and flag filters.
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        from: vec!["shop".into()],
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        subject: vec!["receipt".into()],
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        unread: Some(true),
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        has_attachment: true,
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        after: Some(1500),
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .search(
                    &SearchQuery {
                        mailbox: Some("Inbox".into()),
                        ..Default::default()
                    },
                    50,
                )
                .unwrap()
                .len(),
            1
        );

        // No criteria: newest first within mailbox, mailboxes ordered by name.
        let hits = store.search(&SearchQuery::default(), 50).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].mailbox_name.as_deref(), Some("Archive"));
        assert_eq!(hits[1].mailbox_name.as_deref(), Some("Inbox"));
    }

    #[test]
    fn local_triage_keeps_the_unread_counters_current() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let inbox = store.ensure_mailbox(account, "Inbox", "inbox").unwrap();
        let message = store
            .insert_message(&NewMessage {
                account_id: account,
                mailbox_id: Some(inbox),
                unread: true,
                ..Default::default()
            })
            .unwrap();
        store.recount_mailbox(inbox).unwrap();
        let unread = |store: &Store| {
            store
                .mailboxes()
                .unwrap()
                .into_iter()
                .find(|b| b.id == inbox)
                .unwrap()
                .unread
        };
        assert_eq!(unread(&store), 1);
        let op = store
            .apply_triage(
                message,
                &crate::triage::TriageAction::MarkRead(true),
                "read",
                0,
            )
            .unwrap();
        assert_eq!(
            unread(&store),
            0,
            "reading a message updates the count at once"
        );
        store.undo_triage(op, 1).unwrap();
        assert_eq!(unread(&store), 1, "and so does undoing it");
        // Archiving moves the count with the message.
        store
            .apply_triage(message, &crate::triage::TriageAction::Archive, "archive", 2)
            .unwrap();
        assert_eq!(unread(&store), 0);
    }

    #[test]
    fn only_new_unread_recent_inbox_mail_is_offered_for_notification() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let inbox = store.ensure_mailbox(account, "Inbox", "inbox").unwrap();
        let sent = store.ensure_mailbox(account, "Sent", "sent").unwrap();
        let add = |mailbox, unread, date| {
            store
                .insert_message(&NewMessage {
                    account_id: account,
                    mailbox_id: Some(mailbox),
                    unread,
                    date: Some(date),
                    ..Default::default()
                })
                .unwrap()
        };
        let old = add(inbox, true, 1_000);
        let watermark = store.max_message_id().unwrap();
        assert_eq!(watermark, old);
        let fresh = add(inbox, true, 5_000);
        add(inbox, false, 5_000); // already read elsewhere
        add(sent, true, 5_000); // not the inbox
        add(inbox, true, 10); // arrived late but ancient
        let rows = store.new_unread_inbox(watermark, 4_000, 10).unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![fresh]
        );
    }

    #[test]
    fn unsubscribing_a_calendar_keeps_its_events() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, EventChange};

        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    calendars: vec![CalendarInfo {
                        remote_id: "work".into(),
                        name: Some("Work".into()),
                        subscribed: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        store
            .apply_calendar_batch(
                account,
                "google",
                &calendar,
                &CalendarBatch {
                    changes: vec![EventChange::Upsert(Box::new(CalendarEvent {
                        remote_id: "event".into(),
                        summary: Some("Keep me".into()),
                        ..Default::default()
                    }))],
                    next_sync_token: Some("token".into()),
                    ..Default::default()
                },
                2,
            )
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    unsubscribed: vec!["work".into()],
                    ..Default::default()
                },
                3,
            )
            .unwrap();
        assert!(
            !store.calendars_for_account(account).unwrap()[0]
                .info
                .subscribed
        );
        let count: i64 = store
            .with_db(|db| Ok(db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?))
            .unwrap();
        assert_eq!(
            count, 1,
            "subscription state must not delete the calendar itself"
        );
    }

    #[test]
    fn calendar_visibility_filters_buffered_range_reads_without_deleting_events() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, EventChange};

        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "calendar@example.com", None, 0)
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    calendars: vec![CalendarInfo {
                        remote_id: "primary".into(),
                        subscribed: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        store
            .apply_calendar_batch(
                account,
                "google",
                &calendar,
                &CalendarBatch {
                    changes: vec![
                        EventChange::Upsert(Box::new(CalendarEvent {
                            remote_id: "inside".into(),
                            start_utc: Some(100),
                            end_utc: Some(200),
                            ..Default::default()
                        })),
                        EventChange::Upsert(Box::new(CalendarEvent {
                            remote_id: "recurring-before-range".into(),
                            start_utc: Some(0),
                            end_utc: Some(60),
                            rrule: Some("FREQ=DAILY".into()),
                            ..Default::default()
                        })),
                    ],
                    ..Default::default()
                },
                2,
            )
            .unwrap();

        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        assert!(calendar.visible);
        assert_eq!(calendar.event_count, 2);
        assert_eq!(
            store
                .calendar_events_for_range(50, 250, true)
                .unwrap()
                .len(),
            2
        );

        store.set_calendar_visible(calendar.id, false).unwrap();
        assert!(
            store
                .calendar_events_for_range(50, 250, true)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .calendar_events_for_range(50, 250, false)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.calendars_for_account(account).unwrap()[0].event_count,
            2
        );
    }

    #[test]
    fn moved_instance_upserts_by_original_start_not_current_start() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, EventChange};

        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    calendars: vec![CalendarInfo {
                        remote_id: "work".into(),
                        subscribed: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        for (remote_id, start) in [("instance-a", 100), ("instance-b", 500)] {
            store
                .apply_calendar_batch(
                    account,
                    "google",
                    &calendar,
                    &CalendarBatch {
                        changes: vec![EventChange::Upsert(Box::new(CalendarEvent {
                            remote_id: remote_id.into(),
                            recurring_event_id: Some("series".into()),
                            original_start: Some("2026-01-01T09:00:00Z".into()),
                            start_utc: Some(start),
                            ..Default::default()
                        }))],
                        ..Default::default()
                    },
                    2,
                )
                .unwrap();
        }
        let (count, start): (i64, i64) = store
            .with_db(|db| {
                Ok((
                    db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?,
                    db.query_row("SELECT start_utc FROM event", [], |row| row.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(start, 500);
    }

    #[test]
    fn full_resync_clear_is_one_calendar_only() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, EventChange};

        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    calendars: ["a", "b"]
                        .into_iter()
                        .map(|id| CalendarInfo {
                            remote_id: id.into(),
                            subscribed: true,
                            ..Default::default()
                        })
                        .collect(),
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendars = store.calendars_for_account(account).unwrap();
        for calendar in &calendars {
            store
                .apply_calendar_batch(
                    account,
                    "google",
                    calendar,
                    &CalendarBatch {
                        changes: vec![EventChange::Upsert(Box::new(CalendarEvent {
                            remote_id: format!("event-{}", calendar.info.remote_id),
                            ..Default::default()
                        }))],
                        ..Default::default()
                    },
                    2,
                )
                .unwrap();
        }
        store.clear_calendar_for_resync(calendars[0].id).unwrap();
        let remaining: Vec<String> = store
            .with_db(|db| {
                let mut statement = db.prepare("SELECT remote_id FROM event ORDER BY remote_id")?;
                let rows = statement.query_map([], |row| row.get(0))?;
                Ok(rows.collect::<rusqlite::Result<_>>()?)
            })
            .unwrap();
        assert_eq!(remaining, vec!["event-b"]);
    }

    #[test]
    fn cancelled_exception_hides_only_the_instance() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, EventChange};

        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        store
            .apply_calendar_discovery(
                account,
                "google",
                &CalendarBatch {
                    calendars: vec![CalendarInfo {
                        remote_id: "work".into(),
                        subscribed: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        store
            .apply_calendar_batch(
                account,
                "google",
                &calendar,
                &CalendarBatch {
                    changes: vec![
                        EventChange::Upsert(Box::new(CalendarEvent {
                            remote_id: "series".into(),
                            rrule: Some("FREQ=WEEKLY".into()),
                            ..Default::default()
                        })),
                        EventChange::CancelInstance {
                            recurring_event_id: "series".into(),
                            original_start: "2026-01-08T09:00:00Z".into(),
                        },
                    ],
                    ..Default::default()
                },
                2,
            )
            .unwrap();
        let (masters, exceptions): (i64, i64) = store
            .with_db(|db| {
                Ok((
                    db.query_row("SELECT count(*) FROM event", [], |row| row.get(0))?,
                    db.query_row("SELECT count(*) FROM event_exception", [], |row| row.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((masters, exceptions), (1, 1));
    }

    #[test]
    fn editor_children_tasks_and_reminder_delivery_are_durable() {
        use crate::calendar::{CalendarBatch, CalendarEvent, CalendarInfo, CalendarWrite};

        let store = store_with_account();
        let info = CalendarInfo {
            remote_id: "local".into(),
            name: Some("Personal".into()),
            selected: true,
            subscribed: true,
            supports_scheduling: true,
            ..Default::default()
        };
        store
            .apply_calendar_discovery(
                1,
                "local",
                &CalendarBatch {
                    calendars: vec![info.clone()],
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let calendar = store.calendars_for_account(1).unwrap().remove(0);
        let event = CalendarEvent {
            remote_id: "event".into(),
            ical_uid: Some("event@example.com".into()),
            summary: Some("Review".into()),
            start_utc: Some(10_000),
            end_utc: Some(13_600),
            ..Default::default()
        };
        let event_id = store
            .queue_calendar_write(
                1,
                calendar.id,
                &event,
                &CalendarWrite::Put {
                    calendar: info,
                    event: event.clone(),
                    base_etag: None,
                },
                "event:create",
                1,
            )
            .unwrap();
        store
            .replace_event_reminders(
                event_id,
                &[
                    ReminderKind::MinutesBefore(10),
                    ReminderKind::SameDayMinute(8 * 60),
                ],
            )
            .unwrap();
        let reminder = store.reminders_for_event(event_id).unwrap().remove(0);
        store
            .schedule_reminder_delivery(reminder.id, 10_000, 9_400)
            .unwrap();
        store
            .schedule_reminder_delivery(reminder.id, 10_000, 9_400)
            .unwrap();
        assert_eq!(store.due_reminders(9_400, 0, 10).unwrap().len(), 1);
        store
            .mark_reminder_fired(reminder.id, 10_000, 9_400)
            .unwrap();
        assert!(store.due_reminders(20_000, 0, 10).unwrap().is_empty());

        store
            .replace_event_attendees(
                event_id,
                &[AttendeeRow {
                    id: 0,
                    event_id,
                    email: "me@example.com".into(),
                    display_name: Some("Me".into()),
                    role: "required".into(),
                    status: "accepted".into(),
                    is_self: true,
                }],
            )
            .unwrap();
        assert_eq!(
            store.attendees_for_event(event_id).unwrap()[0].status,
            "accepted"
        );

        let task = store
            .insert_task(1, "Submit report", None, Some(12_000), 1)
            .unwrap();
        assert!(!store.tasks_between(0, 20_000).unwrap()[0].done);
        store.set_task_done(task, true).unwrap();
        assert!(store.tasks_between(0, 20_000).unwrap()[0].done);
    }

    #[test]
    fn disabled_mailboxes_are_kept_for_settings_but_hidden_from_mail() {
        let store = store_with_account();
        let inbox = store.ensure_mailbox(1, "Inbox", "inbox").unwrap();
        let sent = store.ensure_mailbox(1, "Sent", "sent").unwrap();
        store.set_mailbox_sync_enabled(sent, false).unwrap();

        assert_eq!(store.mailboxes().unwrap().len(), 2);
        assert_eq!(store.enabled_mailboxes().unwrap()[0].id, inbox);
        assert_eq!(store.enabled_mailbox_kinds(1).unwrap(), vec!["inbox"]);
    }

    #[test]
    fn deleting_an_account_removes_only_its_unshared_cache_objects() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "snail-delete-account-{}-{nonce}",
            std::process::id()
        ));
        let paths = crate::paths::Paths {
            config: root.join("config"),
            cache: root.join("cache"),
            log: root.join("log"),
        };
        let store = Store::open(&paths).unwrap();
        let first = store
            .insert_account("gmail", "first@example.com", None, 0)
            .unwrap();
        let second = store
            .insert_account("gmail", "second@example.com", None, 0)
            .unwrap();
        let unique = store.cache().put(b"first-only").unwrap();
        let shared = store.cache().put(b"shared").unwrap();
        store
            .insert_message(&NewMessage {
                account_id: first,
                raw_hash: Some(unique.clone()),
                body_hash: Some(shared.clone()),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_message(&NewMessage {
                account_id: second,
                raw_hash: Some(shared.clone()),
                ..Default::default()
            })
            .unwrap();

        store.delete_account(first).unwrap();
        assert_eq!(store.cache().get(&unique).unwrap(), None);
        assert_eq!(
            store.cache().get(&shared).unwrap(),
            Some(b"shared".to_vec())
        );
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn advanced_actions_rebuild_search_and_mark_every_account_cursor() {
        let store = store_with_account();
        store
            .set_sync_state(&SyncState {
                account_id: 1,
                kind: "gmail".into(),
                collection: String::new(),
                history_id: Some(42),
                last_sync: Some(10),
                ..Default::default()
            })
            .unwrap();
        store.force_full_resync(1, 20).unwrap();
        let state = store.get_sync_state(1, "gmail", "").unwrap().unwrap();
        assert!(state.full_resync_needed);
        assert_eq!(state.history_id, None);
        assert_eq!(state.last_sync, Some(20));
        store.rebuild_search_index().unwrap();
    }

    #[test]
    fn no_view_module_reaches_the_store() {
        // plan.md E2.4: the store is only reachable from the background executor. A *view* module
        // (the shell, and anything under `views/`) must not name it; models like `mail_model.rs`
        // and the bins's `main.rs` (bootstrap + CLI) may. `mail_model` is where the E5.12 move to
        // the background executor happens.
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../snail/src");
        if !bin.is_dir() {
            return;
        }
        let mut offenders = Vec::new();
        let mut stack = vec![bin.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let is_view = name == "shell.rs" || path.to_string_lossy().contains("/views/");
                if !is_view || !path.extension().is_some_and(|e| e == "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                if text.contains("snail_core::store") || text.contains("Store::open") {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "view modules must not reach the store directly (use a model): {offenders:?}"
        );
    }
}
