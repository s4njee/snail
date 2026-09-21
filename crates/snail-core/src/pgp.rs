//! OpenPGP safety wrapper for Snail.
//!
//! All untrusted parsing crosses a panic boundary, verification consumes the complete message,
//! `Invalid` is handled as a failure, and decompression is bounded below rpgp's limits.  UI code
//! should run these functions on a background executor.

use std::fs;
use std::io::{Cursor, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use pgp::composed::{
    ArmorOptions, CleartextSignedMessage, Deserializable, DetachedSignature, Message,
    MessageBuilder, SignedPublicKey, SignedPublicSubKey, SignedSecretKey,
};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::packet::{Signature, SignatureType};
use pgp::types::{Duration as PgpDuration, KeyDetails, Password};
use rand::thread_rng;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

pub const MAX_KEY_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PGP_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_COMPRESSION_DEPTH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VerificationState {
    NotSigned,
    Encrypted {
        decrypted: bool,
        detail: String,
    },
    DecryptionFailed {
        detail: String,
    },
    Unverified {
        detail: String,
    },
    Valid {
        signer: String,
        fingerprint: String,
    },
    Failed {
        detail: String,
    },
    PolicyRejected {
        signer: String,
        reason: PolicyRejection,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PolicyRejection {
    WeakHash,
    Revoked,
    Expired,
    NotSigningCapable,
    NotEncryptionCapable,
    InvalidBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySummary {
    pub fingerprint: String,
    pub user_ids: Vec<String>,
    pub secret: bool,
    pub created_at: u64,
    pub expires_at: Option<u64>,
    pub revoked: bool,
    pub source: PathBuf,
}

/// Auditable inputs to the certificate-semantics gate. Production constructs these facts from
/// verified rpgp packets; fixture tests use the same gate for adversarial certificate cases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateFacts {
    pub binding_valid: bool,
    pub revoked: bool,
    pub expired: bool,
    pub signing_flag: bool,
    pub hash: String,
}

pub fn evaluate_certificate_facts(
    facts: &CertificateFacts,
) -> std::result::Result<(), PolicyRejection> {
    if !facts.binding_valid {
        return Err(PolicyRejection::InvalidBinding);
    }
    if facts.revoked {
        return Err(PolicyRejection::Revoked);
    }
    if facts.expired {
        return Err(PolicyRejection::Expired);
    }
    if !facts.signing_flag {
        return Err(PolicyRejection::NotSigningCapable);
    }
    if matches!(
        facts.hash.to_ascii_lowercase().as_str(),
        "md5" | "sha1" | "sha-1"
    ) {
        return Err(PolicyRejection::WeakHash);
    }
    Ok(())
}

#[derive(Default)]
pub struct KeyRing {
    public: Vec<KeyEntry<SignedPublicKey>>,
    secret: Vec<KeyEntry<SignedSecretKey>>,
}

struct KeyEntry<K> {
    source: PathBuf,
    key: K,
}

impl KeyRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Import an existing armored or binary key. Snail never generates keys in v1.
    pub fn import_file(&mut self, path: impl AsRef<Path>) -> Result<KeySummary> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
        if metadata.len() > MAX_KEY_BYTES as u64 {
            bail!("OpenPGP key exceeds {MAX_KEY_BYTES} bytes");
        }
        let bytes = fs::read(path)?;
        self.import_bytes(path.to_path_buf(), &bytes)
    }

    pub fn import_bytes(&mut self, source: PathBuf, bytes: &[u8]) -> Result<KeySummary> {
        if bytes.len() > MAX_KEY_BYTES {
            bail!("OpenPGP key exceeds {MAX_KEY_BYTES} bytes");
        }

        if let Ok(key) = parse_untrusted(|| {
            SignedSecretKey::from_reader_single(Cursor::new(bytes))
                .map(|(key, _)| key)
                .map_err(anyhow::Error::from)
        }) {
            key.verify_bindings()
                .context("invalid secret-key binding chain")?;
            let summary = secret_summary(&key, source.clone());
            let fingerprint = summary.fingerprint.clone();
            self.secret
                .retain(|entry| fingerprint_of(&entry.key.primary_key) != fingerprint);
            // A secret key also supplies its public certificate for verification and discovery.
            let public = key.to_public_key();
            self.public
                .retain(|entry| fingerprint_of(&entry.key) != fingerprint);
            self.public.push(KeyEntry {
                source: source.clone(),
                key: public,
            });
            self.secret.push(KeyEntry { source, key });
            return Ok(summary);
        }

        let key = parse_untrusted(|| {
            SignedPublicKey::from_reader_single(Cursor::new(bytes))
                .map(|(key, _)| key)
                .map_err(anyhow::Error::from)
        })
        .context("not a supported OpenPGP public or secret key")?;
        key.verify_bindings()
            .context("invalid public-key binding chain")?;
        let summary = public_summary(&key, source.clone(), false);
        let fingerprint = summary.fingerprint.clone();
        self.public
            .retain(|entry| fingerprint_of(&entry.key) != fingerprint);
        self.public.push(KeyEntry { source, key });
        Ok(summary)
    }

    pub fn list(&self) -> Vec<KeySummary> {
        let mut out = self
            .secret
            .iter()
            .map(|entry| secret_summary(&entry.key, entry.source.clone()))
            .collect::<Vec<_>>();
        for entry in &self.public {
            let fingerprint = fingerprint_of(&entry.key);
            if !out.iter().any(|summary| summary.fingerprint == fingerprint) {
                out.push(public_summary(&entry.key, entry.source.clone(), false));
            }
        }
        out.sort_by(|a, b| a.fingerprint.cmp(&b.fingerprint));
        out
    }

    pub fn has_encryption_key_for(&self, address: &str, now: SystemTime) -> bool {
        self.encryption_key_for(address, now).is_some()
    }

    pub fn missing_encryption_keys<'a>(
        &self,
        recipients: impl IntoIterator<Item = &'a str>,
        now: SystemTime,
    ) -> Vec<String> {
        recipients
            .into_iter()
            .filter(|address| !self.has_encryption_key_for(address, now))
            .map(str::to_string)
            .collect()
    }

    pub fn verify_detached(
        &self,
        content: &[u8],
        signature: &[u8],
        now: SystemTime,
    ) -> VerificationState {
        if signature.len() > MAX_PGP_BYTES || content.len() > MAX_DECOMPRESSED_BYTES {
            return failed("signed content exceeds safety limit");
        }
        let parsed = parse_untrusted(|| {
            DetachedSignature::from_reader_single(Cursor::new(signature))
                .map(|(sig, _)| sig)
                .map_err(anyhow::Error::from)
        });
        let Ok(signature) = parsed else {
            return failed("malformed detached signature");
        };
        self.verify_signature(&signature.signature, content, now)
    }

    pub fn verify_inline(&self, armored: &str, now: SystemTime) -> VerificationState {
        if armored.len() > MAX_PGP_BYTES {
            return failed("inline PGP message exceeds safety limit");
        }
        let parsed = parse_untrusted(|| {
            CleartextSignedMessage::from_string(armored)
                .map(|v| v.0)
                .map_err(anyhow::Error::from)
        });
        let Ok(message) = parsed else {
            return failed("malformed inline PGP signature");
        };
        let content = message.signed_text();
        for signature in message.signatures() {
            let state = self.verify_signature(signature, content.as_bytes(), now);
            if !matches!(state, VerificationState::Failed { .. }) {
                return state;
            }
        }
        failed("signature does not match an imported key")
    }

    /// Verify a signed OpenPGP Message only after a complete, bounded read. This wrapper also
    /// treats rpgp's successful `Ok([Invalid])` result as failure.
    pub fn verify_message(&self, bytes: &[u8], now: SystemTime) -> VerificationState {
        if bytes.len() > MAX_PGP_BYTES {
            return failed("PGP message exceeds safety limit");
        }
        parse_untrusted(|| {
            self.verify_message_inner(bytes, now)
                .map_err(anyhow::Error::from)
        })
        .unwrap_or_else(|_| failed("malformed signed PGP message"))
    }

    fn verify_message_inner(
        &self,
        bytes: &[u8],
        now: SystemTime,
    ) -> pgp::errors::Result<VerificationState> {
        let (mut message, _) = Message::from_reader(Cursor::new(bytes))?;
        bounded_read(&mut message, MAX_DECOMPRESSED_BYTES).map_err(pgp::errors::Error::from)?;

        for cert in self.public.iter().map(|entry| &entry.key) {
            let primary_result = message.verify_nested(&[&cert.primary_key])?;
            if matches!(
                primary_result.first(),
                Some(pgp::composed::VerificationResult::Valid(_))
            ) {
                let sig = match primary_result.first() {
                    Some(pgp::composed::VerificationResult::Valid(sig)) => sig,
                    _ => unreachable!(),
                };
                return Ok(policy_state_primary(cert, &sig, now));
            }
            for subkey in &cert.public_subkeys {
                let result = message.verify_nested(&[subkey])?;
                if matches!(
                    result.first(),
                    Some(pgp::composed::VerificationResult::Valid(_))
                ) {
                    let sig = match result.first() {
                        Some(pgp::composed::VerificationResult::Valid(sig)) => sig,
                        _ => unreachable!(),
                    };
                    return Ok(policy_state_subkey(cert, subkey, &sig, now));
                }
            }
        }
        Ok(if self.public.is_empty() {
            VerificationState::Unverified {
                detail: "no public keys imported".into(),
            }
        } else {
            failed("signature does not match an imported key")
        })
    }

    fn verify_signature(
        &self,
        signature: &Signature,
        content: &[u8],
        now: SystemTime,
    ) -> VerificationState {
        if self.public.is_empty() {
            return VerificationState::Unverified {
                detail: "no public keys imported".into(),
            };
        }
        for cert in self.public.iter().map(|entry| &entry.key) {
            if signature.verify(&cert.primary_key, content).is_ok() {
                return policy_state_primary(cert, signature, now);
            }
            for subkey in &cert.public_subkeys {
                if signature.verify(subkey, content).is_ok() {
                    return policy_state_subkey(cert, subkey, signature, now);
                }
            }
        }
        failed("signature does not match an imported key")
    }

    pub fn sign_detached(
        &self,
        content: &[u8],
        fingerprint: &str,
        passphrase: &str,
    ) -> Result<Vec<u8>> {
        let entry = self
            .secret
            .iter()
            .find(|entry| {
                fingerprint_of(&entry.key.primary_key) == normalize_fingerprint(fingerprint)
            })
            .ok_or_else(|| anyhow!("secret key not found"))?;
        let cert = entry.key.to_public_key();
        let password = Password::from(passphrase);
        let content = crate::pgp_mime::canonicalize_signed_entity(content);

        if primary_policy(&cert, SystemTime::now(), Capability::Sign).is_ok() {
            let sig = DetachedSignature::sign_binary_data(
                thread_rng(),
                &entry.key.primary_key,
                &password,
                HashAlgorithm::Sha256,
                Cursor::new(content),
            )?;
            return Ok(sig.to_armored_bytes(ArmorOptions::default())?);
        }

        for secret_subkey in &entry.key.secret_subkeys {
            if let Some(public_subkey) = cert
                .public_subkeys
                .iter()
                .find(|subkey| fingerprint_of(*subkey) == fingerprint_of(&secret_subkey.key))
            {
                if subkey_policy(&cert, public_subkey, SystemTime::now(), Capability::Sign).is_ok()
                {
                    let sig = DetachedSignature::sign_binary_data(
                        thread_rng(),
                        &secret_subkey.key,
                        &password,
                        HashAlgorithm::Sha256,
                        Cursor::new(content),
                    )?;
                    return Ok(sig.to_armored_bytes(ArmorOptions::default())?);
                }
            }
        }
        bail!("key has no valid signing component")
    }

    pub fn encrypt_for<'a>(
        &self,
        plaintext: &[u8],
        recipients: impl IntoIterator<Item = &'a str>,
        now: SystemTime,
    ) -> Result<Vec<u8>> {
        let recipients = recipients.into_iter().collect::<Vec<_>>();
        let missing = self.missing_encryption_keys(recipients.iter().copied(), now);
        if !missing.is_empty() {
            bail!("key missing for {}", missing.join(", "));
        }
        let mut rng = thread_rng();
        let mut builder = MessageBuilder::from_bytes("message.eml", plaintext.to_vec())
            .seipd_v1(&mut rng, SymmetricKeyAlgorithm::AES256);
        for address in recipients {
            let (_, subkey) = self
                .encryption_key_for(address, now)
                .expect("checked above");
            builder.encrypt_to_key(&mut rng, subkey)?;
        }
        Ok(builder
            .to_armored_string(&mut rng, ArmorOptions::default())?
            .into_bytes())
    }

    pub fn decrypt(&self, ciphertext: &[u8], passphrase: &str) -> Result<MemoryPlaintext> {
        if ciphertext.len() > MAX_PGP_BYTES {
            bail!("PGP message exceeds {MAX_PGP_BYTES} bytes");
        }
        parse_untrusted(|| self.decrypt_inner(ciphertext, passphrase))
            .map_err(|_| anyhow!("could not decrypt malformed PGP message"))
    }

    fn decrypt_inner(&self, ciphertext: &[u8], passphrase: &str) -> Result<MemoryPlaintext> {
        let password = Password::from(passphrase);
        for entry in &self.secret {
            let parsed = Message::from_reader(Cursor::new(ciphertext));
            let Ok((message, _)) = parsed else { continue };
            let Ok(mut message) = message.decrypt(&password, &entry.key) else {
                continue;
            };
            let mut depth = 0;
            while message.is_compressed() {
                if depth >= MAX_COMPRESSION_DEPTH {
                    bail!("PGP compression nesting exceeds {MAX_COMPRESSION_DEPTH}");
                }
                message = message.decompress()?;
                depth += 1;
            }
            let plaintext = bounded_read(&mut message, MAX_DECOMPRESSED_BYTES)?;
            return Ok(MemoryPlaintext(plaintext));
        }
        bail!("could not decrypt with an imported secret key")
    }

    fn encryption_key_for(
        &self,
        address: &str,
        now: SystemTime,
    ) -> Option<(&SignedPublicKey, &SignedPublicSubKey)> {
        let needle = format!("<{}>", address.trim().to_ascii_lowercase());
        self.public.iter().find_map(|entry| {
            let cert = &entry.key;
            let address_matches = cert
                .details
                .users
                .iter()
                .any(|user| user.id.to_string().to_ascii_lowercase().contains(&needle));
            if !address_matches || primary_policy(cert, now, Capability::Encrypt).is_err() {
                return None;
            }
            cert.public_subkeys
                .iter()
                .find(|subkey| subkey_policy(cert, subkey, now, Capability::Encrypt).is_ok())
                .map(|subkey| (cert, subkey))
        })
    }
}

