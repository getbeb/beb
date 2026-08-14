mod envelope;
mod frame;
mod key;
mod roster;
mod spool;
mod sshsig;
mod util;
mod waitfs;

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use envelope::Headers;
use key::PublicKey;
use spool::Mailbox;

const USAGE: &str = "\
beb {version} delivers signed messages between identities.

  beb init                    key and mailbox from nothing
  beb whoami                  your address
  beb send RECIPIENT [BODY]   sign and deliver, body from argument or stdin
  beb list [--all]            what is waiting, unread by default
  beb read                    consume the next message
  beb peek ID                 inspect one message, consuming nothing
  beb wait [-t SECS]          block until the next message arrives
  beb pack RECIPIENT [BODY]   sign one delivery onto stdout
  beb receive                 install one delivery from stdin";

/// The usage text names its own version: help that cannot say which
/// binary printed it is help you have to go check.
fn usage() -> String {
    USAGE.replace("{version}", env!("CARGO_PKG_VERSION"))
}

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
        Some("peek") => cmd_peek(&args[1..]),
        Some("wait") => cmd_wait(&args[1..]),
        Some("pack") => cmd_pack(&args[1..]),
        Some("receive") => cmd_receive(&args[1..]),
        Some("whoami") => cmd_whoami(),
        Some("--version") => {
            println!("beb {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(v) => Err(format!("unknown verb \"{v}\"\n{}", usage())),
        None => Err(usage()),
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

/// An identity claim at a directory is one of three things, and the
/// difference is load-bearing: Absent means no claim exists, Broken
/// means a claim exists that cannot be established. Only Absent may be
/// passed over; a broken claim always refuses, because failure to
/// establish agreement must never become precedence.
enum IdClaim {
    Absent,
    Broken(String),
}

fn identity_at(dir: &Path) -> Result<Identity, IdClaim> {
    // The claim is the .beb itself; anything wrong past that point is a
    // claim that exists but cannot be established.
    let beb = dir.join(".beb");
    if !beb.exists() {
        return Err(IdClaim::Absent);
    }
    if !beb.is_dir() {
        return Err(IdClaim::Broken("broken identity: .beb is not a directory".into()));
    }
    let private_key = beb.join("id_ed25519");
    if !private_key.is_file() {
        return Err(IdClaim::Broken(
            "broken identity: .beb/id_ed25519 is missing".into(),
        ));
    }
    let text = fs::read_to_string(dir.join(".beb/id_ed25519.pub")).map_err(|_| {
        IdClaim::Broken("broken identity: .beb/id_ed25519.pub is missing".into())
    })?;
    let key = key::parse(&text).map_err(|e| IdClaim::Broken(format!("broken identity: {e}")))?;
    if !key.is_ed25519() {
        return Err(IdClaim::Broken(format!(
            "broken identity: key is {}; beb speaks ssh-ed25519 only",
            key.kind
        )));
    }
    Ok(Identity { key, private_key })
}

/// Two sources, no precedence: the working directory's `.beb`, or the
/// directory named by BEB_IDENTITY (the directory you would have cd'd
/// to). When both are present they must agree, and agreement is judged
/// by canonical public key, never by path. Disagreement refuses, a
/// broken claim on either side refuses, and nothing is ever guessed
/// between two claimants.
fn identity() -> Result<Identity, String> {
    let env = std::env::var_os("BEB_IDENTITY")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let cwd = identity_at(Path::new("."));
    match (env, cwd) {
        (None, Ok(id)) => Ok(id),
        (None, Err(IdClaim::Absent)) => {
            Err("not an identity: no .beb here; run beb init".into())
        }
        (None, Err(IdClaim::Broken(e))) => Err(format!("{e}; fix or remove ./.beb")),
        (Some(dir), cwd) => {
            let from_env = match identity_at(&dir) {
                Ok(id) => id,
                Err(IdClaim::Absent) => {
                    return Err(format!(
                        "BEB_IDENTITY={} has no .beb; run beb init there or unset BEB_IDENTITY",
                        dir.display()
                    ))
                }
                Err(IdClaim::Broken(e)) => {
                    return Err(format!("BEB_IDENTITY={}: {e}", dir.display()))
                }
            };
            match cwd {
                Err(IdClaim::Absent) => Ok(from_env),
                Err(IdClaim::Broken(e)) => Err(format!(
                    "{e} in ./.beb while BEB_IDENTITY is set; agreement cannot be checked. fix or remove ./.beb"
                )),
                Ok(local) if local.key.canonical() == from_env.key.canonical() => Ok(local),
                Ok(_) => Err(format!(
                    "two identities claim this process: BEB_IDENTITY={} and ./.beb disagree; unset BEB_IDENTITY or cd",
                    dir.display()
                )),
            }
        }
    }
}

