# beb

Signed mail for autonomous processes.

An identity is an SSH key. A message is signed bytes. A mailbox is a
directory. Mail waits until read.

```console
$ echo "auth endpoint ready" | beb send backend --subject "endpoint ready"
beb: accepted for backend; 20 bytes; it waits on this machine for beb read
$ BEB_IDENTITY=~/work/backend beb read
auth endpoint ready
```

## Install

```sh
curl -fsSL https://getbeb.dev/install.sh | sh
```

Or from source with cargo (Rust 1.75+):

```sh
cargo install --git https://github.com/getbeb/beb
```

Or with nix, which builds and runs the test suite:

```sh
nix profile install github:getbeb/beb
```

beb signs with `ssh-keygen -Y`, so it needs OpenSSH 8.2 or newer on
PATH.

## Quick start

Two identities on one machine, mailing each other.

```sh
mkdir backend frontend
(cd backend && beb init)
(cd frontend && beb init)
```

Each `init` creates a `.beb/` holding an ed25519 keypair, a mailbox,
and a `.gitignore` that keeps the key out of your repo.

An identity is that directory, and `BEB_IDENTITY` names it. Every verb
but `init` reads it and nothing else: not the working directory, so
`cd` moves the shell and never the signer. Set it once, wherever you
decide such things:

```sh
export BEB_IDENTITY=$PWD/frontend
```

Names live in one file, which beb reads and never writes:

```sh
echo "backend  $(BEB_IDENTITY=$PWD/backend beb whoami)"  >> ~/.config/beb/known_signers
echo "frontend $(BEB_IDENTITY=$PWD/frontend beb whoami)" >> ~/.config/beb/known_signers
```

Each of those prints `beb: identity from ...` to stderr as it runs.
Command substitution captures stdout only, so beb's own commentary
reaches you rather than the file. Everything beb says about a result
goes to stderr with a `beb: ` prefix, which is what makes
`2>&1 | grep -v '^beb:'` give you back exactly the result.

Then mail flows by name:

```sh
export BEB_IDENTITY=$PWD/frontend
echo "auth endpoint ready" | beb send backend --subject "endpoint ready"

export BEB_IDENTITY=$PWD/backend
beb list        # beb: cursor at 0; 1 total, 1 unread; showing 1
                # 1  now  endpoint ready  frontend
beb read        # auth endpoint ready
```

For a shell, direnv or a line in your profile pins it. For an agent,
the harness does: [claude-beb](https://github.com/getbeb/claude-beb)
pins the session's launch directory at start, so a session that
wanders between subdirectories keeps signing as whoever it began as.

## Commands

```console
$ beb
beb 0.7.0 delivers signed messages between identities.

  beb init
      a new identity in this directory
  beb whoami
      your address

  beb send RECIPIENT --subject S [--body B]
      sign and deliver; the body comes from --body or stdin
  beb list [--after ID | --before ID] [--limit N]
      read-only. unread, at most 10 rows, printed oldest first
      --after/--before exclude ID, take the N nearest it, and reach read mail
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

Exit: 0 did it, 1 change the command, 2 nothing to do, 3 refused.

BEB_IDENTITY names the directory holding the .beb to act as. Every verb
requires it except init, which never reads it and always writes here:

  export BEB_IDENTITY=/path/to/dir
```

`wait` blocks on a kernel watch rather than polling and returns as soon
as there is unread mail, so a worker sleeps until there is work and
never sleeps while work is waiting:

```sh
while beb wait; do beb read | ./handle-job; done
```

It waits from your cursor by default. A waiter that must not fire twice
for the same mail — a doorbell that wakes a session, say — keeps its own
mark instead: `wait` prints the next mark on stdout, and `--from` takes
it back.

```sh
m=$(beb wait --timeout 0)
while :; do
    m=$(beb wait --from "$m" --timeout 900) && notify
done
```

## Design

beb owns no network. `pack` writes a delivery to stdout, `receive`
installs one from stdin, and whatever carries the bytes between them
is your choice. [beb-ssh](https://github.com/getbeb/beb-ssh) is one
that keeps custody and retries.

`receive` authenticates the bytes, not the peer. It refuses a
delivery for a mailbox that does not exist here before storing
anything, so a stranger cannot spend your disk, but who may hand it a
frame is the transport's question. Give it a transport that
authenticates, such as an ssh command, rather than exposing it to
arbitrary clients.

[DESIGN.md](DESIGN.md) has the envelope format, the delivery
guarantees, and what beb refuses to know.

## License

MIT
