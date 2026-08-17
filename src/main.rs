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

/// Every entry is a signature and then, indented under it, what the verb
/// does. A single aligned column cannot serve both `beb init` and `beb
/// send RECIPIENT --subject S [--body B]`: the shortest signature is
/// eight characters and the longest is forty-one, so an aligned
/// description either wraps for the long ones or strands the short ones
/// across half a screen. Wrapping is what it did, on three verbs out of
/// nine, which left the eye no rhythm to settle into. Options stay
/// inline where they belong, because the signature is the thing a reader
/// came to find.
const USAGE: &str = "\
beb {version} delivers signed messages between identities.

  beb init NAME
      a new identity in this directory, and a name resolving to it
  beb whoami
      your address, and the name that resolves to it here
  beb contacts
      every name this machine resolves, as known_signers lines

  beb send RECIPIENT --subject S [--body B]
      sign and deliver; the body comes from --body or stdin
  beb list (--unread | --after ID | --before ID) --limit N
      read-only, newest first. --unread is what you have not read;
      --after/--before exclude ID, take the N nearest it, and reach
      read mail. One of the three, and always a limit
  beb read
      the next unread message; moves the cursor past it
  beb peek ID
      one message by id; the cursor does not move
  beb wait [--from ID] [--timeout SECS]
      block until there is unread mail; prints the mark to wait from next

  beb pack RECIPIENT --subject S [--body B]
      sign one delivery onto stdout
  beb drop
      install one delivery from stdin

  beb --help
      this list
  beb --version
      the version alone

Exit: 0 did it, 1 change the command, 2 nothing to do, 3 refused.

BEB_IDENTITY names the directory holding the .beb to act as. Every verb
requires it except init, which never reads it and always writes here:

  export BEB_IDENTITY=/path/to/dir";

/// The usage text names its own version: help that cannot say which
/// binary printed it is help you have to go check.
fn usage() -> String {
    USAGE.replace("{version}", env!("CARGO_PKG_VERSION"))
}

/// What a verb failed at, and the number that says which kind. Prose on
/// stderr is the wrong carrier for the distinction: it is exactly the
/// line a caller filtering with `head` or discarding stderr is most
/// likely to have thrown away, and a reader that cannot tell an empty
/// mailbox from a message that failed verification has a security
/// failure rather than an inconvenience.
///
///     1  the invocation must change: bad verb, bad argument, no pin
///     2  the invocation was fine and there was nothing to do
///     3  beb refused: verification, wrong recipient, or a state change
///        that would have destroyed or duplicated something
///
/// String converts to 1, so every existing `?` keeps its meaning and
/// only the refusals and the empty outcomes are marked.
struct Fail {
    code: i32,
    msg: String,
}

impl From<String> for Fail {
    fn from(msg: String) -> Self {
        Fail { code: 1, msg }
    }
}

impl From<&str> for Fail {
    fn from(msg: &str) -> Self {
        Fail { code: 1, msg: msg.to_string() }
    }
}

/// 3: beb declined, and the decline is the point.
fn refused(msg: impl Into<String>) -> Fail {
    Fail { code: 3, msg: msg.into() }
}

/// 2: nothing was wrong and nothing was there.
fn nothing(msg: impl Into<String>) -> Fail {
    Fail { code: 2, msg: msg.into() }
}

/// Reading anything requires a mailbox somebody claimed here.
///
/// `read` used to write a cursor onto an unclaimed mailbox as a side
/// effect of consuming from it, which claimed the mailbox without ever
/// saying so and made the sixth spool guarantee false: a cursor was
/// supposed to exist if and only if an owner ran `init` here, and a
/// transport reads exactly that bit to decide what it may carry and
/// prune. A verb whose discipline is naming its own effect must not
/// acquire a second, silent one.
///
/// Claiming stays `init`'s, which costs one command and no longer costs
/// anything else: `init` adopts an identity already present without
/// touching its key.
fn claimed(mb: &Mailbox) -> Result<(), Fail> {
    if mb.claimed() {
        return Ok(());
    }
    Err(Fail::from(
        "no mailbox claimed here for this identity\n\
         beb init claims one, keeping the key that is already here",
    ))
}

/// Everything beb says about an artifact goes through here, one `beb: `
/// per line. The prefix is not decoration and not forensics. Callers
/// merge the streams in order to *filter* them -- `cmd | head` filters
/// stdout alone and lets stderr past unfiltered and out of place, so
/// `2>&1 |` is the normal way to read beb, not the exceptional one. The
/// prefix is what makes the merge reversible: `2>&1 | grep -v '^beb:'`
/// reconstructs stdout exactly. A property with one exception in it is
/// not a property anyone can lean on, so there are none; refusals carry
/// the prefix too, and every line of a multi-line one does.
///
/// stdout is flushed first, and that call is load-bearing. Rust's stdout
/// is line buffered, so whole-line output appears to order itself
/// correctly and a body with no trailing newline does not: the tail sits
/// in the buffer while an unbuffered stderr line overtakes it. Under
/// `2>&1` both are the same pipe, and the receipt lands ahead of the
/// artifact it describes.
fn note(msg: &str) {
    let _ = io::stdout().flush();
    let mut err = io::stderr().lock();
    for line in msg.lines() {
        // A blank line stays blank rather than becoming trailing
        // whitespace: the prefix is something callers grep and diff.
        let _ = if line.is_empty() {
            writeln!(err, "beb:")
        } else {
            writeln!(err, "beb: {line}")
        };
    }
}

fn main() {
    // The Rust runtime ignores SIGPIPE; restore the default so
    // `beb list | head` ends quietly instead of panicking on write.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("init") => cmd_init(&args[1..]),
        Some("send") => cmd_send(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        Some("read") => cmd_read(&args[1..]),
        Some("peek") => cmd_peek(&args[1..]),
        Some("wait") => cmd_wait(&args[1..]),
        Some("pack") => cmd_pack(&args[1..]),
        Some("drop") => cmd_drop(&args[1..]),
        // Renamed in 0.9.0. `receive` read as the recipient's act, and
        // it never was: the process running it is the sender's, reaching
        // across to put a delivery down. Its own help line already said
        // "install one delivery from stdin" rather than use its name.
        Some("receive") => Err(Fail::from(
            "there is no receive; the verb is drop, and it takes the same stdin",
        )),
        Some("whoami") => cmd_whoami(),
        Some("contacts") => cmd_contacts(&args[1..]),
        Some("--version") => {
            println!("beb {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        // Help you asked for is the artifact of asking, so it goes to
        // stdout unprefixed and exits 0. Help you did not ask for is a
        // refusal, and a refusal names the fix rather than being the
        // fix: nine lines of usage prefixed `beb: ` and buried in
        // stderr is worse documentation than one line pointing at the
        // command that prints them cleanly.
        // Bare `beb` is not a mistake the way a wrong verb is. A wrong
        // verb means you asked for something specific and missed; no
        // verb at all is the opening question, and the answer to it is
        // the list. Same stream, same exit code, same bytes as --help.
        Some("--help") | None => {
            println!("{}", usage());
            Ok(())
        }
        Some(v) => Err(Fail::from(format!("unknown verb \"{v}\"; beb --help lists the verbs"))),
    };
    if let Err(e) = result {
        note(&e.msg);
        std::process::exit(e.code);
    }
}

struct Identity {
    key: PublicKey,
    private_key: PathBuf,
    /// The directory this identity was resolved from, as `whoami` says
    /// it. The pin is machinery a process cannot see: a SessionStart
    /// hook writes BEB_IDENTITY into an environment file that is
    /// sourced before every command, so the agent signing with a key
    /// never watched anyone choose it. This is how it finds out.
    source: String,
}

/// An identity claim at a directory is either absent or broken. The
/// difference used to decide precedence between two claimants; with one
/// source it only decides which sentence the refusal is, and both
/// refuse.
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
    Ok(Identity {
        key,
        private_key,
        // Filled in by identity(), which is what knows how many claims
        // were on the table and which of them won.
        source: String::new(),
    })
}

/// One source: the directory named by BEB_IDENTITY. Nothing else, and
/// in particular not the working directory.
///
/// beb resolved the working directory's `.beb` until 0.5.3, which read
/// well for a person, who is somewhere, and badly for a program, which
/// is not. An agent moves between subdirectories, spawns shells and
/// hands work to subagents, and every one of those was a chance to sign
/// as somebody else, or as nobody, silently, in a tool whose entire
/// subject is who signed. Ambient identity that changes under a process
/// mid-task is not a convenience.
///
/// What replaces it is the same directory, read once, by whoever starts
/// the process: claude-beb pins the session's launch directory at
/// SessionStart and never on a directory change, direnv pins a shell,
/// an operator pins a command. Deciding it once at the boundary is a
/// thing a caller can be responsible for; re-deciding it per command
/// from wherever the process happened to wander is not.
fn identity() -> Result<Identity, Fail> {
    let dir = std::env::var_os("BEB_IDENTITY")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            Fail::from(
                "BEB_IDENTITY is not set, so there is no identity to sign as\n\
                 set it to a directory holding a .beb: export BEB_IDENTITY=/path/to/dir\n\
                 beb init makes one; it is the only verb that does not need the pin",
            )
        })?;
    // Absent is a 1: the pin points somewhere with no identity, and the
    // fix is to change the pin or run init. Broken is a 3: an identity is
    // claimed here and cannot be established, which is a refusal to be
    // anyone rather than a mistyped path.
    let mut id = identity_at(&dir).map_err(|e| match e {
        IdClaim::Absent => Fail::from(format!(
            "BEB_IDENTITY={p} has no .beb; make one with: (cd {p} && beb init)\n\
             or point BEB_IDENTITY at a directory that already has one",
            p = util::pretty_path(&dir)
        )),
        IdClaim::Broken(e) => refused(format!("BEB_IDENTITY={}: {e}", util::pretty_path(&dir))),
    })?;
    id.source = util::pretty_path(&dir);
    Ok(id)
}

