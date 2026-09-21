//! E0.4 — iCloud: IMAP auth and the §6.7/§6.8 probes, plus CalDAV discovery.
//!
//! Everything here answers a specific `[unverified]` in plan.md §6:
//! - re-issue `CAPABILITY` after auth (4.12) — iCloud hides IDLE/CONDSTORE/QRESYNC until login;
//! - does `ENABLE CONDSTORE` work, and does `SELECT … (CONDSTORE)` work (4.13)?
//! - folder names for the sent/trash mapping (4.18) — no SPECIAL-USE/XLIST on iCloud;
//! - does the CalDAV home advertise RFC 6578 `sync-collection` (11.13)?

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_imap::Client;
use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tokio_rustls::client::TlsStream;

const IMAP_HOST: &str = "imap.mail.me.com";
const IMAP_PORT: u16 = 993;

fn tls_connector() -> TlsConnector {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    TlsConnector::from(Arc::new(config))
}

async fn tls_connect(connector: &TlsConnector) -> Result<TlsStream<TcpStream>> {
    let tcp = TcpStream::connect((IMAP_HOST, IMAP_PORT))
        .await
        .with_context(|| format!("connect {IMAP_HOST}:{IMAP_PORT}"))?;
    let server_name = ServerName::try_from(IMAP_HOST.to_string()).expect("valid dns name");
    connector.connect(server_name, tcp).await.context("TLS")
}

/// async-imap exposes no pre-auth `CAPABILITY` through its public API, so this reads the greeting
/// and issues a raw `CAPABILITY` on a separate connection (as a client that caches the greeting
/// would). This is the exact trap Thunderbird shipped until v80 (plan.md §6.7).
async fn pre_auth_capabilities() -> Result<String> {
    let connector = tls_connector();
    let stream = tls_connect(&connector).await?;
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half);

    let mut greeting = String::new();
    reader.read_line(&mut greeting).await.context("read greeting")?;
    write_half
        .write_all(b"a1 CAPABILITY\r\n")
        .await
        .context("send CAPABILITY")?;
    let mut transcript = greeting.clone();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }
        transcript.push_str(&line);
        if line.starts_with("a1 ") {
            break;
        }
    }
    Ok(transcript.trim().to_string())
}

pub fn run(full_address: &str, password: &str) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new().context("tokio runtime")?;
    runtime.block_on(imap_probe(full_address, password))
}

fn format_capabilities(caps: &async_imap::types::Capabilities) -> String {
    let mut names: Vec<String> = caps.iter().map(|cap| format!("{cap:?}")).collect();
    names.sort();
    names.join(" ")
}

