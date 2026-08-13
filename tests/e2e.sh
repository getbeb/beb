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

# Run beb as identity $1 (a dir under $W), capturing stdout/stderr.
bx() { local d=$1; shift; (cd "$W/$d" && "$BEB" "$@") >"$OUT" 2>"$ERR"; }
mkid() { mkdir -p "$W/$1" && bx "$1" init; }
addr() { (cd "$W/$1" && "$BEB" whoami); }
sha() { if command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi; }
mbox() { echo "$SPOOL/$(printf '%s' "$(addr "$1")" | sha)"; }

# ---- version -----------------------------------------------------------

"$BEB" --version >"$OUT" 2>"$ERR" || die "--version failed"
grep -qE '^beb [0-9]+\.[0-9]+\.[0-9]+$' "$OUT" || die "--version shape: $(cat "$OUT")"
ok "--version prints beb x.y.z"

# ---- identity ----------------------------------------------------------

mkid a || die "init a"
grep -q "^created .beb/id_ed25519, mailbox " "$OUT" || die "init ack: created line"
grep -q "^your address: ssh-ed25519 " "$OUT" || die "init ack: address line"
grep -q "known_signers" "$OUT" || die "init ack: names the roster"
grep -q "^<name> ssh-ed25519 " "$OUT" || die "init ack: template line with <name> blank"
ok "init ack shape"

# The ack names known_signers, so appending to it must work with no
# mkdir in between: init creates the directory, never the file.
test -d "$(dirname "$KS")" || die "init did not create $(dirname "$KS")"
test -e "$KS" && die "init created known_signers; the file is the reader's"
echo "someone ssh-ed25519 AAAA" >>"$KS" 2>"$ERR" || die "append after init failed: $(cat "$ERR")"
rm -f "$KS"
ok "init creates the roster's directory, so its own next step lands"

A=$(addr a)
case "$A" in "ssh-ed25519 "*) ;; *) die "whoami shape: $A" ;; esac
ok "whoami is the address"

grep -qx '\*' "$W/a/.beb/.gitignore" || die "gitignore content"
ok "init writes .beb/.gitignore"

bx a init && die "second init succeeded"
grep -q "already an identity" "$ERR" || die "double init refusal text"
ok "init refuses twice"

mkdir -p "$W/nobody"
bx nobody whoami && die "whoami without identity succeeded"
grep -q "beb init" "$ERR" || die "no-identity refusal names the fix"
ok "no identity refuses, names beb init"

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

bx b send a "auth endpoint ready" || die "send by name"
grep -q "^accepted 1; " "$OUT" || die "send ack: $(cat "$OUT")"
ok "send by name, ack machine-first"

printf 'schema question' | bx b send a || die "send stdin"
grep -q "^accepted 2; " "$OUT" || die "send ack 2"
ok "send body from stdin"

bx c send a "deploy window moved" || die "send from c"

bx b send "$A" "raw key send" || die "send raw key"
ok "send accepts raw key text"

bx b send "$A pasted-comment" "with comment" || die "send .pub-shaped key"
ok "tolerant key parse (comment stripped)"

bx b send "ssh-ed25519 QQ==" nope && die "base64-shaped non-key accepted"
grep -q "not a valid ssh-ed25519" "$ERR" || die "non-key refusal: $(cat "$ERR")"
ok "key text must decode to a real ed25519 key"

bx a list || die "list"
printf '1  b\n2  b\n3  c\n4  b\n5  b\n' | diff - "$OUT" >/dev/null || die "list content: $(cat "$OUT")"
ok "list resolves roster names, id order"

bx b send nosuch hi && die "unknown name accepted"
grep -q 'no "nosuch"' "$ERR" || die "unknown name refusal"
grep -q 'add: nosuch ssh-ed25519' "$ERR" || die "unknown name refusal names the line to add"
ok "unknown name refuses, names the line to add"

