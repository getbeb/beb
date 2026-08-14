# beb

beb delivers signed messages between identities.

An identity is an SSH key. A message is signed bytes. A mailbox is a
directory. Mail waits until read.

## Invariants

1. Identity is a public key.
2. The envelope is five headers, a blank line, the raw body. No
   version field; a different envelope is a different protocol, and
   0.6.0 is one: `date:` and `subject:` joined the grammar and every
   earlier beb refuses mail carrying them.
3. Signatures are detached SSHSIG (`ssh-keygen -Y`), namespace `beb`.
   No homegrown crypto.
4. Reading requires the envelope's `to:` key to equal the reader's
   identity. A valid message for another identity is invalid here.
5. The stored message file is exactly the signed bytes.
6. Delivery ids are mailbox-local, monotonically increasing, and the
   only ordering. `date:` is the sender's claim about when it sent,
   never a sort key: a clock that is wrong, or set wrong, would
   otherwise silently reorder a queue beb guarantees is ordered.
7. Read state is one integer per mailbox, the cursor. `read`
   prints the next unread message and moves the cursor past it;
   `peek ID` prints one by id and moves nothing. Neither destroys
   anything: there is no delete verb, and a message that has been read
   is still there to be peeked. A verb names its
   effect: nothing about consuming hides behind an argument.
8. There is no delete verb. Retention is local policy.
9. Receiving a message triggers nothing.

## Envelope

    from: ssh-ed25519 AAAA...
    to: ssh-ed25519 AAAA...
    nonce: <base64>
    date: 2026-08-15T02:26:34Z
    subject: what this is about

    <body bytes, raw>

These exact bytes are signed; the signature rests beside them, never
inside, so what is stored is what was signed and verification never
reconstructs anything.

The keys in `from:` and `to:` are ssh-ed25519; any other type is a
different protocol. The body is raw, uninterpreted, uncapped: it
streams through disk, so no implementation limit hides in memory. The
nonce is fresh per send, so the same words sent twice are two
messages. A sender time is a claim, and claims belong in the body.

`subject:` is a claim too, and it is in the envelope anyway, which
takes some explaining against the sentence before it. The difference
is not truth -- beb promises nothing about a subject beyond its being
signed -- but who the field is for. A time is for the reader, after
they have the message. A subject is for deciding whether to take the
message at all, and that decision happens in `list`, which has never
read a body byte and should not start: its rows are id and sender
today, both envelope facts. A subject in the body would mean `list`
interprets content, and would mean `beb read | ./handle-job` receives
a heading it did not ask for. The envelope keeps the body exactly the
bytes that were sent.

So the one header a sender writes freely is the one that needs a
grammar. A subject is required, non-empty, at most 120 bytes, and holds
no control characters. The cap is a refusal rather than a truncation,
because a sender who is cut off does not know it and a reader shown
half a claim cannot tell. The control-character rule is not
tidiness: a subject reaches a reader's terminal through `list` without
passing through a body, so an escape sequence in one is a sender
moving somebody else's cursor, repainting their line, or hiding what
it just claimed. It is refused on the way out and again on the way
in, so neither a local send nor an arriving delivery can carry one.

`date:` is the sender's clock and beb says so. It is stored RFC 3339
in UTC and nothing else -- a tolerant parser would accept a shape beb never
writes and read it as a fact, which is the one thing a signed claim
must not become. beb offers no time of its own because it has none
worth offering: a message file's mtime answers "when did this
arrive", and any careless `cp` or `rsync` rewrites it to now, which
`init` adopting a spool that moved makes an ordinary thing to do. A
claim labelled a claim stays honest when the clock behind it is not;
a fact that decays into a wrong answer while still looking like an
answer does not. Email reached the same split long ago, and Gmail
sorts by its own receipt rather than the sender's `Date:` for exactly
this reason -- beb has no receipt to sort by, so it sorts by id and
displays the claim as a claim.

What `read` and `peek` print is that same instant on the clock the
reader is looking at: `2026-08-15 09:26`. Storing UTC and displaying
local is the split every clock keeps, and the two halves have
different jobs. The wire value has to be comparable between machines,
so it carries a zone and a second. The screen value has to be
answerable by whoever is in front of it without arithmetic, so it
carries neither: an offset and a seconds field are precision for a
comparison the envelope already did, and on a receipt they are
characters that tell a reader nothing their own clock had not already
told them. The shape still sorts and still parses, which is all an
agent wanted from it either. `list` shows an age instead, which needs
no zone at all.

