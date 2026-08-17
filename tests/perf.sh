#!/usr/bin/env bash
# Cost must not grow with the size of a mailbox.
#
# Run: BEB=target/release/beb bash tests/perf.sh
#
# Not a wall-clock budget. A threshold in milliseconds says more about
# the machine than the code, and rots the first time CI changes host. So
# every check here is a RATIO: the same command, on the same machine, in
# the same run, against a mailbox 500 times larger. If the answer takes
# about as long either way, the cost is independent of history. If it
# takes 500 times longer, something reads the directory again.
#
# That is the regression this exists to catch. `list`, `read`, `wait` and
# `peek` each used to enumerate and sort every message to answer a
# question about one -- 294ms on a 200k mailbox, against 2ms of process
# startup. The fix was to guess ids from `.counter` instead of listing
# them, and nothing in the code stops the next change from listing again.
set -u

BEB=${BEB:-beb}
# init runs with cwd inside the fixture, so a relative path would stop
# resolving the moment this script changes directory.
case "$BEB" in /*) ;; */*) BEB=$PWD/$BEB ;; esac   # a bare name stays on PATH
SMALL=100
BIG=50000
# Generous on purpose. A real regression is two orders of magnitude; this
# only has to sit above measurement noise, and noise near the ~2ms
# process floor is a large fraction of a small number.
MAX_RATIO=4

n=0
ok() { n=$((n + 1)); echo "ok $n - $1"; }
die() { echo "not ok - $1"; exit 1; }

command -v python3 >/dev/null || die "python3 is needed to build the fixture and time the runs"

W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
export XDG_DATA_HOME=$W/data XDG_CONFIG_HOME=$W/config
mkdir -p "$W/config/beb" "$W/a" "$W/small" "$W/big"

for id in a small big; do
    (cd "$W/$id" && "$BEB" init "$id" >/dev/null 2>&1) || die "init $id"
done
addr() { BEB_IDENTITY="$W/$1" "$BEB" whoami 2>/dev/null; }
mbox() {
    python3 -c '
import base64,sys
sys.stdout.write(base64.b64decode(sys.argv[1].split()[1])[19:51].hex())' "$(addr "$1")"
}

# One real message in each, then the big one is inflated with hardlinks
# to the same frame. Every id is a whole, valid delivery -- the point is
# how many exist, not what is in them.
for id in small big; do
    BEB_IDENTITY="$W/a" "$BEB" send "$(addr "$id")" --subject seed --body x >/dev/null 2>&1 ||
        die "seed $id"
done
for id in small big; do
    BEB_IDENTITY="$W/a" "$BEB" pack "$(addr "$id")" --subject dup --body x >"$W/dup.$id.mbeb" 2>/dev/null ||
        die "pack a duplicate for $id"
    BEB_IDENTITY="$W/$id" "$BEB" drop <"$W/dup.$id.mbeb" >/dev/null 2>&1 ||
        die "install the original for $id"
done

inflate() { # <mailbox dir> <count>
    python3 - "$1" "$2" <<'PY'
import os, sys
mb, n = sys.argv[1], int(sys.argv[2])
d = os.path.join(mb, "msg")
src = os.path.join(d, "%018d" % 1)
for i in range(len(os.listdir(d)) + 1, n + 1):
    os.link(src, os.path.join(d, "%018d" % i))
open(os.path.join(mb, ".counter"), "w").write(str(n))
PY
}
inflate "$XDG_DATA_HOME/beb/$(mbox small)" "$SMALL" || die "inflate small"
inflate "$XDG_DATA_HOME/beb/$(mbox big)" "$BIG" || die "inflate big"

