//! Leaf execution: herdr CLI calls, plugin action invokes, shell commands
//! (background / new pane / new tab), plus the availability probes behind
//! adaptive auto-hide.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context as _, Result};

use crate::context::{now_stamp, HerdrContext};
use crate::model::{Leaf, RunIn};

/// The herdr binary the host told us to use. Falling back to PATH keeps
/// `herdr-whichkey defaults` useful from a plain shell.
pub fn herdr_bin() -> PathBuf {
    std::env::var_os("HERDR_BIN_PATH")
        .map(Into::into)
        .unwrap_or_else(|| PathBuf::from("herdr"))
}

/// Execute one leaf. Called inline for `stick` items (errors go to the
/// strip) and after the menu closes otherwise (errors go to a herdr
/// notification, see `notify_failure`).
pub fn execute(leaf: &Leaf, ctx: &HerdrContext) -> Result<()> {
    let stamp = now_stamp();
    match leaf {
        Leaf::Herdr(cmd) => run_herdr_line(cmd, ctx, stamp),
        Leaf::Action(id) => invoke_action(id),
        Leaf::Run { cmd, run_in, cwd } => {
            let cmd = ctx.expand_placeholders(cmd, stamp);
            let cwd = cwd.as_deref().map(|c| ctx.expand(c, stamp));
            match run_in {
                RunIn::Background => run_background(&cmd, cwd.as_deref()),
                RunIn::Pane => run_in_pane(&cmd, cwd.as_deref(), ctx),
                RunIn::Tab => run_in_tab(&cmd, cwd.as_deref(), ctx),
            }
        }
    }
}

/// Best-effort failure surface for actions that run after the menu closed.
pub fn notify_failure(what: &str, err: &anyhow::Error) {
    let _ = Command::new(herdr_bin())
        .args(["notification", "show", "whichkey: action failed"])
        .arg("--body")
        .arg(format!("{what}: {err:#}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `herdr = "pane split {pane} --direction right"` — split like a shell
/// would, expand each token, exec against the herdr CLI (no shell).
fn run_herdr_line(cmd: &str, ctx: &HerdrContext, stamp: u64) -> Result<()> {
    let tokens = shell_words::split(cmd).with_context(|| format!("bad herdr command: {cmd}"))?;
    if tokens.is_empty() {
        bail!("empty herdr command");
    }
    let args: Vec<String> = tokens.iter().map(|t| ctx.expand(t, stamp)).collect();
    check(Command::new(herdr_bin()).args(&args), &format!("herdr {}", args.join(" ")))?;
    Ok(())
}

fn invoke_action(id: &str) -> Result<()> {
    // Config validation guarantees a qualified "plugin.action" id.
    let (plugin, action) = id.rsplit_once('.').expect("validated action id");
    check(
        Command::new(herdr_bin()).args(["plugin", "action", "invoke", action, "--plugin", plugin]),
        &format!("action {id}"),
    )?;
    Ok(())
}

/// Detached `sh -c`, with a wrapper that surfaces failure as a herdr
/// notification — a background command that dies silently is a footgun.
/// The command rides in an env var so no quoting of user text is needed.
fn run_background(cmd: &str, cwd: Option<&str>) -> Result<()> {
    let script = r#"( eval "$WK_CMD" ) >/dev/null 2>&1 || \
        "$WK_HERDR" notification show 'whichkey: command failed' --body "$WK_CMD" >/dev/null 2>&1"#;
    let mut c = Command::new("sh");
    c.args(["-c", script])
        .env("WK_CMD", cmd)
        .env("WK_HERDR", herdr_bin())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    c.spawn().with_context(|| format!("could not start: {cmd}"))?;
    Ok(())
}

/// Split the focused pane downward and type the command into the new pane.
fn run_in_pane(cmd: &str, cwd: Option<&str>, ctx: &HerdrContext) -> Result<()> {
    if ctx.focused_pane_id.is_empty() {
        bail!("no focused pane in context — can't split");
    }
    let mut c = Command::new(herdr_bin());
    c.args(["pane", "split", &ctx.focused_pane_id, "--direction", "down"]);
    if let Some(dir) = cwd {
        c.args(["--cwd", dir]);
    }
    let out = check(&mut c, "pane split")?;
    let pane = find_str_key(&out, "pane_id")
        .context("pane split succeeded but no pane_id in its output")?;
    check(Command::new(herdr_bin()).args(["pane", "run", &pane, cmd]), "pane run")?;
    Ok(())
}

/// New tab in the current workspace, then type the command into its pane.
fn run_in_tab(cmd: &str, cwd: Option<&str>, ctx: &HerdrContext) -> Result<()> {
    let mut c = Command::new(herdr_bin());
    c.args(["tab", "create"]);
    if !ctx.workspace_id.is_empty() {
        c.args(["--workspace", &ctx.workspace_id]);
    }
    if let Some(dir) = cwd {
        c.args(["--cwd", dir]);
    }
    let out = check(&mut c, "tab create")?;
    let pane = match find_str_key(&out, "pane_id") {
        Some(p) => p,
        None => {
            // Older shape: result carries only the tab id; look its pane up.
            let tab = find_str_key(&out, "tab_id")
                .context("tab create succeeded but no tab_id in its output")?;
            let mut list = Command::new(herdr_bin());
            list.args(["pane", "list"]);
            if !ctx.workspace_id.is_empty() {
                list.args(["--workspace", &ctx.workspace_id]);
            }
            let panes = check(&mut list, "pane list")?;
            pane_in_tab(&panes, &tab).with_context(|| format!("no pane found in new tab {tab}"))?
        }
    };
    check(Command::new(herdr_bin()).args(["pane", "run", &pane, cmd]), "pane run")?;
    Ok(())
}

/// Run a herdr command, error with its stderr on failure, parse the
/// one-line JSON `{"id": …, "result": …}` it prints on success.
fn check(cmd: &mut Command, what: &str) -> Result<serde_json::Value> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not run {what}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        let msg = if err.is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            err.to_string()
        };
        bail!("{what}: {msg}");
    }
    Ok(serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null))
}

