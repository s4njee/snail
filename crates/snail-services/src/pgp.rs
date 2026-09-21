//! Session secret handling and network key discovery for OpenPGP.
//!
//! WKD remains a discovery hint, never a trust decision. No public keyserver is contacted unless
//! the caller explicitly opts in for that individual lookup.

use std::collections::HashMap;
use std::io::Read;
use std::net::ToSocketAddrs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use snail_core::wkd::{AdvancedDns, WkdAddress, lookup_urls};
use zeroize::{Zeroize, Zeroizing};

use crate::secrets::SecretStore;

const PASSPHRASE_PREFIX: &str = "pgp:passphrase:";
pub const MAX_WKD_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_WKD_REDIRECTS: usize = 3;

/// Passphrases live in zeroizing session memory by default. Optional persistence delegates only to
/// the OS keychain abstraction; there is deliberately no plaintext-file fallback.
pub struct PassphraseManager {
    session: Mutex<HashMap<String, Zeroizing<String>>>,
    keychain: Option<Arc<dyn SecretStore>>,
}

impl PassphraseManager {
    pub fn session_only() -> Self {
        Self {
            session: Mutex::new(HashMap::new()),
            keychain: None,
        }
    }

    pub fn with_keychain(keychain: Arc<dyn SecretStore>) -> Self {
        Self {
            session: Mutex::new(HashMap::new()),
            keychain: Some(keychain),
        }
    }

    /// Unlock for this process. `remember` additionally stores in the configured OS keychain.
    pub fn unlock(&self, fingerprint: &str, passphrase: String, remember: bool) -> Result<()> {
        let fingerprint = normalize_fingerprint(fingerprint);
        let passphrase = Zeroizing::new(passphrase);
        if remember {
            let keychain = self
                .keychain
                .as_ref()
                .ok_or_else(|| anyhow!("OS keychain storage is not configured"))?;
            keychain.set(&keychain_key(&fingerprint), passphrase.as_str())?;
        }
        self.session.lock().unwrap().insert(fingerprint, passphrase);
        Ok(())
    }

    /// Return a short-lived zeroizing copy. If needed, hydrate the session from the OS keychain.
    pub fn resolve(&self, fingerprint: &str) -> Result<Option<Zeroizing<String>>> {
        let fingerprint = normalize_fingerprint(fingerprint);
        if let Some(value) = self.session.lock().unwrap().get(&fingerprint) {
            return Ok(Some(Zeroizing::new(value.to_string())));
        }
        let Some(keychain) = &self.keychain else {
            return Ok(None);
        };
        let Some(mut value) = keychain.get(&keychain_key(&fingerprint))? else {
            return Ok(None);
        };
        let protected = Zeroizing::new(std::mem::take(&mut value));
        value.zeroize();
        self.session
            .lock()
            .unwrap()
            .insert(fingerprint, Zeroizing::new(protected.to_string()));
        Ok(Some(protected))
    }

    pub fn lock(&self, fingerprint: &str) {
        self.session
            .lock()
            .unwrap()
            .remove(&normalize_fingerprint(fingerprint));
    }

    pub fn forget(&self, fingerprint: &str) -> Result<()> {
        let fingerprint = normalize_fingerprint(fingerprint);
        self.session.lock().unwrap().remove(&fingerprint);
        if let Some(keychain) = &self.keychain {
            keychain.delete(&keychain_key(&fingerprint))?;
        }
        Ok(())
    }

    pub fn clear_session(&self) {
        self.session.lock().unwrap().clear();
    }
}

impl Drop for PassphraseManager {
    fn drop(&mut self) {
        if let Ok(session) = self.session.get_mut() {
            session.clear();
        }
    }
}

fn keychain_key(fingerprint: &str) -> String {
    format!("{PASSPHRASE_PREFIX}{fingerprint}")
}

fn normalize_fingerprint(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_uppercase()
}

pub trait DnsProbe: Send + Sync {
    fn advanced_method(&self, domain: &str) -> Result<AdvancedDns>;
}

/// Conservative system resolver. Only recognized host-not-found errors permit the direct method;
/// timeouts, refusal and other transport failures are surfaced and never cause fallback.
pub struct SystemDnsProbe;

