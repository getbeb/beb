use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::key::{self, PublicKey};

/// Headers must fit in this prefix; ed25519 keys are ~80 bytes, so this is
/// generous. Nothing past the blank line is ever pulled into memory here.
const HEADER_MAX: usize = 16 * 1024;

#[derive(Debug)]
pub struct Headers {
    pub from: PublicKey,
    pub to: PublicKey,
    /// Validated but unused beyond the grammar; kept because the envelope
    /// has it and Headers describes the envelope.
    #[allow(dead_code)]
    pub nonce: String,
    pub body_offset: u64,
}

pub fn compose(from: &str, to: &str, nonce: &str) -> String {
    format!("from: {from}\nto: {to}\nnonce: {nonce}\n\n")
}

pub fn read_headers(path: &Path) -> Result<Headers, String> {
    let mut f = File::open(path).map_err(|e| format!("cannot open: {e}"))?;
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

/// Strict grammar: `from: `, `to: `, `nonce: ` in this order, LF endings,
/// one blank line. Key values are exactly `type base64`.
pub fn parse_headers(buf: &[u8]) -> Result<Headers, String> {
    let mut off = 0usize;
    let from = field(buf, &mut off, "from: ")?;
    let to = field(buf, &mut off, "to: ")?;
    let nonce = field(buf, &mut off, "nonce: ")?;
    if buf.get(off) != Some(&b'\n') {
        return Err("missing blank line after headers".into());
    }
    off += 1;
    let from = strict_key(&from)?;
    let to = strict_key(&to)?;
    if crate::util::b64_decode(&nonce).is_none() {
        return Err("nonce is not base64".into());
    }
    Ok(Headers {
        from,
        to,
        nonce,
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
        let mut v = compose(from, to, nonce).into_bytes();
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
        let buf = b"to: ssh-ed25519 A\nfrom: ssh-ed25519 B\nnonce: QQ==\n\n";
        assert!(parse_headers(buf).unwrap_err().contains("from:"));
    }

    #[test]
    fn missing_blank_line_refused() {
        let buf = b"from: ssh-ed25519 A\nto: ssh-ed25519 B\nnonce: QQ==\nx";
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
    fn foreign_type_parses_for_caller_to_judge() {
        let buf = envelope("ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB", K2, "QQ==", b"");
        let h = parse_headers(&buf).unwrap();
        assert!(!h.from.is_ed25519());
    }
}
