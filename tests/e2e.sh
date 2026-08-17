#!/usr/bin/env bash
# End-to-end suite. Run via `cargo test` (tests/cli.rs) or by hand:
#   BEB=target/debug/beb bash tests/e2e.sh
set -u

BEB=${BEB:?set BEB to the beb binary}
case "$BEB" in /*) ;; *) BEB=$PWD/$BEB ;; esac

export HOME=$(mktemp -d)
# BEB_IDENTITY too: the suite is run by people who use beb, and an
# identity inherited from the caller's shell would answer for every
# command in it.
unset XDG_DATA_HOME XDG_CONFIG_HOME BEB_IDENTITY 2>/dev/null || true
SPOOL=$HOME/.local/share/beb
KS=$HOME/.config/beb/known_signers
W=$HOME/work
mkdir -p "$W"
# Out of the repository before anything runs. `init` acts on the working
# directory, so a test that forgets to cd would otherwise make or adopt
# an identity in the checkout beb is being built from -- which is how
# `bx a init` came to adopt the repo's own .beb into a fresh spool and
# report success. $BEB was made absolute above, so this is safe.
cd "$HOME" || exit 1

n=0
OUT=$HOME/out.txt
ERR=$HOME/err.txt
ok() { n=$((n + 1)); echo "ok $n - $1"; }
die() {
    echo "not ok - $1"
    echo "--- stdout ---"; cat "$OUT" 2>/dev/null
    echo "--- stderr ---"; cat "$ERR" 2>/dev/null
    exit 1
}

# Run beb as identity $1 (a dir under $W). Every verb but init resolves
# BEB_IDENTITY and nothing else, so the harness pins the way a launcher
# does -- claude-beb at SessionStart, direnv in a shell, an operator on
# one command -- and no test below depends on a working directory.
pin() { local d=$1; shift; BEB_IDENTITY="$W/$d" "$BEB" "$@"; }
bx() { local d=$1; shift; pin "$d" "$@" >"$OUT" 2>"$ERR"; }
# init is the exception, and is run the way a person runs it: unpinned,
# from the directory that is about to become an identity.
mkid() { mkdir -p "$W/$1" && (cd "$W/$1" && "$BEB" init "$1") >"$OUT" 2>"$ERR"; }
# Every init names its identity now, so a sender that has to arrive
# unnamed -- the way mail from another machine does -- has its line
# taken back out. The file stays the reader's to edit; beb only ever
# adds to it.
unname() { grep -v "^$1 " "$KS" >"$KS.un" 2>/dev/null; mv "$KS.un" "$KS"; }
addr() { pin "$1" whoami 2>/dev/null; }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi; }
# A mailbox is named for the key itself, not a hash of it: the 32 raw
# ed25519 bytes in hex. Derivable without hashing, and reversible.
keyhex() { python3 -c '
import base64,sys
blob = base64.b64decode(sys.argv[1].split()[1])
sys.stdout.write(blob[19:51].hex())' "$1"; }
mbox() { echo "$SPOOL/$(keyhex "$(addr "$1")")"; }
# A stored message is one file: the frame, header first. Planting one by
# hand means assembling it the way delivery does.
plant() { # <mailbox> <id> <envelope> <signature>
    e=$(wc -c <"$3" | tr -d ' '); g=$(wc -c <"$4" | tr -d ' ')
    { printf 'beb %s %s\n' "$e" "$g"; cat "$3" "$4"; } >"$1/msg/$2"
    # Delivery raises the counter before it places the message, and the
    # counter is what makes an id guessable -- nothing above it is looked
    # for. A fixture that skipped this would plant a message beb has no
    # reason to believe exists.
    cur=$(cat "$1/.counter" 2>/dev/null || echo 0)
    [ "$((10#$2))" -gt "$((10#$cur))" ] && printf '%s' "$((10#$2))" >"$1/.counter"
    :
}

# ---- version -----------------------------------------------------------

"$BEB" --version >"$OUT" 2>"$ERR" || die "--version failed"
grep -qE '^beb [0-9]+\.[0-9]+\.[0-9]+$' "$OUT" || die "--version shape: $(cat "$OUT")"
ok "--version prints beb x.y.z"

# Help you asked for is the artifact of asking: stdout, unprefixed,
# exit 0, so `beb --help | grep send` behaves like any other listing.
"$BEB" --help >"$OUT" 2>"$ERR" || die "--help exited nonzero"
grep -q '^beb [0-9]' "$OUT" || die "--help does not name its version: $(cat "$OUT")"
grep -q '^  beb send RECIPIENT' "$OUT" || die "--help does not list the verbs: $(cat "$OUT")"
grep -q '^  beb --help' "$OUT" || die "--help does not list itself"
# The pin governs every verb but init and was discoverable only by
# tripping over it. A capability nobody can find is not there.
grep -q 'BEB_IDENTITY' "$OUT" || die "--help never names the one variable every verb reads"
# and says something true about it. "Every verb but init reads
# BEB_IDENTITY" was false -- init reads it and falls back to cwd -- and
# contradicted the init line three rows above, which an agent reading
# the help cold reported as the most confusing message it met.
grep -q 'reads BEB_IDENTITY' "$OUT" && die "--help still claims init does not read the pin"
grep -q 'requires it except init' "$OUT" || die "--help does not say how init differs: $(cat "$OUT")"
grep -q '^beb: ' "$OUT" && die "--help prefixed the list it was asked for"
test -s "$ERR" && die "--help wrote to stderr: $(cat "$ERR")"
ok "--help is an artifact: stdout, unprefixed, exit 0"

# Help nobody asked for is a refusal, and a refusal names the fix
# rather than being the fix. Printing the whole list under an unknown
# verb would be nine prefixed lines in stderr.
# Bare `beb` is the opening question rather than a mistake, so it
# answers with the list, byte for byte what --help prints.
HELP=$HOME/help.txt
"$BEB" --help >"$HELP" 2>/dev/null
"$BEB" >"$OUT" 2>"$ERR" || die "bare beb exited nonzero: $(cat "$ERR")"
test -s "$ERR" && die "bare beb wrote to stderr: $(cat "$ERR")"
cmp -s "$OUT" "$HELP" || die "bare beb and --help differ: $(cat "$OUT")"
ok "bare beb prints exactly what --help prints"

for bad in frobnicate help -h; do
    "$BEB" "$bad" >"$OUT" 2>"$ERR"
    test $? -eq 0 && die "non-verb \"$bad\" succeeded"
    test "$(wc -l <"$ERR" | tr -d ' ')" = 1 || die "non-verb \"$bad\" refused in more than one line: $(cat "$ERR")"
    grep -q '^beb: .*beb --help lists the verbs$' "$ERR" ||
        die "non-verb \"$bad\": the refusal does not name --help: $(cat "$ERR")"
    test -s "$OUT" && die "non-verb \"$bad\" wrote to stdout: $(cat "$OUT")"
done
ok "a non-verb refuses in one prefixed line naming beb --help"

# The property every verb now upholds, checked in one place so a new
# verb cannot quietly break it: `2>&1 | grep -v '^beb:'` is exactly
# stdout. receive was the last exception, printing its ack unprefixed
# where a transport inheriting stdout could not tell prose from an
# artifact.
mkdir -p "$W/um1" "$W/um2"
(cd "$W/um1" && "$BEB" init um1) >/dev/null 2>&1 || die "init um1"
(cd "$W/um2" && "$BEB" init um2) >/dev/null 2>&1 || die "init um2"
UM2=$(addr um2)
pin um1 pack "$UM2" --subject "merged" --body "the body" >"$HOME/um.mbeb" 2>/dev/null || die "pack um"

merged_is_stdout() {   # $1 label, rest: argv
    local label=$1; shift
    "$@" >"$HOME/um.out" 2>/dev/null
    "$@" 2>&1 | grep -v '^beb:' >"$HOME/um.merged"
    cmp -s "$HOME/um.out" "$HOME/um.merged" ||
        die "$label: 2>&1 | grep -v '^beb:' did not reconstruct stdout"
}
merged_is_stdout "whoami" pin um1 whoami
merged_is_stdout "list"   pin um1 list --unread --limit 5
merged_is_stdout "send"   pin um1 send "$UM2" --subject s --body b
merged_is_stdout "wait"   pin um2 wait --timeout 0
pin um2 read >/dev/null 2>&1
"$BEB" drop <"$HOME/um.mbeb" >"$HOME/um.out" 2>/dev/null
"$BEB" drop <"$HOME/um.mbeb" 2>&1 | grep -v '^beb:' >"$HOME/um.merged"
cmp -s "$HOME/um.out" "$HOME/um.merged" || die "receive: the merge did not un-merge"
ok "every verb un-merges: grep -v '^beb:' is exactly stdout"

# ---- identity ----------------------------------------------------------

mkid a || die "init a"
# stdout is the address alone: the same bytes whoami prints, bound for a
# known_signers line on somebody else's machine. Prose in front of it
# would make `beb init >addr` produce a file nobody can use.
test "$(wc -l <"$OUT" | tr -d ' ')" = 1 || die "init stdout is not one line: $(cat "$OUT")"
grep -qx 'ssh-ed25519 [A-Za-z0-9+/=]*' "$OUT" || die "init stdout is not a bare address: $(cat "$OUT")"
# Everything said about it is on stderr, and every line carries the
# prefix, because `2>&1 | grep -v '^beb:'` is how a caller who merged
# the streams gets stdout back.
grep -q "^beb: created .beb/id_ed25519, mailbox .*, cursor at 0$" "$ERR" || die "init ack: created line"
# The ack names two identifiers, a mailbox and a key. Saying "that
# address" left a reader to pick, and an agent evaluating the CLI cold
# ran `whoami` to find out which one `send` wanted.
# No positional reference. A harness that captures the streams
# separately and concatenates them leaves nothing "above" anything, and
# one reported the address glued to the end of the previous sentence.
grep -qi "above" "$ERR" && die "init ack points at a position the output may not have: $(cat "$ERR")"
grep -q "^beb: beb whoami prints your address" "$ERR" || die "init ack: names the verb that reprints it"
# init names the roster because it wrote to it: the name it just took
# is the difference between an address anybody can type and 68
# characters of base64, and the line saying so is a fact about what
# happened, not an instruction to go and do something.
grep -qE "^beb: named [^ ]+ in .*known_signers" "$ERR" ||
    die "init does not say what it named or where: $(cat "$ERR")"
# Still no paste template. That belongs to `read`, at the moment a
# sender is in front of you and cannot be named; here it would be a
# rule to learn about a file init has already written.
grep -q "append a line" "$ERR" && die "init carries a roster template nobody needs here"
grep -v '^beb:' "$ERR" | grep -q . && die "unprefixed line on stderr: $(cat "$ERR")"
# The roster template used to be printed here, pointing at the reader's
# own known_signers -- a file their own key does nothing in. It lives at
# the one moment it is asked for, an unresolved name, and is tested there.
grep -q '<name>' "$OUT" "$ERR" && die "init still prints the roster template"
ok "init ack shape: the address on stdout, everything about it prefixed on stderr"

# Callers merge the streams in order to filter them: `cmd | head` filters
# stdout alone and lets stderr past unfiltered. So the merge has to be
# reversible, which is the whole job of the prefix.
mkdir -p "$W/merged"
(cd "$W/merged" && "$BEB" init merged 2>&1) | grep -v '^beb:' >"$OUT"
test "$(cat "$OUT")" = "$(addr merged)" ||
    die "2>&1 | grep -v '^beb:' did not reconstruct stdout: $(cat "$OUT")"
ok "a merged stream un-merges: grep -v '^beb:' is exactly stdout"

# init writes the roster now, so it makes both the directory and the
# file, and what it writes reads back as exactly one usable name. beb
# still adds nothing else: the reader's own lines are never touched,
# which the merge test below proves against a file written by hand.
test -d "$(dirname "$KS")" || die "init did not create $(dirname "$KS")"
test -f "$KS" || die "init did not write $KS"
grep -qE "^merged ssh-ed25519 [A-Za-z0-9+/]+$" "$KS" ||
    die "the line init wrote is not one usable name: $(cat "$KS")"
echo "someone ssh-ed25519 AAAA" >>"$KS" 2>"$ERR" || die "append after init failed: $(cat "$ERR")"
rm -f "$KS"
ok "init creates the roster's directory, so its own next step lands"

A=$(addr a)
case "$A" in "ssh-ed25519 "*) ;; *) die "whoami shape: $A" ;; esac
ok "whoami is the address"

grep -qx '\*' "$W/a/.beb/.gitignore" || die "gitignore content"
ok "init writes .beb/.gitignore"

(cd "$W/a" && "$BEB" init a) >"$OUT" 2>"$ERR" && die "second init succeeded"
grep -q "already an identity" "$ERR" || die "double init refusal text"
ok "init refuses twice"

# The argument is the name now, so only one shaped like a path still
# needs the old answer: `beb init alpha/` is somebody reaching for the
# pre-0.8.0 meaning, and the fix is still a cd, because init writes
# where it runs.
mkdir -p "$W/argtest" && (cd "$W/argtest" && "$BEB" init sub/) >"$OUT" 2>"$ERR" &&
    die "init accepted a path-shaped name"
grep -q 'reads as a directory, not a name' "$ERR" ||
    die "init did not read a path-shaped name as a place: $(cat "$ERR")"
grep -q '(cd sub/ && beb init NAME)' "$ERR" ||
    die "the refusal does not name the working form: $(cat "$ERR")"
test -e "$W/argtest/.beb" && die "the refused init still created an identity in the cwd"
ok "a path-shaped name is refused as a place, and the refusal names the cd"

# A name the roster cannot carry is refused before a key exists, and
# the refusal says which character it is, not just that the name is bad.
(cd "$W/argtest" && "$BEB" init 'two words') >"$OUT" 2>"$ERR" && die "init took a name with a space"
grep -q 'a name is one word' "$ERR" || die "space refusal: $(cat "$ERR")"
(cd "$W/argtest" && "$BEB" init 'a*b') >"$OUT" 2>"$ERR" && die "init took a wildcard name"
grep -q '"\*" is not allowed in a name' "$ERR" || die "wildcard refusal: $(cat "$ERR")"
(cd "$W/argtest" && "$BEB" init '#x') >"$OUT" 2>"$ERR" && die "init took a comment name"
grep -q 'a line starting with # is a comment' "$ERR" || die "comment refusal: $(cat "$ERR")"
test -e "$W/argtest/.beb" && die "a refused name still made a key"
ok "a name the roster could not carry is refused, by character, before any key exists"

# What init says about the name has to be true: the key is the address,
# the name resolves to it, and sending to the key directly still works.
mkid namedid >/dev/null || die "init namedid"
NID=$(addr namedid)
mkid sender2 >/dev/null || die "init sender2"
pin sender2 send namedid --subject "by name" --body x >/dev/null 2>&1 || die "send by name"
pin sender2 send "$NID" --subject "by key" --body x >/dev/null 2>&1 || die "send by key"
bx namedid list --unread --limit 5 || die "list namedid"
test "$(grep -c 'by name\|by key' "$OUT")" = 2 ||
    die "name and key did not reach one mailbox: $(cat "$OUT")"
ok "the name resolves to the address; the key still addresses it directly"

# whoami says the name as well as the address, because init took one.
# The address stays alone on stdout: a mailbox is named for exactly
# those bytes, and anything computing a path derives it from them.
bx namedid whoami || die "whoami named"
test "$(cat "$OUT")" = "$NID" || die "whoami stdout is not the address alone: $(cat "$OUT")"
grep -q "named namedid here" "$ERR" || die "whoami does not say the name: $(cat "$ERR")"
ok "whoami names the identity on stderr and keeps the address alone on stdout"

# contacts prints the file's own format, so a line appends to somebody
# else's known_signers verbatim -- which means no marker on the line
# that is this identity, however useful that would be to look at.
bx namedid contacts || die "contacts"
grep -q "^namedid ssh-ed25519 " "$OUT" || die "contacts row shape: $(cat "$OUT")"
grep -qE '<-|this identity' "$OUT" && die "contacts marked a row and made it unpasteable: $(cat "$OUT")"
grep -q "namedid is this identity" "$ERR" || die "contacts does not say which is you: $(cat "$ERR")"
# Every stdout line parses as a roster line, which is the whole claim.
# Not `read n ...`: n is this suite's test counter, and clobbering it
# silently restarted the numbering rather than failing anything.
while read -r cname ctype cb64; do
    test -n "$cname" && test "$ctype" = "ssh-ed25519" && test -n "$cb64" ||
        die "contacts printed a line known_signers could not carry: $cname $ctype $cb64"
done <"$OUT"
ok "contacts prints pasteable known_signers lines, commentary on stderr"

# A line the parser cannot use is reported, never silently dropped: a
# name that vanished from the listing is a name whose refusal turns up
# later, at a send, with nothing to connect it to.
printf 'rsaguy ssh-rsa AAAA\n' >>"$KS"
bx namedid contacts || die "contacts with an unusable line"
grep -q "rsaguy" "$OUT" && die "an unusable line was printed as pasteable: $(cat "$OUT")"
grep -q "is not usable (key type ssh-rsa)" "$ERR" || die "unusable line not reported: $(cat "$ERR")"
ok "contacts reports a line it cannot use instead of dropping it"
grep -v '^rsaguy ' "$KS" >"$KS.un" && mv "$KS.un" "$KS"

# init never reads BEB_IDENTITY, so a pin cannot send it anywhere and
# there is no missing-directory case left to refuse: the target is the
# working directory, which exists by definition of being in it.
mkdir -p "$W/argtest/sub"
(cd "$W/argtest" && BEB_IDENTITY=nowhere "$BEB" init argtest) >"$OUT" 2>"$ERR" ||
    die "init refused because of a pin it should not read: $(cat "$ERR")"
test -f "$W/argtest/.beb/id_ed25519" || die "init did not write to the working directory"
test -e "$W/nowhere" && die "init created the directory the pin named"
ok "a pin pointing anywhere at all cannot move or block init"

# The form that refusal prints works once NAME is filled in, which is
# the one word beb cannot supply.
PATH="$(dirname "$BEB"):$PATH" sh -c "cd '$W/argtest' && (cd sub && beb init subid)" >/dev/null 2>&1 ||
    die "the printed (cd sub && beb init NAME) form did not work"
test -f "$W/argtest/sub/.beb/id_ed25519" || die "the printed form made no identity where it said"
ok "the form init prints in that refusal works with a name filled in"

(cd "$W/argtest" && "$BEB" init --force) >"$OUT" 2>"$ERR" && die "init accepted an option"
grep -q 'there is no option "--force"' "$ERR" || die "init option refusal: $(cat "$ERR")"
ok "init names an unknown option as an option, not a directory"

mkdir -p "$W/nobody"
bx nobody whoami && die "whoami without identity succeeded"
grep -q "beb init" "$ERR" || die "no-identity refusal names the fix"
ok "a pin at a directory with no .beb refuses, naming beb init"

# The pin is not optional and its absence is not a fallback. beb read
# the working directory until 0.5.3, which suits a person, who is
# somewhere, and not a program, which moves between subdirectories,
# spawns shells and hands work to subagents. Each of those was a chance
# to sign as somebody else, silently.
(cd "$W/a" && "$BEB" whoami) >"$OUT" 2>"$ERR" &&
    die "whoami resolved with no BEB_IDENTITY, from a directory holding a .beb"
grep -q "BEB_IDENTITY is not set" "$ERR" || die "unpinned refusal: $(cat "$ERR")"
grep -q "export BEB_IDENTITY=" "$ERR" || die "unpinned refusal does not name the export: $(cat "$ERR")"
test -s "$OUT" && die "unpinned whoami wrote to stdout: $(cat "$OUT")"
ok "an unpinned verb refuses even standing inside an identity, and names the export"

# whoami's stdout is the address and nothing else; which directory
# answered goes to stderr. The pin is written by a hook into a file
# sourced before the process began, so this line is the only way a
# process learns who it was made to be.
bx a whoami || die "whoami failed"
grep -qx 'ssh-ed25519 [A-Za-z0-9+/=]*' "$OUT" || die "whoami stdout is not a bare address: $(cat "$OUT")"
grep -q "^beb: identity from BEB_IDENTITY=.*/work/a$" "$ERR" ||
    die "whoami does not name the directory it resolved: $(cat "$ERR")"
