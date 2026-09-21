//! Schema v1 and the migrations that follow it (plan.md E2.1). One `CREATE TABLE IF NOT EXISTS`
//! batch plus a version row in `meta`, exactly as the reference project does it.
//!
//! Migrations are append-only: a new version is a new entry, never an edit to an old one, so a
//! store written by any past release can be walked forward. `migrate` can stop at a target version,
//! which is how the v1→v2 test builds a genuine v1 store.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// `(name, sql)`, applied in order; index + 1 is the version it produces.
pub const MIGRATIONS: &[(&str, &str)] = &[("v1", V1), ("v2", V2), ("v3", V3)];

/// The version a fresh store ends at.
pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS account (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,
    address      TEXT    NOT NULL,
    display_name TEXT,
    created_at   INTEGER NOT NULL,
    UNIQUE (kind, address)
);

CREATE TABLE IF NOT EXISTS identity (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    address      TEXT    NOT NULL,
    display_name TEXT,
    signature    TEXT,
    is_default   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS mailbox (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL DEFAULT 'other',
    unread     INTEGER NOT NULL DEFAULT 0,
    total      INTEGER NOT NULL DEFAULT 0,
    uidvalidity INTEGER,
    UNIQUE (account_id, name)
);

CREATE TABLE IF NOT EXISTS thread (
    id                INTEGER PRIMARY KEY,
    account_id        INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    provider_thread_id TEXT,
    subject           TEXT,
    last_date         INTEGER
);

CREATE TABLE IF NOT EXISTS message (
    id                 INTEGER PRIMARY KEY,
    account_id         INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    mailbox_id         INTEGER REFERENCES mailbox(id) ON DELETE SET NULL,
    thread_id          INTEGER REFERENCES thread(id) ON DELETE SET NULL,
    -- provider identity lives here, never leaks upward (E2.2)
    provider           TEXT    NOT NULL,
    gmail_id           TEXT,
    gmail_thread_id    TEXT,
    gmail_history_id   INTEGER,
    imap_uid           INTEGER,
    imap_uidvalidity   INTEGER,
    imap_modseq        INTEGER,
    -- threading headers, stored for every account kind
    message_id         TEXT,
    in_reply_to        TEXT,
    references_header  TEXT,
    subject            TEXT,
    from_name          TEXT,
    from_addr          TEXT,
    to_json            TEXT,
    cc_json            TEXT,
    bcc_json           TEXT,
    date               INTEGER,
    internal_date      INTEGER,
    size               INTEGER,
    has_attachments    INTEGER NOT NULL DEFAULT 0,
    unread             INTEGER NOT NULL DEFAULT 1,
    starred            INTEGER NOT NULL DEFAULT 0,
    flagged            INTEGER NOT NULL DEFAULT 0,
    preview            TEXT,
    body_hash          TEXT,
    raw_hash           TEXT,
    labels_json        TEXT,
    UNIQUE (account_id, provider, gmail_id),
    UNIQUE (account_id, provider, imap_uidvalidity, imap_uid)
);

CREATE INDEX IF NOT EXISTS message_by_mailbox ON message (mailbox_id, date DESC);
CREATE INDEX IF NOT EXISTS message_by_thread ON message (thread_id, date);
CREATE INDEX IF NOT EXISTS message_by_message_id ON message (message_id);

CREATE TABLE IF NOT EXISTS message_part (
    id           INTEGER PRIMARY KEY,
    message_id   INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    content_type TEXT,
    filename     TEXT,
    disposition  TEXT,
    content_id   TEXT,
    size         INTEGER,
    hash         TEXT
);

CREATE TABLE IF NOT EXISTS attachment (
    id           INTEGER PRIMARY KEY,
    message_id   INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    filename     TEXT,
    content_type TEXT,
    size         INTEGER,
    content_id   TEXT,
    hash         TEXT
);

CREATE TABLE IF NOT EXISTS label (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    color      TEXT,
    UNIQUE (account_id, name)
);

CREATE TABLE IF NOT EXISTS message_label (
    message_id INTEGER NOT NULL REFERENCES message(id) ON DELETE CASCADE,
    label_id   INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, label_id)
);

