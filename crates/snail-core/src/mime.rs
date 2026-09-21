//! MIME parsing (plan.md E4.22, feeding E6.1). `mail-parser` does the work; this is the shape the
//! store and the renderer consume, plus the body-selection rule E0.3 established.

use std::borrow::Cow;

use anyhow::{Context, Result};
use base64::Engine;
use mail_parser::{Encoding, MessageParser, MessagePart, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyPreference {
    Plain,
    #[default]
    Html,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldedPlain {
    pub visible: String,
    pub quoted: Option<String>,
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
    /// The full plain-text view, for the reading pane until E6 renders HTML.
    pub plain: Option<String>,
    pub body: Body,
    pub attachments: Vec<Attachment>,
}

impl Default for Body {
    fn default() -> Self {
        Body::None
    }
}

pub const PREVIEW_CHARS: usize = 240;

/// The plain-text body kept for full-text search (E9.1), capped so one pathological message cannot
/// bloat the store. The exact bytes still live content-addressed in the cache (E2.3).
pub const SEARCH_TEXT_CHARS: usize = 200_000;

/// The searchable plain text of a body: the whole plain-text view, clamped to [`SEARCH_TEXT_CHARS`].
pub fn search_text(plain: &str) -> String {
    plain.chars().take(SEARCH_TEXT_CHARS).collect()
}

/// Parse a raw RFC822 message into the fields the store keeps.
pub fn parse_raw(bytes: &[u8]) -> Result<ParsedMessage> {
    parse_raw_with_preference(bytes, BodyPreference::Html)
}

/// Parse a message while honoring the user's plain-vs-HTML reading preference. Only real MIME
/// parts participate: mail-parser's convenience accessors can synthesize HTML from a lone plain
/// part (the E0.3 selector trap).
pub fn parse_raw_with_preference(
    bytes: &[u8],
    preference: BodyPreference,
) -> Result<ParsedMessage> {
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

    let html = message.html_bodies().find_map(|part| {
        if !part.is_text_html() {
            return None;
        }
        decode_text_part(part, bytes).or_else(|| match &part.body {
            PartType::Html(html) => Some(html.to_string()),
            _ => None,
        })
    });
    let plain = message.text_bodies().find_map(|part| match &part.body {
        PartType::Text(text) => {
            let content_type = part.content_type();
            let flowed = content_type
                .and_then(|content_type| content_type.attribute("format"))
                .is_some_and(|format| format.eq_ignore_ascii_case("flowed"));
            let delsp = content_type
                .and_then(|content_type| content_type.attribute("delsp"))
                .is_some_and(|value| value.eq_ignore_ascii_case("yes"));
            let text = decode_text_part(part, bytes).unwrap_or_else(|| text.to_string());
            Some(if flowed {
                decode_format_flowed(&text, delsp)
            } else {
                text
            })
        }
        _ => None,
    });

    let body = match preference {
        BodyPreference::Html => html
            .clone()
            .map(Body::Html)
            .or_else(|| plain.clone().map(Body::Text))
            .unwrap_or_default(),
        BodyPreference::Plain => plain
            .clone()
            .map(Body::Text)
            .or_else(|| html.clone().map(Body::Html))
            .unwrap_or_default(),
    };

    // The preview comes from the plain-text view of whichever body we chose.
    let preview_source = plain
        .or_else(|| message.body_text(0).map(|text| text.into_owned()))
        .unwrap_or_default();
    let preview = Some(collapse(&preview_source, PREVIEW_CHARS)).filter(|p| !p.is_empty());
    let plain = Some(preview_source).filter(|text| !text.trim().is_empty());

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
            content_id: part.content_id().map(normalize_id),
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
        plain,
        body,
        attachments,
    })
}