bx b send dup hi && die "ambiguous name accepted"
grep -q 'lines 4, 5' "$ERR" || die "ambiguity refusal lines: $(cat "$ERR")"
ok "ambiguous name refuses, names both lines"

bx b send legacy hi && die "rsa roster line accepted"
grep -q 'ssh-rsa' "$ERR" || die "rsa refusal names the type"
grep -q 'line 6' "$ERR" || die "rsa refusal names the line"
ok "rsa roster line refused by name"

bx b send a "still fine" || die "clean names poisoned by bad lines"
ok "bad roster lines do not poison the file"

bx b send 'star*' hi && die "wildcard accepted"
grep -q 'wildcard' "$ERR" || die "wildcard refusal"
ok "wildcard principal refused"

set -- $A
bx b send "$1" "$2" && die "unquoted key accepted"
grep -q 'quote it' "$ERR" || die "unquoted key refusal: $(cat "$ERR")"
ok "unquoted key splitting refuses, names the quoting"

# ---- read: consume and inspect ----------------------------------------
# a's mailbox: 1 auth, 2 schema, 3 deploy, 4 raw-key, 5 with-comment, 6 still-fine

bx a read || die "consume 1"
printf 'auth endpoint ready' | diff - "$OUT" >/dev/null || die "body 1 exact: $(cat "$OUT")"
ok "consume prints exact body, no trailer"

bx a list || die "list after consume"
head -1 "$OUT" | grep -q '^2  b' || die "cursor did not advance"
ok "consume advances cursor"

bx a peek 4 || die "peek 4"
printf 'raw key send' | diff - "$OUT" >/dev/null || die "inspect body"
bx a list || die
test "$(grep -c . "$OUT")" = 5 || die "inspect moved the cursor: $(cat "$OUT")"
ok "inspect prints, cursor untouched"

bx a read || die "consume 2"
printf 'schema question' | diff - "$OUT" >/dev/null || die "order after inspect"
ok "consumption continues in id order after inspect"

MB_A=$(mbox a)
rm "$MB_A/messages/000000000000000003" "$MB_A/signatures/000000000000000003" || die "make gap"
bx a read || die "read over gap"
printf 'raw key send' | diff - "$OUT" >/dev/null || die "gap not skipped: $(cat "$OUT")"
ok "gap stepped over silently, inspected message still consumed"

printf 'garbage' >"$MB_A/signatures/000000000000000005"
bx a read && die "corrupt signature consumed"
grep -q "failed verification" "$ERR" || die "corrupt refusal text"
grep -q "rm '" "$ERR" || die "corrupt refusal names rm"
grep -q "messages/000000000000000005" "$ERR" || die "refusal names message file"
grep -q "signatures/000000000000000005" "$ERR" || die "refusal names signature file"
test -s "$OUT" && die "refusal printed body bytes"
ok "corrupt signature: refused before printing, rm named"

bx a list || die
head -1 "$OUT" | grep -q '^5  ' || die "cursor moved past bad message"
ok "cursor unmoved by refusal"

test -z "$(ls -A "$SPOOL/.tmp" 2>/dev/null)" || die "refusal left scratch litter: $(ls "$SPOOL/.tmp")"
ok "refusal paths leave no litter in .tmp"

rm "$MB_A/messages/000000000000000005" "$MB_A/signatures/000000000000000005"
bx a read || die "read after prune"
printf 'still fine' | diff - "$OUT" >/dev/null || die "stream did not resume: $(cat "$OUT")"
ok "after rm, stream resumes past the gap"

bx a read || die "empty backlog read failed"
test -s "$OUT" && die "empty backlog printed to stdout"
grep -q "no new mail" "$ERR" || die "empty backlog message"
ok "empty backlog: clean exit, says so on stderr"

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

bx a list --all || die "list --all"
grep -q '^1  b' "$OUT" || die "--all hides consumed"
bx a list || die
test -s "$OUT" && die "default list shows consumed"
ok "list --all shows history, default shows unread only"

