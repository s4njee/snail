//! Accounts and authentication (plan.md E3). The two providers share nothing here, which is why the
//! trait boundary sits above this epic — but they share the *error* contract, because "login failed"
//! is useless to a user (E3.6).
//!
//! The risky halves are already proven by the E0.4 spike: Google's loopback OAuth + PKCE, and
//! iCloud's app passwords. This module is the production shape of both, plus the secret plumbing.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::secrets::SecretStore;

/// The four scopes E3.2 decides on: Gmail REST with `gmail.modify` + `gmail.labels`, plus Calendar.
pub const GOOGLE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.labels",
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/calendar.events",
];

pub const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// --- Errors (E3.6) --------------------------------------------------------------------------

/// Every way authentication fails, each with copy a user can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The OAuth client was deleted for inactivity (E3.1b); every refresh token now fails.
    DeletedClient,
    /// The refresh token was revoked or expired.
    TokenRevoked,
    /// The user withdrew consent; re-auth is required.
    ConsentWithdrawn,
    /// A Workspace admin blocked this OAuth client (403 `domainPolicy`).
    WorkspaceBlocked,
    /// The iCloud app-specific password is wrong.
    WrongAppPassword,
    /// Two-factor auth is not enabled on the Apple Account.
    TwoFactorMissing,
    /// The Apple Account password changed, revoking every app-specific password (E3.4).
    AppPasswordRevoked,
    /// Network unreachable — not an error state, just retry.
    Network(String),
    /// The system clock is wrong, which breaks OAuth and is otherwise invisible.
    ClockSkew,
    /// No OS keychain is available (E3.8).
    KeychainUnavailable(String),
    Other(String),
}

impl AuthError {
    pub fn message(&self) -> String {
        match self {
            AuthError::DeletedClient =>
                "Google deleted this app's OAuth client after six months of inactivity. Create a \
                 new Desktop client in Google Cloud Console and sign in again."
                    .into(),
            AuthError::TokenRevoked =>
                "Google no longer accepts this account's sign-in. Sign in again to reconnect."
                    .into(),
            AuthError::ConsentWithdrawn =>
                "Access was withdrawn for this account. Sign in again to restore it.".into(),
            AuthError::WorkspaceBlocked =>
                "Your organization's Google Workspace admin has blocked this app. Ask them to \
                 allowlist the Snail OAuth client, or use an app password."
                    .into(),
            AuthError::WrongAppPassword =>
                "iCloud rejected that app-specific password. Generate a new one at account.apple.com \
                 → Sign-In and Security → App-Specific Passwords."
                    .into(),
            AuthError::TwoFactorMissing =>
                "Two-factor authentication must be enabled on the Apple Account before \
                 app-specific passwords can be created."
                    .into(),
            AuthError::AppPasswordRevoked =>
                "Your Apple Account password changed, which silently revokes every app-specific \
                 password. Generate a new one and sign in again."
                    .into(),
            AuthError::Network(inner) => format!("Could not reach the server: {inner}"),
            AuthError::ClockSkew =>
                "Your system clock is off, which breaks sign-in. Correct the date and time and try \
                 again."
                    .into(),
            AuthError::KeychainUnavailable(inner) =>
                format!("No OS keychain is available ({inner})."),
            AuthError::Other(inner) => inner.clone(),
        }
    }

    /// Map a Google token-endpoint error body to an actionable error.
    pub fn from_google(code: &str, description: &str) -> Self {
        let code = code.to_ascii_lowercase();
        let description = description.to_ascii_lowercase();
        if code == "deleted_client" {
            return AuthError::DeletedClient;
        }
        if description.contains("clock") || description.contains("skew") {
            return AuthError::ClockSkew;
        }
        match code.as_str() {
            "invalid_grant" => {
                if description.contains("revoked") || description.contains("expired") {
                    AuthError::TokenRevoked
                } else {
                    AuthError::ConsentWithdrawn
                }
            }
            "invalid_client" | "unauthorized_client" => AuthError::DeletedClient,
            "access_denied" => AuthError::ConsentWithdrawn,
            other => AuthError::Other(format!("Google sign-in failed ({other}): {description}")),
        }
    }