/// Depth-first search for the first string value under `key` — output
/// shapes vary a little across herdr versions, the field names don't.
fn find_str_key(v: &serde_json::Value, key: &str) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|v| find_str_key(v, key))
        }
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_str_key(v, key)),
        _ => None,
    }
}

/// From `pane list` output, the id of a pane living in `tab`.
fn pane_in_tab(v: &serde_json::Value, tab: &str) -> Option<String> {
    fn walk(v: &serde_json::Value, tab: &str) -> Option<String> {
        match v {
            serde_json::Value::Object(map) => {
                if map.get("tab_id").and_then(|t| t.as_str()) == Some(tab) {
                    if let Some(p) = map.get("pane_id").and_then(|p| p.as_str()) {
                        return Some(p.to_string());
                    }
                }
                map.values().find_map(|v| walk(v, tab))
            }
            serde_json::Value::Array(items) => items.iter().find_map(|v| walk(v, tab)),
            _ => None,
        }
    }
    walk(v, tab)
}

// ── availability probes (adaptive auto-hide) ────────────────────────────

/// Installed plugin ids via `herdr plugin action list`. `None` when the
/// probe fails (no herdr, no server) — callers should then keep action
/// items visible rather than hiding everything.
pub fn installed_plugins() -> Option<HashSet<String>> {
    let out = Command::new(herdr_bin())
        .args(["plugin", "action", "list"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut ids = HashSet::new();
    collect_str_key(&v, "plugin_id", &mut ids);
    Some(ids)
}

fn collect_str_key(v: &serde_json::Value, key: &str, into: &mut HashSet<String>) {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                into.insert(s.clone());
            }
            for v in map.values() {
                collect_str_key(v, key, into);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_str_key(v, key, into);
            }
        }
        _ => {}
    }
}

/// Plain PATH scan — no shell-out per item.
pub fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(name);
        p.is_file() && is_executable(&p)
    })
}

#[cfg(unix)]
fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_p: &std::path::Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_key_search() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"id":1,"result":{"pane":{"pane_id":"w1:p9","tab_id":"w1:t2"}}}"#,
        )
        .unwrap();
        assert_eq!(find_str_key(&v, "pane_id").unwrap(), "w1:p9");
        assert_eq!(pane_in_tab(&v, "w1:t2").unwrap(), "w1:p9");
        assert!(pane_in_tab(&v, "w1:t3").is_none());
    }

    #[test]
    fn path_probe() {
        assert!(binary_on_path("sh"));
        assert!(!binary_on_path("definitely-not-a-real-binary-xyzzy"));
    }
}
