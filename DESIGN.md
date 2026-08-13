# beb

beb delivers signed messages between identities.

An identity is an SSH key. A message is signed bytes. A mailbox is a
directory. Mail waits until read.

## Invariants

1. Identity is a public key.
2. The envelope is final: three headers, a blank line, the raw body.
   No version field; a different envelope is a different protocol.
3. Signatures are detached SSHSIG (`ssh-keygen -Y`), namespace `beb`.
   No homegrown crypto.
4. Reading requires the envelope's `to:` key to equal the reader's
   identity. A valid message for another identity is invalid here.
5. The stored message file is exactly the signed bytes.
6. Delivery ids are mailbox-local, monotonically increasing, and the
   only ordering. There are no timestamps.
7. Read state is one integer per mailbox, the cursor. `read`
   consumes, advancing the cursor one message through the front of
   the queue; `peek ID` inspects and moves nothing. A verb names its
   effect: nothing about consuming hides behind an argument.
8. There is no delete verb. Retention is local policy.
9. Receiving a message triggers nothing.

## Envelope

    from: ssh-ed25519 AAAA...
    to: ssh-ed25519 AAAA...
    nonce: <base64>

    <body bytes, raw>

These exact bytes are signed; the signature rests beside them, never
inside, so what is stored is what was signed and verification never
reconstructs anything.

The headers hold two keys and a base64 nonce, no free text. The keys
in `from:` and `to:` are ssh-ed25519; any other type is a different
protocol. The body is raw, uninterpreted, uncapped: it streams
through disk, so no implementation limit hides in memory. The nonce
is fresh per send, so the same words sent twice are two messages. A
sender time is a claim, and claims belong in the body.

## Identity

    private key = authority
    public key  = address

An identity lives in a directory:

    ~/project/backend/
    └── .beb/
        ├── .gitignore       # contains: *
        ├── id_ed25519
        └── id_ed25519.pub

