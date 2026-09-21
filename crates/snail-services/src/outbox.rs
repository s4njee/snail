//! The outbox (plan.md E7.11/E7.12): drain queued `send` ops through a `MailProvider`, with a
//! dead-letter state instead of an infinite silent retry (E16.5).
//!
//! Provider-agnostic on purpose — Gmail's REST send and iCloud's SMTP send sit behind the same
//! `MailProvider::send`, which is the whole point of the trait boundary.

use anyhow::Result;

use snail_core::providers::MailProvider;
use snail_core::store::{NewMessage, ProviderRef, Store};

/// Give up after this many attempts and move the op to the dead-letter state.
pub const MAX_ATTEMPTS: i64 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    pub sent: usize,
    pub failed: usize,
    pub dead: usize,
}

/// Send every queued message once. Failures stay `failed` until they hit the attempt cap, then go
/// `dead` so the UI can surface them rather than retrying forever.
pub fn drain_sends(
    store: &Store,
    provider: &dyn MailProvider,
    limit: u32,
    now: i64,
) -> Result<SendReport> {
    let mut report = SendReport::default();
    for op in store
        .pending_ops(limit)?
        .into_iter()
        .filter(|op| op.operation == "send")
    {
        let hash = op
            .payload_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
            .and_then(|value| value.get("raw_hash")?.as_str().map(str::to_string));
        let Some(hash) = hash else {
            store.set_op_state(op.id, "dead", Some("send op has no raw_hash"), now)?;
            report.dead += 1;
            continue;
        };
        let Some(raw) = store.cache().get(&hash)? else {
            // We queued it, so a missing object is a real failure, not a re-fetch signal.
            store.set_op_state(op.id, "failed", Some("raw bytes missing from cache"), now)?;
            report.failed += 1;
            continue;
        };

        match provider.send(&raw) {
            Ok(()) => {
                // The sent copy lands in the local Sent mailbox so it is visible offline (E7.11).
                if let Err(error) = record_sent_copy(store, op.account_id, &raw, now) {
                    log::warn!("could not record the sent copy: {error:#}");
                }
                store.set_op_state(op.id, "done", None, now)?;
                report.sent += 1;
            }
            Err(error) => {
                let message = format!("{error:#}");
                if op.attempts + 1 >= MAX_ATTEMPTS {
                    store.set_op_state(op.id, "dead", Some(&message), now)?;
                    report.dead += 1;
                } else {
                    store.set_op_state(op.id, "failed", Some(&message), now)?;
                    report.failed += 1;
                }
            }
        }
    }
    Ok(report)
}

/// Insert the just-sent message into the local Sent mailbox (E7.11). Best-effort: a failure here
/// must not turn a delivered send into a retry.
fn record_sent_copy(store: &Store, account_id: i64, raw: &[u8], now: i64) -> Result<()> {
    let parsed = snail_core::mime::parse_raw(raw).ok();
    let mailbox = store.ensure_mailbox(account_id, "Sent", "sent")?;
    let hash = store.cache().put(raw)?;
    let message = NewMessage {
        account_id,
        mailbox_id: Some(mailbox),
        provider: Some(ProviderRef::Gmail {
            id: format!("sent-{hash}"),
            thread_id: None,
            history_id: None,
        }),
        subject: parsed.as_ref().and_then(|parsed| parsed.subject.clone()),
        from_name: parsed.as_ref().and_then(|parsed| parsed.from_name.clone()),
        from_addr: parsed.as_ref().and_then(|parsed| parsed.from_addr.clone()),
        to_json: parsed
            .as_ref()
            .and_then(|parsed| serde_json::to_string(&parsed.to).ok()),
        date: Some(now),
        preview: parsed.as_ref().and_then(|parsed| parsed.preview.clone()),
        unread: false,
        has_attachments: parsed
            .as_ref()
            .is_some_and(|parsed| !parsed.attachments.is_empty()),
        body_text: parsed
            .as_ref()
            .and_then(|parsed| parsed.plain.clone())
            .map(|plain| snail_core::mime::search_text(&plain)),
        raw_hash: Some(hash),
        ..Default::default()
    };
    store.insert_message_if_new(&message)?;
    store.recount_mailbox(mailbox)?;
    Ok(())
}

