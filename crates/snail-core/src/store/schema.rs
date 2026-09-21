//! Schema v1 and the migrations that follow it (plan.md E2.1). One `CREATE TABLE IF NOT EXISTS`
//! batch plus a version row in `meta`, exactly as the reference project does it.
//!
//! Migrations are append-only: a new version is a new entry, never an edit to an old one, so a
//! store written by any past release can be walked forward. `migrate` can stop at a target version,
//! which is how the v1→v2 test builds a genuine v1 store.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// `(name, sql)`, applied in order; index + 1 is the version it produces.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("v1", V1),
    ("v2", V2),
    ("v3", V3),
    ("v4", V4),
    ("v5", V5),
    ("v6", V6),
    ("v7", V7),
    ("v8", V8),
    ("v9", V9),
];

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

/// v4 adds full-text search (E9.1): a `body_text` column and an external-content FTS5 index over
/// subject, sender, recipients and body.
///
/// The delete trigger passes the old column values **explicitly**. That is the whole trick that
/// makes FTS5 coexist with v1's `ON DELETE CASCADE`: on a cascade the `message` row is already gone
/// when the trigger fires, so an external-content delete that tried to read it back would fail. The
/// values come from `old` instead.
const V4: &str = r#"
ALTER TABLE message ADD COLUMN body_text TEXT;

CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    subject, from_name, from_addr, to_json, cc_json, body_text,
    content = 'message',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS message_fts_ai AFTER INSERT ON message BEGIN
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, to_json, cc_json, body_text)
    VALUES (new.id, new.subject, new.from_name, new.from_addr, new.to_json, new.cc_json, new.body_text);
END;

CREATE TRIGGER IF NOT EXISTS message_fts_ad AFTER DELETE ON message BEGIN
    INSERT INTO message_fts (message_fts, rowid, subject, from_name, from_addr, to_json, cc_json, body_text)
    VALUES ('delete', old.id, old.subject, old.from_name, old.from_addr, old.to_json, old.cc_json, old.body_text);
END;

CREATE TRIGGER IF NOT EXISTS message_fts_au AFTER UPDATE OF subject, from_name, from_addr, to_json, cc_json, body_text ON message BEGIN
    INSERT INTO message_fts (message_fts, rowid, subject, from_name, from_addr, to_json, cc_json, body_text)
    VALUES ('delete', old.id, old.subject, old.from_name, old.from_addr, old.to_json, old.cc_json, old.body_text);
    INSERT INTO message_fts (rowid, subject, from_name, from_addr, to_json, cc_json, body_text)
    VALUES (new.id, new.subject, new.from_name, new.from_addr, new.to_json, new.cc_json, new.body_text);
END;

-- Index whatever is already on disk when this runs on an existing store (a no-op on a fresh one).
INSERT INTO message_fts (message_fts) VALUES ('rebuild');
"#;

/// v5 adds the provider-neutral calendar sync state required by E11. Server canonical bytes are
/// distinct from optimistic local serialization so CalDAV normalization and 412 conflicts cannot
/// silently clobber one side.
const V5: &str = r#"
ALTER TABLE calendar ADD COLUMN href TEXT;
ALTER TABLE calendar ADD COLUMN access_role TEXT;
ALTER TABLE calendar ADD COLUMN subscribed INTEGER NOT NULL DEFAULT 1;
ALTER TABLE calendar ADD COLUMN ctag TEXT;
ALTER TABLE calendar ADD COLUMN supports_sync_collection INTEGER NOT NULL DEFAULT 0;

ALTER TABLE event ADD COLUMN raw_ical TEXT;
ALTER TABLE event ADD COLUMN server_ical TEXT;
ALTER TABLE event ADD COLUMN conflict_json TEXT;

CREATE INDEX IF NOT EXISTS event_by_ical_uid ON event (ical_uid);
CREATE UNIQUE INDEX IF NOT EXISTS event_by_instance_identity
ON event (calendar_id, recurring_event_id, original_start)
WHERE recurring_event_id IS NOT NULL AND original_start IS NOT NULL;
"#;

/// v6 separates provider `selected` metadata from Snail's immediate per-calendar visibility toggle.
const V6: &str = r#"
ALTER TABLE calendar ADD COLUMN visible INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS event_by_calendar_range
ON event (calendar_id, start_utc, end_utc);
"#;