-- Every local mutation is a row here, in the same transaction as the optimistic change (E2.5).
CREATE TABLE IF NOT EXISTS pending_op (
    id              INTEGER PRIMARY KEY,
    account_id      INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    target_kind     TEXT    NOT NULL,
    target_id       INTEGER,
    operation       TEXT    NOT NULL,
    payload_json    TEXT,
    idempotency_key TEXT    NOT NULL UNIQUE,
    attempts        INTEGER NOT NULL DEFAULT 0,
    state           TEXT    NOT NULL DEFAULT 'pending',
    last_error      TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS pending_op_by_state ON pending_op (state, created_at);

-- Per (account, kind, collection) cursor; every provider kind can invalidate its cursor and
-- every one of them has a first-class full-resync recovery (E2.6).
CREATE TABLE IF NOT EXISTS sync_state (
    account_id         INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    kind               TEXT    NOT NULL,
    collection         TEXT    NOT NULL DEFAULT '',
    cursor_json        TEXT,
    history_id         INTEGER,
    sync_token         TEXT,
    uidvalidity        INTEGER,
    highest_modseq     INTEGER,
    ctag               TEXT,
    full_resync_needed INTEGER NOT NULL DEFAULT 0,
    last_sync          INTEGER,
    PRIMARY KEY (account_id, kind, collection)
);

CREATE TABLE IF NOT EXISTS calendar (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    provider   TEXT    NOT NULL,
    remote_id  TEXT    NOT NULL,
    name       TEXT,
    color      TEXT,
    selected   INTEGER NOT NULL DEFAULT 1,
    read_only  INTEGER NOT NULL DEFAULT 0,
    UNIQUE (account_id, provider, remote_id)
);

CREATE TABLE IF NOT EXISTS event (
    id                INTEGER PRIMARY KEY,
    calendar_id       INTEGER NOT NULL REFERENCES calendar(id) ON DELETE CASCADE,
    remote_id         TEXT,
    href              TEXT,
    etag              TEXT,
    ical_uid          TEXT,
    recurring_event_id TEXT,
    original_start    TEXT,
    summary           TEXT,
    location          TEXT,
    description       TEXT,
    start_utc         INTEGER,
    end_utc           INTEGER,
    all_day           INTEGER NOT NULL DEFAULT 0,
    tz                TEXT,
    rrule             TEXT,
    rdate             TEXT,
    exdate            TEXT,
    status            TEXT,
    updated           INTEGER,
    sequence          INTEGER,
    UNIQUE (calendar_id, remote_id)
);

CREATE INDEX IF NOT EXISTS event_by_range ON event (start_utc, end_utc);

CREATE TABLE IF NOT EXISTS event_exception (
    id             INTEGER PRIMARY KEY,
    event_id       INTEGER NOT NULL REFERENCES event(id) ON DELETE CASCADE,
    original_start TEXT    NOT NULL,
    is_cancelled   INTEGER NOT NULL DEFAULT 0,
    start_utc      INTEGER,
    end_utc        INTEGER,
    summary        TEXT,
    UNIQUE (event_id, original_start)
);

CREATE TABLE IF NOT EXISTS reminder (
    id             INTEGER PRIMARY KEY,
    event_id       INTEGER NOT NULL REFERENCES event(id) ON DELETE CASCADE,
    minutes_before INTEGER,
    absolute_utc   INTEGER,
    fired          INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS task (
    id         INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    title      TEXT    NOT NULL,
    notes      TEXT,
    due_utc    INTEGER,
    done       INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL
);
"#;

/// v2 adds a covering index for the sender facet. FTS5 (E9.1) arrives as a later migration, where
/// it can be designed on its own; putting it in v2 tempts trigger-based maintenance that conflicts
/// with the foreign-key cascades v1 already relies on.
const V2: &str = r#"
CREATE INDEX IF NOT EXISTS message_by_from ON message (from_addr, date DESC);
"#;

/// v3 adds harvested contacts, for address autocomplete without any contacts API (E7.3).
const V3: &str = r#"
CREATE TABLE IF NOT EXISTS contact (
    account_id INTEGER NOT NULL REFERENCES account(id) ON DELETE CASCADE,
    address    TEXT    NOT NULL,
    name       TEXT,
    sent       INTEGER NOT NULL DEFAULT 0,
    received   INTEGER NOT NULL DEFAULT 0,
    last_seen  INTEGER,
    PRIMARY KEY (account_id, address)
);

CREATE INDEX IF NOT EXISTS contact_by_last_seen ON contact (account_id, last_seen DESC);
"#;

/// The schema version currently recorded, or 0 for an empty store.
pub fn version(conn: &Connection) -> Result<u32> {
    use rusqlite::OptionalExtension as _;
    // On a fresh database the table itself is absent — that is version 0, not an error.
    let has_meta: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if has_meta.is_none() {
        return Ok(0);
    }
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|value| value.parse().ok()).unwrap_or(0))
}

/// Apply migrations up to `target` (default: latest). Returns the resulting version.
pub fn migrate(conn: &mut Connection, target: Option<u32>) -> Result<u32> {
    let target = target.unwrap_or_else(latest_version).min(latest_version());
    let mut current = version(conn)?;
    while current < target {
        let (name, sql) = MIGRATIONS[current as usize];
        let tx = conn.transaction()?;
        tx.execute_batch(sql)
            .with_context(|| format!("migration {name}"))?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [format!("{}", current + 1)],
        )?;
        tx.commit()?;
        current += 1;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        Connection::open_in_memory().expect("in-memory sqlite")
    }

    #[test]
    fn a_fresh_store_reaches_latest() {
        let mut conn = memory();
        assert_eq!(version(&conn).unwrap(), 0);
        let v = migrate(&mut conn, None).unwrap();
        assert_eq!(v, latest_version());
        assert_eq!(version(&conn).unwrap(), v);
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut conn = memory();
        migrate(&mut conn, None).unwrap();
        migrate(&mut conn, None).unwrap();
        assert_eq!(version(&conn).unwrap(), latest_version());
    }

    #[test]
    fn v1_to_v2_preserves_rows_and_adds_the_index() {
        let mut conn = memory();
        migrate(&mut conn, Some(1)).unwrap();
        assert_eq!(version(&conn).unwrap(), 1);

        conn.execute(
            "INSERT INTO account (kind, address, created_at) VALUES ('gmail', 'a@b.c', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (account_id, provider, subject, from_addr, to_json)
             VALUES (1, 'gmail', 'Quarterly numbers', 'boss@example.com', 'me@example.com')",
            [],
        )
        .unwrap();

        // Walking v1→v2 adds the sender index without touching the existing row.
        migrate(&mut conn, Some(2)).unwrap();
        assert_eq!(version(&conn).unwrap(), 2);
        let index: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'message_by_from'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index, 1);
        let messages: i64 = conn
            .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(messages, 1, "the v1 row survives the migration");
    }

    #[test]
    fn foreign_keys_cascade() {
        let mut conn = memory();
        migrate(&mut conn, None).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute(
            "INSERT INTO account (id, kind, address, created_at) VALUES (1, 'gmail', 'a@b.c', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, account_id, provider, subject) VALUES (1, 1, 'gmail', 'x')",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM account WHERE id = 1", [])
            .unwrap();
        let left: i64 = conn
            .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "deleting an account removes its messages");
    }
}