ok "whoami names the pin that answered, on stderr"

# init never reads BEB_IDENTITY. The pin says which identity to act as,
# and init does not act as one -- it makes one -- so reading it here was
# a category error, and it cost four successive readers the same
# question the help line could not answer without growing: must the
# directory the pin names already exist? It cannot be asked now.
mkdir -p "$W/pinned" "$W/elsewhere"
(cd "$W/elsewhere" && BEB_IDENTITY="$W/pinned" "$BEB" init elsewhere) >"$OUT" 2>"$ERR" ||
    die "init under a pin failed: $(cat "$ERR")"
test -f "$W/elsewhere/.beb/id_ed25519" || die "init did not write to the working directory"
test -e "$W/pinned/.beb" && die "init wrote to the directory the pin named"
ok "init writes to the working directory and ignores the pin entirely"

# Making an identity while pinned elsewhere is legitimate -- a second
# identity has to start somehow -- so it is said rather than refused.
# Silently doing it would leave every later verb answering as the other.
grep -q "BEB_IDENTITY points at .*, so other verbs still act as that identity" "$ERR" ||
    die "init did not say the pin points elsewhere: $(cat "$ERR")"
grep -q "export BEB_IDENTITY=\$PWD to use this one instead" "$ERR" ||
    die "init did not name the export: $(cat "$ERR")"
ok "an identity made while pinned elsewhere says so, and names the fix"

(cd / && BEB_IDENTITY="$W/elsewhere" "$BEB" whoami) >"$OUT" 2>"$ERR" ||
    die "the new identity does not resolve from elsewhere: $(cat "$ERR")"
grep -q "^ssh-ed25519 " "$OUT" || die "pinned whoami: $(cat "$OUT")"
ok "an identity answers from any working directory once the pin names it"

# A refusal must leave nothing. Generating a keypair and then failing
# leaves a private key behind and a directory that answers "already an
# identity" to the retry.
(cd "$W/elsewhere" && "$BEB" init elsewhere) >"$OUT" 2>"$ERR" &&
    die "init succeeded onto an existing identity"
grep -q "already an identity" "$ERR" || die "existing-identity refusal: $(cat "$ERR")"
ok "a second init on the same directory refuses"

# Unpinned, the export is the whole point: nothing else will use what
# was just made until the variable names it.
mkdir -p "$W/seam"
(cd "$W/seam" && "$BEB" init seam) >"$OUT" 2>"$ERR" || die "init seam: $(cat "$ERR")"
grep -q "every other verb needs BEB_IDENTITY set: export BEB_IDENTITY=\$PWD" "$ERR" ||
    die "unpinned init does not name the export: $(cat "$ERR")"
ok "an unpinned init names the export that makes what it just built usable"

# Pinned at the directory being initialised, the export would be noise.
mkdir -p "$W/seam2"
(cd "$W/seam2" && BEB_IDENTITY="$W/seam2" "$BEB" init seam2) >"$OUT" 2>"$ERR" ||
    die "init pinned at itself failed: $(cat "$ERR")"
grep -q "BEB_IDENTITY already points here" "$ERR" ||
    die "init did not notice the pin already named it: $(cat "$ERR")"
grep -q "export BEB_IDENTITY=" "$ERR" &&
    die "init told the caller to set what is already set: $(cat "$ERR")"
ok "an init pinned at its own directory says the pin already fits"

mkid b >/dev/null || die "init b"
B=$(addr b)
mkid c >/dev/null || die "init c"
C=$(addr c)

# ---- roster ------------------------------------------------------------

{
    echo "a $A"
    echo "b $B"
    echo "c $C"
    echo "dup $A"
    echo "dup $B"
    echo "legacy ssh-rsa AAAAB3NzaC1yc2Efakefakefake"
    echo "star* $A"
} >"$KS"

bx b send a --subject "endpoint ready" --body "auth endpoint ready" || die "send by name"
# The id alone on stdout: it is the handle a transport carries by, and
# `id=$(beb send ...)` has to yield a number rather than a sentence.
# Nothing on stdout. The delivery id is an id in somebody else's
# mailbox: the sender cannot peek, read or prune it, because those
# verbs work on the mailbox of whoever runs them. Nor does a transport
# learn it here -- beb-ssh's carry watches the spool for a message in a
# mailbox with no cursor, so it finds ids by listing a directory.
test -s "$OUT" && die "send wrote to stdout, which no caller can use: $(cat "$OUT")"
grep -q '^beb: accepted for a; 19 bytes; it waits on this machine for beb read$' "$ERR" ||
    die "send ack: $(cat "$ERR")"
ok "send captures nothing on stdout and says what it did on stderr"

printf 'schema question' | bx b send a --subject "schema" || die "send stdin"
test -s "$OUT" && die "send stdin wrote to stdout: $(cat "$OUT")"
grep -q '; 15 bytes;' "$ERR" || die "send does not count a stdin body: $(cat "$ERR")"
ok "send body from stdin, counted"

# A subject and a body are two free strings of the same shape. As adjacent
# positionals they were a swap waiting to happen -- `beb send alice "the
# migration needs review" "deploy blocked"` is a valid command meaning
# the wrong thing, and nothing about it looks wrong. Named, the order
# stops mattering and a stray bare argument refuses.
bx b send a "deploy blocked" "needs review" && die "the positional shape was accepted"
grep -q 'takes one recipient, and a subject and body are named' "$ERR" ||
    die "a second bare argument does not name the flags: $(cat "$ERR")"
ok "two bare arguments refuse rather than becoming whichever field they sat beside"

bx b send --subject "flags first" --body "body" c || die "flags before the recipient refused"
grep -q '; 4 bytes;' "$ERR" || die "order-independent parse took the wrong body: $(cat "$ERR")"
bx b send c --subject "long forms" --body "body" || die "long forms refused"
ok "--subject and --body, in any order relative to the recipient"

