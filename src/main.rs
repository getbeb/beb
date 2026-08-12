mod envelope;
mod key;
mod roster;
mod spool;
mod sshsig;
mod util;

use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use envelope::Headers;
use key::PublicKey;
use spool::Mailbox;

const USAGE: &str = "\
beb delivers signed messages between identities on one machine.

  beb init                    key and mailbox from nothing
  beb send RECIPIENT [BODY]   body from argument or stdin
  beb list [--all]            unread by default
  beb read                    consume the next message
  beb read ID                 inspect one message
  beb whoami                  your address";

fn main() {
    // The Rust runtime ignores SIGPIPE; restore the default so
    // `beb list | head` ends quietly instead of panicking on write.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("init") => cmd_init(),
        Some("send") => cmd_send(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        Some("read") => cmd_read(&args[1..]),
        Some("whoami") => cmd_whoami(),
        Some(v) => Err(format!("unknown verb \"{v}\"\n{USAGE}")),
        None => Err(USAGE.to_string()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

struct Identity {
    key: PublicKey,
    private_key: PathBuf,
}

fn identity() -> Result<Identity, String> {
    let private_key = PathBuf::from(".beb/id_ed25519");
    if !private_key.is_file() {
        return Err("not an identity: no .beb here; run beb init".into());
    }
    let text = fs::read_to_string(".beb/id_ed25519.pub")
        .map_err(|_| "broken identity: .beb/id_ed25519.pub is missing".to_string())?;
    let key = key::parse(&text)?;
    if !key.is_ed25519() {
        return Err(format!(
            "identity key is {}; beb speaks ssh-ed25519 only",
            key.kind
        ));
    }
    Ok(Identity { key, private_key })
}

fn cmd_init() -> Result<(), String> {
    if Path::new(".beb").exists() {
        return Err("already an identity: .beb exists here; rm -r .beb to start over".into());
    }
    fs::create_dir(".beb").map_err(|e| format!("cannot create .beb: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(".beb", fs::Permissions::from_mode(0o700));
    }
    fs::write(".beb/.gitignore", "*\n").map_err(|e| format!("cannot write .gitignore: {e}"))?;
    let out = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-C", "", "-f", ".beb/id_ed25519"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "key generation failed: {}",
            util::one_line(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    let me = identity()?;
    let canonical = me.key.canonical();
    let mb = Mailbox::of(&util::spool_root()?, &canonical);
    mb.ensure()?;
    mb.set_cursor(0)?;
    let short: String = mb
        .dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .chars()
        .take(8)
        .collect();
    println!("created .beb/id_ed25519, mailbox {short}...");
    println!("your address: {canonical}");
    println!(
        "name it in {}:",
        util::pretty_path(&util::known_signers_path()?)
    );
    println!("<name> {canonical}");
    Ok(())
}

fn cmd_whoami() -> Result<(), String> {
    println!("{}", identity()?.key.canonical());
    Ok(())
}

fn cmd_send(args: &[String]) -> Result<(), String> {
    let me = identity()?;
    let recipient_arg = args
        .first()
        .ok_or("send needs a recipient: beb send RECIPIENT [BODY]")?;

    let ks_path = util::known_signers_path()?;
    let ks_pretty = util::pretty_path(&ks_path);
    let lines = roster::load(&ks_path);

    let (to, display) = if recipient_arg.chars().any(|c| c.is_whitespace()) {
        let k = key::parse(recipient_arg)?;
        if !k.is_ed25519() {
            return Err(format!(
                "recipient is {}; beb speaks ssh-ed25519 only",
                k.kind
            ));
        }
        let display = roster::reverse(&lines, &k.canonical())
            .map(str::to_string)
            .unwrap_or_else(|| k.canonical());
        (k, display)
    } else if key::looks_like_key_type(recipient_arg) {
        return Err(
            "a key is one argument; quote it: beb send \"ssh-ed25519 AAAA...\" [BODY]".into(),
        );
    } else {
        let k = roster::resolve(&lines, recipient_arg, &ks_pretty)?;
        (k, recipient_arg.clone())
    };

    let spool = util::spool_root()?;
    let tmp = spool.join(".tmp").join(format!("send-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("cannot create tempdir: {e}"))?;
    let result = write_sign_deliver(&me, &to, args, &tmp, &spool);
    let _ = fs::remove_dir_all(&tmp);
    let id = result?;
    println!("accepted {id}; mail waits for {display}");
    Ok(())
}

/// The body streams through disk: envelope tempfile under the spool root
/// (same filesystem as the mailbox, so delivery is a rename), never
/// through a growing buffer. The caller removes the tempdir on every
/// path, success or refusal.
fn write_sign_deliver(
    me: &Identity,
    to: &PublicKey,
    args: &[String],
    tmp: &Path,
    spool: &Path,
) -> Result<u64, String> {
    let env_path = tmp.join("envelope");
    {
        let mut f =
            File::create(&env_path).map_err(|e| format!("cannot write envelope: {e}"))?;
        let nonce = util::random_nonce()?;
        f.write_all(envelope::compose(&me.key.canonical(), &to.canonical(), &nonce).as_bytes())
            .map_err(|e| format!("cannot write envelope: {e}"))?;
        if args.len() > 1 {
            f.write_all(args[1..].join(" ").as_bytes())
                .map_err(|e| format!("cannot write body: {e}"))?;
        } else {
            io::copy(&mut io::stdin().lock(), &mut f)
                .map_err(|e| format!("cannot write body: {e}"))?;
        }
        f.sync_all().map_err(|e| format!("cannot sync envelope: {e}"))?;
    }
    let sig_path = sshsig::sign(&me.private_key, &env_path)?;
    Mailbox::of(spool, &to.canonical()).deliver(&env_path, &sig_path)
}

fn cmd_list(args: &[String]) -> Result<(), String> {
    let all = match args {
        [] => false,
        [a] if a == "--all" => true,
        _ => return Err("list takes --all or nothing".into()),
    };
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    let cursor = mb.cursor();
    let lines = roster::load(&util::known_signers_path()?);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for id in mb.ids() {
        if !all && id <= cursor {
            continue;
        }
        let sender = match envelope::read_headers(&mb.message(id)) {
            Ok(h) => {
                let c = h.from.canonical();
                roster::reverse(&lines, &c).map(str::to_string).unwrap_or(c)
            }
            Err(_) => "?".to_string(),
        };
        writeln!(out, "{id}  {sender}").map_err(|e| format!("cannot write: {e}"))?;
    }
    Ok(())
}

fn cmd_read(args: &[String]) -> Result<(), String> {
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    match args {
        [] => {
            let cursor = mb.cursor();
            let next = mb.ids().into_iter().find(|&id| id > cursor);
            match next {
                None => {
                    eprintln!("no new mail; cursor at {cursor}");
                    Ok(())
                }
                Some(id) => {
                    let h = check(&mb, id, &me)?;
                    print_body(&mb.message(id), h.body_offset)?;
                    mb.set_cursor(id)
                }
            }
        }
        [arg] => {
            let id: u64 = arg
                .parse()
                .ok()
                .filter(|&n| n > 0)
                .ok_or_else(|| format!("not a message id: \"{arg}\""))?;
            if !mb.ids().contains(&id) {
                return Err(format!("no message {id}; beb list --all shows what exists"));
            }
            let h = check(&mb, id, &me)?;
            print_body(&mb.message(id), h.body_offset)
        }
        _ => Err("read takes one id or nothing".into()),
    }
}

/// Everything a message must pass before a byte of it is printed:
/// envelope grammar, ed25519-only, recipient binding, signature. The
/// refusal always names the rm that turns the message into a gap, and the
/// caller's cursor is untouched because nothing has moved yet.
fn check(mb: &Mailbox, id: u64, me: &Identity) -> Result<Headers, String> {
    let mp = mb.message(id);
    let sp = mb.signature(id);
    let rm = format!("rm '{}' '{}'", mp.display(), sp.display());
    let h = envelope::read_headers(&mp)
        .map_err(|e| format!("message {id} is not a beb envelope ({e}); {rm} to make it a gap"))?;
    if !h.from.is_ed25519() || !h.to.is_ed25519() {
        return Err(format!(
            "message {id} has a non-ed25519 key in its envelope; {rm} to make it a gap"
        ));
    }
    if h.to.canonical() != me.key.canonical() {
        return Err(format!(
            "message {id} is addressed to someone else; {rm} to make it a gap"
        ));
    }
    if !sp.is_file() {
        return Err(format!(
            "message {id} has no signature; rm '{}' to make it a gap",
            mp.display()
        ));
    }
    let scratch = util::spool_root()?
        .join(".tmp")
        .join(format!("verify-{}", std::process::id()));
    let verdict = sshsig::verify(&mp, &sp, &h.from.canonical(), &scratch);
    let _ = fs::remove_dir_all(&scratch);
    verdict.map_err(|e| format!("message {id} failed verification ({e}); {rm} to make it a gap"))?;
    Ok(h)
}

/// The body goes file -> stdout with io::copy; it never lands in memory
/// whole.
fn print_body(path: &Path, offset: u64) -> Result<(), String> {
    let mut f = File::open(path).map_err(|e| format!("cannot open message: {e}"))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("cannot seek: {e}"))?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    io::copy(&mut f, &mut out).map_err(|e| format!("cannot print body: {e}"))?;
    out.flush().map_err(|e| format!("cannot print body: {e}"))
}
