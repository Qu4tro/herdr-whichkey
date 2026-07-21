# Spike findings — mouse into a herdr 0.7.4 plugin pane (2026-07-21)

Does herdr forward mouse input from the outer terminal into a plugin pane's
PTY, and under which settings? Everything below is black-box: herdr ships as a
binary, there is no source to read.

Method: a Python harness runs a real herdr client inside a PTY it owns and
writes SGR mouse reports into the master, so herdr cannot tell them from a real
terminal's. Three details make or break that harness:

- The pty slave must be made the **controlling terminal** (`setsid` +
  `TIOCSCTTY`). herdr reads input from `/dev/tty`, not stdin — without it the
  client renders happily and receives nothing, which reads exactly like "herdr
  ignores injected input".
- `HERDR_CONFIG_PATH` points herdr at a throwaway config. A per-session
  `config.toml` under `~/.config/herdr/sessions/<name>/` is **not** read.
- The client screen is reconstructed with `pyte`, which is what turns "herdr
  drew something" into "the item labelled X is at these cells".

Detector: a pane app that requests mouse the way crossterm 0.28's
`EnableMouseCapture` does (`?1000h ?1002h ?1003h ?1015h ?1006h`) and logs every
byte it reads.

## Confirmed

- **Mouse events reach a pane app that requests them.** Press and release both
  arrive, SGR encoding intact, coordinates translated to **pane-relative** —
  host column 100 into a pane at `rect.x = 26` arrives as column 74. Hit-testing
  against `layout::positions()` needs no offset maths.
- **The first click into an unfocused pane is not swallowed**: focus moves to
  that pane *and* the click is delivered. The strip being a split the user isn't
  focused in was a non-issue.
- **A pane that never requests mouse receives nothing.** Degrading to
  keyboard-only is free: don't enable capture and nothing changes.
- **`ui.mouse_capture` is a bool in 0.7.4** (not the `off | press |
  press_release | button_motion | any_motion` enum this was designed against),
  and it does **not** gate delivery. With `mouse_capture = false` herdr enables
  no host mouse reporting at startup — but the moment a pane app asks for mouse,
  herdr turns host reporting on for it. Verified in both directions.
- What the setting really changes is **who eats which button**:

  | gesture | `mouse_capture = true` (default) | `= false` |
  | --- | --- | --- |
  | left click / drag | forwarded | forwarded |
  | shift+left, alt+left | forwarded | forwarded |
  | **right click** | **swallowed** (herdr's pane menu) | forwarded |
  | middle click, ctrl+click | release only (press eaten) | forwarded |
  | wheel | forwarded as buttons 64/65 | same |
  | motion, no button | forwarded (`?1003h`) | same |

- Clicks on herdr's own chrome — sidebar, tab bar, pane borders — are never
  forwarded. The breadcrumb lives in the pane **border title**, so "click the
  breadcrumb to ascend" is impossible: those clicks never reach us.
- **Teardown is herdr's job and it does it.** When the requesting pane dies —
  including `SIGKILL` — herdr sends `?1000l ?1002l ?1003l ?1006l` to the host
  under `mouse_capture = false`, and correctly keeps host mouse on under
  `= true`. The outer terminal cannot be left in mouse mode by the menu dying.
  Verified for all four exit paths: item click, Esc, the launcher's toggle
  (`pane send-keys ctrl+c`), and `SIGTERM`.
- Drag inside a pane is forwarded to the app, and so is shift+click — so with
  capture on there is **no shift-drag escape hatch** for selecting text over the
  strip's rows. The cost is small (a transient 7-row strip) but real, which is
  what `[ui] mouse = false` is for.

## Design consequences

- Ascend can't be right-click (swallowed by default) and can't be the breadcrumb
  (herdr chrome). It is **left click on empty strip space**: Backspace
  semantics, a no-op at the root, so a stray click is never destructive. Right
  click is wired to the same thing, which simply never fires for users on the
  default setting.
- Fire on **press**, not release: the release lands right behind it and would
  otherwise run a sticky item twice.
- Hover highlight is real — any-motion events do arrive. The menu repaints only
  when the hovered item changes, so the motion flood costs nothing.
- Scroll is ignored on purpose. herdr hands the wheel to the app instead of
  scrolling its own scrollback, and a menu that fits on screen has nothing to
  scroll.