# Best-of-N, not mean: the fastest run is the one least disturbed by
# whatever else the machine was doing, and this is a comparison between
# two numbers measured the same way.
ms() { # <identity> <verb...> -> milliseconds; STDIN names a file to feed
    local id=$1; shift
    python3 - "$W/$id" "${STDIN:-}" "$BEB" "$@" <<'PY'
import os, subprocess, sys, time
ident, stdin, beb, *args = sys.argv[1:]
env = dict(os.environ, BEB_IDENTITY=ident)
best = float("inf")
for _ in range(9):
    f = open(stdin, "rb") if stdin else subprocess.DEVNULL
    s = time.perf_counter()
    subprocess.run([beb] + args, env=env, stdin=f,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    best = min(best, (time.perf_counter() - s) * 1000)
    if stdin:
        f.close()
print("%.3f" % best)
PY
}

check() { # <name> <verb...>
    local name=$1; shift
    local s b ratio
    # The cursor goes back to 0 before each, so both mailboxes are asked
    # the same question: everything is unread.
    echo 0 >"$XDG_DATA_HOME/beb/$(mbox small)/cursor"
    echo 0 >"$XDG_DATA_HOME/beb/$(mbox big)/cursor"
    s=$(STDIN=${SMALL_STDIN:-} ms small "$@")
    b=$(STDIN=${BIG_STDIN:-} ms big "$@")
    ratio=$(python3 -c "print('%.1f' % ($b / max($s, 0.001)))")
    printf '    %-22s %6sms at %s, %8sms at %s  (%sx)\n' "$name" "$s" "$SMALL" "$b" "$BIG" "$ratio"
    python3 -c "import sys; sys.exit(0 if $b <= $s * $MAX_RATIO else 1)" ||
        die "$name costs ${ratio}x more on a mailbox ${BIG}/${SMALL} times larger; it scales with history"
}

echo "  mailbox size $SMALL vs $BIG, same machine, best of 9"
check "list"            list --limit 1
check "list --before"   list --before 60 --limit 1
check "wait"            wait --timeout 1
check "peek"            peek 1
check "read"            read

# The sender above is in the roster, which is the cheap path: the hint
# that names an unnamed sender returns before looking at anything. The
# expensive path is a sender never seen before, which used to read every
# envelope in the mailbox and find nothing.
UN=$W/unnamed
mkdir -p "$UN" && (cd "$UN" && "$BEB" init unnamed >/dev/null 2>&1) || die "init unnamed"
grep -v '^unnamed ' "$XDG_CONFIG_HOME/beb/known_signers" >"$W/ks.tmp" && mv "$W/ks.tmp" "$XDG_CONFIG_HOME/beb/known_signers"
for id in small big; do
    BEB_IDENTITY="$UN" "$BEB" send "$(addr "$id")" --subject stranger --body x >/dev/null 2>&1 ||
        die "send from an unnamed sender to $id"
done
check "read (unnamed sender)"  read
ok "reading a mailbox costs the same at $SMALL messages and at $BIG"

# Delivery too. It compares an arriving frame against retained ones to
# stay exactly-once, and that comparison used to run to the beginning of
# history -- 391ms here, growing every day the mailbox was used. It walks
# back a fixed number of ids now, so it costs the same on any mailbox.
SMALL_STDIN=$W/dup.small.mbeb BIG_STDIN=$W/dup.big.mbeb check "drop (a duplicate)" drop
ok "delivery costs the same at $SMALL messages and at $BIG"

# The other axis: not how cost grows, but what a verb pays over the one
# beside it. `read` is `peek` plus a cursor write, and both verify one
# signature, so the difference between them is the cursor and nothing
# else. Written with the drive barrier it was 15ms against peek's 7 --
# one position costing more than the ssh-keygen that checks a signature.
# Same principle as every check above: a ratio between two numbers taken
# the same way on the same machine, never a threshold in milliseconds.
echo 0 >"$XDG_DATA_HOME/beb/$(mbox big)/cursor"
pk=$(ms big peek 1)
echo 0 >"$XDG_DATA_HOME/beb/$(mbox big)/cursor"
rd=$(ms big read)
ratio=$(python3 -c "print('%.1f' % ($rd / max($pk, 0.001)))")
printf '    %-22s %6sms peek, %8sms read  (%sx)\n' "cursor over peek" "$pk" "$rd" "$ratio"
python3 -c "import sys; sys.exit(0 if $rd <= $pk * 1.6 else 1)" ||
    die "read costs ${ratio}x peek; the cursor write is back on the drive barrier"
ok "consuming a message costs about what looking at one costs"

echo "all $n tests passed"
