//! iCloud CalDAV synchronization (plan.md E11.12–E11.19).
//!
//! Server-provided hrefs are authoritative: response paths are never compared to request paths,
//! resource names are never derived from UID, and this client intentionally has no calendar-create
//! operation.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::blocking::{Client, Response};
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH, LOCATION, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use url::Url;

use crate::calendar::{
    CalendarBatch, CalendarCursor, CalendarInfo, CalendarProvider, CalendarWrite, EventChange,
    EventIdentity, WriteOutcome,
};
use crate::ical::parse_event_resource;

const DEFAULT_ROOT: &str = "https://caldav.icloud.com/";
const MAX_DAV_BYTES: usize = 16 * 1024 * 1024;
const MAX_RETRIES: usize = 4;
const MAX_REDIRECTS: usize = 10;
pub const MAX_QUERY_SPAN_DAYS: i64 = 366 * 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalDavErrorKind {
    CredentialRevoked,
    Transient,
    Rediscover,
    Other,
}

#[derive(Debug)]
pub struct CalDavError {
    pub kind: CalDavErrorKind,
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for CalDavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CalDAV {}: {}", self.status, self.message)
    }
}
impl std::error::Error for CalDavError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DavResource {
    pub href: String,
    pub status: Option<u16>,
    pub display_name: Option<String>,
    pub etag: Option<String>,
    pub ctag: Option<String>,
    pub color: Option<String>,
    pub principal_href: Option<String>,
    pub calendar_home_href: Option<String>,
    pub schedule_inbox_href: Option<String>,
    pub schedule_outbox_href: Option<String>,
    pub calendar_data: Option<String>,
    pub is_calendar: bool,
    pub supports_vevent: bool,
    pub supports_sync_collection: bool,
    pub can_write: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DavMultistatus {
    pub responses: Vec<DavResource>,
    pub sync_token: Option<String>,
}

pub struct CalDavClient {
    http: Client,
    username: String,
    password: String,
    /// Retained only for this process; an unexpected 404 asks discovery to resolve the canonical
    /// caldav.icloud.com endpoint again rather than persisting a stale pNN shard.
    shard_root: Mutex<Url>,
}

#[derive(Clone, Copy)]
enum WritePrecondition<'a> {
    None,
    Match(&'a str),
    Create,
}

impl CalDavClient {
    pub fn new(username: &str, app_password: &str) -> Result<Self> {
        Self::with_root(username, app_password, DEFAULT_ROOT)
    }

    pub fn with_root(username: &str, app_password: &str, root: &str) -> Result<Self> {
        let root = Url::parse(root)?;
        if root.scheme() != "https" {
            bail!("CalDAV requires HTTPS");
        }
        Ok(Self {
            http: Client::builder()
                // Follow redirects ourselves so a cross-host iCloud shard redirect keeps Basic
                // auth and the original PROPFIND/REPORT method. Reqwest correctly strips
                // Authorization on automatic cross-origin redirects.
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .user_agent("snail/0.1 (caldav)")
                .build()?,
            username: username.into(),
            password: app_password.into(),
            shard_root: Mutex::new(root),
        })
    }

    fn root(&self) -> Url {
        self.shard_root.lock().expect("CalDAV shard mutex").clone()
    }

    fn resolve(&self, href: &str) -> Result<Url> {
        Ok(self.root().join(href)?)
    }