/// Where a new identity goes. The same rule every other verb resolves
/// by: the directory named by BEB_IDENTITY, or the working directory.
/// init read the working directory alone until 0.5.2, which made it the
/// one verb that answered about a directory nobody had asked about, and
/// made the refusal `run beb init there` unfollowable by the caller most
/// likely to read it.
fn init_dir() -> Result<(PathBuf, bool), String> {
    match std::env::var_os("BEB_IDENTITY").filter(|v| !v.is_empty()) {
        Some(dir) => Ok((PathBuf::from(dir), true)),
        None => Ok((PathBuf::from("."), false)),
    }
}

fn cmd_init() -> Result<(), String> {
    let (dir, from_env) = init_dir()?;
    let beb = dir.join(".beb");
    let shown = |p: &Path| -> String {
        if from_env {
            util::pretty_path(p)
        } else {
            p.strip_prefix(".").unwrap_or(p).display().to_string()
        }
    };
    // Everything that can refuse, refuses before a key exists. A verb that
    // generates a keypair and then fails leaves a private key behind and a
    // directory that answers "already an identity" to the retry.
    if beb.exists() {
        return Err(format!(
            "already an identity: {} exists; rm -r {} to start over",
            shown(&beb),
            shown(&beb)
        ));
    }
    if !dir.is_dir() {
        return Err(format!(
            "BEB_IDENTITY={} is not a directory; create it or unset BEB_IDENTITY",
            dir.display()
        ));
    }
    // Two claimants is a refusal everywhere else, so it is a refusal here
    // rather than something to construct: an identity made in the named
    // directory while the working directory holds another would leave
    // every later verb in this cwd refusing to guess between them.
    if from_env && Path::new(".beb").exists() {
        return Err(format!(
            "./.beb already claims this working directory, so an identity at {} \
would leave both claiming every later command; cd elsewhere or unset BEB_IDENTITY",
            util::pretty_path(&dir)
        ));
    }

    fs::create_dir(&beb).map_err(|e| format!("cannot create {}: {e}", shown(&beb)))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&beb, fs::Permissions::from_mode(0o700));
    }
    let private = beb.join("id_ed25519");
    fs::write(beb.join(".gitignore"), "*\n")
        .map_err(|e| format!("cannot write .gitignore: {e}"))?;
    let out = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-C", "", "-f"])
        .arg(&private)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("cannot run ssh-keygen: {e}"))?;
    if !out.status.success() {
        let _ = fs::remove_dir_all(&beb);
        return Err(format!(
            "key generation failed: {}",
            util::one_line(&String::from_utf8_lossy(&out.stderr))
        ));
    }
    // Resolved from the directory just written, not from the environment
    // again: what init reports is what init made.
    let me = identity_at(&dir).map_err(|e| match e {
        IdClaim::Absent => "the new identity vanished before it could be read".to_string(),
        IdClaim::Broken(e) => e,
    })?;
    let canonical = me.key.canonical();
    let spool = util::spool_root()?;
    let mb = Mailbox::of(&spool, &canonical);
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
    // The ack names known_signers as the next step, so the directory it
    // lives in must exist for an append to land there. beb still never
    // writes the file itself: the names in it are the reader's.
    let ks = util::known_signers_path()?;
    if let Some(d) = ks.parent() {
        fs::create_dir_all(d)
            .map_err(|e| format!("cannot create {}: {e}", util::pretty_path(d)))?;
    }
    println!("created {}, mailbox {short}...", shown(&private));
    println!("your address: {canonical}");
    println!("name it in {}:", util::pretty_path(&ks));
    println!("<name> {canonical}");
    Ok(())
}