bx b send a --body "body only" && die "a missing subject was accepted"
grep -q 'needs a subject' "$ERR" || die "missing subject: $(cat "$ERR")"
bx b send a --subject && die "--subject with no value was accepted"
grep -q -- '--subject needs a value' "$ERR" || die "dangling --subject: $(cat "$ERR")"
bx b send a --subject one --subject two && die "two subjects accepted"
grep -q 'takes one subject' "$ERR" || die "repeated --subject: $(cat "$ERR")"
bx b send a --subject x --headline y && die "unknown option accepted"
grep -q 'has no option "--headline"' "$ERR" || die "unknown option: $(cat "$ERR")"
ok "send names every way the flags can be got wrong"

# Short forms were removed in 0.6.0: beb is read and written mostly by
# programs, which do not save keystrokes and do pay for ambiguity. `-t`
# meant subject on send and timeout on wait for about an hour. A short
# form somebody guesses anyway is answered with the one that works.
bx b send a -s x -b y && die "-s accepted"
grep -q 'send has no -s; the option is --subject' "$ERR" || die "-s refusal: $(cat "$ERR")"
bx a list -f 1 --limit 5 && die "-f accepted"
grep -q 'list has no -f; the option is --from' "$ERR" || die "-f refusal: $(cat "$ERR")"
bx a list -n 2 --limit 5 && die "-n accepted"
grep -q 'list has no -n; the option is --limit' "$ERR" || die "-n refusal: $(cat "$ERR")"
bx a wait -t 1 && die "-t accepted"
grep -q 'wait has no -t; the option is --timeout' "$ERR" || die "-t refusal: $(cat "$ERR")"
ok "a removed short form names the long one rather than dead-ending"

# And the whole listing spells one option one way.
"$BEB" --help >"$OUT" 2>/dev/null
grep -qE '(^| )-[a-z]( |$)' "$OUT" && die "a short form survives in the help: $(grep -nE '(^| )-[a-z]( |$)' "$OUT")"
ok "no short form appears anywhere in the help"

# Every entry is a signature and, indented under it, what the verb
# does. A single aligned column cannot serve both `beb init` and `beb
# send RECIPIENT --subject S [--body B]` -- eight characters against
# forty-one -- so an aligned description either wraps or strands the
# short verbs across half a screen. It wrapped, on three of nine, which
# left the eye no rhythm.
#
# `list` is the one entry with three description lines. Which rows, which
# direction, and that both are required do not fit in fewer, and a cold
# reader handed the one-line version answered "the exact command cannot
# be determined" to both of the questions about digging back.
"$BEB" --help >"$OUT" 2>/dev/null
awk 'length($0) > 78 { print; found=1 } END { exit found }' "$OUT" ||
    die "a help line runs past 78 columns"
test "$(grep -c '^  beb ' "$OUT")" = 12 || die "the help does not list 12 entries"
# Every signature is followed by exactly one indented description.
awk '/^  beb /{ want=1; next } want { if ($0 !~ /^      [^ ]/) { print NR": "$0; bad=1 } want=0 } END { exit bad }' "$OUT" ||
    die "a signature is not followed by an indented description"
test "$(awk '/^  beb list /{f=1;next} /^  beb /{f=0} f&&/^      [^ ]/{n++} END{print n+0}' "$OUT")" = 3 ||
    die "list should carry exactly three description lines"
ok "every entry is a signature and one indented line, inside 78 columns"

# Options stay inline in the signature, where a reader came to find
# them, rather than being hinted at and looked up somewhere else.
grep -q '^  beb send RECIPIENT --subject S \[--body B\]$' "$OUT" || die "send signature: $(grep '  beb send' "$OUT")"
grep -q '^  beb list (--unread | --after ID | --before ID) --limit N$' "$OUT" || die "list signature: $(grep '  beb list' "$OUT")"
grep -q '^  beb wait \[--from ID\] \[--timeout SECS\]$' "$OUT" || die "wait signature: $(grep '  beb wait' "$OUT")"
ok "every option appears inline in the signature that takes it"

# A subject reaches a reader's terminal through `list` without passing
# through a body, so a control character in one is a sender moving
# somebody else's cursor.
bx b send a --subject "$(printf 'wipe\033[2Kline')" --body x && die "an ANSI escape in a subject was accepted"
grep -q 'control character' "$ERR" || die "control-character refusal: $(cat "$ERR")"
bx b send a --subject "" --body x && die "an empty subject was accepted"
grep -q 'subject is empty' "$ERR" || die "empty subject: $(cat "$ERR")"
LONG=$(printf 'x%.0s' $(seq 1 121))
bx b send a --subject "$LONG" --body x && die "an overlong subject was accepted"
grep -q 'the limit is 120' "$ERR" || die "overlong subject: $(cat "$ERR")"
ok "a subject is one plain line, non-empty, and at most 120 bytes"

# The sender's clock, labelled as the sender's. beb has no trustworthy
# time of its own to offer: a file's mtime is rewritten by any careless
# cp or rsync, so it decays into a wrong answer that still looks like an
# answer, while a claim labelled a claim stays honest even when the
# clock behind it is not.
bx b send c --subject "dated" --body x || die "dated send"
MB_C=$(mbox c)
LAST=$(ls "$MB_C/msg" | sort | tail -1)
grep -qE '^date: [0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$' "$MB_C/msg/$LAST" ||
    die "no RFC3339 date header in the envelope: $(head -5 "$MB_C/msg/$LAST")"
SENT=$(sed -n 's/^date: //p' "$MB_C/msg/$LAST" | head -1)
NOWS=$(date -u +%Y-%m-%dT%H:%M:%SZ)
test "${SENT%T*}" = "${NOWS%T*}" || die "date is not today: $SENT vs $NOWS"
ok "send stamps an RFC3339 UTC date the envelope carries"

# It is signed like every other byte, so it cannot be edited in place.
# Same length in, same length out, so the frame header stays true and
# the signature is the only thing left to object.
python3 - "$MB_C/msg/$LAST" <<'EOF' || die "could not rewrite the date"
import sys, re
p = sys.argv[1]
b = open(p, 'rb').read()
out = re.sub(rb'\ndate: [^\n]{20}', b'\ndate: 1999-01-01T00:00:00Z', b, count=1)
assert len(out) == len(b), "rewrite changed the frame length"
open(p, 'wb').write(out)
EOF
bx c peek $((10#$LAST)) && die "a message with an edited date verified"
grep -q "failed verification" "$ERR" || die "edited date was not caught by the signature: $(cat "$ERR")"
ok "the date is inside the signature: editing it destroys the message"
rm -f "$MB_C/msg/$LAST"

# A date beb did not write is refused at the grammar, so a delivery
# cannot carry a shape beb would then have to interpret.
mkdir -p "$W/dt" && (cd "$W/dt" && "$BEB" init dt) >"$HOME/dt.out" 2>/dev/null || die "init dt"
DT=$(cat "$HOME/dt.out")
for bad in "2026-08-15 02:26:34Z" "2026-08-15T02:26:34+00:00" "2026-02-30T00:00:00Z" "not-a-date"; do
    ENV2=$HOME/baddate
    printf 'from: %s\nto: %s\nnonce: AAAAAAAAAAAAAAAAAAAAAA==\ndate: %s\nsubject: t\n\nbody' \
        "$DT" "$DT" "$bad" >"$ENV2"
    # ssh-keygen prompts to overwrite an existing .sig, which blocks
    # on a stale one from the previous iteration.
    rm -f "$ENV2.sig"
    ssh-keygen -Y sign -n beb -f "$W/dt/.beb/id_ed25519" "$ENV2" 2>/dev/null </dev/null || die "sign $bad"
    DT_MB="$SPOOL/$(keyhex "$DT")"
    plant "$DT_MB" 000000000000000042 "$ENV2" "$ENV2.sig"
    env BEB_IDENTITY="$W/dt" "$BEB" read >"$OUT" 2>"$ERR" && die "malformed date accepted: $bad"
    grep -q "date is not YYYY-MM-DDTHH:MM:SSZ" "$ERR" || die "date refusal for \"$bad\": $(cat "$ERR")"
    rm -f "$DT_MB/msg/000000000000000042"
done
ok "a date beb did not write is refused at the grammar, signature or not"

# list shows how long ago the sender says it sent, and shows a clock
# that is ahead as ahead rather than clamping it to zero.
bx a list --after 0 --limit 50 || die "list for age"
grep -qE '^[0-9]+  (now|[0-9]+[smhd])  ' "$OUT" || die "no age column: $(cat "$OUT")"
ok "list carries an age column from the claimed date"

# An empty body is a legal message and silence about it is how a lost
# heredoc looks exactly like a delivered one.
printf '' | bx b send c --subject "nothing to say" || die "send empty"
grep -q '; 0 bytes;' "$ERR" || die "empty body not named: $(cat "$ERR")"
ok "an empty body is accepted and said out loud"

# Two outcomes wore one sentence until 0.5.3. A recipient with no
# mailbox claimed here cannot read what was just written for them, and
# the cursor init writes is the only thing that tells the cases apart.
mkdir -p "$W/stranger"
(cd "$W/stranger" && "$BEB" init stranger) >"$HOME/s.out" 2>/dev/null || die "init stranger"
S=$(cat "$HOME/s.out")
rm -rf "$SPOOL/$(keyhex "$S")"
bx b send "$S" --subject "into the void" --body "body for a stranger" || die "send to unclaimed"
grep -q 'nobody here reads it, so it waits in the outbox as [0-9]*$' "$ERR" ||
    die "send to a non-resident reads as local delivery: $(cat "$ERR")"
# The next step is named in beb's own vocabulary. An agent learns this
# tool from this tool, so a word for something beb neither implements
# nor defines is a word it cannot look up.
grep -q '^beb: a carrier takes it from there; nothing else on this machine will$' "$ERR" ||
    die "the outbox ack does not say who moves it: $(cat "$ERR")"
ok "a non-resident recipient sends to the outbox, and the ack says what happens next"

# It never creates a mailbox for somebody who does not read here. That
# is what makes a directory in the spool mean a reader lives here, which
# is in turn what lets `drop` refuse a stranger.
test -e "$SPOOL/$(keyhex "$S")" &&
    die "sending to a non-resident created a mailbox for them"
test -f "$SPOOL/outbox/000000000000000001-$(keyhex "$S")" ||
    die "the delivery is not in the outbox: $(ls "$SPOOL/outbox" 2>&1)"
ok "a mailbox in the spool means a reader here; outbound mail is not one"

# The outbox is a place, not an interface. Everything a carrier needs is
# in the name -- an id to order by and the address to route on -- so it
# reads a directory and needs no beb process at all. That is why there
# is no `pickup` and no `rm`: two verbs existed only because the
# recipient was inside the frame, where a carrier must not look.
OUT_ENTRY=$(ls "$SPOOL/outbox" | head -1)
test -n "$OUT_ENTRY" || die "the outbox is empty"
case "$OUT_ENTRY" in
    000000000000000001-*) ;;
    *) die "the outbox entry is not <id>-<recipient>: $OUT_ENTRY" ;;
esac
test "${OUT_ENTRY#*-}" = "$(keyhex "$S")" ||
    die "the name does not carry the recipient: $OUT_ENTRY"
head -c 4 "$SPOOL/outbox/$OUT_ENTRY" | grep -qx 'beb ' ||
    die "the outbox entry is not a whole frame"
ok "an outbox entry names its order and its recipient, and holds a whole frame"

# A carrier ships it and unlinks it. No beb involved, and nothing beb
# has to be asked for.
"$BEB" drop <"$SPOOL/outbox/$OUT_ENTRY" >"$OUT" 2>"$ERR" &&
    die "a frame for a key that reads elsewhere was installed here"
rm -f "$SPOOL/outbox/$OUT_ENTRY"
test -e "$SPOOL/outbox/$OUT_ENTRY" && die "the carrier could not remove what it shipped"
ok "a carrier drains the outbox with readdir and unlink, and nothing else"

bx b send "$B" --subject "note to self" --body "the body of the note" || die "send to self"
grep -q '^beb: accepted for you; 20 bytes; it waits on this machine for beb read$' "$ERR" ||
    die "send to self prints a raw key back at the sender: $(cat "$ERR")"
ok "a message to yourself says so, and names the verb that collects it"

bx c send a --subject "deploy window" --body "deploy window moved" || die "send from c"

bx b send "$A" --subject "raw key" --body "raw key send" || die "send raw key"
ok "send accepts raw key text"

bx b send "$A pasted-comment" --subject "pub shaped" --body "with comment" || die "send .pub-shaped key"
ok "tolerant key parse (comment stripped)"

bx b send "ssh-ed25519 QQ==" --subject t --body nope && die "base64-shaped non-key accepted"
grep -q "not a valid ssh-ed25519" "$ERR" || die "non-key refusal: $(cat "$ERR")"
ok "key text must decode to a real ed25519 key"

# Title before sender: the subject is capped at 120 bytes so it can be a
# column, while an unnamed sender is 68 characters of base64 in the form
# `send` accepts, and a column of those would push every subject off the
# screen. Titles are padded to the widest in the listing.
bx a list --unread --limit 50 || die "list"
# Newest first: a listing is read to find out what happened, and the
# oldest rows of a full mailbox say nothing about what just arrived.
printf '5  now  pub shaped      b\n4  now  raw key         b\n3  now  deploy window   c\n2  now  schema          b\n1  now  endpoint ready  b\n' |
    diff - "$OUT" >/dev/null || die "list content: $(cat "$OUT")"