    fn request(
        &self,
        method: Method,
        url: Url,
        depth: Option<&str>,
        body: Option<&str>,
        content_type: Option<&str>,
        precondition: WritePrecondition<'_>,
    ) -> Result<Response> {
        let mut current_url = url;
        'retry: for attempt in 0..MAX_RETRIES {
            let mut redirects = 0;
            loop {
                let mut request = self
                    .http
                    .request(method.clone(), current_url.clone())
                    .basic_auth(&self.username, Some(&self.password));
                if let Some(depth) = depth {
                    request = request.header("Depth", depth);
                }
                if let Some(content_type) = content_type {
                    request = request.header(CONTENT_TYPE, content_type);
                }
                match precondition {
                    WritePrecondition::None => {}
                    WritePrecondition::Match(etag) => {
                        request = request.header(IF_MATCH, etag);
                    }
                    WritePrecondition::Create => {
                        // A new opaque resource must not overwrite an object another client created.
                        request = request.header(IF_NONE_MATCH, "*");
                    }
                }
                if let Some(body) = body {
                    request = request.body(body.to_owned());
                }
                let response = request.send()?;
                let status = response.status();
                if matches!(status.as_u16(), 301 | 302 | 307 | 308) {
                    if redirects == MAX_REDIRECTS {
                        bail!("CalDAV exceeded {MAX_REDIRECTS} redirects");
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .and_then(|value| value.to_str().ok())
                        .context("CalDAV redirect had no valid Location")?;
                    let next = current_url.join(location)?;
                    if next.scheme() != "https" {
                        bail!("CalDAV refused a non-HTTPS redirect");
                    }
                    if !trusted_redirect(&current_url, &next) {
                        bail!("CalDAV refused to send credentials to redirect host");
                    }
                    current_url = next;
                    redirects += 1;
                    continue;
                }
                if status == StatusCode::UNAUTHORIZED {
                    return Err(anyhow!(CalDavError {
                        kind: CalDavErrorKind::CredentialRevoked,
                        status: 401,
                        message: "iCloud rejected the app-specific password".into(),
                    }));
                }
                if matches!(status.as_u16(), 403 | 429 | 503) {
                    if attempt + 1 == MAX_RETRIES {
                        return Err(anyhow!(CalDavError {
                            kind: CalDavErrorKind::Transient,
                            status: status.as_u16(),
                            message: "rate limited; retry later".into(),
                        }));
                    }
                    thread::sleep(retry_delay(&response, attempt));
                    continue 'retry;
                }
                return Ok(response);
            }
        }
        unreachable!()
    }

    fn dav(&self, method: Method, url: Url, depth: &str, body: &str) -> Result<DavMultistatus> {
        let response = self.request(
            method,
            url,
            Some(depth),
            Some(body),
            Some("application/xml; charset=utf-8"),
            WritePrecondition::None,
        )?;
        if response.status() == StatusCode::NOT_FOUND {
            *self.shard_root.lock().expect("CalDAV shard mutex") = Url::parse(DEFAULT_ROOT)?;
            return Err(anyhow!(CalDavError {
                kind: CalDavErrorKind::Rediscover,
                status: 404,
                message: "the cached iCloud shard disappeared; rediscover from caldav.icloud.com"
                    .into(),
            }));
        }
        if !response.status().is_success() && response.status() != StatusCode::MULTI_STATUS {
            return Err(status_error(response));
        }
        let final_url = response.url().clone();
        let mut shard = final_url;
        shard.set_path("/");
        shard.set_query(None);
        *self.shard_root.lock().expect("CalDAV shard mutex") = shard;
        let bytes = response.bytes()?;
        if bytes.len() > MAX_DAV_BYTES {
            bail!("CalDAV multistatus exceeds {MAX_DAV_BYTES} bytes");
        }
        parse_multistatus(std::str::from_utf8(&bytes).context("CalDAV XML is not UTF-8")?)
    }

    fn discover(&self) -> Result<Vec<CalendarInfo>> {
        let principal = self.dav(
            propfind(),
            self.root().join("/.well-known/caldav")?,
            "0",
            PRINCIPAL_BODY,
        )?;
        let principal_href = principal
            .responses
            .iter()
            .find_map(|r| r.principal_href.as_deref())
            .context("CalDAV returned no current-user-principal")?;
        let home = self.dav(propfind(), self.resolve(principal_href)?, "0", HOME_BODY)?;
        let home_href = home
            .responses
            .iter()
            .find_map(|r| r.calendar_home_href.as_deref())
            .context("CalDAV returned no calendar-home-set")?;
        let supports_scheduling = home.responses.iter().any(|resource| {
            resource.schedule_inbox_href.is_some() && resource.schedule_outbox_href.is_some()
        });
        let collections = self.dav(propfind(), self.resolve(home_href)?, "1", DISCOVERY_BODY)?;
        Ok(collections
            .responses
            .into_iter()
            .filter(|r| r.is_calendar && r.supports_vevent)
            .map(|r| CalendarInfo {
                remote_id: r.href.clone(),
                href: self.resolve(&r.href).ok().map(|url| url.to_string()),
                name: r.display_name,
                color: r.color.as_deref().map(normalize_calendar_color),
                selected: true,
                access_role: Some(if r.can_write { "writer" } else { "reader" }.into()),
                read_only: !r.can_write,
                subscribed: true,
                ctag: r.ctag,
                supports_sync_collection: r.supports_sync_collection,
                supports_scheduling,
            })
            .collect())
    }

