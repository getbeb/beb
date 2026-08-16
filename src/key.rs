pub const ED25519: &str = "ssh-ed25519";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub kind: String,
    pub b64: String,
}

impl PublicKey {
    pub fn canonical(&self) -> String {
        format!("{} {}", self.kind, self.b64)
    }

    pub fn is_ed25519(&self) -> bool {
        self.kind == ED25519
    }
}

fn is_b64(s: &str) -> bool {
    !s.is_empty()
        && s.trim_end_matches('=')
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
}

/// The SSH wire blob of an ed25519 public key: string "ssh-ed25519",
/// string of exactly 32 key bytes, nothing after. 4+11+4+32 bytes.
fn valid_ed25519_blob(b64: &str) -> bool {
    match crate::util::b64_decode(b64) {
        Some(blob) => {
            blob.len() == 51
                && blob[0..4] == [0, 0, 0, 11]
                && &blob[4..15] == b"ssh-ed25519"
                && blob[15..19] == [0, 0, 0, 32]
        }
        None => false,
    }
}

/// The name of this key's mailbox: the 32 raw key bytes, in hex.
///
/// Not a hash of the key text. The two are the same length -- ed25519
/// keys and sha256 digests are both 32 bytes -- so nothing about the
/// spool's shape changes, but this one is derivable without hashing and
/// reversible back to the key it names. That matters because a
/// transport must never have to compute a spool path: the one that did
/// carried its own sha256 to do it, and drifted.
///
/// Only ed25519 reaches a mailbox. Every other key type is refused by
/// name at every use site -- `send`, `pack`, `receive` and the frame
/// reader all check `is_ed25519` first -- so the fallback below is
/// unreachable, and exists so this function is total rather than
/// fallible at eight call sites that have already ruled it out.
pub fn mailbox_name(canonical: &str) -> String {
    if let Some(b64) = canonical.split_whitespace().nth(1) {
        if let Some(blob) = crate::util::b64_decode(b64) {
            if blob.len() == 51 && &blob[4..15] == b"ssh-ed25519" {
                return blob[19..51].iter().map(|b| format!("{b:02x}")).collect();
            }
        }
    }
    crate::util::sha256_hex(canonical)
}

/// Tolerant parse of public key text: `<type> <base64> [comment]`,
/// surrounding whitespace and any trailing comment ignored. An ed25519
/// key must decode to an actual ed25519 public-key blob: "identity is a
/// public key" means the key, not text resembling one. Foreign types are
/// parsed only far enough to be refused by name at every use site.
pub fn parse(text: &str) -> Result<PublicKey, String> {
    let mut it = text.split_whitespace();
    let kind = it.next().ok_or("empty key text")?;
    let b64 = it
        .next()
        .ok_or_else(|| format!("\"{}\" has no base64 field; a key is \"type base64\"", kind))?;
    if !is_b64(b64) {
        return Err(format!("\"{}\" is not base64; a key is \"type base64\"", b64));
    }
    if kind == ED25519 && !valid_ed25519_blob(b64) {
        return Err("not a valid ssh-ed25519 public key (bad key blob)".into());
    }
    Ok(PublicKey {
        kind: kind.to_string(),
        b64: b64.to_string(),
    })
}

/// True for tokens that look like the type field of an SSH public key.
/// Used to catch an unquoted key splitting into several arguments.
pub fn looks_like_key_type(tok: &str) -> bool {
    tok.starts_with("ssh-") || tok.starts_with("ecdsa-") || tok.starts_with("sk-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFv7BidWkQPvjU9Qz+J3BWNuFmqssCIorRaHYge3gKOQ";

    #[test]
    fn parses_real_key() {
        let k = parse(K1).unwrap();
        assert_eq!(k.canonical(), K1);
        assert!(k.is_ed25519());
    }

    #[test]
    fn strips_comment_and_whitespace() {
        let k = parse(&format!("  {K1} user@host extra\n")).unwrap();
        assert_eq!(k.canonical(), K1);
    }

    #[test]
    fn foreign_type_parses_but_is_not_ed25519() {
        let k = parse("ssh-rsa AAAAB3NzaC1yc2EAAAADAQAB").unwrap();
        assert!(!k.is_ed25519());
        assert_eq!(k.kind, "ssh-rsa");
    }

    #[test]
    fn rejects_junk() {
        assert!(parse("").is_err());
        assert!(parse("ssh-ed25519").is_err());
        assert!(parse("ssh-ed25519 not!base64").is_err());
    }

    #[test]
    fn base64_shaped_text_is_not_a_key() {
        // Decodes fine, is not an ed25519 blob.
        assert!(parse("ssh-ed25519 QQ==").is_err());
        // Too short to be any blob.
        assert!(parse("ssh-ed25519 A").is_err());
        // A different key type's blob under the ed25519 label.
        assert!(parse("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQAB").is_err());
        // The real blob, truncated.
        assert!(parse("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA").is_err());
    }

    #[test]
    fn key_type_detection() {
        assert!(looks_like_key_type("ssh-ed25519"));
        assert!(looks_like_key_type("ecdsa-sha2-nistp256"));
        assert!(looks_like_key_type("sk-ssh-ed25519@openssh.com"));
        assert!(!looks_like_key_type("backend"));
    }
}