What a sender genuinely needs to assert about time still belongs in
the body, where a claim about the world sits next to the rest of the
message. `date:` is in the envelope for the same reason `subject:` is:
`list` decides whether a message is worth opening, and it has never
read a body byte.

Both come after `nonce:` so the routing prefix is untouched: beb-ssh
reads the first two lines to find a destination and treats the rest
as opaque, so a transport carries this mail without knowing either
word.

## Identity

    private key = authority
    public key  = address

An identity lives in a directory:

    ~/project/backend/
    └── .beb/
        ├── .gitignore       # contains: *
        ├── id_ed25519
        └── id_ed25519.pub

Every command but `init` resolves identity the same way: the `.beb`
of the directory named by `BEB_IDENTITY`. There is no second source.
No working directory, no walk upward, nothing global, no default: a
process that has not been told who it is refuses to be anyone.

    BEB_IDENTITY=~/project/backend beb send frontend "migration ready"

A `BEB_IDENTITY` naming a directory with no `.beb` is a refusal
naming `beb init`, and one naming a broken `.beb` is a refusal
keeping its own reason. An empty value is unset, never a fallback.
One public key is one identity, wherever its `.beb` directory is made
available, so the same directory copied to another path is the same
identity: it is judged by canonical public key, never by path.

beb read the working directory until 0.5.3, and that read well for a
person, who is somewhere. It reads badly for a program, which is not.
An agent moves between subdirectories, spawns shells and hands work
to subagents, and every one of those was a chance to sign as somebody
else, or as nobody, silently, in a tool whose entire subject is who
signed. Ambient identity that changes under a process mid-task is not
a convenience.

What replaces it is the same directory, read once, by whoever starts
the process. claude-beb pins a session's launch directory at
SessionStart and deliberately not on a directory change, writing it
to an environment file sourced before every command, so `cd` moves
the shell and not the signer. direnv pins a shell. An operator pins a
command. Deciding identity once, at a boundary, is something a caller
can be held responsible for; re-deciding it per command from wherever
the process has wandered is not.

That makes the pin invisible to the process it governs, which is
exactly why `whoami` names it. A hook wrote it before the session
existed and every subshell and subagent inherited it, so no part of
the running program ever watched the choice happen. The address stays
alone on stdout, because it is bytes bound for a `known_signers` line
and must stay exactly that:

    $ beb whoami
    ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...
    beb: identity from BEB_IDENTITY=~/project/backend

`init` is the exception, and never reads `BEB_IDENTITY` at all. It
writes to the working directory, full stop.

It read the pin from 0.5.2 until 0.6.0, so that the refusal every other
verb speaks -- "BEB_IDENTITY=/x has no .beb" -- could be answered
without changing directory. That was a category error: the pin says
which identity to act as, and `init` does not act as one, it makes one.
It also cost four successive readers the same question, which no help
line could answer without growing a clause: must the directory the pin
names already exist? Now it cannot be asked, the missing-directory
refusal is gone with it, and the refusal elsewhere names a `cd`, which
is followable by anything that can run a command.

Making an identity while pinned somewhere else stays legitimate -- a
second identity has to start somehow -- so `init` says where the new
one stands rather than refusing:

    beb: BEB_IDENTITY points at ~/work/backend, so other verbs still
         act as that identity
    beb: export BEB_IDENTITY=$PWD to use this one instead

and when the pin already names the directory being initialised it says
that instead, because the export would be telling a caller to set what
is already set.

`init` makes two things, and they are separate. A keypair is never
touched once it exists. A mailbox is claimed when nobody has claimed
it here.

beb never asks where a `.beb` came from, and could not answer if it
did. The question it asks is local and checkable: does this key have
a cursor in this spool? An identity carried to another machine
reaches that state, and so does a spool that was deleted, an
`XDG_DATA_HOME` pointed somewhere new, and a restore that brought
back the identity but not the mail. All of them want the same thing,
so none of them needs telling apart:

    $ beb init
    ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...
    beb: claimed mailbox 714889c0 in ~/.local/share/beb for the .beb
         already here, cursor at 0
    beb: 2 already waiting; beb list shows them