    fn multiget(&self, calendar: &CalendarInfo, hrefs: &[String]) -> Result<Vec<EventChange>> {
        if hrefs.is_empty() {
            return Ok(Vec::new());
        }
        self.dav(
            report(),
            calendar_url(calendar)?,
            "1",
            &multiget_body(hrefs),
        )?
        .responses
        .into_iter()
        .filter(|r| r.status.unwrap_or(200) < 400)
        .filter_map(|r| {
            r.calendar_data.map(|data| {
                parse_event_resource(&r.href, r.etag.as_deref(), &data)
                    .map(Box::new)
                    .map(EventChange::Upsert)
            })
        })
        .collect()
    }

    fn sync_collection(
        &self,
        calendar: &CalendarInfo,
        token: Option<&str>,
    ) -> Result<CalendarBatch> {
        let status = self.dav(
            report(),
            calendar_url(calendar)?,
            "1",
            &sync_collection_body(token),
        )?;
        let mut batch = CalendarBatch {
            next_sync_token: status.sync_token,
            ..Default::default()
        };
        let mut changed = Vec::new();
        for resource in status.responses {
            if same_resource(&resource.href, calendar) {
                continue;
            }
            if resource.status == Some(404) {
                batch
                    .changes
                    .push(EventChange::Delete(EventIdentity::Event(resource.href)));
            } else {
                changed.push(resource.href);
            }
        }
        batch.changes.extend(self.multiget(calendar, &changed)?);
        Ok(batch)
    }

    fn ctag_sync(&self, calendar: &CalendarInfo, cursor: &CalendarCursor) -> Result<CalendarBatch> {
        let ctag = self.dav(propfind(), calendar_url(calendar)?, "0", CTAG_BODY)?;
        let next_ctag = ctag.responses.iter().find_map(|r| r.ctag.clone());
        if cursor.ctag.is_some() && cursor.ctag == next_ctag {
            return Ok(CalendarBatch {
                next_ctag,
                ..Default::default()
            });
        }
        let listing = self.dav(propfind(), calendar_url(calendar)?, "1", ETAG_BODY)?;
        let old: HashMap<&str, &str> = cursor
            .etags
            .iter()
            .map(|(href, etag)| (href.as_str(), etag.as_str()))
            .collect();
        let mut current = HashSet::new();
        let mut changed = Vec::new();
        for resource in listing.responses {
            if same_resource(&resource.href, calendar) {
                continue;
            }
            current.insert(resource.href.clone());
            if resource
                .etag
                .as_deref()
                .is_none_or(|etag| old.get(resource.href.as_str()).copied() != Some(etag))
            {
                changed.push(resource.href);
            }
        }
        let mut batch = CalendarBatch {
            next_ctag,
            ..Default::default()
        };
        batch.changes.extend(self.multiget(calendar, &changed)?);
        for (href, _) in &cursor.etags {
            if !current.contains(href) {
                batch
                    .changes
                    .push(EventChange::Delete(EventIdentity::Event(href.clone())));
            }
        }
        Ok(batch)
    }

    fn get_resource(&self, href: &str) -> Result<(String, Option<String>)> {
        let response = self.request(
            Method::GET,
            self.resolve(href)?,
            None,
            None,
            None,
            WritePrecondition::None,
        )?;
        if response.status() == StatusCode::NOT_FOUND {
            *self.shard_root.lock().expect("CalDAV shard mutex") = Url::parse(DEFAULT_ROOT)?;
        }
        if !response.status().is_success() {
            return Err(status_error(response));
        }
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        Ok((response.text()?, etag))
    }
}

impl CalendarProvider for CalDavClient {
    fn discover_calendars(&self, _cursor: Option<&str>) -> Result<CalendarBatch> {
        Ok(CalendarBatch {
            calendars: self.discover()?,
            ..Default::default()
        })
    }

