//! Theme resolution: read herdr's own config to find the active theme,
//! map it to a built-in palette (canonical upstream values), fall back to
//! plain ANSI for unknown themes, and apply `[theme]` overrides last.

use std::path::PathBuf;

use anyhow::{bail, Result};
use crossterm::style::Color;
use serde::Deserialize;

/// The six roles the strip paints with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Strip background. `Color::Reset` means "don't paint" (ANSI mode).
    pub bg: Color,
    /// Slightly raised background (breadcrumb bar, hint row).
    pub surface: Color,
    pub fg: Color,
    /// De-emphasized text (footer hints, separators).
    pub dim: Color,
    /// Item keys and group markers.
    pub accent: Color,
    /// Inline error text.
    pub warn: Color,
}

/// `[theme]` section of whichkey.toml — every field optional, applied on
/// top of whatever palette was resolved from herdr's config.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeOverrides {
    /// Force a palette by name, skipping herdr-config detection.
    pub name: Option<String>,
    pub bg: Option<String>,
    pub surface: Option<String>,
    pub fg: Option<String>,
    pub dim: Option<String>,
    pub accent: Option<String>,
    pub warn: Option<String>,
}

/// The `[theme]` table of herdr's own config.toml (fields we care about).
#[derive(Debug, Default, Deserialize)]
struct HerdrThemeSection {
    name: Option<String>,
    #[serde(default)]
    auto_switch: bool,
    dark_name: Option<String>,
    light_name: Option<String>,
    /// User-defined themes: [theme.custom.<name>] with color keys.
    #[serde(default)]
    custom: toml::map::Map<String, toml::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct HerdrConfig {
    #[serde(default)]
    theme: HerdrThemeSection,
}

pub fn resolve(overrides: &ThemeOverrides) -> Palette {
    let mut pal = match &overrides.name {
        Some(name) => builtin(name).unwrap_or(ANSI),
        None => from_herdr_config().unwrap_or(ANSI),
    };
    apply_overrides(&mut pal, overrides);
    pal
}

fn from_herdr_config() -> Option<Palette> {
    let text = std::fs::read_to_string(herdr_config_path()).ok()?;
    let cfg: HerdrConfig = toml::from_str(&text).ok()?;
    let theme = &cfg.theme;

    let name = if theme.auto_switch {
        // OSC 11 querying is deferred (docs/design.md) — COLORFGBG is the
        // best signal we get without touching the terminal. Default dark.
        let light = terminal_is_light();
        let picked = if light { &theme.light_name } else { &theme.dark_name };
        picked.clone().or_else(|| theme.name.clone())?
    } else {
        theme.name.clone()?
    };

    if let Some(custom) = theme.custom.get(&name) {
        if let Some(pal) = palette_from_custom(custom) {
            return Some(pal);
        }
    }
    builtin(&name)
}

/// herdr's own config.toml — the theme comes out of it, and so does the
/// key binding that opens us (`trigger.rs`). `HERDR_CONFIG_PATH` is
/// herdr's documented override (`herdr --help`); it names the file
/// itself, not a directory, and outranks the XDG location — measured, so
/// a session started against a custom config reads as that session.
pub fn herdr_config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("HERDR_CONFIG_PATH").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
    });
    base.join("herdr/config.toml")
}

/// COLORFGBG is "fg;bg" in ANSI indices; bg 7/15 → light background.
fn terminal_is_light() -> bool {
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| v.rsplit(';').next().and_then(|bg| bg.parse::<u8>().ok()))
        .map(|bg| bg == 7 || bg == 15)
        .unwrap_or(false)
}

/// Map a `[theme.custom.<name>]` table best-effort: accept a few aliases
/// per role, require at least bg+fg to consider it usable.
fn palette_from_custom(value: &toml::Value) -> Option<Palette> {
    let table = value.as_table()?;
    let get = |names: &[&str]| -> Option<Color> {
        names
            .iter()
            .find_map(|n| table.get(*n))
            .and_then(|v| v.as_str())
            .and_then(|s| parse_hex(s).ok())
    };
    let bg = get(&["bg", "background"])?;
    let fg = get(&["fg", "foreground", "text"])?;
    Some(Palette {
        bg,
        surface: get(&["surface", "selection", "highlight"]).unwrap_or(bg),
        fg,
        dim: get(&["dim", "muted", "comment"]).unwrap_or(fg),
        accent: get(&["accent", "primary", "blue"]).unwrap_or(fg),
        warn: get(&["warn", "error", "red"]).unwrap_or(fg),
    })
}

