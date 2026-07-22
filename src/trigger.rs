//! Press-again-to-close, for the surface herdr can't deliver it on.
//!
//! On a split the trigger never reaches us: herdr handles its own prefix
//! bindings, fires the action again, and the launcher's lock turns that
//! into a close. A focused popup gets no such service — herdr routes
//! every key into the pane instead, the binding never fires, and the
//! keystrokes arrive here as ordinary input (measured: `\x02` then `' '`
//! for `prefix+space`, docs/spike-popup-panes.md). The keys land in this
//! process, so this is where the toggle has to live.
//!
//! What the trigger *is* is the user's business — it lives in herdr's own
//! `config.toml`, since herdr 0.7 cannot bind keys from plugin manifests.
//! We read the binding that invokes us, and its prefix, from there.
//!
//! The menu's own navigation outranks whatever we read: herdr accepts
//! `esc` as a prefix, and Esc closing the menu matters more than the
//! trigger doing it (the caller resets us on those keys).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

/// The action a binding names to open this menu — the one the README
/// tells you to bind, and the only one that gets us on screen.
const OPEN_ACTION: &str = "herdr-whichkey.open";

/// herdr's own default, from its commented config template.
const DEFAULT_PREFIX: &str = "ctrl+b";

/// The `[keys]` table of herdr's config.toml — the two parts of it that
/// say which keystrokes open us. Everything else there is somebody
/// else's binding.
#[derive(Debug, Default, Deserialize)]
struct Keys {
    prefix: Option<String>,
    #[serde(default)]
    command: Vec<CommandKey>,
}

#[derive(Debug, Deserialize)]
struct CommandKey {
    key: Option<String>,
    command: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct HerdrConfig {
    #[serde(default)]
    keys: Keys,
}

/// One keystroke: a key plus the modifiers that must be held with it.
/// Shift is not one of them — it rides on the character (`shift+g`
/// arrives as `G`, same as [`crate::keys::parse_key`] reads it), so
/// [`normalize`] folds it into `code` and the case of the character is
/// what tells `shift+g` from `g`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Chord {
    code: KeyCode,
    ctrl: bool,
    alt: bool,
}

impl Chord {
    fn matches(&self, ev: &KeyEvent) -> bool {
        self.code == normalize(ev.code, self.ctrl)
            && self.ctrl == ev.modifiers.contains(KeyModifiers::CONTROL)
            && self.alt == ev.modifiers.contains(KeyModifiers::ALT)
    }
}

/// The comparable form of a key. Case carries shift, so it is compared —
/// except under ctrl, which loses it on the way here: ctrl+b and
/// ctrl+shift+b are the same byte, and a config that spells one of them
/// must not stop matching the other.
fn normalize(code: KeyCode, ctrl: bool) -> KeyCode {
    match code {
        KeyCode::Char(c) if ctrl => KeyCode::Char(c.to_ascii_lowercase()),
        code => code,
    }
}

/// What a key did to the trigger sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// The whole sequence just landed: close.
    Complete,
    /// It started (or continued) one — the key belongs to the trigger and
    /// not to the menu, so nothing else may act on it.
    Pending,
    /// Nothing to do with us; handle it as a menu key.
    No,
}

/// The keystroke sequence that opens this menu, and how far through it
/// the user currently is.
#[derive(Debug)]
pub struct Trigger {
    chords: Vec<Chord>,
    label: String,
    at: usize,
}

impl Trigger {
    /// The binding as herdr's config has it, or None when there is no
    /// readable one — an unparseable config, a menu opened some other
    /// way, a `key` we don't understand. None just means the menu keeps
    /// closing on Esc and ctrl+c, so a miss here costs nothing.
    pub fn from_herdr_config() -> Option<Self> {
        Self::from_toml(&std::fs::read_to_string(crate::theme::herdr_config_path()).ok()?)
    }

    fn from_toml(text: &str) -> Option<Self> {
        let cfg: HerdrConfig = toml::from_str(text).ok()?;
        let bound = cfg.keys.command.iter().find(|c| c.command.as_deref() == Some(OPEN_ACTION))?;
        Self::parse(bound.key.as_deref()?, cfg.keys.prefix.as_deref().unwrap_or(DEFAULT_PREFIX))
    }