    fn sync_calendar(
        &self,
        calendar: &CalendarInfo,
        cursor: &CalendarCursor,
    ) -> Result<CalendarBatch> {
        if calendar.supports_sync_collection {
            self.sync_collection(calendar, cursor.sync_token.as_deref())
        } else {
            self.ctag_sync(calendar, cursor)
        }
    }

    fn apply(&self, writes: &[CalendarWrite]) -> Result<Vec<WriteOutcome>> {
        let mut outcomes = Vec::with_capacity(writes.len());
        for write in writes {
            let (calendar, event, base_etag, deleting) = match write {
                CalendarWrite::Put {
                    calendar,
                    event,
                    base_etag,
                } => (calendar, event, base_etag, false),
                CalendarWrite::Delete {
                    calendar,
                    event,
                    base_etag,
                } => (calendar, event, base_etag, true),
            };
            let generated_href;
            let create_only = event.href.is_none() && !deleting;
            let href = match event.href.as_deref() {
                Some(href) => href,
                None if !deleting => {
                    // New resources need a filename; choose an opaque random one, never the UID.
                    generated_href = calendar_url(calendar)?
                        .join(&format!("{:032x}.ics", rand::random::<u128>()))?
                        .to_string();
                    generated_href.as_str()
                }
                None => anyhow::bail!("CalDAV delete has no server-provided href"),
            };
            let response = if deleting {
                self.request(
                    Method::DELETE,
                    self.resolve(href)?,
                    None,
                    None,
                    None,
                    base_etag
                        .as_deref()
                        .map(WritePrecondition::Match)
                        .unwrap_or(WritePrecondition::None),
                )?
            } else {
                // Existing resources without a cached ETag must be refreshed before the PUT.
                // `If-None-Match: *` is reserved for the opaque URL generated above.
                let refreshed_etag = if !create_only && base_etag.is_none() {
                    self.get_resource(href)?.1
                } else {
                    None
                };
                self.request(
                    Method::PUT,
                    self.resolve(href)?,
                    None,
                    Some(
                        event
                            .raw_ical
                            .as_deref()
                            .context("CalDAV PUT has no iCalendar body")?,
                    ),
                    Some("text/calendar; charset=utf-8"),
                    if create_only {
                        WritePrecondition::Create
                    } else {
                        base_etag
                            .as_deref()
                            .or(refreshed_etag.as_deref())
                            .map(WritePrecondition::Match)
                            .unwrap_or(WritePrecondition::None)
                    },
                )?
            };
            if response.status() == StatusCode::PRECONDITION_FAILED {
                let server = self
                    .get_resource(href)
                    .ok()
                    .and_then(|(raw, etag)| parse_event_resource(href, etag.as_deref(), &raw).ok());
                outcomes.push(WriteOutcome::Conflict {
                    local: Box::new(event.clone()),
                    server: server.map(Box::new),
                });
            } else if !response.status().is_success() && response.status() != StatusCode::NOT_FOUND
            {
                return Err(status_error(response));
            } else if deleting {
                outcomes.push(WriteOutcome::Deleted(event.identity()));
            } else if let Some(etag) = response
                .headers()
                .get(ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
            {
                let mut applied = event.clone();
                applied.href = Some(href.to_string());
                if applied.remote_id.is_empty() {
                    applied.remote_id = href.to_string();
                }
                applied.etag = Some(etag);
                outcomes.push(WriteOutcome::Applied(Box::new(applied)));
            } else {
                // Apple may normalize the body and omit ETag. GET canonical bytes + real ETag.
                let (canonical, etag) = self.get_resource(href)?;
                outcomes.push(WriteOutcome::Applied(Box::new(parse_event_resource(
                    href,
                    etag.as_deref(),
                    &canonical,
                )?)));
            }
        }
        Ok(outcomes)
    }
}

fn calendar_url(calendar: &CalendarInfo) -> Result<Url> {
    Url::parse(calendar.href.as_deref().unwrap_or(&calendar.remote_id))
        .context("calendar has no absolute server href")
}

fn trusted_redirect(from: &Url, to: &Url) -> bool {
    if from.host_str() == to.host_str() {
        return true;
    }
    let is_icloud_caldav = |host: &str| {
        host == "caldav.icloud.com"
            || host
                .strip_prefix('p')
                .and_then(|host| host.strip_suffix("-caldav.icloud.com"))
                .is_some_and(|shard| !shard.is_empty() && shard.bytes().all(|b| b.is_ascii_digit()))
    };
    from.host_str().is_some_and(is_icloud_caldav) && to.host_str().is_some_and(is_icloud_caldav)
}

fn retry_delay(response: &Response, attempt: usize) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|value| retry_after_value(value, chrono::Utc::now().timestamp()))
        .unwrap_or_else(|| Duration::from_secs(1_u64 << attempt.min(6)))
}

