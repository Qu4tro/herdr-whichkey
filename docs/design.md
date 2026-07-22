# Design — herdr-whichkey

Agreed 2026-07-20, before implementation. This is the contract the milestones
build against; platform facts backing it are in
[spike-popup-panes.md](spike-popup-panes.md).

## What it is

A blezz/which-key-style action menu for herdr: one trigger key opens a hint
strip, then every subsequent press is a single keystroke — no typing, no Enter.
Nothing like it exists on the marketplace (herdr-command-palette,
herdr-navigator, and herdr-plus quick actions are all type-to-fuzzy-search);
herdr's own prefix mode is single-keystroke but blind, and navigate mode covers
navigation only.

## Decisions

### Surface: bottom strip by default, configurable

Full-width strip (~5–7 rows) in a split plugin pane, keys in columns, `»`
marks groups. Chosen over a centered popup (occludes content) and a
full-screen modal (most disruptive). Breadcrumb of the current group path in
the strip's border title.

**Open risk (resolved):** `plugin pane open` has `--width/--height` but no
position flag, so popups float centered and cannot dock. The surface is
`split --direction down`, which briefly reflows the focused pane — the cost
the no-flicker goal pays for docking.

**Amended (hw-16fc):** the default stands, the *only-choice* did not.
`[layout] placement` picks between three surfaces, because the trade-off is
a user's to make, not ours:

| placement | surface | reflows the focused pane | size knob |
|---|---|---|---|
| `bottom` (default) | split below | yes | `height`, rows |
| `right` | split beside | yes | `width`, columns |
| `popup` | centered float | **no** | `width` × `height`, outer cells |

`top` and `left` are **not expressible**: herdr 0.7.5's `--direction` takes
only `down` and `right` and rejects `up`/`left` outright, and the workaround
— split then move — reflows the focused pane twice, which is the one thing
this design is built to avoid. Cut rather than hacked (hw-y4df).

### Trigger and lifecycle

- Suggested binding: `prefix+space` via `[[keys.command]]` in herdr's
  `config.toml` (herdr 0.7 cannot bind keys from plugin manifests; README
  ships the snippet).
- The bound action runs a launcher that opens the popup and **stays alive
  until the menu closes** (done-fifo from the binary), because herdr may tear
  popup views down when the invoking action exits. Correct under both
  possible teardown semantics.
- A trigger while the menu is already open is a no-op ("popup already open").

### Leaf types (v1)

| Leaf | Meaning |
|---|---|
| `herdr = "…"` | herdr CLI call via `HERDR_BIN_PATH` against the socket, no shell |
| `action = "plugin.action"` | invoke any installed plugin's action |
| `run = "…"` | shell command, with `in = "background" \| "pane" \| "tab"` and optional `cwd` |

- Placeholders `{pane}` `{tab}` `{workspace}` `{cwd}` expand to the context
  captured when the menu opened — sourced from `HERDR_PLUGIN_CONTEXT_JSON`
  only (the plain env vars can leak from the caller; see spike).
- `~` and `$VARS` expand in commands and paths.
- Background-run failures surface via herdr notification + plugin log — never
  a UI interruption.
- Deferred to v2: send-keys-to-previous-pane leaves; running a TUI *inside*
  the strip (embedded PTY, floax-style).

### Config: flat key-paths, overlay for settings only

One TOML file in the plugin config dir (`herdr plugin config-dir
herdr-whichkey`). Keys are keystroke sequences in herdr's own key-string
format; `"g s"` = press `g` then `s`. Groups spring into existence when a
deeper path mentions them; labels optional.

```toml
[menu]
"d"   = { label = "dotfiles", run = "chezmoi edit", in = "pane" }
"g s" = false                  # hide a default item
"g"   = { label = "Git" }      # relabel a default group
"r h" = { label = "grow left", herdr = "pane resize --direction left", stick = true }
```

Defaults live in the binary and the user's file overlays them: only additions
and overrides are written; `= false` removes a default. Plugin updates improve
defaults without touching the user's file. Discoverability (chosen over a
seeded-full-copy model):

- First run seeds the config with a delimited, fully-commented dump of every
  resolved default — uncomment a line to override it.
- `herdr-whichkey defaults` prints the live resolved tree (post
  plugin-detection) anytime.

**Amended (hw-b422):** the overlay stands for *settings* and is gone for
*bindings*. One file could not have both properties it needed — settings want
defaults that keep updating, bindings want the file to be the order — so the
config is two files, each getting the one it needs:

