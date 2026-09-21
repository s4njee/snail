//! Compose, reply and forward (plan.md E7.4/E7.8). Pure: build a draft, derive a reply or a forward
//! from a parsed message, and serialize to RFC822 with `mail-builder`. No UI, no network.

use anyhow::Result;
use mail_builder::MessageBuilder;

use crate::mime::{ParsedMessage, Recipient};

/// A message being written. Plain text by default (E7.8).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub to: Vec<Recipient>,
    pub cc: Vec<Recipient>,
    pub bcc: Vec<Recipient>,
    pub subject: String,
    /// Plain text (always present, E7.8).
    pub body_text: String,
    /// Optional rich alternative, sent as `multipart/alternative` (E7.8).
    pub body_html: Option<String>,
    /// Attachments, sent as `multipart/mixed` (E7.7).
    pub attachments: Vec<Attachment>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl Attachment {
    /// The provider size limit is 20 MB for iCloud (E4.20); warn above ~18 MB to leave header room.
    pub const WARN_BYTES: usize = 18 * 1024 * 1024;
}

impl Default for Attachment {
    fn default() -> Self {
        Self {
            filename: String::new(),
            content_type: "application/octet-stream".into(),
            bytes: Vec::new(),
        }
    }
}

/// Serialize a draft to RFC822 bytes. Angular brackets are added around the threading ids.
pub fn build_raw(draft: &Draft) -> Result<Vec<u8>> {
    let addresses = |list: &[Recipient]| -> Vec<(String, String)> {
        list.iter()
            .map(|recipient| {
                (
                    recipient.name.clone().unwrap_or_default(),
                    recipient.address.clone(),
                )
            })
            .collect()
    };

    let mut builder = MessageBuilder::new()
        .from((
            draft.from_name.clone().unwrap_or_default(),
            draft.from_addr.clone().unwrap_or_default(),
        ))
        .to(addresses(&draft.to))
        .subject(draft.subject.clone())
        .text_body(draft.body_text.clone());

    if let Some(html) = draft
        .body_html
        .as_deref()
        .filter(|html| !html.trim().is_empty())
    {
        builder = builder.html_body(html.to_string());
    }
    if !draft.cc.is_empty() {
        builder = builder.cc(addresses(&draft.cc));
    }
    if !draft.bcc.is_empty() {
        builder = builder.bcc(addresses(&draft.bcc));
    }
    for attachment in &draft.attachments {
        builder = builder.attachment(
            attachment.content_type.clone(),
            attachment.filename.clone(),
            attachment.bytes.clone(),
        );
    }
    if let Some(in_reply_to) = draft.in_reply_to.as_deref().filter(|id| !id.is_empty()) {
        builder = builder.in_reply_to(format!("<{in_reply_to}>"));
    }
    if let Some(references) = draft.references.as_deref().filter(|ids| !ids.is_empty()) {
        builder = builder.references(format!("<{}>", references.replace(' ', "> <")));
    }

    let mut out = Vec::new();
    builder.write_to(&mut out)?;
    Ok(out)
}

/// Derive a reply. `self_addr` excludes the user's own address from a reply-all.
pub fn reply(original: &ParsedMessage, self_addr: Option<&str>, all: bool) -> Draft {
    let mut draft = Draft {
        subject: with_prefix(&original.subject.clone().unwrap_or_default(), "Re:"),
        in_reply_to: original.message_id.clone(),
        references: chain_references(original),
        ..Default::default()
    };

    if let Some(address) = &original.from_addr {
        draft.to.push(Recipient {
            name: original.from_name.clone(),
            address: address.clone(),
        });
    }

    if all {
        let reply_to = original.from_addr.as_deref();
        for recipient in &original.to {
            if Some(recipient.address.as_str()) == self_addr
                || Some(recipient.address.as_str()) == reply_to
            {
                continue;
            }
            draft.cc.push(recipient.clone());
        }
    }

    draft.body_text = quote(original);
    draft
}