Mail can predate the claim, because another identity on this machine
may have sent to that key before anyone could read it, and all of it
is unread the moment a cursor exists. A broken `.beb` is never
adopted: writing a cursor for an identity that cannot be established
would claim a mailbox for a key nobody can prove they hold.

A copy of a `.beb` sitting beside its original on one machine is
refused, because one key is one mailbox and that mailbox is already
claimed. Two spools are a different matter: the same key claimed
under two `XDG_DATA_HOME`s has two mailboxes, two cursors and two id
sequences, which is what mailbox-local ids have always meant. beb
does not reconcile them and nothing here pretends to.

Treating any existing `.beb` as a refusal left a hole with no way
out. `whoami` answered, `list` printed nothing, `read` said "no new
mail" as though the mailbox were merely empty, and a delivery for
that key was refused with "its owner claims one with: beb init" --
which `init` answered with "rm -r .beb". The only followable
instruction beb offered destroyed the identity it was about.

That leaves a seam, and the seam is `init`'s to name. A harness pins
at a boundary that has already passed, and only where a `.beb` is
already there, so the session that runs `init` is precisely the
session not pinned to what it just built. An ack that stopped at the
address would hand back an identity nothing in that shell can use:

    $ beb init
    ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...
    beb: created .beb/id_ed25519, mailbox c97e8412 in
         ~/.local/share/beb, cursor at 0
    beb: every other verb needs BEB_IDENTITY set: export
         BEB_IDENTITY=$PWD
    beb: beb whoami prints your address; give it to whoever should
         reach you

The export line is absent when `init` was itself pinned, because then
it is the pin, and repeating it back says nothing.

`BEB_IDENTITY` was in this document and absent from the help text
when an agent asked for pinning to be built, which is how a
capability nobody can find turns out not to be there. The same was
true of `known_signers`, found by grepping strings out of the binary.
Both are named now where the question arises, which is what the third
clause of the interface rule means when it reaches things that were
never printed at all.

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

`read` and `peek` name it too, when the sender has none:

    beb: 1 from ssh-ed25519 AAAA..., "hello", 2026-08-15 04:26;
         cursor 0 -> 1
    beb: that sender has no name here; append a line to
         ~/.config/beb/known_signers:
    beb: <name> ssh-ed25519 AAAA...

`init` carried that hint until 0.6.0, which is the one moment nobody
has a correspondent to name: an agent reading beb cold called it
irrelevant to every task it had, while the moment a reader was staring
at 68 characters of somebody's base64 said nothing at all. Here it is
self-limiting, which is what makes repeating it honest -- it appears
only while that sender is unnamed and stops the first time anybody
acts on it -- and it carries the key, so the fix is a line to paste
rather than a rule to go and learn. The template is the one `init` used
to print, pointed at somebody else's key, which was its correct use.

An address is a public key, so it needs a channel you can
authenticate, never a private one. Where ssh already reaches the
other machine, the exchange is two one-liners:

    echo "pve $(ssh pve 'BEB_IDENTITY=~/work beb whoami')" >> ~/.config/beb/known_signers
    echo "mac $(beb whoami)" | ssh pve 'cat >> ~/.config/beb/known_signers'

Only first contact needs this. A reply never does: `list` prints an
unknown sender's key in exactly the form `send` accepts.

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
6. a mailbox has a `cursor` if and only if its owner claimed it here
   with `beb init`; a mailbox without one holds mail for a key that
   does not live on this machine. No other verb writes one: `read`
   advanced a cursor onto an unclaimed mailbox until 0.5.3, claiming
   it as a side effect of consuming, which made this guarantee false
   in exactly the case a transport depends on it. Reading now requires
   the claim instead of creating it

The sixth is what makes the spool answer a question it is never asked
directly: who lives here. `init` writes a cursor, delivery never does,
so a mailbox that has one has a reader and a mailbox that has none is
mail with nowhere local to go. That was true before it was written
down; it is a guarantee now because a transport reads it to decide
what to carry, and an implementation free to write a cursor at
delivery time would strand every outbound message in silence.

Only beb writes messages. How it writes them is implementation, free
to change: a per-mailbox flock, counter first, then signature, then
message, each write-fsync-rename, so a crash leaves a gap in the
numbering, never a reused id, never a visible message without its
signature. A send that fails midway, out of disk included, may consume
an id and leave a stray signature: debris that is never a message,
prunable like anything stored. What cannot happen is a visible
message without its signature.

