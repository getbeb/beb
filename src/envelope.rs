use std::fs::File;
use std::io::Read;

use crate::key::{self, PublicKey};

/// Headers must fit in this prefix; ed25519 keys are ~80 bytes, so this is
/// generous. Nothing past the blank line is ever pulled into memory here.
pub const HEADER_MAX: usize = 16 * 1024;

/// A subject is short on purpose. It exists so a reader can decide whether
/// a message is worth opening, and a line `list` prints in a column
/// stops being that the moment it needs wrapping. 120 bytes is a
/// sentence, not a paragraph, and the limit is a refusal rather than a
/// truncation: a sender who is cut off does not know it, and a reader
/// shown half a claim cannot tell.
pub const SUBJECT_MAX: usize = 120;

#[derive(Debug)]
pub struct Headers {
    pub from: PublicKey,
    pub to: PublicKey,
    /// Validated but unused beyond the grammar; kept because the envelope
    /// has it and Headers describes the envelope.
    #[allow(dead_code)]
    pub nonce: String,
    /// When the sender says it sent. A claim, signed like every other
    /// claim, and never an ordering: delivery ids are the only ordering
    /// beb has, and a clock that is wrong or set wrong would otherwise
    /// silently reorder a queue beb guarantees is ordered.
    pub date: String,
    /// What the message is about, by the sender's account. It is a claim
    /// like any other, and it is signed like any other, which is all beb
    /// promises about it.
    pub subject: String,
    pub body_offset: u64,
}

/// The one header a sender writes freely, so the one that needs a
/// grammar. It reaches a reader's terminal through `list` without ever
/// passing through a body, and a control character there is not a
/// display glitch: it is a sender moving somebody else's cursor,
/// repainting a line, or hiding what it just claimed. Refused on the way
/// in and on the way out, so neither a local send nor an arriving
/// delivery can carry one.
pub fn validate_subject(t: &str) -> Result<(), String> {
    if t.is_empty() {
        return Err("subject is empty; a message says what it is about".into());
    }
    if t.len() > SUBJECT_MAX {
        return Err(format!(
            "subject is {} bytes; the limit is {SUBJECT_MAX}",
            t.len()
        ));
    }
    if let Some(c) = t.chars().find(|c| c.is_control()) {
        return Err(format!(
            "subject holds a control character ({:?}); titles are one plain line",
            c
        ));
    }
    Ok(())
}

/// The sender's clock, and nothing pretends otherwise. beb has no
/// trustworthy time of its own to offer: a file's mtime is rewritten by
/// any careless `cp` or `rsync`, so it decays into a wrong answer that
/// still looks like an answer, while a claim labelled as a claim stays
/// honest even when the clock behind it is not.
pub fn validate_date(d: &str) -> Result<(), String> {
    if crate::util::parse_rfc3339(d).is_none() {
        return Err(format!(
            "date is not YYYY-MM-DDTHH:MM:SSZ: \"{d}\""
        ));
    }
    Ok(())
}

pub fn compose(from: &str, to: &str, nonce: &str, date: &str, subject: &str) -> String {
    format!("from: {from}\nto: {to}\nnonce: {nonce}\ndate: {date}\nsubject: {subject}\n\n")
}


/// Headers from an open file, reading forward from wherever it stands.
/// The descriptor stays the caller's, so a caller that verifies and then
/// prints can do both against the one inode it opened, never a pathname
/// looked up twice.
pub fn read_headers_from(f: &mut File) -> Result<Headers, String> {
    let mut buf = vec![0u8; HEADER_MAX];
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..]).map_err(|e| format!("cannot read: {e}"))?;
        if k == 0 {
            break;
        }
        n += k;
    }
    parse_headers(&buf[..n])
}

/// Strict grammar: `from: `, `to: `, `nonce: `, `date: `, `subject: ` in
/// this order,
/// LF endings, one blank line. Key values are exactly `type base64`.
///
/// `subject:` came fourth rather than anywhere earlier so the routing
/// prefix stays exactly what it was: beb-ssh reads the first two lines
/// to find a destination and treats the rest as opaque, so a transport
/// carries titled mail without knowing the word.
pub fn parse_headers(buf: &[u8]) -> Result<Headers, String> {
    let mut off = 0usize;
    let from = field(buf, &mut off, "from: ")?;
    let to = field(buf, &mut off, "to: ")?;
    let nonce = field(buf, &mut off, "nonce: ")?;
    let date = field(buf, &mut off, "date: ")?;
    let subject = field(buf, &mut off, "subject: ")?;
    if buf.get(off) != Some(&b'\n') {
        return Err("missing blank line after headers".into());
    }
    off += 1;
    let from = strict_key(&from)?;
    let to = strict_key(&to)?;
    if crate::util::b64_decode(&nonce).is_none() {
        return Err("nonce is not base64".into());
    }
    validate_date(&date)?;
    validate_subject(&subject)?;
    Ok(Headers {
        from,
        to,
        nonce,
        date,
        subject,
        body_offset: off as u64,
    })
}

