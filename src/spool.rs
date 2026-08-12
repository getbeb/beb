use std::fs::{self, File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use crate::util::{fsync_dir, sha256_hex, write_atomic};

pub struct Mailbox {
    pub dir: PathBuf,
}

fn name(id: u64) -> String {
    format!("{:018}", id)
}

impl Mailbox {
    pub fn of(spool: &Path, canonical_key: &str) -> Mailbox {
        Mailbox {
            dir: spool.join(sha256_hex(canonical_key)),
        }
    }

    pub fn messages(&self) -> PathBuf {
        self.dir.join("messages")
    }

    pub fn signatures(&self) -> PathBuf {
        self.dir.join("signatures")
    }

    pub fn message(&self, id: u64) -> PathBuf {
        self.messages().join(name(id))
    }

    pub fn signature(&self, id: u64) -> PathBuf {
        self.signatures().join(name(id))
    }

    pub fn ensure(&self) -> Result<(), String> {
        fs::create_dir_all(self.messages())
            .and_then(|_| fs::create_dir_all(self.signatures()))
            .map_err(|e| format!("cannot create mailbox: {e}"))
    }

    /// Delivery ids present, ascending. Tolerant: non-numeric names
    /// (retention dotfiles, strays) are ignored.
    pub fn ids(&self) -> Vec<u64> {
        let mut out: Vec<u64> = match fs::read_dir(self.messages()) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))
                .collect(),
            Err(_) => Vec::new(),
        };
        out.sort_unstable();
        out
    }

    pub fn cursor(&self) -> u64 {
        fs::read_to_string(self.dir.join("cursor"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn set_cursor(&self, id: u64) -> Result<(), String> {
        write_atomic(&self.dir.join("cursor"), id.to_string().as_bytes())
            .map_err(|e| format!("cannot write cursor: {e}"))
    }

    /// Accept a signed envelope. The write order is the proof: counter
    /// first, then signature, then message, each durable before the next,
    /// so a crash leaves a gap, never a reused id, never a visible message
    /// without its signature. A failure partway may consume an id and
    /// leave a stray signature, debris that is never a message; what it
    /// can never do is make a message visible without its signature.
    pub fn deliver(&self, envelope: &Path, signature: &Path) -> Result<u64, String> {
        self.ensure()?;
        let _lock = self.lock()?;
        self.install(envelope, signature)
    }

    /// Accept unless the exact envelope bytes are already retained. The
    /// duplicate check and the insertion happen under the same exclusive
    /// lock, so concurrent retries of one delivery converge to one
    /// message: the decision is atomic with the act.
    pub fn deliver_once(&self, envelope: &Path, signature: &Path) -> Result<Delivered, String> {
        self.ensure()?;
        let _lock = self.lock()?;

        let incoming_len = fs::metadata(envelope)
            .map_err(|e| format!("cannot stat envelope: {e}"))?
            .len();
        let mut incoming_hash: Option<String> = None;
        for id in self.ids() {
            let p = self.message(id);
            if fs::metadata(&p).map(|m| m.len()).unwrap_or(0) != incoming_len {
                continue;
            }
            if incoming_hash.is_none() {
                incoming_hash = Some(
                    crate::util::sha256_file(envelope)
                        .map_err(|e| format!("cannot hash envelope: {e}"))?,
                );
            }
            if crate::util::sha256_file(&p).ok().as_deref() == incoming_hash.as_deref() {
                return Ok(Delivered::Already(id));
            }
        }
        self.install(envelope, signature).map(Delivered::Fresh)
    }

    fn lock(&self) -> Result<File, String> {
        let lock = OpenOptions::new()
            .create(true)
            .write(true)
            .open(self.dir.join(".lock"))
            .map_err(|e| format!("cannot open mailbox lock: {e}"))?;
        flock_exclusive(&lock)?;
        Ok(lock)
    }

    /// Counter, then signature, then message; the caller holds the lock.
    fn install(&self, envelope: &Path, signature: &Path) -> Result<u64, String> {
        let counter_path = self.dir.join(".counter");
        let counter: u64 = fs::read_to_string(&counter_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let id = counter + 1;
        write_atomic(&counter_path, id.to_string().as_bytes())
            .map_err(|e| format!("cannot advance counter: {e}"))?;

        place(signature, &self.signature(id), &self.signatures())?;
        place(envelope, &self.message(id), &self.messages())?;
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
    fn mailbox_dir_is_sha256_of_key_text() {
        let mb = Mailbox::of(Path::new("/spool"), "ssh-ed25519 AAAA");
        assert_eq!(
            mb.dir,
            Path::new("/spool").join(crate::util::sha256_hex("ssh-ed25519 AAAA"))
        );
    }
}
