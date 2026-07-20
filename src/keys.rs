//! Menu key parsing — herdr's key-string vocabulary, reduced to the subset
//! that makes sense for single-keystroke menu steps (printable keys).

use anyhow::{bail, Result};

/// Named punctuation herdr's config accepts, mapped to the character.
const NAMED: &[(&str, char)] = &[
    ("space", ' '),
    ("minus", '-'),
    ("plus", '+'),
    ("comma", ','),
    ("period", '.'),
    ("slash", '/'),
    ("backslash", '\\'),
    ("semicolon", ';'),
    ("colon", ':'),
    ("ampersand", '&'),
    ("backtick", '`'),
    ("equals", '='),
    ("question", '?'),
];

/// Keys the menu itself owns; a config using them for items is an error.
const RESERVED: &[&str] = &["esc", "escape", "backspace", "bksp", "enter", "tab"];

/// Parse one key token from a config path ("g", "shift+g", "space", "?").
pub fn parse_key(token: &str) -> Result<char> {
    let t = token.trim();
    if t.is_empty() {
        bail!("empty key token");
    }
    let lower = t.to_ascii_lowercase();
    if RESERVED.contains(&lower.as_str()) {
        bail!("'{t}' is reserved for menu navigation (esc closes, backspace goes up)");
    }
    if let Some(rest) = lower.strip_prefix("shift+") {
        let c = parse_key(rest)?;
        return Ok(c.to_ascii_uppercase());
    }
    if let Some(&(_, c)) = NAMED.iter().find(|(n, _)| *n == lower) {
        return Ok(c);
    }
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if !c.is_control() => Ok(c),
        _ => bail!(
            "'{t}' is not a single key — use one character, shift+<char>, or a name like 'space'/'comma'"
        ),
    }
}

/// Parse a full path key like "g s" into its key sequence.
pub fn parse_path(path: &str) -> Result<Vec<char>> {
    path.split_whitespace().map(parse_key).collect()
}

/// Render a key for display in the strip (space shown by name).
pub fn display_key(c: char) -> String {
    if c == ' ' {
        "spc".into()
    } else {
        c.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_named_shift() {
        assert_eq!(parse_key("g").unwrap(), 'g');
        assert_eq!(parse_key("G").unwrap(), 'G');
        assert_eq!(parse_key("shift+g").unwrap(), 'G');
        assert_eq!(parse_key("space").unwrap(), ' ');
        assert_eq!(parse_key("question").unwrap(), '?');
        assert_eq!(parse_key("?").unwrap(), '?');
    }

    #[test]
    fn reserved_and_junk_rejected() {
        assert!(parse_key("esc").is_err());
        assert!(parse_key("backspace").is_err());
        assert!(parse_key("gg").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn paths() {
        assert_eq!(parse_path("g s").unwrap(), vec!['g', 's']);
        assert_eq!(parse_path("r shift+h").unwrap(), vec!['r', 'H']);
    }
}
