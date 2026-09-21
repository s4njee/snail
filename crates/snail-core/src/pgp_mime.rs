//! RFC 3156 MIME framing and canonicalization. Cryptographic operations are in [`crate::pgp`].

use anyhow::{Result, bail};
use mail_parser::{MessageParser, MimeHeaders, PartType};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CryptoEnvelope {
    None,
    DetachedSigned { entity: Vec<u8>, signature: Vec<u8> },
    Encrypted(Vec<u8>),
    InlineSigned(String),
    InlineEncrypted(Vec<u8>),
}

/// Locate the OpenPGP payload without reserializing MIME. For multipart/signed the first child is
/// sliced from the original raw message so its headers and transfer encoding remain exact.
pub fn envelope(raw: &[u8]) -> CryptoEnvelope {
    let Some(message) = MessageParser::default().parse(raw) else {
        return CryptoEnvelope::None;
    };
    let Some(root) = message.parts.first() else {
        return CryptoEnvelope::None;
    };
    let content_type = root.content_type();
    let multipart = content_type
        .and_then(|ct| ct.subtype())
        .map(str::to_ascii_lowercase);
    let protocol = content_type
        .and_then(|ct| ct.attribute("protocol"))
        .map(|value| value.trim_matches('"').to_ascii_lowercase());

    if multipart.as_deref() == Some("signed")
        && protocol.as_deref() == Some("application/pgp-signature")
    {
        if let PartType::Multipart(children) = &root.body {
            if let (Some(entity), Some(signature)) = (
                children
                    .first()
                    .and_then(|id| message.parts.get(*id as usize)),
                children
                    .get(1)
                    .and_then(|id| message.parts.get(*id as usize)),
            ) {
                let entity = raw
                    .get(entity.offset_header as usize..entity.offset_end as usize)
                    .unwrap_or_default()
                    .to_vec();
                if let Some(signature) = decoded_body(signature) {
                    return CryptoEnvelope::DetachedSigned { entity, signature };
                }
            }
        }
    }

    if multipart.as_deref() == Some("encrypted")
        && protocol.as_deref() == Some("application/pgp-encrypted")
    {
        if let PartType::Multipart(children) = &root.body {
            if let Some(payload) = children
                .get(1)
                .and_then(|id| message.parts.get(*id as usize))
                .and_then(decoded_body)
            {
                return CryptoEnvelope::Encrypted(payload);
            }
        }
    }

    for part in &message.parts {
        let Some(text) = part.text_contents() else {
            continue;
        };
        if let Some(block) = armored_block(
            text,
            "-----BEGIN PGP SIGNED MESSAGE-----",
            "-----END PGP SIGNATURE-----",
        ) {
            return CryptoEnvelope::InlineSigned(block.to_string());
        }
        if let Some(block) = armored_block(
            text,
            "-----BEGIN PGP MESSAGE-----",
            "-----END PGP MESSAGE-----",
        ) {
            return CryptoEnvelope::InlineEncrypted(block.as_bytes().to_vec());
        }
    }
    CryptoEnvelope::None
}

/// Public keys attached to mail are discovery candidates only. Import still verifies their binding
/// chain and does not grant trust merely because a message carried them.
pub fn attached_public_keys(raw: &[u8]) -> Vec<Vec<u8>> {
    let Some(message) = MessageParser::default().parse(raw) else {
        return Vec::new();
    };
    message
        .parts
        .iter()
        .filter(|part| {
            part.content_type().is_some_and(|ct| {
                ct.ctype().eq_ignore_ascii_case("application")
                    && ct
                        .subtype()
                        .is_some_and(|subtype| subtype.eq_ignore_ascii_case("pgp-keys"))
            })
        })
        .filter_map(decoded_body)
        .collect()
}

fn decoded_body(part: &mail_parser::MessagePart<'_>) -> Option<Vec<u8>> {
    match &part.body {
        PartType::Binary(bytes) | PartType::InlineBinary(bytes) => Some(bytes.to_vec()),
        PartType::Text(text) | PartType::Html(text) => Some(text.as_bytes().to_vec()),
        _ => None,
    }
}

