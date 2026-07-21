#!/usr/bin/env bash
# Point scripts/build.sh at a published release: set VERSION and pin the
# sha256 of each prebuilt tarball, read from that release's SHA256SUMS asset.
#
#   scripts/pin-release.sh v0.2.3
#
# release.yml runs this after publishing, so the pins cannot be forgotten.
# They were, for v0.2.1 and v0.2.2: every arm of sha_for() shipped empty,
# fetch_prebuilt() bailed on the empty pin, and every install quietly built
# from source instead. Nothing failed, so nothing surfaced it.
#
# A tarball's checksum does not exist until the tarball is built, so the
# commit tagged vX can never contain vX's own pins — this always lands as a
# follow-up commit on main.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO="${REPO:-Qu4tro/herdr-whichkey}"
BUILD_SH="scripts/build.sh"

# The triples build.sh pins, and release.yml's build matrix produces. A
# release missing any of them is a broken release, not a partial one.
TRIPLES=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  aarch64-apple-darwin
  x86_64-apple-darwin
)

version="${1:-}"
case "$version" in
  v*) ;;
  *)
    echo "usage: $0 <version>   (e.g. $0 v0.2.3)" >&2
    exit 2
    ;;
esac

# Never move VERSION backwards. Re-cutting an old tag after a newer release
# exists is not hypothetical here — v0.2.1 was deleted and re-cut as v0.2.2 —
# and its release would otherwise rewrite build.sh back down to the older
# version and four matching pins, land a green job, and leave installs on a
# stale binary with nothing failing. Re-pinning the same version stays a
# no-op, so re-running a release is still safe.
current="$(sed -n 's|^VERSION="\(.*\)"$|\1|p' "$BUILD_SH")"
if [ -n "$current" ] && [ "$version" != "$current" ] &&
  [ "$(printf '%s\n%s\n' "$version" "$current" | sort -V | head -1)" = "$version" ]; then
  echo "pin-release: $BUILD_SH is pinned to $current — refusing to downgrade it to $version" >&2
  exit 1
fi

tmp="$(mktemp -d)"
# shellcheck disable=SC2064  # expand $tmp now, not at exit
trap "rm -rf '$tmp'" EXIT

url="https://github.com/$REPO/releases/download/$version/SHA256SUMS"
if ! curl -fsSL --retry 2 -o "$tmp/SHA256SUMS" "$url"; then
  echo "pin-release: cannot fetch $url — is $version published?" >&2
  exit 1
fi

# Collect every sum before writing anything: a half-pinned build.sh, some
# triples on the new release and some still on the old, is worse than an
# untouched one.
sums=()
for triple in "${TRIPLES[@]}"; do
  sum="$(awk -v want="herdr-whichkey-$version-$triple.tar.gz" \
    '{ name = $2; sub(/^\.\//, "", name); if (name == want) print $1 }' \
    "$tmp/SHA256SUMS")"
  if ! [[ "$sum" =~ ^[0-9a-f]{64}$ ]]; then
    echo "pin-release: no sha256 for $triple in $version SHA256SUMS" >&2
    exit 1
  fi
  sums+=("$sum")
done

sed -E -i.bak "s|^VERSION=\".*\"\$|VERSION=\"$version\"|" "$BUILD_SH"
for i in "${!TRIPLES[@]}"; do
  sed -E -i.bak \
    "s|^([[:space:]]*${TRIPLES[$i]}\)[[:space:]]*echo \")[^\"]*(\" ;;)\$|\1${sums[$i]}\2|" \
    "$BUILD_SH"
done
rm -f "$BUILD_SH.bak"

# Read the values back out of the rewritten file rather than trusting the
# sed to have matched: a pattern that silently matched nothing would
# otherwise leave the old pins in place and still exit 0.
if ! grep -qE "^VERSION=\"$version\"\$" "$BUILD_SH"; then
  echo "pin-release: failed to set VERSION in $BUILD_SH" >&2
  exit 1
fi
for i in "${!TRIPLES[@]}"; do
  if ! grep -qE "^[[:space:]]*${TRIPLES[$i]}\)[[:space:]]+echo \"${sums[$i]}\" ;;\$" "$BUILD_SH"; then
    echo "pin-release: failed to pin ${TRIPLES[$i]} in $BUILD_SH" >&2
    exit 1
  fi
done

echo "pin-release: $BUILD_SH pinned to $version"
