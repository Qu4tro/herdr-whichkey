#!/usr/bin/env bash
# Install-time build: prefer a sha256-pinned prebuilt release binary for
# this host triple, fall back to cargo. herdr runs this via [[build]] on
# plugin install/update, from the plugin root.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="Qu4tro/herdr-whichkey"
VERSION="v0.3.0"
DEST_DIR="target/release"

# Pinned sha256 of each release tarball, from the release's SHA256SUMS
# asset. Written by scripts/pin-release.sh, which release.yml runs after
# publishing — edit these by hand and the next release overwrites you. An
# empty pin means "no prebuilt for this triple yet" — the source build
# takes over.
sha_for() {
  case "$1" in
    x86_64-unknown-linux-gnu)  echo "0b8a13a2e2ec562ed48abad841a01ae8cea5555ba220dcb5307dbb04196fbd74" ;;
    aarch64-unknown-linux-gnu) echo "0a58794dd9bd9f21657a7b4b35a6f99c3d5bd8117a100ca8a16f481c6f5cd44f" ;;
    aarch64-apple-darwin)      echo "d1b4b29d58042fd2537d2fcd6fd68d28a3732dbd9f351423282faf7f8d48e61f" ;;
    x86_64-apple-darwin)       echo "7a79ec52d7017b427a987f6939fea65c2c09dbeafd99cc2245618fbf8eace76d" ;;
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
  local triple sha url tmp stage
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
  # Unpack into a staging dir and rename into place as the very last step, so
  # a tar that dies partway leaves its debris in staging rather than a
  # truncated herdr-whichkey at the path [[panes]] runs. Staging lives under
  # DEST_DIR, not $tmp, to keep that last step a same-filesystem rename: a
  # cross-device mv copies, and a copy can fail half-written.
  #
  # Every step carries its own `|| return 1`. errexit cannot be relied on here
  # — the `if fetch_prebuilt` call site puts the whole function body in a
  # condition context, which disables it.
  mkdir -p "$DEST_DIR" || return 1
  stage="$(mktemp -d "$DEST_DIR/.stage-XXXXXX")" || return 1
  # shellcheck disable=SC2064  # expand both paths now, not at exit
  trap "rm -rf '$tmp' '$stage'" EXIT
  tar -xzf "$tmp/pkg.tar.gz" -C "$stage" herdr-whichkey || return 1
  chmod +x "$stage/herdr-whichkey" || return 1
  mv "$stage/herdr-whichkey" "$DEST_DIR/herdr-whichkey" || return 1
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