fn field(buf: &[u8], off: &mut usize, prefix: &str) -> Result<String, String> {
    let rest = &buf[*off..];
    if !rest.starts_with(prefix.as_bytes()) {
        return Err(format!("expected \"{}\" header", prefix.trim_end()));
    }
    let start = prefix.len();
    let nl = rest[start..]
        .iter()
        .position(|&b| b == b'\n')
        .ok_or_else(|| format!("unterminated \"{}\" header", prefix.trim_end()))?;
    let value = std::str::from_utf8(&rest[start..start + nl])
        .map_err(|_| format!("\"{}\" header is not utf-8", prefix.trim_end()))?
        .to_string();
    *off += start + nl + 1;
    Ok(value)
}

fn strict_key(v: &str) -> Result<PublicKey, String> {
    let parts: Vec<&str> = v.split(' ').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err("envelope key is not \"type base64\"".into());
    }
    key::parse(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFv7BidWkQPvjU9Qz+J3BWNuFmqssCIorRaHYge3gKOQ";
    const K2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKOBoNSMpcu5CaPKvBT4dO4cH+sHV1Pw0LfkEY1yHOHi";

    fn envelope(from: &str, to: &str, nonce: &str, body: &[u8]) -> Vec<u8> {
        let mut v = compose(from, to, nonce, "2026-08-15T02:26:34Z", "a subject").into_bytes();
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn roundtrip() {
        let buf = envelope(K1, K2, "QUJD", b"hello");
        let h = parse_headers(&buf).unwrap();
        assert_eq!(h.from.canonical(), K1);
        assert_eq!(h.to.canonical(), K2);
        assert_eq!(h.nonce, "QUJD");
        assert_eq!(h.subject, "a subject");
        assert_eq!(h.date, "2026-08-15T02:26:34Z");
        assert_eq!(&buf[h.body_offset as usize..], b"hello");
    }

    #[test]
    fn empty_body_is_legal() {
        let buf = envelope(K1, K2, "QQ==", b"");
        let h = parse_headers(&buf).unwrap();
        assert_eq!(h.body_offset as usize, buf.len());
    }

    #[test]
    fn wrong_order_refused() {
        let buf = b"to: ssh-ed25519 A\nfrom: ssh-ed25519 B\nnonce: QQ==\ndate: 2026-08-15T02:26:34Z\nsubject: t\n\n";
        assert!(parse_headers(buf).unwrap_err().contains("from:"));
    }

    #[test]
    fn missing_blank_line_refused() {
        let buf = b"from: ssh-ed25519 A\nto: ssh-ed25519 B\nnonce: QQ==\ndate: 2026-08-15T02:26:34Z\nsubject: t\nx";
        assert!(parse_headers(buf).unwrap_err().contains("blank line"));
    }

    #[test]
    fn key_with_comment_refused_in_envelope() {
        let buf = envelope(&format!("{K1} comment"), K2, "QQ==", b"");
        assert!(parse_headers(&buf).is_err());
    }

    #[test]
    fn undecodable_key_text_refused() {
        let buf = envelope("ssh-ed25519 A", K2, "QQ==", b"");
        assert!(parse_headers(&buf).is_err());
    }

    #[test]
    fn bad_nonce_refused() {
        let buf = envelope(K1, K2, "not base64!", b"");
        assert!(parse_headers(&buf).is_err());
        let buf = envelope(K1, K2, "YWJ", b""); // not a multiple of four
        assert!(parse_headers(&buf).is_err());
    }

    #[test]
    fn title_is_required() {
        let buf = b"from: ssh-ed25519 A\nto: ssh-ed25519 B\nnonce: QQ==\ndate: 2026-08-15T02:26:34Z\n\nbody";
        assert!(parse_headers(buf).unwrap_err().contains("subject"));
    }

    #[test]
    fn title_grammar() {
        assert!(validate_subject("").is_err());
        assert!(validate_subject(&"x".repeat(SUBJECT_MAX + 1)).is_err());
        assert!(validate_subject(&"x".repeat(SUBJECT_MAX)).is_ok());
        // A sender writes this and `list` prints it to somebody else's
        // terminal, so an escape sequence here moves their cursor.
        assert!(validate_subject("deploy\u{1b}[2Kblocked").is_err());
        assert!(validate_subject("deploy\u{7}blocked").is_err());
        assert!(validate_subject("deploy blocked: needs review").is_ok());
    }

    #[test]
    fn a_subject_that_arrives_bad_is_refused_too() {
        let mut v = format!("from: {K1}\nto: {K2}\nnonce: QUJD\ndate: 2026-08-15T02:26:34Z\nsubject: bad\u{1b}[2K\n\n").into_bytes();
        v.extend_from_slice(b"body");
        assert!(parse_headers(&v).unwrap_err().contains("control character"));
    }

    #[test]
    fn date_is_required_and_strict() {
        let buf = b"from: ssh-ed25519 A\nto: ssh-ed25519 B\nnonce: QQ==\nsubject: t\n\nbody";
        assert!(parse_headers(buf).unwrap_err().contains("date"));
        assert!(validate_date("2026-08-15T02:26:34Z").is_ok());
        assert!(validate_date("2026-08-15 02:26:34Z").is_err());
        assert!(validate_date("2026-02-30T00:00:00Z").is_err());
        assert!(validate_date("").is_err());
    }

    #[test]
    fn foreign_type_parses_for_caller_to_judge() {
        let buf = envelope("ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB", K2, "QQ==", b"");
        let h = parse_headers(&buf).unwrap();
        assert!(!h.from.is_ed25519());
    }
}