ok "list shows id, subject, sender in id order, subjects padded to a column"

# A listing names an unnamed sender by the tail of its key, not by 68
# characters of base64. Ten rows of the same key buried the subjects the
# rows exist to show; an agent reading beb cold said it "dominated the
# output". `read` keeps the whole key, because that is where a reply
# gets composed -- and the short form is a substring of the long one, so
# a reader can tell the row and the message name one party.
mkdir -p "$W/hk" && (cd "$W/hk" && "$BEB" init hk) >"$HOME/hk.out" 2>/dev/null || die "init hk"
unname hk
HK=$(cat "$HOME/hk.out")
mkid hr >/dev/null || die "init hr"
HR=$(addr hr)
pin hk send "$HR" --subject "unnamed sender" --body x >/dev/null 2>&1 || die "send from hk"
bx hr list --unread --limit 50 || die "list with an unnamed sender"
grep -q 'ssh-ed25519' "$OUT" && die "a listing printed a raw key: $(cat "$OUT")"
grep -qE '  \.\.\.[A-Za-z0-9+/]{8}$' "$OUT" || die "no elided key in the row: $(cat "$OUT")"
grep -q "\.\.\.${HK: -8}$" "$OUT" || die "the row's tail is not that sender's key: $(cat "$OUT")"
ok "a listing names an unnamed sender by the tail of the key read prints whole"

bx hr read || die "read from an unnamed sender"
grep -q "$HK" "$ERR" || die "read did not carry the whole key: $(cat "$ERR")"
ok "read carries the whole key, where a reply gets composed"

# send names the recipient the caller just typed the same way, but the
# beb pack line it prints is a command and keeps the key.
bx hk send "$HR" --subject "second" --body x || die "second send"
grep -q 'ssh-ed25519' "$ERR" && die "the send ack echoed the key back at the caller: $(cat "$ERR")"
ok "send names an unnamed recipient by key tail rather than echoing the key"

bx b send nosuch --subject t --body hi && die "unknown name accepted"
grep -q 'no "nosuch"' "$ERR" || die "unknown name refusal"
grep -q 'add: nosuch ssh-ed25519' "$ERR" || die "unknown name refusal names the line to add"
ok "unknown name refuses, names the line to add"

bx b send dup --subject t --body hi && die "ambiguous name accepted"
grep -q 'lines 4, 5' "$ERR" || die "ambiguity refusal lines: $(cat "$ERR")"
ok "ambiguous name refuses, names both lines"

bx b send legacy --subject t --body hi && die "rsa roster line accepted"
grep -q 'ssh-rsa' "$ERR" || die "rsa refusal names the type"
grep -q 'line 6' "$ERR" || die "rsa refusal names the line"
ok "rsa roster line refused by name"

bx b send a --subject "clean name" --body "still fine" || die "clean names poisoned by bad lines"
ok "bad roster lines do not poison the file"

bx b send 'star*' --subject t --body hi && die "wildcard accepted"
grep -q 'wildcard' "$ERR" || die "wildcard refusal"
ok "wildcard principal refused"

set -- $A
bx b send "$1" "$2" --subject t --body x && die "unquoted key accepted"
grep -q 'quote it' "$ERR" || die "unquoted key refusal: $(cat "$ERR")"
ok "unquoted key splitting refuses, names the quoting"

# ---- read: consume and inspect ----------------------------------------
# a's mailbox: 1 auth, 2 schema, 3 deploy, 4 raw-key, 5 with-comment, 6 still-fine

bx a read || die "consume 1"
printf 'auth endpoint ready' | diff - "$OUT" >/dev/null || die "body 1 exact: $(cat "$OUT")"
ok "consume prints exact body, no trailer"

# `read` said nothing about what it did, and the help called it
# "consume", which reads as destruction. Nothing is destroyed: beb has
# no delete verb, retention is local policy, and the message stays on
# disk. Only the cursor moves, and now the receipt says so.
#
# On its own mailbox, so the receipts do not disturb a's sequence.
mkdir -p "$W/look" && (cd "$W/look" && "$BEB" init look) >"$HOME/look.out" 2>/dev/null || die "init look"
LOOK=$(cat "$HOME/look.out")
bx a send "$LOOK" --subject "first look" --body "a body" || die "send to look"
bx a send "$LOOK" --subject "second look" --body "another" || die "send to look again"

bx look read || die "read for its receipt"
grep -qE '^beb: 1 from [^,]+, "first look", [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}; cursor 0 -> 1$' "$ERR" ||
    die "read receipt: $(cat "$ERR")"
test "$(cat "$(mbox look)/cursor")" = 1 || die "read did not advance the cursor"
ok "read names the message, its subject, the claimed date and the cursor move"

# The message is still there afterwards, which is the whole reason
# "consume" was the wrong word.
bx look peek 1 || die "peek a message already read"
grep -q 'cursor stays at 1' "$ERR" || die "peek receipt: $(cat "$ERR")"
test "$(cat "$(mbox look)/cursor")" = 1 || die "peek moved the cursor"
printf 'a body' | diff - "$OUT" >/dev/null || die "a read message did not survive: $(cat "$OUT")"
ok "peek says the cursor stays, it does, and a read message is still readable"

# The roster hint rides with the key it is about, and stops once acted
# on. It sat in `init` until 0.6.0 -- the one moment nobody has a
# correspondent to name -- while the moment a reader was staring at 68
# characters of somebody's base64 said nothing at all.
mkdir -p "$W/nn" && (cd "$W/nn" && "$BEB" init nn) >"$HOME/nn.out" 2>/dev/null || die "init nn"
unname nn
NN=$(cat "$HOME/nn.out")
mkid nr >/dev/null || die "init nr"
NR=$(addr nr)
pin nn send "$NR" --subject "from a stranger" --body b >/dev/null 2>&1 || die "send from nn"
bx nr read || die "read from an unnamed sender"
grep -q "that sender has no name here; append a line to" "$ERR" || die "no roster hint: $(cat "$ERR")"
grep -q "^beb: <name> $NN\$" "$ERR" || die "the hint does not carry the sender's key: $(cat "$ERR")"
ok "an unnamed sender brings the line that names it, key included"

# Follow it verbatim, and the hint stops.
printf 'unnamedsender %s\n' "$NN" >>"$KS"
pin nn send "$NR" --subject "second from stranger" --body b >/dev/null 2>&1 || die "second send from nn"
bx nr read || die "read after naming"
grep -q "no name here" "$ERR" && die "the hint survived being acted on: $(cat "$ERR")"
grep -q "from unnamedsender," "$ERR" || die "the name did not take: $(cat "$ERR")"
ok "naming the sender stops the hint: it is self-limiting"

# Once per sender, not once per message. An agent draining five messages
# from one unnamed sender got the same two lines five times and called
# them noise, interleaved with the bodies.
mkdir -p "$W/bulk" && (cd "$W/bulk" && "$BEB" init bulk) >"$HOME/bulk.out" 2>/dev/null || die "init bulk"
unname bulk
BULK=$(cat "$HOME/bulk.out")
mkid drain >/dev/null || die "init drain"
DR=$(addr drain)
for i in 1 2 3 4; do
    pin bulk send "$DR" --subject "b$i" --body "body $i" >/dev/null 2>&1 || die "bulk send $i"
done
hints=0
for i in 1 2 3 4; do
    bx drain read || die "drain read $i"
    grep -q "no name here" "$ERR" && hints=$((hints + 1))
done
test "$hints" = 1 || die "the roster hint fired $hints times for one sender, want 1"
ok "the roster hint fires once per sender, not once per message"

# A raw body rarely ends in a newline, so the next thing written anywhere
# runs into its last byte. stdout cannot carry the fix -- what is printed
# there has to be the signed bytes and nothing else -- so the separator
# goes to stderr.
mkdir -p "$W/nl2" && (cd "$W/nl2" && "$BEB" init nl2) >"$HOME/nl2.out" 2>/dev/null || die "init nl2"
NL2=$(cat "$HOME/nl2.out")
printf 'no trailing newline' | bx a send "$NL2" --subject "raw" || die "send raw"
pin nl2 read >"$OUT" 2>"$ERR" || die "read raw"
test "$(wc -c <"$OUT" | tr -d ' ')" = 19 || die "stdout is not the exact body: $(od -c "$OUT" | head -2)"
tail -c 1 "$ERR" | od -An -c | grep -q '\\n' || die "no separator after a body that ends mid-line"
ok "a body with no trailing newline gets its separator on stderr, not stdout"

# And a body that already ends in a newline gets none.
printf 'ends with one\n' | bx a send "$NL2" --subject "ends" || die "send ends"
pin nl2 read >"$OUT" 2>"$ERR" || die "read ends"
grep -c '^$' "$ERR" | grep -qx '0' || die "a separator was added to a body that did not need one"
ok "a body that ends a line gets no separator"

# The exit codes exist and are now findable.
"$BEB" --help >"$OUT" 2>/dev/null
grep -q '^Exit: 0 did it, 1 change the command, 2 nothing to do, 3 refused.$' "$OUT" ||
    die "the help does not carry the exit codes: $(grep -i exit "$OUT")"
ok "the help names what an exit code means"

# The envelope carries UTC and only UTC; the receipt is display, and
# reads that instant out on the clock the reader is looking at. No
# offset and no seconds: both are precision for comparing instants
# between machines, which is the envelope's job and already done there.
STORED=$(sed -n 's/^date: //p' "$(mbox look)/msg/000000000000000001")
case "$STORED" in *Z) ;; *) die "the stored date is not UTC: $STORED" ;; esac
UTC_SHAPE="${STORED%:*}"; UTC_SHAPE="${UTC_SHAPE/T/ }"
TZ=UTC bx look peek 1 || die "peek under TZ=UTC"
grep -q "\"first look\", $UTC_SHAPE;" "$ERR" || die "TZ=UTC receipt: $(cat "$ERR") want $UTC_SHAPE"
TZ=Asia/Jakarta bx look peek 1 || die "peek under TZ=Asia/Jakarta"
grep -qE '"first look", [0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}; cursor stays' "$ERR" ||
    die "TZ=Asia/Jakarta receipt: $(cat "$ERR")"
grep -q "$UTC_SHAPE;" "$ERR" && die "a +07:00 zone printed the UTC wall clock: $(cat "$ERR")"
grep -q "Z;" "$ERR" && die "the receipt carried a zone marker: $(cat "$ERR")"
TZ=America/New_York bx look peek 1 || die "peek under TZ=America/New_York"
grep -q "$UTC_SHAPE;" "$ERR" && die "a -04:00 zone printed the UTC wall clock: $(cat "$ERR")"
ok "the receipt reads the stored UTC instant out on the local clock, no zone and no seconds"

# Nothing is ever written behind the artifact. A body is raw and usually
# has no trailing newline, so a line printed after one is glued to its
# last byte and `grep -v '^beb:'` cannot strip what does not start a
# line -- which would break the one property that makes merging safe.
# The earlier un-merge test used init, whose stdout is a whole line, so
# it could not have caught this.
mkdir -p "$W/nl" && (cd "$W/nl" && "$BEB" init nl) >"$HOME/nl.out" 2>/dev/null || die "init nl"
NL=$(cat "$HOME/nl.out")
printf 'no trailing newline' | bx a send "$NL" --subject "raw" || die "send raw body"
pin nl read 2>&1 | grep -v '^beb:' >"$OUT"
printf 'no trailing newline\n' | diff - "$OUT" >/dev/null ||
    die "a merged read did not un-merge to the body: $(od -c "$OUT" | head -3)"
ok "a body with no trailing newline still un-merges: beb never speaks after the artifact"

bx a list --unread --limit 50 || die "list after consume"
grep -qE '^1  ' "$OUT" && die "cursor did not advance: a consumed id is still listed"
tail -1 "$OUT" | grep -qE '^2 .* b$' || die "the oldest unread is not 2: $(tail -1 "$OUT")"
ok "consume advances cursor"

bx a peek 4 || die "peek 4"
printf 'raw key send' | diff - "$OUT" >/dev/null || die "inspect body"
bx a list --unread --limit 50 || die "list after peek"
test "$(grep -c . "$OUT")" = 5 || die "inspect moved the cursor: $(cat "$OUT")"
ok "inspect prints, cursor untouched"

bx a read || die "consume 2"
printf 'schema question' | diff - "$OUT" >/dev/null || die "order after inspect"
ok "consumption continues in id order after inspect"

MB_A=$(mbox a)
rm "$MB_A/msg/000000000000000003" || die "make gap"
bx a read || die "read over gap"
printf 'raw key send' | diff - "$OUT" >/dev/null || die "gap not skipped: $(cat "$OUT")"
ok "gap stepped over silently, inspected message still consumed"

# The signature lives behind the body in the same file now, so
# corrupting it means writing over the frame's tail. The length in the
# header stays right; the bytes do not.
python3 - "$MB_A/msg/000000000000000005" <<'EOF' || die "could not corrupt the signature"
import sys
p = sys.argv[1]
b = bytearray(open(p, 'rb').read())
b[-40:] = b'X' * 40
open(p, 'wb').write(bytes(b))
EOF
bx a read && die "corrupt signature consumed"
grep -q "failed verification" "$ERR" || die "corrupt refusal text"
grep -q "rm '" "$ERR" || die "corrupt refusal names rm"
grep -q "msg/000000000000000005" "$ERR" || die "refusal names the message file"
test -s "$OUT" && die "refusal printed body bytes"
ok "corrupt signature: refused before printing, rm named"

