//! Mail reads for the shell (plan.md E5). A **model**, not a view: it is the only module in the app
//! crate that touches the store, which is what keeps `store::tests::no_view_module_reaches_the_store`
//! honest.
//!
//! First cut: these reads are local and fast, so they run inline. E5.12 moves them onto the
//! background executor with a generation guard (§2).

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use snail_core::mime::{BodyPreference, ParsedMessage};
use snail_core::pgp::{KeyRing, KeySummary, VerificationState};
use snail_core::pgp_mime::CryptoEnvelope;
use snail_core::store::Store;
use snail_services::pgp::PassphraseManager;
use snail_services::pgp::{DiscoveryResult, WkdClient};
use snail_services::secrets::KeyringStore;

// The shell sees these through the model, so it never names the store.
pub use snail_core::store::{MailboxRow, MessageRow, SearchHit};

#[derive(Clone)]
pub struct MailModel {
    store: Arc<Store>,
    pgp: Arc<Mutex<KeyRing>>,
    pgp_passphrases: Arc<PassphraseManager>,
}

pub struct CryptoReading {
    pub state: VerificationState,
    /// Decrypted RFC822/MIME entity. This value exists in task/UI memory only and never reaches
    /// `Store`, `Cache`, draft autosave, or FTS.
    pub plaintext: Option<Vec<u8>>,
}