    /// Map an iCloud IMAP/SMTP failure string. There is no structured error here, so the messages
    /// are the interface.
    pub fn from_icloud(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        if lower.contains("authenticationfailed") || lower.contains("invalid credentials") {
            AuthError::WrongAppPassword
        } else if lower.contains("two-factor") || lower.contains("2fa") {
            AuthError::TwoFactorMissing
        } else {
            AuthError::Other(text.to_string())
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for AuthError {}

// --- Google OAuth (E3.1, E3.3) ----------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub scope: String,
    pub token_type: String,
}

impl Tokens {
    pub fn is_fresh(&self) -> bool {
        now_epoch() + 60 < self.expires_at
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs()
}

/// The installed-app OAuth client. The secret is not confidential (E3.1) but is injected rather
/// than committed (E3.1's public-repo note).
#[derive(Clone, Debug)]
pub struct GoogleOAuth {
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleOAuth {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        }
    }

    /// The consent URL. `code_challenge_method=S256` is explicit: Google silently defaults to
    /// `plain` when a challenge is sent without it (E3.1).
    pub fn authorize_url(&self, redirect_uri: &str, state: &str, code_challenge: &str) -> String {
        format!(
            "{GOOGLE_AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}\
             &code_challenge={code_challenge}&code_challenge_method=S256&access_type=offline\
             &prompt=consent",
            urlencode(&self.client_id),
            urlencode(redirect_uri),
            urlencode(&GOOGLE_SCOPES.join(" ")),
            urlencode(state),
        )
    }

    /// Run the loopback flow: bind a random `127.0.0.1` port, open the system browser, catch the
    /// redirect, and exchange the code.
    pub fn authorize_loopback(&self) -> Result<Tokens> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).context("bind loopback redirect")?;
        let port = listener.local_addr()?.port();
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth2callback");

        let verifier = pkce_verifier();
        let challenge = pkce_challenge(&verifier);
        let state = pkce_verifier();

        let url = self.authorize_url(&redirect_uri, &state, &challenge);
        open_in_browser(&url)?;

        let (mut stream, _) = listener.accept().context("wait for the redirect")?;
        let mut buffer = [0u8; 8192];
        let read = stream.read(&mut buffer)?;
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| anyhow!("malformed redirect"))?;
        let params = parse_query(target.split_once('?').map(|(_, q)| q).unwrap_or(""));

        let body = b"<html><body style=\"font-family:sans-serif;padding:3rem\">\
            <h2>Snail is connected.</h2><p>You can close this tab.</p></body></html>";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();

        if let Some(error) = params.get("error") {
            return Err(AuthError::from_google(error, params.get("error_description").map(String::as_str).unwrap_or("")).into());
        }
        if params.get("state").map(String::as_str) != Some(state.as_str()) {
            bail!("state mismatch — refusing the authorization code");
        }
        let code = params.get("code").ok_or_else(|| anyhow!("no code in redirect"))?;
        self.exchange(code, &redirect_uri, &verifier)
    }

    pub fn exchange(&self, code: &str, redirect_uri: &str, verifier: &str) -> Result<Tokens> {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("code", code),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", redirect_uri),
                ("grant_type", "authorization_code"),
                ("code_verifier", verifier),
            ])
            .send()?;
        parse_token_response(response)
    }

    /// Refresh ahead of expiry. A refresh may return a new refresh token; keep it if so.
    pub fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        let response = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()?;
        parse_token_response(response)
    }
}