Pruning is not writing, and it was never beb's. Retention is local
policy, gaps are legal, and every refusal beb speaks names the exact
`rm` that makes one. So a transport may prune on the operator's
behalf, and only where the sixth guarantee says no one local can
read: it removes what it has taken durable custody of, by id, in a
mailbox with no cursor. A mailbox with an owner is never touched by
anything but beb and that owner.

The cursor is the highest delivery id consumed. `list` shows beyond
it; consumption advances it, never backward. Next always means the
smallest present id above the cursor, never cursor plus one: gaps
are legal, and consumption steps over them without comment. Nothing
crosses the cursor that is not verified and addressed to this
identity; the stream advances only over messages that passed both
checks, or holes the owner made on purpose. One mailbox, one
reader.

One reader is a guarantee, not an assumption. Consumption takes an
exclusive lock and holds it across choosing the message, verifying
it, printing it, and advancing the cursor, the same rigor delivery
has always had. Unlocked, two readers can choose the same id, and a
cursor read before another reader's write and set after it moves
backwards, handing a message out twice; agents overlap by nature, so
the second reader is a hook or a supervisor, not an exotic case. The
reader's lock is a different file from the delivery lock, because
`read` holds it for as long as its stdout takes to drain: readers
wait for readers, and delivery waits for neither.

The spool holds plaintext bodies. beb authenticates and does not
encrypt, so confidentiality here is the filesystem's, and beb states
it rather than inheriting it: every directory beb makes is 0700 and
every file 0600, set at creation, not left to whatever umask the
process started under. Nothing repairs a spool made wide by
something else: beb states the modes of what it creates, and a
directory it did not create is the operator's.

Local filesystem only, never NFS: the flock and rename atomicity
this leans on are kernel guarantees network filesystems do not keep.

## Interface

    beb init
        an identity here, or in BEB_IDENTITY; the directory must exist
    beb whoami
        your address

    beb send RECIPIENT --subject S [--body B]
        sign and deliver; the body comes from --body or stdin
    beb list [--from ID] [--limit N]
        what is waiting, the next 10 by default
    beb read
        the next unread message; moves the cursor past it
    beb peek ID
        one message by id; the cursor does not move
    beb wait [--from ID] [--timeout SECS]
        block until there is unread mail; prints the mark to wait from next

    beb pack RECIPIENT --subject S [--body B]
        sign one delivery onto stdout
    beb receive
        install one delivery from stdin

    beb --help
        this list
    beb --version
        the version alone

`send` and `pack` name their subject and body rather than taking them
in order. Two free strings of the same shape side by side are a swap
waiting to happen: `beb send alice "the migration needs review"
"deploy blocked"` is a valid command that means the wrong thing, and
nothing about it looks wrong. Named, the order stops mattering, and a
second bare argument becomes a refusal instead of whichever field it
landed beside. The body still comes from stdin when `--body` is
absent, so `beb send alice --subject "nightly" <report.txt` is the
ordinary shape.

There are no short forms. beb is read and written mostly by programs,
which do not save keystrokes and do pay for ambiguity: `-t` meant
subject on `send` and timeout on `wait` for about an hour of 0.6.0's
development, and `-b` is bcc in `mail`. One spelling per option, and a
short form somebody guesses anyway is answered with the one that works
rather than with "unknown option".

Options stay inline in each signature rather than being hinted at and
looked up elsewhere, which is why the description sits on its own
indented line. A single aligned column cannot serve both `beb init`
and `beb send RECIPIENT --subject S [--body B]`: eight characters
against forty-one, so an aligned description either wraps for the long
signatures or strands the short ones across half a screen. It wrapped,
on three verbs of nine, and left the eye no rhythm to settle into.

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

`wait` blocks until there is a message at or above a mark, and returns
at once if there already is. The mark is the cursor unless `--from ID`
names another, so plain `beb wait` means "block until I have something
to read" and the obvious worker loop is correct:

    while beb wait; do beb read | ./handle-job; done

It was edge-triggered until 0.6.0, marking from the highest id present
when the call started, on the reasoning that "what stands unread" was
`list`'s question. That edge was only ever meaningful inside a single
call. `wait` re-marks on entry, so a message arriving between two calls
sits under the second one's high-water mark and wakes nothing, and
every real caller loops -- in legs, so a supervisor can stand it down
-- which put a gap at every leg boundary. claude-beb's doorbell patched
it by snapshotting `beb list` and ringing only when the listing was
non-empty and changed: ten lines of shell doing what a mark does.