async fn imap_probe(full_address: &str, password: &str) -> Result<()> {
    println!("--- pre-auth, raw connection ---");
    match pre_auth_capabilities().await {
        Ok(transcript) => println!("{transcript}\n"),
        Err(error) => println!("pre-auth probe failed: {error}\n"),
    }

    let connector = tls_connector();
    let tls = tls_connect(&connector).await?;
    let client = Client::new(tls);

    let local = full_address.split('@').next().unwrap_or(full_address);
    // Apple documents the username asymmetry (E3.4b): the IMAP username is the name part only.
    // Try the full address first anyway, then fall back to the local part.
    let (mut session, used_username) = match client.login(full_address, password).await {
        Ok(session) => (session, full_address.to_string()),
        Err((first_error, retry_client)) => {
            println!("login as {full_address:?} failed ({first_error}); retrying as {local:?}");
            match retry_client.login(local, password).await {
                Ok(session) => (session, local.to_string()),
                Err((second_error, _)) => bail!(
                    "IMAP login failed as both {full_address:?} ({first_error}) and {local:?} ({second_error})"
                ),
            }
        }
    };
    println!("logged in as {used_username:?}");

    // 4.12: re-issue CAPABILITY after auth. iCloud reveals IDLE/CONDSTORE/QRESYNC only now.
    let post_auth = session.capabilities().await.context("post-auth CAPABILITY")?;
    println!("\n--- post-auth capabilities ---\n{}\n", format_capabilities(&post_auth));

    // 4.13(a): ENABLE CONDSTORE, the form the plan says to use instead of SELECT (CONDSTORE).
    match session.run_command_and_check_ok("ENABLE CONDSTORE").await {
        Ok(()) => println!("ENABLE CONDSTORE: OK"),
        Err(error) => println!("ENABLE CONDSTORE: FAILED ({error})"),
    }
    // 4.13(a): SELECT INBOX (CONDSTORE) — the form the plan says iCloud rejects.
    match session.select_condstore("INBOX").await {
        Ok(mailbox) => println!("SELECT INBOX (CONDSTORE): OK ({} messages)", mailbox.exists),
        Err(error) => println!("SELECT INBOX (CONDSTORE): FAILED ({error})"),
    }

    let mailbox = session.select("INBOX").await.context("SELECT INBOX")?;
    println!(
        "\nINBOX: {} messages, {} recent, uidnext={:?}, uidvalidity={:?}\n",
        mailbox.exists, mailbox.recent, mailbox.uid_next, mailbox.uid_validity
    );

    // 4.18: folder names for the sent/trash mapping. iCloud advertises no SPECIAL-USE/XLIST.
    println!("Folders (LIST \"\" *):");
    {
        let folders = session.list(Some(""), Some("*")).await.context("LIST")?;
        let folders: Vec<_> = folders.try_collect().await.context("collect LIST")?;
        for folder in folders {
            println!("  {:<34} {:?}", folder.name(), folder.attributes());
        }
    }

    println!("\nINBOX: up to 50 headers (uid BODY.PEEK[HEADER.FIELDS ...])");
    {
        let mut fetch = session
            .uid_fetch("1:50", "(BODY.PEEK[HEADER.FIELDS (FROM SUBJECT DATE)])")
            .await
            .context("UID FETCH")?;
        let mut count = 0;
        while let Some(item) = fetch.try_next().await? {
            if let Some(header) = item.header() {
                let text = String::from_utf8_lossy(header);
                let field = |name: &str| {
                    text.lines()
                        .find(|line| line.to_ascii_lowercase().starts_with(name))
                        .map(|line| line[name.len()..].trim().to_string())
                        .unwrap_or_default()
                };
                count += 1;
                println!(
                    "  {:<40}  {:<28}  {}",
                    truncate(&field("from:"), 40),
                    truncate(&field("date:"), 28),
                    truncate(&field("subject:"), 60),
                );
            }
        }
        println!("  ({count} headers)");
    }

    let _ = session.logout().await;
    Ok(())
}

/// CalDAV discovery (E11.12) plus the RFC 6578 `sync-collection` probe (E11.13).
pub fn caldav(full_address: &str, password: &str) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let dav = |method: &str, url: &str, depth: &str, body: &str| -> Result<(reqwest::StatusCode, String)> {
        let response = client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).expect("valid method"),
                url,
            )
            .basic_auth(full_address, Some(password))
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body.to_string())
            .send()
            .with_context(|| format!("{method} {url}"))?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        Ok((status, text))
    };

    // iCloud answers a GET on /.well-known/caldav with 400; it wants a PROPFIND for
    // current-user-principal. Then the principal yields calendar-home-set (E11.12).
    let (status, body) = dav(
        "PROPFIND",
        "https://caldav.icloud.com/.well-known/caldav",
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?>
           <d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#,
    )?;
    println!("PROPFIND /.well-known/caldav -> {status}");
    let principal = find_href_after(&normalize_dav(&body), "current-user-principal")
        .ok_or_else(|| anyhow::anyhow!("no current-user-principal in well-known response"))?;
    let principal = absolute("https://caldav.icloud.com", &principal);
    println!("  current-user-principal: {principal}");

    let (status, body) = dav(
        "PROPFIND",
        &principal,
        "0",
        r#"<?xml version="1.0" encoding="utf-8"?>
           <d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
             <d:prop><c:calendar-home-set/></d:prop>
           </d:propfind>"#,
    )?;
    println!("PROPFIND principal -> {status}");
    let home = find_href_after(&normalize_dav(&body), "calendar-home-set")
        .ok_or_else(|| anyhow::anyhow!("no calendar-home-set in principal response"))?;
    let home = absolute("https://caldav.icloud.com", &home);
    println!("  calendar-home-set: {home}");

    let (status, body) = dav(
        "PROPFIND",
        &home,
        "1",
        r#"<?xml version="1.0" encoding="utf-8"?>
           <d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
             <d:prop>
               <d:displayname/><d:resourcetype/><d:getctag/>
               <c:supported-calendar-component-set/>
               <d:supported-report-set/>
             </d:prop>
           </d:propfind>"#,
    )?;
    println!("PROPFIND home (Depth 1) -> {status}");

    let calendars = split_responses(&normalize_dav(&body));
    let origin = origin_of(&home);
    println!("  {} collection responses (origin {origin})", calendars.len());
    let home_path = home.trim_end_matches('/').to_string();
    let mut first_calendar = None;
    for response in &calendars {
        let Some(href) = find_href_after(response, "response") else {
            continue;
        };
        let full = absolute(&origin, &href);
        let name = find_text_after(response, "displayname").unwrap_or_default();
        let ctag = find_text_after(response, "getctag").unwrap_or_default();
        let kind = if response.contains("calendar") { "calendar" } else { "" };
        let has_sync_collection = response.contains("sync-collection");
        println!(
            "    {:<62} name={:<20} ctag={:<10} {kind}{}",
            truncate(&full, 62),
            truncate(&name, 20),
            truncate(&ctag, 10),
            if has_sync_collection { " [sync-collection]" } else { "" }
        );
        let is_special = ["/inbox/", "/outbox/", "/notification/"]
            .iter()
            .any(|part| full.contains(part));
        let is_home = full.trim_end_matches('/') == home_path;
        if kind == "calendar" && !is_special && !is_home && first_calendar.is_none() {
            first_calendar = Some((full, has_sync_collection));
        }
    }

    if let Some((url, has_sync_collection)) = first_calendar {
        println!(
            "\nRFC 6578 sync-collection advertised on a real calendar: {}",
            if has_sync_collection { "YES" } else { "no" }
        );
        // A bounded time-range: iCloud rejects far-past/far-future ranges (E11.16).
        let (status, body) = dav(
            "REPORT",
            &url,
            "1",
            r#"<?xml version="1.0" encoding="utf-8"?>
               <c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
                 <d:prop><d:getetag/><c:calendar-data/></d:prop>
                 <c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT">
                   <c:time-range start="20260101T000000Z" end="20260201T000000Z"/>
                 </c:comp-filter></c:comp-filter></c:filter>
               </c:calendar-query>"#,
        )?;
        println!("calendar-query REPORT -> {status}, {} bytes", body.len());
    }

    Ok(())
}