fn parse_token_response(response: reqwest::blocking::Response) -> Result<Tokens> {
    let status = response.status();
    let text = response.text()?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("token endpoint returned non-JSON ({status})"))?;
    if !status.is_success() {
        let code = value.get("error").and_then(|v| v.as_str()).unwrap_or("unknown");
        let description = value
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        return Err(AuthError::from_google(code, description).into());
    }
    Ok(Tokens {
        access_token: value.get("access_token").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        refresh_token: value.get("refresh_token").and_then(|v| v.as_str()).map(str::to_string),
        expires_at: now_epoch() + value.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600),
        scope: value.get("scope").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        token_type: value.get("token_type").and_then(|v| v.as_str()).unwrap_or("Bearer").to_string(),
    })
}

pub fn pkce_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (percent_decode(key), percent_decode(value)))
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(url).spawn().context("open the system browser")?;
    Ok(())
}

// --- iCloud (E3.4, E3.4b) --------------------------------------------------------------------

/// The **IMAP** username is the name part only; the **SMTP** username is the full address
/// (E3.4b). Every client gets this wrong once.
pub fn imap_username(full_address: &str) -> &str {
    full_address.split('@').next().unwrap_or(full_address)
}

pub fn smtp_username(full_address: &str) -> &str {
    full_address
}

/// The order to try IMAP logins: the full address first, then the local part (E3.4b). The field
/// stays user-editable.
pub fn imap_usernames_to_try(full_address: &str) -> Vec<String> {
    let local = imap_username(full_address).to_string();
    let mut candidates = vec![full_address.to_string()];
    if local != full_address {
        candidates.push(local);
    }
    candidates
}

// --- Account store (E3.3, E3.4, E3.7) --------------------------------------------------------

/// Accounts plus their secrets. Secrets live in the `SecretStore`, never the database (E2.8).
pub struct AccountStore {
    store: snail_core::store::Store,
    secrets: Arc<dyn SecretStore>,
}

impl AccountStore {
    pub fn new(store: snail_core::store::Store, secrets: Arc<dyn SecretStore>) -> Self {
        Self { store, secrets }
    }

    pub fn store(&self) -> &snail_core::store::Store {
        &self.store
    }

    pub fn google_refresh_key(address: &str) -> String {
        format!("google:refresh:{address}")
    }

    pub fn icloud_password_key(address: &str) -> String {
        format!("icloud:app-password:{address}")
    }

    pub fn pgp_passphrase_key(fingerprint: &str) -> String {
        format!("pgp:passphrase:{fingerprint}")
    }

    pub fn set_google_refresh_token(&self, address: &str, token: &str) -> Result<()> {
        self.secrets
            .set(&Self::google_refresh_key(address), token)
    }

    pub fn google_refresh_token(&self, address: &str) -> Result<Option<String>> {
        self.secrets.get(&Self::google_refresh_key(address))
    }

    pub fn set_icloud_app_password(&self, address: &str, password: &str) -> Result<()> {
        self.secrets
            .set(&Self::icloud_password_key(address), password)
    }

    pub fn icloud_app_password(&self, address: &str) -> Result<Option<String>> {
        self.secrets.get(&Self::icloud_password_key(address))
    }

    /// Remove every secret for an account (E14.9).
    pub fn forget(&self, kind: &str, address: &str) -> Result<()> {
        match kind {
            "gmail" | "google" => self.secrets.delete(&Self::google_refresh_key(address)),
            "icloud" => self.secrets.delete(&Self::icloud_password_key(address)),
            _ => Ok(()),
        }
    }

