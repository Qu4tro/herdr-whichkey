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

// ── split-surface sizing ────────────────────────────────────────────────

/// Shrink our bottom-split pane to `target` rows before the first frame.
/// Plugin splits always open at ratio 0.5 and take no ratio flag, and
/// `pane resize` on a bottom-most pane can only grow it — so the shrink
/// is a grow of the original pane (our split sibling) downward, by an
/// exact ratio delta. Best-effort: on any failure the menu just stays at
/// the 50% herdr gave it.
pub fn fit_split_height(target: u16) {
    let Some(own) = std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.is_empty()) else {
        return;
    };
    let Ok(layout) = check(
        Command::new(herdr_bin()).args(["pane", "layout", "--pane", &own]),
        "pane layout",
    ) else {
        return;
    };
    let layout = &layout["result"]["layout"];
    let Some(own_rect) = layout["panes"].as_array().and_then(|ps| {
        ps.iter()
            .find(|p| p["pane_id"].as_str() == Some(own.as_str()))
            .map(|p| p["rect"].clone())
    }) else {
        return;
    };
    // The innermost down-split holding our pane. Its stored ratio is the
    // exact value resize amounts add to — deriving it from row counts is
    // off by the rounding herdr applied when it laid the rows out.
    let Some((ratio_now, split_h)) = layout["splits"].as_array().and_then(|ss| {
        ss.iter()
            .filter(|s| s["direction"].as_str() == Some("down"))
            .filter(|s| rect_contains(&s["rect"], &own_rect))
            .min_by_key(|s| s["rect"]["height"].as_u64().unwrap_or(u64::MAX))
            .and_then(|s| Some((s["ratio"].as_f64()?, s["rect"]["height"].as_u64()?)))
    }) else {
        return;
    };
    let Some(amount) = fit_amount(ratio_now, split_h, u64::from(target)) else { return };
    // The sibling is whoever was focused when the launcher split it off.
    // If the ids match, the env is not ours to trust (see the context-leak
    // trap in docs/spike-popup-panes.md) — don't resize anything.
    let Some(orig) = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v["focused_pane_id"].as_str().map(String::from))
    else {
        return;
    };
    if orig == own {
        return;
    }
    let _ = check(
        Command::new(herdr_bin()).args([
            "pane",
            "resize",
            "--pane",
            &orig,
            "--direction",
            "down",
            "--amount",
            &format!("{amount:.4}"),
        ]),
        "pane resize",
    );
}

/// Ratio delta taking the split from `ratio_now` to the ratio that lays
/// our (second) pane out at `target` rows; None when there is nothing
/// (safe) to do. Single-call amounts cap out around 0.5, which a
/// 0.5-ratio split never needs; the server also clamps ratios to 0.9, so
/// on very tall tabs the strip bottoms out at 10% instead of 8 rows.
fn fit_amount(ratio_now: f64, split_h: u64, target: u64) -> Option<f64> {
    if split_h <= target {
        return None;
    }
    let delta = (split_h - target) as f64 / split_h as f64 - ratio_now;
    (delta > 0.0).then_some(delta)
}

/// Both rects are `{x, y, width, height}`; true when `inner` fits inside.
fn rect_contains(outer: &serde_json::Value, inner: &serde_json::Value) -> bool {
    let f = |v: &serde_json::Value, k: &str| v[k].as_i64();
    match (
        (f(outer, "x"), f(outer, "y"), f(outer, "width"), f(outer, "height")),
        (f(inner, "x"), f(inner, "y"), f(inner, "width"), f(inner, "height")),
    ) {
        ((Some(ox), Some(oy), Some(ow), Some(oh)), (Some(ix), Some(iy), Some(iw), Some(ih))) => {
            ix >= ox && iy >= oy && ix + iw <= ox + ow && iy + ih <= oy + oh
        }
        _ => false,
    }
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
    fn fit_amount_targets_exact_rows() {
        // 23-row split at ratio 0.5, want 8 menu rows: top must reach
        // 15/23, so grow it by the difference.
        assert!((fit_amount(0.5, 23, 8).unwrap() - (15.0 / 23.0 - 0.5)).abs() < 1e-9);
        // Already at/above the needed ratio, or a split too small: no-op.
        assert_eq!(fit_amount(0.9, 23, 8), None);
        assert_eq!(fit_amount(0.5, 8, 8), None);
        assert_eq!(fit_amount(0.5, 6, 8), None);
    }

    #[test]
    fn rect_containment() {
        let r = |x, y, w, h| serde_json::json!({"x": x, "y": y, "width": w, "height": h});
        assert!(rect_contains(&r(0, 0, 80, 23), &r(0, 15, 80, 8)));
        assert!(rect_contains(&r(0, 0, 80, 23), &r(0, 0, 80, 23)));
        assert!(!rect_contains(&r(0, 0, 40, 23), &r(30, 0, 40, 8)));
        assert!(!rect_contains(&serde_json::json!({}), &r(0, 0, 1, 1)));
    }

    #[test]
    fn path_probe() {
        assert!(binary_on_path("sh"));
        assert!(!binary_on_path("definitely-not-a-real-binary-xyzzy"));
    }
}
