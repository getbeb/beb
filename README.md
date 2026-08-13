# beb

Signed mail for autonomous processes.

An identity is an SSH key. A message is signed bytes. A mailbox is a
directory. Mail waits until read.

```console
$ echo "auth endpoint ready" | beb send backend
accepted 1; mail waits for backend
$ cd ../backend && beb read
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
and a `.gitignore` that keeps the key out of your repo. An identity is
that directory: beb uses the `.beb` under your feet, or the one named
by `BEB_IDENTITY` for a process that cannot cd.

Names live in one file, which beb reads and never writes:

```sh
echo "backend $(cd backend && beb whoami)" >> ~/.config/beb/known_signers
echo "frontend $(cd frontend && beb whoami)" >> ~/.config/beb/known_signers
```

Then mail flows by name:

```sh
cd frontend
echo "auth endpoint ready" | beb send backend   # accepted 1; mail waits for backend

cd ../backend
beb list                                        # 1  frontend
beb read                                        # auth endpoint ready
```

## Commands

```console
$ beb
beb 0.5.0 delivers signed messages between identities.

  beb init                    key and mailbox from nothing
  beb whoami                  your address
  beb send RECIPIENT [BODY]   sign and deliver, body from argument or stdin
  beb list [--all]            what is waiting, unread by default
  beb read                    consume the next message
  beb peek ID                 inspect one message, consuming nothing
  beb wait [-t SECS]          block until the next message arrives
  beb pack RECIPIENT [BODY]   sign one delivery onto stdout
  beb receive                 install one delivery from stdin
```

`wait` blocks on a kernel watch rather than polling and returns on the
next arrival, so a worker sleeps until there is work:

```sh
while beb wait; do beb read | ./handle-job; done
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