/// Where the new identity stands relative to the pin, which is the one
/// thing `init` can say that nothing else will.
///
/// "a new session pins itself" was here until an agent reading beb cold
/// called it the most confusing line it met: it took "session" to mean
/// the next beb invocation, concluded one would pin itself from the
/// working directory, and ran the next verb unpinned. A session is a
/// harness's word and a harness is a thing beb neither owns nor can
/// explain. What a reader needs is that the variable governs every later
/// command, and what to type.
///
/// Making an identity while pinned elsewhere is legitimate -- a second
/// identity has to start somehow -- so it is said rather than refused.
/// Silently doing it would leave every later verb answering as the other
/// one.
fn pin_note() {
    match std::env::var_os("BEB_IDENTITY").filter(|v| !v.is_empty()) {
        None => note("every other verb needs BEB_IDENTITY set: export BEB_IDENTITY=$PWD"),
        Some(pin) => {
            let here = fs::canonicalize(".").ok();
            let there = fs::canonicalize(PathBuf::from(&pin)).ok();
            if here.is_some() && here == there {
                note("BEB_IDENTITY already points here, so every verb uses this identity");
            } else {
                note(&format!(
                    "BEB_IDENTITY points at {}, so other verbs still act as that identity\n\
                     export BEB_IDENTITY=$PWD to use this one instead",
                    util::pretty_path(Path::new(&pin))
                ));
            }
        }
    }
}

fn cmd_init(args: &[String]) -> Result<(), Fail> {
    // The name is required, and it is not the address: the key is. A
    // name is a local alias that resolves to one, and `send` takes
    // either -- a raw key works and always has. What the name buys is
    // that nobody has to type 68 characters of base64, and naming used
    // to be a second step done by hand: append a line to a file, in a
    // format you had to know, with the key pasted in. Every identity
    // made on this machine needed it eventually, so init takes it at the
    // one moment the key is in front of it. A machine holding many
    // identities is a building of rooms, and a room with no nameplate is
    // one you can only reach by knowing its exact coordinates.
    let name = match args.len() {
        1 => args[0].clone(),
        0 => {
            return Err(Fail::from(
                "init needs a name for this identity: beb init NAME\nthe address is the key; the name is what resolves to it here",
            ))
        }
        _ => {
            return Err(
                format!("init takes one name: beb init {}", args[0]).into()
            )
        }
    };
    if let Some(a) = name.strip_prefix('-') {
        let _ = a;
        return Err(format!("init takes a name, and there is no option \"{name}\"").into());
    }
    // `beb init alpha` used to mean "make an identity in ./alpha", and
    // silently wrote one here instead until 0.5.3. It means the name
    // now, so only an argument shaped like a path still needs the old
    // answer -- and it is still a cd, because init writes where it runs.
    if name.contains('/') {
        return Err(format!(
            "\"{name}\" reads as a directory, not a name; for an identity there: (cd {name} && beb init NAME)"
        )
        .into());
    }
    roster::validate_name(&name).map_err(Fail::from)?;

    // Read before anything is made. A name already spoken for is a
    // refusal, and every refusal init can speak comes before a key
    // exists on disk.
    let ks = util::known_signers_path()?;
    let roster_lines = roster::load(&ks);
    let taken = roster_lines.iter().find(|l| l.name == name);
    let already_taken = |l: &roster::Line| -> Fail {
        refused(format!(
            "\"{name}\" already names an identity, on line {} of {}\npick another name, or remove that line",
            l.lineno,
            util::pretty_path(&ks)
        ))
    };
    // The working directory, always. `BEB_IDENTITY` says which identity
    // to act as, and `init` does not act as one -- it makes one -- so
    // reading the pin here was a category error. It also cost four
    // successive readers the same question, which the help line could
    // not answer without getting longer: does the directory the pin
    // names have to exist already? Now it cannot be asked.
    let dir = PathBuf::from(".");
    let beb = dir.join(".beb");
    let shown = |p: &Path| -> String { p.strip_prefix(".").unwrap_or(p).display().to_string() };
    // Everything that can refuse, refuses before a key exists. A verb that
    // generates a keypair and then fails leaves a private key behind and a
    // directory that answers "already an identity" to the retry.
    //
    // An existing `.beb` is not automatically that refusal, though, and
    // treating it as one left a hole with no way out. A `.beb` carried to
    // a second machine has no mailbox in that machine's spool: `whoami`
    // answers, `list` prints nothing, `read` says "no new mail" as though
    // the mailbox were merely empty, and a delivery for that key is
    // refused with "its owner claims one with: beb init" -- advice that
    // `init` then answered with "rm -r .beb", which is to say, delete
    // your private key. The only followable instruction beb offered
    // destroyed the identity it was about.
    //
    // So the two things `init` makes are separate. A keypair is never
    // touched once it exists. A mailbox is claimed if nobody has claimed
    // it here, which is exactly the sixth spool guarantee read in the
    // one direction it had not been: a cursor means a reader lives here,
    // and its absence is a job to finish rather than a state to refuse.
    if beb.exists() {
        let me = identity_at(&dir).map_err(|e| match e {
            IdClaim::Absent => Fail::from("the identity vanished while init was reading it"),
            // A broken claim is never adopted. Writing a cursor for an
            // identity that cannot be established would claim a mailbox
            // for a key nobody can prove they hold.
            IdClaim::Broken(e) => refused(format!("{e}; fix or remove {}", shown(&beb))),
        })?;
        let spool = util::spool_root()?;
        let mb = Mailbox::of(&spool, &me.key.canonical());
        if mb.claimed() {
            return Err(refused(format!(
                "already an identity here, and its mailbox is claimed\n\
                 rm -r {} to start over",
                shown(&beb)
            )));
        }
        // The name may already be this identity's, which is what a
        // re-run of the same init looks like: not an error, and not a
        // second line saying the same thing twice.
        let named_already = match taken {
            Some(l) if l.key.as_ref().map(|k| k.canonical()) == Some(me.key.canonical()) => true,
            Some(l) => return Err(already_taken(l)),
            None => false,
        };
        mb.ensure()?;
        mb.set_cursor(0)?;
        // Mail addressed to this key before anybody here could read it
        // sits in the outbox, queued to leave. It should not leave now:
        // its recipient just moved in. A carrier that shipped it would
        // be carrying mail away from the machine that can deliver it.
        let taken = claim_from_outbox(&spool, &mb, &me.key.canonical())?;
        let waiting = mb.window_after(0, usize::MAX).len();
        if !named_already {
            roster::append(&ks, &name, &me.key.canonical())?;
        }
        println!("{}", me.key.canonical());
        if !named_already {
            note(&format!("named {name} in {}", util::pretty_path(&ks)));
        }
        note(&format!(
            "claimed mailbox {} in {} for the {} already here, cursor at 0",
            &key::mailbox_name(&me.key.canonical())[..8],
            util::pretty_path(&spool),
            shown(&beb)
        ));
        // Mail can predate the claim: another identity on this machine may
        // have sent to this key before anybody could read it, and those
        // messages are all unread the moment a cursor exists.
        if waiting > 0 {
            let from_outbox = if taken > 0 {
                format!(", {taken} taken back from the outbox")
            } else {
                String::new()
            };
            note(&format!(
                "{waiting} already waiting{from_outbox}; beb list shows them"
            ));
        }
        pin_note();
        return Ok(());
    }
    // No "is not a directory" refusal any more: the target is the
    // working directory, which exists by definition of being in it.

    // Nothing here holds a key yet, so a name already in the file cannot
    // be this identity's. Refused before ssh-keygen runs, like every
    // other refusal init speaks.
    if let Some(l) = taken {
        return Err(already_taken(l));
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
        ).into());
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
    // The name lands last, after the identity it names exists and its
    // mailbox is claimed, so a failure anywhere above leaves no line
    // pointing at a key that was never made.
    roster::append(&ks, &name, &canonical)?;
    // The address is the artifact and goes out alone, because it is the
    // same bytes `whoami` prints and lands in the same place: a
    // known_signers line on someone else's machine. Everything else init
    // has to say is said about it.
    println!("{canonical}");
    note(&format!(
        "created {}, mailbox {short} in {}, cursor at 0",
        shown(&private),
        util::pretty_path(&spool)
    ));
    note(&format!(
        "named {name} in {}; beb send {name} now resolves to that key",
        util::pretty_path(&ks)
    ));
    // The seam init has to name. Every other verb resolves the pin, and
    // the pin cannot have existed a moment ago, because there was no
    // identity for it to name. A harness that sets it does so at a
    // session boundary that has already passed: claude-beb pins the
    // launch directory at SessionStart and only where a `.beb` is
    // already there, so the session that runs init is precisely the
    // session that is not pinned to what it just made. Saying the export
    // here is the difference between an identity and a dead one.
    pin_note();
    // Not "name it here" -- init just did, and said so above. What this
    // address is still for is somebody else's machine, where the name
    // is theirs to pick: a roster is the reader's, so the name written
    // here is a local alias, never a fact about their file. The line
    // format is not repeated at this point either; `contacts` prints
    // lines in it, and resolve() prints one at the moment anybody needs
    // it, which is a name that did not resolve.
    // Not "the line above". The ack names two identifiers, a mailbox and
    // a key, so a reader working out what `send` wants does have to be
    // told which -- but "above" is a claim about layout, and layout is
    // the first thing to go. An agent harness that captures the two
    // streams separately and concatenates them reported this address
    // glued onto the end of the previous sentence, with nothing above
    // anything. Naming the verb that reprints it holds under any
    // arrangement of the output, which is the same reason `send` names
    // `beb pack` instead of describing a carrier.
    // No roster hint here. `init` is the one moment nobody has a
    // correspondent to name, so the line landed where it could not be
    // acted on; `read` carries it now, at the moment a reader is looking
    // at a sender it cannot name. What init does say is the name it just
    // wrote, because that line is the difference between an address
    // anybody can type and 68 characters of base64.
    note("beb whoami prints your address; give it to whoever should reach you");
    Ok(())
}