/// v7 makes the event editor's reminders, attendees, recurrence exceptions and local tasks durable.
/// Reminder delivery is normalized separately so a recurring event may fire once per occurrence
/// without the scheduler ever needing to mutate the reminder definition itself.
const V7: &str = r#"
ALTER TABLE reminder ADD COLUMN same_day_minute INTEGER;

CREATE TABLE IF NOT EXISTS reminder_delivery (
    reminder_id      INTEGER NOT NULL REFERENCES reminder(id) ON DELETE CASCADE,
    occurrence_start INTEGER NOT NULL,
    due_utc          INTEGER NOT NULL,
    fired_at         INTEGER,
    PRIMARY KEY (reminder_id, occurrence_start)
);
CREATE INDEX IF NOT EXISTS reminder_delivery_due
ON reminder_delivery (fired_at, due_utc);

CREATE TABLE IF NOT EXISTS attendee (
    id           INTEGER PRIMARY KEY,
    event_id     INTEGER NOT NULL REFERENCES event(id) ON DELETE CASCADE,
    email        TEXT    NOT NULL,
    display_name TEXT,
    role         TEXT    NOT NULL DEFAULT 'required',
    status       TEXT    NOT NULL DEFAULT 'needs-action',
    is_self      INTEGER NOT NULL DEFAULT 0,
    UNIQUE (event_id, email)
);

CREATE INDEX IF NOT EXISTS task_by_due ON task (due_utc, done);
"#;

/// v8 persists the RFC 6638 capability probe used to gate CalDAV RSVP writes.
const V8: &str = r#"
ALTER TABLE calendar ADD COLUMN supports_scheduling INTEGER NOT NULL DEFAULT 0;
"#;

/// v9 persists the per-mailbox choices made before an account's first full pull and later from
/// Settings → Accounts.
const V9: &str = r#"
ALTER TABLE mailbox ADD COLUMN sync_enabled INTEGER NOT NULL DEFAULT 1;
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

    #[test]
    fn the_search_index_tracks_inserts_and_cascades() {
        let mut conn = memory();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        migrate(&mut conn, None).unwrap();
        conn.execute(
            "INSERT INTO account (id, kind, address, created_at) VALUES (1, 'gmail', 'a@b.c', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, account_id, provider, subject, body_text)
             VALUES (1, 1, 'gmail', 'Quarterly numbers', 'the almanac geometry checks out')",
            [],
        )
        .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH 'almanac'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1, "the insert trigger indexed the body");

        // The interesting case: the FTS delete trigger must survive a foreign-key cascade, when the
        // message row is already gone and cannot be read back from the content table.
        conn.execute("DELETE FROM account WHERE id = 1", [])
            .unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH 'almanac'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 0, "the cascade removed the index row too");
    }

    #[test]
    fn migrating_an_existing_store_backfills_the_index() {
        let mut conn = memory();
        migrate(&mut conn, Some(3)).unwrap();
        conn.execute(
            "INSERT INTO account (id, kind, address, created_at) VALUES (1, 'gmail', 'a@b.c', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message (id, account_id, provider, subject) VALUES (1, 1, 'gmail', 'legacy subject')",
            [],
        )
        .unwrap();

        migrate(&mut conn, Some(4)).unwrap();
        let hits: i64 = conn
            .query_row(
                "SELECT count(*) FROM message_fts WHERE message_fts MATCH 'legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            hits, 1,
            "the rebuild indexed a row that predates the migration"
        );
    }

    #[test]
    fn mailbox_sync_selection_defaults_on_for_existing_rows() {
        let mut conn = memory();
        migrate(&mut conn, Some(8)).unwrap();
        conn.execute(
            "INSERT INTO account (id, kind, address, created_at) VALUES (1, 'gmail', 'a@b.c', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO mailbox (id, account_id, name, kind) VALUES (1, 1, 'Inbox', 'inbox')",
            [],
        )
        .unwrap();

        migrate(&mut conn, Some(9)).unwrap();
        let enabled: i64 = conn
            .query_row("SELECT sync_enabled FROM mailbox WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(enabled, 1);
    }
}
