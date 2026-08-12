# beb

Signed messages between identities on one machine.

An identity is an SSH key that lives in a directory. Every message is
signed with `ssh-keygen -Y` and stored as the exact signed bytes, one
file per message, in a spool you can read with `cat`. There is no
daemon, no account, no registry, and no size cap. Mail waits until
the recipient chooses to read it.

```
frontend/.beb                     ops/.beb
     |                                |
     |  beb send backend              |  beb send backend
     v                                v
   +----------------------------------------+
   |            backend's mailbox           |
   |                                        |
   |   1  from frontend   signed, waiting   |
   |   2  from ops        signed, waiting   |
   +----------------------------------------+
                      |
                      |  beb list, beb read
                      v
                 backend/.beb
```

## Install

```sh
curl -fsSL https://getbeb.dev/install.sh | sh
```

That fetches the prebuilt binary for your platform from the latest
release, verifies its checksum, and installs it to `~/.local/bin`
(override with `BEB_INSTALL_DIR`).

From source, with cargo (Rust 1.75 or newer):

```sh
cargo install --git https://github.com/getbeb/beb
```

With nix (builds and runs the test suite):

```sh
nix profile install github:getbeb/beb
```

beb drives `ssh-keygen -Y` for all signing and verification, so it
needs OpenSSH 8.2 or newer on PATH at runtime. The nix package
bundles its own copy.

## Quick start

An identity is a directory. Make two and let one mail the other:

```sh
mkdir backend frontend

(cd backend && beb init)
(cd frontend && beb init)
```

Each `init` prints an address and the line that names it. Paste both
lines into `~/.config/beb/known_signers`:

```
backend ssh-ed25519 AAAA...
frontend ssh-ed25519 AAAA...
```

Then mail flows by name:

```sh
cd frontend
echo "auth endpoint ready" | beb send backend
# accepted 1; mail waits for backend

cd ../backend
beb list
# 1  frontend
beb read
# auth endpoint ready
```

## Commands

```
beb init                    key and mailbox from nothing
beb send RECIPIENT [BODY]   body from argument or stdin
beb list [--all]            unread by default
beb read                    consume the next message
beb read ID                 inspect one message
beb wait [-t SECS]          block until the next message arrives
beb whoami                  your address
```

`wait` blocks on a kernel watch, not a poll, and is edge-triggered:
mail already unread does not return it (that is `list`'s question),
the next arrival does. It prints nothing and exits 0 on arrival, 1 on
timeout, so `beb wait && beb read` is a complete event loop.

Every ack names the next step; every refusal names the fix.

## Identity

Identity resolves to `./.beb` in the working directory, nothing else.
No environment variable, nothing global, no default: where you run is
who you are, and a process running where no `.beb` exists refuses to
be anyone. Scoping identity is the shell's job:

```sh
(cd ~/work/backend && beb send frontend "migration ready")
```

`init` generates the keypair inside `.beb/` along with a `.gitignore`
that keeps the key out of your repository.

## Naming

Names live in `~/.config/beb/known_signers`, one line per name, in
ssh's allowed_signers format. beb reads the file and never writes it:
a name is assigned by its reader, and a sender's claim about its own
name never enters beb.

Nothing requires being named. A stranger's mail is delivered and
verified exactly like a named sender's; it just lists as the full
public key, which `send` also accepts, so any listed sender is
addressable as shown.

## The spool

Messages rest in `~/.local/share/beb/`, one directory per mailbox,
one file per message, beside its detached signature:

```
~/.local/share/beb/<sha256 of key text>/
├── messages/000000000000000001
├── signatures/000000000000000001
└── cursor
```

A message file is exactly the signed bytes: `cat` shows it, `grep`
reaches its body, and `ssh-keygen -Y verify` takes it as-is. `read`
verifies the signature and the recipient binding before printing a
byte; a message that fails either check is refused with the exact
`rm` that removes it, and the numbering tolerates the gap.

The full design, including the envelope format, the delivery
guarantees, and what beb refuses to know, is in
[DESIGN.md](DESIGN.md).

## License

MIT