impl MailModel {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            pgp: Arc::new(Mutex::new(KeyRing::new())),
            pgp_passphrases: Arc::new(PassphraseManager::with_keychain(Arc::new(
                KeyringStore::new(),
            ))),
        }
    }

    pub fn import_pgp_key(&self, path: &Path) -> anyhow::Result<KeySummary> {
        self.pgp.lock().unwrap().import_file(path)
    }

    /// WKD is a discovery convention, not a trust signal. The client applies the draft's DNS-only
    /// fallback rule and this import still validates the certificate binding chain.
    pub fn discover_pgp_key(&self, email: &str) -> anyhow::Result<Option<KeySummary>> {
        match WkdClient::system()?.lookup(email)? {
            DiscoveryResult::Found(bytes) => self
                .pgp
                .lock()
                .unwrap()
                .import_bytes(format!("wkd-{email}.pgp").into(), &bytes)
                .map(Some),
            DiscoveryResult::NotFound => Ok(None),
        }
    }

    pub fn pgp_keys(&self) -> Vec<KeySummary> {
        self.pgp.lock().unwrap().list()
    }

    pub fn signature_for(&self, address: Option<&str>) -> Option<String> {
        let address = address?.trim();
        if address.is_empty() {
            return None;
        }
        for account in self.store.accounts().unwrap_or_default() {
            for identity in self.store.identities(account.id).unwrap_or_default() {
                if identity.address.eq_ignore_ascii_case(address)
                    && let Some(signature) = identity.signature
                    && !signature.trim().is_empty()
                {
                    return Some(signature);
                }
            }
        }
        None
    }

    pub fn missing_pgp_encryption_keys(&self, recipients: &[String]) -> Vec<String> {
        self.pgp
            .lock()
            .unwrap()
            .missing_encryption_keys(recipients.iter().map(String::as_str), SystemTime::now())
    }

    /// Apply RFC 3156 signing and/or encryption. This is CPU work and must be called from a
    /// background executor. Missing recipient keys are checked before any message is queued.
    pub fn protect_outbound(
        &self,
        raw: &[u8],
        recipients: &[String],
        sign: bool,
        encrypt: bool,
    ) -> anyhow::Result<Vec<u8>> {
        if !sign && !encrypt {
            return Ok(raw.to_vec());
        }
        let (outer, mut entity) = snail_core::pgp_mime::split_rfc822(raw)?;
        let ring = self.pgp.lock().unwrap();
        if sign {
            let secret = ring
                .list()
                .into_iter()
                .find(|key| key.secret)
                .ok_or_else(|| anyhow::anyhow!("no secret OpenPGP key is imported"))?;
            let passphrase = self
                .pgp_passphrases
                .resolve(&secret.fingerprint)?
                .unwrap_or_default();
            let signature = ring.sign_detached(&entity, &secret.fingerprint, &passphrase)?;
            let boundary = format!("snail-signed-{:016x}", rand::random::<u64>());
            entity = snail_core::pgp_mime::multipart_signed(&entity, &signature, &boundary)?;
        }
        if encrypt {
            let missing = ring
                .missing_encryption_keys(recipients.iter().map(String::as_str), SystemTime::now());
            if !missing.is_empty() {
                anyhow::bail!("key missing for {}", missing.join(", "));
            }
            let encrypted = ring.encrypt_for(
                &entity,
                recipients.iter().map(String::as_str),
                SystemTime::now(),
            )?;
            let boundary = format!("snail-encrypted-{:016x}", rand::random::<u64>());
            entity = snail_core::pgp_mime::multipart_encrypted(&encrypted, &boundary)?;
        }
        Ok(snail_core::pgp_mime::join_rfc822(outer, &entity))
    }

    pub fn unlock_pgp_key(
        &self,
        fingerprint: &str,
        passphrase: String,
        remember: bool,
    ) -> anyhow::Result<()> {
        self.pgp_passphrases
            .unlock(fingerprint, passphrase, remember)
    }

    /// Analyze one raw message. Callers must use a background executor; intentionally no store
    /// write is performed even when decryption succeeds.
    pub fn analyze_pgp(&self, raw: &[u8]) -> CryptoReading {
        // Attached certificates are discovery candidates. `import_bytes` checks the complete
        // binding chain before they can enter the local keyring.
        for (index, attached) in snail_core::pgp_mime::attached_public_keys(raw)
            .into_iter()
            .enumerate()
        {
            let source = format!("attached-mail-key-{index}.asc").into();
            if let Err(error) = self.pgp.lock().unwrap().import_bytes(source, &attached) {
                log::warn!("ignored invalid attached OpenPGP key: {error:#}");
            }
        }

        let now = SystemTime::now();
        match snail_core::pgp_mime::envelope(raw) {
            CryptoEnvelope::None => CryptoReading {
                state: VerificationState::NotSigned,
                plaintext: None,
            },
            CryptoEnvelope::DetachedSigned { entity, signature } => CryptoReading {
                state: self.pgp.lock().unwrap().verify_detached(
                    &snail_core::pgp_mime::canonicalize_signed_entity(&entity),
                    &signature,
                    now,
                ),
                plaintext: None,
            },
            CryptoEnvelope::InlineSigned(armored) => CryptoReading {
                state: self.pgp.lock().unwrap().verify_inline(&armored, now),
                plaintext: None,
            },
            CryptoEnvelope::Encrypted(ciphertext) | CryptoEnvelope::InlineEncrypted(ciphertext) => {
                self.decrypt_pgp_payload(&ciphertext)
            }
        }
    }

    fn decrypt_pgp_payload(&self, ciphertext: &[u8]) -> CryptoReading {
        let summaries = self.pgp.lock().unwrap().list();
        if !summaries.iter().any(|key| key.secret) {
            return CryptoReading {
                state: VerificationState::Encrypted {
                    decrypted: false,
                    detail: "Encrypted · import the matching secret key".into(),
                },
                plaintext: None,
            };
        }

        // Unprotected secret keys work without a prompt. Protected keys use the per-session cache
        // or, when opted into previously, the OS keychain.
        if let Ok(plaintext) = self.pgp.lock().unwrap().decrypt(ciphertext, "") {
            return CryptoReading {
                state: VerificationState::Encrypted {
                    decrypted: true,
                    detail: "Encrypted · decrypted in memory".into(),
                },
                plaintext: Some(plaintext.as_bytes().to_vec()),
            };
        }
        let mut needs_prompt = false;
        for key in summaries.iter().filter(|key| key.secret) {
            match self.pgp_passphrases.resolve(&key.fingerprint) {
                Ok(Some(passphrase)) => {
                    if let Ok(plaintext) = self.pgp.lock().unwrap().decrypt(ciphertext, &passphrase)
                    {
                        return CryptoReading {
                            state: VerificationState::Encrypted {
                                decrypted: true,
                                detail: "Encrypted · decrypted in memory".into(),
                            },
                            plaintext: Some(plaintext.as_bytes().to_vec()),
                        };
                    }
                }
                Ok(None) => needs_prompt = true,
                Err(error) => log::warn!("could not read PGP passphrase from keychain: {error:#}"),
            }
        }
        CryptoReading {
            state: if needs_prompt {
                VerificationState::Encrypted {
                    decrypted: false,
                    detail: "Encrypted · passphrase required".into(),
                }
            } else {
                VerificationState::DecryptionFailed {
                    detail: "Could not decrypt this message".into(),
                }
            },
            plaintext: None,
        }
    }
    pub fn mailboxes(&self) -> Vec<MailboxRow> {
        self.store.enabled_mailboxes().unwrap_or_default()
    }

    pub fn page(&self, mailbox_id: i64, limit: u32) -> Vec<MessageRow> {
        self.store.page(mailbox_id, limit, 0).unwrap_or_default()
    }

    pub fn message(&self, id: i64) -> Option<MessageRow> {
        self.store.message(id).ok().flatten()
    }

    /// Parse a message's cached raw MIME. `None` means the cache file is gone (re-fetch is E2.3).
    pub fn parsed_with_preference(
        &self,
        id: i64,
        preference: BodyPreference,
    ) -> Option<ParsedMessage> {
        let raw = self.store.raw_bytes(id).ok().flatten()?;
        snail_core::mime::parse_raw_with_preference(&raw, preference).ok()
    }

    /// The cached raw MIME bytes, for resolving inline `cid:` images (E6.8).
    pub fn raw(&self, id: i64) -> Option<Vec<u8>> {
        self.store.raw_bytes(id).ok().flatten()
    }

    /// Parse a cached message for reply/forward derivation (E7.4).
    pub fn parsed(&self, id: i64) -> Option<ParsedMessage> {
        snail_core::mime::parse_raw(&self.raw(id)?).ok()
    }

    /// The first account's address, to exclude the user from a reply-all.
    pub fn first_account_address(&self) -> Option<String> {
        self.store
            .accounts()
            .ok()?
            .first()
            .map(|account| account.address.clone())
    }

    /// Apply a triage action optimistically and queue it (E8.4). Returns the op id for undo.
    pub fn triage(
        &self,
        message_id: i64,
        action: &snail_core::triage::TriageAction,
    ) -> Option<i64> {
        let now = now_epoch();
        let key = format!("{}:{}:{now}", action.operation(), message_id);
        self.store.apply_triage(message_id, action, &key, now).ok()
    }

    /// Undo a triage op (E8.5): restore the row and cancel or invert the queued op.
    pub fn undo_triage(&self, op_id: i64) -> bool {
        self.store.undo_triage(op_id, now_epoch()).unwrap_or(false)
    }

    /// Put back any triage op the provider rejected (E8.4); returns how many were rolled back.
    pub fn rollback_failed_triage(&self) -> usize {
        self.store.rollback_failed_triage(now_epoch()).unwrap_or(0)
    }

    /// Fix previews stored with CSS in them (once per store). Run off the main thread.
    pub fn repair_previews(&self) -> usize {
        self.store.repair_html_previews().unwrap_or_else(|error| {
            log::warn!("preview repair failed: {error:#}");
            0
        })
    }

    /// The mailbox a message is filed in, to open it from a notification.
    pub fn mailbox_of(&self, message_id: i64) -> Option<i64> {
        self.store.message_mailbox(message_id).ok().flatten()
    }

    pub fn thread_id_of(&self, message_id: i64) -> Option<i64> {
        self.store.thread_id_of(message_id).ok().flatten()
    }

    pub fn thread_messages(&self, thread_id: i64) -> Vec<MessageRow> {
        self.store.messages_in_thread(thread_id).unwrap_or_default()
    }

    /// One row per thread, for the "group by thread" list (E8.3).
    pub fn thread_page(&self, mailbox_id: i64, limit: u32) -> Vec<snail_core::store::ThreadRow> {
        self.store
            .thread_page(mailbox_id, limit)
            .unwrap_or_default()
    }

    /// Address autocomplete from the harvested contacts (E7.3).
    pub fn suggest_contacts(&self, prefix: &str, limit: u32) -> Vec<snail_core::store::ContactRow> {
        let Some(account_id) = self.first_account_id() else {
            return Vec::new();
        };
        self.store
            .suggest_contacts(account_id, prefix, limit)
            .unwrap_or_default()
    }

    /// Full-text search, local only (E9.1/E9.5). Runs on the background executor in the shell.
    pub fn search(&self, query: &snail_core::store::SearchQuery, limit: u32) -> Vec<SearchHit> {
        self.store.search(query, limit).unwrap_or_default()
    }

    /// Translate the parsed UI query into the store's criteria (the two crates do not depend on
    /// each other, so the bin is where they meet).
    pub fn search_criteria(query: &snail_ui::search::Query) -> snail_core::store::SearchQuery {
        snail_core::store::SearchQuery {
            terms: query.terms.clone(),
            phrases: query.phrases.clone(),
            from: query.from.clone(),
            to: query.to.clone(),
            subject: query.subject.clone(),
            mailbox: query.mailbox.clone(),
            unread: query.unread,
            has_attachment: query.has_attachment,
            after: query.after,
            before: query.before,
        }
    }

    /// Rebuild the contacts table from the messages already on disk (E7.3).
    pub fn harvest_contacts(&self) -> Option<usize> {
        self.store.harvest_contacts(self.first_account_id()?).ok()
    }

    /// Pending send count, for the sync footer's "N changes pending" (E16.6/E7.12).
    pub fn pending_sends(&self) -> i64 {
        self.store
            .op_counts()
            .map(|counts| counts.outstanding() + counts.dead)
            .unwrap_or(0)
    }

    /// `(failed_or_dead, pending)` send counts for the sidebar footer (E7.12).
    pub fn send_queue(&self) -> (i64, i64) {
        let counts = self.store.op_counts().unwrap_or_default();
        (counts.failed + counts.dead, counts.pending)
    }

    /// Explicit retry: move failed and dead-lettered sends back to pending (E7.12).
    pub fn retry_failed_sends(&self) -> usize {
        self.store.requeue_failed_ops().unwrap_or(0)
    }

    /// The first account, for a first-cut send (E7.11 will pick by identity).
    pub fn first_account_id(&self) -> Option<i64> {
        self.store
            .accounts()
            .ok()?
            .first()
            .map(|account| account.id)
    }

    /// Autosave a draft (E7.6): the raw message goes to the cache, the row to the Drafts mailbox.
    /// Returns the message id to reuse on the next autosave.
    pub fn save_draft(
        &self,
        draft_id: Option<i64>,
        subject: &str,
        body_text: &str,
        to_json: &str,
        raw: &[u8],
    ) -> Option<i64> {
        let account_id = self.first_account_id()?;
        let raw_hash = self.store.cache().put(raw).ok();
        let now = now_epoch();
        self.store
            .save_draft(
                draft_id,
                account_id,
                subject,
                body_text,
                to_json,
                raw_hash.as_deref(),
                now,
            )
            .ok()
    }

    /// Discard a draft (a sent message, or an explicit delete).
    pub fn delete_message(&self, id: i64) -> bool {
        self.store.delete_message(id).is_ok()
    }

    /// Queue an outbound message (E7.10): the raw bytes go to the cache, and the `pending_op` holds
    /// the hash so nothing large sits in SQLite. Blocked by default (after the undo window).
    pub fn enqueue_send(&self, raw: &[u8]) -> Option<String> {
        let account_id = self.first_account_id()?;
        let hash = self.store.cache().put(raw).ok()?;
        self.store
            .enqueue_op(&snail_core::store::NewOp {
                account_id,
                target_kind: "message".into(),
                target_id: None,
                operation: "send".into(),
                payload_json: Some(format!("{{\"raw_hash\":\"{hash}\"}}")),
                idempotency_key: format!("send:{hash}"),
                now: now_epoch(),
            })
            .ok()?;
        Some(hash)
    }

    /// Fetch a remote image, served from the content-addressed cache when it has been seen (E6.8).
    /// Runs on whatever thread calls it — the shell calls it from the background executor.
    pub fn fetch_remote_image(&self, url: &str) -> Option<Vec<u8>> {
        if let Ok(Some(cached)) = self.store.cache().get_alias(url) {
            return Some(cached);
        }
        let response = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .ok()?
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "gzip")
            .header(reqwest::header::USER_AGENT, "snail/0.1 (gzip)")
            .send()
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let bytes = response.bytes().ok()?.to_vec();
        if bytes.len() > 10 * 1024 * 1024 {
            return None;
        }
        let _ = self.store.cache().put_alias(url, &bytes);
        Some(bytes)
    }
}

fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use snail_core::store::NewIdentity;

    #[test]
    fn signatures_are_resolved_per_identity_case_insensitively() {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let account = store
            .insert_account("gmail", "me@example.com", Some("Me"), 0)
            .unwrap();
        store
            .insert_identity(&NewIdentity {
                account_id: account,
                address: "Alias@Example.com".into(),
                display_name: Some("Alias".into()),
                signature: Some("Regards,\nAlias".into()),
                is_default: true,
            })
            .unwrap();
        let model = MailModel::new(store);
        assert_eq!(
            model.signature_for(Some("alias@example.COM")).as_deref(),
            Some("Regards,\nAlias")
        );
        assert_eq!(model.signature_for(Some("other@example.com")), None);
    }
}
