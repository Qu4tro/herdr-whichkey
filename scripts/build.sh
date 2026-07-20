#!/usr/bin/env bash
# Install-time build: prefer a prebuilt release binary for this host triple,
# fall back to cargo. Runs from the plugin root (herdr sets cwd there).
set -euo pipefail

# TODO(distribution milestone): fetch sha256-pinned prebuilt from GitHub
# Releases for {x86_64,aarch64}-{unknown-linux-gnu,apple-darwin} before
# falling back to a source build.

if ! command -v cargo >/dev/null 2>&1; then
  echo "herdr-whichkey: no prebuilt available yet and cargo not found — install Rust (https://rustup.rs) and retry" >&2
  exit 1
fi

cargo build --release