fn cmd_whoami() -> Result<(), String> {
    println!("{}", identity()?.key.canonical());
    Ok(())
}

/// RECIPIENT is a roster name or tolerantly parsed key text; the display
/// form is the name when one is known.
fn resolve_recipient(
    arg: &str,
    lines: &[roster::Line],
    ks_pretty: &str,
    verb: &str,
) -> Result<(PublicKey, String), String> {
    if arg.chars().any(|c| c.is_whitespace()) {
        let k = key::parse(arg)?;
        if !k.is_ed25519() {
            return Err(format!("recipient is {}; beb speaks ssh-ed25519 only", k.kind));
        }
        let display = roster::reverse(lines, &k.canonical())
            .map(str::to_string)
            .unwrap_or_else(|| k.canonical());
        Ok((k, display))
    } else if key::looks_like_key_type(arg) {
        Err(format!(
            "a key is one argument; quote it: beb {verb} \"ssh-ed25519 AAAA...\" [BODY]"
        ))
    } else {
        let k = roster::resolve(lines, arg, ks_pretty)?;
        Ok((k, arg.to_string()))
    }
}

fn cmd_send(args: &[String]) -> Result<(), String> {
    let me = identity()?;
    let recipient_arg = args
        .first()
        .ok_or("send needs a recipient: beb send RECIPIENT [BODY]")?;
    let ks_path = util::known_signers_path()?;
    let lines = roster::load(&ks_path);
    let (to, display) = resolve_recipient(recipient_arg, &lines, &util::pretty_path(&ks_path), "send")?;

    let spool = util::spool_root()?;
    let tmp = util::scratch_dir(&spool.join(".tmp"), "send")?;
    let result = write_signed_envelope(&me, &to, args, &tmp)
        .and_then(|(env, sig)| Mailbox::of(&spool, &to.canonical()).deliver(&env, &sig));
    let _ = fs::remove_dir_all(&tmp);
    let id = result?;
    println!("accepted {id}; mail waits for {display}");
    Ok(())
}

