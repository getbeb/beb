use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_string())
}

pub fn spool_root() -> Result<PathBuf, String> {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("beb"));
        }
    }
    Ok(home()?.join(".local/share/beb"))
}

pub fn known_signers_path() -> Result<PathBuf, String> {
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        if !x.is_empty() {
            return Ok(PathBuf::from(x).join("beb/known_signers"));
        }
    }
    Ok(home()?.join(".config/beb/known_signers"))
}

pub fn pretty_path(p: &Path) -> String {
    if let Ok(h) = home() {
        if let Ok(rest) = p.strip_prefix(&h) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Streaming file hash; the file never sits in memory whole.
pub fn sha256_file(path: &std::path::Path) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{:02x}", b)).collect())
}

pub fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64(data: &[u8]) -> String {
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Canonical base64 decode (RFC 4648): length a multiple of four, padding
/// only at the end, nothing outside the alphabet, and re-encoding must
/// reproduce the input exactly, so non-canonical encodings (nonzero
/// discarded pad bits) are refused too. None for the empty string, which
/// is never a valid value in an envelope.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let b = s.as_bytes();
    if b.is_empty() || b.len() % 4 != 0 {
        return None;
    }
    let pad = b.iter().rev().take_while(|&&c| c == b'=').count();
    if pad > 2 || b[..b.len() - pad].contains(&b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for chunk in b.chunks(4) {
        let mut n: u32 = 0;
        for &c in chunk {
            n = (n << 6) | if c == b'=' { 0 } else { val(c)? };
        }
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    out.truncate(out.len() - pad);
    if b64(&out) != s {
        return None;
    }
    Some(out)
}

pub fn random_nonce() -> Result<String, String> {
    let mut f = File::open("/dev/urandom").map_err(|e| format!("cannot open /dev/urandom: {e}"))?;
    let mut buf = [0u8; 16];
    f.read_exact(&mut buf)
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))?;
    Ok(b64(&buf))
}

pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().expect("write_atomic path has a parent");
    let tmp = dir.join(format!(".tmp-{}", std::process::id()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    fsync_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_vectors() {
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"abc"), "YWJj");
        assert_eq!(b64(b"ab"), "YWI=");
        assert_eq!(b64(b"a"), "YQ==");
        assert_eq!(b64(&[0u8; 16]), "AAAAAAAAAAAAAAAAAAAAAA==");
    }

    #[test]
    fn b64_decode_strict() {
        assert_eq!(b64_decode("YWJj"), Some(b"abc".to_vec()));
        assert_eq!(b64_decode("YWI="), Some(b"ab".to_vec()));
        assert_eq!(b64_decode("YQ=="), Some(b"a".to_vec()));
        assert_eq!(b64_decode(""), None);
        assert_eq!(b64_decode("YWJ"), None); // not a multiple of four
        assert_eq!(b64_decode("Y==="), None); // over-padded
        assert_eq!(b64_decode("Y=Y="), None); // padding inside
        assert_eq!(b64_decode("Y!Jj"), None); // outside the alphabet
        assert_eq!(b64_decode("YR=="), None); // non-canonical: pad bits nonzero
        assert_eq!(b64_decode("YWK="), None); // non-canonical: pad bits nonzero
    }

    #[test]
    fn b64_roundtrip() {
        for data in [&b""[..], b"a", b"ab", b"abc", &[0u8; 32]] {
            if data.is_empty() {
                continue;
            }
            assert_eq!(b64_decode(&b64(data)).as_deref(), Some(data));
        }
    }

    #[test]
    fn sha256_known() {
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn one_line_collapses() {
        assert_eq!(one_line("a\nb\r\n  c"), "a b c");
    }
}
