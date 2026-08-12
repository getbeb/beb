#!/bin/sh
# Install beb-ssh: detect the platform, fetch the latest release binary,
# verify its checksum, place it on PATH.
#
#   curl -fsSL https://getbeb.dev/beb-ssh.sh | sh
#
# BEB_SSH_INSTALL_DIR overrides the destination (default ~/.local/bin).
set -eu

REPO=getbeb/beb-ssh
BIN_DIR=${BEB_SSH_INSTALL_DIR:-$HOME/.local/bin}

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin) os=apple-darwin ;;
    Linux) os=unknown-linux-musl ;;
    *) echo "unsupported OS: $os; build from source with: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
esac
case "$arch" in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) echo "unsupported architecture: $arch; build from source with: cargo install --git https://github.com/$REPO" >&2; exit 1 ;;
esac
target=$arch-$os

url=https://github.com/$REPO/releases/latest/download
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$url/beb-ssh-$target" -o "$tmp/beb-ssh"
curl -fsSL "$url/SHA256SUMS" -o "$tmp/SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
    have=$(sha256sum "$tmp/beb-ssh" | awk '{print $1}')
else
    have=$(shasum -a 256 "$tmp/beb-ssh" | awk '{print $1}')
fi
want=$(awk -v f="beb-ssh-$target" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
if [ -z "$want" ] || [ "$have" != "$want" ]; then
    echo "checksum mismatch for beb-ssh-$target; not installed" >&2
    exit 1
fi

mkdir -p "$BIN_DIR"
install -m 755 "$tmp/beb-ssh" "$BIN_DIR/beb-ssh"
echo "installed $BIN_DIR/beb-ssh"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) echo "note: $BIN_DIR is not on PATH; add it to your shell profile" ;;
esac
command -v beb >/dev/null 2>&1 ||
    echo "note: beb-ssh needs beb 0.4.0+ on PATH; install it with: curl -fsSL https://getbeb.dev/install.sh | sh" >&2