So the mark is the caller's, the way `list --from` is, and the default
is the one mark beb already keeps for that reader. A waiter that must
not fire twice for the same mail -- a doorbell wakes a session, and a
session woken for mail it was already told about wakes forever -- names
its own instead. beb holds no notion of what anybody has been told,
which was never its state to keep.

`wait` hands that mark back. Its stdout is one line, the highest id
present plus one, which is what `--from` wants next time, and the
receipt names it: an unlabelled number is worse than none. An agent
reading beb cold called the bare mark "worse than useless on its own"
-- it had to recall a phrase from `--help` to know what the digit was,
and then could not work out why it read 3 when the message it had just
been told about was 2. Without it a
caller went `beb list --from 1 --limit 0 | tail -1 | awk`, parsing a
listing meant for people to recover a number beb already had. It prints
on a timeout too, because nothing arriving does not make the mark less
true, and that is what lets a waiter with no history start:

    m=$(beb wait --timeout 0)
    while :; do
        m=$(beb wait --from "$m" --timeout 900) && ring
    done

`--timeout` bounds the wait in seconds, and a timeout exits 2 -- an
expected outcome rather than a refusal -- naming the cursor, which is
the number a caller needs to ask anything else. `wait` is the spool's
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

What is printed is what was verified, and that holds on the
descriptor rather than on the name. Both verbs open the message
once and read its headers, hand that open file to the verifier, and
print the body from the same handle. A pathname resolved a second
time after verification would make the claim depend on the path
still meaning the same file.

Every ack names the next step; every refusal names the fix; every ack
names what changed. The CLI is the documentation.

The third clause is the one that was missing, and it was found by an
agent onboarding onto beb for the first time. The verbs that read well
were the additive ones, where naming the next step was the whole
story. The verbs that confused were the ones that changed state you
could not see: `send` did not say the mail stayed here, `peek` did not
say the cursor had not moved, `list` did not say which side of the
cursor you were looking at, and `read` on an empty body looked exactly
like `read` on an empty mailbox. Five reports, one absent sentence.

So each verb says what it did to the spool:

    beb init              created .beb/id_ed25519, mailbox c97e8412
                          in ~/.local/share/beb, cursor at 0
    beb pack bob          packed for bob, "deploy blocked";
                          574-byte delivery
    beb receive           accepted 4 for backend; from alice,
                          "deploy blocked"
    beb receive           already delivered as 4; nothing added
    beb send alice        accepted for alice; 22 bytes; alice reads
                          it here
    beb send bob          accepted for bob; 22 bytes; nobody here
                          reads it
                          beb pack bob writes a delivery you can
                          carry to that machine
    beb list              cursor at 3; 5 total, 2 unread; showing 2
                          4  now  deploy blocked   alice
                          5  2h   schema question  frontend
    beb list --limit 0    cursor at 3; 5 total, 2 unread; showing 5
    beb read              4 from alice, "deploy blocked",
                          2026-08-15 09:26; cursor 3 -> 4
    beb peek 5            5 from alice, "schema question",
                          2026-08-15 08:04; cursor stays at 3
    beb wait              mail arrived; 2 unread; next mark 8
    beb wait              2 unread; next mark 8
    beb wait --timeout 30 nothing arrived in 30s; cursor at 7;
                          next mark 8

`send` tells the two cases apart with the sixth spool guarantee and
nothing else: a mailbox with a cursor has a reader here, one without
does not. It says "nobody here reads it" rather than "bob lives
elsewhere", because the second is a claim about bob and beb has no
way to know it. What it knows is that nobody ran `init` for that key
on this machine.

The unclaimed case names `beb pack`, and this is a constraint on
every word beb prints. Whoever reads this output learns the tool from
the tool: the help text is the only place the question gets asked, so
a word that appears nowhere but one receipt is a word with no
definition anywhere. An earlier draft said "waits for a carrier",
which named a thing beb neither implements nor defines and which
belongs to whatever moves the bytes, beb-ssh's word rather than
beb's. `pack` is a verb in the help text with a description beside
it. The line is printed ready to run, key text quoted, because beb
refuses an unquoted key and advice a tool would reject is not advice.