/// The address on stdout, because it is bytes bound for a
/// `known_signers` line and must stay exactly that. Which directory
/// answered goes to stderr, because a process does not otherwise get to
/// find out: the pin is written into an environment file by a hook that
/// ran before the session existed, and inherited through every subshell
/// and subagent after it. Signing as somebody you never saw chosen is
/// the failure this line exists to make visible.
fn cmd_whoami() -> Result<(), Fail> {
    let me = identity()?;
    // The address alone, and deliberately not `<name> <address>`. A
    // known_signers line is that shape and it is tempting to print one
    // here, but the address is a hash input, not only text: a mailbox
    // directory is sha256 of exactly these bytes, and beb-ssh computes
    // one by hashing what this prints. A name in front would silently
    // hash to a mailbox that does not exist. `contacts` prints the
    // pasteable line; this prints the thing the line is about.
    println!("{}", me.key.canonical());
    // The name is worth saying now, and was not before 0.8.0: until
    // `init` took one, beb usually had no name for you, so there was
    // nothing here to report.
    let named = util::known_signers_path()
        .ok()
        .map(|p| roster::load(&p))
        .and_then(|lines| roster::reverse(&lines, &me.key.canonical()).map(str::to_string));
    match named {
        Some(name) => note(&format!(
            "identity from BEB_IDENTITY={}, named {name} here",
            me.source
        )),
        None => note(&format!("identity from BEB_IDENTITY={}", me.source)),
    }
    Ok(())
}

/// contacts: every name this machine can resolve, in the file's own
/// format, so a line is copied rather than transcribed.
///
/// stdout is `<name> <key>` and nothing else -- no marker beside the
/// line that is this identity, however useful that would be to look at.
/// The whole value of printing roster lines is that they append to
/// somebody else's known_signers verbatim, and a trailing annotation
/// would make exactly one line in the output the one that cannot.
/// Which line is yours is a thing said about the list, so it is said on
/// stderr with everything else beb says.
///
/// Lines the parser cannot use are shown too, with the reason. A name
/// that silently vanished from a listing would be a name whose refusal
/// arrives later, at a send, with nothing to connect it to.
fn cmd_contacts(args: &[String]) -> Result<(), Fail> {
    if let Some(a) = args.first() {
        return Err(format!("contacts takes nothing: beb contacts (got \"{a}\")").into());
    }
    let me = identity()?;
    let path = util::known_signers_path()?;
    let lines = roster::load(&path);
    let pretty = util::pretty_path(&path);
    if lines.is_empty() {
        return Err(nothing(format!(
            "no names in {pretty}; beb init NAME writes one, and read names a sender you can add"
        )));
    }
    let mine = me.key.canonical();
    // Only usable names set the column. An unusable line is reported on
    // stderr rather than printed, so letting its name widen the rows
    // would indent every pasteable line to fit one that is not there.
    let width = lines
        .iter()
        .filter(|l| l.key.is_some())
        .map(|l| l.name.chars().count())
        .max()
        .unwrap_or(0);
    let usable = lines.iter().filter(|l| l.key.is_some()).count();
    let self_name = roster::reverse(&lines, &mine).map(str::to_string);
    match &self_name {
        Some(n) => note(&format!(
            "{usable} of {} names in {pretty}; {n} is this identity",
            lines.len()
        )),
        None => note(&format!(
            "{usable} of {} names in {pretty}; this identity is not among them",
            lines.len()
        )),
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for l in &lines {
        let pad = " ".repeat(width - l.name.chars().count());
        match (&l.key, &l.issue) {
            (Some(k), _) => writeln!(out, "{}{pad} {}", l.name, k.canonical()),
            // Said on stderr, because it is not a line anybody can paste.
            (None, issue) => {
                note(&format!(
                    "line {} is not usable ({}); it is refused by name when used",
                    l.lineno,
                    match issue {
                        Some(roster::Issue::Options) => "carries options".to_string(),
                        Some(roster::Issue::Wildcard) => "the name is a pattern".to_string(),
                        Some(roster::Issue::Comma) => "several principals".to_string(),
                        Some(roster::Issue::KeyType(t)) => format!("key type {t}"),
                        _ => "malformed".to_string(),
                    }
                ));
                Ok(())
            }
        }
        .map_err(|e| format!("cannot write: {e}"))?;
    }
    Ok(())
}

/// Short forms were removed in 0.6.0. beb is read and written mostly by
/// programs, which do not save keystrokes and do pay for ambiguity: `-t`
/// meant subject on `send` and timeout on `wait` for about an hour, and
/// `-b` is bcc in `mail`. One spelling per option, and a short form
/// somebody guessed anyway is answered with the one that works rather
/// than with "unknown option".
fn long_form(verb: &str, short: &str) -> Option<&'static str> {
    match (verb, short) {
        (_, "-s") => Some("--subject"),
        (_, "-b") => Some("--body"),
        ("list", "-f") => Some("--from"),
        ("list", "-n") => Some("--limit"),
        ("wait", "-t") => Some("--timeout"),
        _ => None,
    }
}

/// What `send` and `pack` were asked to send.
///
/// A subject and a body are two free strings of the same shape, and side
/// by side as positionals they are a swap waiting to happen: `beb send
/// alice "the migration needs review" "deploy blocked"` is a valid
/// command that means the wrong thing, and nothing about it looks
/// wrong. Naming them takes the order out of the question, and a second
/// bare argument becomes a refusal instead of whichever field it landed
/// beside.
struct Outgoing {
    recipient: String,
    subject: String,
    body: Option<String>,
}

/// Order-independent: flags may come before or after the recipient,
/// because a caller assembling a command should not have to remember a
/// sequence beb never had a reason to require.
fn parse_outgoing(args: &[String], verb: &str) -> Result<Outgoing, Fail> {
    let form = format!("beb {verb} RECIPIENT --subject S [--body B]");
    let mut recipient: Option<String> = None;
    let mut subject: Option<String> = None;
    let mut body: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let take = |what: &str, slot: &mut Option<String>| -> Result<(), Fail> {
            if slot.is_some() {
                return Err(format!("{verb} takes one {what}: {form}").into());
            }
            let v = args
                .get(i + 1)
                .ok_or_else(|| Fail::from(format!("{a} needs a value: {form}")))?;
            *slot = Some(v.clone());
            Ok(())
        };
        match a {
            "--subject" => {
                take("subject", &mut subject)?;
                i += 2;
            }
            "--body" => {
                take("body", &mut body)?;
                i += 2;
            }
            _ if a.starts_with('-') && a.len() > 1 => {
                return Err(match long_form(verb, a) {
                    Some(long) => format!("{verb} has no {a}; the option is {long}: {form}"),
                    None => format!("{verb} has no option \"{a}\": {form}"),
                }
                .into())
            }
            _ => {
                if let Some(first) = &recipient {
                    // An unquoted key splits into two bare arguments, and
                    // "one recipient" is a true sentence that names the
                    // wrong fix. The shape is recognisable, so say the
                    // thing that helps.
                    if key::looks_like_key_type(first) {
                        return Err(format!(
                            "a key is one argument; quote it: beb {verb} \"{first} AAAA...\" --subject S"
                        )
                        .into());
                    }
                    // Otherwise: the old positional shape, caught rather
                    // than silently reinterpreted.
                    return Err(format!(
                        "{verb} takes one recipient, and a subject and body are named\n{form}"
                    )
                    .into());
                }
                recipient = Some(args[i].clone());
                i += 1;
            }
        }
    }
    let recipient =
        recipient.ok_or_else(|| Fail::from(format!("{verb} needs a recipient: {form}")))?;
    let subject = subject.ok_or_else(|| {
        Fail::from(format!(
            "{verb} needs a subject: {form}\n\
             a subject is what beb list shows, so a reader can skip what they do not need"
        ))
    })?;
    envelope::validate_subject(&subject)?;
    Ok(Outgoing { recipient, subject, body })
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

