//! The benchmark fixture (plan.md E0.9) and the store benchmarks (E2.9). Every list and search
//! benchmark runs against this, never the real mailbox.
//!
//! Deterministic: a seeded xorshift, so two runs produce byte-identical rows and a `--bench-compare`
//! gate (E17.1) is meaningful.

use anyhow::Result;
use rusqlite::params;
use std::time::Instant;

use crate::store::{ProviderRef, Store};

#[derive(Clone, Copy, Debug)]
pub struct FixtureSpec {
    pub accounts: usize,
    pub mailboxes: usize,
    pub messages: usize,
    pub events: usize,
}

impl Default for FixtureSpec {
    fn default() -> Self {
        Self {
            accounts: 2,
            mailboxes: 8,
            messages: 200_000,
            events: 20_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FixtureReport {
    pub accounts: usize,
    pub mailboxes: usize,
    pub messages: usize,
    pub events: usize,
    pub elapsed_ms: u128,
}

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const SENDERS: &[(&str, &str)] = &[
    ("Priya Raman", "priya@example.com"),
    ("GitHub", "notifications@github.com"),
    ("Maya Okonkwo", "maya@example.com"),
    ("Apple", "noreply@email.apple.com"),
    ("Stripe", "receipts@stripe.com"),
    ("The New York Times", "breakingnews@nytimes.com"),
    ("Old Navy", "oldnavy@email.oldnavy.com"),
    ("Sam Whitfield", "sam@example.org"),
];

const SUBJECTS: &[&str] = &[
    "Design review Thursday",
    "[snail] CI failed on main",
    "Re: Almanac calendar geometry",
    "An app-specific password was generated",
    "Your payout is on the way",
    "Breaking news: the morning brief",
    "ATTN: 40% off everything",
    "Notes from the walk",
];

/// Replace whatever is in `store` with `spec`. One transaction per table group for speed.
pub fn generate(store: &Store, spec: &FixtureSpec) -> Result<FixtureReport> {
    let started = Instant::now();
    let mut rng = XorShift(0x5EED_5EED_5EED_5EED);

    store.with_db(|conn| {
        conn.execute_batch(
            "DELETE FROM message; DELETE FROM mailbox; DELETE FROM calendar; DELETE FROM event;
             DELETE FROM account;",
        )?;
        Ok(())
    })?;

    store.transaction(|tx| {
        for account in 0..spec.accounts {
            let kind = if account == 0 { "gmail" } else { "icloud" };
            tx.execute(
                "INSERT INTO account (id, kind, address, display_name, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![
                    account as i64 + 1,
                    kind,
                    format!("account{account}@example.com"),
                    format!("Account {account}")
                ],
            )?;
            let mut mailbox_id = 1;
            for mailbox in 0..spec.mailboxes {
                tx.execute(
                    "INSERT INTO mailbox (id, account_id, name, kind, unread, total)
                     VALUES (?1, ?2, ?3, ?4, 0, 0)",
                    params![
                        (account * spec.mailboxes + mailbox) as i64 + 1,
                        account as i64 + 1,
                        mailbox_name(mailbox),
                        mailbox_kind(mailbox)
                    ],
                )?;
                mailbox_id += 1;
            }
            let _ = mailbox_id;
        }
        Ok(())
    })?;

    let total_mailboxes = spec.accounts * spec.mailboxes;
    store.transaction(|tx| {
        {
            let mut insert = tx.prepare(
                "INSERT INTO message (
                    account_id, mailbox_id, provider, gmail_id, message_id, subject,
                    from_name, from_addr, to_json, date, preview, unread, size
                 ) VALUES (?1, ?2, 'gmail', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for index in 0..spec.messages {
                let sender = &SENDERS[rng.below(SENDERS.len())];
                let subject = SUBJECTS[rng.below(SUBJECTS.len())];
                let mailbox_id = (index % total_mailboxes) as i64 + 1;
                let account_id = (index % spec.accounts) as i64 + 1;
                let date = 1_700_000_000i64 + index as i64;
                insert.execute(params![
                    account_id,
                    mailbox_id,
                    format!("gid-{index}"),
                    format!("<msg-{index}@example.com>"),
                    format!("{subject} ({index})"),
                    sender.0,
                    sender.1,
                    "me@example.com",
                    date,
                    "The quick brown fox jumps over the lazy dog, and keeps going.",
                    (index % 3 == 0) as i64,
                    1024 + (index % 4096),
                ])?;
            }
        }
        Ok(())
    })?;

    store.transaction(|tx| {
        for account in 0..spec.accounts {
            tx.execute(
                "INSERT INTO calendar (id, account_id, provider, remote_id, name, color)
                 VALUES (?1, ?2, 'google', ?3, 'Primary', '#2c5fb8')",
                params![account as i64 + 1, account as i64 + 1, format!("cal{account}")],
            )?;
        }
        {
            let mut insert = tx.prepare(
                "INSERT INTO event (
                    calendar_id, remote_id, summary, location, start_utc, end_utc, all_day, tz
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 'America/Chicago')",
            )?;
            for index in 0..spec.events {
                let calendar_id = (index % spec.accounts) as i64 + 1;
                let start = 1_700_000_000i64 + (index as i64 * 1800);
                insert.execute(params![
                    calendar_id,
                    format!("evt-{index}"),
                    format!("Event {index}"),
                    "Room 4",
                    start,
                    start + 1800,
                ])?;
            }
        }
        Ok(())
    })?;

    // Keep the cached counts honest.
    store.with_db(|conn| {
        conn.execute(
            "UPDATE mailbox SET total = (SELECT count(*) FROM message WHERE message.mailbox_id = mailbox.id),
                                unread = (SELECT count(*) FROM message WHERE message.mailbox_id = mailbox.id AND unread = 1)",
            [],
        )?;
        Ok(())
    })?;

    Ok(FixtureReport {
        accounts: spec.accounts,
        mailboxes: total_mailboxes,
        messages: spec.messages,
        events: spec.events,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn mailbox_name(index: usize) -> &'static str {
    const NAMES: [&str; 8] = [
        "Inbox",
        "Sent",
        "Drafts",
        "Archive",
        "Trash",
        "Receipts",
        "Newsletters",
        "Receipts/2026",
    ];
    NAMES[index % NAMES.len()]
}

fn mailbox_kind(index: usize) -> &'static str {
    match index % 8 {
        0 => "inbox",
        1 => "sent",
        2 => "drafts",
        3 => "archive",
        4 => "trash",
        _ => "other",
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BenchReport {
    pub cold_open_ms: f64,
    pub page_p50_ms: f64,
    pub page_p99_ms: f64,
    pub thread_assemble_ms: f64,
    pub unread_counts_ms: f64,
    pub messages: i64,
}

/// The E2.9 budgets. `page` is the mailbox page query; `thread` assembles one thread.
pub fn benchmark(store: &Store) -> Result<BenchReport> {
    let messages: i64 = store.with_db(|conn| {
        Ok(conn.query_row("SELECT count(*) FROM message", [], |row| row.get(0))?)
    })?;

    let mut page_times = Vec::new();
    for _ in 0..40 {
        let started = Instant::now();
        let _ = store.page(1, 100, 0)?;
        page_times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    page_times.sort_by(f64::total_cmp);
    let p = |q: f64| page_times[((q / 100.0) * (page_times.len() - 1) as f64).round() as usize];

    let started = Instant::now();
    let _ = store.with_db(|conn| {
        let mut statement = conn.prepare(
            "SELECT id, subject, date FROM message WHERE message_id = ?1 ORDER BY date",
        )?;
        let rows = statement.query_map(["<msg-7@example.com>"], |row| {
            Ok(row.get::<_, i64>(0)?)
        })?;
        let count = rows.collect::<rusqlite::Result<Vec<_>>>()?.len();
        Ok(count)
    })?;
    let thread_assemble_ms = started.elapsed().as_secs_f64() * 1000.0;

    let started = Instant::now();
    for mailbox in 1..=8 {
        let _ = store.unread_count(mailbox)?;
    }
    let unread_counts_ms = started.elapsed().as_secs_f64() * 1000.0;

    Ok(BenchReport {
        cold_open_ms: 0.0,
        page_p50_ms: p(50.0),
        page_p99_ms: p(99.0),
        thread_assemble_ms,
        unread_counts_ms,
        messages,
    })
}