/// Construct and sign, touching no mailbox. The body streams through
/// disk: envelope tempfile under the spool root (same filesystem as the
/// mailbox, so delivery is a rename), never through a growing buffer.
/// The caller removes the tempdir on every path, success or refusal.
fn write_signed_envelope(
    me: &Identity,
    to: &PublicKey,
    args: &[String],
    tmp: &Path,
) -> Result<(PathBuf, PathBuf), String> {
    let env_path = tmp.join("envelope");
    {
        let mut f =
            util::private_file(&env_path).map_err(|e| format!("cannot write envelope: {e}"))?;
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
    Ok((env_path, sig_path))
}

/// pack: construct -> sign -> frame on stdout. No mailbox, counter, or
/// cursor is touched anywhere; stdout is the product and success is
/// silent.
fn cmd_pack(args: &[String]) -> Result<(), String> {
    let me = identity()?;
    let recipient_arg = args
        .first()
        .ok_or("pack needs a recipient: beb pack RECIPIENT [BODY]")?;
    let ks_path = util::known_signers_path()?;
    let lines = roster::load(&ks_path);
    let (to, _) = resolve_recipient(recipient_arg, &lines, &util::pretty_path(&ks_path), "pack")?;

    let spool = util::spool_root()?;
    let tmp = util::scratch_dir(&spool.join(".tmp"), "pack")?;
    let result = write_signed_envelope(&me, &to, args, &tmp).and_then(|(env, sig)| {
        let el = fs::metadata(&env).map_err(|e| format!("cannot stat envelope: {e}"))?.len();
        let sl = fs::metadata(&sig).map_err(|e| format!("cannot stat signature: {e}"))?.len();
        let stdout = io::stdout();
        let mut out = stdout.lock();
        frame::write_header(&mut out, el, sl)
            .and_then(|_| io::copy(&mut File::open(&env)?, &mut out).map(|_| ()))
            .and_then(|_| io::copy(&mut File::open(&sig)?, &mut out).map(|_| ()))
            .and_then(|_| out.flush())
            .map_err(|e| format!("cannot write the delivery: {e}"))
    });
    let _ = fs::remove_dir_all(&tmp);
    result
}

/// receive: one frame from stdin, verified before anything becomes
/// visible, installed through the same machinery as local delivery.
/// It resolves no identity: the delivery carries its own address, and
/// a mailbox that already exists here is what makes that address a
/// resident. Receiving is not reading, so nothing here needs a private
/// key.
fn cmd_receive(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("receive takes nothing; the delivery arrives on stdin".into());
    }
    let spool = util::spool_root()?;
    let tmp = util::scratch_dir(&spool.join(".tmp"), "receive")?;
    let result = receive_one(&spool, &tmp);
    let _ = fs::remove_dir_all(&tmp);
    match result? {
        spool::Delivered::Fresh(id) => println!("accepted {id}; read with: beb read"),
        spool::Delivered::Already(id) => println!("accepted {id}; already delivered"),
    }
    Ok(())
}

fn receive_one(spool: &Path, tmp: &Path) -> Result<spool::Delivered, String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    // The frame refuses an impossible signature length here, before a byte
    // of it is read.
    let (el, sl) = frame::read_header(&mut input)?;

    // Admission runs on the header prefix, in memory, bounded by the same
    // limit the envelope grammar has always had. Nothing reaches disk
    // until the delivery has named a mailbox that exists here, so an
    // arbitrary stranger cannot spend the recipient's disk: the mailbox
    // check is no longer downstream of writing the body.
    let want = el.min(envelope::HEADER_MAX as u64) as usize;
    let mut prefix = vec![0u8; want];
    fill(&mut input, &mut prefix, el, "envelope")?;
    let h = envelope::parse_headers(&prefix)
        .map_err(|e| format!("delivery is not a beb envelope ({e})"))?;
    if !h.from.is_ed25519() || !h.to.is_ed25519() {
        return Err("envelope has a non-ed25519 key; beb speaks ssh-ed25519 only".into());
    }
    // The envelope names its own mailbox, and an existing mailbox is
    // the only admission: an identity that has run `beb init` here
    // lives here, and one that has not cannot be conjured by anything
    // arriving from outside. So a delivery for a stranger is refused
    // rather than minting a mailbox nobody reads.
    let mailbox = Mailbox::of(spool, &h.to.canonical());
    if !mailbox.dir.is_dir() {
        return Err(format!(
            "no mailbox here for {}; its owner claims one with: beb init",
            &util::sha256_hex(&h.to.canonical())[..8]
        ));
    }

    // Admitted. The body may land now, still uncapped and still streaming
    // through disk: a signature covers the whole envelope, so no design
    // that refuses to hold a body in memory can verify one before storing
    // it. What the admission bought is that only a resident's address can
    // ask for the space.
    let env_path = tmp.join("envelope");
    let sig_path = tmp.join("envelope.sig");
    let mut env = util::private_file(&env_path)
        .map_err(|e| format!("cannot write envelope: {e}"))?;
    env.write_all(&prefix)
        .map_err(|e| format!("cannot write envelope: {e}"))?;
    copy_exact(&mut input, &mut env, el - want as u64, want as u64, el, "envelope")?;
    let mut sig = util::private_file(&sig_path)
        .map_err(|e| format!("cannot write signature: {e}"))?;
    copy_exact(&mut input, &mut sig, sl, 0, sl, "signature")?;

    let mut trail = [0u8; 1];
    let extra = input
        .read(&mut trail)
        .map_err(|e| format!("cannot read frame: {e}"))?;
    if extra != 0 {
        return Err("trailing bytes after the frame; one frame is one delivery".into());
    }

    sshsig::verify(&mut env, &sig_path, &h.from.canonical(), &spool.join(".tmp"))
        .map_err(|e| format!("signature verification failed ({e})"))?;

    // Idempotent over retained history, atomically: the dedup decision
    // and the insertion happen inside the mailbox lock, so concurrent
    // retries of the same delivery converge to one message.
    mailbox.deliver_once(&env_path, &sig_path)
}

