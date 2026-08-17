use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
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

/// Days between the civil date and 1970-01-01, and back. Howard
/// Hinnant's algorithms, proleptic Gregorian, valid far past anything
/// beb will see.
///
/// Hand-written for the same reason base64 above is: beb takes no
/// dependency it can spell out in thirty lines, and a date library is a
/// large surface for two conversions with exact, testable answers.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`. UTC only: a local
/// offset is a fact about the sender's machine, and the one thing a
/// timestamp must not do is need context to read.
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// The exact shape `rfc3339` writes, and nothing else. A tolerant parser
/// here would accept a timestamp beb never produces and read it as a
/// fact, which is the whole thing a signed claim must not become.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':'
        || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let num = |a: usize, z: usize| -> Option<i64> {
        let t = &s[a..z];
        if !t.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    };
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    // Round-trip: rejects 31 April and 30 February, which the ranges
    // above let through.
    let (ry, rm, rd) = civil_from_days(days);
    if (ry, rm, rd) != (y, mo, d) {
        return None;
    }
    Some(days * 86_400 + h * 3600 + mi * 60 + sec)
}

/// The same instant, in this machine's zone, written the way somebody
/// reads a clock. The envelope carries UTC and only UTC, so nothing
/// about the message changes here -- this is display, and display has
/// one job: `2026-08-15T02:26:34Z` makes a reader do arithmetic to
/// answer "was that this morning", and `2026-08-15 09:26` does not.
///
/// No offset and no seconds. Both are precision for comparing instants
/// between machines, which is the wire format's job and already done in
/// the envelope; on a receipt they are characters that carry nothing a
/// reader in front of their own clock did not already know. The shape
/// still sorts and still parses, which is the whole of what an agent
/// needs from it.
///
/// `localtime_r` because zone rules are the system's: DST, historical
/// offsets and the TZ variable are a database, not arithmetic, and beb
/// already links libc. A failure falls back to UTC rather than inventing
/// an offset.
pub fn local_stamp(secs: i64) -> String {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = secs as libc::time_t;
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return rfc3339(secs);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year as i64 + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn now_secs() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .map_err(|_| "the system clock is before 1970".to_string())
}

fn urandom(buf: &mut [u8]) -> Result<(), String> {
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(buf))
        .map_err(|e| format!("cannot read /dev/urandom: {e}"))
}

pub fn random_nonce() -> Result<String, String> {
    let mut buf = [0u8; 16];
    urandom(&mut buf)?;
    Ok(b64(&buf))
}

/// A name no one else can predict. Scratch paths are built from this
/// rather than the pid: pids repeat, so a predictable name is one an
/// attacker who can write the directory could occupy in advance.
pub fn random_hex() -> Result<String, String> {
    let mut buf = [0u8; 12];
    urandom(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{:02x}", b)).collect())
}

/// The spool holds plaintext bodies: beb authenticates, it does not
/// encrypt. So every directory beb makes is 0700 and every file it makes
/// is 0600, set at creation rather than inherited from whatever umask the
/// process happened to start with. Confidentiality that depends on the
/// environment is confidentiality you cannot state.
pub const DIR_MODE: u32 = 0o700;
pub const FILE_MODE: u32 = 0o600;

pub fn private_dir_all(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(path)
}

/// Create a file that must not exist yet, 0600 from the first byte.
/// Exclusive creation is what makes a random scratch name worth having:
/// together they refuse a path an attacker planted. Readable as well as
/// writable, so a caller that fills a file can go on to verify from the
/// same descriptor instead of reopening its name.
pub fn private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
}

/// A fresh scratch directory under the spool's `.tmp`: private, and named
/// unguessably rather than by pid. Created non-recursively, so an existing
/// name is an error rather than something to reuse; that plus 0700 on the
/// spool leaves nothing for a planted path to catch.
pub fn scratch_dir(tmp_root: &Path, what: &str) -> Result<PathBuf, String> {
    private_dir_all(tmp_root).map_err(|e| format!("cannot create tempdir: {e}"))?;
    let dir = tmp_root.join(format!("{what}-{}", random_hex()?));
    fs::DirBuilder::new()
        .mode(DIR_MODE)
        .create(&dir)
        .map_err(|e| format!("cannot create tempdir: {e}"))?;
    Ok(dir)
}

pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// `fsync(2)`, without the drive barrier `sync_all` carries on macOS.
///
/// The kernel hands the bytes to the device and does not wait for the
/// device to flush its own write cache. A process that dies, a `kill
/// -9`, a panic: all keep the write. Losing power keeps whatever the
/// drive got around to. 3.52ms against 0.016ms measured here, which is
/// the whole reason there is a choice to make.
fn fsync_now(f: &File) -> io::Result<()> {
    if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Replace a file's contents in one step: write a scratch file, sync it,
/// rename it over the target, sync the directory. A reader sees the old
/// bytes or the new ones and never a partial write, and after this
/// returns the new ones survive the drive losing power.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_replace(path, bytes, |f| f.sync_all())
}

