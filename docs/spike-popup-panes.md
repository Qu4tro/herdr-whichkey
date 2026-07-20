# Spike findings — herdr 0.7.4 popup panes (2026-07-20)

Method: headless scratch session (`herdr --session wkspike server`), throwaway
plugin with a `popup`-placement pane entrypoint and two actions (open-then-exit,
open-then-wait), invoked via CLI. A tmux-hosted `session attach` client was
attempted for rendering checks but silently exited early, so **no rendering
observation below is valid** — display-level questions move to live validation.

## Confirmed (server-side, reliable)

- `herdr plugin pane open --plugin X --entrypoint Y --placement popup --width 100% --height 8 --focus`
  succeeds; the entrypoint process starts and runs.
- **Popup is a singleton per plugin**: second open fails with
  `plugin_pane_open_failed: "popup already open"`. Menu binary must ensure
  clean exit and the launcher must handle the already-open error (treat as
  "toggle/no-op" or close-and-reopen).
- **Popup has no pane id**: absent from `pane list`, `api snapshot` panes and
  layouts. It cannot be addressed by `pane read`/`send-keys`/`plugin pane close`.
  Lifecycle = the entrypoint process's lifetime (exit process → slot frees).
- **Popup persists server-side after the invoking action exits** (invocation
  via CLI/api at least). floax documented teardown-on-action-exit for
  keybinding-invoked transient views on 0.7.1; unverified here for the
  `keybinding` invocation source on 0.7.4. **Design consequence: the action
  (launcher) stays alive until the menu closes — correct under both semantics.**
- `HERDR_PLUGIN_CONTEXT_JSON` is injected into both action and pane processes:
  `workspace_id/label/cwd`, `tab_id/label`, `focused_pane_id`,
  `focused_pane_cwd`, `focused_pane_status`, `invocation_source`,
  `correlation_id`. This is the source for `{pane}/{tab}/{workspace}/{cwd}`
  placeholders.
- **Trap: `HERDR_TAB_ID` / `HERDR_WORKSPACE_ID` in the spawned pane's env can
  be inherited from the *invoking CLI's* environment** (observed: values from
  the pane I invoked from, in a different session). Never read those in the
  plugin; use `HERDR_PLUGIN_CONTEXT_JSON` only.
- Actions additionally get `HERDR_PLUGIN_ACTION_ID` and `HERDR_PANE_ID` (the
  focused pane at invocation).
- `--placement split --direction down` from `plugin pane open` returns a full
  pane object with a real `pane_id` — the fallback surface if popups can't
  dock bottom.
- Nested-herdr guard: a herdr client refuses to start inside a herdr pane
  unless configured otherwise ("recursive descent denied") — dev/testing
  concern only.
- **Popups need the normal workspace view**: `plugin pane open --placement
  popup` fails with `ui_busy: "popup panes can only open from the normal
  workspace view"` when no workspace is focused (observed on a fresh headless
  server before any `workspace focus`; likely also applies to alt views like
  pickers). The launcher surfaces this via its pane-open-failed path; the
  keybinding just does nothing visible. Live validation should check which
  user-reachable views trigger it.
- **Death by signal**: if the menu binary dies by SIGTERM/SIGHUP (herdr
  teardown), Rust `Drop` never runs — the binary installs a signal handler
  that writes the done-fifo itself, and all fifo writes are `O_NONBLOCK`
  (a reader-less fifo would otherwise block our own exit forever).
  Verified: SIGTERM → launcher unblocks immediately.

## Resolved by live validation (2026-07-20, herdr 0.7.4)

The four open questions, answered with the user at a real client:

1. Keybinding-invoked popups **render** and input routes to them. ✓
2. Geometry: `--width 100% --height 8` floats **centered** — popups cannot
   dock. `overlay` placement is full-screen (rejects width/height) but
   returns a **real pane_id** (addressable, closable — unlike popups).
   **Decision: ship `split --direction down` as the surface**, self-fit to
   8 rows (below).
3. `--focus` routes input; focus returns to the previous pane on close —
   for splits it falls to the split sibling, which is the same pane. ✓
4. Theme matched (gruvbox) with no flashes or flicker. ✓

Split-surface findings that shaped the implementation:

- `plugin pane open --placement split` always opens at **ratio 0.5** and
  takes no ratio flag; `layout.set_split_ratio` exists in the API but has
  no CLI. The binary therefore resizes itself before its first frame.
- `pane resize --amount` is an **exact ratio delta** on the split that
  contains the pane in that direction. Magnitude is capped per call
  (~0.5), ratios clamp to [0.1, 0.9] (so on very tall tabs the strip
  bottoms out at 10% instead of 8 rows), negative amounts do not reverse
  direction, and resizing a bottom-most pane "down" is a no-op — shrink
  the bottom pane by growing the sibling above it downward.
- Rows are laid out as `round(height × ratio)`: compute deltas from the
  **stored split ratio** (`pane layout` → `splits[].ratio`), not from row
  counts, or the result is off by one.
- Split plugin panes are **not singletons** — a second open stacks a
  second pane. The launcher holds a lock directory with the open pane id;
  a second press sends `ctrl+c` to that pane (split panes are
  addressable; `pane send-keys` accepts herdr key-string names), turning
  re-invocation into a toggle.
- Split pane processes get their **own** id in `HERDR_PANE_ID`
  (server-injected, verified against a cross-session caller) and the
  original pane's id as `focused_pane_id` in `HERDR_PLUGIN_CONTEXT_JSON`
  — everything the self-fit resize needs.
- The pane closes when its process exits, same as popups; the launcher's
  fifo/lock cleanup was verified through invoke → toggle → state dir
  empty.
