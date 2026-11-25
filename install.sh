#!/bin/sh
# Install or update grove from the latest GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/dschoeni/grove-cli/main/install.sh | sh
#
# Re-run to update. Environment:
#   GROVE_VERSION       tag to install (default: latest release), e.g. v0.3.0
#   GROVE_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
set -eu

REPO="dschoeni/grove-cli"
INSTALL_DIR="${GROVE_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '[grove-install] %s\n' "$*" >&2; }
die() { say "error: $*"; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "'$1' is required"; }
need curl
need tar
need uname
need mktemp

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) die "unsupported OS '$os' (Windows: download the .zip from https://github.com/$REPO/releases)" ;;
esac
case "$arch" in
  x86_64 | amd64)  arch_part="x86_64" ;;
  aarch64 | arm64) arch_part="aarch64" ;;
  *) die "unsupported architecture '$arch'" ;;
esac
target="$arch_part-$os_part"

version="${GROVE_VERSION:-}"
if [ -z "$version" ]; then
  # Follow the /releases/latest redirect instead of the API to avoid rate limits.
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") \
    || die "could not determine the latest release"
  version=${url##*/}
  case "$version" in v*) ;; *) die "could not determine the latest release (got '$url')" ;; esac
fi

name="grove-$version-$target"
base="https://github.com/$REPO/releases/download/$version"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading grove $version for $target"
curl -fsSL -o "$tmp/$name.tar.gz" "$base/$name.tar.gz" || die "download failed: $base/$name.tar.gz"

if curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" 2>/dev/null; then
  expected=$(awk -v f="$name.tar.gz" '$2 == f || $2 == "*"f { print $1 }' "$tmp/SHA256SUMS")
  [ -n "$expected" ] || die "$name.tar.gz is not listed in SHA256SUMS"
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$name.tar.gz" | awk '{ print $1 }')
  elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmp/$name.tar.gz" | awk '{ print $1 }')
  else
    actual=""
    say "warning: no sha256sum/shasum found, skipping checksum verification"
  fi
  [ -z "$actual" ] || [ "$actual" = "$expected" ] || die "checksum mismatch for $name.tar.gz"
else
  say "warning: SHA256SUMS not found, skipping checksum verification"
fi

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
[ -f "$tmp/$name/grove" ] || die "archive did not contain $name/grove"

mkdir -p "$INSTALL_DIR"
# Install via a temp file + rename so updating a running grove can't fail with ETXTBSY.
cp "$tmp/$name/grove" "$INSTALL_DIR/.grove.new"
chmod 755 "$INSTALL_DIR/.grove.new"
mv -f "$INSTALL_DIR/.grove.new" "$INSTALL_DIR/grove"

say "installed grove $version to $INSTALL_DIR/grove"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) say "note: $INSTALL_DIR is not on your PATH; add it, e.g.: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

if [ "$os" = "Linux" ] && ! command -v bwrap >/dev/null 2>&1; then
  say "note: 'bwrap' not found; install bubblewrap for sandboxing (or use --no-sandbox)"
fi
