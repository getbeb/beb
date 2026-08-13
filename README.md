# beb

Signed messages between identities.

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

## Usage

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

Nine verbs, and no verb has two effects. Everything else is files: an identity is a directory, a mailbox is a directory, a
message is the bytes that were signed.

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

## Waiting

`wait` blocks on a kernel watch, not a poll, and is edge-triggered:
mail already unread does not return it (that is `list`'s question),
the next arrival does. It prints nothing and exits 0 on arrival, 1 on
timeout, so `beb wait && beb read` is a complete event loop.

Every ack names the next step; every refusal names the fix.

## Identity

Identity resolves to `./.beb` in the working directory, or to the
`.beb` of the directory named by `BEB_IDENTITY` for processes that
cannot cd (a launchd job, a supervisor's child). There is no
precedence: when both are present they must agree (by public key,
not path), and disagreement is a refusal naming both fixes. Nothing
global, no default: a process that has not been told who it is
refuses to be anyone.

```sh
(cd ~/work/backend && beb send frontend "migration ready")
BEB_IDENTITY=~/work/backend beb send frontend "migration ready"
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

An address is a public key, so it needs an authentic channel, never
a private one. Where you already have ssh to the other machine, the
exchange is two one-liners:

```sh
# learn theirs
echo "pve $(ssh pve 'cd ~/work && beb whoami')" >> ~/.config/beb/known_signers

# hand them yours
echo "mac $(beb whoami)" | ssh pve 'cat >> ~/.config/beb/known_signers'
```

Anywhere else, `beb whoami` is one line of text: paste it through
whatever channel you already trust, and prepend the name you want
to call them. Only first contact needs this. A reply never does:
`beb list` prints an unknown sender's key in exactly the form
`send` accepts, so you can answer a stranger before deciding what
to call them.

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

## Across machines

A message travels as an mbeb: the exact signed envelope and its
detached signature, safely framed, self-contained.

```sh
beb pack bob "the schema is ready" > note.mbeb   # sign, don't deliver
beb pack bob < report.md | ssh host beb receive  # any pipe is a transport
```

`pack` writes the delivery to stdout and touches no mailbox;
`.mbeb` is the conventional name when one is saved as a file.
`receive` verifies everything (envelope, mailbox, signature)
before installing the delivery into the mailbox its own `to:` names,
as an ordinary local message: same ids, same guarantees, and a
parked `beb wait` notices it like any other arrival. It resolves no
identity and needs no private key, so it runs anywhere on the
machine; a delivery for a key with no mailbox here is refused,
naming `beb init`, because running init is what makes an identity
live here and mail from outside may not conjure a mailbox nobody
reads. Reading stays yours alone: `read` resolves the identity under
your feet and refuses anything addressed elsewhere. Receive is
idempotent over retained history: the same delivery presented again
acks the existing id without installing a second copy, so a
store-and-forward carrier may retry freely.

beb still never touches a network: `pack` makes bytes, `receive`
accepts bytes, and how they travel (ssh, http, a copied file) is
your choice.

The full design, including the envelope format, the delivery
guarantees, and what beb refuses to know, is in
[DESIGN.md](DESIGN.md).

## License

MIT