fn cmd_send(args: &[String]) -> Result<(), Fail> {
    let me = identity()?;
    let out = parse_outgoing(args, "send")?;
    let ks_path = util::known_signers_path()?;
    let lines = roster::load(&ks_path);
    let (to, display) =
        resolve_recipient(&out.recipient, &lines, &util::pretty_path(&ks_path), "send")?;

    // Your own key needs no name to be recognizable, and printing 68
    // characters of base64 back at the sender is not recognition. Nor is
    // printing the key they just typed: an unnamed recipient is named by
    // its key's tail in the sentence, while the `beb pack` line below
    // keeps the whole key, because that one is a command to run.
    let full = display.clone();
    let display = if to.canonical() == me.key.canonical() {
        "you".to_string()
    } else if roster::reverse(&lines, &to.canonical()).is_none() {
        short_key(&to.canonical())
    } else {
        display
    };

    let spool = util::spool_root()?;
    let mb = Mailbox::of(&spool, &to.canonical());
    let tmp = util::scratch_dir(&spool.join(".tmp"), "send")?;
    // Where it lands is decided before it is signed, because the two
    // destinations are different places, not two states of one place: a
    // mailbox belongs to a reader on this machine, and the outbox holds
    // what has to leave it.
    let resident = mb.claimed();
    let result = write_signed_envelope(&me, &to, &out.subject, out.body.as_deref(), &tmp)
        .and_then(|(env, sig, body)| {
            spool::assemble(&env, &sig)
                .and_then(|f| {
                    if resident {
                        mb.deliver(&f)
                    } else {
                        spool::Outbox::at(&spool).put(&f, &key::mailbox_name(&to.canonical()))
                    }
                })
                .map(|id| (id, body))
        });
    let _ = fs::remove_dir_all(&tmp);
    let (id, body) = result?;

    // Nothing on stdout either way. A delivered id belongs to somebody
    // else's mailbox, which the sender cannot peek, read or prune. An
    // outbox id is this machine's, but a carrier discovers it from
    // `beb pickup` rather than by parsing a sentence. A number whose
    // reader cannot act on it is not an artifact.
    let _ = id;

    // Two outcomes wore one sentence until 0.5.3. Delivery to a mailbox
    // nobody has claimed here waits in a directory the recipient cannot
    // reach. The spool has always been able to tell them apart, by the
    // cursor `init` writes and delivery never does; the ack never asked.
    //
    // The unclaimed case says `beb pack` and not "a carrier". An agent
    // learns this tool from this tool, and "carrier" is the one word beb
    // would print that names something beb neither implements nor
    // defines: it belongs to whatever moves the bytes, which for beb-ssh
    // is beb-ssh's word. `pack` is in the help text with a description,
    // so it is a next step the reader can already look up.
    if resident {
        // The recipient is named once, and never in subject position. An
        // unnamed one displays as 68 characters of base64, and
        // "ssh-ed25519 AAAA... reads it here" was read by an agent as the
        // key itself doing the reading. "here" got a referent for the
        // same reason: it meant this machine, and nothing said so.
        //
        // A message to yourself needs no branch of its own: `display` is
        // already "you", and what happens to it is what happens to any
        // message landing in a mailbox somebody claimed here.
        note(&format!(
            "accepted for {display}; {body} bytes; it waits on this machine for beb read"
        ));
    } else {
        // Quoted when it is key text, because beb refuses an unquoted key
        // and a refusal is a poor thing to be told to type. The roster
        // name is preferred when there is one: shorter, and the reader
        // already chose it.
        let arg = if full.chars().any(char::is_whitespace) {
            format!("\"{full}\"")
        } else {
            full.clone()
        };
        let _ = &arg;
        note(&format!(
            "accepted for {display}; {body} bytes; nobody here reads it, so it waits in the outbox as {id}\n\
             a carrier takes it from there; nothing else on this machine will"
        ));
    }
    Ok(())
}

/// Construct and sign, touching no mailbox. The body streams through
/// disk: envelope tempfile under the spool root (same filesystem as the
/// mailbox, so delivery is a rename), never through a growing buffer.
/// The caller removes the tempdir on every path, success or refusal.
fn write_signed_envelope(
    me: &Identity,
    to: &PublicKey,
    subject: &str,
    body_arg: Option<&str>,
    tmp: &Path,
) -> Result<(PathBuf, PathBuf, u64), String> {
    let env_path = tmp.join("envelope");
    let body;
    {
        let mut f =
            util::private_file(&env_path).map_err(|e| format!("cannot write envelope: {e}"))?;
        let nonce = util::random_nonce()?;
        f.write_all(
            envelope::compose(
                &me.key.canonical(),
                &to.canonical(),
                &nonce,
                &util::rfc3339(util::now_secs()?),
                subject,
            )
            .as_bytes(),
        )
        .map_err(|e| format!("cannot write envelope: {e}"))?;
        if let Some(text) = body_arg {
            f.write_all(text.as_bytes())
                .map_err(|e| format!("cannot write body: {e}"))?;
            body = text.len() as u64;
        } else {
            body = io::copy(&mut io::stdin().lock(), &mut f)
                .map_err(|e| format!("cannot write body: {e}"))?;
        }
        // Not synced. This is a temp inside a scratch directory that is
        // removed on every path out, read back a moment later by
        // ssh-keygen and by the frame assembler, and referenced by
        // nothing after that. A crash here loses a send that had not
        // happened yet. Durability begins where the bytes become
        // visible, which is the rename in `place`.
        //
        // It is not a cheap line to keep: on macOS Rust's sync_all is
        // fcntl(F_FULLFSYNC), which waits for the drive to flush its
        // write cache -- 3.4ms measured here, against 0.05ms for the
        // fsync(2) that most software calls and believes is durable.
    }
    let sig_path = sshsig::sign(&me.private_key, &env_path)?;
    Ok((env_path, sig_path, body))
}

/// pack: construct -> sign -> frame on stdout. No mailbox, counter, or
/// cursor is touched anywhere; stdout is the product and success is
/// silent.
fn cmd_pack(args: &[String]) -> Result<(), Fail> {
    let me = identity()?;
    let out = parse_outgoing(args, "pack")?;
    let ks_path = util::known_signers_path()?;
    let lines = roster::load(&ks_path);
    let (to, display) =
        resolve_recipient(&out.recipient, &lines, &util::pretty_path(&ks_path), "pack")?;

    let spool = util::spool_root()?;
    let tmp = util::scratch_dir(&spool.join(".tmp"), "pack")?;
    let result =
        write_signed_envelope(&me, &to, &out.subject, out.body.as_deref(), &tmp).and_then(
            |(env, sig, _)| {
                let el = fs::metadata(&env).map_err(|e| format!("cannot stat envelope: {e}"))?.len();
                let sl = fs::metadata(&sig).map_err(|e| format!("cannot stat signature: {e}"))?.len();
                let stdout = io::stdout();
                let mut w = stdout.lock();
                frame::write_header(&mut w, el, sl)
                    .and_then(|_| io::copy(&mut File::open(&env)?, &mut w).map(|_| ()))
                    .and_then(|_| io::copy(&mut File::open(&sig)?, &mut w).map(|_| ()))
                    .and_then(|_| w.flush())
                    .map_err(|e| format!("cannot write the delivery: {e}"))?;
                // The header is a line, not a fixed width, so the
                // delivery size is measured rather than assumed.
                Ok(frame::header_len(el, sl) + el + sl)
            },
        );
    let _ = fs::remove_dir_all(&tmp);
    let bytes = result?;
    // pack was the one verb that said nothing. Its artifact goes to
    // stdout and almost always straight into a file, so a reader who
    // redirected it saw no output at all and had no way to tell a
    // delivery from an empty file without opening it. Every other verb
    // reports what it did; this one had the same duty and skipped it.
    note(&format!(
        "packed for {display}, \"{}\"; {bytes}-byte delivery",
        out.subject
    ));
    Ok(())
}