    /// Register an account and its default identity in one step (E3.4c, E3.7).
    pub fn add_account(
        &self,
        kind: &str,
        address: &str,
        display_name: Option<&str>,
        now: i64,
    ) -> Result<i64> {
        if let Some(existing) = self.store.account_by_address(kind, address)? {
            return Ok(existing.id);
        }
        let id = self.store.insert_account(kind, address, display_name, now)?;
        self.store.insert_identity(&snail_core::store::NewIdentity {
            account_id: id,
            address: address.to_string(),
            display_name: display_name.map(str::to_string),
            signature: None,
            is_default: true,
        })?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn authorize_url_pins_s256_and_loopback() {
        let oauth = GoogleOAuth::new("client-123", "secret");
        let verifier = pkce_verifier();
        let challenge = pkce_challenge(&verifier);
        let url = oauth.authorize_url("http://127.0.0.1:4444/oauth2callback", "state-xyz", &challenge);
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A4444"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("gmail.modify"));
        // The challenge is the SHA-256 of the verifier, not the verifier itself.
        assert!(url.contains(&challenge));
        assert!(!url.contains(&verifier));
    }

    #[test]
    fn pkce_challenge_is_deterministic_and_matches_rfc_shape() {
        let challenge = pkce_challenge("abc");
        assert_eq!(challenge, pkce_challenge("abc"));
        // base64url of a 32-byte digest is 43 characters, unpadded.
        assert_eq!(challenge.len(), 43);
        assert!(!challenge.contains('='));
    }

    #[test]
    fn google_errors_map_to_actionable_variants() {
        assert_eq!(
            AuthError::from_google("deleted_client", ""),
            AuthError::DeletedClient
        );
        assert_eq!(
            AuthError::from_google("invalid_grant", "Token has been expired or revoked."),
            AuthError::TokenRevoked
        );
        assert_eq!(
            AuthError::from_google("invalid_grant", "clock skew too great"),
            AuthError::ClockSkew
        );
        assert_eq!(
            AuthError::from_google("access_denied", ""),
            AuthError::ConsentWithdrawn
        );
        // Every message is non-empty and specific.
        for error in [
            AuthError::DeletedClient,
            AuthError::TokenRevoked,
            AuthError::WorkspaceBlocked,
            AuthError::WrongAppPassword,
            AuthError::AppPasswordRevoked,
            AuthError::ClockSkew,
        ] {
            assert!(!error.message().is_empty(), "{error:?}");
        }
    }

    #[test]
    fn icloud_errors_map_to_app_password_variants() {
        assert_eq!(
            AuthError::from_icloud("NO [AUTHENTICATIONFAILED] Invalid credentials"),
            AuthError::WrongAppPassword
        );
        assert!(matches!(
            AuthError::from_icloud("something else"),
            AuthError::Other(_)
        ));
    }

    #[test]
    fn the_imap_username_is_the_name_part_but_smtp_is_the_full_address() {
        assert_eq!(imap_username("johnappleseed@icloud.com"), "johnappleseed");
        assert_eq!(smtp_username("johnappleseed@icloud.com"), "johnappleseed@icloud.com");
        assert_eq!(
            imap_usernames_to_try("johnappleseed@icloud.com"),
            vec!["johnappleseed@icloud.com", "johnappleseed"]
        );
    }

    #[test]
    fn secrets_round_trip_through_the_account_store() {
        let store = snail_core::store::Store::open_in_memory().unwrap();
        let secrets = Arc::new(MemoryStore::new());
        let accounts = AccountStore::new(store, secrets.clone());

        let id = accounts
            .add_account("icloud", "me@icloud.com", Some("Me"), 0)
            .unwrap();
        assert!(id > 0);
        // Adding twice is idempotent.
        assert_eq!(
            accounts.add_account("icloud", "me@icloud.com", None, 0).unwrap(),
            id
        );

        accounts
            .set_icloud_app_password("me@icloud.com", "abcd-efgh-ijkl-mnop")
            .unwrap();
        assert_eq!(
            accounts.icloud_app_password("me@icloud.com").unwrap().as_deref(),
            Some("abcd-efgh-ijkl-mnop")
        );
        accounts.forget("icloud", "me@icloud.com").unwrap();
        assert_eq!(accounts.icloud_app_password("me@icloud.com").unwrap(), None);
    }
}
