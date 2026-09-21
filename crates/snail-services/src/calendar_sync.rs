//! Provider-neutral calendar sync orchestration (plan.md E11.3, E11.7, E11.21).
//!
//! Local writes go up first. Pull batches are committed atomically with their terminal cursor, and
//! a Google 410 clears/reloads exactly one calendar rather than the entire account.

use anyhow::{Context, Result};

use snail_core::calendar::{
    CalendarBatch, CalendarCursor, CalendarProvider, CalendarWrite, EventChange, WriteOutcome,
};
use snail_core::store::Store;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalendarSyncReport {
    pub calendars: usize,
    pub upserted: usize,
    pub deleted: usize,
    pub conflicts: usize,
    pub pushed: usize,
    pub full_resyncs: usize,
}

impl CalendarSyncReport {
    pub fn changed(&self) -> bool {
        self.upserted + self.deleted + self.conflicts + self.pushed + self.full_resyncs > 0
    }
}

pub fn sync_account(
    store: &Store,
    account_id: i64,
    provider_name: &str,
    provider: &dyn CalendarProvider,
    now: i64,
) -> Result<CalendarSyncReport> {
    sync_account_with_power(
        store,
        account_id,
        provider_name,
        provider,
        now,
        system_on_battery(),
    )
}

fn system_on_battery() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .ok()
            .is_some_and(|output| String::from_utf8_lossy(&output.stdout).contains("Battery Power"))
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
            return false;
        };
        let mut saw_ac = false;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with("ac") || name.contains("mains") {
                saw_ac = true;
                if std::fs::read_to_string(entry.path().join("online"))
                    .ok()
                    .is_some_and(|value| value.trim() == "1")
                {
                    return false;
                }
            }
        }
        return saw_ac;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    false
}

pub fn sync_account_with_power(
    store: &Store,
    account_id: i64,
    provider_name: &str,
    provider: &dyn CalendarProvider,
    now: i64,
    on_battery: bool,
) -> Result<CalendarSyncReport> {
    let mut report = CalendarSyncReport::default();
    push_local_writes(store, account_id, provider, now, &mut report)?;

    let list_kind = format!("calendar-list:{provider_name}");
    let list_state = store.get_sync_state(account_id, &list_kind, "")?;
    let jitter = ((account_id.unsigned_abs().wrapping_mul(7919) % 1000) as f64) / 999.0;
    let cadence = snail_core::calendar::poll_interval_secs(600, jitter, on_battery) as i64;
    if list_state
        .as_ref()
        .and_then(|state| state.last_sync)
        .is_some_and(|last| now.saturating_sub(last) < cadence)
    {
        return Ok(report);
    }
    let mut full_discovery = list_state
        .as_ref()
        .and_then(|state| state.sync_token.as_ref())
        .is_none();
    let mut discovery = provider.discover_calendars(
        list_state
            .as_ref()
            .and_then(|state| state.sync_token.as_deref()),
    )?;
    if discovery.full_resync_required {
        discovery = provider.discover_calendars(None)?;
        full_discovery = true;
    }
    if full_discovery {
        let present: std::collections::HashSet<&str> = discovery
            .calendars
            .iter()
            .map(|calendar| calendar.remote_id.as_str())
            .collect();
        for existing in store.calendars_for_account(account_id)? {
            if existing.provider == provider_name
                && existing.info.subscribed
                && !present.contains(existing.info.remote_id.as_str())
            {
                discovery.unsubscribed.push(existing.info.remote_id);
            }
        }
    }
    store.apply_calendar_discovery(account_id, provider_name, &discovery, now)?;

    for calendar in store
        .calendars_for_account(account_id)?
        .into_iter()
        .filter(|calendar| {
            calendar.provider == provider_name && calendar.info.subscribed && calendar.visible
        })
    {
        report.calendars += 1;
        let cursor = store.calendar_cursor(account_id, provider_name, &calendar)?;
        let mut batch = provider.sync_calendar(&calendar.info, &cursor)?;
        if batch.full_resync_required {
            // Record invalidation first, then wipe and repopulate only this collection.
            store.apply_calendar_batch(account_id, provider_name, &calendar, &batch, now)?;
            store.clear_calendar_for_resync(calendar.id)?;
            batch = provider.sync_calendar(&calendar.info, &CalendarCursor::default())?;
            if batch.full_resync_required {
                anyhow::bail!("provider invalidated a fresh full calendar sync");
            }
            report.full_resyncs += 1;
        }
        count_changes(&batch, &mut report);
        store.apply_calendar_batch(account_id, provider_name, &calendar, &batch, now)?;
    }
    Ok(report)
}

