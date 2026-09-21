//! E0.4 — both accounts authenticate and pull one page (plan.md §3).
//!
//! Google: the loopback OAuth + PKCE flow for an **installed app**, then Gmail and Calendar pages.
//! The corpus dumper for E0.3 rides on the same token.
//!
//! Run:
//!   cargo run -- google          # consent + profile + 50 Gmail headers + one Calendar page
//!   cargo run -- corpus [dir]    # E0.3: write raw MIME (.eml) for the worst-case categories
//!
//! Credentials live in `../../.secrets/e0.json` (0600, gitignored). Tokens are cached to
//! `../../.secrets/e0-tokens.json` (0600). Nothing secret is ever printed.

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod icloud;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.labels",
    "https://www.googleapis.com/auth/calendar",
    "https://www.googleapis.com/auth/calendar.events",
];

fn repo_root() -> PathBuf {
    // spikes/e0.4 -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn secrets_dir() -> PathBuf {
    repo_root().join(".secrets")
}

#[derive(Deserialize)]
struct Credentials {
    google_client_id: String,
    google_client_secret: String,
    #[allow(dead_code)]
    icloud_email: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    scope: String,
    token_type: String,
}

impl Tokens {
    fn is_fresh(&self) -> bool {
        now_epoch() + 60 < self.expires_at
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_secs()
}

fn credentials() -> Result<Credentials> {
    let path = secrets_dir().join("e0.json");
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("read {} — is the credentials file in place?", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

fn tokens_path() -> PathBuf {
    secrets_dir().join("e0-tokens.json")
}

fn load_tokens() -> Result<Option<Tokens>> {
    let path = tokens_path();
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&fs::read_to_string(&path)?)?))
}

fn save_tokens(tokens: &Tokens) -> Result<()> {
    let path = tokens_path();
    fs::write(&path, serde_json::to_vec_pretty(tokens)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(url).spawn().context("open the system browser")?;
    Ok(())
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect()
}

/// Redirect URI is loopback on an ephemeral port. Google notes client firewalls can break
/// `localhost`, so `127.0.0.1` is used explicitly (plan.md E3.1).
fn authorize(client_id: &str, client_secret: &str) -> Result<Tokens> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("bind loopback redirect")?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth2callback");

    let mut verifier_bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut verifier_bytes);
    let code_verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let code_challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));

    let mut state_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut state_bytes);
    let state = URL_SAFE_NO_PAD.encode(state_bytes);

    let auth_url = format!(
        "{AUTH_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}\
         &code_challenge={code_challenge}&code_challenge_method=S256&access_type=offline&prompt=consent",
        urlencode(client_id),
        urlencode(&redirect_uri),
        urlencode(&SCOPES.join(" ")),
        urlencode(&state),
    );

    println!("Opening the system browser for consent…");
    println!("If it does not open, paste this:\n{auth_url}\n");
    open_in_browser(&auth_url)?;

    let (mut stream, _) = listener.accept().context("wait for the redirect")?;
    let mut buffer = [0u8; 8192];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("malformed redirect request"))?;
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
        bail!("consent returned error: {error}");
    }
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        bail!("state mismatch — possible CSRF, refusing the code");
    }
    let code = params
        .get("code")
        .ok_or_else(|| anyhow!("no code in redirect"))?
        .clone();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
        scope: Option<String>,
        token_type: String,
    }
    let response: TokenResponse = client
        .post(TOKEN_URL)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", code_verifier.as_str()),
        ])
        .send()?
        .error_for_status()?
        .json()?;

    Ok(Tokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token.unwrap_or_default(),
        expires_at: now_epoch() + response.expires_in,
        scope: response.scope.unwrap_or_default(),
        token_type: response.token_type,
    })
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