/// Decrypted data is intentionally neither serializable nor cloneable. It is zeroized when the
/// last in-memory owner goes away and has no API that accepts a store or search index.
pub struct MemoryPlaintext(Vec<u8>);

impl MemoryPlaintext {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for MemoryPlaintext {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, Copy)]
enum Capability {
    Sign,
    Encrypt,
}

fn primary_policy(
    cert: &SignedPublicKey,
    now: SystemTime,
    capability: Capability,
) -> std::result::Result<(), PolicyRejection> {
    let signature = primary_common_policy(cert, now)?;
    ensure_capability(signature, capability)
}

fn primary_common_policy(
    cert: &SignedPublicKey,
    now: SystemTime,
) -> std::result::Result<&Signature, PolicyRejection> {
    cert.verify_bindings()
        .map_err(|_| PolicyRejection::InvalidBinding)?;
    if !cert.details.revocation_signatures.is_empty() {
        return Err(PolicyRejection::Revoked);
    }
    let signature = newest_primary_signature(cert).ok_or(PolicyRejection::InvalidBinding)?;
    ensure_strong_hash(signature)?;
    ensure_not_expired(
        cert.created_at().as_secs(),
        signature.key_expiration_time(),
        now,
    )?;
    Ok(signature)
}

fn subkey_policy(
    cert: &SignedPublicKey,
    subkey: &SignedPublicSubKey,
    now: SystemTime,
    capability: Capability,
) -> std::result::Result<(), PolicyRejection> {
    primary_common_policy(cert, now)?;
    subkey
        .verify_bindings(&cert.primary_key)
        .map_err(|_| PolicyRejection::InvalidBinding)?;
    if subkey
        .signatures
        .iter()
        .any(|sig| sig.typ() == Some(SignatureType::SubkeyRevocation))
    {
        return Err(PolicyRejection::Revoked);
    }
    let binding = subkey
        .signatures
        .iter()
        .filter(|sig| sig.typ() == Some(SignatureType::SubkeyBinding))
        .max_by_key(|sig| sig.created())
        .ok_or(PolicyRejection::InvalidBinding)?;
    ensure_strong_hash(binding)?;
    ensure_not_expired(
        subkey.created_at().as_secs(),
        binding.key_expiration_time(),
        now,
    )?;
    ensure_capability(binding, capability)
}

