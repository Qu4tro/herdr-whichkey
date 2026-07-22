# herdr-whichkey

A blezz/which-key-style action menu for [herdr](https://herdr.dev): one trigger
key, then a bottom hint strip and **one keystroke per action** — no typing, no
Enter. Nested groups, sticky (hydra-style) repeat for resize, and defaults that
adapt to the herdr plugins and tools you actually have installed.

> **Status: v0.2.2 released.**

## Install

```bash
herdr plugin install Qu4tro/herdr-whichkey
```

Install fetches a checksum-pinned prebuilt binary for Linux/macOS
(x86_64/aarch64) and falls back to `cargo build --release` where none fits.

herdr 0.7 does not bind keys from plugin manifests, so add a binding to
`~/.config/herdr/config.toml` and reload:

```toml
[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "herdr-whichkey.open"
description = "whichkey menu"
```

```bash
herdr server reload-config
```

`prefix` (Ctrl+B by default), then `space`, then single keys walk the menu.
The strip docks as a 7-row split under the pane you're in and puts the
layout back when it closes — or opens beside it, or floats over it, as
[`[layout] placement`](#layout) says. Esc goes back up a level (and closes
from the root), Backspace also goes back, Ctrl+C always closes, and
pressing the trigger again closes the menu. Unknown keys just hint;
nothing flashes. Items are [clickable](#mouse) too.

## Defaults

`a` agents (claude/codex into splits or workspaces) · `p` panes · `r` resize
(sticky) · `t` tabs · `w` workspaces · `g` git (lazygit, worktrunk) · `f` jump
(zoxide, command palette, navigator) · `s` shell & files (floax, yazi,
structupath, pluck).

Items requiring a binary (`lazygit`, `yazi`, …) or another herdr plugin hide
themselves automatically when the dependency is missing. See what you'd get
right now:

```bash
herdr-whichkey defaults      # resolved tree, with (hidden: …) annotations
```

## Configuration

Two files, in the plugin config dir (`herdr plugin config-dir herdr-whichkey`):

| file | holds | who owns it |
|---|---|---|
| `keys.toml` | your whole menu | **you** — written once by `init`, never touched again |
| `whichkey.toml` | `[layout]` `[theme]` `[ui]` settings | shared: anything you don't set tracks the built-in default |

Out of the box neither exists and the shipped menu renders, so the plugin
works the moment herdr installs it. Take the menu over when you want it:

```bash
herdr-whichkey init          # writes keys.toml (the shipped menu) + a whichkey.toml stub
```

`init` never overwrites a file that's already there. After it runs, `keys.toml`
**is** the menu — nothing is overlaid onto it, and **file order is display
order**, so moving a line moves the item. Settings work the other way round:
`whichkey.toml` is sparse, every knob in the stub starts commented out, and a
knob you leave commented keeps tracking its built-in default as the plugin
updates. That asymmetry is deliberate — your bindings are yours to freeze,
the rest keeps improving.

```bash
herdr-whichkey defaults --shipped    # the menu init writes — diff yours against it
```

### The menu (`keys.toml`)

```toml
[menu]
"d"   = { label = "dotfiles", run = "chezmoi edit", in = "pane" }
"t n" = { label = "tab here", herdr = "tab create --cwd {cwd}" }
```

- Keys use herdr's own key-string format; `"g s"` means press `g` then `s`;
  `shift+g`, `space`, `comma`, … work as in herdr's config.
- Leaves (exactly one per item):
  - `herdr = "…"` — a herdr CLI call, no shell involved.
  - `action = "plugin.action"` — invoke any installed plugin's action.
  - `run = "…"` — shell command, with `in = "background" | "pane" | "tab"`
    (background failures surface as a herdr notification) and optional
    `cwd = "…"`.
- Placeholders `{pane}` `{tab}` `{workspace}` `{cwd}` `{stamp}` expand to where
  you were when the menu opened; `~` and `$VARS` also expand where a shell
  would expand them.
- `stick = true` keeps the menu open for repeatable actions (resize). Set it on
  each action item — it isn't inherited, and a group that sets it is rejected.
- `requires = "binary"` hides the item unless the binary is on PATH.
- To drop an item, delete its line. There's no `= false` — nothing to hide
  against once the file is the whole menu.
- Two lines naming the same key are rejected as a typo rather than one quietly
  winning — including different spellings of it: `"g  s"` is `"g s"`, and
  `"shift+g"`, `"?"` are `"G"`, `"question"`.
- A group's position is its own line's. Writing `"g x"` before `"g"` springs
  the `g` group into existence early, but the `"g" = { label = … }` line is
  what anchors it, so moving that line moves the group and everything under it.
  A group you never declare sits where it first appears.

Keeping the menu elsewhere — in your dotfiles, say — is one line in
`whichkey.toml`, relative to that file unless absolute, with `~` and `$VARS`
expanded:

```toml
keys_path = "~/dotfiles/whichkey-keys.toml"
```

> **Upgrading from v0.2.x?** The old `whichkey.toml` had a `[menu]` table that
> overlaid the defaults. It is no longer read, and the plugin says so on
> startup rather than ignoring it. Two ways out:
>
> - Run `herdr-whichkey init`. It writes the shipped menu to `keys.toml` (or to
>   your `keys_path`, which it carries over into the new file), then renames
>   your old config to `whichkey.toml.pre-split` — nothing is deleted — and
>   writes a fresh settings stub. Port the `[menu]` entries you want out of the
>   `.pre-split` file into the keys file by hand. If the rest of the old config
>   doesn't parse, `init` refuses rather than migrating settings it couldn't
>   read: fix what it reports, then run it again.
> - Or migrate by hand: move the `[menu]` table into `keys.toml` yourself,
>   dropping any `= false` lines and adding back the defaults you want to keep
>   (`herdr-whichkey defaults --shipped` prints them).
>
> Either way your bindings need one pass by hand: they used to be a diff
> against the defaults, and `keys.toml` is the whole menu.

## Mouse

The strip is clickable as well as typeable — herdr forwards mouse input into
plugin panes, whatever its own `ui.mouse_capture` is set to
(see [docs/spike-mouse.md](docs/spike-mouse.md)).

- **Click an item** to fire it, exactly as its key would: `stick = true` items
  keep the menu open, everything else closes it.
- **Click a group** to descend into it. **Click empty strip space** to go back
  up one level — Backspace, not close, so a stray click is never destructive.
- The item under the pointer highlights, so you can see what you'd hit.
- The scroll wheel does nothing; the menu always fits on screen.

Two herdr quirks worth knowing, neither of them ours to fix:

- Under herdr's default `ui.mouse_capture = true`, **right-click over the strip
  opens herdr's own pane menu** — it never reaches the plugin. Right-click
  ascends a level only if you run with `mouse_capture = false` or set a
  `right_click_passthrough_modifier`.
- While the menu is open it owns the mouse in its own rows, so herdr's
  drag-to-select doesn't work over them (shift-drag isn't an escape hatch
  either — herdr forwards that too). Turn the whole thing off if you'd rather
  keep selection:

```toml
[ui]
mouse = false     # keyboard-only; the strip stops requesting mouse input
```

## Layout

### Where the menu opens

```toml
[layout]
placement = "bottom"   # bottom (default) | right | popup
```

| `placement` | what you get | reflows your pane | sized by |
|---|---|---|---|
| `bottom` | full-width strip split below the focused pane | yes | `height` |
| `right` | tall narrow list split beside it | yes | `width` |
| `popup` | centered float over the workspace | **no** | `width` × `height` |

`width` and `height` are plain cell counts, and each placement uses the
one along its own axis — that is the whole trade for keeping a single
pair of knobs instead of three sets:

```toml
[layout]
height = 7     # bottom: strip rows to size to (~2 go to pane chrome)
width  = 32    # right:  columns to size to (~2 go to pane chrome)
               # popup:  both, as the float's outer size (~2 each to its border)
```

The knob the placement doesn't use is ignored, not an error — switching
placement doesn't mean rewriting the section.

Two limits are clamped rather than reported, because there is nothing
useful to do with a menu smaller than its own chrome:

| | minimum | ceiling |
|---|---|---|
| `bottom` | `height` 3 | 90% of the pane it splits |
| `right` | `width` 8 | 90% of the pane it splits |
| `popup` | `width` 20, `height` 6 | the screen |

The ceiling on the splits is herdr's: it clamps a split ratio to
[0.1, 0.9], so on a short tab a large `height` lands at 90% instead of
where you asked. Sizes below or above the placement's own default work in
both directions — a split opens at half the pane and is resized to fit
either way.

`top` and `left` aren't offered: herdr only splits `down` and `right`, and
faking the others means splitting and then moving, which reflows your pane
twice — the one thing the strip exists to avoid.

**Popup notes** (see [docs/spike-popup-panes.md](docs/spike-popup-panes.md)):

- Pressing the trigger again closes a popup, but by a different route
  than it closes a split: herdr routes every key into a focused popup, so
  the binding never fires — the menu recognizes the keystrokes itself,
  reading the binding out of your herdr `config.toml`. Rebind it there
  and the close follows. Esc and Ctrl+C close it either way.
- A popup's border title is fixed, so the breadcrumb is drawn on the
  popup's first body row instead — clicking it goes back up a level.

### How items spread out

Items flow into a grid: the area's width decides the column count, and
CSS content-distribution keywords (computed by [taffy], the CSS grid
engine) decide the spacing. Each placement defaults to what reads well on
it — a spread footer grid at the bottom, a packed list on the right, a
centered block in a popup — and the knobs override:

```toml
[layout]
justify = "space-evenly"  # columns: start | center | end |
align   = "space-around"  # rows:      space-between | space-around | space-evenly
columns = 4               # pin the column count (default: fit the width)
gutter  = 9               # cells between columns (default: half an item)
```

`justify` spreads the columns across the area, `align` spreads the rows
down it. `gutter` sets the spacing floor when fitting columns and the
actual gap under `start`/`center`/`end` (the `space-*` modes make their
own gaps).

[taffy]: https://github.com/DioxusLabs/taffy

## Theming

The strip reads herdr's own `config.toml` and matches its theme (all 18
built-in themes, including `terminal`/ANSI, and `[theme.custom.*]` entries;
`auto_switch` respected best-effort). Force or tweak per-role colors in your
whichkey.toml:

```toml
[theme]
name   = "gruvbox"     # optional: skip detection
accent = "#fe8019"     # bg, surface, fg, dim, accent, warn
```

## License

MIT
