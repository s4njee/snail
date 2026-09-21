//! The local store (plan.md E2): one `rusqlite` connection behind a `Mutex`, WAL, touched only from
//! the background executor. Every read is served from here, so offline is the normal case.

pub mod schema;

#[cfg(test)]
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
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
    pub body_hash: Option<String>,
    pub raw_hash: Option<String>,
    pub labels_json: Option<String>,
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
                    preview, unread, body_hash, raw_hash, labels_json
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23
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
                    message.body_hash,
                    message.raw_hash,
                    message.labels_json,
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
            let (state, payload_json, target_id): (String, Option<String>, Option<i64>) = tx.query_row(
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
            let account_id: i64 = tx.query_row(
                "SELECT account_id FROM message WHERE id = ?1",
                [message_id],
                |row| row.get(0),
            )?;

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

            let inverse = payload
                .action
                .toggle_inverse()
                .or_else(|| {
                    payload.previous_mailbox.clone().map(|mailbox| TriageAction::Move { mailbox })
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
    pub fn message(&self, message_id: i64) -> Result<Option<MessageRow>> {        self.with_db(|conn| {
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
            conn.execute("DELETE FROM contact WHERE account_id = ?1", params![account_id])?;
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
            let mut contacts = std::collections::BTreeMap::<String, (Option<String>, i64, i64, i64)>::new();
            for row in rows {
                let (name, from, to_json, date) = row?;
                if let Some(address) = from.filter(|address| !address.is_empty()) {
                    let entry = contacts.entry(address).or_default();
                    entry.0 = entry.0.take().or(name);
                    entry.2 += 1;
                    entry.3 = entry.3.max(date.unwrap_or(0));
                }
                if let Some(to_json) = to_json {
                    if let Ok(recipients) = serde_json::from_str::<Vec<crate::mime::Recipient>>(&to_json) {
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
    pub fn suggest_contacts(&self, account_id: i64, prefix: &str, limit: u32) -> Result<Vec<ContactRow>> {
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

    /// Remove an account and everything that cascades from it (E14.9).
    pub fn delete_account(&self, account_id: i64) -> Result<()> {
        self.with_db(|conn| {
            conn.execute("DELETE FROM account WHERE id = ?1", [account_id])?;
            Ok(())
        })
    }
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
        assert!(store.suggest_contacts(1, "me@", 5).unwrap().iter().any(|c| c.address == "me@example.com"));
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
        assert_eq!(store.page(archive, 10, 0).unwrap().len(), 1, "moved locally");
        assert_eq!(store.pending_ops(10).unwrap().len(), 1, "queued remotely");

        // Undo before dispatch: back to Inbox, op cancelled (not retried).
        assert!(!store.undo_triage(op, 1).unwrap());
        assert_eq!(store.page(inbox, 10, 0).unwrap().len(), 1);
        assert!(store.pending_ops(10).unwrap().is_empty(), "cancelled op is not pending");
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
        assert!(store.message(message).unwrap().unwrap().unread, "flag restored");
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
        assert_eq!(page[0].thread_id, -solo, "keyed by -id so it never collides");
        assert!(page.iter().any(|row| row.newest_id == threaded));
        // The threadless message is not dropped from the grouped list.
        assert!(page.iter().any(|row| row.newest_id == solo));
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
