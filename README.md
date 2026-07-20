# herdr-whichkey

A blezz/which-key-style action menu for [herdr](https://herdr.dev): one trigger
key, then a bottom hint strip and **one keystroke per action** — no typing, no
Enter. Nested groups, sticky (hydra-style) repeat for resize/navigation, and
defaults that adapt to the herdr plugins you actually have installed.

> **Status: pre-release scaffold.** The plugin skeleton, manifest, and launcher
> lifecycle are in place; menu content, config overlay, and theming are landing
> next. Not yet on the marketplace.

## Install

```bash
herdr plugin install Qu4tro/herdr-whichkey
```

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

`prefix` (Ctrl+B by default), then `space`, then single keys walk the menu:
`g` git, `p` panes, `a` agents… Esc closes, Backspace goes up one level.

## Configuration

Your menu lives in one TOML file (`herdr plugin config-dir herdr-whichkey`),
overlaying the built-in defaults — you write only what you add or change:

```toml
[menu]
"d"   = { label = "dotfiles", run = "chezmoi edit", in = "pane" }
"g s" = false                  # hide a default item
"r h" = { label = "grow left", herdr = "pane resize --direction left", stick = true }
```

- Keys use herdr's own key-string format; `"g s"` means press `g` then `s`.
- Leaves: `herdr = "…"` (herdr CLI), `action = "plugin.action"` (any installed
  plugin), `run = "…"` (shell, with `in = "background" | "pane" | "tab"`).
- `{pane}`, `{tab}`, `{workspace}`, `{cwd}` expand to where you were when the
  menu opened; `~` and `$VARS` expand in commands and paths.
- `stick = true` keeps the menu open for repeatable actions.
- `herdr-whichkey defaults` prints the fully-resolved tree.

## License

MIT
