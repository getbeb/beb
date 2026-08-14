//! The mbeb frame: `beb <envelope bytes> <signature bytes>\n`, then
//! exactly those bytes, then end of input. Lengths first, so nothing is
//! delimited and no body can collide with a delimiter. No version field:
//! a different frame is a different protocol.

use std::io::{self, Read, Write};

const HEADER_MAX: usize = 64;

/// The envelope length stays uncapped, because a body is uncapped and it
/// streams through disk either way. The signature length does not: an
/// armored ed25519 SSHSIG is under 300 bytes, so this is generous by more
/// than twenty times. It is not a policy knob but a fact about the format
/// — a claimed signature this large is not a signature — and the frame
/// says so before a byte of it is read, let alone written.
pub const SIGNATURE_MAX: u64 = 8 * 1024;

pub fn write_header(w: &mut impl Write, envelope: u64, signature: u64) -> io::Result<()> {
    writeln!(w, "beb {envelope} {signature}")
}

/// What `write_header` will occupy. The header is a line rather than a
/// fixed width, so a caller reporting the size of a whole delivery has
/// to measure it instead of assuming one.
pub fn header_len(envelope: u64, signature: u64) -> u64 {
    format!("beb {envelope} {signature}\n").len() as u64
}

pub fn read_header(r: &mut impl Read) -> Result<(u64, u64), String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = r
            .read(&mut byte)
            .map_err(|e| format!("cannot read frame: {e}"))?;
        if n == 0 {
            return Err("truncated frame: input ended inside the header".into());
        }
        if byte[0] == b'\n' {
            break;
        }
        if buf.len() >= HEADER_MAX {
            return Err("not an mbeb: header line too long".into());
        }
        buf.push(byte[0]);
    }
    let s = std::str::from_utf8(&buf).map_err(|_| "not an mbeb: header is not utf-8".to_string())?;
    let parts: Vec<&str> = s.split(' ').collect();
    if parts.len() != 3 || parts[0] != "beb" {
        return Err(
            "not an mbeb: expected \"beb <envelope bytes> <signature bytes>\"".into(),
        );
    }
    let (envelope, signature) = (parse_len(parts[1])?, parse_len(parts[2])?);
    if signature > SIGNATURE_MAX {
        return Err(format!(
            "not an mbeb: signature claims {signature} bytes; no ssh signature exceeds {SIGNATURE_MAX}"
        ));
    }
    Ok((envelope, signature))
}

fn parse_len(s: &str) -> Result<u64, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("not an mbeb: \"{s}\" is not a byte count"));
    }
    s.parse()
        .map_err(|_| format!("not an mbeb: \"{s}\" is out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(bytes: &[u8]) -> Result<(u64, u64), String> {
        read_header(&mut &bytes[..])
    }

    #[test]
    fn header_len_matches_what_is_written() {
        for (e, s) in [(0u64, 0u64), (1, 1), (215, 268), (1_000_000, 8_191)] {
            let mut buf = Vec::new();
            write_header(&mut buf, e, s).unwrap();
            assert_eq!(header_len(e, s), buf.len() as u64, "{e} {s}");
        }
    }

    #[test]
    fn roundtrip() {
        let mut out = Vec::new();
        write_header(&mut out, 42, 7).unwrap();
        assert_eq!(out, b"beb 42 7\n");
        assert_eq!(parse(&out).unwrap(), (42, 7));
    }

    #[test]
    fn strictness() {
        assert!(parse(b"beb 42 7").is_err()); // no newline
        assert!(parse(b"beb 42\n").is_err()); // one count
        assert!(parse(b"beb 42 7 9\n").is_err()); // three counts
        assert!(parse(b"bob 42 7\n").is_err()); // wrong magic
        assert!(parse(b"beb 4a 7\n").is_err()); // not decimal
        assert!(parse(b"beb -4 7\n").is_err()); // signed
        assert!(parse(b"beb  42 7\n").is_err()); // double space
        assert!(parse(b"").is_err()); // empty input
    }

    #[test]
    fn absurd_signature_length_refused_before_any_bytes() {
        assert!(parse(format!("beb 1 {}\n", SIGNATURE_MAX).as_bytes()).is_ok());
        let err = parse(format!("beb 1 {}\n", SIGNATURE_MAX + 1).as_bytes()).unwrap_err();
        assert!(err.contains("no ssh signature exceeds"));
        assert!(parse(b"beb 500000000000 500000000000\n").is_err());
    }

    #[test]
    fn envelope_length_stays_uncapped() {
        assert_eq!(parse(b"beb 500000000000 294\n").unwrap(), (500000000000, 294));
    }

    #[test]
    fn overlong_header_refused() {
        let mut long = b"beb ".to_vec();
        long.extend(std::iter::repeat(b'9').take(100));
        long.push(b'\n');
        assert!(parse(&long).is_err());
    }
}
