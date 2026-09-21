//! The Gmail sync pass (plan.md E4.7, E8.4, E7.11, E16.9): push the user's local changes, then
//! pull Gmail's. One pass is synchronous and self-contained so the app can run it on its
//! background executor on whatever schedule it likes; nothing here knows about GPUI.
//!
//! Order matters. Local changes go **up first**, so the pull that follows sees the server already
//! agreeing with them; and the history cursor is committed only after its batch is applied, so a
//! pass that dies half way re-runs from the last good point (E4.4, E4.23).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};

use snail_core::calendar::CalendarProvider;
use snail_core::providers::gmail::{GmailClient, TokenSource};
use snail_core::providers::google_calendar::GoogleCalendarClient;
use snail_core::providers::{MailProvider, RemoteOp};
use snail_core::store::{AccountRow, Store, SyncState};
use snail_core::triage::{TriageAction, TriagePayload};

use crate::auth::{AccountStore, AuthError, GoogleOAuth, Tokens, now_epoch};
use crate::outbox::{self, MAX_ATTEMPTS};

// --- Tokens (E3.3) --------------------------------------------------------------------------

/// Access tokens for one Google account, refreshed from the keychain's refresh token shortly
/// before they expire. A rotated refresh token is written back. An auth failure is kept, typed,
/// because by the time it surfaces through a Gmail call it is only a string.
pub struct GoogleTokens {
    oauth: GoogleOAuth,
    accounts: Arc<AccountStore>,
    address: String,
    cached: Mutex<Option<Tokens>>,
    failure: Mutex<Option<AuthError>>,
}

impl GoogleTokens {
    pub fn new(oauth: GoogleOAuth, accounts: Arc<AccountStore>, address: &str) -> Self {
        Self {
            oauth,
            accounts,
            address: address.to_string(),
            cached: Mutex::new(None),
            failure: Mutex::new(None),
        }
    }

    /// Seed with tokens just obtained from a sign-in, saving the refresh round trip.
    pub fn with_tokens(self, tokens: Tokens) -> Self {
        *self.cached.lock().unwrap() = Some(tokens);
        self
    }

    /// The last auth failure, if the most recent refresh failed.
    pub fn failure(&self) -> Option<AuthError> {
        self.failure.lock().unwrap().clone()
    }

    fn fail(&self, error: AuthError) -> anyhow::Error {
        *self.failure.lock().unwrap() = Some(error.clone());
        error.into()
    }
}

impl TokenSource for GoogleTokens {
    fn access_token(&self) -> Result<String> {
        let mut cached = self.cached.lock().unwrap();
        if let Some(tokens) = cached.as_ref().filter(|tokens| tokens.is_fresh()) {
            return Ok(tokens.access_token.clone());
        }
        let refresh_token = match self.accounts.google_refresh_token(&self.address) {
            Ok(Some(token)) => token,
            Ok(None) => return Err(self.fail(AuthError::TokenRevoked)),
            Err(error) => {
                return Err(self.fail(AuthError::KeychainUnavailable(format!("{error:#}"))));
            }
        };
        let tokens = match self.oauth.refresh(&refresh_token) {
            Ok(tokens) => tokens,
            Err(error) => {
                return Err(match error.downcast::<AuthError>() {
                    Ok(auth) => self.fail(auth),
                    // A network failure is not an auth state; the next pass retries (E16).
                    Err(other) => other,
                });
            }
        };
        if let Some(rotated) = tokens
            .refresh_token
            .as_deref()
            .filter(|new| *new != refresh_token)
            && let Err(error) = self
                .accounts
                .set_google_refresh_token(&self.address, rotated)
        {
            log::warn!("could not save the rotated refresh token: {error:#}");
        }
        *self.failure.lock().unwrap() = None;
        let access = tokens.access_token.clone();
        *cached = Some(tokens);
        Ok(access)
    }
}

// --- Sign-in (E3.1, E14.3) ------------------------------------------------------------------

/// A freshly connected account.
pub struct Connected {
    pub account_id: i64,
    pub address: String,
    pub tokens: Tokens,
}