fn apply_overrides(pal: &mut Palette, o: &ThemeOverrides) {
    let slots: [(&Option<String>, &mut Color); 6] = [
        (&o.bg, &mut pal.bg),
        (&o.surface, &mut pal.surface),
        (&o.fg, &mut pal.fg),
        (&o.dim, &mut pal.dim),
        (&o.accent, &mut pal.accent),
        (&o.warn, &mut pal.warn),
    ];
    for (src, slot) in slots {
        if let Some(hex) = src {
            if let Ok(c) = parse_hex(hex) {
                *slot = c;
            }
        }
    }
}

pub fn parse_hex(s: &str) -> Result<Color> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("'{s}' is not a #rrggbb color");
    }
    let n = u32::from_str_radix(h, 16).unwrap();
    Ok(Color::Rgb { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8 })
}

const fn rgb(n: u32) -> Color {
    Color::Rgb { r: (n >> 16) as u8, g: (n >> 8) as u8, b: n as u8 }
}

/// ANSI-only palette: paint nothing we don't have to, use the terminal's
/// own colors. Also the fallback for themes we don't know.
pub const ANSI: Palette = Palette {
    bg: Color::Reset,
    surface: Color::Reset,
    fg: Color::Reset,
    dim: Color::DarkGrey,
    accent: Color::Cyan,
    warn: Color::Red,
};