/// Fill the buffer from the stream; short is a truncated frame. `total` is
/// what the frame claimed for this part, so the refusal counts the part
/// rather than the read.
fn fill(r: &mut impl Read, buf: &mut [u8], total: u64, what: &str) -> Result<(), String> {
    let mut n = 0;
    while n < buf.len() {
        let k = r
            .read(&mut buf[n..])
            .map_err(|e| format!("cannot read {what}: {e}"))?;
        if k == 0 {
            return Err(format!(
                "truncated frame: {what} ended after {n} of {total} bytes"
            ));
        }
        n += k;
    }
    Ok(())
}

/// Stream exactly `n` more bytes into the file and make them durable;
/// fewer is a truncated frame. `have` is what already landed and `total`
/// what the frame claimed, so the refusal counts the whole part.
fn copy_exact(
    r: &mut impl Read,
    f: &mut File,
    n: u64,
    have: u64,
    total: u64,
    what: &str,
) -> Result<(), String> {
    let copied = io::copy(&mut r.by_ref().take(n), f)
        .map_err(|e| format!("cannot read {what}: {e}"))?;
    if copied != n {
        return Err(format!(
            "truncated frame: {what} ended after {} of {total} bytes",
            have + copied
        ));
    }
    f.sync_all().map_err(|e| format!("cannot sync {what}: {e}"))
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

/// read consumes: the smallest id above the cursor, then the cursor
/// moves to it. It takes nothing, because a verb whose effect depended
/// on whether an argument was present would hide a cursor move behind
/// output that looks the same either way.
fn cmd_read(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("read takes nothing; inspect one message with: beb peek ID".into());
    }
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    if !mb.dir.is_dir() {
        eprintln!("no new mail; cursor at 0");
        return Ok(());
    }
    // Consumption is serialized the way delivery always has been: choosing
    // the message, verifying it, printing it, and advancing the cursor all
    // happen under one lock. Without it two readers can choose the same id,
    // and a cursor read before another reader's write and set after it
    // moves the cursor backwards, handing a message out twice. The lock is
    // the reader's alone, so a slow stdout stalls other readers and never
    // a sender.
    let _lock = mb.read_lock()?;
    let cursor = mb.cursor();
    match mb.ids().into_iter().find(|&id| id > cursor) {
        None => {
            eprintln!("no new mail; cursor at {cursor}");
            Ok(())
        }
        Some(id) => {
            let (mut f, h) = check(&mb, id, &me)?;
            print_body(&mut f, h.body_offset)?;
            mb.set_cursor(id)
        }
    }
}

/// peek inspects: same verification, same bytes, and the cursor is
/// untouched. Looking at a message is not consuming it.
fn cmd_peek(args: &[String]) -> Result<(), String> {
    let arg = match args {
        [one] => one,
        _ => return Err("peek takes one id: beb peek ID".into()),
    };
    let id: u64 = arg
        .parse()
        .ok()
        .filter(|&n| n > 0)
        .ok_or_else(|| format!("not a message id: \"{arg}\""))?;
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    if !mb.ids().contains(&id) {
        return Err(format!("no message {id}; beb list --all shows what exists"));
    }
    let (mut f, h) = check(&mb, id, &me)?;
    print_body(&mut f, h.body_offset)
}