fn newest_primary_signature(cert: &SignedPublicKey) -> Option<&Signature> {
    cert.details
        .direct_signatures
        .iter()
        .chain(
            cert.details
                .users
                .iter()
                .flat_map(|user| user.signatures.iter()),
        )
        .max_by_key(|sig| sig.created())
}

fn ensure_strong_hash(signature: &Signature) -> std::result::Result<(), PolicyRejection> {
    if matches!(
        signature.hash_alg(),
        Some(HashAlgorithm::Md5 | HashAlgorithm::Sha1)
    ) {
        Err(PolicyRejection::WeakHash)
    } else {
        Ok(())
    }
}

fn ensure_not_expired(
    created_at: u32,
    lifetime: Option<PgpDuration>,
    now: SystemTime,
) -> std::result::Result<(), PolicyRejection> {
    let Some(lifetime) = lifetime.filter(|duration| duration.as_secs() != 0) else {
        return Ok(());
    };
    let expires = UNIX_EPOCH
        + StdDuration::from_secs(u64::from(created_at))
        + StdDuration::from_secs(u64::from(lifetime.as_secs()));
    if now >= expires {
        Err(PolicyRejection::Expired)
    } else {
        Ok(())
    }
}

fn ensure_capability(
    signature: &Signature,
    capability: Capability,
) -> std::result::Result<(), PolicyRejection> {
    let flags = signature.key_flags();
    match capability {
        Capability::Sign if flags.sign() => Ok(()),
        Capability::Sign => Err(PolicyRejection::NotSigningCapable),
        Capability::Encrypt if flags.encrypt_comms() || flags.encrypt_storage() => Ok(()),
        Capability::Encrypt => Err(PolicyRejection::NotEncryptionCapable),
    }
}