fn access_token() -> Result<String> {
    let credentials = credentials()?;
    if let Some(tokens) = load_tokens()? {
        if tokens.is_fresh() {
            return Ok(tokens.access_token);
        }
        if !tokens.refresh_token.is_empty() {
            println!("access token stale; refreshing");
            let refreshed = refresh(&credentials, &tokens.refresh_token)?;
            save_tokens(&refreshed)?;
            return Ok(refreshed.access_token);
        }
    }
    let tokens = authorize(&credentials.google_client_id, &credentials.google_client_secret)?;
    save_tokens(&tokens)?;
    Ok(tokens.access_token)
}

fn refresh(credentials: &Credentials, refresh_token: &str) -> Result<Tokens> {
    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: u64,
        scope: Option<String>,
        token_type: String,
    }
    let response: TokenResponse = reqwest::blocking::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("client_id", credentials.google_client_id.as_str()),
            ("client_secret", credentials.google_client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(Tokens {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .unwrap_or_else(|| refresh_token.to_string()),
        expires_at: now_epoch() + response.expires_in,
        scope: response.scope.unwrap_or_default(),
        token_type: response.token_type,
    })
}

fn get_json(token: &str, url: &str) -> Result<serde_json::Value> {
    let response = reqwest::blocking::Client::new()
        .get(url)
        .bearer_auth(token)
        // Google silently disables compression without `gzip` in the User-Agent (plan.md E4.9).
        .header(reqwest::header::ACCEPT_ENCODING, "gzip")
        .header(reqwest::header::USER_AGENT, "snail/0.1 (gzip) (E0.4 spike)")
        .send()?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        bail!("GET {url} -> {status}: {text}");
    }
    serde_json::from_str(&text).with_context(|| {
        format!(
            "parse JSON from {url} ({status}); first 120 bytes: {:?}",
            &text.as_bytes()[..text.len().min(120)]
        )
    })
}

fn google() -> Result<()> {
    let token = access_token()?;

    let profile = get_json(
        &token,
        "https://gmail.googleapis.com/gmail/v1/users/me/profile",
    )?;
    println!(
        "Gmail profile: {} ({} messages, {} threads)",
        profile["emailAddress"].as_str().unwrap_or("?"),
        profile["messagesTotal"],
        profile["threadsTotal"]
    );

    let list = get_json(
        &token,
        "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=50&labelIds=INBOX",
    )?;
    let ids: Vec<&str> = list["messages"]
        .as_array()
        .map(|array| array.iter().filter_map(|m| m["id"].as_str()).collect())
        .unwrap_or_default();
    println!("\nGmail: {} of the newest INBOX messages (headers)\n", ids.len());
    for id in &ids {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format=metadata\
             &metadataHeaders=From&metadataHeaders=Subject&metadataHeaders=Date"
        );
        let message = get_json(&token, &url)?;
        let headers = message["payload"]["headers"].as_array().cloned().unwrap_or_default();
        let header = |name: &str| {
            headers
                .iter()
                .find(|h| {
                    h["name"]
                        .as_str()
                        .map(|value| value.eq_ignore_ascii_case(name))
                        .unwrap_or(false)
                })
                .and_then(|h| h["value"].as_str())
                .unwrap_or("")
                .to_string()
        };
        println!(
            "  {:<40}  {:<28}  {}",
            truncate(&header("From"), 40),
            truncate(&header("Date"), 28),
            truncate(&header("Subject"), 60),
        );
    }

    let calendars = get_json(
        &token,
        "https://www.googleapis.com/calendar/v3/users/me/calendarList?maxResults=50",
    )?;
    println!("\nGoogle Calendar: calendars");
    for item in calendars["items"].as_array().cloned().unwrap_or_default() {
        println!(
            "  {:<32} {}  role={}",
            truncate(item["summary"].as_str().unwrap_or("?"), 32),
            item["id"].as_str().unwrap_or("?"),
            item["accessRole"].as_str().unwrap_or("?"),
        );
    }
    let events = get_json(
        &token,
        "https://www.googleapis.com/calendar/v3/calendars/primary/events\
         ?maxResults=50&singleEvents=false",
    )?;
    let event_count = events["items"].as_array().map(Vec::len).unwrap_or(0);
    println!("  primary: {event_count} events on the first page");
    for event in events["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .take(5)
    {
        println!(
            "    {}  {}",
            event["start"]["dateTime"].as_str().unwrap_or("(all-day/recurring)"),
            truncate(event["summary"].as_str().unwrap_or("(no title)"), 60),
        );
    }

    Ok(())
}

fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        input.to_string()
    } else {
        let head: String = input.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Gmail's `raw` is base64url, but padding is not always absent, so be lenient.
fn decode_raw(encoded: &str) -> Result<Vec<u8>> {
    let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let trimmed = cleaned.trim_end_matches('=');
    URL_SAFE_NO_PAD.decode(trimmed).or_else(|_| {
        let standard = trimmed.replace('-', "+").replace('_', "/");
        STANDARD.decode(standard)
    })
    .context("decode Gmail raw (base64url)")
}

/// E0.3: dump raw MIME for the worst-case categories into `.eml` files.
fn corpus(out_dir: &Path) -> Result<()> {
    let token = access_token()?;
    fs::create_dir_all(out_dir)?;
    // Each query targets one of E0.3's hard categories; each takes up to 3 so the corpus reaches
    // ~20 messages while staying weighted to the worst cases rather than the average inbox.
    let queries: &[(&str, &str)] = &[
        ("newsletter", "category:promotions"),
        ("github", "from:notifications@github.com OR from:github.com"),
        (
            "calendar-invite",
            "from:calendar-notification@google.com OR subject:(invitation) OR has:calendar",
        ),
        ("quoted-chain", "subject:(re:)"),
        ("attachment", "has:attachment"),
        ("plain", "is:unread -has:attachment"),
        ("receipt", "subject:(receipt OR order OR shipped OR delivery)"),
        ("notification", "category:updates"),
    ];

    let mut written = 0usize;
    for (slug, query) in queries {
        let url = format!(
            "https://gmail.googleapis.com/gmail/v1/users/me/messages?maxResults=3&q={}",
            urlencode(query)
        );
        let list = get_json(&token, &url)?;
        let ids: Vec<String> = list["messages"]
            .as_array()
            .map(|array| array.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        for (index, id) in ids.iter().enumerate() {
            let raw = get_json(
                &token,
                &format!("https://gmail.googleapis.com/gmail/v1/users/me/messages/{id}?format=raw"),
            )?;
            let Some(encoded) = raw["raw"].as_str() else {
                continue;
            };
            let bytes = decode_raw(encoded)?;
            let name = format!("{slug}-{index}-{}.eml", &id[..id.len().min(8)]);
            fs::write(out_dir.join(&name), &bytes)?;
            written += 1;
            println!("  wrote {name} ({} bytes)", bytes.len());
        }
    }
    println!("\n{written} messages written to {}", out_dir.display());
    if written == 0 {
        bail!("no messages dumped — check the queries and scopes");
    }
    Ok(())
}

fn main() -> Result<()> {
    // rustls cannot pick a provider automatically when several are in the tree.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("google") => google(),
        Some("corpus") => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| repo_root().join("spikes/corpus"));
            corpus(&dir)
        }
        Some("auth") => {
            let token = access_token()?;
            println!("have a token ({} chars), refresh stored: {}", token.len(), load_tokens()?.map(|t| !t.refresh_token.is_empty()).unwrap_or(false));
            Ok(())
        }
        Some("icloud") => {
            let credentials = credentials()?;
            let password = keychain_password()?;
            icloud::run(&credentials.icloud_email, &password)
        }
        Some("caldav") => {
            let credentials = credentials()?;
            let password = keychain_password()?;
            icloud::caldav(&credentials.icloud_email, &password)
        }
        _ => {
            eprintln!("usage: snail-e0-4 <google|corpus [dir]|auth|icloud|caldav>");
            std::process::exit(2);
        }
    }
}

/// Read the iCloud app-specific password from the macOS keychain (it is never on disk).
fn keychain_password() -> Result<String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "dev.snail.app.icloud", "-w"])
        .output()
        .context("run `security`")?;
    if !output.status.success() {
        bail!("no keychain entry for dev.snail.app.icloud");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}