Every command resolves identity the same way: the `.beb` of the
working directory, or the `.beb` of the directory named by
`BEB_IDENTITY`. The env var is for processes launched where cd is
not available (a launchd job, a supervisor's child): its value is
the directory you would have changed to, set deliberately at launch,
and children inheriting their parent's self through the environment
is capability the Unix way.

There is no precedence between the two. When both are present they
must agree, and agreement is judged by canonical public key, never
by path: two directories holding the same key are the same identity.
Disagreement is a refusal naming both fixes. A `BEB_IDENTITY`
pointing at a directory with no `.beb` is a refusal naming `beb
init`, never a fallback. No walk upward, nothing global, no default:
a process that has not been told who it is refuses to be anyone.

Scoping identity in a shell remains the shell's job:

    (cd ~/project/backend && beb send frontend "migration ready")
    BEB_IDENTITY=~/project/backend beb send frontend "migration ready"

One public key is one identity, wherever its `.beb` directory is
made available.

## Naming

Names live in one reader-owned file, in ssh's allowed_signers
format:

    ~/.config/beb/known_signers
      backend   ssh-ed25519 AAAA...
      frontend  ssh-ed25519 AAAA...

beb reads this file and never writes it. It honors a subset: one
literal principal, key type, base64, optional comment, blank lines,
`#` comments. Anything more (options, wildcards, comma-separated
principals, any key type but ssh-ed25519) is refused by name when
that name is used, never misparsed; such lines do not poison the
rest of the file.

Resolution is addressing and display only. Verification never
consults the roster; `read` verifies against the envelope's `from:`
key, so a stranger's mail verifies exactly like a named sender's.

A name listed with two keys is not a send target; the refusal names
both lines. An unknown name's refusal names the line to add.

## Spool

    ~/.local/share/beb/
    └── 9288c0759597cb39.../     # one mailbox per identity,
        ├── messages/            # named sha256 of its key text
        │   ├── 000000000000000001
        │   └── 000000000000000002
        ├── signatures/
        │   ├── 000000000000000001
        │   └── 000000000000000002
        └── cursor               # contains: 1

Guarantees, for anything that reads:

1. a mailbox directory is the lowercase hex sha256 of the public key
   text (`<type> <base64>`) it belongs to
2. a file under `messages/` is named by its delivery id
3. a visible file is whole
4. a visible file never changes
5. a visible message's signature is already beside it in `signatures/`

Only beb writes. How it writes is implementation, free to change: a
per-mailbox flock, counter first, then signature, then message, each
write-fsync-rename, so a crash leaves a gap in the numbering, never a
reused id, never a visible message without its signature. A send that
fails midway, out of disk included, may consume an id and leave a
stray signature: debris that is never a message, prunable like
anything stored. What cannot happen is a visible message without its
signature.

The cursor is the highest delivery id consumed. `list` shows beyond
it; consumption advances it, never backward. Next always means the
smallest present id above the cursor, never cursor plus one: gaps
are legal, and consumption steps over them without comment. Nothing
crosses the cursor that is not verified and addressed to this
identity; the stream advances only over messages that passed both
checks, or holes the owner made on purpose. One mailbox, one
reader.

Local filesystem only, never NFS: the flock and rename atomicity
this leans on are kernel guarantees network filesystems do not keep.

## Interface

    beb init                    key and mailbox from nothing
    beb whoami                  your address
    beb send RECIPIENT [BODY]   sign and deliver, body from argument or stdin
    beb list [--all]            what is waiting, unread by default
    beb read                    consume the next message
    beb peek ID                 inspect one message, consuming nothing
    beb wait [-t SECS]          block until the next message arrives
    beb pack RECIPIENT [BODY]   sign one delivery onto stdout
    beb receive                 install one delivery from stdin

The order is what a reader meets: who you are, then sending, then
reading, then the pair that crosses machines. A message is what
rests in a mailbox, a delivery is one in transit, and mail is the
mass noun for both.

`init` creates `./.beb`: an ed25519 keypair and a `.gitignore`
containing `*`, so the key cannot be committed. It refuses if
`./.beb` already exists, creates the mailbox, and prints the address
shaped for the file where names live; the one blank is never beb's
to fill.

    $ cd ~/project/backend && beb init
    created .beb/id_ed25519, mailbox 9288c075...
    your address: ssh-ed25519 AAAA...
    name it in ~/.config/beb/known_signers:
    <name> ssh-ed25519 AAAA...

`RECIPIENT` is a roster name or public key text. Key text is parsed
tolerantly (a trailing comment and whitespace are stripped, so a
pasted `.pub` line works as-is); the canonical two-field text is what
enters the envelope and names the mailbox. `send` signs, then
delivers into the recipient's mailbox, creating it if absent.

    echo "auth endpoint ready" | beb send backend

`list` prints one machine-stable line per message: the delivery id,
then the sender, as its roster name when known and the raw key
otherwise. Either form is exactly what `send` accepts, so a listed
sender is always addressable.

    beb list
    3  frontend
    4  ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...

`wait` blocks until a message arrives, then exits 0, printing
nothing. It is edge-triggered on purpose: mail already unread when it
starts does not return it, because "what stands unread" is `list`'s
question and `wait` answers only "when does something new land". The
two compose into any policy a reader wants, and neither implies the
other. `-t` bounds the wait in seconds; a timeout exits 1, silently,
an expected outcome rather than a refusal. `wait` is the spool's
watchability promise extended to processes that cannot hold a kernel
watch themselves (a shell hook, a script): the reader blocks by its
own choice, wake policy stays above beb, and receiving still triggers
nothing.

`read` consumes: it takes the smallest delivery id above the
cursor, verifies its signature, prints the raw body and nothing
else, and sets the cursor to that id. The cursor only ever advances
to the message just consumed, so skipping is impossible by
construction. An empty backlog says so and exits cleanly. It takes
no argument, and a stray one is a refusal naming peek.

`peek ID` inspects: same verification, same output, and the cursor
is untouched. Looking at a message is not consuming it. An id that
does not exist is a refusal. The two are separate verbs because
their outputs are identical and their effects are not: a cursor
move must never depend on whether an argument happened to be
typed.

A message that fails verification, or whose `to:` is not the
reader's identity, is refused before a byte is printed, the cursor
stays where it is, and the refusal names the fix: the exact `rm`
for that message and signature pair. Pruning turns the bad message
into a gap, gaps are legal, and the stream resumes on the next
read.

Every ack names the next step; every refusal names the fix. The CLI
is the documentation.

## Portable delivery

A message can leave the machine as an mbeb: the exact signed
envelope bytes and their detached signature, safely framed. `.mbeb`
is the conventional filename when one is persisted; the extension
has no semantic effect, is never part of the signed data, and
`receive` reads stdin as the sole authority. beb still never touches
a network: `pack` makes bytes, `receive` accepts bytes, and how they
travel — ssh, http, a pipe, a copied file — is the operator's
choice, owed nothing by beb.

    beb pack bob "the schema is ready" > note.mbeb
    beb pack bob < report.md | ssh host beb receive

The frame is lengths-then-bytes:

    beb <envelope byte count> <signature byte count>\n
    <envelope bytes><signature bytes>

then end of input. Nothing is delimited, so no body can collide with
a delimiter; there is no version field, because a different frame is
a different protocol; one frame is one delivery, and trailing bytes
are a refusal. The frame carries the two byte sequences and nothing
else: no host, no route, no time, no delivery id. Networking never
enters the signed bytes.

`pack` signs and does not deliver: normal identity resolution,
normal recipient resolution, the normal envelope, and no mailbox,
counter, or cursor is touched anywhere. Its stdout is the product
and its success is silent. Bodies stream through disk both ways;
nothing holds a body in memory.

`receive` installs into the mailbox the envelope names, and
resolves no identity of its own: the delivery already carries its
address, receiving is not reading, and nothing on this path needs a
private key. It is the same act as a local `send`, which has always
written into the recipient's mailbox rather than the sender's — the
address decides, never the directory the process happens to stand
in.

An existing mailbox is the whole admission. A delivery for a key
with no mailbox here is refused, naming `beb init`: running init is
what makes an identity live on this machine, and nothing arriving
from outside may conjure a mailbox nobody reads. That is the line
that keeps a carrier from filling a disk with mailboxes for
invented keys, and it is why the check is existence rather than
identity — the spool is the list of who lives here, kept by the
filesystem, needing no second register.

`receive` verifies before anything becomes visible: frame, envelope
grammar, ed25519 only, a mailbox that exists, signature, and only
then installs through the same lock, counter, write ordering, and
durability as local delivery. Failure at any step is fail-closed
and leaves nothing visible; as with any delivery, a failure after id
allocation may leave a gap, and gaps are legal.

Reading stays bound to identity, and that is where the boundary
lives: `read` and `list` resolve the identity under your feet and
`read` refuses a message whose `to:` is not that identity. A
mailbox may receive without a reader present; only its owner can
consume it.

An installed mbeb is an ordinary local message. `list`, `read`, and
`wait` cannot tell it crossed a machine; its delivery id is assigned
here, so the same message carries different ids on different
machines; the cursor does not move; nothing is woken by beb itself —
a watching runtime notices the arrival the way it notices any other.

The transport is untrusted: it may copy, delay, reorder, replay, or
inspect deliveries, and authentication comes solely from the signed
bytes. `receive` is idempotent over retained history: a delivery
whose exact envelope bytes are already present is accepted without a
second copy, and the ack names the existing id (`accepted <id>;
already delivered`) — so a store-and-forward carrier may retry
freely (the duplicate check and the insertion happen under the same
mailbox lock, so concurrent retries converge to one message), and
at-least-once transport becomes exactly-once mail for
as long as the original is retained. The mailbox remembers exactly
what it retains: a pruned message is a forgotten one, and a replay
after pruning installs anew. Identity is the envelope bytes alone —
the nonce makes deliberate repeats distinct messages, and the
signature stays outside, because any valid signature over the same
bytes is the same message.

## Out of scope

Moving bytes between machines (`pack` emits and `receive` accepts;
every carrier is the operator's choice), relays, admission,
deduplication, broadcast, presence, threads, wake policy. Whatever
comes later, the envelope and the reader guarantees do not change.

## Design test

Every proposed feature answers one question:

> Is this necessary to deliver an authenticated message from one key
> to another on this machine?
