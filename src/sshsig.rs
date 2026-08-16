use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::util::{one_line, private_file, scratch_dir, FILE_MODE};

/// Sign `file` with the private key; ssh-keygen writes `file.sig` beside it.
/// The body never passes through this process: ssh-keygen reads the file.
pub fn sign(private_key: &Path, file: &Path) -> Result<PathBuf, String> {
    let out = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "beb", "-f"])
        .arg(private_key)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "signing failed: {}",
            one_line(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    let sig = PathBuf::from(format!("{}.sig", file.display()));
    if !sig.is_file() {
        return Err("signing produced no signature file".into());
    }
    // ssh-keygen made this one, so its mode came from the umask; the file
    // is about to be renamed into the spool, where beb states the modes.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&sig, fs::Permissions::from_mode(FILE_MODE))
        .map_err(|e| format!("cannot set signature permissions: {e}"))?;
    Ok(sig)
}

/// Verify the open `message` against `signature` for the key claimed in
/// `from:`. Consults no trust store: the one-line allowed_signers handed
/// to ssh-keygen is built from the envelope itself.
///
/// The message arrives as a descriptor, not a path, and that is the point:
/// what gets verified is the inode the caller holds open, so a caller that
/// goes on to print from the same handle prints exactly the bytes that
/// passed. A pathname resolved a second time is a different question.
/// The descriptor is rewound here and left wherever ssh-keygen stopped
/// reading, so the caller seeks before using it again.
/// Verify one frame's envelope against the signature stored beside it in
/// the same file.
///
/// Both are slices now rather than whole files, so the envelope is piped
/// to ssh-keygen instead of handed over as a descriptor, and the
/// signature is written out to scratch because `-s` takes a path. What
/// does not change is the property the caller depends on: the bytes
/// verified and the bytes printed come from one open file, so nothing
/// can be swapped underneath between the two.
pub fn verify(
    message: &mut File,
    env_off: u64,
    env_len: u64,
    sig_off: u64,
    sig_len: u64,
    from_canonical: &str,
    tmp_root: &Path,
) -> Result<(), String> {
    let scratch = scratch_dir(tmp_root, "verify")?;
    let verdict = verify_in(
        message,
        env_off,
        env_len,
        sig_off,
        sig_len,
        from_canonical,
        &scratch,
    );
    let _ = fs::remove_dir_all(&scratch);
    verdict
}

fn verify_in(
    message: &mut File,
    env_off: u64,
    env_len: u64,
    sig_off: u64,
    sig_len: u64,
    from_canonical: &str,
    scratch: &Path,
) -> Result<(), String> {
    let allowed = scratch.join("allowed");
    private_file(&allowed)
        .and_then(|mut f| f.write_all(format!("beb {from_canonical}\n").as_bytes()))
        .map_err(|e| format!("cannot write scratch file: {e}"))?;

    let sig_path = scratch.join("sig");
    message
        .seek(SeekFrom::Start(sig_off))
        .map_err(|e| format!("cannot seek to the signature: {e}"))?;
    {
        let mut out =
            private_file(&sig_path).map_err(|e| format!("cannot write scratch file: {e}"))?;
        let n = io::copy(&mut message.take(sig_len), &mut out)
            .map_err(|e| format!("cannot read the signature: {e}"))?;
        if n != sig_len {
            return Err("the frame ends inside its signature".into());
        }
    }

    message
        .seek(SeekFrom::Start(env_off))
        .map_err(|e| format!("cannot seek to the envelope: {e}"))?;
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-I", "beb", "-n", "beb", "-f"])
        .arg(&allowed)
        .arg("-s")
        .arg(&sig_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    {
        // Dropped before the wait, or ssh-keygen never sees end of input.
        let mut stdin = child.stdin.take().ok_or("cannot hand over the envelope")?;
        let n = io::copy(&mut message.take(env_len), &mut stdin)
            .map_err(|e| format!("cannot hand over the envelope: {e}"))?;
        if n != env_len {
            return Err("the frame ends inside its envelope".into());
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(one_line(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}