/// The whole Google sign-in: browser consent, then the profile to learn which address this is,
/// then the refresh token into the keychain and the account into the store. Re-connecting an
/// account that already exists replaces its token and keeps its mail.
pub fn connect_google(
    accounts: &Arc<AccountStore>,
    oauth: &GoogleOAuth,
    cancel: &AtomicBool,
) -> Result<Connected> {
    let tokens = oauth.authorize_loopback_until(cancel, crate::auth::LOOPBACK_TIMEOUT)?;
    let refresh_token = tokens.refresh_token.clone().ok_or_else(|| {
        anyhow!(AuthError::Other(
            "Google returned no refresh token, so Snail could not stay signed in. Remove Snail at \
             myaccount.google.com → Security → Third-party connections, then sign in again."
                .into()
        ))
    })?;
    let client = GmailClient::new(Arc::new(snail_core::providers::gmail::FixedToken(
        tokens.access_token.clone(),
    )))?;
    let profile = client.profile()?;
    if profile.email.is_empty() {
        return Err(anyhow!("Gmail did not say which address this account is"));
    }
    accounts.set_google_refresh_token(&profile.email, &refresh_token)?;
    let account_id = accounts.add_account("gmail", &profile.email, None, now_epoch() as i64)?;
    log::info!("connected Gmail account {} (#{account_id})", profile.email);
    Ok(Connected {
        account_id,
        address: profile.email,
        tokens,
    })
}

// --- The sync pass --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct SyncOptions {
    /// The backfill window (plan.md §8 open question 3: the last 30 days).
    pub backfill_days: u32,
    /// A ceiling on the first sync, so a busy account still finishes it in a couple of minutes.
    pub backfill_limit: usize,
    /// How long a local change waits before it is pushed, so the undo bar can still cancel it
    /// locally (E8.5). Longer than the bar's six seconds.
    pub settle_secs: i64,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            backfill_days: 30,
            backfill_limit: 2000,
            settle_secs: 8,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// This pass was a first sync (or a full resync after the history window expired).
    pub backfilled: bool,
    pub inserted: usize,
    pub deleted: usize,
    pub relabelled: usize,
    pub pushed: usize,
    pub rejected: usize,
    pub sent: usize,
    pub send_failed: usize,
    pub calendar_upserted: usize,
    pub calendar_deleted: usize,
    pub calendar_conflicts: usize,
    pub calendar_full_resyncs: usize,
}

impl SyncReport {
    /// Whether the pass changed anything the UI shows.
    pub fn changed(&self) -> bool {
        self.inserted
            + self.deleted
            + self.relabelled
            + self.pushed
            + self.rejected
            + self.sent
            + self.send_failed
            + self.calendar_upserted
            + self.calendar_deleted
            + self.calendar_conflicts
            + self.calendar_full_resyncs
            > 0
            || self.backfilled
    }
}

/// Why a pass stopped. Auth is its own case: it needs the user, and retrying will not help.
#[derive(Debug)]
pub enum SyncError {
    Auth(AuthError),
    Other(anyhow::Error),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Auth(error) => write!(formatter, "{error}"),
            SyncError::Other(error) => write!(formatter, "{error:#}"),
        }
    }
}

/// One Gmail account, ready to sync.
pub struct GmailAccount {
    pub account_id: i64,
    pub address: String,
    tokens: Arc<GoogleTokens>,
    client: GmailClient,
    calendar: GoogleCalendarClient,
}

impl GmailAccount {
    pub fn new(account: &AccountRow, tokens: GoogleTokens) -> Result<Self> {
        let tokens = Arc::new(tokens);
        Ok(Self {
            account_id: account.id,
            address: account.address.clone(),
            client: GmailClient::new(tokens.clone())?,
            calendar: GoogleCalendarClient::new(tokens.clone())?,
            tokens,
        })
    }

    /// Discover the account's selectable collections without pulling message bodies or events.
    /// This is the boundary between successful OAuth and the user's first-sync choices.
    pub fn discover_collections(&self, store: &Store, now: i64) -> Result<()> {
        for (name, kind) in [
            ("Inbox", "inbox"),
            ("Sent", "sent"),
            ("Drafts", "drafts"),
            ("Archive", "archive"),
            ("Trash", "trash"),
        ] {
            store.ensure_mailbox(self.account_id, name, kind)?;
        }
        for label in self.client.labels()?.into_iter().filter(|label| label.user) {
            store.ensure_mailbox(self.account_id, &label.name, "other")?;
        }
        let discovery = self.calendar.discover_calendars(None)?;
        store.apply_calendar_discovery(self.account_id, "google", &discovery, now)?;
        Ok(())
    }

