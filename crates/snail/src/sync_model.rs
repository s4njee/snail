//! Accounts and background sync for the app (plan.md E3, E4, E14.3, E16.9). A **model**, not a
//! view: like `mail_model` it may touch the store, and it is the only place the app talks to the
//! keychain or the network for mail.
//!
//! One loop per app. It wakes every second but only runs a pass when one is due: on a fixed poll,
//! as soon as a queued local change has outlived its undo window, or right after a sign-in. The
//! pass itself (`snail_services::sync`) runs on the background executor; the loop only moves
//! progress and results back to the main thread, where the shell hears about them as
//! [`SyncChanged`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::*;

use snail_core::store::{AccountRow, Store};
use snail_services::auth::{AccountStore, AuthError, GoogleOAuth};
use snail_services::secrets::KeyringStore;
use snail_services::sync::{
    self as mail_sync, GmailAccount, GoogleTokens, SyncError, SyncOptions, SyncReport,
};

/// After a pass that failed for a transient reason, wait this long before the next.
const RETRY: Duration = Duration::from_secs(30);
/// How often the running pass's progress is copied to the UI, and so how often mail streaming in
/// during a first sync appears in the list.
const PROGRESS_TICK: Duration = Duration::from_millis(750);

/// What the sidebar footer says about sync.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncStatus {
    /// Nothing has run yet this session.
    Starting,
    /// A pass is running. `first` is a first sync (or a resync), with `done` of `total` fetched.
    Syncing {
        first: bool,
        done: usize,
        total: usize,
    },
    UpToDate {
        at: Instant,
    },
    /// Could not reach Gmail. Offline is normal; the loop retries on its own.
    Offline(String),
    /// Only the user can fix this: sign in again.
    NeedsSignIn {
        address: String,
        message: String,
    },
    /// No OAuth client is configured yet.
    NoClient,
}

/// Emitted whenever the store may have changed or the status did.
pub struct SyncChanged;

pub struct SyncHub {
    store: Arc<Store>,
    accounts: Arc<AccountStore>,
    enabled: bool,
    status: SyncStatus,
    account_status: HashMap<i64, SyncStatus>,
    /// The user asked to sign in again (or add the account) from inside the app.
    reconnecting: bool,
    /// Accounts with warm access tokens, reused across passes.
    live: HashMap<i64, Arc<GmailAccount>>,
    /// A freshly authenticated account remains here until the user confirms collection choices.
    pending_setup: Option<i64>,
    running: bool,
    /// Per-account next pull, plus explicit one-shot requests (manual and user-triggered sync).
    account_due: HashMap<i64, Instant>,
    forced_accounts: HashSet<i64>,
    /// New-mail notifications cover messages stored after this id. Starts at the newest message
    /// at launch, so opening the app never replays the mailbox.
    notified_through: i64,
    _loop: Task<()>,
}

/// A notification was clicked: show this message.
pub struct OpenMessage(pub i64);

impl EventEmitter<OpenMessage> for SyncHub {}

impl EventEmitter<SyncChanged> for SyncHub {}

struct Global(Entity<SyncHub>);
impl gpui_kit::Global for Global {}

/// A pass's inputs, gathered on the main thread and moved to the background.
struct Job {
    store: Arc<Store>,
    accounts: Arc<AccountStore>,
    rows: Vec<AccountRow>,
    icloud_rows: Vec<AccountRow>,
    live: HashMap<i64, Arc<GmailAccount>>,
}

#[derive(Default)]
struct Progress {
    first: bool,
    done: usize,
    total: usize,
    /// Bumped whenever a batch lands, so the UI knows to re-read the list.
    batches: usize,
}