/// The `scheme://host[:port]` prefix of a URL, for resolving relative DAV hrefs.
fn origin_of(url: &str) -> String {
    match url.find("://") {
        Some(scheme_end) => {
            let after = &url[scheme_end + 3..];
            let host_end = after.find('/').unwrap_or(after.len());
            url[..scheme_end + 3 + host_end].to_string()
        }
        None => String::new(),
    }
}

/// Strip XML namespace prefixes so the naive helpers can read iCloud's default-namespace bodies
/// (`<response xmlns="DAV:">`) and prefixed ones (`<d:response>`) with the same code.
fn normalize_dav(xml: &str) -> String {
    let mut out = xml.to_string();
    for prefix in ["d:", "D:", "c:", "C:", "cs:", "CS:", "cal:", "x:"] {
        out = out
            .replace(&format!("<{prefix}"), "<")
            .replace(&format!("</{prefix}"), "</");
    }
    out
}

fn absolute(host: &str, href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else {
        format!("{host}{href}")
    }
}

fn truncate(input: &str, max: usize) -> String {
    let cleaned = input.trim();
    if cleaned.chars().count() <= max {
        cleaned.to_string()
    } else {
        let head: String = cleaned.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Find the text of the first `<...href ...>...</...href>` after `tag`.
fn find_href_after(xml: &str, tag: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let start = lower.find(&tag.to_ascii_lowercase())?;
    let href_at = lower[start..].find("<href")? + start;
    let open = lower[href_at..].find('>')? + href_at;
    let close = lower[open..].find('<')? + open;
    Some(xml[open + 1..close].trim().to_string())
}

/// Find the text of the first `<tag ...>...</tag>` (self-closing yields empty).
fn find_text_after(xml: &str, tag: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let at = lower.find(&format!("<{tag}"))?;
    let open = lower[at..].find('>')? + at;
    let close = lower[open..].find('<')? + open;
    Some(xml[open + 1..close].trim().to_string())
}

/// Split a multistatus body into per-`<response>` chunks.
fn split_responses(xml: &str) -> Vec<String> {
    let lower = xml.to_ascii_lowercase();
    let mut chunks = Vec::new();
    let mut cursor = 0;
    while let Some(open) = lower[cursor..].find("<response") {
        let open = cursor + open;
        if let Some(close) = lower[open..].find("</response>") {
            let end = open + close + "</response>".len();
            chunks.push(xml[open..end].to_string());
            cursor = end;
        } else {
            break;
        }
    }
    chunks
}