#[cfg(test)]
mod tests {    use super::*;
    use anyhow::{Result, bail};
    use snail_core::providers::{Changes, OpOutcome, RawMessage, RemoteOp};
    use snail_core::store::{NewOp, SyncState};

    struct FakeProvider {
        succeeds: bool,
    }

    impl MailProvider for FakeProvider {
        fn list_changes(&self, _cursor: &SyncState) -> Result<Changes> {
            Ok(Changes::default())
        }
        fn fetch_raw(&self, _ids: &[String]) -> Result<Vec<RawMessage>> {
            Ok(Vec::new())
        }
        fn apply(&self, _ops: &[RemoteOp]) -> Result<Vec<OpOutcome>> {
            Ok(Vec::new())
        }
        fn send(&self, _raw: &[u8]) -> Result<()> {
            if self.succeeds {
                Ok(())
            } else {
                bail!("smtp said no")
            }
        }
    }

    fn queued_send(store: &Store, raw: &[u8], key: &str) {
        let hash = store.cache().put(raw).unwrap();
        store
            .enqueue_op(&NewOp {
                account_id: 1,
                target_kind: "message".into(),
                target_id: None,
                operation: "send".into(),
                payload_json: Some(format!("{{\"raw_hash\":\"{hash}\"}}")),
                idempotency_key: key.into(),
                now: 0,
            })
            .unwrap();
    }

    #[test]
    fn a_queued_send_is_delivered_and_marked_done() {
        let store = Store::open_in_memory().unwrap();
        store.insert_account("gmail", "me@example.com", None, 0).unwrap();
        queued_send(&store, b"raw message", "send:1");

        let report = drain_sends(&store, &FakeProvider { succeeds: true }, 10, 0).unwrap();
        assert_eq!(report, SendReport { sent: 1, failed: 0, dead: 0 });
        // A delivered op is no longer pending.
        assert!(store.pending_ops(10).unwrap().is_empty());
        // The sent copy landed in the local Sent mailbox (E7.11).
        let sent = store.mailbox_id(1, "Sent").unwrap().unwrap();
        assert_eq!(store.page(sent, 10, 0).unwrap().len(), 1);
    }

    #[test]
    fn failures_retry_until_the_cap_then_go_dead() {
        let store = Store::open_in_memory().unwrap();
        store.insert_account("gmail", "me@example.com", None, 0).unwrap();
        queued_send(&store, b"raw message", "send:1");
        let provider = FakeProvider { succeeds: false };

        // The first four attempts leave it retryable.
        for _ in 0..MAX_ATTEMPTS - 1 {
            let report = drain_sends(&store, &provider, 10, 0).unwrap();
            assert_eq!(report.failed, 1);
        }
        // The fifth moves it to the dead-letter state.
        let report = drain_sends(&store, &provider, 10, 0).unwrap();
        assert_eq!(report.dead, 1);
        assert_eq!(store.op_counts().unwrap().dead, 1);
        assert!(store.pending_ops(10).unwrap().is_empty(), "dead ops are not retried");
    }

    #[test]
    fn an_op_with_no_raw_hash_is_dead_lettered_rather_than_looping() {
        let store = Store::open_in_memory().unwrap();
        store.insert_account("gmail", "me@example.com", None, 0).unwrap();
        store
            .enqueue_op(&NewOp {
                account_id: 1,
                target_kind: "message".into(),
                target_id: None,
                operation: "send".into(),
                payload_json: Some("{}".into()),
                idempotency_key: "send:bad".into(),
                now: 0,
            })
            .unwrap();
        let report = drain_sends(&store, &FakeProvider { succeeds: true }, 10, 0).unwrap();
        assert_eq!(report.dead, 1);
    }
}