struct PassResult {
    live: HashMap<i64, Arc<GmailAccount>>,
    no_client: bool,
    /// Some account was on its first sync (or a resync): its mail is history, not news.
    backfilling: bool,
    attempted: Vec<AccountRow>,
    outcomes: Vec<(AccountRow, Result<SyncReport, SyncError>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupCollectionKind {
    Mailbox,
    Calendar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupCollection {
    pub id: i64,
    pub kind: SetupCollectionKind,
    pub name: String,
    pub color: Option<String>,
    pub enabled: bool,
}

impl SyncHub {
    /// Create the hub and make it reachable from any window via [`SyncHub::global`].
    pub fn install(store: Arc<Store>, enabled: bool, cx: &mut App) -> Entity<SyncHub> {
        let hub = cx.new(|cx| SyncHub::new(store, enabled, cx));
        cx.set_global(Global(hub.clone()));
        hub
    }

    pub fn global(cx: &App) -> Option<Entity<SyncHub>> {
        cx.try_global::<Global>().map(|global| global.0.clone())
    }

    fn new(store: Arc<Store>, enabled: bool, cx: &mut Context<Self>) -> Self {
        let accounts = Arc::new(AccountStore::new(
            store.clone(),
            Arc::new(KeyringStore::new()),
        ));
        let run = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(job) = this.update(cx, |hub, cx| hub.take_job(cx)) else {
                    break;
                };
                let Some(job) = job else {
                    continue;
                };
                let progress = Arc::new(Mutex::new(Progress::default()));
                let finished = Arc::new(Mutex::new(None));
                {
                    let progress = progress.clone();
                    let finished = finished.clone();
                    cx.background_executor()
                        .spawn(async move {
                            let result = run_pass(job, &progress);
                            *finished.lock().unwrap() = Some(result);
                        })
                        .detach();
                }
                let mut seen_batches = 0;
                loop {
                    cx.background_executor().timer(PROGRESS_TICK).await;
                    if let Some(result) = finished.lock().unwrap().take() {
                        this.update(cx, |hub, cx| hub.finish(result, cx)).ok();
                        break;
                    }
                    let (first, done, total, batches) = {
                        let progress = progress.lock().unwrap();
                        (
                            progress.first,
                            progress.done,
                            progress.total,
                            progress.batches,
                        )
                    };
                    let landed = batches != seen_batches;
                    seen_batches = batches;
                    let alive = this.update(cx, |hub, cx| {
                        let status = SyncStatus::Syncing { first, done, total };
                        if hub.status != status || landed {
                            hub.status = status;
                            cx.emit(SyncChanged);
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        return;
                    }
                }
            }
        });
        let notified_through = store.max_message_id().unwrap_or(0);
        let remote_accounts = store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|account| account.kind != "local")
            .collect::<Vec<_>>();
        let account_due = remote_accounts
            .iter()
            .map(|account| (account.id, Instant::now()))
            .collect();
        let account_status = remote_accounts
            .into_iter()
            .map(|account| (account.id, SyncStatus::Starting))
            .collect();
        Self {
            store,
            accounts,
            enabled,
            status: SyncStatus::Starting,
            account_status,
            reconnecting: false,
            live: HashMap::new(),
            pending_setup: None,
            running: false,
            account_due,
            forced_accounts: HashSet::new(),
            notified_through,
            _loop: run,
        }
    }

    pub fn status(&self) -> &SyncStatus {
        &self.status
    }

    pub fn account_status(&self, account_id: i64) -> Option<&SyncStatus> {
        self.account_status.get(&account_id)
    }

    /// Accounts that sync with a provider (the `local` one holds `.eml` imports).
    fn remote_accounts(&self) -> Vec<AccountRow> {
        self.store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|account| account.kind != "local")
            .collect()
    }

    /// Show onboarding instead of the mailbox: there is no account yet, or the user asked to
    /// (re)connect one.
    pub fn needs_onboarding(&self) -> bool {
        self.reconnecting || self.remote_accounts().is_empty()
    }

    pub fn has_accounts(&self) -> bool {
        !self.remote_accounts().is_empty()
    }

    /// A first sync is running, so an empty mailbox means "not here yet", not "empty".
    pub fn is_first_sync(&self) -> bool {
        matches!(self.status, SyncStatus::Syncing { first: true, .. })
            || (self.status == SyncStatus::Starting && self.enabled)
    }

    /// Open onboarding over the mailbox, to reconnect an account or fix the OAuth client.
    pub fn begin_reconnect(&mut self, cx: &mut Context<Self>) {
        self.reconnecting = true;
        cx.emit(SyncChanged);
        cx.notify();
    }

