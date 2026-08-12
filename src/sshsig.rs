use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::util::one_line;

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
    Ok(sig)
}

/// Verify `message` against `signature` for the key claimed in `from:`.
/// Consults no trust store: the one-line allowed_signers handed to
/// ssh-keygen is built from the envelope itself. The message streams into
/// ssh-keygen as its stdin, straight from the file.
pub fn verify(
    message: &Path,
    signature: &Path,
    from_canonical: &str,
    scratch: &Path,
) -> Result<(), String> {
    fs::create_dir_all(scratch).map_err(|e| format!("cannot create scratch dir: {e}"))?;
    let allowed = scratch.join("allowed");
    fs::write(&allowed, format!("beb {from_canonical}\n"))
        .map_err(|e| format!("cannot write scratch file: {e}"))?;
    let stdin = File::open(message).map_err(|e| format!("cannot open message: {e}"))?;
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