# ---- recipient binding and foreign types -------------------------------

mkid d >/dev/null || die "init d"
D=$(addr d)
mkid e >/dev/null || die "init e"
E=$(addr e)
echo "d $D" >>"$KS"
echo "e $E" >>"$KS"

bx d send e "for e only" || die "send d->e"
MB_D=$(mbox d)
MB_E=$(mbox e)
cp "$MB_E/messages/000000000000000001" "$MB_D/messages/000000000000000090"
cp "$MB_E/signatures/000000000000000001" "$MB_D/signatures/000000000000000090"

bx d read && die "misaddressed message consumed"
grep -q "someone else" "$ERR" || die "wrong-to refusal text"
grep -q "messages/000000000000000090" "$ERR" || die "wrong-to refusal names message"
grep -q "signatures/000000000000000090" "$ERR" || die "wrong-to refusal names signature"
ok "consume refuses a valid message addressed elsewhere"

bx d peek 90 && die "misaddressed message inspected"
grep -q "someone else" "$ERR" || die "wrong-to inspect refusal"
ok "inspect refuses it too"

bx d list || die
grep -q '^90  ' "$OUT" || die "cursor advanced past misaddressed"
ok "cursor unmoved by binding refusal"

rm "$MB_D/messages/000000000000000090" "$MB_D/signatures/000000000000000090"
bx e send d "legit mail" || die "send e->d"
bx d read || die "read after removing misaddressed"
printf 'legit mail' | diff - "$OUT" >/dev/null || die "resume body"
ok "after rm, d's stream resumes"

ssh-keygen -t ecdsa -N "" -q -C "" -f "$HOME/ec" || die "ecdsa keygen"
EC=$(cat "$HOME/ec.pub")
bx d send "$EC" nope && die "ecdsa recipient accepted"
grep -q "ecdsa" "$ERR" || die "ecdsa refusal names the type"
grep -q "ssh-ed25519 only" "$ERR" || die "ecdsa refusal names the protocol"
ok "foreign recipient key type refused, type named"

ENVF=$HOME/intruder
printf 'from: %s\nto: %s\nnonce: AAAAAAAAAAAAAAAAAAAAAA==\n\nintruder' \
    "$(awk '{print $1" "$2}' "$HOME/ec.pub")" "$D" >"$ENVF"
ssh-keygen -Y sign -n beb -f "$HOME/ec" "$ENVF" 2>/dev/null || die "manual ecdsa sign"
cp "$ENVF" "$MB_D/messages/000000000000000091"
cp "$ENVF.sig" "$MB_D/signatures/000000000000000091"
bx d read && die "foreign-from envelope consumed"
grep -q "non-ed25519" "$ERR" || die "foreign-from refusal text"
grep -q "rm '" "$ERR" || die "foreign-from refusal names rm"
ok "validly signed non-ed25519 from: refused"
rm "$MB_D/messages/000000000000000091" "$MB_D/signatures/000000000000000091"

# ---- wait: edge-triggered block until arrival --------------------------

mkid w1 >/dev/null || die "init w1"
W1=$(addr w1)
mkid w2 >/dev/null || die "init w2"

(sleep 1 && cd "$W/w2" && "$BEB" send "$W1" "wake up" >/dev/null 2>&1) &
t0=$(date +%s)
(cd "$W/w1" && "$BEB" wait -t 15) >"$OUT" 2>"$ERR" || die "wait did not return on arrival"
t1=$(date +%s)
test $((t1 - t0)) -lt 10 || die "wait took $((t1 - t0))s; not event-driven"
test -s "$OUT" && die "wait printed to stdout"
ok "wait returns on arrival, prints nothing"

# w1 now has standing unread mail: wait must NOT return for it.
(cd "$W/w1" && "$BEB" wait -t 2) >"$OUT" 2>"$ERR" && die "wait returned on standing unread"
test -s "$ERR" && die "timeout was not silent: $(cat "$ERR")"
ok "standing unread does not return; timeout exits 1 silently"