fn decode_text_part(part: &MessagePart<'_>, raw_message: &[u8]) -> Option<String> {
    let decoded_by_parser = match &part.body {
        PartType::Text(text) | PartType::Html(text) => text,
        _ => return None,
    };
    if !part.is_encoding_problem && !decoded_by_parser.contains('\u{fffd}') {
        return None;
    }
    let charset = part.content_type()?.attribute("charset")?;
    let encoding = encoding_rs::Encoding::for_label(charset.trim().as_bytes())?;
    let body = raw_message.get(part.offset_body as usize..part.offset_end as usize)?;
    let decoded_transfer = match part.encoding {
        Encoding::None => Cow::Borrowed(body),
        Encoding::Base64 => {
            let compact: Vec<u8> = body
                .iter()
                .copied()
                .filter(|byte| !byte.is_ascii_whitespace())
                .collect();
            Cow::Owned(
                base64::engine::general_purpose::STANDARD
                    .decode(compact)
                    .ok()?,
            )
        }
        Encoding::QuotedPrintable => {
            Cow::Owned(quoted_printable::decode(body, quoted_printable::ParseMode::Robust).ok()?)
        }
    };
    let (decoded, _, _) = encoding.decode(&decoded_transfer);
    Some(decoded.into_owned())
}

/// Decode RFC 3676 `format=flowed` text. Soft line breaks are joined only when quote depth is
/// unchanged; signature separators and fixed lines remain intact.
pub fn decode_format_flowed(input: &str, delsp: bool) -> String {
    fn split_quote(line: &str) -> (usize, &str) {
        let depth = line.bytes().take_while(|byte| *byte == b'>').count();
        let mut content = &line[depth..];
        if let Some(unspaced) = content.strip_prefix(' ') {
            content = unspaced;
        }
        (depth, content)
    }

    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut output = String::new();
    let mut index = 0;
    while index < lines.len() {
        let (depth, mut content) = split_quote(lines[index]);
        if depth > 0 {
            output.push_str(&">".repeat(depth));
            output.push(' ');
        }

        loop {
            let signature = content == "-- ";
            let flowed = !signature && content.ends_with(' ');
            if flowed && delsp {
                content = content.strip_suffix(' ').unwrap_or(content);
            }
            output.push_str(content);
            if !flowed || index + 1 >= lines.len() {
                break;
            }
            let (next_depth, next_content) = split_quote(lines[index + 1]);
            if next_depth != depth {
                break;
            }
            index += 1;
            content = next_content;
        }

        if index + 1 < lines.len() {
            output.push('\n');
        }
        index += 1;
    }
    output
}

/// Split a plain-text reply at its first quoted block so the reading pane can fold it behind a
/// disclosure control (E6.9).
pub fn fold_plain_quotes(input: &str) -> FoldedPlain {
    let lines: Vec<&str> = input.lines().collect();
    let boundary = lines
        .iter()
        .position(|line| line.trim_start().starts_with('>'));
    let Some(boundary) = boundary else {
        return FoldedPlain {
            visible: input.to_string(),
            quoted: None,
        };
    };
    // Include a conventional "On … wrote:" lead-in in the folded section.
    let boundary = boundary
        .checked_sub(1)
        .filter(|previous| lines[*previous].trim_end().ends_with("wrote:"))
        .unwrap_or(boundary);
    FoldedPlain {
        visible: lines[..boundary].join("\n").trim_end().to_string(),
        quoted: Some(lines[boundary..].join("\n")),
    }
}

/// An inline image referenced by `cid:` (E6.8). The bytes come from the cached raw MIME.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineImage {
    pub content_id: String,
    pub content_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// Every part with a `Content-ID`, for resolving `cid:` references (E6.8). mail-parser classifies
/// non-text parts as attachments, which is where inline images live.
pub fn inline_images(raw: &[u8]) -> Vec<InlineImage> {
    let Some(message) = MessageParser::default().parse(raw) else {
        return Vec::new();
    };
    let mut images = Vec::new();
    for part in message.attachments() {
        let Some(content_id) = part.content_id() else {
            continue;
        };
        let content_id = content_id
            .trim()
            .trim_start_matches('<')
            .trim_end_matches('>')
            .trim_start_matches("cid:")
            .to_string();
        if content_id.is_empty() {
            continue;
        }
        let content_type = part
            .content_type()
            .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("")));
        images.push(InlineImage {
            content_id,
            content_type,
            bytes: part.contents().to_vec(),
        });
    }
    images
}