    /// `("prefix+space", "ctrl+b")` → ctrl+b, then space. A binding
    /// without the `prefix+` marker is a single direct chord.
    fn parse(key: &str, prefix: &str) -> Option<Self> {
        let key = key.trim();
        let mut chords = Vec::new();
        // ASCII-lowercasing keeps every byte offset, so the marker can be
        // matched case-insensitively and cut from the original.
        let last = if key.to_ascii_lowercase().starts_with("prefix+") {
            // Two presses, not one chord: herdr's prefix mode, then the
            // key it arms.
            chords.push(parse_chord(prefix)?);
            &key["prefix+".len()..]
        } else {
            key
        };
        chords.push(parse_chord(last)?);
        Some(Self { chords, label: key.to_string(), at: 0 })
    }

    /// The binding as written, for telling the user which key they are
    /// halfway through.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Forget a half-typed sequence: the menu's own navigation keys take
    /// priority over the trigger and end any run through it.
    pub fn reset(&mut self) {
        self.at = 0;
    }

    pub fn feed(&mut self, ev: &KeyEvent) -> Step {
        if !self.chords[self.at].matches(ev) {
            // A miss ends the run — but the key that ended it can start
            // the next one (ctrl+b ctrl+b leaves one press pending), so
            // fall back to the start and try it there once.
            self.at = 0;
            if !self.chords[0].matches(ev) {
                return Step::No;
            }
        }
        self.at += 1;
        if self.at < self.chords.len() {
            Step::Pending
        } else {
            self.at = 0;
            Step::Complete
        }
    }
}

/// One herdr key string ("ctrl+b", "space", "f12", "shift+g").
fn parse_chord(token: &str) -> Option<Chord> {
    let (mut ctrl, mut alt, mut shift, mut code) = (false, false, false, None);
    for part in token.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" | "opt" | "option" | "meta" => alt = true,
            "shift" => shift = true,
            // Nothing a keystroke read inside a pane can observe.
            "cmd" | "super" | "win" => {}
            _ => code = Some(key_code(part)?),
        }
    }
    // Shift is spent here rather than kept as a modifier: it is the
    // shifted character that reaches us, so `shift+g` is `G` — the same
    // reading the menu's own key parser gives it.
    let code = match code? {
        KeyCode::Char(c) if shift => KeyCode::Char(c.to_ascii_uppercase()),
        code => code,
    };
    Some(Chord { code: normalize(code, ctrl), ctrl, alt })
}