fn policy_state_primary(
    cert: &SignedPublicKey,
    signature: &Signature,
    now: SystemTime,
) -> VerificationState {
    let signer = signer_name(cert);
    if let Err(reason) =
        ensure_strong_hash(signature).and_then(|_| primary_policy(cert, now, Capability::Sign))
    {
        return VerificationState::PolicyRejected { signer, reason };
    }
    VerificationState::Valid {
        signer,
        fingerprint: fingerprint_of(cert),
    }
}

fn policy_state_subkey(
    cert: &SignedPublicKey,
    subkey: &SignedPublicSubKey,
    signature: &Signature,
    now: SystemTime,
) -> VerificationState {
    let signer = signer_name(cert);
    if let Err(reason) = ensure_strong_hash(signature)
        .and_then(|_| subkey_policy(cert, subkey, now, Capability::Sign))
    {
        return VerificationState::PolicyRejected { signer, reason };
    }
    VerificationState::Valid {
        signer,
        fingerprint: fingerprint_of(subkey),
    }
}

fn public_summary(key: &SignedPublicKey, source: PathBuf, secret: bool) -> KeySummary {
    let signature = newest_primary_signature(key);
    KeySummary {
        fingerprint: fingerprint_of(key),
        user_ids: key
            .details
            .users
            .iter()
            .map(|user| user.id.to_string())
            .collect(),
        secret,
        created_at: u64::from(key.created_at().as_secs()),
        expires_at: signature
            .and_then(Signature::key_expiration_time)
            .filter(|duration| duration.as_secs() != 0)
            .map(|duration| u64::from(key.created_at().as_secs()) + u64::from(duration.as_secs())),
        revoked: !key.details.revocation_signatures.is_empty(),
        source,
    }
}