fn push_local_writes(
    store: &Store,
    account_id: i64,
    provider: &dyn CalendarProvider,
    now: i64,
    report: &mut CalendarSyncReport,
) -> Result<()> {
    let ops: Vec<_> = store
        .settled_ops(account_id, i64::MAX, 200)?
        .into_iter()
        .filter(|op| {
            op.target_kind == "event"
                && matches!(op.operation.as_str(), "calendar_put" | "calendar_delete")
        })
        .collect();
    for op in ops {
        let write: CalendarWrite = serde_json::from_str(
            op.payload_json
                .as_deref()
                .context("calendar op has no payload")?,
        )?;
        match provider.apply(std::slice::from_ref(&write)) {
            Ok(outcomes) => {
                let outcome = outcomes
                    .into_iter()
                    .next()
                    .context("provider returned no outcome")?;
                match outcome {
                    WriteOutcome::Conflict { server, .. } => {
                        if let Some(event_id) = op.target_id {
                            store.record_calendar_conflict(event_id, server.as_deref())?;
                        }
                        store.set_op_state(op.id, "conflict", Some("remote event changed"), now)?;
                        report.conflicts += 1;
                    }
                    WriteOutcome::Applied(event) => {
                        if let Some(event_id) = op.target_id {
                            store.accept_calendar_write(event_id, &event)?;
                        }
                        store.set_op_state(op.id, "done", None, now)?;
                        report.pushed += 1;
                    }
                    WriteOutcome::Deleted(_) => {
                        if let Some(event_id) = op.target_id {
                            store.delete_calendar_event(event_id)?;
                        }
                        store.set_op_state(op.id, "done", None, now)?;
                        report.pushed += 1;
                    }
                }
            }
            Err(error) => {
                store.set_op_state(op.id, "failed", Some(&format!("{error:#}")), now)?;
                return Err(error);
            }
        }
    }
    Ok(())
}