fn header_text(value: &mail_parser::HeaderValue<'_>) -> Option<String> {
    use mail_parser::HeaderValue;
    let text = match value {
        HeaderValue::Text(text) => text.to_string(),
        HeaderValue::TextList(list) => list.join(" "),
        HeaderValue::Address(address) => address
            .first()
            .and_then(|addr| addr.address())
            .unwrap_or("")
            .to_string(),
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
        assert!(
            parsed
                .references
                .as_deref()
                .unwrap()
                .contains("m0@example.com")
        );
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
        assert!(
            matches!(parsed.body, Body::Text(_)),
            "plain must not be converted to html"
        );
    }

    #[test]
    fn inline_images_are_extracted_by_content_id() {
        let raw = b"From: a@b.c\r\nSubject: pic\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=BB\r\n\r\n\
--BB\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:logo1\">\r\n\
--BB\r\nContent-Type: image/png; name=\"logo.png\"\r\nContent-ID: <logo1>\r\n\
Content-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--BB--\r\n";
        let images = inline_images(raw);
        assert_eq!(images.len(), 1, "one cid part");
        assert_eq!(images[0].content_id, "logo1");
        assert_eq!(images[0].content_type.as_deref(), Some("image/png"));
        assert!(!images[0].bytes.is_empty());
    }

    #[test]
    fn honors_plain_body_preference_without_using_synthetic_parts() {
        let raw = b"From: a@b.c\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=X\r\n\r\n--X\r\nContent-Type: text/plain\r\n\r\nplain\r\n--X\r\nContent-Type: text/html\r\n\r\n<b>html</b>\r\n--X--\r\n";
        assert!(matches!(
            parse_raw_with_preference(raw, BodyPreference::Plain)
                .unwrap()
                .body,
            Body::Text(ref text) if text.contains("plain")
        ));
    }

    #[test]
    fn decodes_format_flowed_and_preserves_quote_depth() {
        let input = "A flowed line \r\ncontinues here.\r\n> quoted line \r\n> continues.\r\n-- \r\nsignature";
        assert_eq!(
            decode_format_flowed(input, false),
            "A flowed line continues here.\n> quoted line continues.\n-- \nsignature"
        );
        assert_eq!(decode_format_flowed("joined \r\nword", true), "joinedword");
    }

    #[test]
    fn folds_plain_quoted_replies_at_the_first_boundary() {
        let folded = fold_plain_quotes("New answer\n\nOn Monday, Pat wrote:\n> Old answer\n> More");
        assert_eq!(folded.visible, "New answer");
        assert_eq!(
            folded.quoted.as_deref(),
            Some("On Monday, Pat wrote:\n> Old answer\n> More")
        );
    }

    #[test]
    fn mail_parser_decodes_common_legacy_charsets() {
        let cases: &[(&str, &[u8], &str)] = &[
            ("iso-8859-1", b"caf\xe9", "café"),
            ("shift_jis", b"\x93\xfa\x96\x7b", "日本"),
            ("gb2312", b"\xd6\xd0\xb9\xfa", "中国"),
        ];
        for (charset, bytes, expected) in cases {
            let mut raw = format!(
                "From: a@b.c\r\nContent-Type: text/plain; charset={charset}\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
            )
            .into_bytes();
            raw.extend_from_slice(bytes);
            let parsed = parse_raw(&raw).unwrap();
            assert!(
                matches!(parsed.body, Body::Text(ref text) if text.contains(expected)),
                "{charset}: {:?}",
                parsed.body
            );
        }
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
        assert_eq!(
            parsed.attachments[0].filename.as_deref(),
            Some("report.pdf")
        );
        assert!(parsed.attachments[0].size > 0);
    }
}
