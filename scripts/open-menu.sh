#!/usr/bin/env bash
# Launcher for the whichkey menu popup.
#
# Opens the plugin pane and then blocks until the menu binary signals it is
# done. Staying alive matters: herdr may tear popup views down the moment the
# invoking action exits (observed on 0.7.1, unconfirmed on 0.7.4 — docs/spike-popup-panes.md),
# so the safe lifecycle under both semantics is action-lives-as-long-as-menu.
set -euo pipefail

STATE_DIR="${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}/herdr-whichkey}"
mkdir -p "$STATE_DIR"
FIFO="$STATE_DIR/menu-$$.done"
mkfifo "$FIFO"
trap 'rm -f "$FIFO"' EXIT

OUT=$("$HERDR_BIN_PATH" plugin pane open \
  --plugin herdr-whichkey --entrypoint menu \
  --placement popup --width 100% --height 8 --focus \
  --env "WHICHKEY_DONE_FIFO=$FIFO" 2>&1) || true

case "$OUT" in
  *popup\ already\ open*)
    # Menu is already up — treat the trigger as a no-op rather than an error.
    exit 0
    ;;
  *error*)
    echo "herdr-whichkey: pane open failed: $OUT" >&2
    exit 1
    ;;
esac

# Block until the menu exits (fifo write), with a safety timeout so a wedged
# menu can never pin the action forever.
read -t 600 -r _ < "$FIFO" || true