bx a list --unread --limit 50 || die "list after a refused read"
tail -1 "$OUT" | grep -q '^5  ' || die "cursor moved past bad message: $(tail -1 "$OUT")"
ok "cursor unmoved by refusal"

test -z "$(ls -A "$SPOOL/.tmp" 2>/dev/null)" || die "refusal left scratch litter: $(ls "$SPOOL/.tmp")"
ok "refusal paths leave no litter in .tmp"

rm "$MB_A/msg/000000000000000005"
bx a read || die "read after prune"
printf 'still fine' | diff - "$OUT" >/dev/null || die "stream did not resume: $(cat "$OUT")"
ok "after rm, stream resumes past the gap"

# An empty mailbox is 2, not 0 and never 1. A caller must be able to
# tell "nothing to do" from "your command was wrong" and from "beb
# refused", without parsing a word of prose: prose on stderr is exactly
# what a caller filtering with head is most likely to have discarded.
bx a read
test $? -eq 2 || die "empty backlog did not exit 2"
test -s "$OUT" && die "empty backlog printed to stdout"
grep -q "no new mail" "$ERR" || die "empty backlog message"
ok "empty backlog exits 2: nothing to do, said so on stderr"

bx a peek 999 && die "missing id accepted"
grep -q "no message 999" "$ERR" || die "missing id refusal"
ok "inspect of missing id refuses"

# A verb names its effect: read never inspects, peek never consumes.
bx a read 4 >"$OUT" 2>"$ERR" && die "read with an id accepted"
grep -q "beb peek ID" "$ERR" || die "read refusal names peek: $(cat "$ERR")"
CUR=$(cat "$(mbox a)/cursor")
bx a peek 4 >/dev/null 2>&1 || die "peek 4 again"
test "$(cat "$(mbox a)/cursor")" = "$CUR" || die "peek moved the cursor"
ok "read takes no id and peek moves no cursor: the verb is the effect"

# The header states the cursor every time, on stderr, before the rows.
# Absent, it was ambiguous between an old build, an empty listing and a
# truncated read; on stdout it would corrupt `beb list | wc -l`; behind
# the rows it is what a head or a display limit throws away first.
bx a list --after 0 --limit 50 || die "full list"
grep -q '^beb: showing [0-9]*; cursor at [0-9]*' "$ERR" ||
    die "full list header: $(cat "$ERR")"
grep -q 'cursor at' "$OUT" && die "the header landed on stdout, where it would be counted as a message"
# Exit is 0 or 2 depending on whether anything was unread; the header
# is printed either way, which is the whole point of it.
bx a list --unread --limit 50
grep -q '^beb: showing [0-9]*; cursor at [0-9]*' "$ERR" || die "list header: $(cat "$ERR")"
CUR_NOW=$(cat "$(mbox a)/cursor")
grep -q "cursor at $CUR_NOW" "$ERR" || die "the header does not state the real cursor: $(cat "$ERR")"
UNREAD=$(grep -c . "$OUT")
grep -q "showing $UNREAD" "$ERR" || die "shown count disagrees with the rows: $(cat "$ERR")"
ok "list states the cursor and the counts, on stderr, ahead of the rows"

# The header must precede the listing in a merged stream, so a caller
# who pipes both through head still learns where the cursor is.
FIRST=$(pin a list --after 0 --limit 50 2>&1 | head -1)
case "$FIRST" in "beb: showing "*) ;; *) die "merged list does not lead with the header: $FIRST" ;; esac
ok "a merged list leads with the header, so truncation cannot eat it"

# A mailbox is unbounded and an agent's context is not, so a listing has
# to be a window. It used to default to ten rows from the cursor, which
# bounded the flood and hid the bound: a caller that never named a limit
# has no reason to look for one, and read ten rows of twenty-five as the
# whole mailbox. So the limit is required, and so is the direction --
# --unread is the set the default used to mean without saying so, while
# --after and --before reach mail already read.
mkdir -p "$W/pg" && (cd "$W/pg" && "$BEB" init pg) >"$HOME/pg.out" 2>/dev/null || die "init pg"
PG=$(cat "$HOME/pg.out")
for i in $(seq 1 14); do bx a send "$PG" --subject "m$i" --body b || die "send m$i"; done
env BEB_IDENTITY="$W/pg" "$BEB" read >/dev/null 2>&1 || die "read one"

bx pg list && die "a listing with no selector and no limit was served"
grep -q 'needs to say which' "$ERR" || die "bare list refusal: $(cat "$ERR")"
bx pg list --unread && die "a listing with no limit was served"
grep -q 'needs --limit' "$ERR" || die "missing-limit refusal: $(cat "$ERR")"
ok "a listing says which rows and how many, or it is refused"

bx pg list --unread --limit 10 || die "paged list"
test "$(grep -c . "$OUT")" = 10 || die "--limit 10 gave $(grep -c . "$OUT") rows"
head -1 "$OUT" | grep -q '^ *14  ' || die "the page does not start at the newest: $(head -1 "$OUT")"
grep -q '^beb: showing 10, more; cursor at 1; read next is 2$' "$ERR" || die "paged header: $(cat "$ERR")"
# Newest first, so the row `read` will hand over next is the one thing a
# listing cannot show. The header names it.
# A window that does not say it is a window reads as the whole. The
# counts are gone -- they cost a full scan -- but "more waiting" is one
# stat, and it is the part a consumer carrying a single line needs.
# The header carries no totals. Counting everything a mailbox holds, and
# everything above the cursor, cost a full directory read on every
# listing -- 292ms at 200k messages -- to print two numbers nobody acts
# on. Where the window is, and whether there is more, are what a pager
# needs, and the hints below say the second as a command.
grep -q 'total' "$ERR" && die "the header still counts the whole mailbox"
# The unread view offers one direction. Its top row is the newest
# message there is, so there is no "newer", and paging below the cursor
# would leave the set that was asked for.
grep -q '^beb: older: beb list --before 5 --limit 10$' "$ERR" || die "no older hint: $(cat "$ERR")"
grep -q 'newer:' "$ERR" && die "the unread view offered to page above the newest message"
ok "list pages from the cursor forward, says how many it showed, and names the next page"

# The header is what makes a window safe to print: a paged listing that
# did not say it was paged would read as the whole, and an agent would
# act on a tenth of its mail believing it had seen all of it.
bx pg list --unread --limit 3 || die "list --limit 3"
test "$(grep -c . "$OUT")" = 3 || die "--limit 3 gave $(grep -c . "$OUT") rows"
grep -q 'showing 3, more' "$ERR" || die "limit header: $(cat "$ERR")"
ok "--limit narrows the window and the header says how narrow"

# An explicit -f is a request, not a filter, so already-read messages
# are in range.
bx pg list --after 0 --limit 2 || die "list --after 0"
tail -1 "$OUT" | grep -q '^1  ' || die "-f did not reach a consumed message: $(cat "$OUT")"
ok "-f reaches back past the cursor into what was already read"

# Zero was "no limit", which is the one count arithmetic can produce and
# the one that returns the whole mailbox. Every other bad count refused;
# this one was obeyed.
bx pg list --unread --limit 0 && die "--limit 0 accepted"
grep -q 'not a count: "0"' "$ERR" || die "--limit 0 refusal: $(cat "$ERR")"
ok "--limit 0 is not a count, the way -1 and abc are not"

bx pg list --after 999 --limit 5
test $? -eq 2 || die "a window past the end did not exit 2"
grep -q 'showing 0' "$ERR" || die "empty window header: $(cat "$ERR")"
ok "a window past the end shows nothing and exits 2"

# --all was a second way to say -f 1 -n 0, and it needed its own rule
# for what it meant beside a window. The refusal names what replaced it.
bx pg list --all --limit 5 && die "--all accepted"
grep -q 'page with --after 0 --limit N' "$ERR" ||
    die "--all refusal does not name its replacement: $(cat "$ERR")"
bx pg list --before 0 --limit 5 && die "--before 0 accepted"
grep -q 'not a message id: "0"' "$ERR" || die "-f 0 refusal: $(cat "$ERR")"
bx pg list --after && die "--after with no value accepted"
grep -q -- '--after needs an id' "$ERR" || die "dangling --after: $(cat "$ERR")"
bx pg list --sideways --limit 5 && die "unknown list option accepted"
grep -q 'list has no option "--sideways"' "$ERR" || die "unknown list option: $(cat "$ERR")"
# Exclusive cursors, so a caller pages by handing back an id it was just
# shown: the last row to walk forward, the first row to walk back.
# Nothing is computed, which is what makes gaps harmless -- `--from`
# arithmetic returned 7 rows for a 10-row request the moment a carrier
# had pruned three ids out of the range.
# Anchored at the start of the mailbox, because the unread view begins
# at the newest and has nowhere forward to walk.
bx pg list --after 0 --limit 4 || die "first page"
LAST=$(head -1 "$OUT" | awk '{print $1}')
FIRST=$(tail -1 "$OUT" | awk '{print $1}')
bx pg list --after "$LAST" --limit 4 || die "--after next page"
grep -qE "^ *$LAST  " "$OUT" && die "--after repeated the id it was given: $(cat "$OUT")"
NEXT_FIRST=$(tail -1 "$OUT" | awk '{print $1}')
test "$NEXT_FIRST" -gt "$LAST" || die "--after went backwards"
ok "--after excludes its id and walks forward from a row you were shown"

# Anchored where history exists: the sixth id in the mailbox always has
# at least five below it.
FIRST=$(pin pg list --after 0 --limit 50 2>/dev/null | awk '{print $1}' | sort -n | sed -n '6p')
bx pg list --before "$FIRST" --limit 4 || die "--before previous page"
grep -qE "^ *$FIRST  " "$OUT" && die "--before repeated the id it was given"
PREV_LAST=$(head -1 "$OUT" | awk '{print $1}')
test "$PREV_LAST" -lt "$FIRST" || die "--before went forwards"
test "$(grep -c . "$OUT")" = 4 || die "--before gave $(grep -c . "$OUT") rows, want the 4 nearest"
ok "--before excludes its id and takes the N nearest below it"

# Rows print newest first whichever end they came from: the boundary
# chooses which rows, never their order.
sort -rn -c "$OUT" 2>/dev/null || die "a --before page did not print descending"
ok "a backward page still prints newest first"

# An exclusive cursor can name every interior boundary and neither end.
# Ids start at 1, so --after 0 is the only way to say "from the start";
# --before 0 names nothing and refuses.
bx pg list --after 0 --limit 2 || die "--after 0"
tail -1 "$OUT" | grep -qE '^ *1  ' || die "--after 0 did not reach the beginning: $(cat "$OUT")"
bx pg list --before 0 --limit 5 && die "--before 0 accepted"
grep -q 'not a message id: "0"' "$ERR" || die "--before 0 refusal: $(cat "$ERR")"
ok "--after 0 names the start of the mailbox; --before 0 names nothing"

bx pg list --after 5 --before 9 && die "two boundaries accepted"
grep -q 'takes one of --unread, --after, --before' "$ERR" || die "two-boundary refusal: $(cat "$ERR")"
bx pg list --unread --before 9 --limit 2 && die "--unread beside a boundary accepted"
grep -q 'takes one of --unread, --after, --before' "$ERR" || die "mixed-selector refusal: $(cat "$ERR")"
bx pg list --from 1 --limit 5 && die "--from accepted"
grep -q 'list has no --from; --after ID pages forward and --before ID back' "$ERR" ||
    die "--from refusal does not name what replaced it: $(cat "$ERR")"
ok "one boundary at a time, and --from names the pair that replaced it"

# Every other verb names the next step; list printed a window and said
# nothing about how to move it, though the ids to move it with were in
# the rows. An agent paging cold inferred both boundaries correctly and
# still reported that the output did not tell it what to do next.
# A navigation window, which is the one that can offer both directions.
bx pg list --before 9 --limit 3 || die "paging hints"
PG_LAST=$(grep -E '^ *[0-9]' "$OUT" | head -1 | awk '{print $1}')
PG_FIRST=$(grep -E '^ *[0-9]' "$OUT" | tail -1 | awk '{print $1}')
grep -q "^beb: newer: beb list --after $PG_LAST --limit 3\$" "$ERR" || die "no forward hint: $(cat "$ERR")"
grep -q "^beb: older: beb list --before $PG_FIRST --limit 3\$" "$ERR" || die "no backward hint: $(cat "$ERR")"
# and the commands it prints run as printed
eval "pin pg $(sed -n 's/^beb: newer: beb //p' "$ERR")" >/dev/null 2>&1 || die "the newer hint did not run"
eval "pin pg $(sed -n 's/^beb: older: beb //p' "$ERR")" >/dev/null 2>&1 || die "the older hint did not run"
ok "list names both ways out of the window, and both commands run as printed"