/// Derive a forward: the original's headers and body, quoted, with no threading headers.
pub fn forward(original: &ParsedMessage) -> Draft {
    let mut body = String::from("\n\n---------- Forwarded message ----------\n");
    if let Some(from) = &original.from_addr {
        body.push_str(&format!(
            "From: {}{}\n",
            original.from_name.clone().unwrap_or_default(),
            if original.from_name.is_some() {
                format!(" <{from}>")
            } else {
                from.clone()
            }
        ));
    }
    body.push_str(&format!(
        "Subject: {}\n",
        original.subject.clone().unwrap_or_default()
    ));
    body.push_str(&format!(
        "Message-ID: {}\n\n",
        original.message_id.clone().unwrap_or_default()
    ));
    body.push_str(original.plain.as_deref().unwrap_or(""));
    Draft {
        subject: with_prefix(&original.subject.clone().unwrap_or_default(), "Fwd:"),
        body_text: body,
        ..Default::default()
    }
}

/// `On <date>, <from> wrote:` followed by the original, `> `-quoted.
pub fn quote(original: &ParsedMessage) -> String {
    let from = original
        .from_name
        .clone()
        .or_else(|| original.from_addr.clone())
        .unwrap_or_else(|| "someone".into());
    let when = original
        .date
        .map(|epoch| format!("{epoch}"))
        .unwrap_or_else(|| "an earlier date".into());
    let mut out = format!("\n\nOn {when}, {from} wrote:\n");
    for line in original.plain.as_deref().unwrap_or("").lines() {
        if line.is_empty() {
            out.push_str(">\n");
        } else {
            out.push_str("> ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Insert a signature above the quoted text (E7.9), with the standard `-- ` separator.
pub fn with_signature(body: &str, signature: &str) -> String {
    let signature = signature.trim_end();
    if signature.is_empty() {
        return body.to_string();
    }
    match body.find("\n\nOn ") {
        Some(index) => format!(
            "{}\n\n-- \n{signature}\n{}",
            body[..index].trim_end(),
            &body[index..]
        ),
        None => format!("{}\n\n-- \n{signature}\n", body.trim_end()),
    }
}

/// Ensure a subject carries `Re:`/`Fwd:` exactly once (case-insensitive).
pub fn with_prefix(subject: &str, prefix: &str) -> String {
    let trimmed = subject.trim();
    let lower = trimmed.to_ascii_lowercase();
    let needle = prefix.to_ascii_lowercase();
    if lower.starts_with(&needle) {
        trimmed.to_string()
    } else {
        format!("{prefix} {trimmed}")
    }
}

/// `References` for a reply: the original chain plus the original's own id, deduplicated.
pub fn chain_references(original: &ParsedMessage) -> Option<String> {
    let mut ids: Vec<String> = original
        .references
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect();
    if let Some(id) = &original.message_id {
        if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
            ids.push(id.clone());
        }
    }
    if ids.is_empty() {
        None
    } else {
        Some(ids.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mime::{Body, Recipient};

    fn original() -> ParsedMessage {
        ParsedMessage {
            message_id: Some("m2@example.com".into()),
            in_reply_to: Some("m1@example.com".into()),
            references: Some("m0@example.com m1@example.com".into()),
            subject: Some("Almanac geometry".into()),
            from_name: Some("Maya".into()),
            from_addr: Some("maya@example.com".into()),
            to: vec![
                Recipient {
                    name: Some("Me".into()),
                    address: "me@example.com".into(),
                },
                Recipient {
                    name: None,
                    address: "other@example.com".into(),
                },
            ],
            date: Some(1_700_000_000),
            preview: Some("The grid checks out".into()),
            plain: Some("The grid checks out.\nSecond line.".into()),
            body: Body::Text("The grid checks out.\nSecond line.".into()),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn reply_sets_threading_headers_and_quotes() {
        let draft = reply(&original(), Some("me@example.com"), false);
        assert_eq!(draft.subject, "Re: Almanac geometry");
        assert_eq!(draft.in_reply_to.as_deref(), Some("m2@example.com"));
        assert_eq!(
            draft.references.as_deref(),
            Some("m0@example.com m1@example.com m2@example.com")
        );
        assert_eq!(draft.to[0].address, "maya@example.com");
        assert!(draft.cc.is_empty(), "plain reply does not cc");
        assert!(draft.body_text.contains("> The grid checks out."));
        assert!(draft.body_text.contains("wrote:"));
    }

    #[test]
    fn reply_all_excludes_the_user_and_the_original_sender() {
        let draft = reply(&original(), Some("me@example.com"), true);
        let addresses: Vec<&str> = draft.cc.iter().map(|r| r.address.as_str()).collect();
        assert!(!addresses.contains(&"me@example.com"), "not to self");
        assert!(
            !addresses.contains(&"maya@example.com"),
            "not to the sender again"
        );
        assert!(addresses.contains(&"other@example.com"));
    }

    #[test]
    fn attachments_and_html_round_trip() {
        let mut draft = Draft {
            to: vec![Recipient {
                name: None,
                address: "a@b.c".into(),
            }],
            from_addr: Some("me@example.com".into()),
            subject: "files".into(),
            body_text: "see attached".into(),
            body_html: Some("<p>see attached</p>".into()),
            attachments: vec![Attachment {
                filename: "report.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: b"PDFBYTES".to_vec(),
            }],
            ..Default::default()
        };
        draft.from_name = Some("Me".into());
        let raw = build_raw(&draft).unwrap();
        let parsed = crate::mime::parse_raw(&raw).unwrap();
        assert!(
            matches!(parsed.body, Body::Html(_)),
            "html alternative wins for display"
        );
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(
            parsed.attachments[0].filename.as_deref(),
            Some("report.pdf")
        );
    }

    #[test]
    fn signature_is_inserted_above_the_quote() {
        let body = "thanks\n\nOn 1700000000, Maya wrote:\n> hi";
        let signed = with_signature(body, "— Sanjee");
        assert!(signed.contains("-- \n— Sanjee"));
        assert!(
            signed.find("— Sanjee").unwrap() < signed.find("> hi").unwrap(),
            "signature must precede the quote"
        );
        // An empty signature is a no-op.
        assert_eq!(with_signature(body, "  "), body);
    }

    #[test]
    fn subject_prefix_is_not_doubled() {
        assert_eq!(with_prefix("Almanac", "Re:"), "Re: Almanac");
        assert_eq!(with_prefix("Re: Almanac", "Re:"), "Re: Almanac");
        assert_eq!(with_prefix("RE: Almanac", "Re:"), "RE: Almanac");
    }

    #[test]
    fn forward_has_no_threading_headers() {
        let draft = forward(&original());
        assert_eq!(draft.subject, "Fwd: Almanac geometry");
        assert!(draft.in_reply_to.is_none());
        assert!(draft.references.is_none());
        assert!(draft.body_text.contains("Forwarded message"));
    }

    #[test]
    fn build_raw_round_trips_through_the_parser() {
        let mut draft = reply(&original(), Some("me@example.com"), false);
        draft.from_name = Some("Me".into());
        draft.from_addr = Some("me@example.com".into());
        let raw = build_raw(&draft).unwrap();
        let parsed = crate::mime::parse_raw(&raw).unwrap();
        assert_eq!(parsed.subject.as_deref(), Some("Re: Almanac geometry"));
        assert_eq!(parsed.in_reply_to.as_deref(), Some("m2@example.com"));
        assert!(
            parsed
                .references
                .as_deref()
                .unwrap()
                .contains("m2@example.com")
        );
        assert_eq!(parsed.to[0].address, "maya@example.com");
        assert!(matches!(parsed.body, Body::Text(_)));
    }
}
