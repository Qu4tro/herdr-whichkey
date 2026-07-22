#!/usr/bin/env bash
# Install-time build: prefer a sha256-pinned prebuilt release binary for
# this host triple, fall back to cargo. herdr runs this via [[build]] on
# plugin install/update, from the plugin root.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="Qu4tro/herdr-whichkey"
VERSION="v0.2.2"
DEST_DIR="target/release"

# Pinned sha256 of each release tarball, from the release's SHA256SUMS
# asset. Written by scripts/pin-release.sh, which release.yml runs after
# publishing — edit these by hand and the next release overwrites you. An
# empty pin means "no prebuilt for this triple yet" — the source build
# takes over.
sha_for() {
  case "$1" in
    x86_64-unknown-linux-gnu)  echo "d1fc2684af268e901efa80201ff9fc327892cba3806b2caa913a78eb72d54e5f" ;;
    aarch64-unknown-linux-gnu) echo "7db5edc455c695890237626bfd6c5f09507fa6f1d85f90d54041fadf91814bfb" ;;
    aarch64-apple-darwin)      echo "0249407d00875a3bc78fa81a28557e033c38a75adda96f9d49546123e8e343ee" ;;
    x86_64-apple-darwin)       echo "3cbb3ed7a855d485bf2b8e95c64cd31bf189b02f763ed43d5625c7c5bd8f3ebf" ;;
    *)                         echo "" ;;
  esac
}

host_triple() {
  local os arch
  case "$(uname -s)" in
    Linux)  os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *) return 1 ;;
  esac
  case "$(uname -m)" in
    x86_64 | amd64)  arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) return 1 ;;
  esac
  echo "${arch}-${os}"
}

checksum_ok() { # file expected-sha
  if command -v sha256sum >/dev/null 2>&1; then
    echo "$2  $1" | sha256sum -c - >/dev/null 2>&1
  else
    echo "$2  $1" | shasum -a 256 -c - >/dev/null 2>&1
  fi
}

fetch_prebuilt() {
  local triple sha url tmp
  triple="$(host_triple)" || return 1
  sha="$(sha_for "$triple")"
  [ -n "$sha" ] || return 1
  command -v curl >/dev/null 2>&1 || return 1

  tmp="$(mktemp -d)"
  # shellcheck disable=SC2064  # expand $tmp now, not at exit
  trap "rm -rf '$tmp'" EXIT
  url="https://github.com/$REPO/releases/download/$VERSION/herdr-whichkey-$VERSION-$triple.tar.gz"
  curl -fsSL --retry 2 -o "$tmp/pkg.tar.gz" "$url" || return 1
  if ! checksum_ok "$tmp/pkg.tar.gz" "$sha"; then
    echo "herdr-whichkey: checksum mismatch for $url — refusing prebuilt, building from source" >&2
    return 1
  fi
  # Explicit `|| return 1` on every step below, not just the ones above: this
  # function is called as an `if` condition, which disables errexit for its
  # whole body. Without these a half-finished tar would fall through to chmod,
  # whose success would be reported as the function's — an "installed prebuilt"
  # log over a truncated binary.
  mkdir -p "$DEST_DIR" || return 1
  tar -xzf "$tmp/pkg.tar.gz" -C "$DEST_DIR" herdr-whichkey || return 1
  chmod +x "$DEST_DIR/herdr-whichkey" || return 1
}

if fetch_prebuilt; then
  echo "herdr-whichkey: installed prebuilt $VERSION"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "herdr-whichkey: no prebuilt for this platform and cargo not found — install Rust (https://rustup.rs) and retry" >&2
  exit 1
fi
echo "herdr-whichkey: building from source (no prebuilt for this platform/version)"
# --locked so an install builds the dependency versions the release tested,
# not whatever resolves today.
cargo build --release --locked