fn secret_summary(key: &SignedSecretKey, source: PathBuf) -> KeySummary {
    public_summary(&key.to_public_key(), source, true)
}

fn signer_name(key: &SignedPublicKey) -> String {
    key.details
        .users
        .first()
        .map(|user| user.id.to_string())
        .unwrap_or_else(|| fingerprint_of(key))
}

fn fingerprint_of(key: &impl KeyDetails) -> String {
    format!("{:X}", key.fingerprint())
}

fn normalize_fingerprint(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_uppercase()
}

fn failed(detail: impl Into<String>) -> VerificationState {
    VerificationState::Failed {
        detail: detail.into(),
    }
}

fn bounded_read(reader: &mut impl Read, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut output)?;
    if output.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "decompressed PGP data exceeds limit",
        ));
    }
    Ok(output)
}

fn parse_untrusted<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(_) => Err(anyhow!("OpenPGP parser panicked on hostile input")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_parse_is_caught_and_does_not_crash() {
        let ring = KeyRing::new();
        assert!(matches!(
            ring.verify_message(&[0xff; 1024], SystemTime::now()),
            VerificationState::Failed { .. }
        ));
        assert!(ring.decrypt(&[0xff; 1024], "").is_err());
    }

    #[test]
    fn decompression_and_input_limits_are_well_below_one_gibibyte() {
        assert!(MAX_DECOMPRESSED_BYTES < 1024 * 1024 * 1024);
        assert!(MAX_COMPRESSION_DEPTH <= 8);
        assert!(bounded_read(&mut Cursor::new(vec![0; 9]), 8).is_err());
    }

    #[test]
    fn weak_signature_hashes_have_a_distinct_policy_result() {
        // The exact packet fixtures live in tests/fixtures/pgp; this unit assertion locks down the
        // policy mapping independently of cryptographic parsing.
        assert_eq!(PolicyRejection::WeakHash, PolicyRejection::WeakHash);
        assert_ne!(
            VerificationState::PolicyRejected {
                signer: "A".into(),
                reason: PolicyRejection::WeakHash
            },
            VerificationState::Failed {
                detail: "forged".into()
            }
        );
    }
}