Neither case puts anything on stdout. The delivery exists and has an
id, but that id belongs to somebody else's mailbox, and every verb
that could act on it -- `peek`, `read`, the `rm` a refusal names --
works on the mailbox of the identity running it. The sender cannot
reach the number they were handed.

Nor is a transport the missing consumer. beb-ssh's `carry` finds work
by watching for a message in a mailbox with no cursor, so it learns
ids by listing a directory and never by reading this stdout. A number
whose only reader cannot act on it is not an artifact, and a verb with
nothing to capture writes nothing.

The byte count is there because an empty body is a legal message.
Without it a heredoc that never arrived and one that did are the same
sentence.

A message to yourself is its own case, and says so. It is a real
thing to want, being the only note an agent can leave that outlives
the context it was written in, and `beb read` collects it like any
other. What it must not do is look like an accident, which is what
68 characters of your own base64 in place of a name looked like.

`list` prints its header always, including over an empty listing. Conditional output costs a consumer more than a
redundant line: absence would be ambiguous between an old build, a
plain listing and a truncated read, while a stated cursor plus
monotonic ids make read state computable rather than marked. Nothing
in a listing needs a marker once the cursor is a number the reader
has, and without the header nothing was computable at all: a full
listing printed identical bytes before and after a read.

A row is `id  age  subject  sender`. The age is how long ago the sender
says it sent, and a clock that is ahead shows `+2h` rather than being
clamped to zero, because skew is worth seeing and hiding it would let
a wrong clock read as a right one. The subject comes before the sender,
which inverts the usual order for a reason the usual order does not
have to deal with: an unnamed sender displays as 68 characters of
base64, in exactly the form `send` accepts, and a column of those
pushes every subject off the screen. The subject is capped, so it is the
field that can be a column; the sender is unbounded, so it goes last
where it can run. A message beb cannot parse still gets a row, with
`?` in both fields, because a listing that skipped it would make a
damaged message invisible rather than visible and refusable.

The header is prose, so it goes to stderr: the rows are the artifact,
and `beb list | wc -l` has to count messages. It goes ahead of them,
because a listing has no bound and a receipt behind an unbounded
artifact is the first thing a `head` or a display limit discards.


An empty listing exits 2. The command was right and there was nothing
to list, which is the same answer `read` gives an empty mailbox, and
the header is the whole report rather than a line printed before one.

`list` is paged, because a mailbox is unbounded and an agent's context
is not: an unpaged listing is a flood waiting for the one morning
nobody was reading. Ten rows by default, `--limit` for another count, `--limit 0` for no
limit at all.

The window runs forward from the cursor, which is the direction `read`
consumes in. A listing of the newest messages would show a tail while
`read` handed over the head, and the row an agent acted on would not
be one it had seen. `--from` names a different start, and then
messages already read are in range, because an explicit id is a
request rather than a filter.

`--from` takes an id and not an offset, and the difference is not
cosmetic. An offset names the nth row of whatever the set happens to
be, and this set changes under the reader: mail arrives while they
page, and pruning is legal, so the twenty-first row is a different
message before and after either. An id is the same message forever.
A reader pages by the last id it saw plus one, and cannot re-read a
row or step over one.

The header carries four facts and the same four every time: where the
cursor is, how much the mailbox holds, how much is unread, and how
much of that reached the screen. The last is what makes a window safe
to print. A paged listing that did not say it was paged would read as
the whole, and a reader would act on a tenth of its mail believing it
had seen all of it.

## Where each line goes

    stdout   the artifact, when there is one
    stderr   everything said about it

A body, a frame, a listing, an address and a mark are artifacts; the
sentence said about one is not.

`pack` was the last verb to say nothing at all. Its artifact goes to
stdout and almost always straight into a file, so a reader who
redirected it saw no output and could not tell a delivery from an
empty file without opening it -- an agent reading beb cold asked for
"a concise diagnostic on stderr" instead of the silence. The size it
reports is the whole frame, header included, measured rather than
assumed, because the header is a line and not a fixed width.