fn retry_after_value(value: &str, now_utc: i64) -> Option<Duration> {
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|deadline| deadline.timestamp().saturating_sub(now_utc))
                .and_then(|seconds| u64::try_from(seconds).ok())
                .map(Duration::from_secs)
        })
}

fn status_error(response: Response) -> anyhow::Error {
    let status = response.status();
    anyhow!(CalDavError {
        kind: if status == StatusCode::NOT_FOUND {
            CalDavErrorKind::Rediscover
        } else if matches!(status.as_u16(), 403 | 429 | 503) {
            CalDavErrorKind::Transient
        } else {
            CalDavErrorKind::Other
        },
        status: status.as_u16(),
        message: status.canonical_reason().unwrap_or("request failed").into(),
    })
}

fn propfind() -> Method {
    Method::from_bytes(b"PROPFIND").unwrap()
}
fn report() -> Method {
    Method::from_bytes(b"REPORT").unwrap()
}

const PRINCIPAL_BODY: &str = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:current-user-principal/></d:prop></d:propfind>";
const HOME_BODY: &str = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><c:calendar-home-set/><c:schedule-inbox-URL/><c:schedule-outbox-URL/></d:prop></d:propfind>";
const DISCOVERY_BODY: &str = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:cs=\"http://calendarserver.org/ns/\" xmlns:a=\"http://apple.com/ns/ical/\"><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/><cs:getctag/><a:calendar-color/><d:current-user-privilege-set/><d:supported-report-set/></d:prop></d:propfind>";
const CTAG_BODY: &str = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\" xmlns:cs=\"http://calendarserver.org/ns/\"><d:prop><cs:getctag/></d:prop></d:propfind>";
const ETAG_BODY: &str = "<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:getetag/><d:resourcetype/></d:prop></d:propfind>";

fn sync_collection_body(token: Option<&str>) -> String {
    let token = token
        .map(|token| format!("<d:sync-token>{}</d:sync-token>", xml_escape(token)))
        .unwrap_or_default();
    format!(
        "<?xml version=\"1.0\"?><d:sync-collection xmlns:d=\"DAV:\">{token}<d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>"
    )
}
fn multiget_body(hrefs: &[String]) -> String {
    let hrefs = hrefs
        .iter()
        .map(|h| format!("<d:href>{}</d:href>", xml_escape(h)))
        .collect::<String>();
    format!(
        "<?xml version=\"1.0\"?><c:calendar-multiget xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}</c:calendar-multiget>"
    )
}

/// All time-range queries are bounded, avoiding CalendarServer's 403 max-date-time response.
pub fn bounded_calendar_query(start_utc: i64, end_utc: i64) -> Result<String> {
    if end_utc <= start_utc || end_utc - start_utc > MAX_QUERY_SPAN_DAYS * 86_400 {
        bail!("CalDAV query range is invalid or exceeds {MAX_QUERY_SPAN_DAYS} days");
    }
    let start = chrono::DateTime::from_timestamp(start_utc, 0)
        .context("invalid start")?
        .format("%Y%m%dT%H%M%SZ");
    let end = chrono::DateTime::from_timestamp(end_utc, 0)
        .context("invalid end")?
        .format("%Y%m%dT%H%M%SZ");
    Ok(format!(
        "<?xml version=\"1.0\"?><c:calendar-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"><c:time-range start=\"{start}\" end=\"{end}\"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"
    ))
}