    /// One full pass: push, send, pull. `progress` reports `(done, total)` while messages are
    /// being fetched, which on a first sync is most of the pass.
    pub fn sync(
        &self,
        store: &Store,
        options: &SyncOptions,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SyncReport, SyncError> {
        self.sync_inner(store, options, progress).map_err(|error| {
            match self
                .tokens
                .failure()
                .or_else(|| error.downcast_ref::<AuthError>().cloned())
            {
                Some(auth) => SyncError::Auth(auth),
                None => SyncError::Other(error),
            }
        })
    }

    fn sync_inner(
        &self,
        store: &Store,
        options: &SyncOptions,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SyncReport> {
        let mut report = SyncReport::default();
        let now = now_epoch() as i64;

        // 1. Up: settled triage, then queued sends.
        let (pushed, rejected) = self.push_triage(store, now - options.settle_secs, now)?;
        report.pushed = pushed;
        report.rejected = rejected;
        let sends = outbox::drain_sends_for(store, &self.client, Some(self.account_id), 20, now)?;
        if let Some(auth) = self.tokens.failure() {
            // A send that failed only because the token did is not the message's fault.
            return Err(auth.into());
        }
        report.sent = sends.sent;
        report.send_failed = sends.failed + sends.dead;

        // 2. Down: history since the cursor, or a selection-aware backfill when there is none.
        // With every mailbox disabled, mail pulling pauses while calendar and outbound work keep
        // functioning. A later re-enable still has no cursor and therefore receives a backfill.
        let enabled_mailboxes = store.enabled_mailbox_filters(self.account_id)?;
        if !enabled_mailboxes.is_empty() {
            let state = store
                .get_sync_state(self.account_id, "gmail", "")?
                .filter(|state| !state.full_resync_needed && state.history_id.is_some());
            let changes = match &state {
                Some(state) => Some(self.client.list_changes(state)?),
                None => None,
            };
            match changes {
                Some(changes) if !changes.full_resync => {
                    let applied =
                        self.client
                            .apply_changes(store, self.account_id, &changes, progress)?;
                    report.inserted = applied.inserted;
                    report.deleted = applied.deleted;
                    report.relabelled = applied.relabelled;
                    let history_id = changes
                        .next_cursor
                        .as_deref()
                        .and_then(|cursor| cursor.parse::<i64>().ok())
                        .or(state.as_ref().and_then(|state| state.history_id));
                    store.set_sync_state(&SyncState {
                        account_id: self.account_id,
                        kind: "gmail".into(),
                        collection: String::new(),
                        history_id,
                        last_sync: Some(now),
                        ..Default::default()
                    })?;
                }
                _ => {
                    let query = selected_mail_query(options.backfill_days, &enabled_mailboxes);
                    let backfill = self.client.backfill_query_with_progress(
                        store,
                        self.account_id,
                        None,
                        &query,
                        options.backfill_limit,
                        progress,
                    )?;
                    report.backfilled = true;
                    report.inserted = backfill.inserted;
                }
            }
        }
        // Calendar shares the OAuth account but has independent per-collection cursors and quota.
        // A transient calendar failure does not roll back mail already committed in this pass.
        match crate::calendar_sync::sync_account(
            store,
            self.account_id,
            "google",
            &self.calendar,
            now,
        ) {
            Ok(calendar) => {
                report.calendar_upserted = calendar.upserted;
                report.calendar_deleted = calendar.deleted;
                report.calendar_conflicts = calendar.conflicts;
                report.calendar_full_resyncs = calendar.full_resyncs;
            }
            Err(error) => log::warn!("calendar sync {}: {error:#}", self.address),
        }
        Ok(report)
    }

    /// Push every settled triage op for this account. Returns `(pushed, rejected)`. A network
    /// failure leaves the op pending for the next pass; a rejection dead-letters it, and the shell
    /// then rolls the row back with a visible notice (E8.4).
    fn push_triage(&self, store: &Store, created_before: i64, now: i64) -> Result<(usize, usize)> {
        let mut pushed = 0;
        let mut rejected = 0;
        for op in store.settled_ops(self.account_id, created_before, 100)? {
            if op.target_kind != "message" || op.operation == "send" {
                continue;
            }
            let action = op
                .payload_json
                .as_deref()
                .and_then(TriagePayload::from_json)
                .map(|payload| payload.action);
            let gmail_id = match op.target_id {
                Some(id) => store.gmail_id_of(id)?,
                None => None,
            };
            // Local-only rows (a sent stand-in, an import) have nothing to push.
            let gmail_id = gmail_id.filter(|id| !is_local_only(id));
            let delta = action.as_ref().and_then(gmail_label_delta);
            let (Some(gmail_id), Some((add, remove))) = (gmail_id, delta) else {
                store.set_op_state(op.id, "done", None, now)?;
                continue;
            };
            let outcome = self
                .client
                .apply(&[RemoteOp::Label {
                    id: gmail_id.clone(),
                    add: add.clone(),
                    remove: remove.clone(),
                }])?
                .into_iter()
                .next();
            if let Some(auth) = self.tokens.failure() {
                return Err(auth.into());
            }
            match outcome {
                Some(outcome) if outcome.ok => {
                    // Keep the stored labels in step, so the next history merge starts from truth.
                    if let (Some(message_id), Some((_, mut labels))) = (
                        op.target_id,
                        store.gmail_message_labels(self.account_id, &gmail_id)?,
                    ) {
                        snail_core::providers::gmail::merge_labels(&mut labels, &add, &remove);
                        let kind = snail_core::providers::gmail::mailbox_kind_for_labels(&labels);
                        let mailbox = store.ensure_mailbox(
                            self.account_id,
                            kind.display_name(),
                            kind.as_str(),
                        )?;
                        store.set_gmail_labels(message_id, &labels, mailbox)?;
                    }
                    store.set_op_state(op.id, "done", None, now)?;
                    pushed += 1;
                }
                outcome => {
                    let error = outcome
                        .as_ref()
                        .and_then(|outcome| outcome.error.clone())
                        .unwrap_or_else(|| "no outcome".into());
                    if error.contains("(Auth)") {
                        // The token was accepted for refresh but not for this call.
                        return Err(AuthError::TokenRevoked.into());
                    }
                    let terminal = outcome.as_ref().is_some_and(|outcome| outcome.terminal)
                        || op.attempts + 1 >= MAX_ATTEMPTS
                        || is_rejection(&error);
                    if terminal {
                        store.set_op_state(op.id, "dead", Some(&error), now)?;
                        rejected += 1;
                    } else {
                        store.set_op_state(op.id, "pending", Some(&error), now)?;
                    }
                }
            }
        }
        Ok((pushed, rejected))
    }
}

fn selected_mail_query(days: u32, mailboxes: &[(String, String)]) -> String {
    const ALL: [&str; 5] = ["inbox", "sent", "drafts", "archive", "trash"];
    if ALL
        .iter()
        .all(|kind| mailboxes.iter().any(|(_, value)| value == kind))
    {
        return snail_core::providers::gmail::backfill_query(days);
    }
    let mut terms: Vec<String> = Vec::new();
    for kind in ALL {
        if !mailboxes.iter().any(|(_, value)| value == kind) {
            continue;
        }
        terms.push(
            match kind {
                "inbox" => "in:inbox",
                "sent" => "in:sent",
                "drafts" => "in:drafts",
                "trash" => "in:trash",
                // Gmail has no `in:archive`; Archive means mail outside the four special buckets.
                "archive" => "(-in:inbox -in:sent -in:drafts -in:trash)",
                _ => unreachable!(),
            }
            .into(),
        );
    }
    for (name, kind) in mailboxes {
        if kind == "other" {
            terms.push(format!("label:\"{}\"", name.replace('"', "")));
        }
    }
    format!("newer_than:{days}d {{{}}}", terms.join(" "))
}

/// A 4xx from Gmail other than throttling: the change itself was refused, so retrying is useless.
fn is_rejection(error: &str) -> bool {
    error.starts_with("Gmail 4") && !error.starts_with("Gmail 429")
}

/// Rows the store invented rather than fetched: a sent message's stand-in (E7.11) and imports.
fn is_local_only(gmail_id: &str) -> bool {
    gmail_id.starts_with("sent-") || gmail_id.starts_with("import-")
}

/// A triage action as Gmail label edits (E8.6): archive is removing `INBOX`, trash is adding
/// `TRASH`, read is removing `UNREAD`. `None` for a move Gmail has no label for yet.
pub fn gmail_label_delta(action: &TriageAction) -> Option<(Vec<String>, Vec<String>)> {
    let labels = |names: &[&str]| {
        names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>()
    };
    Some(match action {
        TriageAction::Archive => (vec![], labels(&["INBOX"])),
        TriageAction::Trash => (labels(&["TRASH"]), labels(&["INBOX"])),
        TriageAction::MarkRead(true) => (vec![], labels(&["UNREAD"])),
        TriageAction::MarkRead(false) => (labels(&["UNREAD"]), vec![]),
        TriageAction::MarkFlagged(true) => (labels(&["STARRED"]), vec![]),
        TriageAction::MarkFlagged(false) => (vec![], labels(&["STARRED"])),
        // Undo of archive/trash arrives as a move back to where the message was (E8.5).
        TriageAction::Move { mailbox } => match mailbox.as_str() {
            "Inbox" => (labels(&["INBOX"]), labels(&["TRASH"])),
            "Archive" => (vec![], labels(&["INBOX", "TRASH"])),
            "Trash" => (labels(&["TRASH"]), labels(&["INBOX"])),
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn triage_maps_onto_gmail_labels() {
        assert_eq!(
            gmail_label_delta(&TriageAction::Archive),
            Some((vec![], strings(&["INBOX"])))
        );
        assert_eq!(
            gmail_label_delta(&TriageAction::Trash),
            Some((strings(&["TRASH"]), strings(&["INBOX"])))
        );
        assert_eq!(
            gmail_label_delta(&TriageAction::MarkRead(true)),
            Some((vec![], strings(&["UNREAD"])))
        );
        assert_eq!(
            gmail_label_delta(&TriageAction::MarkFlagged(true)),
            Some((strings(&["STARRED"]), vec![]))
        );
    }

    #[test]
    fn undoing_archive_or_trash_puts_the_message_back_in_the_inbox() {
        // Not "add Inbox, remove INBOX": the inverse of trash must also drop TRASH.
        assert_eq!(
            gmail_label_delta(&TriageAction::Move {
                mailbox: "Inbox".into()
            }),
            Some((strings(&["INBOX"]), strings(&["TRASH"])))
        );
        assert_eq!(
            gmail_label_delta(&TriageAction::Move {
                mailbox: "Receipts".into()
            }),
            None
        );
    }

    #[test]
    fn throttling_is_retried_but_a_refusal_is_not() {
        assert!(is_rejection(
            "Gmail 404 (HistoryExpired): Requested entity was not found."
        ));
        assert!(!is_rejection("Gmail 429 (Retryable): slow down"));
        assert!(!is_rejection("Gmail 0 (Other): connection refused"));
    }

    #[test]
    fn stand_ins_are_never_pushed() {
        assert!(is_local_only("sent-3f2a"));
        assert!(is_local_only("import-99"));
        assert!(!is_local_only("18c2f0a1b2"));
    }

    #[test]
    fn a_change_waits_out_the_undo_window_before_it_is_pushed() {
        let store = Store::open_in_memory().unwrap();
        let account = store
            .insert_account("gmail", "me@example.com", None, 0)
            .unwrap();
        let inbox = store.ensure_mailbox(account, "Inbox", "inbox").unwrap();
        let message = store
            .insert_message(&snail_core::store::NewMessage {
                account_id: account,
                mailbox_id: Some(inbox),
                provider: Some(snail_core::store::ProviderRef::Gmail {
                    id: "m1".into(),
                    thread_id: None,
                    history_id: None,
                }),
                ..Default::default()
            })
            .unwrap();
        store
            .apply_triage(message, &TriageAction::Archive, "k", 100)
            .unwrap();
        assert!(store.settled_ops(account, 99, 10).unwrap().is_empty());
        assert_eq!(store.settled_ops(account, 100, 10).unwrap().len(), 1);
        // Another account's queue is not this one's.
        assert!(store.settled_ops(account + 1, 1000, 10).unwrap().is_empty());
    }

    #[test]
    fn a_report_with_anything_in_it_counts_as_a_change() {
        assert!(!SyncReport::default().changed());
        assert!(
            SyncReport {
                relabelled: 1,
                ..Default::default()
            }
            .changed()
        );
        assert!(
            SyncReport {
                backfilled: true,
                ..Default::default()
            }
            .changed()
        );
    }

    #[test]
    fn first_pull_query_honours_selected_mailboxes() {
        assert_eq!(
            selected_mail_query(
                30,
                &[
                    ("Inbox".into(), "inbox".into()),
                    ("Sent".into(), "sent".into()),
                    ("Receipts/2026".into(), "other".into()),
                ]
            ),
            "newer_than:30d {in:inbox in:sent label:\"Receipts/2026\"}"
        );
        assert_eq!(
            selected_mail_query(7, &[("Archive".into(), "archive".into())]),
            "newer_than:7d {(-in:inbox -in:sent -in:drafts -in:trash)}"
        );
        assert_eq!(
            selected_mail_query(
                30,
                &[
                    ("Inbox".into(), "inbox".into()),
                    ("Sent".into(), "sent".into()),
                    ("Drafts".into(), "drafts".into()),
                    ("Archive".into(), "archive".into()),
                    ("Trash".into(), "trash".into()),
                ],
            ),
            "newer_than:30d"
        );
    }
}