/// herdr's built-in theme roster (extracted from the 0.7.4 binary), with
/// canonical palette values from each theme's upstream definition.
pub fn builtin(name: &str) -> Option<Palette> {
    let p = match name {
        "terminal" => ANSI,
        "catppuccin" => Palette {
            bg: rgb(0x1e1e2e),
            surface: rgb(0x313244),
            fg: rgb(0xcdd6f4),
            dim: rgb(0x6c7086),
            accent: rgb(0xcba6f7),
            warn: rgb(0xf38ba8),
        },
        "catppuccin-latte" => Palette {
            bg: rgb(0xeff1f5),
            surface: rgb(0xccd0da),
            fg: rgb(0x4c4f69),
            dim: rgb(0x8c8fa1),
            accent: rgb(0x8839ef),
            warn: rgb(0xd20f39),
        },
        "tokyo-night" => Palette {
            bg: rgb(0x1a1b26),
            surface: rgb(0x292e42),
            fg: rgb(0xc0caf5),
            dim: rgb(0x565f89),
            accent: rgb(0x7aa2f7),
            warn: rgb(0xf7768e),
        },
        "tokyo-night-day" => Palette {
            bg: rgb(0xe1e2e7),
            surface: rgb(0xc4c8da),
            fg: rgb(0x3760bf),
            dim: rgb(0x848cb5),
            accent: rgb(0x2e7de9),
            warn: rgb(0xf52a65),
        },
        "dracula" => Palette {
            bg: rgb(0x282a36),
            surface: rgb(0x44475a),
            fg: rgb(0xf8f8f2),
            dim: rgb(0x6272a4),
            accent: rgb(0xbd93f9),
            warn: rgb(0xff5555),
        },
        "nord" => Palette {
            bg: rgb(0x2e3440),
            surface: rgb(0x3b4252),
            fg: rgb(0xd8dee9),
            dim: rgb(0x616e88),
            accent: rgb(0x88c0d0),
            warn: rgb(0xbf616a),
        },
        "gruvbox" => Palette {
            bg: rgb(0x282828),
            surface: rgb(0x3c3836),
            fg: rgb(0xebdbb2),
            dim: rgb(0x928374),
            accent: rgb(0xfe8019),
            warn: rgb(0xfb4934),
        },
        "gruvbox-light" => Palette {
            bg: rgb(0xfbf1c7),
            surface: rgb(0xebdbb2),
            fg: rgb(0x3c3836),
            dim: rgb(0x928374),
            accent: rgb(0xaf3a03),
            warn: rgb(0xcc241d),
        },
        "one-dark" => Palette {
            bg: rgb(0x282c34),
            surface: rgb(0x3e4451),
            fg: rgb(0xabb2bf),
            dim: rgb(0x5c6370),
            accent: rgb(0x61afef),
            warn: rgb(0xe06c75),
        },
        "one-light" => Palette {
            bg: rgb(0xfafafa),
            surface: rgb(0xe5e5e6),
            fg: rgb(0x383a42),
            dim: rgb(0xa0a1a7),
            accent: rgb(0x4078f2),
            warn: rgb(0xe45649),
        },
        "solarized" => Palette {
            bg: rgb(0x002b36),
            surface: rgb(0x073642),
            fg: rgb(0x839496),
            dim: rgb(0x586e75),
            accent: rgb(0x268bd2),
            warn: rgb(0xdc322f),
        },
        "solarized-light" => Palette {
            bg: rgb(0xfdf6e3),
            surface: rgb(0xeee8d5),
            fg: rgb(0x657b83),
            dim: rgb(0x93a1a1),
            accent: rgb(0x268bd2),
            warn: rgb(0xdc322f),
        },
        "kanagawa" => Palette {
            bg: rgb(0x1f1f28),
            surface: rgb(0x2a2a37),
            fg: rgb(0xdcd7ba),
            dim: rgb(0x727169),
            accent: rgb(0x7e9cd8),
            warn: rgb(0xff5d62),
        },
        "kanagawa-lotus" => Palette {
            bg: rgb(0xf2ecbc),
            surface: rgb(0xdcd5ac),
            fg: rgb(0x545464),
            dim: rgb(0x8a8980),
            accent: rgb(0x4d699b),
            warn: rgb(0xc84053),
        },
        "rose-pine" => Palette {
            bg: rgb(0x191724),
            surface: rgb(0x26233a),
            fg: rgb(0xe0def4),
            dim: rgb(0x6e6a86),
            accent: rgb(0xc4a7e7),
            warn: rgb(0xeb6f92),
        },
        "rose-pine-dawn" => Palette {
            bg: rgb(0xfaf4ed),
            surface: rgb(0xf2e9e1),
            fg: rgb(0x575279),
            dim: rgb(0x9893a5),
            accent: rgb(0x907aa9),
            warn: rgb(0xb4637a),
        },
        "vesper" => Palette {
            bg: rgb(0x101010),
            surface: rgb(0x232323),
            fg: rgb(0xffffff),
            dim: rgb(0x7e7e7e),
            accent: rgb(0xffc799),
            warn: rgb(0xff8080),
        },
        _ => return None,
    };
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#1e1e2e").unwrap(), rgb(0x1e1e2e));
        assert_eq!(parse_hex("cdd6f4").unwrap(), rgb(0xcdd6f4));
        assert!(parse_hex("#12345").is_err());
        assert!(parse_hex("not-a-color").is_err());
    }

    #[test]
    fn roster_complete() {
        // Every theme name shipped in herdr 0.7.4 resolves.
        for name in [
            "catppuccin",
            "catppuccin-latte",
            "terminal",
            "tokyo-night",
            "tokyo-night-day",
            "dracula",
            "nord",
            "gruvbox",
            "gruvbox-light",
            "one-dark",
            "one-light",
            "solarized",
            "solarized-light",
            "kanagawa",
            "kanagawa-lotus",
            "rose-pine",
            "rose-pine-dawn",
            "vesper",
        ] {
            assert!(builtin(name).is_some(), "missing palette for {name}");
        }
        assert!(builtin("no-such-theme").is_none());
    }

    #[test]
    fn overrides_apply() {
        let o = ThemeOverrides {
            name: Some("dracula".into()),
            accent: Some("#ffffff".into()),
            ..Default::default()
        };
        let pal = resolve(&o);
        assert_eq!(pal.bg, rgb(0x282a36));
        assert_eq!(pal.accent, rgb(0xffffff));
    }
}