fn count_changes(batch: &CalendarBatch, report: &mut CalendarSyncReport) {
    for change in &batch.changes {
        match change {
            EventChange::Upsert(_) => report.upserted += 1,
            EventChange::Delete(_) | EventChange::CancelInstance { .. } => report.deleted += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use snail_core::calendar::{CalendarEvent, CalendarInfo, EventIdentity};
    use snail_core::store::NewIdentity;

    use super::*;

    struct Fake {
        full_once: Mutex<bool>,
        conflict: bool,
    }

    impl CalendarProvider for Fake {
        fn discover_calendars(&self, _cursor: Option<&str>) -> Result<CalendarBatch> {
            Ok(CalendarBatch {
                calendars: vec![CalendarInfo {
                    remote_id: "work".into(),
                    name: Some("Work".into()),
                    selected: true,
                    subscribed: true,
                    access_role: Some("writer".into()),
                    ..Default::default()
                }],
                next_sync_token: Some("list-token".into()),
                ..Default::default()
            })
        }

        fn sync_calendar(
            &self,
            _calendar: &CalendarInfo,
            cursor: &CalendarCursor,
        ) -> Result<CalendarBatch> {
            if cursor.sync_token.is_some()
                && std::mem::replace(&mut *self.full_once.lock().unwrap(), false)
            {
                return Ok(CalendarBatch {
                    full_resync_required: true,
                    ..Default::default()
                });
            }
            Ok(CalendarBatch {
                changes: vec![EventChange::Upsert(Box::new(CalendarEvent {
                    remote_id: "event-1".into(),
                    ical_uid: Some("event@example.com".into()),
                    summary: Some("Planning".into()),
                    ..Default::default()
                }))],
                next_sync_token: Some("event-token".into()),
                ..Default::default()
            })
        }

        fn apply(&self, writes: &[CalendarWrite]) -> Result<Vec<WriteOutcome>> {
            Ok(writes
                .iter()
                .map(|write| match write {
                    CalendarWrite::Put { event, .. } if self.conflict => WriteOutcome::Conflict {
                        local: Box::new(event.clone()),
                        server: Some(Box::new(CalendarEvent {
                            remote_id: event.remote_id.clone(),
                            summary: Some("Server version".into()),
                            ..Default::default()
                        })),
                    },
                    CalendarWrite::Put { event, .. } => {
                        WriteOutcome::Applied(Box::new(event.clone()))
                    }
                    CalendarWrite::Delete { event, .. } => {
                        WriteOutcome::Deleted(EventIdentity::Event(event.remote_id.clone()))
                    }
                })
                .collect())
        }
    }

    #[test]
    fn terminal_tokens_are_persisted_per_collection() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        store
            .insert_identity(&NewIdentity {
                account_id: account,
                address: "me@example.com".into(),
                display_name: None,
                signature: None,
                is_default: true,
            })
            .unwrap();
        let fake = Fake {
            full_once: Mutex::new(false),
            conflict: false,
        };
        sync_account_with_power(&store, account, "google", &fake, 10, false).unwrap();
        let state = store
            .get_sync_state(account, "calendar:google", "work")
            .unwrap()
            .unwrap();
        assert_eq!(state.sync_token.as_deref(), Some("event-token"));
    }

    #[test]
    fn a_410_resync_is_collection_local() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let fake = Fake {
            full_once: Mutex::new(false),
            conflict: false,
        };
        sync_account_with_power(&store, account, "google", &fake, 1, false).unwrap();
        *fake.full_once.lock().unwrap() = true;
        let report =
            sync_account_with_power(&store, account, "google", &fake, 1_002, false).unwrap();
        assert_eq!(report.full_resyncs, 1);
        assert_eq!(report.calendars, 1);
    }

    #[test]
    fn a_412_becomes_a_persisted_conflict_instead_of_a_clobber() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let initial = Fake {
            full_once: Mutex::new(false),
            conflict: false,
        };
        sync_account_with_power(&store, account, "google", &initial, 1, false).unwrap();
        let calendar = store.calendars_for_account(account).unwrap().remove(0);
        let event = CalendarEvent {
            remote_id: "event-1".into(),
            summary: Some("Local edit".into()),
            ..Default::default()
        };
        let write = CalendarWrite::Put {
            calendar: calendar.info.clone(),
            event: event.clone(),
            base_etag: Some("\"old\"".into()),
        };
        store
            .queue_calendar_write(account, calendar.id, &event, &write, "edit-1", 2)
            .unwrap();
        let conflict = Fake {
            full_once: Mutex::new(false),
            conflict: true,
        };
        let report =
            sync_account_with_power(&store, account, "google", &conflict, 1_002, false).unwrap();
        assert_eq!(report.conflicts, 1);
        let (state, conflict_json, summary): (String, Option<String>, Option<String>) = store
            .with_db(|db| {
                Ok(db.query_row(
                    "SELECT p.state, e.conflict_json, e.summary
                     FROM pending_op p JOIN event e ON e.id = p.target_id
                     WHERE p.idempotency_key = 'edit-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?)
            })
            .unwrap();
        assert_eq!(state, "conflict");
        assert!(conflict_json.is_some());
        assert_eq!(summary.as_deref(), Some("Local edit"));
    }
}
