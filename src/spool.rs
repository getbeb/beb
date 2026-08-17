use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::util::{fsync_dir, private_dir_all, write_atomic, write_atomic_no_barrier, FILE_MODE};

/// How far back a duplicate is looked for, in ids.
///
/// Not a knob. It is the one number that turns "exactly once, forever"
/// into something a delivery can afford, and it states a guarantee: a
/// retransmission is recognised if fewer than this many ids have been
/// issued since the original. A transport retries within seconds of a
/// lost acknowledgement, and a depot re-offers what a courier failed to
/// confirm -- both are near the top of the mailbox, and neither is a
/// thousand messages behind it.
const DEDUPE_WINDOW: u64 = 1000;

pub struct Mailbox {
    pub dir: PathBuf,
}

fn name(id: u64) -> String {
    format!("{:018}", id)
}

impl Mailbox {
    pub fn of(spool: &Path, canonical_key: &str) -> Mailbox {
        Mailbox {
            dir: spool.join(crate::key::mailbox_name(canonical_key)),
        }
    }

    pub fn msgs(&self) -> PathBuf {
        self.dir.join("msg")
    }

    pub fn msg(&self, id: u64) -> PathBuf {
        self.msgs().join(name(id))
    }

    pub fn ensure(&self) -> Result<(), String> {
        private_dir_all(&self.msgs()).map_err(|e| format!("cannot create mailbox: {e}"))
    }

    /// A stored message, opened once and positioned at its first
    /// envelope byte, with the two lengths its frame header declared.
    ///
    /// One file, not two. A message used to be an envelope beside a
    /// detached signature, which meant the bytes that travel and the
    /// bytes that rest were different arrangements of the same thing --
    /// and a message, once received, could never be handed back to a
    /// transport as the frame it arrived as. Storing the frame makes
    /// delivery a write and collection a read.
    pub fn open_frame(&self, id: u64) -> Result<(File, u64, u64, u64), String> {
        let path = self.msg(id);
        let mut f = File::open(&path).map_err(|e| format!("cannot open message {id}: {e}"))?;
        let (env, sig) = crate::frame::read_header(&mut f)?;
        Ok((f, crate::frame::header_len(env, sig), env, sig))
    }

