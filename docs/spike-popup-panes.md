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

## herdr 0.7.5 re-verification (2026-07-22, for the placement work)

Everything above still holds on 0.7.5. What changed is the **help text**,
not the behaviour — `plugin pane open --help` now advertises only
`overlay | split | tab | zoomed` and lists no `--width`/`--height`, but
`--placement popup` still parses and both size flags are still accepted
(`--ratio` is the one that really doesn't exist: "unknown option").
Probed the way the flags were probed the first time, with an unknown
`--plugin` so validation errors surface before any pane opens.

- `--direction` still takes **only** `down` and `right`; `up` and `left`
  are rejected with `invalid split direction: up`. **top and left remain
  inexpressible**, which is why the placement enum is bottom|right|popup.
- `--placement` on the CLI **overrides the manifest's** `[[panes]]
  placement`, so one `menu` entrypoint serves all three surfaces.
- The CLI exit code is trustworthy: 0 on open, 1 on `popup already open`
  and on an unknown plugin. The launcher branches on it rather than on
  the shape of the JSON.

Measured live against a real client (a herdr client in a PTY the harness
owns, rendered with pyte — herdr reads from `/dev/tty`, so the child needs
the slave as its controlling terminal):

- **A popup is a bordered box**, `--width`/`--height` are its *outer*
  size, and the interior is 2 cols and 2 rows smaller. Percentages are of
  the host screen; the default with neither flag is 50%/50%; sizes clamp
  to the screen.
- The border **title is the manifest's** `[[panes]] title` and nothing
  can change it: a popup has no pane id, so `pane rename` cannot address
  it, and **OSC 2 from inside the pane does not retitle it either**
  (verified). The breadcrumb therefore lives in the popup's *body*, one
  row of it, where the split puts it in the border title.
- The popup process gets **no `HERDR_PANE_ID` at all** (split panes get
  their own). The context-leak trap above still bites the neighbours —
  `HERDR_TAB_ID`/`HERDR_WORKSPACE_ID` in the popup's env were the
  *invoking CLI's*, from a different session — so nothing may read them.
- **No reflow, measured**: with the popup open the focused pane's rect is
  byte-identical before, during and after (`84×31 at 26,1`). Both split
  placements move it.
- **A focused popup swallows the trigger key.** herdr does not process
  its own `prefix+…` bindings while a popup has focus: pressing the
  whichkey binding again logs no action invocation at all (herdr's plugin
  log stays flat across three presses) and the keystrokes arrive in the
  popup as ordinary input. The same press *does* toggle a bottom split,
  which herdr keeps routing prefix keys for. So on the popup surface Esc
  and ctrl+c close the menu, and press-again-to-close is not available.
- Toggle-to-close still works at the launcher level, and has to be built
  differently: with no pane id there is nothing to `pane send-keys`, so
  the binary writes its pid into the launcher's lock and the second
  invocation signals it. Verified: second invocation → SIGTERM → done
  fifo → launcher unblocks → lock and fifo cleaned → reopen works.