/// receive: one frame from stdin, verified before anything becomes
/// visible, installed through the same machinery as local delivery.
/// It resolves no identity: the delivery carries its own address, and
/// a mailbox that already exists here is what makes that address a
/// resident. Receiving is not reading, so nothing here needs a private
/// key.
fn cmd_drop(args: &[String]) -> Result<(), Fail> {
    if !args.is_empty() {
        return Err("receive takes nothing; the delivery arrives on stdin".into());
    }
    let spool = util::spool_root()?;
    let tmp = util::scratch_dir(&spool.join(".tmp"), "receive")?;
    let result = receive_one(&spool, &tmp);
    let _ = fs::remove_dir_all(&tmp);
    // Nothing on stdout. The id names a message in a mailbox this
    // process cannot open: `receive` resolves no identity, holds no key
    // and never reads, so the one caller that sees this output cannot
    // act on the number. beb-ssh proves it from the other side -- both
    // of its call sites inherit stdout and look only at the exit code --
    // and until 0.6.0 this prose went to stdout unprefixed, where
    // `2>&1 | grep -v '^beb:'` could not tell it from an artifact.
    let (delivered, h) = result?;
    let lines = util::known_signers_path().map(|p| roster::load(&p)).unwrap_or_default();
    let name = |k: &str| -> String {
        roster::reverse(&lines, k)
            .map(str::to_string)
            .unwrap_or_else(|| short_key(k))
    };
    let from = {
        let c = h.from.canonical();
        // The sender keeps its full key when it has no name: an operator
        // reading a transport's log is one `known_signers` line away from
        // naming it, and a tail is not bytes `send` accepts.
        roster::reverse(&lines, &c).map(str::to_string).unwrap_or(c)
    };
    match delivered {
        spool::Delivered::Fresh(id) => note(&format!(
            "accepted {id} for {}; from {from}, \"{}\"",
            name(&h.to.canonical()),
            h.subject
        )),
        // Still a success, and still exit 0: a transport that retries a
        // delivery it already made must not be told it failed.
        spool::Delivered::Already(id) => {
            note(&format!("already delivered as {id}; nothing added"))
        }
    }
    Ok(())
}

fn receive_one(spool: &Path, tmp: &Path) -> Result<(spool::Delivered, Headers), Fail> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    // The frame refuses an impossible signature length here, before a byte
    // of it is read.
    let (el, sl) = frame::read_header(&mut input).map_err(refused)?;

    // Admission runs on the header prefix, in memory, bounded by the same
    // limit the envelope grammar has always had. Nothing reaches disk
    // until the delivery has named a mailbox that exists here, so an
    // arbitrary stranger cannot spend the recipient's disk: the mailbox
    // check is no longer downstream of writing the body.
    let want = el.min(envelope::HEADER_MAX as u64) as usize;
    let mut prefix = vec![0u8; want];
    fill(&mut input, &mut prefix, el, "envelope")?;
    let h = envelope::parse_headers(&prefix)
        .map_err(|e| refused(format!("delivery is not a beb envelope ({e})")))?;
    if !h.from.is_ed25519() || !h.to.is_ed25519() {
        return Err(refused("envelope has a non-ed25519 key; beb speaks ssh-ed25519 only"));
    }
    // The envelope names its own mailbox, and an existing mailbox is
    // the only admission: an identity that has run `beb init` here
    // lives here, and one that has not cannot be conjured by anything
    // arriving from outside. So a delivery for a stranger is refused
    // rather than minting a mailbox nobody reads.
    // Claimed, not merely present. The directory's existence was the test
    // until 0.5.3, and a local `beb send` to a key that lives elsewhere
    // creates that directory: one outbound message to a stranger opened
    // this machine to unbounded inbound deliveries addressed to them. The
    // sixth guarantee says what was meant all along -- a cursor exists if
    // and only if an owner claimed the mailbox here with `init` -- and a
    // mailbox holding outbound mail for somebody else is exactly the case
    // that must not admit anything.
    let mailbox = Mailbox::of(spool, &h.to.canonical());
    if !mailbox.claimed() {
        return Err(refused(format!(
            "no mailbox here for {}; its owner claims one with: beb init",
            &key::mailbox_name(&h.to.canonical())[..8]
        )));
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
        return Err(refused("trailing bytes after the frame; one frame is one delivery"));
    }

    // Assembled before it is verified, and the assembled frame is what
    // gets stored -- so the bytes checked and the bytes kept are the same
    // file, not two arrangements of the same parts.
    let frame_path = spool::assemble(&env_path, &sig_path)?;
    let mut frame = fs::File::open(&frame_path)
        .map_err(|e| format!("cannot open the assembled frame: {e}"))?;
    let (env_len, sig_len) = crate::frame::read_header(&mut frame)?;
    let env_off = crate::frame::header_len(env_len, sig_len);
    sshsig::verify(
        &mut frame,
        env_off,
        env_len,
        env_off + env_len,
        sig_len,
        &h.from.canonical(),
        &spool.join(".tmp"),
    )
        .map_err(|e| refused(format!("signature verification failed ({e})")))?;

    // Idempotent over retained history, atomically: the dedup decision
    // and the insertion happen inside the mailbox lock, so concurrent
    // retries of the same delivery converge to one message.
    mailbox
        .deliver_once(&frame_path)
        .map(|d| (d, h))
        .map_err(Fail::from)
}