`receive` learned it just before. Its ack went to stdout,
unprefixed, until 0.6.0 -- and a transport is exactly the caller that
cannot cope with that, since beb-ssh runs `beb receive` with stdout
inherited, so beb's prose landed in the middle of the transport's own
output with nothing to filter it by. The id it printed was no use
either: `receive` resolves no identity, holds no key and never reads,
so the process running it cannot open the message it just named. Both
of beb-ssh's call sites look only at the exit code, which is the whole
answer a caller needs, and a replay stays exit 0 because a transport
retrying a delivery it already made must not be told it failed. stdout carries the
value a caller would capture, stderr carries the prose, and a verb
with nothing to capture writes nothing to stdout at all.

`init` is the smallest case and sets the shape. Its artifact is the
address, the same bytes `whoami` prints, bound for a `known_signers`
line on somebody else's machine:

    $ beb init
    ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA...
    beb: created .beb/id_ed25519, mailbox c97e8412 in
         ~/.local/share/beb, cursor at 0
    beb: every other verb needs BEB_IDENTITY set: export
         BEB_IDENTITY=$PWD
    beb: beb whoami prints your address; give it to whoever should
         reach you
    beb: name others in ~/.config/beb/known_signers

It named the roster template until 0.5.3, printing `<name> ssh-ed25519
AAAA...` under the path of the reader's own `known_signers`. The
roster is reader-owned, so a key in its owner's own roster does
nothing, and the file that address belongs in is the correspondent's.
Followed literally the line accomplished nothing, and the format it
taught is already printed at the one moment anybody asks for it, which
is a name that failed to resolve.

That split holds only while the streams are separate, and callers
merge them constantly. The reason they merge is not that stderr would
be lost otherwise: `beb list | head` filters stdout alone and lets
stderr past unfiltered and out of place. They merge in order to filter
both at once, which makes a merge something beb has to keep reversible
rather than something it has to survive.

The prefix is what makes it reversible. Every line beb writes to
stderr begins `beb: `, so `2>&1 | grep -v '^beb:'` is exactly stdout
and `grep '^beb:'` is exactly the prose. A property with one exception
in it is not one anybody can lean on, so there are none: refusals
carry the prefix, and every line of a multi-line refusal carries it.
It is still not proof against a body whose own line begins `beb: `. A
caller who needs certainty keeps the streams apart, and that is the
documented answer.

`--help` is the one place that could have looked like an exception and
is not. Usage is the artifact of asking for usage, so it goes to
stdout, unprefixed, exit 0, and `beb --help | grep send` behaves like
any other listing. Usage nobody asked for is a different thing: an
unknown verb printed the whole list under its refusal until 0.5.3,
which under the prefix rule is nine lines of `beb: ` in stderr, and a
refusal names the fix rather than being the fix. So a non-verb is one
line:

    $ beb frobnicate
    beb: unknown verb "frobnicate"; beb --help lists the verbs

which is what `beb help` and `beb -h` get too, self-correcting in one
hop rather than earning aliases.

Bare `beb` is not that. A wrong verb means you asked for something
specific and missed; no verb at all is the opening question, and the
answer to it is the list. So it prints byte for byte what `--help`
prints, on stdout, exit 0. The cost is that `beb $verb` with `verb`
unset succeeds quietly and hands a caller the usage text where it
expected a result, which is the price of every tool that answers a
bare invocation, and cheaper than making the first command anyone
types a failure.

The list names `--help` and
`--version` among the verbs for the same reason `known_signers` and
`BEB_IDENTITY` are named where they are asked about: a capability
nobody can find is a capability that is not there.

Order under a merge is beb's job too, and one call does it. Rust's
stdout is line buffered and stderr is unbuffered, so whole lines
appear to order themselves correctly and a body with no trailing
newline does not: its tail waits in the buffer while the stderr line
overtakes it, and under `2>&1` both are the same pipe. So stdout is
flushed before anything is written to stderr. Without that flush the
receipt arrives ahead of the artifact it describes, which is the
ordinary behaviour of programs that do not bother and the reason
merged output so often reads out of sequence.