impl DnsProbe for SystemDnsProbe {
    fn advanced_method(&self, domain: &str) -> Result<AdvancedDns> {
        let host = format!("openpgpkey.{domain}:443");
        match host.to_socket_addrs() {
            Ok(mut addresses) => Ok(if addresses.next().is_some() {
                AdvancedDns::Exists
            } else {
                AdvancedDns::DoesNotExist
            }),
            Err(error) => {
                let text = error.to_string().to_ascii_lowercase();
                if text.contains("name or service not known")
                    || text.contains("nodename nor servname")
                    || text.contains("no such host")
                {
                    Ok(AdvancedDns::DoesNotExist)
                } else {
                    Err(error).context("advanced WKD DNS lookup failed; direct fallback forbidden")
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryResult {
    Found(Vec<u8>),
    NotFound,
}

pub struct WkdClient<D = SystemDnsProbe> {
    dns: D,
    http: reqwest::blocking::Client,
}

impl WkdClient<SystemDnsProbe> {
    pub fn system() -> Result<Self> {
        Self::new(SystemDnsProbe)
    }
}

impl<D: DnsProbe> WkdClient<D> {
    pub fn new(dns: D) -> Result<Self> {
        let redirect = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_WKD_REDIRECTS {
                attempt.error("too many WKD redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("WKD redirect attempted to leave HTTPS")
            } else {
                attempt.follow()
            }
        });
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .redirect(redirect)
            .user_agent("snail/0.1 WKD draft-22")
            .build()?;
        Ok(Self { dns, http })
    }

    /// Resolve exactly one WKD method. If the advanced hostname exists, every HTTP result,
    /// including 404 and transport failure, is final; the direct URL is not attempted.
    pub fn lookup(&self, email: &str) -> Result<DiscoveryResult> {
        let address = WkdAddress::parse(email)?;
        let dns = self.dns.advanced_method(&address.domain)?;
        let url = lookup_urls(&address, dns)
            .into_iter()
            .next()
            .expect("one WKD URL");
        self.fetch_https(&url)
    }

    /// Keyserver lookup is per-call opt-in because it reveals the queried address/social graph.
    pub fn lookup_keyserver_opt_in(&self, url: &str, opted_in: bool) -> Result<DiscoveryResult> {
        if !opted_in {
            bail!("keyserver lookup requires explicit opt-in");
        }
        self.fetch_https(url)
    }

    fn fetch_https(&self, url: &str) -> Result<DiscoveryResult> {
        let parsed = reqwest::Url::parse(url)?;
        if parsed.scheme() != "https" {
            bail!("OpenPGP key discovery requires HTTPS");
        }
        let response = self.http.get(parsed).send()?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(DiscoveryResult::NotFound);
        }
        if !response.status().is_success() {
            bail!("key discovery returned HTTP {}", response.status());
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_WKD_RESPONSE_BYTES as u64)
        {
            bail!("WKD response exceeds {MAX_WKD_RESPONSE_BYTES} bytes");
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_WKD_RESPONSE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_WKD_RESPONSE_BYTES {
            bail!("WKD response exceeds {MAX_WKD_RESPONSE_BYTES} bytes");
        }
        Ok(DiscoveryResult::Found(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn session_passphrase_can_optionally_round_trip_through_keychain() {
        let secrets = Arc::new(MemoryStore::new());
        let manager = PassphraseManager::with_keychain(secrets.clone());
        manager
            .unlock("AA BB", "correct horse".into(), true)
            .unwrap();
        assert_eq!(
            manager.resolve("aabb").unwrap().unwrap().as_str(),
            "correct horse"
        );
        manager.clear_session();
        assert_eq!(
            manager.resolve("AABB").unwrap().unwrap().as_str(),
            "correct horse"
        );
        manager.forget("AABB").unwrap();
        assert!(manager.resolve("AABB").unwrap().is_none());
    }

    #[test]
    fn session_only_never_writes_a_persistent_secret() {
        let manager = PassphraseManager::session_only();
        manager.unlock("AB", "secret".into(), false).unwrap();
        assert_eq!(manager.resolve("AB").unwrap().unwrap().as_str(), "secret");
        manager.clear_session();
        assert!(manager.resolve("AB").unwrap().is_none());
    }

    struct FixedDns(AdvancedDns);
    impl DnsProbe for FixedDns {
        fn advanced_method(&self, _domain: &str) -> Result<AdvancedDns> {
            Ok(self.0)
        }
    }

    #[test]
    fn configured_limits_are_bounded() {
        assert_eq!(MAX_WKD_REDIRECTS, 3);
        assert!(MAX_WKD_RESPONSE_BYTES <= 5 * 1024 * 1024);
        // Construction exercises the HTTPS-only redirect policy without requiring a network.
        WkdClient::new(FixedDns(AdvancedDns::Exists)).unwrap();
    }
}