# Nothing to offer when the window is the whole mailbox: an offer to page
# would be an offer to see the same rows again.
bx pg list --after 0 --limit 50 || die "full listing"
grep -qE '^beb: (newer|older):' "$ERR" && die "offered to page a listing that showed everything: $(cat "$ERR")"
ok "a listing that shows everything offers no paging"

ok "list names every way its window can be got wrong"

# Every command beb prints has to run as printed. peek's refusal named
# --all after --all was removed, which is advice that refuses.
bx pg peek 999 && die "missing id accepted"
ADVICE=$(sed -n 's/^beb: no message 999; \(.*\) shows what exists$/\1/p' "$ERR")
test -n "$ADVICE" || die "peek refusal shape changed: $(cat "$ERR")"
eval "pin pg ${ADVICE#beb }" >/dev/null 2>"$ERR" || die "the command peek names was refused: $(cat "$ERR")"
ok "the listing peek points at is a command that runs"

bx a list --after 0 --limit 50 || die "full list again"
grep -qE '^1 .* b$' "$OUT" || die "--all hides consumed"
bx a list --unread --limit 5
test $? -eq 2 || die "a fully consumed mailbox did not list as nothing to do"
test -s "$OUT" && die "default list shows consumed"
ok "-f 1 -n 0 shows history, the default shows unread only"

# ---- recipient binding and foreign types -------------------------------

mkid d >/dev/null || die "init d"
D=$(addr d)
mkid e >/dev/null || die "init e"
E=$(addr e)

bx d send e --subject "for e" --body "for e only" || die "send d->e"
MB_D=$(mbox d)
MB_E=$(mbox e)
cp "$MB_E/msg/000000000000000001" "$MB_D/msg/000000000000000090"
printf '90' >"$MB_D/.counter"

bx d read && die "misaddressed message consumed"
grep -q "someone else" "$ERR" || die "wrong-to refusal text"
grep -q "msg/000000000000000090" "$ERR" || die "wrong-to refusal names the message"
ok "consume refuses a valid message addressed elsewhere"

bx d peek 90 && die "misaddressed message inspected"
grep -q "someone else" "$ERR" || die "wrong-to inspect refusal"
ok "inspect refuses it too"

bx d list --unread --limit 50 || die "list after a binding refusal"
grep -q '^90  ' "$OUT" || die "cursor advanced past misaddressed"
ok "cursor unmoved by binding refusal"

rm "$MB_D/msg/000000000000000090"
bx e send d --subject "legit" --body "legit mail" || die "send e->d"
bx d read || die "read after removing misaddressed"
printf 'legit mail' | diff - "$OUT" >/dev/null || die "resume body"
ok "after rm, d's stream resumes"

ssh-keygen -t ecdsa -N "" -q -C "" -f "$HOME/ec" || die "ecdsa keygen"
EC=$(cat "$HOME/ec.pub")
bx d send "$EC" --subject t --body nope && die "ecdsa recipient accepted"
grep -q "ecdsa" "$ERR" || die "ecdsa refusal names the type"
grep -q "ssh-ed25519 only" "$ERR" || die "ecdsa refusal names the protocol"
ok "foreign recipient key type refused, type named"

ENVF=$HOME/intruder
printf 'from: %s\nto: %s\nnonce: AAAAAAAAAAAAAAAAAAAAAA==\ndate: 2026-08-15T02:26:34Z\nsubject: intruding\n\nintruder' \
    "$(awk '{print $1" "$2}' "$HOME/ec.pub")" "$D" >"$ENVF"
ssh-keygen -Y sign -n beb -f "$HOME/ec" "$ENVF" 2>/dev/null || die "manual ecdsa sign"
NEXT_D=$(printf '%018d' $(( $(cat "$MB_D/.counter") + 1 )))
plant "$MB_D" "$NEXT_D" "$ENVF" "$ENVF.sig"
bx d read && die "foreign-from envelope consumed"
grep -q "non-ed25519" "$ERR" || die "foreign-from refusal text"
grep -q "rm '" "$ERR" || die "foreign-from refusal names rm"
ok "validly signed non-ed25519 from: refused"
rm "$MB_D/msg/$NEXT_D"

# ---- wait: block until there is something to read ----------------------

mkid w1 >/dev/null || die "init w1"
W1=$(addr w1)
mkid w2 >/dev/null || die "init w2"

# Nothing unread: it blocks, and wakes on arrival.
(sleep 1 && pin w2 send "$W1" --subject "wake up" --body "body" >/dev/null 2>&1) &
t0=$(date +%s)
(pin w1 wait --timeout 15) >"$OUT" 2>"$ERR" || die "wait did not return on arrival"
t1=$(date +%s)
test $((t1 - t0)) -lt 10 || die "wait took $((t1 - t0))s; not event-driven"
grep -qx '[0-9]+' "$OUT" 2>/dev/null || grep -qE '^[0-9]+$' "$OUT" ||
    die "wait did not hand back a mark: $(cat "$OUT")"
grep -q '^beb: mail arrived; next mark [0-9]*$' "$ERR" || die "arrival receipt: $(cat "$ERR")"
ok "wait blocks with nothing unread and wakes on arrival"

# Something unread: it returns at once. No --timeout, so a wait that did
# not return would hang here rather than fail quietly.
t0=$(date +%s)
(pin w1 wait) >"$OUT" 2>"$ERR" || die "wait did not return with unread mail in front of it"
t1=$(date +%s)
test $((t1 - t0)) -le 2 || die "wait took $((t1 - t0))s to notice mail it already had"
grep -q '^beb: mail is waiting; next mark [0-9]*$' "$ERR" || die "standing receipt: $(cat "$ERR")"
ok "wait returns at once when mail is already unread"

# Which makes the obvious worker loop correct: a message arriving while
# the handler is busy is unread at the next wait, so it returns rather
# than blocking for the message after it.
mkid lp >/dev/null || die "init lp"
LP=$(addr lp)
(pin a send "$LP" --subject "job 1" --body one >/dev/null 2>&1
 sleep 1
 pin a send "$LP" --subject "job 2" --body two >/dev/null 2>&1) &
handled=0
while pin lp wait --timeout 4 >/dev/null 2>&1; do
    pin lp read >/dev/null 2>&1 || break
    handled=$((handled + 1))
    sleep 1.5                       # a handler that takes a moment
    [ "$handled" -ge 5 ] && break
done
wait
test "$handled" = 2 || die "the worker loop handled $handled of 2 messages"
pin lp list --unread --limit 50 >"$OUT" 2>"$ERR"
grep -q 'showing 0;' "$ERR" || die "the loop left mail unread: $(cat "$ERR")"
ok "while beb wait; do beb read; done drains without stalling"

# --from names a mark of the caller's own, for a waiter that must not
# fire again for mail it has already acted on. The cursor cannot serve:
# it belongs to whoever runs `read`, and a doorbell never reads.
LP_LAST=$(pin lp list --after 0 --limit 50 2>/dev/null | head -1 | awk '{print $1}')
(pin lp wait --from "$LP_LAST" --timeout 1) >"$OUT" 2>"$ERR" ||
    die "--from at an existing id did not return"
(pin lp wait --from $((LP_LAST + 1)) --timeout 1) >"$OUT" 2>"$ERR" &&
    die "--from past the end returned"
grep -q 'nothing arrived in 1s' "$ERR" || die "--from timeout receipt: $(cat "$ERR")"
ok "--from waits from a caller's mark instead of the cursor"

# A mark closes the gap between calls. Anything that lands while nobody
# is waiting is still above the mark when the next call arms.
mkid gp >/dev/null || die "init gp"
GP=$(addr gp)
(pin gp wait --from 1 --timeout 1) >"$OUT" 2>"$ERR" && die "wait fired on an empty mailbox"
pin a send "$GP" --subject "in the gap" --body x >/dev/null 2>&1 || die "send into the gap"
(pin gp wait --from 1 --timeout 2) >"$OUT" 2>"$ERR" ||
    die "a message that landed between calls was missed"
ok "a mark survives between calls: nothing falls into the gap"

# wait hands the mark back on stdout. Without it a caller went `beb list
# --from 1 --limit 0 | tail -1 | awk` -- parsing a listing meant for
# people to recover a number beb already had.
mkid mk >/dev/null || die "init mk"
MK=$(addr mk)
pin mk wait --timeout 0 >"$OUT" 2>"$ERR"
test $? -eq 2 || die "poll on an empty mailbox did not exit 2"
test "$(cat "$OUT")" = 1 || die "an empty mailbox handed back mark $(cat "$OUT"), want 1"
grep -q "next mark $(cat "$OUT")\$" "$ERR" ||
    die "the receipt does not name the mark it printed: $(cat "$OUT") vs $(cat "$ERR")"
ok "wait prints the mark, names it in the receipt, and does both on a timeout"

M=$(cat "$OUT")
pin a send "$MK" --subject "one" --body x >/dev/null 2>&1 || die "send one"
M2=$(pin mk wait --from "$M" --timeout 2 2>/dev/null) || die "wait --from did not fire"
test "$M2" = 2 || die "mark after one message is $M2, want 2"
# The mark it handed back must not fire again for what it already named.
pin mk wait --from "$M2" --timeout 1 >"$OUT" 2>"$ERR" &&
    die "the returned mark fired again for mail already announced"
test "$(cat "$OUT")" = 2 || die "an unchanged mailbox moved the mark"
ok "the mark it returns does not fire again: a doorbell cannot wake-loop"

# And it composes into the whole loop without any list parsing.
pin a send "$MK" --subject "two" --body x >/dev/null 2>&1 || die "send two"
M3=$(pin mk wait --from "$M2" --timeout 2 2>/dev/null) || die "second wait did not fire"
test "$M3" = 3 || die "mark after two messages is $M3, want 3"
# Nothing was read: the doorbell never touches the cursor.
test "$(cat "$(mbox mk)/cursor")" = 0 || die "waiting moved the cursor"
ok "mark, ring, mark: the loop needs no list and never moves the cursor"

mkid w3 >/dev/null || die "init w3"
(pin w3 wait --timeout 1) >"$OUT" 2>"$ERR"
test $? -eq 2 || die "a timeout did not exit 2"
grep -q '^beb: nothing arrived in 1s; cursor at 0; next mark [0-9]*$' "$ERR" || die "timeout receipt: $(cat "$ERR")"
ok "a timeout exits 2 and names the cursor"

pin w3 wait --from 0 >"$OUT" 2>"$ERR" && die "--from 0 accepted"
grep -q 'not a message id: "0"' "$ERR" || die "--from 0 refusal: $(cat "$ERR")"
pin w3 wait --from >"$OUT" 2>"$ERR" && die "--from with no value accepted"
grep -q -- '--from needs a value' "$ERR" || die "dangling --after: $(cat "$ERR")"
pin w3 wait --sideways >"$OUT" 2>"$ERR" && die "unknown wait option accepted"
grep -q 'wait has no option "--sideways"' "$ERR" || die "unknown wait option: $(cat "$ERR")"
pin w3 wait -t 1 >"$OUT" 2>"$ERR" && die "-t accepted"
grep -q 'wait has no -t; the option is --timeout' "$ERR" || die "-t refusal: $(cat "$ERR")"
ok "wait names every way its arguments can be got wrong"

bx nobody wait && die "wait without identity succeeded"
grep -q "beb init" "$ERR" || die "wait refusal names the fix"
ok "wait refuses without an identity"

# ---- BEB_IDENTITY: the only source -------------------------------------

bx a whoami || die "pinned identity failed"
test "$(cat "$OUT")" = "$A" || die "pin resolved the wrong key"
ok "the pin names the identity, from any working directory"

# The pin is a path, and identity is what lives at it. A copy of a
# .beb at another path is the same identity, because one public key is
# one identity wherever its directory is made available.
mkdir -p "$W/a-twin" && cp -R "$W/a/.beb" "$W/a-twin/.beb"
bx a-twin whoami || die "twin refused"
test "$(cat "$OUT")" = "$A" || die "twin resolved a different key: $(cat "$OUT")"
ok "the same .beb at another path is the same identity"