Position within a merge is decided by what truncation destroys, since
a reader's view of a flood is bounded and the bound falls at the end.
Where the artifact is bounded, as an address or an id is, order is
free and the artifact goes first, because it is what a `head -1`
should catch. Where it is unbounded, as a body is, the receipt goes in
front of it: behind it, it is the first thing a `head` or a display
limit throws away, and in front it sits at a known offset where
stripping it is arithmetic rather than a backwards scan through
arbitrary bytes for a boundary that binary content can forge. Nothing
is ever written behind an artifact. A body is raw and usually does not
end in a newline, so a line printed after one is glued to its last
byte -- `...continuebeb: cursor 0 -> 1` -- and `grep -v '^beb:'`
cannot strip what does not begin a line. A trailing receipt would
break the one property that makes merging safe, to confirm something
the exit code already carries. So beb says everything it has to say
and then hands over the artifact, and `read` states the cursor move
before the write it depends on: the cursor advances only after the
body is out, so a failed write is a non-zero exit and the receipt was
a statement of intent.

## What an exit code means

    0   did the thing
    1   change the invocation: unknown verb, bad argument, no pin
    2   nothing to do: no mail, timed out, nothing waiting
    3   refused: verification, wrong recipient, bad envelope, or a
        state change that would destroy or duplicate something

3 never collapses into 2. A reader that cannot tell an empty mailbox
from a message that failed verification has a security failure rather
than an inconvenience, and prose on stderr is the wrong carrier for
that distinction, because it is exactly the line a caller is most
likely to have discarded. With this table a program never needs to
parse a word beb prints.

3 covers more than verification, because the line that matters is not
"cryptography failed" but "beb declined, and the declining is the
answer". `already an identity` is a refusal to overwrite a keypair;
`no mailbox here` is a refusal to mint one for a stranger; a broken
`.beb` under the pin is a refusal to be anyone. None of those are the
caller mistyping something, which is what 1 means, and all of them are
states a program should notice rather than retry.

The boundary between 1 and 3 is who has to change. 1 says change the
command or the environment and try again: an unknown verb, a stray
argument, an unset pin, a pin at a directory with no `.beb`. 3 says
the command was right and beb still said no. A truncated frame is 3
and the disk error while writing that frame is 1, because one is a
delivery beb rejected and the other is a machine that failed.

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

The envelope count is uncapped, because a body is, and it streams
through disk either way. The signature count is not: an armored
ed25519 SSHSIG is under 300 bytes, so a claim of gigabytes is not a
signature, and the frame says so before reading any of it. That
bound is arithmetic about the format rather than a policy knob, so
it needs no setting to tune and no environment variable to forget.

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

A claimed mailbox is the whole admission. A delivery for a key
nobody has claimed here is refused, naming `beb init`: running init
is what makes an identity live on this machine, and nothing arriving
from outside may conjure a mailbox nobody reads. That is the line
that keeps a carrier from filling a disk with mailboxes for
invented keys, and it is why the check is the spool rather than a
second register — the filesystem is already the list of who lives
here.

Claimed, not merely present, and the difference is not pedantry. The
test was the mailbox directory's existence until 0.5.3, and a local
`beb send` to a key that lives elsewhere creates that directory: one
outbound message to a stranger opened this machine to unbounded
inbound deliveries addressed to them. The sixth guarantee already
said what was meant — a cursor exists if and only if an owner
claimed the mailbox here — and a mailbox holding outbound mail for
somebody else is precisely the case that must not admit anything.

Admission runs before anything is stored. The address lives inside
the envelope, so the check cannot come first in the frame, but it
can come first on disk: `receive` reads the header prefix into
memory, bounded by the same limit the envelope grammar has always
had, and refuses a stranger there. A caller who is not writing to a
resident spends none of the recipient's disk, whatever its lengths
announced.

Past admission the body streams to disk before its signature is
checked, and that is inherent rather than an oversight: a signature
covers the whole envelope, so nothing that refuses to hold a body in
memory can verify one before storing it. What admission buys is that
only a resident's address can ask for the space. Beyond that, how
much a carrier may deliver is the carrier's question — `receive`
reads one frame from stdin and authenticates the bytes; who is
allowed to hand it a frame belongs to the transport, which is the
piece that has a peer to authenticate. Exposed with no transport in
front of it, `receive` is as open as the pipe feeding it.

`receive` verifies before anything becomes visible: frame, envelope
grammar, ed25519 only, a mailbox that is claimed, signature, and only
then installs through the same lock, counter, write ordering, and
durability as local delivery. Failure at any step is fail-closed
and leaves nothing visible; as with any delivery, a failure after id
allocation may leave a gap, and gaps are legal.

Reading stays bound to identity, and that is where the boundary
lives: `read` and `list` resolve the pinned identity and `read`
refuses a message whose `to:` is not that identity. A
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
