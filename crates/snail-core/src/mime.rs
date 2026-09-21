//! MIME parsing (plan.md E4.22, feeding E6.1). `mail-parser` does the work; this is the shape the
//! store and the renderer consume, plus the body-selection rule E0.3 established.

use anyhow::{Context, Result};
use mail_parser::{MimeHeaders, MessageParser};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Recipient {
    pub name: Option<String>,
    pub address: String,
}

/// The display body. Selection follows E0.3: a real `text/html` part wins; otherwise the plain
/// text. `body_html(pos)` alone cannot be trusted because it *converts* plain text to HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    Html(String),
    Text(String),
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content_id: Option<String>,
    pub size: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ParsedMessage {
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub to: Vec<Recipient>,
    pub date: Option<i64>,
    pub preview: Option<String>,
    pub body: Body,
    pub attachments: Vec<Attachment>,
}

impl Default for Body {
    fn default() -> Self {
        Body::None
    }
}

pub const PREVIEW_CHARS: usize = 240;

/// Parse a raw RFC822 message into the fields the store keeps.
pub fn parse_raw(bytes: &[u8]) -> Result<ParsedMessage> {
    let message = MessageParser::default()
        .parse(bytes)
        .context("mail-parser rejected the message")?;

    let (from_name, from_addr) = match message.from().and_then(|address| address.first()) {
        Some(addr) => (
            addr.name().map(str::to_string),
            addr.address().map(str::to_string),
        ),
        None => (None, None),
    };

    let to = message
        .to()
        .map(|address| {
            address
                .iter()
                .filter_map(|addr| {
                    addr.address().map(|value| Recipient {
                        name: addr.name().map(str::to_string),
                        address: value.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let body = if message.html_bodies().any(|part| part.is_text_html()) {
        Body::Html(message.body_html(0).map(|c| c.into_owned()).unwrap_or_default())
    } else if let Some(text) = message.body_text(0) {
        Body::Text(text.into_owned())
    } else {
        Body::None
    };

    // The preview comes from the plain-text view of whichever body we chose.
    let preview_source = message
        .body_text(0)
        .map(|c| c.into_owned())
        .unwrap_or_default();
    let preview = Some(collapse(&preview_source, PREVIEW_CHARS)).filter(|p| !p.is_empty());

    let attachments = message
        .attachments()
        .map(|part| Attachment {
            filename: part.attachment_name().map(str::to_string),
            content_type: part.content_type().map(|content_type| {
                format!(
                    "{}/{}",
                    content_type.ctype(),
                    content_type.subtype().unwrap_or("")
                )
            }),
            content_id: None,
            size: part.len(),
        })
        .collect();

    Ok(ParsedMessage {
        // Angle brackets are on some headers and not others; normalize so threading (E8.1) matches.
        message_id: message.message_id().map(normalize_id),
        in_reply_to: header_text(message.in_reply_to()).map(|value| normalize_id(&value)),
        references: header_text(message.references()).map(|value| normalize_references(&value)),
        subject: message.subject().map(str::to_string),
        from_name,
        from_addr,
        to,
        date: message.date().map(|date| date.to_timestamp()),
        preview,
        body,
        attachments,
    })
}

fn header_text(value: &mail_parser::HeaderValue<'_>) -> Option<String> {
    use mail_parser::HeaderValue;
    let text = match value {
        HeaderValue::Text(text) => text.to_string(),
        HeaderValue::TextList(list) => list.join(" "),
        HeaderValue::Address(address) => {
            address.first().and_then(|addr| addr.address()).unwrap_or("").to_string()
        }
        _ => String::new(),
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Collapse whitespace and clamp to `max` characters.
pub fn collapse(input: &str, max: usize) -> String {
    let collapsed = input.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(max).collect()
}

/// Strip the angle brackets a Message-ID may or may not carry.
pub fn normalize_id(input: &str) -> String {
    let trimmed = input.trim();
    let trimmed = trimmed.strip_prefix('<').unwrap_or(trimmed);
    let trimmed = trimmed.strip_suffix('>').unwrap_or(trimmed);
    trimmed.to_string()
}

/// Normalize a `References:` header into space-separated bare ids.
pub fn normalize_references(input: &str) -> String {
    input
        .split_whitespace()
        .map(normalize_id)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: &[u8] = b"From: Maya Okonkwo <maya@example.com>\r\n\
To: Me <me@example.com>, Second <two@example.com>\r\n\
Subject: Re: Almanac geometry\r\n\
Message-ID: <m2@example.com>\r\n\
In-Reply-To: <m1@example.com>\r\n\
References: <m0@example.com> <m1@example.com>\r\n\
Date: Mon, 21 Sep 2026 10:00:00 -0500\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
The 6x7 grid maths checks out.\r\n";

    #[test]
    fn parses_threading_headers_and_a_plain_body() {
        let parsed = parse_raw(PLAIN).unwrap();
        assert_eq!(parsed.subject.as_deref(), Some("Re: Almanac geometry"));
        assert_eq!(parsed.message_id.as_deref(), Some("m2@example.com"));
        assert_eq!(parsed.in_reply_to.as_deref(), Some("m1@example.com"));
        assert!(parsed.references.as_deref().unwrap().contains("m0@example.com"));
        assert_eq!(parsed.from_name.as_deref(), Some("Maya Okonkwo"));
        assert_eq!(parsed.from_addr.as_deref(), Some("maya@example.com"));
        assert_eq!(parsed.to.len(), 2);
        assert_eq!(parsed.to[1].address, "two@example.com");
        assert!(matches!(parsed.body, Body::Text(_)));
        assert!(parsed.preview.unwrap().starts_with("The 6x7 grid"));
        assert!(parsed.date.is_some());
    }

    #[test]
    fn a_real_html_part_wins_over_plain_text() {
        let raw = b"From: a@b.c\r\nSubject: hi\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=BB\r\n\r\n\
--BB\r\nContent-Type: text/plain\r\n\r\nplain version\r\n\
--BB\r\nContent-Type: text/html\r\n\r\n<p>html version</p>\r\n--BB--\r\n";
        let parsed = parse_raw(raw).unwrap();
        match parsed.body {
            Body::Html(html) => assert!(html.contains("html version")),
            other => panic!("expected html, got {other:?}"),
        }
    }

    #[test]
    fn a_plain_only_message_stays_plain() {
        let parsed = parse_raw(PLAIN).unwrap();
        assert!(matches!(parsed.body, Body::Text(_)), "plain must not be converted to html");
    }

    #[test]
    fn attachments_are_listed() {
        let raw = b"From: a@b.c\r\nSubject: files\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=BB\r\n\r\n\
--BB\r\nContent-Type: text/plain\r\n\r\nsee attached\r\n\
--BB\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\r\nPDFBYTES\r\n--BB--\r\n";
        let parsed = parse_raw(raw).unwrap();
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(parsed.attachments[0].filename.as_deref(), Some("report.pdf"));
        assert!(parsed.attachments[0].size > 0);
    }
}