fn armored_block<'a>(text: &'a str, begin: &str, end: &str) -> Option<&'a str> {
    let start = text.find(begin)?;
    let finish = text[start..].find(end)? + start + end.len();
    Some(&text[start..finish])
}

/// Canonicalize a MIME entity before detached signing: normalize every line ending to CRLF,
/// remove transport-fragile trailing SP/TAB and end with exactly one CRLF.
pub fn canonicalize_signed_entity(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 16);
    let mut line = Vec::new();
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'\r' if input.get(i + 1) == Some(&b'\n') => {
                append_line(&mut out, &mut line);
                i += 2;
            }
            b'\r' | b'\n' => {
                append_line(&mut out, &mut line);
                i += 1;
            }
            byte => {
                line.push(byte);
                i += 1;
            }
        }
    }
    if !line.is_empty() {
        append_line(&mut out, &mut line);
    } else if out.is_empty() {
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn append_line(out: &mut Vec<u8>, line: &mut Vec<u8>) {
    while matches!(line.last(), Some(b' ' | b'\t')) {
        line.pop();
    }
    out.append(line);
    out.extend_from_slice(b"\r\n");
}

/// Build a complete multipart/signed body. The first part is precisely the bytes that were signed.
pub fn multipart_signed(
    entity: &[u8],
    armored_signature: &[u8],
    boundary: &str,
) -> Result<Vec<u8>> {
    validate_boundary(boundary)?;
    let entity = canonicalize_signed_entity(entity);
    let signature = canonicalize_crlf(armored_signature);
    let mut out = Vec::new();
    out.extend_from_slice(
        format!(
            "Content-Type: multipart/signed; protocol=\"application/pgp-signature\"; micalg=pgp-sha256; boundary=\"{boundary}\"\r\n\r\n"
        )
        .as_bytes(),
    );
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(&entity);
    out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    out.extend_from_slice(b"Content-Type: application/pgp-signature; name=\"signature.asc\"\r\nContent-Disposition: attachment; filename=\"signature.asc\"\r\n\r\n");
    out.extend_from_slice(&signature);
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(out)
}

pub fn multipart_encrypted(armored_message: &[u8], boundary: &str) -> Result<Vec<u8>> {
    validate_boundary(boundary)?;
    let message = canonicalize_crlf(armored_message);
    let mut out = format!(
        "Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"{boundary}\"\r\n\r\n--{boundary}\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--{boundary}\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\nContent-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(&message);
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok(out)
}

/// Separate RFC5322 envelope headers from the MIME entity that is protected. Content headers move
/// with the entity; routing/display headers stay visible outside RFC 3156 protection.
pub fn split_rfc822(raw: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let normalized = canonicalize_crlf(raw);
    let separator = normalized
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("message has no header/body separator"))?;
    let headers = &normalized[..separator + 2];
    let body = &normalized[separator + 4..];
    let mut outer = Vec::new();
    let mut content = Vec::new();
    let mut current_is_content = false;
    for line in headers.split_inclusive(|byte| *byte == b'\n') {
        let continuation = matches!(line.first(), Some(b' ' | b'\t'));
        if !continuation {
            let name = line.split(|byte| *byte == b':').next().unwrap_or_default();
            current_is_content = name.eq_ignore_ascii_case(b"content-type")
                || name.eq_ignore_ascii_case(b"content-transfer-encoding")
                || name.eq_ignore_ascii_case(b"content-disposition");
        }
        if current_is_content {
            content.extend_from_slice(line)
        } else {
            outer.extend_from_slice(line)
        }
    }
    if content.is_empty() {
        content.extend_from_slice(b"Content-Type: text/plain; charset=utf-8\r\n");
    }
    content.extend_from_slice(b"\r\n");
    content.extend_from_slice(body);
    Ok((outer, content))
}

pub fn join_rfc822(mut outer_headers: Vec<u8>, mime_entity: &[u8]) -> Vec<u8> {
    if !outer_headers.ends_with(b"\r\n") {
        outer_headers.extend_from_slice(b"\r\n");
    }
    if !header_present(&outer_headers, b"mime-version") {
        outer_headers.extend_from_slice(b"MIME-Version: 1.0\r\n");
    }
    outer_headers.extend_from_slice(mime_entity);
    outer_headers
}

fn header_present(headers: &[u8], name: &[u8]) -> bool {
    headers.split(|byte| *byte == b'\n').any(|line| {
        line.split(|byte| *byte == b':')
            .next()
            .is_some_and(|candidate| candidate.trim_ascii().eq_ignore_ascii_case(name))
    })
}

fn canonicalize_crlf(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 8);
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\r' && input.get(i + 1) == Some(&b'\n') {
            out.extend_from_slice(b"\r\n");
            i += 2;
        } else if matches!(input[i], b'\r' | b'\n') {
            out.extend_from_slice(b"\r\n");
            i += 1;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

fn validate_boundary(boundary: &str) -> Result<()> {
    if boundary.is_empty()
        || boundary.len() > 70
        || !boundary.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'\''
                        | b'('
                        | b')'
                        | b'+'
                        | b'_'
                        | b','
                        | b'-'
                        | b'.'
                        | b'/'
                        | b':'
                        | b'='
                        | b'?'
                )
        })
    {
        bail!("invalid MIME boundary");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_canonicalization_is_crlf_and_strips_trailing_whitespace() {
        assert_eq!(
            canonicalize_signed_entity(b"Header: value \t\n\nbody  \rnext"),
            b"Header: value\r\n\r\nbody\r\nnext\r\n"
        );
    }

    #[test]
    fn signed_wrapper_has_rfc3156_protocol_and_exact_signed_first_part() {
        let entity = b"Content-Type: text/plain\n\nhello\n";
        let out = multipart_signed(
            entity,
            b"-----BEGIN PGP SIGNATURE-----\nX\n-----END PGP SIGNATURE-----",
            "snail-boundary",
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("multipart/signed; protocol=\"application/pgp-signature\""));
        assert!(text.contains("--snail-boundary\r\nContent-Type: text/plain\r\n\r\nhello\r\n"));
        assert!(text.ends_with("--snail-boundary--\r\n"));
    }

    #[test]
    fn rejects_boundary_injection() {
        assert!(multipart_signed(b"x", b"sig", "bad\r\nheader").is_err());
    }

    #[test]
    fn recognizes_rfc3156_signed_and_encrypted_envelopes() {
        let signed = multipart_signed(
            b"Content-Type: text/plain\r\n\r\nhello\r\n",
            b"-----BEGIN PGP SIGNATURE-----\r\nX\r\n-----END PGP SIGNATURE-----\r\n",
            "snail-sign",
        )
        .unwrap();
        assert!(matches!(
            envelope(&signed),
            CryptoEnvelope::DetachedSigned { .. }
        ));

        let encrypted = multipart_encrypted(
            b"-----BEGIN PGP MESSAGE-----\r\nX\r\n-----END PGP MESSAGE-----\r\n",
            "snail-encrypt",
        )
        .unwrap();
        assert!(matches!(envelope(&encrypted), CryptoEnvelope::Encrypted(_)));
        let parsed = crate::mime::parse_raw(&encrypted).unwrap();
        assert!(
            parsed.plain.is_none(),
            "ciphertext/control parts must never become searchable body text"
        );
    }

    #[test]
    fn recognizes_inline_blocks_inside_regular_mail() {
        let raw = b"Content-Type: text/plain\r\n\r\n-----BEGIN PGP MESSAGE-----\r\nX\r\n-----END PGP MESSAGE-----\r\n";
        assert!(matches!(envelope(raw), CryptoEnvelope::InlineEncrypted(_)));
    }

    #[test]
    fn rfc822_split_keeps_envelope_visible_and_content_protected() {
        let raw = b"From: a@example.org\nSubject: hello\nMIME-Version: 1.0\nContent-Type: text/plain;\n charset=utf-8\n\nbody\n";
        let (outer, entity) = split_rfc822(raw).unwrap();
        assert!(String::from_utf8_lossy(&outer).contains("Subject: hello"));
        assert!(!String::from_utf8_lossy(&outer).contains("Content-Type"));
        assert!(
            String::from_utf8_lossy(&entity)
                .contains("Content-Type: text/plain;\r\n charset=utf-8")
        );
        assert!(
            join_rfc822(outer, &entity)
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
        );
    }
}