/// herdr's key vocabulary, as far as a keystroke inside a pane can tell
/// it apart: the special keys by name, function keys, the punctuation
/// names the menu's own parser already knows, and single characters.
/// Names are matched case-insensitively; a bare character is not, since
/// its case is what shift comes through as.
fn key_code(name: &str) -> Option<KeyCode> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "backspace" | "bksp" => KeyCode::Backspace,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                return Some(KeyCode::F(n));
            }
            match crate::keys::named_char(&lower) {
                Some(c) => KeyCode::Char(c),
                None => {
                    let mut chars = name.chars();
                    match (chars.next(), chars.next()) {
                        (Some(c), None) => KeyCode::Char(c),
                        _ => return None,
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventKind;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        press(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn plain(c: char) -> KeyEvent {
        press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// The README's own binding, keystroke for keystroke as it arrives in
    /// a focused popup: ctrl+b, then space.
    #[test]
    fn the_readme_binding_closes_the_menu_on_its_second_press() {
        let mut t = Trigger::parse("prefix+space", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain(' ')), Step::Complete);
        // And again, from the top: finishing the sequence rearms it.
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain(' ')), Step::Complete);
    }

    #[test]
    fn menu_keys_are_left_alone() {
        let mut t = Trigger::parse("prefix+space", "ctrl+b").unwrap();
        // Plain 'b' is a menu item, not the prefix — modifiers are part
        // of the match, so the item still fires.
        assert_eq!(t.feed(&plain('b')), Step::No);
        assert_eq!(t.feed(&plain('g')), Step::No);
        // A space on its own is a menu key too: only the full sequence
        // closes, so an item bound to `space` keeps working.
        assert_eq!(t.feed(&plain(' ')), Step::No);
    }

    #[test]
    fn a_broken_sequence_can_start_the_next_one() {
        let mut t = Trigger::parse("prefix+space", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        // Not the second key — but it is the first, so we are one press
        // in again rather than back to nothing.
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain(' ')), Step::Complete);
        // A genuine miss drops the sequence, and the key falls through to
        // the menu instead of being swallowed.
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain('g')), Step::No);
        assert_eq!(t.feed(&plain(' ')), Step::No);
    }

    /// The menu resets us when it handles Esc/Backspace/ctrl+c itself, so
    /// a sequence half-typed before an ascend can't finish afterwards and
    /// close a menu the user was navigating.
    #[test]
    fn a_reset_sequence_starts_over() {
        let mut t = Trigger::parse("prefix+space", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        t.reset();
        assert_eq!(t.feed(&plain(' ')), Step::No);
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain(' ')), Step::Complete);
    }

    #[test]
    fn direct_chords_and_function_keys_take_one_press() {
        let mut t = Trigger::parse("ctrl+alt+n", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('n')), Step::No); // alt missing
        assert_eq!(
            t.feed(&press(KeyCode::Char('n'), KeyModifiers::CONTROL | KeyModifiers::ALT)),
            Step::Complete
        );

        let mut t = Trigger::parse("f12", "ctrl+b").unwrap();
        assert_eq!(t.feed(&press(KeyCode::F(12), KeyModifiers::NONE)), Step::Complete);
        assert_eq!(t.feed(&plain('f')), Step::No);
    }

    /// A prefix the user changed, and the punctuation names herdr accepts.
    #[test]
    fn the_configured_prefix_and_named_keys_are_honoured() {
        let mut t = Trigger::parse("prefix+comma", "f12").unwrap();
        assert_eq!(t.feed(&press(KeyCode::F(12), KeyModifiers::NONE)), Step::Pending);
        assert_eq!(t.feed(&plain(',')), Step::Complete);

        // shift rides on the character: `shift+g` is `G`, and plain `g`
        // is a different key — a menu item bound to it must still fire
        // rather than closing the menu.
        let mut t = Trigger::parse("prefix+shift+g", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain('g')), Step::No);
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&press(KeyCode::Char('G'), KeyModifiers::SHIFT)), Step::Complete);

        // And the other way around: an unshifted binding is not closed by
        // the shifted key. `shift+g` and `G` are the same binding.
        let mut t = Trigger::parse("prefix+g", "ctrl+b").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&press(KeyCode::Char('G'), KeyModifiers::SHIFT)), Step::No);
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain('g')), Step::Complete);
        assert_eq!(
            Trigger::parse("shift+g", "ctrl+b").unwrap().chords,
            [Chord { code: KeyCode::Char('G'), ctrl: false, alt: false }]
        );

        // A ctrl chord carries no case at all — the byte is the same
        // either way, so a prefix written `ctrl+B` still matches it.
        let mut t = Trigger::parse("prefix+space", "ctrl+B").unwrap();
        assert_eq!(t.feed(&ctrl('b')), Step::Pending);
        assert_eq!(t.feed(&plain(' ')), Step::Complete);
    }

    #[test]
    fn reads_the_binding_out_of_herdrs_config() {
        // The README's snippet, in a config with the usual company.
        let cfg = r#"
onboarding = false
[theme]
name = "gruvbox"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "herdr-zoxide.browse"

[[keys.command]]
key = "prefix+space"
type = "plugin_action"
command = "herdr-whichkey.open"
description = "whichkey menu"
"#;
        let t = Trigger::from_toml(cfg).unwrap();
        assert_eq!(t.label(), "prefix+space");
        assert_eq!(t.chords.len(), 2);
        assert_eq!(t.chords[0], Chord { code: KeyCode::Char('b'), ctrl: true, alt: false });

        // A custom prefix applies to our binding too.
        let with_prefix = format!("[keys]\nprefix = \"f12\"\n{cfg}");
        let t = Trigger::from_toml(&with_prefix).unwrap();
        assert_eq!(t.chords[0], Chord { code: KeyCode::F(12), ctrl: false, alt: false });
    }

    #[test]
    fn no_binding_of_ours_means_no_trigger() {
        // Somebody else's bindings, an unparseable file, and a binding
        // with no key: all leave Esc and ctrl+c as the only ways out.
        assert!(Trigger::from_toml(
            "[[keys.command]]\nkey = \"prefix+o\"\ncommand = \"other.thing\"\n"
        )
        .is_none());
        assert!(Trigger::from_toml("this is not toml {{{").is_none());
        assert!(
            Trigger::from_toml("[[keys.command]]\ncommand = \"herdr-whichkey.open\"\n").is_none()
        );
        // A key string we can't make sense of is not a trigger either.
        assert!(Trigger::parse("prefix+notakey", "ctrl+b").is_none());
        assert!(Trigger::parse("", "ctrl+b").is_none());
    }
}
