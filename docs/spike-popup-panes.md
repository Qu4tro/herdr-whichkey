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

## Open — needs a live attached client (task: live UX validation)

1. Does a popup opened by a **keybinding-invoked** action render, and does it
   survive the action exiting? (Launcher-stays-alive makes the answer
   non-blocking either way.)
2. Geometry of `--width 100% --height 8`: bottom-docked strip or centered
   float? If centered-only → decide: accept centered strip vs `split
   --direction down` fallback (which reflows the focused pane briefly).
3. Does `--focus` route keyboard input to the popup process, and does focus
   return to the previous pane when the popup closes?
4. Theme match and flicker under the real client.
