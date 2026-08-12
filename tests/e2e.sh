#!/usr/bin/env bash
# End-to-end suite. Run via `cargo test` (tests/cli.rs) or by hand:
#   BEB=target/debug/beb bash tests/e2e.sh
set -u

BEB=${BEB:?set BEB to the beb binary}
case "$BEB" in /*) ;; *) BEB=$PWD/$BEB ;; esac

export HOME=$(mktemp -d)
unset XDG_DATA_HOME XDG_CONFIG_HOME 2>/dev/null || true
SPOOL=$HOME/.local/share/beb
KS=$HOME/.config/beb/known_signers
mkdir -p "$HOME/.config/beb"
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

# ---- identity ----------------------------------------------------------

mkid a || die "init a"
grep -q "^created .beb/id_ed25519, mailbox " "$OUT" || die "init ack: created line"
grep -q "^your address: ssh-ed25519 " "$OUT" || die "init ack: address line"
grep -q "known_signers" "$OUT" || die "init ack: names the roster"
grep -q "^<name> ssh-ed25519 " "$OUT" || die "init ack: template line with <name> blank"
ok "init ack shape"

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

bx a read 4 || die "inspect 4"
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

bx a read 999 && die "missing id accepted"
grep -q "no message 999" "$ERR" || die "missing id refusal"
ok "inspect of missing id refuses"

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

bx d read 90 && die "misaddressed message inspected"
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

# ---- large body streams ------------------------------------------------

BIG=$HOME/big
head -c 33554432 /dev/urandom >"$BIG" || die "make 32MB body"
(cd "$W/e" && "$BEB" send d <"$BIG") >"$OUT" 2>"$ERR" || die "large send"
(cd "$W/d" && "$BEB" read) >"$HOME/big.out" 2>"$ERR" || die "large read"
cmp -s "$BIG" "$HOME/big.out" || die "large body mismatch"
ok "32MB body round-trips byte-exact"

echo "all $n tests passed"