    /// Leave a reconnect the user started without finishing it.
    pub fn cancel_reconnect(&mut self, cx: &mut Context<Self>) {
        self.reconnecting = false;
        cx.emit(SyncChanged);
        cx.notify();
    }

    /// Run a pass now rather than at the next poll.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        self.forced_accounts
            .extend(self.remote_accounts().into_iter().map(|account| account.id));
        cx.notify();
    }

    pub fn sync_account_now(&mut self, account_id: i64, cx: &mut Context<Self>) {
        if self.pending_setup == Some(account_id) {
            self.pending_setup = None;
            self.reconnecting = false;
        }
        self.forced_accounts.insert(account_id);
        self.account_status
            .entry(account_id)
            .or_insert(SyncStatus::Starting);
        cx.notify();
    }

    pub fn stage_account_setup(&mut self, account_id: i64, cx: &mut Context<Self>) {
        self.pending_setup = Some(account_id);
        self.reconnecting = false;
        self.account_due.remove(&account_id);
        self.forced_accounts.remove(&account_id);
        self.account_status.insert(account_id, SyncStatus::Starting);
        cx.notify();
    }

    pub fn forget_account(&mut self, account_id: i64, cx: &mut Context<Self>) {
        self.live.remove(&account_id);
        self.account_due.remove(&account_id);
        self.forced_accounts.remove(&account_id);
        self.account_status.remove(&account_id);
        if self.pending_setup == Some(account_id) {
            self.pending_setup = None;
        }
        cx.emit(SyncChanged);
        cx.notify();
    }

    pub fn set_account_cadence(&mut self, account_id: i64, minutes: u32, cx: &mut Context<Self>) {
        self.account_due.insert(
            account_id,
            Instant::now()
                + if minutes == 0 {
                    Duration::from_secs(365 * 24 * 60 * 60)
                } else {
                    Duration::from_secs(u64::from(minutes) * 60)
                },
        );
        cx.notify();
    }

    pub fn setup_collections(&self) -> Vec<SetupCollection> {
        let Some(account_id) = self.pending_setup else {
            return Vec::new();
        };
        let mut result = Vec::new();
        if let Ok(mailboxes) = self.store.mailboxes() {
            for mailbox in mailboxes
                .into_iter()
                .filter(|mailbox| mailbox.account_id == account_id)
            {
                result.push(SetupCollection {
                    id: mailbox.id,
                    kind: SetupCollectionKind::Mailbox,
                    name: mailbox.name,
                    color: None,
                    enabled: self.store.mailbox_sync_enabled(mailbox.id).unwrap_or(true),
                });
            }
        }
        if let Ok(calendars) = self.store.calendars_for_account(account_id) {
            for calendar in calendars {
                result.push(SetupCollection {
                    id: calendar.id,
                    kind: SetupCollectionKind::Calendar,
                    name: calendar.info.name.unwrap_or_else(|| "Calendar".into()),
                    color: calendar.info.color,
                    enabled: calendar.visible,
                });
            }
        }
        result
    }

    pub fn set_setup_collection(
        &mut self,
        collection: &SetupCollection,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        let result = match collection.kind {
            SetupCollectionKind::Mailbox => {
                self.store.set_mailbox_sync_enabled(collection.id, enabled)
            }
            SetupCollectionKind::Calendar => {
                self.store.set_calendar_visible(collection.id, enabled)
            }
        };
        if let Err(error) = result {
            log::warn!("could not update setup collection: {error:#}");
        }
        cx.notify();
    }

    /// Confirm discovery choices and permit the first full pull.
    pub fn finish_setup(&mut self, cx: &mut Context<Self>) {
        if let Some(account_id) = self.pending_setup.take() {
            self.account_due.insert(account_id, Instant::now());
            self.forced_accounts.insert(account_id);
        }
        self.reconnecting = false;
        cx.emit(SyncChanged);
        cx.notify();
    }

    /// The saved OAuth client, read off the main thread: the keychain can block on a prompt.
    pub fn load_client(&self, cx: &mut Context<Self>) -> Task<Option<GoogleOAuth>> {
        let accounts = self.accounts.clone();
        cx.background_executor().spawn(async move {
            accounts.google_client().unwrap_or_else(|error| {
                log::warn!("could not read the Google OAuth client: {error:#}");
                None
            })
        })
    }

    /// Save the OAuth client to the keychain.
    pub fn save_client(
        &self,
        client: GoogleOAuth,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let accounts = self.accounts.clone();
        cx.background_executor().spawn(async move {
            accounts
                .set_google_client(&client)
                .map_err(|error| format!("Could not save to the keychain: {error:#}"))
        })
    }

    /// Sign in with Google in the system browser, store the refresh token, and start the first
    /// sync. Resolves to the connected address, or a message to show.
    pub fn connect(
        &mut self,
        oauth: GoogleOAuth,
        cancel: Arc<AtomicBool>,
        cx: &mut Context<Self>,
    ) -> Task<Result<String, String>> {
        let accounts = self.accounts.clone();
        let store = self.store.clone();
        let signing_in = cx.background_executor().spawn({
            let oauth = oauth.clone();
            async move {
                let connected = mail_sync::connect_google(&accounts, &oauth, &cancel)?;
                let row = store
                    .account_by_address("gmail", &connected.address)?
                    .ok_or_else(|| anyhow::anyhow!("the account was not saved"))?;
                let tokens = GoogleTokens::new(oauth, accounts, &connected.address)
                    .with_tokens(connected.tokens);
                let account = Arc::new(GmailAccount::new(&row, tokens)?);
                account.discover_collections(&store, current_epoch())?;
                anyhow::Ok((row.clone(), account))
            }
        });
        cx.spawn(async move |this, cx| {
            let result = signing_in.await;
            this.update(cx, |hub, cx| match result {
                Ok((row, account)) => {
                    hub.live.insert(row.id, account);
                    hub.pending_setup = Some(row.id);
                    hub.reconnecting = true;
                    hub.status = SyncStatus::Starting;
                    hub.account_status.insert(row.id, SyncStatus::Starting);
                    cx.emit(SyncChanged);
                    cx.notify();
                    Ok(row.address)
                }
                Err(error) => Err(match error.downcast_ref::<AuthError>() {
                    Some(auth) => auth.message(),
                    None => format!("{error:#}"),
                }),
            })
            .unwrap_or_else(|_| Err("Snail is shutting down".into()))
        })
    }

    /// The next pass's inputs, when one is due and none is running.
    fn take_job(&mut self, cx: &mut Context<Self>) -> Option<Job> {
        if !self.enabled || self.running {
            return None;
        }
        let now = Instant::now();
        let op_due = self
            .store
            .oldest_outstanding_op_at()
            .ok()
            .flatten()
            .is_some_and(|created| created <= current_epoch() - SyncOptions::default().settle_secs);
        // A failing op must not turn into a pass every second: honour the retry delay.
        let retrying = matches!(self.status, SyncStatus::Offline(_));
        let pending_setup = self.pending_setup;
        let due = |hub: &Self, account: &AccountRow| {
            if pending_setup == Some(account.id) {
                return false;
            }
            if hub.forced_accounts.contains(&account.id) || (op_due && !retrying) {
                return true;
            }
            let cadence = crate::settings::account_sync_minutes(&account.address, cx);
            cadence > 0
                && hub
                    .account_due
                    .get(&account.id)
                    .is_none_or(|deadline| *deadline <= now)
        };
        let accounts: Vec<AccountRow> = self
            .remote_accounts()
            .into_iter()
            .filter(|account| due(self, account))
            .collect();
        let rows: Vec<AccountRow> = accounts
            .iter()
            .filter(|account| account.kind == "gmail")
            .cloned()
            .collect();
        let icloud_rows: Vec<AccountRow> = accounts
            .into_iter()
            .filter(|account| account.kind == "icloud")
            .collect();
        if rows.is_empty() && icloud_rows.is_empty() {
            return None;
        }
        self.running = true;
        for account in rows.iter().chain(icloud_rows.iter()) {
            self.account_status.insert(
                account.id,
                SyncStatus::Syncing {
                    first: false,
                    done: 0,
                    total: 0,
                },
            );
        }
        if !matches!(self.status, SyncStatus::Syncing { .. }) {
            self.status = SyncStatus::Syncing {
                first: false,
                done: 0,
                total: 0,
            };
            cx.notify();
        }
        Some(Job {
            store: self.store.clone(),
            accounts: self.accounts.clone(),
            rows,
            icloud_rows,
            live: std::mem::take(&mut self.live),
        })
    }

    fn finish(&mut self, result: PassResult, cx: &mut Context<Self>) {
        self.running = false;
        self.live = result.live;
        let now = Instant::now();
        self.status = SyncStatus::UpToDate { at: now };
        for account in &result.attempted {
            self.forced_accounts.remove(&account.id);
            let minutes = crate::settings::account_sync_minutes(&account.address, cx);
            // Manual accounts retain a distant deadline and run only through `forced_accounts`.
            let delay = if minutes == 0 {
                Duration::from_secs(365 * 24 * 60 * 60)
            } else {
                Duration::from_secs(u64::from(minutes) * 60)
            };
            self.account_due.insert(account.id, now + delay);
        }
        if result.no_client {
            self.status = SyncStatus::NoClient;
            for account in &result.attempted {
                if account.kind == "gmail" {
                    self.account_status.insert(account.id, SyncStatus::NoClient);
                }
            }
        }
        for (account, outcome) in result.outcomes {
            match outcome {
                Ok(report) => {
                    self.account_status
                        .insert(account.id, SyncStatus::UpToDate { at: now });
                    if report.changed() {
                        log::info!("sync {}: {report:?}", account.address);
                    }
                }
                Err(SyncError::Auth(error)) => {
                    log::warn!("sync {}: needs sign-in: {error}", account.address);
                    // A new client or token will be built on the next pass.
                    self.live.remove(&account.id);
                    self.account_status.insert(
                        account.id,
                        SyncStatus::NeedsSignIn {
                            address: account.address.clone(),
                            message: error.message(),
                        },
                    );
                    self.status = SyncStatus::NeedsSignIn {
                        address: account.address,
                        message: error.message(),
                    };
                }
                Err(SyncError::Other(error)) => {
                    log::warn!("sync {}: {error:#}", account.address);
                    self.account_status
                        .insert(account.id, SyncStatus::Offline(format!("{error:#}")));
                    if !matches!(self.status, SyncStatus::NeedsSignIn { .. }) {
                        self.status = SyncStatus::Offline(format!("{error:#}"));
                        self.account_due.insert(account.id, now + RETRY);
                    }
                }
            }
        }
        let failed = self.status != SyncStatus::UpToDate { at: now };
        self.notify_new_mail(result.backfilling, failed, cx);
        cx.emit(SyncChanged);
        cx.notify();
    }

    /// Post a notification for mail that arrived in this pass (E16.8). A first sync is history,
    /// not news, so it only moves the watermark. After a failed pass the watermark stays, so what
    /// did land is announced after the next good one.
    fn notify_new_mail(&mut self, backfilling: bool, failed: bool, cx: &mut Context<Self>) {
        let newest = self.store.max_message_id().unwrap_or(self.notified_through);
        if backfilling {
            self.notified_through = newest;
            return;
        }
        if failed {
            return;
        }
        // Mail that shows up late but is days old (a delayed delivery, an old thread relabelled
        // into the inbox) is not "new".
        let since = current_epoch() - NOTIFY_MAX_AGE_SECS;
        let fresh = self
            .store
            .new_unread_inbox(self.notified_through, since, 50)
            .unwrap_or_default();
        self.notified_through = newest;
        for notification in crate::notify::new_mail(&fresh) {
            cx.show_system_notification(notification);
        }
    }

    /// Ask the shell to open a message (a notification was clicked).
    pub fn open_message(&mut self, id: i64, cx: &mut Context<Self>) {
        cx.emit(OpenMessage(id));
    }
}

