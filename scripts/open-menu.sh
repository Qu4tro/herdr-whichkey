#!/usr/bin/env bash
# Launcher for the whichkey menu pane.
#
# Opens the menu on whatever surface `[layout] placement` asks for — a
# split below the focused pane (bottom), a split to its right (right), or
# a centered float (popup) — and blocks until the menu binary signals
# done. Staying alive matters: herdr may tear plugin views down the moment
# the invoking action exits (docs/spike-popup-panes.md).
#
# A lock directory turns re-invocation into a toggle. It holds whatever
# the second press needs to close the menu that is already up: the pane id
# for a split (`pane send-keys ctrl+c`), and the menu's own pid, which the
# binary writes there — a popup has no pane id anywhere in herdr, so
# signalling the process is the only handle on it.
set -euo pipefail

ROOT="${HERDR_PLUGIN_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
BIN="$ROOT/target/release/herdr-whichkey"

STATE_DIR="${HERDR_PLUGIN_STATE_DIR:-${TMPDIR:-/tmp}/herdr-whichkey}"
mkdir -p "$STATE_DIR"
LOCK="$STATE_DIR/menu.lock" # mkdir is atomic — the open instance owns it

# What to open is a whichkey.toml question, and the binary is what parses
# whichkey.toml. It answers with defaults if the config is unreadable, so
# a broken config still opens a menu — which is what renders the error.
PLACEMENT=bottom
WIDTH=0
HEIGHT=0
if RESOLVED=$("$BIN" surface 2>/dev/null); then
  while IFS='=' read -r key value; do
    case "$key" in
      placement) PLACEMENT="$value" ;;
      width) WIDTH="$value" ;;
      height) HEIGHT="$value" ;;
    esac
  done <<<"$RESOLVED"
fi

# True while the lock still has an owner. Either a launcher we can see
# running, or one so new it hasn't stamped its pid yet: `mkdir` claims the
# lock and the stamp lands a moment later, and a press that reads the
# empty dir in between must not take it for a crash leftover and reclaim
# it — that is two menus on screen, and one launcher's trap deleting the
# other's live lock.
lock_has_owner() {
  local owner
  owner=$(cat "$LOCK/launcher" 2>/dev/null || true)
  if [ -n "$owner" ]; then
    # Matched on the command line, not just liveness: pids get recycled,
    # and a stranger holding this one would wedge the menu shut forever.
    case "$(ps -p "$owner" -o args= 2>/dev/null || true)" in
      *open-menu.sh*) return 0 ;;
    esac
    return 1
  fi
  [ -n "$(find "$LOCK" -maxdepth 0 -mmin -1 2>/dev/null || true)" ]
}

# True when the menu that owns the lock was told to close itself.
close_open_menu() {
  local pane pid
  pane=$(cat "$LOCK/pane" 2>/dev/null || true)
  if [ -n "$pane" ] && "$HERDR_BIN_PATH" pane send-keys "$pane" ctrl+c >/dev/null 2>&1; then
    return 0
  fi
  # No pane id (popup), or the pane is already gone: signal the process.
  # Only after checking it is still ours — pids get recycled, and SIGTERM
  # to a stranger is not a toggle. The binary's handler writes the
  # done-fifo and restores the terminal, same as ctrl+c.
  pid=$(cat "$LOCK/pid" 2>/dev/null || true)
  case "$(ps -p "${pid:-0}" -o comm= 2>/dev/null || true)" in
    *herdr-whichkey) kill -TERM "$pid" 2>/dev/null && return 0 ;;
  esac
  return 1
}

if ! mkdir "$LOCK" 2>/dev/null; then
  if close_open_menu; then
    exit 0 # toggle: told the open menu to close itself
  fi
  if lock_has_owner; then
    exit 0 # still opening — let it finish rather than race it
  fi
  rm -rf "$LOCK" # stale (crash leftover) — reclaim and open fresh
  mkdir "$LOCK" 2>/dev/null || exit 0
fi
# Stamp the lock before anything slow: opening the pane takes long enough
# for a second press to land inside it, and this is what tells that press
# the menu is on its way up.
printf '%s\n' "$$" >"$LOCK/launcher"

FIFO="$STATE_DIR/menu-$$.done"
mkfifo "$FIFO"
trap 'rm -rf "$LOCK" "$FIFO"' EXIT

case "$PLACEMENT" in
  right) PLACE=(--placement split --direction right) ;;
  # Popups are the one surface herdr sizes at open; splits open at ratio
  # 0.5 whatever you ask for and the binary shrinks itself instead.
  popup) PLACE=(--placement popup --width "$WIDTH" --height "$HEIGHT") ;;
  *) PLACE=(--placement split --direction down) ;;
esac

if ! OUT=$("$HERDR_BIN_PATH" plugin pane open \
  --plugin herdr-whichkey --entrypoint menu \
  "${PLACE[@]}" --focus \
  --env "WHICHKEY_DONE_FIFO=$FIFO" \
  --env "WHICHKEY_LOCK_DIR=$LOCK" \
  --env "WHICHKEY_SURFACE=$PLACEMENT" 2>&1); then
  # Popups are singletons: herdr refuses a second one. Getting here means
  # a menu is up that we found no lock for, so leave it alone rather than
  # fight it — Esc still closes it. The EXIT trap drops the lock we took,
  # so the next press opens fresh.
  case "$OUT" in
    *'popup already open'*) exit 0 ;;
  esac
  echo "herdr-whichkey: pane open failed: $OUT" >&2
  exit 1
fi

# Splits come back as a pane object; popups have no pane id at all, and
# are closed through the pid the binary writes into the lock instead. The
# `|| true` is load-bearing under `set -e`: grep exits 1 when it matches
# nothing, which is every popup, and would take the launcher down with it
# before it ever reached the fifo below.
PANE_ID=$(printf '%s' "$OUT" | grep -o '"pane_id":"[^"]*"' | head -n1 | cut -d'"' -f4 || true)
if [ -n "$PANE_ID" ]; then
  printf '%s\n' "$PANE_ID" >"$LOCK/pane"
fi

# Block until the menu exits (fifo write), with a safety timeout so a
# wedged menu can never pin the action forever.
read -t 600 -r _ <"$FIFO" || true