/// The same replacement, ordered but not barriered.
///
/// For state that a crash may lose without losing a message: what comes
/// back is an older value of something beb recomputes, not a gap where
/// a delivery was. The rename is still atomic, so the file is never
/// half-written; only the wait for the drive's own cache is skipped.
/// Nothing that decides whether a message exists may use this.
pub fn write_atomic_no_barrier(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_replace(path, bytes, fsync_now)
}

fn write_replace(path: &Path, bytes: &[u8], sync: fn(&File) -> io::Result<()>) -> io::Result<()> {
    let dir = path.parent().expect("write_atomic path has a parent");
    let name = random_hex().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let tmp = dir.join(format!(".tmp-{name}"));
    {
        let mut f = private_file(&tmp)?;
        f.write_all(bytes)?;
        sync(&f)?;
    }
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // The directory entry is synced the same way the file was: a barrier
    // on one and not the other is a barrier on neither.
    sync(&File::open(dir)?)
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

    // Both replacements have to leave the same file behind: the barrier
    // is a question about power loss, never about what a reader sees.
    #[test]
    fn write_atomic_both_ways_replace_in_place() {
        use std::os::unix::fs::PermissionsExt;
        for (name, write) in [
            (
                "barriered",
                write_atomic as fn(&Path, &[u8]) -> io::Result<()>,
            ),
            ("plain", write_atomic_no_barrier),
        ] {
            let dir = std::env::temp_dir().join(format!("beb-wa-{}-{}", name, random_hex().unwrap()));
            fs::create_dir(&dir).unwrap();
            let p = dir.join("f");
            write(&p, b"first").unwrap();
            assert_eq!(fs::read(&p).unwrap(), b"first");
            write(&p, b"second").unwrap();
            assert_eq!(fs::read(&p).unwrap(), b"second", "{name} did not replace");
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                FILE_MODE,
                "{name} left the wrong mode"
            );
            // No scratch file survives either path.
            let left: Vec<_> = fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n != "f")
                .collect();
            assert!(left.is_empty(), "{name} left {left:?}");
            fs::remove_dir_all(&dir).unwrap();
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
    fn rfc3339_vectors() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(rfc3339(1_786_000_000), "2026-08-06T07:06:40Z");
        // a leap day, and the century rule around it
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn rfc3339_round_trips() {
        for t in [0i64, 1, 951_782_400, 1_000_000_000, 1_786_000_000, 4_102_444_800] {
            assert_eq!(parse_rfc3339(&rfc3339(t)), Some(t), "{t}");
        }
    }

    #[test]
    fn parse_rfc3339_is_strict() {
        assert_eq!(parse_rfc3339("2026-08-15T02:26:34Z"), Some(1_786_760_794));
        assert_eq!(parse_rfc3339(""), None);
        assert_eq!(parse_rfc3339("2026-08-15T02:26:34"), None);   // no Z
        assert_eq!(parse_rfc3339("2026-08-15 02:26:34Z"), None);  // space, not T
        assert_eq!(parse_rfc3339("2026-08-15T02:26:34+00:00"), None);
        assert_eq!(parse_rfc3339("2026-8-15T02:26:34Z"), None);   // unpadded
        assert_eq!(parse_rfc3339("2026-13-01T00:00:00Z"), None);  // month 13
        assert_eq!(parse_rfc3339("2026-04-31T00:00:00Z"), None);  // April has 30
        assert_eq!(parse_rfc3339("2026-02-30T00:00:00Z"), None);
        assert_eq!(parse_rfc3339("2025-02-29T00:00:00Z"), None);  // not a leap year
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z").is_some(), true);
        assert_eq!(parse_rfc3339("2026-08-15T24:00:00Z"), None);
        assert_eq!(parse_rfc3339("2026-08-15T02:60:00Z"), None);
    }

    // POSIX does not require localtime_r to consult TZ again once the
    // zone is cached, so a test that changes TZ has to say so.
    extern "C" {
        fn tzset();
    }

    #[test]
    fn local_stamp_follows_the_zone() {
        // The stored value is UTC; only the reading changes.
        std::env::set_var("TZ", "UTC");
        unsafe { tzset() };
        assert_eq!(local_stamp(1_000_000_000), "2001-09-09 01:46");
        std::env::set_var("TZ", "Asia/Jakarta");
        unsafe { tzset() };
        assert_eq!(local_stamp(1_000_000_000), "2001-09-09 08:46");
        std::env::set_var("TZ", "America/New_York");
        unsafe { tzset() };
        // the same instant, in a zone west of UTC and on DST that day
        assert_eq!(local_stamp(1_000_000_000), "2001-09-08 21:46");
        std::env::set_var("TZ", "UTC");
        unsafe { tzset() };
    }

    #[test]
    fn one_line_collapses() {
        assert_eq!(one_line("a\nb\r\n  c"), "a b c");
    }
}