# Standing anywhere at all changes nothing: there is no second claimant
# left for a working directory to be.
(cd "$W/b" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" ||
    die "a cwd identity interfered with the pin: $(cat "$ERR")"
test "$(cat "$OUT")" = "$A" || die "cwd won over the pin: $(cat "$OUT")"
ok "a .beb in the working directory is not consulted, even a valid one"

mkdir -p "$W/nobeb"
bx nobeb whoami && die "a pin at a directory with no .beb resolved"
grep -q "has no .beb" "$ERR" || die "absent refusal text: $(cat "$ERR")"
grep -q "beb init" "$ERR" || die "absent refusal names beb init"
ok "a pin with no .beb refuses as absent, naming beb init"

# A broken claim is not an absent one, and neither is passed over.
mkdir -p "$W/cracked" && cp -R "$W/a/.beb" "$W/cracked/.beb" && printf 'garbage' >"$W/cracked/.beb/id_ed25519.pub"
bx cracked whoami && die "broken identity resolved"
grep -q "broken identity" "$ERR" || die "broken refusal text: $(cat "$ERR")"
grep -q "has no .beb" "$ERR" && die "broken mislabeled as absent"
ok "a broken pin refuses as broken, keeping its own reason"

# A .beb whose private key is gone is a damaged claim, not an absent one.
mkdir -p "$W/keyless" && cp -R "$W/a/.beb" "$W/keyless/.beb" && rm "$W/keyless/.beb/id_ed25519"
bx keyless whoami && die "keyless identity resolved"
grep -q "id_ed25519 is missing" "$ERR" || die "keyless refusal text: $(cat "$ERR")"
ok "a missing private key refuses as broken, not absent"

# An empty pin is an unset one, not a pin at the working directory.
(cd "$W/a" && BEB_IDENTITY= "$BEB" whoami) >"$OUT" 2>"$ERR" && die "empty BEB_IDENTITY resolved"
grep -q "BEB_IDENTITY is not set" "$ERR" || die "empty pin refusal: $(cat "$ERR")"
ok "an empty BEB_IDENTITY is unset, never a fallback to cwd"

# ---- a carried .beb: init claims, and never clobbers --------------------

# A .beb copied to another machine has no mailbox in that machine's
# spool. Until 0.5.3 that was a dead end with no exit: whoami answered,
# read said "no new mail" as though the mailbox were merely empty, and a
# delivery for the key was refused with "its owner claims one with: beb
# init" -- which init answered with "rm -r .beb", i.e. delete your
# private key. The only followable instruction destroyed the identity.
mkdir -p "$W/csrc" && (cd "$W/csrc" && "$BEB" init csrc) >"$HOME/cs.out" 2>/dev/null || die "init csrc"
CS=$(cat "$HOME/cs.out")
CS_MB="$SPOOL/$(keyhex "$CS")"
mkdir -p "$W/carried" && cp -R "$W/csrc/.beb" "$W/carried/.beb"
rm -rf "$CS_MB"                                  # as if this key had never lived here

# Reading an unclaimed mailbox said "no new mail" -- untrue, since
# there was no mailbox -- and wrote a cursor as a side effect of
# consuming, claiming it silently. That made the sixth guarantee false,
# and a transport reads exactly that bit to decide what it may carry.
env BEB_IDENTITY="$W/carried" "$BEB" read >"$OUT" 2>"$ERR"
test $? -eq 1 || die "an unclaimed mailbox did not refuse: $(cat "$ERR")"
grep -q 'no mailbox claimed here for this identity' "$ERR" || die "unclaimed read: $(cat "$ERR")"
grep -q 'beb init claims one' "$ERR" || die "unclaimed read does not name init: $(cat "$ERR")"
test -e "$CS_MB/cursor" && die "a refused read claimed the mailbox anyway"
env BEB_IDENTITY="$W/carried" "$BEB" list >"$OUT" 2>"$ERR"
test $? -eq 1 || die "list on an unclaimed mailbox did not refuse"
env BEB_IDENTITY="$W/carried" "$BEB" peek 1 >"$OUT" 2>"$ERR"
test $? -eq 1 || die "peek on an unclaimed mailbox did not refuse"
test -e "$CS_MB/cursor" && die "a reading verb claimed the mailbox"

# Mail can predate the claim: another identity here may have sent to the
# key before anybody could read it.
bx a send "$CS" --subject "predates" --body "predates the claim" || die "send to an unclaimed key failed"
test -e "$CS_MB/cursor" && die "send claimed a mailbox for an absent owner"

(cd "$W/carried" && "$BEB" init carried) >"$OUT" 2>"$ERR" ||
    die "init refused to claim a mailbox for a .beb it did not create: $(cat "$ERR")"
grep -q '^beb: claimed mailbox .* for the .beb already here, cursor at 0$' "$ERR" ||
    die "adoption ack: $(cat "$ERR")"
test "$(cat "$OUT")" = "$CS" || die "adoption reported a different key"
cmp -s "$W/carried/.beb/id_ed25519" "$W/csrc/.beb/id_ed25519" ||
    die "init overwrote the private key it was supposed to adopt"
test -f "$CS_MB/cursor" || die "adoption wrote no cursor"
# Mail sent before the claim went to the outbox, queued to leave. The
# claim takes it back: its recipient now reads here, so shipping it out
# would carry it away from the one machine that can deliver it.
grep -q '^beb: 1 already waiting, 1 taken back from the outbox; beb list shows them$' "$ERR" ||
    die "adoption did not take back the mail queued to leave: $(cat "$ERR")"
test "$(ls "$SPOOL/outbox" 2>/dev/null | grep -c .)" = 0 ||
    die "the claimed mail is still queued to leave"
ok "init claims a mailbox for a .beb already here, keeps the key, names waiting mail"

# The mail it named is readable, which is the whole point of claiming.
env BEB_IDENTITY="$W/carried" "$BEB" read >"$OUT" 2>"$ERR" || die "read after adoption: $(cat "$ERR")"
printf 'predates the claim' | diff - "$OUT" >/dev/null || die "adopted mail did not read back: $(cat "$OUT")"
ok "mail that predated the claim reads after it"
MBOX_COUNT=$(ls "$SPOOL" | wc -l | tr -d ' ')

(cd "$W/carried" && "$BEB" init carried) >"$OUT" 2>"$ERR" && die "second init on a claimed mailbox succeeded"
grep -q 'already an identity here, and its mailbox is claimed' "$ERR" || die "reclaim refusal: $(cat "$ERR")"
ok "a claimed mailbox refuses init again"

# beb never asks where a .beb came from; it asks whether this key has a
# cursor in this spool. A copy beside its original is therefore refused
# -- one key is one mailbox, and that mailbox is already claimed -- so
# adoption can never mint a second mailbox for a key that has one.
mkdir -p "$W/sidecopy" && cp -R "$W/csrc/.beb" "$W/sidecopy/.beb"
(cd "$W/sidecopy" && "$BEB" init sidecopy) >"$OUT" 2>"$ERR" &&
    die "init adopted a copy whose mailbox is already claimed"
grep -q 'already an identity here, and its mailbox is claimed' "$ERR" ||
    die "side-copy refusal: $(cat "$ERR")"
test "$(ls "$SPOOL" | wc -l | tr -d ' ')" = "$MBOX_COUNT" ||
    die "a refused adoption changed the number of mailboxes"
ok "a .beb beside its original is refused: one key is one mailbox"

# A spool this key has never been claimed in adopts it, wherever the
# .beb has been. Provenance is not the test and cannot be.
env XDG_DATA_HOME="$HOME/spool2" sh -c "cd '$W/csrc' && '$BEB' init csrc" >"$OUT" 2>"$ERR" ||
    die "init did not claim in a second spool: $(cat "$ERR")"
grep -q '^beb: claimed mailbox .* cursor at 0$' "$ERR" || die "second-spool ack: $(cat "$ERR")"
test "$(ls "$HOME/spool2/beb" | wc -l | tr -d ' ')" = 1 || die "second spool has no mailbox"
ok "the same key claims in a spool that has never seen it"

# drop admits a resident only, and residency is now the plainest thing
# in the spool: a mailbox directory exists. It used to be a cursor file,
# because a local send to a key living elsewhere created the directory --
# one outbound message to a stranger opened this machine to unbounded
# inbound deliveries addressed to them. Outbound mail no longer creates
# anything for its recipient, so the directory means what it looks like.
mkdir -p "$W/ek" && (cd "$W/ek" && "$BEB" init ek) >"$HOME/ek.out" 2>/dev/null || die "init ek"
EK=$(cat "$HOME/ek.out")
EK_MB="$SPOOL/$(keyhex "$EK")"
env BEB_IDENTITY="$W/ek" "$BEB" pack "$EK" --subject "from outside" --body "body" >"$HOME/stranger2.mbeb" 2>/dev/null
rm -rf "$EK_MB"                                  # as if that key lived elsewhere
bx a send "$EK" --subject "outbound" --body "body" || die "send to an absent key failed"
test -e "$EK_MB" && die "outbound mail created a mailbox for a key that reads elsewhere"
"$BEB" drop <"$HOME/stranger2.mbeb" >"$OUT" 2>"$ERR"
test $? -eq 3 || die "drop admitted a delivery for a key that does not read here"
grep -q "no mailbox here for" "$ERR" || die "unclaimed drop refusal: $(cat "$ERR")"
ok "drop admits a resident only: an outbox is not an open door"

# ---- exit codes: the distinction prose cannot carry --------------------

# 0 did it, 1 change the invocation, 2 nothing to do, 3 refused. A reader
# that cannot tell an empty mailbox from a message that failed
# verification has a security failure rather than an inconvenience, and
# stderr is exactly the stream a caller filtering with head discards.
code() { local want=$1 what=$2; shift 2; "$@" >/dev/null 2>&1; local got=$?
    test "$got" = "$want" || die "exit code: $what gave $got, want $want"; }

code 0 "--help"                    "$BEB" --help
code 0 "whoami pinned"             env BEB_IDENTITY="$W/a" "$BEB" whoami
code 1 "unknown verb"              "$BEB" frobnicate
code 1 "bare read argument"        env BEB_IDENTITY="$W/a" "$BEB" read 4
code 1 "no pin"                    env -u BEB_IDENTITY "$BEB" whoami
code 1 "pin without .beb"          env BEB_IDENTITY="$W/nobody" "$BEB" whoami
code 1 "init with no name"         env -u BEB_IDENTITY "$BEB" init
code 1 "init with a path-shaped name" env -u BEB_IDENTITY "$BEB" init sub/
code 1 "unknown roster name"       env BEB_IDENTITY="$W/a" "$BEB" send nosuch --subject t --body hi
code 2 "wait timeout"              env BEB_IDENTITY="$W/a" "$BEB" wait --timeout 1
code 3 "already an identity"       sh -c "cd '$W/a' && '$BEB' init a"
code 3 "a name already taken"      sh -c "cd '$W/nobody' && '$BEB' init b"
ok "exit codes: 0 did it, 1 fix the command, 2 nothing to do, 3 refused"

# 3 must never collapse into 2. This is the pair the table exists for:
# an empty mailbox and a message that will not verify are both "read
# printed no body", and only the code tells them apart.
MB_A=$(mbox a)
env BEB_IDENTITY="$W/a" "$BEB" read >/dev/null 2>&1
test $? -eq 2 || die "a drained mailbox did not exit 2"
# The corrupt message has to sit above the cursor, or read skips it and
# the mailbox is merely empty again -- which is the very confusion the
# code exists to prevent.
NEXT=$(printf '%018d' $(( $(cat "$MB_A/cursor") + 1 )))
printf 'garbage' >"$MB_A/msg/$NEXT"
# Above the counter is not looked for, so a fixture has to raise it the
# way a delivery would.
cur=$(cat "$MB_A/.counter" 2>/dev/null || echo 0)
[ "$((10#$NEXT))" -gt "$((10#$cur))" ] && printf '%s' "$((10#$NEXT))" >"$MB_A/.counter"
env BEB_IDENTITY="$W/a" "$BEB" read >"$OUT" 2>"$ERR"
test $? -eq 3 || die "a corrupt message did not exit 3: $(cat "$ERR")"
test -s "$OUT" && die "a refused read printed a body"
rm -f "$MB_A/msg/$NEXT"
ok "3 never collapses into 2: empty and unverifiable are different numbers"

# ---- 40 parallel senders: the flock is load-bearing --------------------

mkid p >/dev/null || die "init p"
P=$(addr p)
mkid q >/dev/null || die "init q"
for i in $(seq 1 40); do
    (pin q send "$P" --subject "msg $i" --body "body $i") >/dev/null 2>&1 &
done
wait
MB_P=$(mbox p)
test "$(ls "$MB_P/msg" | wc -l | tr -d ' ')" = 40 || die "parallel: $(ls "$MB_P/msg" | wc -l) messages, want 40"
ls "$MB_P/msg" | sort | tail -1 | grep -qx '000000000000000040' || die "parallel: ids not 1..40"
# A frame carries its signature or it is not a frame, so "every message
# has one" is no longer a set comparison -- it is whether each one reads.
for f in "$MB_P"/msg/*; do
    head -c 4 "$f" | grep -qx 'beb ' || die "parallel: $f is not a frame"
done
ok "40 parallel senders: 40 messages, ids 1..40, no reuse, every one a whole frame"

# ---- concurrent readers: consumption is serialized too -----------------

# Delivery has always been locked; consumption needs the same lock. A
# cursor read before another reader's write and set after it moves
# backwards, and the message in between is handed out twice.

mkid rs >/dev/null || die "init rs"
mkid rr >/dev/null || die "init rr"
RR=$(addr rr)
for i in 1 2 3 4 5 6; do
    (pin rs send "$RR" --subject "t$i" --body "body-$i") >/dev/null 2>"$ERR" || die "send body-$i"
done
for i in 1 2 3 4 5 6; do
    (pin rr read) >"$HOME/rd.$i.out" 2>"$HOME/rd.$i.err" &
done
wait
for i in 1 2 3 4 5 6; do
    test -s "$HOME/rd.$i.out" || die "concurrent reader $i got nothing: $(cat "$HOME/rd.$i.err")"
done
for i in 1 2 3 4 5 6; do cat "$HOME/rd.$i.out"; echo; done | sort >"$HOME/rd.all"
test "$(sort -u "$HOME/rd.all" | wc -l | tr -d ' ')" = 6 ||
    die "concurrent readers repeated a message: $(tr '\n' ' ' <"$HOME/rd.all")"
test "$(cat "$(mbox rr)/cursor")" = 6 || die "cursor after six concurrent reads: $(cat "$(mbox rr)/cursor")"
ok "6 concurrent readers: six distinct bodies, cursor at 6, never backwards"

# ---- portable delivery: pack | receive ---------------------------------

mkid m1 >/dev/null || die "init m1"
mkid m2 >/dev/null || die "init m2"
M2=$(addr m2)
mkid m3 >/dev/null || die "init m3"
echo "carrier $M2" >>"$KS"

(pin m1 pack carrier --subject "over the wall" --body "over the wall") >"$HOME/note.mbeb" 2>"$ERR" || die "pack failed"
test -s "$HOME/note.mbeb" || die "pack wrote nothing"
# pack said nothing at all until 0.6.0. Its artifact goes to stdout and
# almost always straight into a file, so a reader who redirected it saw
# no output and could not tell a delivery from an empty file without
# opening it. The size it reports is the whole frame, measured.
grep -q '^beb: packed for [^,]*, ".*"; [0-9]*-byte delivery$' "$ERR" || die "pack receipt: $(cat "$ERR")"
PACKED=$(sed -n 's/.*; \([0-9]*\)-byte delivery$/\1/p' "$ERR")
test "$PACKED" = "$(wc -c <"$HOME/note.mbeb" | tr -d ' ')" ||
    die "pack reported $PACKED bytes, file is $(wc -c <"$HOME/note.mbeb" | tr -d ' ')"
head -c 4 "$HOME/note.mbeb" | grep -q "^beb " || die "frame header shape"
ok "pack: silent success, frame on stdout"

test "$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')" = 0 || die "pack touched the recipient mailbox"
ok "pack delivers nothing"

# receive resolves no identity: the envelope carries its address, so a
# delivery installs from anywhere, even where no .beb exists.
(pin nobody drop <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "receive failed: $(cat "$ERR")"
# Nothing on stdout: the id names a message in a mailbox this process
# cannot open -- receive resolves no identity and never reads -- and
# beb-ssh proves it from the other side, inheriting stdout at both call
# sites and reading only the exit code. Until 0.6.0 this prose went to
# stdout unprefixed, where grep -v '^beb:' could not tell it from an
# artifact.
test -s "$OUT" && die "receive wrote to stdout: $(cat "$OUT")"
grep -q '^beb: accepted 1 for [^;]*; from [^,]*, ".*"$' "$ERR" || die "receive ack: $(cat "$ERR")"
(pin m2 read) >"$OUT" 2>"$ERR" || die "read received mail"
printf 'over the wall' | diff - "$OUT" >/dev/null || die "body across the wall: $(cat "$OUT")"
ok "receive installs by the envelope's address, needing no identity of its own"

# Standing in another identity never redirects a delivery: the address
# decides, not the reader.
(pin m1 pack carrier --subject "second" --body "second for m2") | (pin m3 drop) >"$OUT" 2>"$ERR" || die "receive from m3 failed: $(cat "$ERR")"
test "$(ls "$(mbox m3)/msg" | wc -l | tr -d ' ')" = 0 || die "delivery landed in the reader's mailbox"
test "$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')" = 2 || die "delivery did not land in the addressed mailbox"
(pin m2 read) >"$OUT" 2>"$ERR" || die "read second"
printf 'second for m2' | diff - "$OUT" >/dev/null || die "second body: $(cat "$OUT")"
ok "the address decides the mailbox, never the directory receive runs in"

# A mailbox that does not exist here is not conjured by mail arriving:
# residence is having run beb init, and a stranger is refused.
ssh-keygen -q -t ed25519 -N "" -C stranger -f "$HOME/stranger" </dev/null || die "keygen stranger"
echo "outsider $(awk '{print $1" "$2}' "$HOME/stranger.pub")" >>"$KS"
(pin m1 pack outsider --subject "nobody home" --body "nobody home") >"$HOME/stranger.mbeb" || die "pack outsider"
(pin m2 drop <"$HOME/stranger.mbeb") >"$OUT" 2>"$ERR" && die "delivery for a stranger accepted"
grep -q "no mailbox here" "$ERR" || die "stranger refusal text: $(cat "$ERR")"
grep -q "beb init" "$ERR" || die "stranger refusal names the fix: $(cat "$ERR")"
STRANGER_BOX=$(keyhex "$(awk '{print $1" "$2}' "$HOME/stranger.pub")")
test -d "$SPOOL/$STRANGER_BOX" && die "refused delivery minted a mailbox"
ok "no mailbox here: refused, nothing minted, the refusal names beb init"

# The frame's lengths are the sender's claim, and a claim is not a licence
# to spend the recipient's disk. Admission runs on the header prefix, in
# memory, so a delivery for a stranger writes nothing at all however much
# it announces it is about to send.
HDR=$(head -1 "$HOME/stranger.mbeb")
SL=$(printf '%s' "$HDR" | awk '{print $3}')
BEFORE=$(du -sk "$SPOOL" | awk '{print $1}')
{
    printf 'beb 500000000000 %s\n' "$SL"
    tail -c "+$((${#HDR} + 2))" "$HOME/stranger.mbeb"
    head -c 33554432 /dev/zero
} | (pin m2 drop) >"$OUT" 2>"$ERR" && die "unbounded delivery for a stranger accepted"
grep -q "no mailbox here" "$ERR" || die "unbounded stranger refusal text: $(cat "$ERR")"
AFTER=$(du -sk "$SPOOL" | awk '{print $1}')
test "$((AFTER - BEFORE))" -lt 1024 || die "refused delivery wrote $((AFTER - BEFORE))KB to the spool"
test -z "$(ls -A "$SPOOL/.tmp" 2>/dev/null)" || die "refused delivery left litter: $(ls "$SPOOL/.tmp")"
ok "a claimed 500GB envelope for a stranger: refused before a byte reaches disk"

# The envelope stays uncapped because a body is. A signature does not: an
# armored ed25519 signature is under 300 bytes, so the frame refuses an
# impossible one before reading any of it.
BEFORE=$(du -sk "$SPOOL" | awk '{print $1}')
{ printf 'beb 500000000000 500000000000\n'; head -c 1048576 /dev/zero; } |
    (pin m2 drop) >"$OUT" 2>"$ERR" && die "absurd signature length accepted"
grep -q "no ssh signature exceeds" "$ERR" || die "signature cap refusal text: $(cat "$ERR")"
AFTER=$(du -sk "$SPOOL" | awk '{print $1}')
test "$((AFTER - BEFORE))" -lt 1024 || die "refused frame wrote $((AFTER - BEFORE))KB to the spool"
ok "a signature length no signature can have is refused at the frame"

(pin m2 drop <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "replay errored"
test -s "$OUT" && die "a replayed receive wrote to stdout: $(cat "$OUT")"
grep -q '^beb: already delivered as 1; nothing added$' "$ERR" || die "replay ack: $(cat "$ERR")"
test "$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')" = 2 || die "replay installed a second copy"
ok "replay: idempotent, acks the existing id, no second copy"

# Dedup decides whether a delivery is already here, so a message it cannot
# read must never read as one that is not there: that would quietly
# downgrade exactly-once to maybe-twice. It refuses and names the rm.
if [ "$(id -u)" != 0 ]; then
    chmod 000 "$(mbox m2)/msg/000000000000000001"
    (pin m2 drop <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" &&
        die "receive read an unreadable message as absent"
    grep -q "cannot hash message 1" "$ERR" || die "unreadable refusal text: $(cat "$ERR")"
    grep -q "to make it a gap" "$ERR" || die "unreadable refusal names the rm: $(cat "$ERR")"
    chmod 600 "$(mbox m2)/msg/000000000000000001"
    test "$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')" = 2 || die "the refusal installed a copy"
    ok "an unreadable message refuses the delivery instead of reading as absent"
fi

cp "$HOME/note.mbeb" "$HOME/tampered.mbeb"
python3 -c "
import pathlib
p = pathlib.Path('$HOME/tampered.mbeb')
b = bytearray(p.read_bytes())
b[-10] ^= 0xFF
p.write_bytes(bytes(b))
"
(pin m2 drop <"$HOME/tampered.mbeb") >"$OUT" 2>"$ERR" && die "tampered delivery accepted"
grep -q "verification failed" "$ERR" || die "tamper refusal text: $(cat "$ERR")"
ok "tampered delivery: refused, nothing visible"

head -c 100 "$HOME/note.mbeb" | (pin m2 drop) >"$OUT" 2>"$ERR" && die "truncated accepted"
grep -q "truncated" "$ERR" || die "truncation refusal text: $(cat "$ERR")"
ok "truncated frame refused"

{ cat "$HOME/note.mbeb"; printf 'extra'; } | (pin m2 drop) >"$OUT" 2>"$ERR" && die "trailing bytes accepted"
grep -q "trailing" "$ERR" || die "trailing refusal text: $(cat "$ERR")"
test "$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')" = 2 || die "trailing garbage installed a message"
ok "trailing bytes refused, nothing installed"

head -c 1048576 /dev/urandom >"$HOME/bin.body"
(pin m1 pack "$M2" --subject "binary" <"$HOME/bin.body") | (pin m2 drop) >"$OUT" 2>"$ERR" || die "binary pipe failed"
(pin m2 read) >"$HOME/bin.out" 2>"$ERR" || die "read binary"
cmp -s "$HOME/bin.body" "$HOME/bin.out" || die "binary body mismatch"
ok "binary body round-trips through a pipe, byte-exact"

# Idempotency spans exactly retained history: prune the original and the
# same delivery installs anew.
rm "$(mbox m2)/msg/000000000000000001"
(pin m2 drop <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "post-prune replay failed"
grep -q "^beb: accepted 4 for " "$ERR" || die "post-prune ack: $(cat "$ERR")"
ok "pruned then replayed: the mailbox remembers what it retains"

# The dedup decision is atomic with insertion: 20 concurrent receives of
# one fresh delivery converge to exactly one message.
(pin m1 pack "$M2" --subject "race" --body "race payload") >"$HOME/race.mbeb" 2>/dev/null || die "pack race"
before=$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')
for i in $(seq 1 20); do
    (pin m2 drop <"$HOME/race.mbeb") >"$HOME/rc.$i.out" 2>"$HOME/rc.$i.err" &
done
wait
fresh=0
already=0
for i in $(seq 1 20); do
    grep -qE "^beb: (accepted|already delivered)" "$HOME/rc.$i.err" ||
        die "parallel receive $i failed: $(cat "$HOME/rc.$i.err")"
    test -s "$HOME/rc.$i.out" && die "parallel receive $i wrote to stdout"
    if grep -q "already delivered" "$HOME/rc.$i.err"; then already=$((already + 1)); else fresh=$((fresh + 1)); fi
done
test "$fresh" = 1 || die "concurrent receives: $fresh fresh installs, want 1"
test "$already" = 19 || die "concurrent receives: $already already-acks, want 19"
after=$(ls "$(mbox m2)/msg" | wc -l | tr -d ' ')
test $((after - before)) = 1 || die "concurrent receives installed $((after - before)) messages"
ok "20 concurrent receives: one install, dedup atomic with insertion"

# ---- the spool is private by construction, not by umask ----------------

# beb authenticates and does not encrypt, so the spool holds plaintext
# bodies. Confidentiality that depends on whatever umask the process
# started under is confidentiality you cannot state, so beb states the
# modes itself: every directory it makes is 0700 and every file 0600.

mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }

mkdir -p "$W/perm"
(umask 000 && cd "$W/perm" && "$BEB" init perm) >"$OUT" 2>"$ERR" || die "init under umask 000: $(cat "$ERR")"
PERM=$(addr perm)
PB=$(mbox perm)
(umask 000 && pin rs send "$PERM" --subject "perms" --body "private by construction") >/dev/null 2>"$ERR" ||
    die "send under umask 000: $(cat "$ERR")"
test "$(mode "$SPOOL")" = 700 || die "spool root mode $(mode "$SPOOL"), want 700"
test "$(mode "$PB")" = 700 || die "mailbox mode $(mode "$PB"), want 700"
test "$(mode "$PB/msg")" = 700 || die "messages mode $(mode "$PB/msg"), want 700"

test "$(mode "$PB/msg/000000000000000001")" = 600 || die "message mode $(mode "$PB/msg/000000000000000001"), want 600"

test "$(mode "$PB/cursor")" = 600 || die "cursor mode $(mode "$PB/cursor"), want 600"
test "$(mode "$W/perm/.beb")" = 700 || die ".beb mode $(mode "$W/perm/.beb"), want 700"
ok "under umask 000 the spool is still 0700 dirs and 0600 files"

# ---- large body streams ------------------------------------------------

BIG=$HOME/big
head -c 33554432 /dev/urandom >"$BIG" || die "make 32MB body"
(pin e send d --subject "big" <"$BIG") >"$OUT" 2>"$ERR" || die "large send"
(pin d read) >"$HOME/big.out" 2>"$ERR" || die "large read"
cmp -s "$BIG" "$HOME/big.out" || die "large body mismatch"
ok "32MB body round-trips byte-exact"

echo "all $n tests passed"
