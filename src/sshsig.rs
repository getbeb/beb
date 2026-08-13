use std::fs::{self, File};
use std::io::{Seek, SeekFrom, Write};
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
pub fn verify(
    message: &mut File,
    signature: &Path,
    from_canonical: &str,
    tmp_root: &Path,
) -> Result<(), String> {
    let scratch = scratch_dir(tmp_root, "verify")?;
    let verdict = verify_in(message, signature, from_canonical, &scratch);
    let _ = fs::remove_dir_all(&scratch);
    verdict
}

fn verify_in(
    message: &mut File,
    signature: &Path,
    from_canonical: &str,
    scratch: &Path,
) -> Result<(), String> {
    let allowed = scratch.join("allowed");
    private_file(&allowed)
        .and_then(|mut f| f.write_all(format!("beb {from_canonical}\n").as_bytes()))
        .map_err(|e| format!("cannot write scratch file: {e}"))?;
    message
        .seek(SeekFrom::Start(0))
        .map_err(|e| format!("cannot rewind message: {e}"))?;
    let stdin = message
        .try_clone()
        .map_err(|e| format!("cannot hand over the message: {e}"))?;
    let out = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-I", "beb", "-n", "beb", "-f"])
        .arg(&allowed)
        .arg("-s")
        .arg(signature)
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(one_line(&String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}
