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

One agent mails another that is not running yet; when it starts it
reads, replies, and the reply lands immediately.

https://github.com/user-attachments/assets/a4ca60ce-7abc-4893-8553-e4457ed7bd81

```
beb          signs, stores and reads mail on one machine
beb-courier  carries it between machines
beb-depot    holds it when two machines cannot reach each other

identity ─ beb ─ courier ─────── courier ─ beb ─ identity
                         \     /
                          depot
                        (optional)
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
(cd backend && beb init backend)
(cd frontend && beb init frontend)
```

Each `init` creates a `.beb/` holding an ed25519 keypair and a
`.gitignore` that keeps the key out of your repo, and claims that
identity's mailbox in this machine's spool.

`BEB_IDENTITY` names that directory, and anything that signs or reads
takes the key from there and nowhere else: not the working directory, so
`cd` moves the shell and never the signer. Set it once, wherever you
decide such things:

```sh
export BEB_IDENTITY=$PWD/frontend
```

`init` named each one in `~/.config/beb/known_signers` as it went, so
they can already address each other. That file is yours to edit; `init`
is the only thing in beb that writes to it, and it only ever appends.
A name already taken is a refusal, not a second line.

A name like `backend` is a local alias in `known_signers`. The key it
resolves to is the identity, and the address couriers route on is
derived from that key.

`beb contacts` reads it back in the file's own format, so a line can be
pasted into someone else's:

```console
$ beb contacts
beb: 2 of 2 names in ~/.config/beb/known_signers; frontend is this identity
backend   ssh-ed25519 AAAA...
frontend  ssh-ed25519 AAAA...
```

Everything beb says about a result goes to stderr with a `beb: `
prefix, which is what makes `2>&1 | grep -v '^beb:'` give you back
exactly the result.

Mail flows by name:

```sh
export BEB_IDENTITY=$PWD/frontend
echo "auth endpoint ready" | beb send backend --subject "endpoint ready"

export BEB_IDENTITY=$PWD/backend
beb list --unread --limit 5
                # beb: showing 1; cursor at 0; read next is 1
                # 1  now  endpoint ready  frontend
beb read        # auth endpoint ready
```

For a shell, direnv or a line in your profile pins it. For an agent, the
harness does.

All of that is one machine. beb owns no network, so carrying mail
between machines is [beb-courier](https://github.com/getbeb/beb-courier)
and [beb-depot](https://github.com/getbeb/beb-depot)'s job.

## Agents

| harness | plugin | announce at turn end | wake an idle session |
|---|---|---|---|
| Claude Code | [claude-beb](https://github.com/getbeb/claude-beb) | yes | yes |
| Codex | [codex-beb](https://github.com/getbeb/codex-beb) | yes | no |
| pi | [pi-beb](https://github.com/getbeb/pi-beb) | yes | yes |

Wake policy is handled by the agent's runtime plugins. As a rule,
nothing interrupts mid-turn. Each fixes one identity for the session and
announces mail without reading it.

## Commands

```console
$ beb
beb 0.11.0 delivers signed messages between identities.

  beb init NAME
      a new identity in this directory, and a name resolving to it
  beb whoami
      your address, and the name that resolves to it here
  beb contacts
      every name this machine resolves, as known_signers lines

  beb send RECIPIENT --subject S [--body B]
      sign and deliver; the body comes from --body or stdin
  beb list (--unread | --after ID | --before ID) --limit N
      read-only, newest first. --unread is what you have not read;
      --after/--before exclude ID, take the N nearest it, and reach
      read mail. One of the three, and always a limit
  beb read
      the next unread message; moves the cursor past it
  beb peek ID
      one message by id; the cursor does not move
  beb wait [--from ID] [--timeout SECS]
      block until there is unread mail; prints the mark to wait from next

  beb pack RECIPIENT --subject S [--body B]
      sign one delivery onto stdout
  beb drop
      install one delivery from stdin
  beb sign NAMESPACE
      sign stdin as this identity; the signature goes to stdout

  beb --help
      this list
  beb --version
      the version alone

Exit: 0 did it, 1 change the command, 2 nothing to do, 3 refused.

BEB_IDENTITY names the directory holding the .beb to act as. Anything
that signs, reads, or is the identity needs one:

  export BEB_IDENTITY=/path/to/dir

init, drop and contacts do not.
```

`wait` blocks on a kernel watch rather than polling and returns as soon
as there is unread mail, so a worker sleeps until there is work and
never sleeps while work is waiting:

```sh
while beb wait; do beb read | ./handle-job; done
```

It waits from your cursor by default. A waiter that must not fire twice
for the same mail (a doorbell that wakes a session, say) keeps its own
mark instead: `wait` prints the next mark on stdout, and `--from` takes
it back.

```sh
m=$(beb wait --timeout 0)
while :; do
    m=$(beb wait --from "$m" --timeout 900) && notify
done
```

## Design

beb owns no network: `pack` writes a delivery to stdout and `drop`
installs one from stdin, and [beb-courier](https://github.com/getbeb/beb-courier)
with [beb-depot](https://github.com/getbeb/beb-depot) are one pair that
carries between them.

[DESIGN.md](DESIGN.md) has the envelope format, the delivery
guarantees, and what beb refuses to know.

## License

MIT