bx nobody wait && die "wait without identity succeeded"
grep -q "beb init" "$ERR" || die "wait refusal names the fix"
ok "wait refuses without an identity"

# ---- BEB_IDENTITY: env identity, no precedence -------------------------

(cd "$W/nobody" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" || die "env identity failed"
test "$(cat "$OUT")" = "$A" || die "env identity wrong key"
ok "BEB_IDENTITY alone: the env directory's identity"

(cd "$W/a" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" || die "env+cwd same dir failed"
ok "env and cwd agreeing on the same dir works"

mkdir -p "$W/a-twin" && cp -R "$W/a/.beb" "$W/a-twin/.beb"
(cd "$W/a" && BEB_IDENTITY="$W/a-twin" "$BEB" whoami) >"$OUT" 2>"$ERR" || die "same-key twin refused"
ok "agreement is by public key, not path"

(cd "$W/b" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "conflicting identities resolved"
grep -q "two identities" "$ERR" || die "conflict refusal text: $(cat "$ERR")"
grep -q "unset BEB_IDENTITY or cd" "$ERR" || die "conflict refusal names fixes"
ok "disagreement refuses, names both fixes"

mkdir -p "$W/nobeb"
(cd "$W/nobody" && BEB_IDENTITY="$W/nobeb" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "broken env resolved"
grep -q "beb init" "$ERR" || die "broken env refusal names beb init"
ok "broken BEB_IDENTITY refuses"

(cd "$W/a" && BEB_IDENTITY="$W/nobeb" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "broken env fell back to cwd"
ok "broken BEB_IDENTITY never falls back, even over a valid cwd"

# A broken claim is not an absent one, on either side.
mkdir -p "$W/cracked" && cp -R "$W/a/.beb" "$W/cracked/.beb" && printf 'garbage' >"$W/cracked/.beb/id_ed25519.pub"

bx cracked whoami && die "broken cwd identity resolved"
grep -q "broken identity" "$ERR" || die "broken cwd refusal text: $(cat "$ERR")"
ok "broken cwd identity refuses as broken, not absent"

(cd "$W/cracked" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "broken cwd ignored under env"
grep -q "agreement cannot be checked" "$ERR" || die "broken-cwd-under-env text: $(cat "$ERR")"
ok "broken cwd + valid env refuses: no agreement, no precedence"

(cd "$W/a" && BEB_IDENTITY="$W/cracked" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "broken env identity resolved"
grep -q "broken identity" "$ERR" || die "broken env keeps its reason: $(cat "$ERR")"
grep -q "has no .beb" "$ERR" && die "broken env mislabeled as absent"
ok "valid cwd + broken env refuses with the real reason"

# A .beb whose private key is gone is a damaged claim, not an absent one.
mkdir -p "$W/keyless" && cp -R "$W/a/.beb" "$W/keyless/.beb" && rm "$W/keyless/.beb/id_ed25519"

bx keyless whoami && die "keyless identity resolved"
grep -q "id_ed25519 is missing" "$ERR" || die "keyless refusal text: $(cat "$ERR")"
ok "missing private key refuses as broken, not absent"

(cd "$W/keyless" && BEB_IDENTITY="$W/a" "$BEB" whoami) >"$OUT" 2>"$ERR" && die "keyless cwd ignored under env"
grep -q "agreement cannot be checked" "$ERR" || die "keyless-under-env text: $(cat "$ERR")"
ok "missing private key + valid env refuses: agreement cannot be established"

# ---- 40 parallel senders: the flock is load-bearing --------------------

mkid p >/dev/null || die "init p"
P=$(addr p)
mkid q >/dev/null || die "init q"
for i in $(seq 1 40); do
    (cd "$W/q" && "$BEB" send "$P" "msg $i") >/dev/null 2>&1 &
done
wait
MB_P=$(mbox p)
test "$(ls "$MB_P/messages" | wc -l | tr -d ' ')" = 40 || die "parallel: $(ls "$MB_P/messages" | wc -l) messages, want 40"
ls "$MB_P/messages" | sort | tail -1 | grep -qx '000000000000000040' || die "parallel: ids not 1..40"
diff <(ls "$MB_P/messages" | sort) <(ls "$MB_P/signatures" | sort) >/dev/null || die "parallel: signature set differs from message set"
ok "40 parallel senders: 40 messages, ids 1..40, no reuse, every signature present"

# ---- concurrent readers: consumption is serialized too -----------------

# Delivery has always been locked; consumption needs the same lock. A
# cursor read before another reader's write and set after it moves
# backwards, and the message in between is handed out twice.

mkid rs >/dev/null || die "init rs"
mkid rr >/dev/null || die "init rr"
RR=$(addr rr)
for i in 1 2 3 4 5 6; do
    (cd "$W/rs" && "$BEB" send "$RR" "body-$i") >/dev/null 2>"$ERR" || die "send body-$i"
done
for i in 1 2 3 4 5 6; do
    (cd "$W/rr" && "$BEB" read) >"$HOME/rd.$i.out" 2>"$HOME/rd.$i.err" &
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

(cd "$W/m1" && "$BEB" pack carrier "over the wall") >"$HOME/note.mbeb" 2>"$ERR" || die "pack failed"
test -s "$HOME/note.mbeb" || die "pack wrote nothing"
test -s "$ERR" && die "pack spoke on success: $(cat "$ERR")"
head -c 4 "$HOME/note.mbeb" | grep -q "^beb " || die "frame header shape"
ok "pack: silent success, frame on stdout"

test "$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')" = 0 || die "pack touched the recipient mailbox"
ok "pack delivers nothing"

# receive resolves no identity: the envelope carries its address, so a
# delivery installs from anywhere, even where no .beb exists.
(cd "$W/nobody" && "$BEB" receive <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "receive failed: $(cat "$ERR")"
grep -q "^accepted 1; read with: beb read$" "$OUT" || die "receive ack: $(cat "$OUT")"
(cd "$W/m2" && "$BEB" read) >"$OUT" 2>"$ERR" || die "read received mail"
printf 'over the wall' | diff - "$OUT" >/dev/null || die "body across the wall: $(cat "$OUT")"
ok "receive installs by the envelope's address, needing no identity of its own"

# Standing in another identity never redirects a delivery: the address
# decides, not the reader.
(cd "$W/m1" && "$BEB" pack carrier "second for m2") | (cd "$W/m3" && "$BEB" receive) >"$OUT" 2>"$ERR" || die "receive from m3 failed: $(cat "$ERR")"
test "$(ls "$(mbox m3)/messages" | wc -l | tr -d ' ')" = 0 || die "delivery landed in the reader's mailbox"
test "$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')" = 2 || die "delivery did not land in the addressed mailbox"
(cd "$W/m2" && "$BEB" read) >"$OUT" 2>"$ERR" || die "read second"
printf 'second for m2' | diff - "$OUT" >/dev/null || die "second body: $(cat "$OUT")"
ok "the address decides the mailbox, never the directory receive runs in"

# A mailbox that does not exist here is not conjured by mail arriving:
# residence is having run beb init, and a stranger is refused.
ssh-keygen -q -t ed25519 -N "" -C stranger -f "$HOME/stranger" </dev/null || die "keygen stranger"
echo "stranger $(awk '{print $1" "$2}' "$HOME/stranger.pub")" >>"$KS"
(cd "$W/m1" && "$BEB" pack stranger "nobody home") >"$HOME/stranger.mbeb" || die "pack stranger"
(cd "$W/m2" && "$BEB" receive <"$HOME/stranger.mbeb") >"$OUT" 2>"$ERR" && die "delivery for a stranger accepted"
grep -q "no mailbox here" "$ERR" || die "stranger refusal text: $(cat "$ERR")"
grep -q "beb init" "$ERR" || die "stranger refusal names the fix: $(cat "$ERR")"
STRANGER_BOX=$(printf '%s' "$(awk '{print $1" "$2}' "$HOME/stranger.pub")" | shasum -a 256 | awk '{print $1}')
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
} | (cd "$W/m2" && "$BEB" receive) >"$OUT" 2>"$ERR" && die "unbounded delivery for a stranger accepted"
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
    (cd "$W/m2" && "$BEB" receive) >"$OUT" 2>"$ERR" && die "absurd signature length accepted"
grep -q "no ssh signature exceeds" "$ERR" || die "signature cap refusal text: $(cat "$ERR")"
AFTER=$(du -sk "$SPOOL" | awk '{print $1}')
test "$((AFTER - BEFORE))" -lt 1024 || die "refused frame wrote $((AFTER - BEFORE))KB to the spool"
ok "a signature length no signature can have is refused at the frame"

(cd "$W/m2" && "$BEB" receive <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "replay errored"
grep -q "^accepted 1; already delivered$" "$OUT" || die "replay ack: $(cat "$OUT")"
test "$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')" = 2 || die "replay installed a second copy"
ok "replay: idempotent, acks the existing id, no second copy"

# Dedup decides whether a delivery is already here, so a message it cannot
# read must never read as one that is not there: that would quietly
# downgrade exactly-once to maybe-twice. It refuses and names the rm.
if [ "$(id -u)" != 0 ]; then
    chmod 000 "$(mbox m2)/messages/000000000000000001"
    (cd "$W/m2" && "$BEB" receive <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" &&
        die "receive read an unreadable message as absent"
    grep -q "cannot hash message 1" "$ERR" || die "unreadable refusal text: $(cat "$ERR")"
    grep -q "to make it a gap" "$ERR" || die "unreadable refusal names the rm: $(cat "$ERR")"
    chmod 600 "$(mbox m2)/messages/000000000000000001"
    test "$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')" = 2 || die "the refusal installed a copy"
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
(cd "$W/m2" && "$BEB" receive <"$HOME/tampered.mbeb") >"$OUT" 2>"$ERR" && die "tampered delivery accepted"
grep -q "verification failed" "$ERR" || die "tamper refusal text: $(cat "$ERR")"
ok "tampered delivery: refused, nothing visible"

head -c 100 "$HOME/note.mbeb" | (cd "$W/m2" && "$BEB" receive) >"$OUT" 2>"$ERR" && die "truncated accepted"
grep -q "truncated" "$ERR" || die "truncation refusal text: $(cat "$ERR")"
ok "truncated frame refused"

{ cat "$HOME/note.mbeb"; printf 'extra'; } | (cd "$W/m2" && "$BEB" receive) >"$OUT" 2>"$ERR" && die "trailing bytes accepted"
grep -q "trailing" "$ERR" || die "trailing refusal text: $(cat "$ERR")"
test "$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')" = 2 || die "trailing garbage installed a message"
ok "trailing bytes refused, nothing installed"

head -c 1048576 /dev/urandom >"$HOME/bin.body"
(cd "$W/m1" && "$BEB" pack "$M2" <"$HOME/bin.body") | (cd "$W/m2" && "$BEB" receive) >"$OUT" 2>"$ERR" || die "binary pipe failed"
(cd "$W/m2" && "$BEB" read) >"$HOME/bin.out" 2>"$ERR" || die "read binary"
cmp -s "$HOME/bin.body" "$HOME/bin.out" || die "binary body mismatch"
ok "binary body round-trips through a pipe, byte-exact"

# Idempotency spans exactly retained history: prune the original and the
# same delivery installs anew.
rm "$(mbox m2)/messages/000000000000000001" "$(mbox m2)/signatures/000000000000000001"
(cd "$W/m2" && "$BEB" receive <"$HOME/note.mbeb") >"$OUT" 2>"$ERR" || die "post-prune replay failed"
grep -q "^accepted 4; read with: beb read$" "$OUT" || die "post-prune ack: $(cat "$OUT")"
ok "pruned then replayed: the mailbox remembers what it retains"

# The dedup decision is atomic with insertion: 20 concurrent receives of
# one fresh delivery converge to exactly one message.
(cd "$W/m1" && "$BEB" pack "$M2" "race payload") >"$HOME/race.mbeb" 2>/dev/null || die "pack race"
before=$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')
for i in $(seq 1 20); do
    (cd "$W/m2" && "$BEB" receive <"$HOME/race.mbeb") >"$HOME/rc.$i.out" 2>"$HOME/rc.$i.err" &
done
wait
fresh=0
already=0
for i in $(seq 1 20); do
    grep -q "^accepted" "$HOME/rc.$i.out" || die "parallel receive $i failed: $(cat "$HOME/rc.$i.err")"
    if grep -q "already delivered" "$HOME/rc.$i.out"; then already=$((already + 1)); else fresh=$((fresh + 1)); fi
done
test "$fresh" = 1 || die "concurrent receives: $fresh fresh installs, want 1"
test "$already" = 19 || die "concurrent receives: $already already-acks, want 19"
after=$(ls "$(mbox m2)/messages" | wc -l | tr -d ' ')
test $((after - before)) = 1 || die "concurrent receives installed $((after - before)) messages"
ok "20 concurrent receives: one install, dedup atomic with insertion"

# ---- the spool is private by construction, not by umask ----------------

# beb authenticates and does not encrypt, so the spool holds plaintext
# bodies. Confidentiality that depends on whatever umask the process
# started under is confidentiality you cannot state, so beb states the
# modes itself: every directory it makes is 0700 and every file 0600.

mode() { stat -c '%a' "$1" 2>/dev/null || stat -f '%Lp' "$1"; }

mkdir -p "$W/perm"
(umask 000 && cd "$W/perm" && "$BEB" init) >"$OUT" 2>"$ERR" || die "init under umask 000: $(cat "$ERR")"
PERM=$(addr perm)
PB=$(mbox perm)
(umask 000 && cd "$W/rs" && "$BEB" send "$PERM" "private by construction") >/dev/null 2>"$ERR" ||
    die "send under umask 000: $(cat "$ERR")"
test "$(mode "$SPOOL")" = 700 || die "spool root mode $(mode "$SPOOL"), want 700"
test "$(mode "$PB")" = 700 || die "mailbox mode $(mode "$PB"), want 700"
test "$(mode "$PB/messages")" = 700 || die "messages mode $(mode "$PB/messages"), want 700"
test "$(mode "$PB/signatures")" = 700 || die "signatures mode $(mode "$PB/signatures"), want 700"
test "$(mode "$PB/messages/000000000000000001")" = 600 || die "message mode $(mode "$PB/messages/000000000000000001"), want 600"
test "$(mode "$PB/signatures/000000000000000001")" = 600 || die "signature mode $(mode "$PB/signatures/000000000000000001"), want 600"
test "$(mode "$PB/cursor")" = 600 || die "cursor mode $(mode "$PB/cursor"), want 600"
test "$(mode "$W/perm/.beb")" = 700 || die ".beb mode $(mode "$W/perm/.beb"), want 700"
ok "under umask 000 the spool is still 0700 dirs and 0600 files"

# ---- large body streams ------------------------------------------------

BIG=$HOME/big
head -c 33554432 /dev/urandom >"$BIG" || die "make 32MB body"
(cd "$W/e" && "$BEB" send d <"$BIG") >"$OUT" 2>"$ERR" || die "large send"
(cd "$W/d" && "$BEB" read) >"$HOME/big.out" 2>"$ERR" || die "large read"
cmp -s "$BIG" "$HOME/big.out" || die "large body mismatch"
ok "32MB body round-trips byte-exact"

echo "all $n tests passed"