fn same_resource(href: &str, calendar: &CalendarInfo) -> bool {
    let calendar = calendar.href.as_deref().unwrap_or(&calendar.remote_id);
    href.trim_end_matches('/') == calendar.trim_end_matches('/')
}
fn normalize_calendar_color(value: &str) -> String {
    if value.starts_with('#') && value.len() == 9 {
        value[..7].to_string()
    } else {
        value.to_string()
    }
}
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn parse_multistatus(xml: &str) -> Result<DavMultistatus> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<String>::new();
    let mut out = DavMultistatus::default();
    let mut current: Option<DavResource> = None;
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                let name = local_name(start.name().as_ref());
                if name == "response" {
                    current = Some(DavResource::default());
                }
                mark_element(
                    &name,
                    &stack,
                    start.attributes().flatten().filter_map(|a| {
                        let key = local_name(a.key.as_ref());
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()?
                            .into_owned();
                        Some((key, value))
                    }),
                    current.as_mut(),
                );
                stack.push(name);
            }
            Event::Empty(empty) => {
                let name = local_name(empty.name().as_ref());
                mark_element(
                    &name,
                    &stack,
                    empty.attributes().flatten().filter_map(|a| {
                        let key = local_name(a.key.as_ref());
                        let value = a
                            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            .ok()?
                            .into_owned();
                        Some((key, value))
                    }),
                    current.as_mut(),
                );
            }
            Event::Text(text) => {
                assign_text(
                    &stack,
                    text.decode()?.into_owned(),
                    current.as_mut(),
                    &mut out,
                );
            }
            Event::CData(text) => {
                assign_text(
                    &stack,
                    text.decode()?.into_owned(),
                    current.as_mut(),
                    &mut out,
                );
            }
            Event::End(end) => {
                if local_name(end.name().as_ref()) == "response"
                    && let Some(resource) = current.take()
                {
                    out.responses.push(resource);
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn mark_element(
    name: &str,
    stack: &[String],
    attrs: impl Iterator<Item = (String, String)>,
    resource: Option<&mut DavResource>,
) {
    let Some(resource) = resource else { return };
    if name == "calendar" && stack.last().is_some_and(|n| n == "resourcetype") {
        resource.is_calendar = true;
    } else if name == "sync-collection" && stack.iter().any(|n| n == "supported-report-set") {
        resource.supports_sync_collection = true;
    } else if name == "write" && stack.iter().any(|n| n == "privilege") {
        resource.can_write = true;
    } else if name == "comp"
        && attrs
            .into_iter()
            .any(|(k, v)| k == "name" && v.eq_ignore_ascii_case("VEVENT"))
    {
        resource.supports_vevent = true;
    }
}

fn assign_text(
    stack: &[String],
    value: String,
    resource: Option<&mut DavResource>,
    out: &mut DavMultistatus,
) {
    let name = stack.last().map(String::as_str).unwrap_or("");
    let parent = stack.iter().rev().nth(1).map(String::as_str).unwrap_or("");
    if let Some(resource) = resource {
        match name {
            "href" if stack.iter().any(|n| n == "current-user-principal") => {
                resource.principal_href = Some(value)
            }
            "href" if stack.iter().any(|n| n == "calendar-home-set") => {
                resource.calendar_home_href = Some(value)
            }
            "href" if stack.iter().any(|n| n == "schedule-inbox-url") => {
                resource.schedule_inbox_href = Some(value)
            }
            "href" if stack.iter().any(|n| n == "schedule-outbox-url") => {
                resource.schedule_outbox_href = Some(value)
            }
            "href" if parent == "response" => resource.href = value,
            "displayname" => resource.display_name = Some(value),
            "getetag" => resource.etag = Some(value),
            "getctag" => resource.ctag = Some(value),
            "calendar-color" => resource.color = Some(value),
            "calendar-data" => resource.calendar_data = Some(value),
            "status" => resource.status = status_code(&value),
            _ => {}
        }
    } else if name == "sync-token" {
        out.sync_token = Some(value);
    }
}

fn local_name(name: &[u8]) -> String {
    let name = std::str::from_utf8(name).unwrap_or_default();
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}
fn status_code(value: &str) -> Option<u16> {
    value.split_ascii_whitespace().find_map(|p| p.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_accepts_iclouds_different_principal_path() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/803/principal/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/803/principal/</d:href></d:current-user-principal><c:calendar-home-set><d:href>/803/calendars/</d:href></c:calendar-home-set></d:prop></d:propstat></d:response></d:multistatus>";
        let resource = parse_multistatus(xml).unwrap().responses.remove(0);
        assert_eq!(resource.principal_href.as_deref(), Some("/803/principal/"));
        assert_eq!(
            resource.calendar_home_href.as_deref(),
            Some("/803/calendars/")
        );
    }

    #[test]
    fn capability_probe_filters_collection_itself() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/home/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response><d:response><d:href>/home/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name=\"VEVENT\"/></c:supported-calendar-component-set><d:supported-report-set><d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report></d:supported-report-set></d:prop></d:propstat></d:response></d:multistatus>";
        let parsed = parse_multistatus(xml).unwrap();
        assert!(!parsed.responses[0].is_calendar);
        assert!(parsed.responses[1].is_calendar);
        assert!(parsed.responses[1].supports_vevent);
        assert!(parsed.responses[1].supports_sync_collection);
    }

    #[test]
    fn scheduling_requires_both_rfc6638_endpoints() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:response><d:href>/principal/</d:href><d:propstat><d:prop><c:schedule-inbox-URL><d:href>/inbox/</d:href></c:schedule-inbox-URL><c:schedule-outbox-URL><d:href>/outbox/</d:href></c:schedule-outbox-URL></d:prop></d:propstat></d:response></d:multistatus>";
        let resource = parse_multistatus(xml).unwrap().responses.remove(0);
        assert_eq!(resource.schedule_inbox_href.as_deref(), Some("/inbox/"));
        assert_eq!(resource.schedule_outbox_href.as_deref(), Some("/outbox/"));
    }

    #[test]
    fn opaque_hrefs_and_etags_survive_parsing() {
        let xml = "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/cal/not-the-uid.ics</d:href><d:propstat><d:prop><d:getetag>&quot;7&quot;</d:getetag></d:prop></d:propstat></d:response></d:multistatus>";
        let resource = parse_multistatus(xml).unwrap().responses.remove(0);
        assert_eq!(resource.href, "/cal/not-the-uid.ics");
        assert_eq!(resource.etag.as_deref(), Some("7"));
    }

    #[test]
    fn time_range_is_bounded() {
        assert!(bounded_calendar_query(0, 86_400).is_ok());
        assert!(bounded_calendar_query(0, (MAX_QUERY_SPAN_DAYS + 1) * 86_400).is_err());
    }

    #[test]
    fn request_bodies_escape_server_values() {
        assert!(sync_collection_body(Some("a&b")).contains("a&amp;b"));
        assert!(multiget_body(&["/a&b.ics".into()]).contains("/a&amp;b.ics"));
    }

    #[test]
    fn initial_sync_omits_the_sync_token_element() {
        let body = sync_collection_body(None);
        assert!(!body.contains("sync-token"));
        assert!(body.contains("sync-level>1"));
    }

    #[test]
    fn apple_color_alpha_is_removed() {
        assert_eq!(normalize_calendar_color("#336699FF"), "#336699");
        assert_eq!(normalize_calendar_color("#336699"), "#336699");
    }

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_value("12", 0), Some(Duration::from_secs(12)));
        assert_eq!(
            retry_after_value("Sun, 06 Nov 1994 08:49:37 GMT", 784_111_767),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn credentials_only_follow_icloud_shard_or_same_host_redirects() {
        let canonical = Url::parse("https://caldav.icloud.com/.well-known/caldav").unwrap();
        let shard = Url::parse("https://p146-caldav.icloud.com/123/principal/").unwrap();
        let attacker = Url::parse("https://example.com/steal").unwrap();
        assert!(trusted_redirect(&canonical, &shard));
        assert!(trusted_redirect(&shard, &canonical));
        assert!(trusted_redirect(
            &attacker,
            &Url::parse("https://example.com/next").unwrap()
        ));
        assert!(!trusted_redirect(&canonical, &attacker));
    }
}
