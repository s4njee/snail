//! Web Key Directory mechanics (draft-koch-openpgp-webkey-service-22).
//!
//! WKD is discovery, not a trust decision.  Network policy lives in `snail-services`; this module
//! deliberately only normalizes an address, hashes it and decides whether direct fallback is
//! permitted.  Keeping the state machine here makes the important DNS-vs-HTTP distinction easy to
//! test without touching the network.

use anyhow::{Result, bail};
use sha1::{Digest, Sha1};

const ZBASE32: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WkdAddress {
    pub local: String,
    pub domain: String,
    pub hash: String,
}

impl WkdAddress {
    pub fn parse(email: &str) -> Result<Self> {
        let (local, domain) = email
            .trim()
            .rsplit_once('@')
            .ok_or_else(|| anyhow::anyhow!("WKD address has no domain"))?;
        if local.is_empty() || domain.is_empty() || domain.contains(['/', '?', '#', '@']) {
            bail!("invalid WKD address");
        }
        // The draft specifies ASCII case folding only. Unicode code points are preserved.
        let local = local
            .chars()
            .map(|ch| {
                if ch.is_ascii() {
                    ch.to_ascii_lowercase()
                } else {
                    ch
                }
            })
            .collect::<String>();
        let domain = domain.to_ascii_lowercase();
        let digest = Sha1::digest(local.as_bytes());
        let hash = zbase32(&digest);
        debug_assert_eq!(hash.len(), 32);
        Ok(Self {
            local,
            domain,
            hash,
        })
    }

    pub fn advanced_url(&self) -> String {
        format!(
            "https://openpgpkey.{0}/.well-known/openpgpkey/{0}/hu/{1}?l={2}",
            self.domain,
            self.hash,
            percent_encode(&self.local)
        )
    }

    pub fn direct_url(&self) -> String {
        format!(
            "https://{0}/.well-known/openpgpkey/hu/{1}?l={2}",
            self.domain,
            self.hash,
            percent_encode(&self.local)
        )
    }
}

/// Result of checking whether `openpgpkey.<domain>` exists in DNS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvancedDns {
    Exists,
    DoesNotExist,
}

/// The URLs permitted by the draft.  An existing advanced-method host is authoritative even when
/// its HTTP request fails or returns 404; direct fallback happens only for DNS NXDOMAIN.
pub fn lookup_urls(address: &WkdAddress, dns: AdvancedDns) -> Vec<String> {
    match dns {
        AdvancedDns::Exists => vec![address.advanced_url()],
        AdvancedDns::DoesNotExist => vec![address.direct_url()],
    }
}

fn zbase32(bytes: &[u8]) -> String {
    let mut output = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut bits = 0_u32;
    let mut count = 0_u8;
    for &byte in bytes {
        bits = (bits << 8) | u32::from(byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            output.push(ZBASE32[((bits >> count) & 31) as usize] as char);
        }
    }
    if count != 0 {
        output.push(ZBASE32[((bits << (5 - count)) & 31) as usize] as char);
    }
    output
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_vector_and_urls_are_exact() {
        let address = WkdAddress::parse("Joe.Doe@Example.ORG").unwrap();
        assert_eq!(address.local, "joe.doe");
        assert_eq!(address.hash, "iy9q119eutrkn8s1mk4r39qejnbu3n5q");
        assert_eq!(
            address.advanced_url(),
            "https://openpgpkey.example.org/.well-known/openpgpkey/example.org/hu/iy9q119eutrkn8s1mk4r39qejnbu3n5q?l=joe.doe"
        );
    }

    #[test]
    fn only_ascii_in_local_part_is_folded() {
        let address = WkdAddress::parse("J\u{00d6}RG+Mail@example.org").unwrap();
        assert_eq!(address.local, "j\u{00d6}rg+mail");
        assert!(address.direct_url().ends_with("?l=j%C3%96rg%2Bmail"));
    }

    #[test]
    fn direct_is_selected_only_for_dns_nonexistence() {
        let address = WkdAddress::parse("a@example.org").unwrap();
        assert_eq!(
            lookup_urls(&address, AdvancedDns::Exists),
            vec![address.advanced_url()]
        );
        assert_eq!(
            lookup_urls(&address, AdvancedDns::DoesNotExist),
            vec![address.direct_url()]
        );
    }
}
