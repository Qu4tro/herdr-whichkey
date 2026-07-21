# herdr-whichkey

A blezz/which-key-style action menu for [herdr](https://herdr.dev): one trigger
key, then a bottom hint strip and **one keystroke per action** — no typing, no
Enter. Nested groups, sticky (hydra-style) repeat for resize, and defaults that
adapt to the herdr plugins and tools you actually have installed.

> **Status: v0.2.0 released.**

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
layout back when it closes. Esc goes back up a level (and closes from the
root), Backspace also goes back, Ctrl+C always closes — and pressing the
trigger again closes the menu. Unknown keys just hint; nothing flashes.

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

Your menu lives in one TOML file (`herdr plugin config-dir herdr-whichkey`,
then `whichkey.toml` — seeded on first run with every default as a commented
line). It overlays the built-in defaults: you write only what you add or
change.

```toml
[menu]
"d"   = { label = "dotfiles", run = "chezmoi edit", in = "pane" }
"g g" = false                  # hide a default item (and any subtree)
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
- `stick = true` keeps the menu open for repeatable actions (resize).
- `requires = "binary"` hides the item unless the binary is on PATH.

## Layout

Items flow into a footer-style grid: the strip width decides the column
count, and CSS content-distribution keywords (computed by [taffy], the
CSS grid engine) decide the spacing. Tune it in whichkey.toml:

```toml
[layout]
justify = "space-evenly"  # columns: start | center | end |
align   = "space-around"  # rows:      space-between | space-around | space-evenly
height  = 7               # strip height in split rows (~2 are pane chrome)
columns = 4               # pin the column count (default: fit the width)
gutter  = 9               # cells between columns (default: half an item)
```

`justify` spreads the columns across the strip, `align` spreads the rows
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