/// Edge-triggered: block until a message arrives after this call starts.
/// Standing unread mail is `list`'s question and does not return; a new
/// delivery id above the high-water mark at entry does. Arrival exits 0
/// printing nothing; `-t SECS` bounds the wait, and a timeout exits 1
/// silently, an expected outcome rather than a refusal.
fn cmd_wait(args: &[String]) -> Result<(), String> {
    let timeout = match args {
        [] => None,
        [flag, secs] if flag == "-t" => Some(
            secs.parse::<u64>()
                .map_err(|_| format!("not a number of seconds: \"{secs}\""))?,
        ),
        _ => return Err("wait takes -t SECS or nothing".into()),
    };
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    let messages = mb.messages();
    if !messages.is_dir() {
        return Err("no mailbox in the spool; beb init creates one".into());
    }
    // Baseline first, then arm: an arrival before the baseline is
    // standing mail (ignored on purpose), an arrival between baseline
    // and watch registration is caught by the loop's first scan, and an
    // arrival after registration is a kernel event. Sampled the other
    // way around, an arrival in the gap is absorbed into the baseline
    // and its wakeup is lost.
    let base = mb.ids().last().copied().unwrap_or(0);
    let watch = waitfs::DirWatch::new(&messages)
        .map_err(|e| format!("cannot watch the mailbox: {e}"))?;
    let deadline = timeout.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    loop {
        if mb.ids().into_iter().any(|id| id > base) {
            return Ok(());
        }
        let remaining = match deadline {
            None => None,
            Some(d) => {
                let now = std::time::Instant::now();
                if now >= d {
                    std::process::exit(1);
                }
                Some(d - now)
            }
        };
        // A signal bubbles up as Interrupted so the remaining time is
        // recomputed from the absolute deadline; retrying with the same
        // duration inside waitfs would let repeated signals stretch -t.
        match watch.wait(remaining) {
            Ok(true) => {}
            Ok(false) => std::process::exit(1),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("cannot wait on the mailbox: {e}")),
        }
    }
}

/// Everything a message must pass before a byte of it is printed:
/// envelope grammar, ed25519-only, recipient binding, signature. The
/// refusal always names the rm that turns the message into a gap, and the
/// caller's cursor is untouched because nothing has moved yet.
///
/// The message is opened once and the open file is what comes back, so the
/// headers, the bytes ssh-keygen verified, and the bytes printed are all
/// the one inode. Reopening a pathname after verifying it would leave the
/// claim "what is printed is what was verified" resting on the path still
/// meaning the same file.
fn check(mb: &Mailbox, id: u64, me: &Identity) -> Result<(File, Headers), String> {
    let mp = mb.message(id);
    let sp = mb.signature(id);
    let rm = format!("rm '{}' '{}'", mp.display(), sp.display());
    let mut f = File::open(&mp)
        .map_err(|e| format!("message {id} cannot be opened ({e}); {rm} to make it a gap"))?;
    let h = envelope::read_headers_from(&mut f)
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
    sshsig::verify(&mut f, &sp, &h.from.canonical(), &util::spool_root()?.join(".tmp"))
        .map_err(|e| format!("message {id} failed verification ({e}); {rm} to make it a gap"))?;
    Ok((f, h))
}

/// The body goes file -> stdout with io::copy; it never lands in memory
/// whole. The file is the one check() verified.
fn print_body(f: &mut File, offset: u64) -> Result<(), String> {
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("cannot seek: {e}"))?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    io::copy(f, &mut out).map_err(|e| format!("cannot print body: {e}"))?;
    out.flush().map_err(|e| format!("cannot print body: {e}"))
}