    /// The highest id ever assigned here, whether or not it survives.
    ///
    /// Written before the message it names, so it is always at least the
    /// largest id in `msg/`, and it is what makes ids guessable: a
    /// caller can walk the space without reading the directory.
    pub fn high(&self) -> u64 {
        fs::read_to_string(self.dir.join(".counter"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn has(&self, id: u64) -> bool {
        self.msg(id).is_file()
    }

    /// The first id above `after` that is still here.
    ///
    /// One `stat` in the ordinary case, because ids are dense: only
    /// pruning and crashed sends leave holes, and each costs one more.
    /// This is the whole reason ids are a counter rather than a
    /// timestamp -- `after + 1` is a path you can guess, and a name you
    /// can guess is a directory you never have to read.
    ///
    /// A 200k-message mailbox answered this in 292ms by listing and
    /// sorting the directory; a stat is under four microseconds.
    pub fn next_after(&self, after: u64) -> Option<u64> {
        let high = self.high();
        let mut id = after + 1;
        while id <= high {
            if self.has(id) {
                return Some(id);
            }
            id += 1;
        }
        None
    }

    /// Up to `limit` ids above `after`, ascending.
    pub fn window_after(&self, after: u64, limit: usize) -> Vec<u64> {
        let high = self.high();
        let mut out = Vec::new();
        let mut id = after + 1;
        while id <= high && out.len() < limit {
            if self.has(id) {
                out.push(id);
            }
            id += 1;
        }
        out
    }

    /// The `limit` ids nearest below `before`, ascending.
    pub fn window_before(&self, before: u64, limit: usize) -> Vec<u64> {
        let mut out = Vec::new();
        let mut id = before;
        while id > 1 && out.len() < limit {
            id -= 1;
            if self.has(id) {
                out.push(id);
            }
        }
        out.reverse();
        out
    }

    /// The `limit` ids nearest below `before` that are still above
    /// `floor`, ascending.
    ///
    /// `window_before` walks to the start of the mailbox; this stops at a
    /// floor, which is what "the newest unread" needs: the cursor is the
    /// floor, so the walk cannot fall into mail already read.
    pub fn window_between(&self, floor: u64, before: u64, limit: usize) -> Vec<u64> {
        let mut out = Vec::new();
        let mut id = before;
        while id > floor + 1 && out.len() < limit {
            id -= 1;
            if self.has(id) {
                out.push(id);
            }
        }
        out.reverse();
        out
    }

    /// Whether anything is still here below `id`, for a paging hint.
    pub fn any_below(&self, id: u64) -> bool {
        !self.window_before(id, 1).is_empty()
    }

    pub fn cursor(&self) -> u64 {
        fs::read_to_string(self.dir.join("cursor"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Whether this mailbox has an owner on this machine.
    ///
    /// The directory existing is the whole test, because `init` is now
    /// the only thing that creates one: mail for a key that reads
    /// somewhere else goes to the outbox, not into a mailbox nobody
    /// claimed. Until 0.9.0 `send` created those, so a directory proved
    /// nothing and the cursor file had to carry residency as well as
    /// position -- two meanings in one file, with a comment warning that
    /// `cursor() == 0` was not the test. One of those meanings has moved
    /// out, and the cursor is a position again.
    pub fn claimed(&self) -> bool {
        self.dir.is_dir()
    }

    /// Written atomically, and without the drive barrier.
    ///
    /// The barrier is what makes a delivery real, and the cursor is not a
    /// delivery: it is a position over messages that are already durable.
    /// Losing it to a power cut hands back an older position, so the next
    /// `read` shows a message that was shown once before -- the same
    /// outcome as the machine dying one instruction earlier, which the
    /// cursor already has to survive. Nothing is lost that beb cannot say
    /// again.
    ///
    /// It is not a small saving: `read` was 14.9ms and is 7.3ms, because
    /// the two barriers behind one cursor write cost more than the
    /// ssh-keygen that verifies the signature.
    pub fn set_cursor(&self, id: u64) -> Result<(), String> {
        write_atomic_no_barrier(&self.dir.join("cursor"), id.to_string().as_bytes())
            .map_err(|e| format!("cannot write cursor: {e}"))
    }

    /// Accept a signed envelope. The write order is the proof: counter
    /// first, then signature, then message, each durable before the next,
    /// so a crash leaves a gap, never a reused id, never a visible message
    /// without its signature. A failure partway may consume an id and
    /// leave a stray signature, debris that is never a message; what it
    /// can never do is make a message visible without its signature.
    pub fn deliver(&self, frame: &Path) -> Result<u64, String> {
        self.ensure()?;
        let _lock = self.lock()?;
        self.install(frame)
    }

    /// Accept unless the exact envelope bytes are already retained. The
    /// duplicate check and the insertion happen under the same exclusive
    /// lock, so concurrent retries of one delivery converge to one
    /// message: the decision is atomic with the act.
    pub fn deliver_once(&self, frame: &Path) -> Result<Delivered, String> {
        self.ensure()?;
        let _lock = self.lock()?;

        let incoming_len = fs::metadata(frame)
            .map_err(|e| format!("cannot stat frame: {e}"))?
            .len();
        let mut incoming_hash: Option<String> = None;

        // Backwards from the newest, and not far. A duplicate is a
        // transport retrying a delivery whose acknowledgement went
        // missing, so it always arrives close behind the original -- and
        // walking down from `.counter` needs no directory read, because
        // an id is a path you can guess in either direction.
        //
        // This compared against every message ever retained until 0.9.0:
        // 391ms on a 50k mailbox, growing forever, to answer a question
        // about the last few minutes. What that bought was exactly-once
        // for all time; what it cost was a delivery that gets slower
        // every day it works.
        let high = self.high();
        let floor = high.saturating_sub(DEDUPE_WINDOW);
        let mut id = high;
        while id > floor {
            let p = self.msg(id);
            // Length is only the fast path to skip hashing. A message
            // that cannot be measured cannot be ruled out as the
            // duplicate, and an unreadable message must never read as an
            // absent one: that would quietly downgrade exactly-once to
            // maybe-twice. Gone is different from unreadable, because a
            // pruned message is a gap and gaps are legal.
            match fs::metadata(&p) {
                Ok(m) if m.len() == incoming_len => {
                    if incoming_hash.is_none() {
                        incoming_hash = Some(
                            crate::util::sha256_file(frame)
                                .map_err(|e| format!("cannot hash frame: {e}"))?,
                        );
                    }
                    match crate::util::sha256_file(&p) {
                        Ok(h) if Some(h.as_str()) == incoming_hash.as_deref() => {
                            return Ok(Delivered::Already(id))
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(format!(
                                "cannot hash message {id} ({e}); rm '{}' to make it a gap",
                                p.display()
                            ))
                        }
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "cannot stat message {id} ({e}); rm '{}' to make it a gap",
                        p.display()
                    ))
                }
            }
            id -= 1;
        }
        self.install(frame).map(Delivered::Fresh)
    }

    fn lock(&self) -> Result<File, String> {
        self.lock_file(".lock", "mailbox")
    }

    /// The consumption lock, held across choosing a message, verifying it,
    /// printing it, and advancing the cursor. Delivery has always been
    /// serialized; consumption needs the same rigor, because a cursor read
    /// before another reader's write and set after it moves the cursor
    /// backwards and hands the same message out twice.
    ///
    /// It is a different file from the delivery lock on purpose: `read`
    /// holds this one for as long as its stdout takes to drain, and a
    /// reader piping a large body into something slow must not stall
    /// senders. Readers wait for readers; delivery never waits for either.
    pub fn read_lock(&self) -> Result<File, String> {
        self.lock_file(".reading", "reader")
    }

    fn lock_file(&self, name: &str, what: &str) -> Result<File, String> {
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .mode(FILE_MODE)
            .open(self.dir.join(name))
            .map_err(|e| format!("cannot open {what} lock: {e}"))?;
        flock_exclusive(&lock)?;
        Ok(lock)
    }

    /// Counter, then message; the caller holds the lock.
    ///
    /// One file to place instead of two, so the old ordering worry --
    /// never a visible message without its signature -- is now a
    /// property of the format rather than of the write order: a frame
    /// carries both or it is not a frame.
    fn install(&self, frame: &Path) -> Result<u64, String> {
        let counter_path = self.dir.join(".counter");
        let counter: u64 = fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let id = counter + 1;
        write_atomic(&counter_path, id.to_string().as_bytes())
            .map_err(|e| format!("cannot advance counter: {e}"))?;

        place(frame, &self.msg(id), &self.msgs())?;
        Ok(id)
    }
}

/// Envelope and detached signature into one frame, beside them in the
/// same scratch directory, so what lands in the spool is exactly what
/// would travel.
pub fn assemble(envelope: &Path, signature: &Path) -> Result<PathBuf, String> {
    let env_len = fs::metadata(envelope)
        .map_err(|e| format!("cannot stat envelope: {e}"))?
        .len();
    let sig_len = fs::metadata(signature)
        .map_err(|e| format!("cannot stat signature: {e}"))?
        .len();
    let out = envelope.with_extension("frame");
    let mut f = crate::util::private_file(&out).map_err(|e| format!("cannot write frame: {e}"))?;
    crate::frame::write_header(&mut f, env_len, sig_len)
        .map_err(|e| format!("cannot write frame: {e}"))?;
    for part in [envelope, signature] {
        let mut r = File::open(part).map_err(|e| format!("cannot read {}: {e}", part.display()))?;
        std::io::copy(&mut r, &mut f).map_err(|e| format!("cannot write frame: {e}"))?;
    }
    // Not synced here either. `place` opens this file and syncs it
    // before the rename that makes it visible, so syncing twice buys
    // one extra full flush and no durability.
    Ok(out)
}

/// Frames held here for keys that read somewhere else.
///
/// Flat and per-spool rather than per-recipient: one directory means one
/// kernel watch for "is there anything to carry", instead of one per
/// correspondent. A carrier reads it in id order and removes what it has
/// handed over.
pub struct Outbox {
    pub dir: PathBuf,
}

impl Outbox {
    pub fn at(spool: &Path) -> Outbox {
        Outbox {
            dir: spool.join("outbox"),
        }
    }

    /// What is waiting to leave, oldest first: the id, the recipient,
    /// and the file.
    ///
    /// The recipient is in the name, which is the whole reason a carrier
    /// needs nothing from beb to drain this directory. It used to be
    /// inside the frame only, so a carrier either parsed a frame -- the
    /// one thing it must never do -- or asked beb to read it out, which
    /// cost two process spawns per message to recover a field that could
    /// have been in the filename all along.
    pub fn entries(&self) -> Vec<(u64, String, PathBuf)> {
        let mut out: Vec<(u64, String, PathBuf)> = match fs::read_dir(&self.dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let n = e.file_name().to_str()?.to_string();
                    let (id, to) = n.split_once('-')?;
                    Some((id.parse().ok()?, to.to_string(), e.path()))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        out.sort_unstable_by_key(|(id, _, _)| *id);
        out
    }

    pub fn put(&self, frame: &Path, to: &str) -> Result<u64, String> {
        private_dir_all(&self.dir).map_err(|e| format!("cannot create the outbox: {e}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .mode(FILE_MODE)
            .open(self.dir.join(".lock"))
            .map_err(|e| format!("cannot open outbox lock: {e}"))?;
        flock_exclusive(&lock)?;
        let counter_path = self.dir.join(".counter");
        let counter: u64 = fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let id = counter + 1;
        write_atomic(&counter_path, id.to_string().as_bytes())
            .map_err(|e| format!("cannot advance the outbox counter: {e}"))?;
        place(frame, &self.dir.join(format!("{}-{to}", name(id))), &self.dir)?;
        Ok(id)
    }
}

pub enum Delivered {
    Fresh(u64),
    Already(u64),
}

/// Durable move into the spool: fsync the file, rename it in (same
/// filesystem, because send's tempdir lives under the spool root), fsync
/// the directory.
///
/// This is the durability boundary, and the only one. Callers hand over
/// a temp file and do not sync it themselves: nothing before the rename
/// is reachable after a crash, so a barrier spent there is a barrier
/// spent on bytes nobody will look for.
fn place(src: &Path, dst: &Path, dir: &Path) -> Result<(), String> {
    File::open(src)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("cannot sync {}: {e}", src.display()))?;
    fs::rename(src, dst).map_err(|e| format!("cannot place {}: {e}", dst.display()))?;
    fsync_dir(dir).map_err(|e| format!("cannot sync {}: {e}", dir.display()))
}

fn flock_exclusive(f: &File) -> Result<(), String> {
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    if r != 0 {
        return Err(format!(
            "cannot lock mailbox: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_names_are_18_digit() {
        assert_eq!(name(1), "000000000000000001");
        assert_eq!(name(42), "000000000000000042");
    }

    #[test]
    fn mailbox_dir_is_the_key_in_hex() {
        // A real key, so the blob decodes; the directory must be the 32
        // key bytes in hex, and must reverse back to the same key.
        let canonical = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINgR8hdn1Atho8CT1CP90F81jvU9B7pMmWkSFci6DXVu";
        let mb = Mailbox::of(Path::new("/spool"), canonical);
        let name = mb.dir.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), 64, "{name}");
        assert_eq!(
            name,
            "d811f21767d40b61a3c093d423fdd05f358ef53d07ba4c99691215c8ba0d756e"
        );
        // and it is not the hash of the text it names
        assert_ne!(name, crate::util::sha256_hex(canonical));
    }
}