| | whichkey.toml | keys file (`keys.toml`) |
|---|---|---|
| holds | `[layout]` `[theme]` `[ui]`, `keys_path` | the whole `[menu]` tree |
| sparse? | yes — undeclared falls back to built-in | no — it *is* the menu |
| we update it | yes, via the built-in defaults it falls back to | never, after `init` writes it |
| `init` writes it | commented stub | uncommented, verbatim |

The asymmetry in that last row is the whole design: a commented knob keeps
tracking a default we still own, an uncommented binding is frozen because it
is now the user's. File order in the keys file is display order.

Consequences: no overlay for bindings means no `= false` (nothing to hide
against) and no replace-in-place (nothing to replace) — both deleted rather
than kept working-but-inert, since a `= false` line in a single-source file
would parse cleanly and silently do nothing. Two paths that normalize to the
same sequence (`"g s"` and `"g  s"`) are a hard error for the same reason.
Discoverability moves from a first-run seed to `herdr-whichkey init` on
demand, plus `defaults --shipped` to print what init would write, so a
customised tree can be diffed against the shipped one.

Until `init` runs, the compiled-in tree renders: the plugin works the moment
herdr installs it, and taking the menu over is a deliberate act.

### Default tree: adaptive

Draft top level: `a` agents, `p` panes (+ sticky resize under `r`), `t` tabs,
`w` workspaces, `g` git (lazygit/hunk/worktrunk/gh-pr), `f` jump
(navigator/sessionizer/zoxide), `s` shell (floax/yazi/new pane), `?` help.
Items referencing a plugin action auto-hide when that plugin isn't installed
(`herdr plugin action list`, cached per open). Exact tree reviewed at
implementation.

### Interaction

- Esc closes; Backspace ascends one level; unknown key = quiet inline hint
  (no bell, no flash); no auto-close timeout.
- `stick = true` keeps the strip open after an action fires (hydra-style
  repeat) — used by resize and next/prev navigation defaults.

### Theming: match herdr's chrome

herdr exposes no theme API, so: parse `~/.config/herdr/config.toml`
(`theme.name`, `theme.custom.*`), ship truecolor palettes for herdr's built-in
themes (default `catppuccin`), fall back to ANSI-16 for unknown themes, and
honor `theme.auto_switch` best-effort via OSC 11 appearance detection
(defaulting dark). A `[theme]` section in the whichkey config is the final
override.

### Distribution: prebuilt + cargo fallback

Manifest `[[build]]` script fetches a sha256-pinned release binary for the
host triple ({x86_64,aarch64} × {linux-gnu, apple-darwin}, built by GitHub
Actions on tag), falling back to `cargo build --release`. Repo:
github.com/Qu4tro/herdr-whichkey, tagged `herdr-plugin` for the marketplace at
publish time.

## Rejected along the way

- **Centered popup / full-screen modal** as *the* surface — occlusion and
  disruption; the bottom strip keeps the workspace readable, and stays the
  default. Superseded in part: the popup is now an **option**
  (`placement = "popup"`), because it is the one surface that never
  reflows the focused pane — measured, not assumed. It pays for that with
  occlusion, with a breadcrumb it has to draw in its own body (a popup has
  no pane id, so nothing can retitle its border), and with press-again-to-
  close, which herdr cannot deliver to a focused popup. A trade, offered
  rather than made for the user. Full-screen modal stays rejected.
- **Seeded-full-copy config** — transparent but frozen; updates never reach
  the user. Overlay + commented seed + `defaults` subcommand covers
  discoverability without the fork. Scoped, not reversed (hw-b422): the
  objection only ever bit **bindings**, and bindings are now deliberately
  outside our update scope — nobody's defaults but the user's should decide
  which key does what, so a frozen copy of them is the point rather than the
  cost. It stands unchanged for **settings**, which stay sparse and keep
  falling back to defaults we go on improving.
- **Nested-tables / array-of-items config schemas** — more ceremony per item,
  reorganizing a branch touches every child header.
- **Send-keys leaf in v1** — focus tracking and timing make it the flakiest
  capability; deferred rather than shipped half-right.

## v2 backlog

Send-keys leaves · embedded-PTY runs in the strip · ~~defaults sync surface
(refreshing the commented seed after updates)~~ — dropped by hw-b422: it only
ever applied to bindings, and the keys file is the user's to sync or not
(`defaults --shipped` is the diff surface) · whatever live validation
surfaces. Tracked as tk tickets at publish time.
