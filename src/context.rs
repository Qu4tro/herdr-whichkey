//! Invocation context and placeholder expansion.

use anyhow::{Context as _, Result};
use serde::Deserialize;

/// Context herdr injects into every plugin process. The only trustworthy
/// source for "where was the user" — the plain HERDR_TAB_ID /
/// HERDR_WORKSPACE_ID env vars can leak from the invoking CLI's own
/// environment (see docs/spike-popup-panes.md).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HerdrContext {
    #[serde(default)]
    pub workspace_id: String,
    #[serde(default)]
    pub workspace_cwd: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub focused_pane_id: String,
    #[serde(default)]
    pub focused_pane_cwd: String,
    /// "keybinding" | "cli" | … — part of the context contract; kept for
    /// debugging even though nothing branches on it yet.
    #[serde(default)]
    #[allow(dead_code)]
    pub invocation_source: String,
}

impl HerdrContext {
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON").context(
            "HERDR_PLUGIN_CONTEXT_JSON not set — run via the herdr plugin, not directly",
        )?;
        serde_json::from_str(&raw).context("could not parse HERDR_PLUGIN_CONTEXT_JSON")
    }

    pub fn cwd(&self) -> &str {
        if self.focused_pane_cwd.is_empty() {
            &self.workspace_cwd
        } else {
            &self.focused_pane_cwd
        }
    }

    /// Expand {pane} {tab} {workspace} {cwd} {stamp} only. Shell commands
    /// go through this — the shell handles $VARS and ~ itself.
    pub fn expand_placeholders(&self, input: &str, stamp: u64) -> String {
        input
            .replace("{pane}", &self.focused_pane_id)
            .replace("{tab}", &self.tab_id)
            .replace("{workspace}", &self.workspace_id)
            .replace("{cwd}", self.cwd())
            .replace("{stamp}", &stamp.to_string())
    }

    /// Placeholders, then $VARS and a leading ~. Used for herdr argv tokens
    /// and cwd paths, which no shell ever sees.
    pub fn expand(&self, input: &str, stamp: u64) -> String {
        expand_env_tilde(&self.expand_placeholders(input, stamp))
    }
}

pub fn now_stamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn expand_env_tilde(input: &str) -> String {
    let mut s = String::with_capacity(input.len());
    if let Some(rest) = input.strip_prefix("~/") {
        s.push_str(&std::env::var("HOME").unwrap_or_else(|_| "~".into()));
        s.push('/');
        s.push_str(&expand_vars(rest));
        return s;
    }
    if input == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| "~".into());
    }
    expand_vars(input)
}

fn expand_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let braced = matches!(chars.peek(), Some((_, '{')));
        if braced {
            chars.next();
        }
        let mut name = String::new();
        while let Some(&(_, nc)) = chars.peek() {
            let ok = if braced { nc != '}' } else { nc.is_ascii_alphanumeric() || nc == '_' };
            if !ok {
                break;
            }
            name.push(nc);
            chars.next();
        }
        if braced {
            chars.next(); // consume '}'
        }
        if name.is_empty() {
            out.push('$');
        } else {
            out.push_str(&std::env::var(&name).unwrap_or_default());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> HerdrContext {
        HerdrContext {
            workspace_id: "w1".into(),
            workspace_cwd: "/ws".into(),
            tab_id: "w1:t1".into(),
            focused_pane_id: "w1:p1".into(),
            focused_pane_cwd: "/here".into(),
            invocation_source: "cli".into(),
        }
    }

    #[test]
    fn placeholders() {
        assert_eq!(
            ctx().expand("pane split {pane} --cwd {cwd}", 7),
            "pane split w1:p1 --cwd /here"
        );
        assert_eq!(ctx().expand("claude-{stamp}", 7), "claude-7");
    }

    #[test]
    fn env_and_tilde() {
        std::env::set_var("WK_TEST_VAR", "xyz");
        assert_eq!(ctx().expand("$WK_TEST_VAR/${WK_TEST_VAR}", 0), "xyz/xyz");
        let home = std::env::var("HOME").unwrap();
        assert_eq!(ctx().expand("~/code", 0), format!("{home}/code"));
        assert_eq!(ctx().expand("a~b", 0), "a~b");
    }
}