/// Fill the buffer from the stream; short is a truncated frame. `total` is
/// what the frame claimed for this part, so the refusal counts the part
/// rather than the read.
fn fill(r: &mut impl Read, buf: &mut [u8], total: u64, what: &str) -> Result<(), Fail> {
    let mut n = 0;
    while n < buf.len() {
        let k = r
            .read(&mut buf[n..])
            .map_err(|e| Fail::from(format!("cannot read {what}: {e}")))?;
        if k == 0 {
            return Err(refused(format!(
                "truncated frame: {what} ended after {n} of {total} bytes"
            )));
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
) -> Result<(), Fail> {
    let copied = io::copy(&mut r.by_ref().take(n), f)
        .map_err(|e| Fail::from(format!("cannot read {what}: {e}")))?;
    if copied != n {
        // A short frame is a refusal, not a disk problem. The sync below
        // is the disk problem, and stays a 1.
        return Err(refused(format!(
            "truncated frame: {what} ended after {} of {total} bytes",
            have + copied
        )));
    }
    f.sync_all()
        .map_err(|e| Fail::from(format!("cannot sync {what}: {e}")))
}

/// How long ago the sender says it sent, in the fewest characters that
/// still answer "is this stale". It is the sender's clock, so a clock
/// that is ahead shows `+2h` rather than being clamped to zero: skew is
/// worth seeing, and hiding it would let a wrong clock read as a right
/// one.
fn age(claimed: i64, now: i64) -> String {
    let d = now - claimed;
    let (sign, d) = if d < 0 { ("+", -d) } else { ("", d) };
    if d < 60 {
        return if sign.is_empty() { "now".into() } else { format!("+{d}s") };
    }
    let (n, unit) = match d {
        _ if d < 3600 => (d / 60, 'm'),
        _ if d < 86_400 => (d / 3600, 'h'),
        _ => (d / 86_400, 'd'),
    };
    format!("{sign}{n}{unit}")
}

fn cmd_list(args: &[String]) -> Result<(), Fail> {
    const FORM: &str = "beb list (--unread | --after ID | --before ID) --limit N";
    let mut after: Option<u64> = None;
    let mut before: Option<u64> = None;
    let mut unread = false;
    let mut count: Option<usize> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let val = |what: &str| -> Result<&String, Fail> {
            args.get(i + 1)
                .ok_or_else(|| Fail::from(format!("{a} needs {what}: {FORM}")))
        };
        // `--after 0` is legal and `--before 0` is not, which is the whole
        // difference between a boundary and a message. Ids start at 1, so
        // "after 0" is the only way to name the start of the mailbox: an
        // exclusive cursor can name every interior boundary and neither
        // end, and losing "from the beginning" is what switching from the
        // inclusive `--from` would otherwise cost.
        match a {
            "--after" => {
                let v = val("an id")?;
                after = Some(v.parse::<u64>().map_err(|_| {
                    Fail::from(format!("not a message id: \"{v}\""))
                })?);
                i += 2;
            }
            "--before" => {
                let v = val("an id")?;
                before = Some(
                    v.parse::<u64>()
                        .ok()
                        .filter(|&n| n > 0)
                        .ok_or_else(|| Fail::from(format!("not a message id: \"{v}\"")))?,
                );
                i += 2;
            }
            "--unread" => {
                unread = true;
                i += 1;
            }
            "--limit" => {
                let v = val("a count")?;
                // Zero used to mean "no limit", which is the one count a
                // caller can arrive at by arithmetic and the one that
                // returns the whole mailbox. Every other bad count is
                // refused; this one was obeyed.
                count = Some(
                    v.parse::<usize>()
                        .ok()
                        .filter(|&n| n > 0)
                        .ok_or_else(|| Fail::from(format!("not a count: \"{v}\"")))?,
                );
                i += 2;
            }
            // Both were spellings for a window, and both name what
            // replaced them rather than reporting an unknown option: a
            // caller typing either knows exactly what they want.
            "--all" => {
                return Err(format!(
                    "list has no --all; page with --after 0 --limit N, or ask for --unread\n{FORM}"
                )
                .into())
            }
            // `--from` was the inclusive forward-only cursor of 0.6.0. It
            // could not page backwards at all, and an agent reading the
            // help cold answered "the exact command cannot be determined"
            // to both digging questions.
            "--from" => {
                return Err(format!(
                    "list has no --from; --after ID pages forward and --before ID back\n{FORM}"
                )
                .into())
            }
            _ => {
                return Err(match long_form("list", a) {
                    Some(long) => format!("list has no {a}; the option is {long}: {FORM}"),
                    None => format!("list has no option \"{a}\": {FORM}"),
                }
                .into())
            }
        }
    }
    // One selector, and it has to be said.
    //
    // The default used to be "unread from the cursor", which is a
    // different *set* than --after and --before return, not a different
    // window on the same one: those two reach mail already read. A caller
    // who wrote neither got the unread set without asking for it, and a
    // caller who wrote one got read mail without being told. Naming it is
    // the whole fix.
    let selectors = usize::from(unread) + usize::from(after.is_some()) + usize::from(before.is_some());
    if selectors == 0 {
        return Err(format!(
            "list needs to say which: --unread, --after ID, or --before ID\n{FORM}"
        )
        .into());
    }
    if selectors > 1 {
        return Err(format!("list takes one of --unread, --after, --before: {FORM}").into());
    }
    // And how many. A page nobody asked for is a page nobody knows the
    // size of: an agent that never named a limit has no reason to look
    // for one, and reads ten rows of twenty-five as the whole mailbox.
    let limit = count.ok_or_else(|| Fail::from(format!("list needs --limit N: {FORM}")))?;

    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    claimed(&mb)?;
    let cursor = mb.cursor();

    // Both cursors are exclusive, so a caller pages by handing back an id
    // it was just shown: the last row to walk forward, the first row to
    // walk back. Nothing is ever computed, which is what makes gaps
    // harmless -- arithmetic on ids breaks the moment a carrier prunes.
    //
    // Rows always print oldest first, whichever end they were taken from.
    // The boundary chooses which rows, never their order, so a listing
    // reads the same way every time and in the same direction `read`
    // hands messages over.
    // Held ascending here whatever was asked for, because the hints and
    // the "is there more" stat are both about the ends of the window.
    // Only the printing is reversed.
    let shown: Vec<u64> = match (unread, after, before) {
        // The newest unread, not the oldest: a listing is read to find
        // out what has happened, and the oldest ten of twenty-five say
        // nothing about the one that just arrived. The cursor is the
        // floor, so the walk cannot fall into mail already read.
        (true, _, _) => mb.window_between(cursor, mb.high() + 1, limit),
        (_, Some(a), _) => mb.window_after(a, limit),
        (_, _, Some(b)) => mb.window_before(b, limit),
        _ => unreachable!("a selector was required above"),
    };

    // Four facts, the same four every time: where the cursor is, how much
    // the mailbox holds, how much of it is unread, and how much of that is
    // on the screen. The last is what makes a window safe to print -- a
    // paged listing that did not say it was paged would read as the whole,
    // and a reader would act on a tenth of its mail believing it had seen
    // all of it. It also answers "is there more": fewer rows than --limit
    // is the end of the walk.
    //
    // It goes to stderr, where everything beb says about an artifact goes,
    // so `beb list | wc -l` counts messages and not prose, and first
    // because a listing has no bound and a receipt behind an unbounded
    // artifact is the first thing a `head` throws away.
    // No totals. "N total, M unread" cost a full directory read and a
    // count of everything above the cursor -- 292ms on a 200k-message
    // mailbox, paid on every listing, to print two numbers nobody acts
    // on.
    //
    // But whether there is MORE has to stay, and in the header rather
    // than only in the hint below it: a window that does not say it is a
    // window reads as the whole, and an agent acts on a tenth of its
    // mail believing it has seen all of it. A consumer that carries one
    // line of what beb says -- claude-beb's drain carries the first --
    // would otherwise carry the part that cannot tell the difference.
    // It is one stat, not a count.
    // "more" is about the direction being walked, and says only that:
    // more rows that way. It used to read ", more waiting", which claims
    // unread mail -- true of the old default view and false the moment a
    // caller paged into read mail, where a fully-read mailbox would
    // answer "showing 0" to one command and "more waiting" to the next.
    // In the unread view "more" has to mean more *unread*: the rows stop
    // at the cursor, so asking whether anything exists below the window
    // answers with mail already read and offers to page into it.
    let more = match (unread, after) {
        (true, _) => shown
            .first()
            .is_some_and(|&f| !mb.window_between(cursor, f, 1).is_empty()),
        (_, Some(_)) => shown.last().is_some_and(|&l| mb.next_after(l).is_some()),
        _ => shown.first().is_some_and(|&f| mb.any_below(f)),
    };
    // What `read` would hand over is the one fact a listing cannot show
    // once it is ordered newest first: the next row to be consumed is at
    // the bottom, or off the page entirely. One stat, and omitted when
    // there is nothing to read.
    let next = mb.next_after(cursor);
    let header = format!(
        "showing {}{}; cursor at {cursor}{}",
        shown.len(),
        if more { ", more" } else { "" },
        match next {
            Some(r) => format!("; read next is {r}"),
            None => String::new(),
        }
    );
    if shown.is_empty() {
        // The header is the whole report, so it is the refusal's message
        // rather than a line printed before one. Exit 2: the command was
        // right and there was nothing to list.
        return Err(nothing(header));
    }
    note(&header);
    // Every other verb names the next step; this one printed a window and
    // said nothing about how to move it, even though the ids to move it
    // with were sitting in the rows. An agent paging cold inferred both
    // boundaries correctly and still reported that "actual output did not
    // tell me what to do next".
    //
    // Only when there is somewhere to go. A listing with nothing above and
    // nothing below is the whole mailbox, and an offer to page it would be
    // an offer to see the same rows again.
    let first = *shown.first().expect("shown is not empty here");
    let last = *shown.last().expect("shown is not empty here");
    // The hints carry --limit too, so following one is a paste and stays
    // as explicit as the command that produced it.
    //
    // The unread view offers only one direction, and only while there is
    // unread left in it: its top row is the newest message there is, and
    // an offer to page below the cursor is an offer to leave the set the
    // caller asked for.
    if unread {
        if more {
            note(&format!("older: beb list --before {first} --limit {limit}"));
        }
    } else {
        if mb.next_after(last).is_some() {
            note(&format!("newer: beb list --after {last} --limit {limit}"));
        }
        if mb.any_below(first) {
            note(&format!("older: beb list --before {first} --limit {limit}"));
        }
    }

    let lines = roster::load(&util::known_signers_path()?);
    let now = util::now_secs()?;
    let rows: Vec<(u64, String, String, String)> = shown
        .into_iter()
        .map(|id| match frame_headers(&mb, id) {
            Ok(h) => {
                let c = h.from.canonical();
                let sender = roster::reverse(&lines, &c)
                    .map(str::to_string)
                    .unwrap_or_else(|| short_key(&c));
                let when = util::parse_rfc3339(&h.date)
                    .map(|t| age(t, now))
                    .unwrap_or_else(|| "?".into());
                (id, when, h.subject, sender)
            }
            // A message beb cannot parse still gets a row: a listing that
            // silently skipped it would make a damaged message invisible
            // rather than visible and refusable.
            Err(_) => (id, "?".to_string(), "?".to_string(), "?".to_string()),
        })
        .collect();
    // Two padded columns, both bounded: the age is a handful of
    // characters and the subject is capped at 120. The sender is the only
    // unbounded field, so it stays last where it can run.
    let iw = rows.iter().map(|(i, _, _, _)| i.to_string().len()).max().unwrap_or(0);
    let aw = rows.iter().map(|(_, a, _, _)| a.chars().count()).max().unwrap_or(0);
    let tw = rows.iter().map(|(_, _, t, _)| t.chars().count()).max().unwrap_or(0);

    let stdout = io::stdout();
    let mut out = stdout.lock();
    // Newest first. The rows were gathered ascending because the window's
    // ends are what the hints and the "more" stat are about; only the
    // printing is reversed, so the thing that just arrived is the first
    // line read rather than the tenth.
    for (id, when, subject, sender) in rows.into_iter().rev() {
        let ap = " ".repeat(aw - when.chars().count());
        let tp = " ".repeat(tw - subject.chars().count());
        writeln!(out, "{id:>iw$}  {when}{ap}  {subject}{tp}  {sender}")
            .map_err(|e| format!("cannot write: {e}"))?;
    }
    Ok(())
}

/// Frames in the outbox addressed to a key that has just claimed a
/// mailbox here, moved into it.
///
/// Only `init` does this, and only when adopting: a freshly generated
/// key cannot have been written to. What it fixes is the window where a
/// `.beb` arrives after its mail did -- the mail was queued to leave
/// because nobody read here yet, and now somebody does.
fn claim_from_outbox(spool: &Path, mb: &Mailbox, mine: &str) -> Result<usize, String> {
    let ob = spool::Outbox::at(spool);
    let want = key::mailbox_name(mine);
    let mut taken = 0;
    for (_, to, path) in ob.entries() {
        if to != want {
            continue;
        }
        // deliver renames it in, so the outbox entry is consumed by the
        // move rather than deleted afterwards: one atomic step, and no
        // window where the frame exists in both places or neither.
        mb.deliver(&path)?;
        taken += 1;
    }
    if taken > 0 {
        util::fsync_dir(&ob.dir).map_err(|e| format!("cannot sync the outbox: {e}"))?;
    }
    Ok(taken)
}

/// How beb names a key nobody has named: the last eight characters of
/// its base64, elided. Scanning ten rows of the same 68-character key
/// buries the subjects the rows exist to show -- an agent reading beb
/// cold said it "dominated the output" -- while `read` keeps the key
/// whole, because that is where a reply gets composed.
///
/// A tail, not the mailbox hash this printed until 0.7.0. The hash named
/// the same correspondent in a second namespace nothing else beb prints
/// shares: a reader who saw `5629b03c` in a listing and the whole key
/// from `peek` had no way to tell they were one party without hashing it
/// themselves, and no way to tell two rows apart from two senders. A
/// tail is a substring of the string `read`, `peek` and `whoami` all
/// print, so the eye does the join.
///
/// A tail and not a head: the first 25 characters of every ed25519 key
/// are the algorithm name and the key length, identical for every signer
/// alive, so a leading elision distinguishes nobody.
///
/// The mailbox hash is still how beb names a mailbox as storage -- what
/// `init` claims, what `receive` refuses for. That is a directory, and a
/// directory is not a correspondent. The rule is the fallback: wherever
/// beb reaches past a roster name, it reaches for the key, because the
/// roster maps one to the other.
fn short_key(canonical: &str) -> String {
    match canonical.rsplit(' ').next() {
        Some(b64) if b64.len() > 8 => format!("...{}", &b64[b64.len() - 8..]),
        _ => canonical.to_string(),
    }
}

/// Who a message is from and what it says it is about, in the words the
/// reader already has: the roster name when there is one, and the key in
/// exactly the form `send` accepts when there is not.
///
/// The line to paste when a sender has no name, and nothing when it has
/// one. `init` carried this hint unconditionally until 0.6.0, at the one
/// moment nobody has a correspondent to name -- an agent reading beb
/// cold called it irrelevant to every task it had -- while the moment a
/// reader is actually looking at 68 characters of somebody's base64 said
/// nothing at all.
///
/// It is self-limiting, which is what makes repeating it honest: it
/// appears only while that sender is unnamed and stops the first time
/// anybody acts on it. The template itself is the one `init` used to
/// print, pointed at somebody else's key, which was its correct use.
fn name_hint(mb: &Mailbox, id: u64, h: &Headers) -> Option<String> {
    let lines = util::known_signers_path().ok().map(|p| roster::load(&p))?;
    let c = h.from.canonical();
    if roster::reverse(&lines, &c).is_some() {
        return None;
    }
    // Once per sender, not once per message. Self-limiting only limits a
    // reader who acts on it, and an agent draining five messages from one
    // unnamed sender got the same two lines five times and called them
    // noise. So the hint belongs to the earliest message from that sender
    // still in the mailbox: anything below this id from the same key means
    // the offer was already made. The scan stops at the first match, so a
    // sender that writes often costs one envelope read.
    // Walked backwards from just under this message, not forwards from
    // the start: the nearest earlier message from the same sender is the
    // one that decides, and it is usually one or two ids away. Walking
    // up from 1 meant reading every envelope in the mailbox to answer a
    // question about the last few.
    if (1..id)
        .rev()
        .any(|other| {
            frame_headers(&mb, other)
                .map(|prev| prev.from.canonical() == c)
                .unwrap_or(false)
        })
    {
        return None;
    }
    let ks = util::known_signers_path().ok()?;
    Some(format!(
        "that sender has no name here; append a line to {}:\n<name> {c}",
        util::pretty_path(&ks)
    ))
}

/// The claimed date in full rather than the age `list` shows: this is
/// the verb you reach for when looking closely at one message, and an
/// age is a summary of exactly the value printed here. A size is not
/// included, because by the time it could be read the body is already
/// on its way out and nothing can be done with the number.
fn describe(_mb: &Mailbox, id: u64, h: &Headers) -> String {
    let lines = util::known_signers_path()
        .map(|p| roster::load(&p))
        .unwrap_or_default();
    let c = h.from.canonical();
    let sender = roster::reverse(&lines, &c).map(str::to_string).unwrap_or(c);
    // The claimed instant, written the way somebody reads a clock. The
    // envelope keeps UTC and only UTC; this is display, and a receipt
    // is read by whoever is standing in front of it.
    let when = util::parse_rfc3339(&h.date)
        .map(util::local_stamp)
        .unwrap_or_else(|| h.date.clone());
    format!("{id} from {sender}, \"{}\", {when}", h.subject)
}

/// read takes the smallest id above the cursor, prints it, and moves
/// the cursor past it. Nothing is destroyed: there is no delete verb, and
/// the message stays where it is. The cursor
/// moves to it. It takes nothing, because a verb whose effect depended
/// on whether an argument was present would hide a cursor move behind
/// output that looks the same either way.
fn cmd_read(args: &[String]) -> Result<(), Fail> {
    if !args.is_empty() {
        return Err("read takes nothing; inspect one message with: beb peek ID".into());
    }
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    // Before the cursor is touched. Consuming from an unclaimed mailbox
    // wrote one as a side effect, which claimed the mailbox silently.
    claimed(&mb)?;
    // Consumption is serialized the way delivery always has been: choosing
    // the message, verifying it, printing it, and advancing the cursor all
    // happen under one lock. Without it two readers can choose the same id,
    // and a cursor read before another reader's write and set after it
    // moves the cursor backwards, handing a message out twice. The lock is
    // the reader's alone, so a slow stdout stalls other readers and never
    // a sender.
    let _lock = mb.read_lock()?;
    let cursor = mb.cursor();
    match mb.next_after(cursor) {
        None => {
            Err(nothing(format!("no new mail; cursor at {cursor}")))
        }
        Some(id) => {
            let (mut f, body_off, body_end, h) = check(&mb, id, &me)?;
            // Everything beb has to say comes before the body, and
            // nothing comes after it. A body is raw and usually does not
            // end in a newline, so a line written behind one is glued to
            // its last byte -- `...continuebeb: cursor 0 -> 1` -- and
            // `grep -v '^beb:'` cannot strip what is not at the start of
            // a line. The receipt would break the one property that
            // makes merging the streams safe.
            //
            // What it costs is that the cursor move is stated before the
            // write it depends on. The exit code carries that: the
            // cursor advances only after the body is out, so a failed
            // write is a non-zero exit and the receipt was a statement
            // of intent. Prose is the wrong carrier for "did it work"
            // anyway, which is what the code table is for.
            note(&format!("{}; cursor {cursor} -> {id}", describe(&mb, id, &h)));
            if let Some(hint) = name_hint(&mb, id, &h) {
                note(&hint);
            }
            print_body(&mut f, body_off, body_end)?;
            mb.set_cursor(id).map_err(Fail::from)
        }
    }
}

/// peek inspects: same verification, same bytes, and the cursor is
/// untouched. Looking at a message is not consuming it.
fn cmd_peek(args: &[String]) -> Result<(), Fail> {
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
    claimed(&mb)?;
    if !mb.has(id) {
        return Err(format!(
            "no message {id}; beb list --after 0 --limit 50 shows what exists"
        )
        .into());
    }
    let (mut f, body_off, body_end, h) = check(&mb, id, &me)?;
    // The whole difference between this verb and `read`, said out loud.
    // Their outputs are the same bytes and their effects are not, and
    // for as long as neither said so a reader had to know which was
    // which from somewhere else.
    let cursor = mb.cursor();
    note(&format!("{}; cursor stays at {cursor}", describe(&mb, id, &h)));
    if let Some(hint) = name_hint(&mb, id, &h) {
        note(&hint);
    }
    print_body(&mut f, body_off, body_end).map_err(Fail::from)
}

/// Block until there is a message at or above a mark. The mark is the
/// cursor unless `--from ID` names another, so plain `beb wait` means
/// "block until I have something to read" and returns at once if there
/// already is something.
///
/// It was edge-triggered until 0.6.0: the mark was the highest id
/// present when the call started, so mail already unread never woke it.
/// That edge was only meaningful inside a single call. `wait`
/// re-baselines on entry, so a message arriving between two calls sits
/// under the second one's high-water mark and wakes nothing, and every
/// real caller loops -- in legs, so a supervisor can stand it down --
/// which puts a gap at every leg boundary. claude-beb's doorbell patched
/// that by snapshotting `beb list` and ringing only when the listing was
/// non-empty and changed: ten lines of shell doing what a mark does.
///
/// So the mark is the caller's, the way `list --from` is, and the
/// default is the one mark beb already keeps for that reader. A worker
/// gets the obvious loop. A doorbell that must not re-ring for mail it
/// has already announced passes the id it rang for. beb holds no notion
/// of what anybody has been told, which was never its state to keep.
///
/// And it hands the mark back. `wait` had no artifact, so a caller that
/// needed one went `beb list --from 1 --limit 0 | tail -1 | awk` --
/// parsing a listing meant for people to recover a number beb already
/// had. stdout is now one line: the mark that means "everything I have
/// been told about is below this", which is the highest id present plus
/// one. It prints on a timeout too, because nothing arriving does not
/// make the mark less true, and that is what lets `--timeout 0` bootstrap
/// a waiter with no history:
///
///     m=$(beb wait --timeout 0)
///     while :; do
///         m=$(beb wait --from "$m" --timeout 900) && ring
///     done
fn cmd_wait(args: &[String]) -> Result<(), Fail> {
    const FORM: &str = "beb wait [--from ID] [--timeout SECS]";
    let mut timeout: Option<u64> = None;
    let mut from: Option<u64> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let val = || -> Result<&String, Fail> {
            args.get(i + 1)
                .ok_or_else(|| Fail::from(format!("{a} needs a value: {FORM}")))
        };
        match a {
            "--timeout" => {
                let v = val()?;
                timeout = Some(
                    v.parse::<u64>()
                        .map_err(|_| Fail::from(format!("not a number of seconds: \"{v}\"")))?,
                );
                i += 2;
            }
            "--from" => {
                let v = val()?;
                from = Some(
                    v.parse::<u64>()
                        .ok()
                        .filter(|&n| n > 0)
                        .ok_or_else(|| Fail::from(format!("not a message id: \"{v}\"")))?,
                );
                i += 2;
            }
            _ => {
                return Err(match long_form("wait", a) {
                    Some(long) => format!("wait has no {a}; the option is {long}: {FORM}"),
                    None => format!("wait has no option \"{a}\": {FORM}"),
                }
                .into())
            }
        }
    }
    let me = identity()?;
    let mb = Mailbox::of(&util::spool_root()?, &me.key.canonical());
    // The same claim every reading verb needs: waiting on a mailbox
    // nobody claimed here is waiting on somebody else's outbound mail.
    claimed(&mb)?;
    let messages = mb.msgs();

    // The watch is armed before the first look either way, so an arrival
    // in between is caught by the loop's first scan rather than falling
    // between the two.
    let mark = from.unwrap_or_else(|| mb.cursor() + 1);
    let watch = waitfs::DirWatch::new(&messages)
        .map_err(|e| format!("cannot watch the mailbox: {e}"))?;
    let deadline = timeout.map(|s| std::time::Instant::now() + std::time::Duration::from_secs(s));
    let mut blocked = false;
    loop {
        // The mark a caller passes back: one past everything assigned,
        // which the counter already knows. It is bounded and one line,
        // so it goes first, the way `init`'s address does.
        let next = mb.high() + 1;
        if mb.next_after(mark - 1).is_some() {
            println!("{next}");
            // The receipt names the number, because an unlabelled one is
            // worse than none. An agent reading beb cold called the bare
            // mark "worse than useless on its own": it had to remember a
            // phrase from --help to know what the digit meant, and then
            // could not work out why it was 3 when the message it had
            // just been told about was 2. Every other verb describes its
            // artifact beside it; this one printed a number and
            // explained nothing.
            note(&if blocked {
                format!("mail arrived; next mark {next}")
            } else {
                format!("mail is waiting; next mark {next}")
            });
            return Ok(());
        }
        let expired = || {
            // The mark on a timeout too. Nothing arriving does not make
            // it less true, and a caller that has to keep its own mark
            // across a timeout would otherwise have to remember the one
            // it sent in.
            println!("{next}");
            nothing(format!(
                "nothing arrived in {}s; cursor at {}; next mark {next}",
                timeout.unwrap_or(0),
                mb.cursor()
            ))
        };
        let remaining = match deadline {
            None => None,
            Some(d) => {
                let now = std::time::Instant::now();
                if now >= d {
                    return Err(expired());
                }
                Some(d - now)
            }
        };
        // A signal bubbles up as Interrupted so the remaining time is
        // recomputed from the absolute deadline; retrying with the same
        // duration inside waitfs would let repeated signals stretch the
        // timeout.
        match watch.wait(remaining) {
            Ok(true) => blocked = true,
            Ok(false) => return Err(expired()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("cannot wait on the mailbox: {e}").into()),
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
fn check(mb: &Mailbox, id: u64, me: &Identity) -> Result<(File, u64, u64, Headers), Fail> {
    let mp = mb.msg(id);
    let rm = format!("rm '{}'", mp.display());
    let (mut f, env_off, env_len, sig_len) = mb
        .open_frame(id)
        .map_err(|e| refused(format!("message {id} is not a beb frame ({e}); {rm} to make it a gap")))?;
    f.seek(SeekFrom::Start(env_off))
        .map_err(|e| format!("cannot seek: {e}"))?;
    let h = envelope::read_headers_from(&mut f)
        .map_err(|e| refused(format!("message {id} is not a beb envelope ({e}); {rm} to make it a gap")))?;

    if !h.from.is_ed25519() || !h.to.is_ed25519() {
        return Err(refused(format!(
            "message {id} has a non-ed25519 key in its envelope; {rm} to make it a gap"
        )));
    }
    if h.to.canonical() != me.key.canonical() {
        return Err(refused(format!(
            "message {id} is addressed to someone else; {rm} to make it a gap"
        )));
    }
    sshsig::verify(
        &mut f,
        env_off,
        env_len,
        env_off + env_len,
        sig_len,
        &h.from.canonical(),
        &util::spool_root()?.join(".tmp"),
    )
    .map_err(|e| refused(format!("message {id} failed verification ({e}); {rm} to make it a gap")))?;
    // Relative to the envelope, not the file: the header reader buffers,
    // so where the descriptor happens to sit afterwards is not where the
    // body starts. The envelope knows its own offset; the frame says
    // where the envelope begins.
    Ok((f, env_off + h.body_offset, env_off + env_len, h))
}

/// The envelope headers of a stored frame, for the verbs that describe a
/// message without opening it: `list`, and `peek`'s look at its
/// neighbours.
fn frame_headers(mb: &Mailbox, id: u64) -> Result<Headers, String> {
    let (mut f, env_off, _, _) = mb.open_frame(id)?;
    f.seek(SeekFrom::Start(env_off)).map_err(|e| format!("cannot seek: {e}"))?;
    envelope::read_headers_from(&mut f)
}

/// The body goes file -> stdout with io::copy; it never lands in memory
/// whole. The file is the one check() verified.
///
/// A body is raw and usually does not end in a newline, so the next
/// thing written anywhere runs into its last byte: an agent draining a
/// mailbox saw `Body 01beb: 2 from ...` and called it the worst defect
/// in the tool. stdout cannot carry the fix, because what is printed
/// there has to be the bytes that were signed and nothing else -- beb
/// carries binary bodies. So the line break goes to stderr, where
/// everything beb adds already goes. It is a separator rather than
/// speech, which is why it carries no `beb: `.
fn print_body(f: &mut File, offset: u64, end: u64) -> Result<(), String> {
    // The end is passed rather than measured: a body no longer runs to
    // the end of its file, because the signature sits behind it in the
    // same frame. Copying to EOF would print the signature too.
    let mut last = [0u8; 1];
    if end > offset {
        f.seek(SeekFrom::Start(end - 1))
            .map_err(|e| format!("cannot seek: {e}"))?;
        f.read_exact(&mut last)
            .map_err(|e| format!("cannot read body: {e}"))?;
    }
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("cannot seek: {e}"))?;
    let stdout = io::stdout();
    let mut out = stdout.lock();
    io::copy(&mut f.take(end - offset), &mut out).map_err(|e| format!("cannot print body: {e}"))?;
    out.flush().map_err(|e| format!("cannot print body: {e}"))?;
    if end > offset && last[0] != b'\n' {
        let _ = writeln!(io::stderr());
    }
    Ok(())
}
