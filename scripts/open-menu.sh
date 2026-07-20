#!/usr/bin/env bash
# Launcher for the whichkey menu pane (bottom split).
#
# Opens the plugin pane as a split below the focused pane — popups can
# only float centered on herdr 0.7.4 — and blocks until the menu binary
# signals done. Staying alive matters: herdr may tear plugin views down
# the moment the invoking action exits (docs/spike-popup-panes.md).
#
# Split panes are not singletons (only popups get "popup already open"),
# so a lock directory holds the open pane's id: a second press finds the
# lock and closes the menu (toggle) instead of stacking another one.
set -euo pipefail

STATE_DIR="${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}/herdr-whichkey}"
mkdir -p "$STATE_DIR"
LOCK="$STATE_DIR/menu.lock" # mkdir is atomic — the open instance owns it

if ! mkdir "$LOCK" 2>/dev/null; then
  PANE=$(cat "$LOCK/pane" 2>/dev/null || true)
  if [ -n "$PANE" ] && "$HERDR_BIN_PATH" pane send-keys "$PANE" ctrl+c >/dev/null 2>&1; then
    exit 0 # toggle: told the open menu to close itself
  fi
  rm -rf "$LOCK" # stale (crash leftover) — reclaim and open fresh
  mkdir "$LOCK" 2>/dev/null || exit 0
fi

FIFO="$STATE_DIR/menu-$$.done"
mkfifo "$FIFO"
trap 'rm -rf "$LOCK" "$FIFO"' EXIT

OUT=$("$HERDR_BIN_PATH" plugin pane open \
  --plugin herdr-whichkey --entrypoint menu \
  --placement split --direction down --focus \
  --env "WHICHKEY_DONE_FIFO=$FIFO" \
  --env "WHICHKEY_SURFACE=split" 2>&1) || true

PANE_ID=$(printf '%s' "$OUT" | grep -o '"pane_id":"[^"]*"' | head -n1 | cut -d'"' -f4)
if [ -z "$PANE_ID" ]; then
  echo "herdr-whichkey: pane open failed: $OUT" >&2
  exit 1
fi
printf '%s\n' "$PANE_ID" >"$LOCK/pane"

# Block until the menu exits (fifo write), with a safety timeout so a
# wedged menu can never pin the action forever.
read -t 600 -r _ <"$FIFO" || true