/// Mail older than this when it arrives is not announced.
const NOTIFY_MAX_AGE_SECS: i64 = 2 * 86_400;

/// One pass over every Gmail account. Runs on the background executor.
fn run_pass(job: Job, progress: &Mutex<Progress>) -> PassResult {
    let Job {
        store,
        accounts,
        rows,
        icloud_rows,
        mut live,
    } = job;
    let attempted = rows
        .iter()
        .chain(icloud_rows.iter())
        .cloned()
        .collect::<Vec<_>>();
    let mut outcomes = Vec::new();
    let mut backfilling = false;

    // iCloud Calendar is independent of the not-yet-calendar-aware IMAP path. Its app-specific
    // password comes only from the keychain and a 401 is surfaced as credential revocation.
    for row in icloud_rows {
        let outcome = match accounts.icloud_app_password(&row.address) {
            Ok(Some(password)) => {
                match snail_core::providers::caldav::CalDavClient::new(&row.address, &password) {
                    Ok(client) => snail_services::calendar_sync::sync_account(
                        &store,
                        row.id,
                        "caldav",
                        &client,
                        current_epoch(),
                    )
                    .map(|calendar| SyncReport {
                        calendar_upserted: calendar.upserted,
                        calendar_deleted: calendar.deleted,
                        calendar_conflicts: calendar.conflicts,
                        calendar_full_resyncs: calendar.full_resyncs,
                        ..Default::default()
                    })
                    .map_err(|error| {
                        if error
                            .downcast_ref::<snail_core::providers::caldav::CalDavError>()
                            .is_some_and(|error| {
                                error.kind
                                    == snail_core::providers::caldav::CalDavErrorKind::CredentialRevoked
                            })
                        {
                            SyncError::Auth(AuthError::AppPasswordRevoked)
                        } else {
                            SyncError::Other(error)
                        }
                    }),
                    Err(error) => Err(SyncError::Other(error)),
                }
            }
            Ok(None) => Err(SyncError::Auth(AuthError::AppPasswordRevoked)),
            Err(error) => Err(SyncError::Auth(AuthError::KeychainUnavailable(format!(
                "{error:#}"
            )))),
        };
        outcomes.push((row, outcome));
    }

    let oauth = match accounts.google_client() {
        Ok(Some(oauth)) => oauth,
        Ok(None) if rows.is_empty() => {
            return PassResult {
                live,
                no_client: false,
                backfilling,
                attempted,
                outcomes,
            };
        }
        Ok(None) => {
            return PassResult {
                live,
                no_client: true,
                backfilling,
                attempted,
                outcomes,
            };
        }
        Err(error) => {
            let error = AuthError::KeychainUnavailable(format!("{error:#}"));
            outcomes.extend(
                rows.into_iter()
                    .map(|row| (row, Err(SyncError::Auth(error.clone())))),
            );
            return PassResult {
                live,
                no_client: false,
                backfilling,
                attempted,
                outcomes,
            };
        }
    };
    let options = SyncOptions::default();
    for row in rows {
        let account = match live.get(&row.id) {
            Some(account) => account.clone(),
            None => match GmailAccount::new(
                &row,
                GoogleTokens::new(oauth.clone(), accounts.clone(), &row.address),
            ) {
                Ok(account) => {
                    let account = Arc::new(account);
                    live.insert(row.id, account.clone());
                    account
                }
                Err(error) => {
                    outcomes.push((row, Err(SyncError::Other(error))));
                    continue;
                }
            },
        };
        let first = store
            .get_sync_state(row.id, "gmail", "")
            .ok()
            .flatten()
            .is_none_or(|state| state.history_id.is_none() || state.full_resync_needed);
        progress.lock().unwrap().first = first;
        backfilling |= first;
        let outcome = account.sync(&store, &options, &mut |done, total| {
            let mut progress = progress.lock().unwrap();
            progress.done = done;
            progress.total = total;
            progress.batches += 1;
        });
        outcomes.push((row, outcome));
    }
    PassResult {
        live,
        no_client: false,
        backfilling,
        attempted,
        outcomes,
    }
}

fn current_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
