//! herdr-whichkey — blezz/which-key-style single-keystroke action menu for herdr.
//!
//! Runs inside a herdr plugin pane (bottom strip). Reads the menu tree from
//! built-in defaults overlaid with the user's whichkey.toml, renders key hints
//! in columns, dispatches on single keystrokes.
//!
//! Scaffold state: module layout and process lifecycle are real; menu content,
//! config merge, dispatch, and theming are stubs filled in by later milestones.

use std::io::Write as _;
use std::time::Duration;

use anyhow::{Context as _, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{cursor, execute, style, terminal};
use serde::Deserialize;

/// Invocation context herdr injects into every plugin process.
/// The only trustworthy source for "where was the user" — the plain
/// HERDR_TAB_ID/HERDR_WORKSPACE_ID env vars can leak from the invoking
/// CLI's own environment (see docs/spike-popup-panes.md).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct HerdrContext {
    workspace_id: String,
    #[serde(default)]
    workspace_label: String,
    #[serde(default)]
    workspace_cwd: String,
    tab_id: String,
    #[serde(default)]
    tab_label: String,
    #[serde(default)]
    focused_pane_id: String,
    #[serde(default)]
    focused_pane_cwd: String,
    #[serde(default)]
    invocation_source: String,
}

impl HerdrContext {
    fn from_env() -> Result<Self> {
        let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
            .context("HERDR_PLUGIN_CONTEXT_JSON not set — run via the herdr plugin, not directly")?;
        serde_json::from_str(&raw).context("could not parse HERDR_PLUGIN_CONTEXT_JSON")
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("defaults") => print_defaults(),
        Some("--version") | Some("-V") => {
            println!("herdr-whichkey {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None | Some("menu") => run_menu(),
        Some(other) => anyhow::bail!("unknown subcommand: {other} (try: menu, defaults)"),
    }
}

/// Print the resolved menu tree (built-ins + user overlay) as TOML.
/// Milestone: defaults + adaptive plugin detection.
fn print_defaults() -> Result<()> {
    println!("# herdr-whichkey resolved defaults (stub — populated in the defaults milestone)");
    println!("[menu]");
    Ok(())
}

/// The menu loop: raw mode, draw the strip, read single keystrokes, dispatch.
fn run_menu() -> Result<()> {
    let ctx = HerdrContext::from_env()?;

    // Tell the launcher we're done on every exit path (incl. panics), so it
    // can stop keeping the invoking action alive — required because herdr may
    // tear popup views down when the invoking action exits (docs/spike-popup-panes.md).
    let _done = DoneSignal::from_env();

    terminal::enable_raw_mode()?;
    let result = menu_loop(&ctx);
    terminal::disable_raw_mode()?;
    result
}

fn menu_loop(ctx: &HerdrContext) -> Result<()> {
    let mut out = std::io::stdout();
    execute!(out, cursor::Hide)?;

    // Placeholder strip until the real tree/renderer lands (core milestone).
    let (cols, _rows) = terminal::size().unwrap_or((80, 8));
    execute!(
        out,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
        style::Print(format!(
            "whichkey scaffold — pane {} ws {} · press any key to echo, Esc/q to close",
            ctx.focused_pane_id, ctx.workspace_id
        )),
        cursor::MoveTo(0, 1),
        style::Print("─".repeat(cols as usize)),
    )?;
    out.flush()?;

    loop {
        if !event::poll(Duration::from_millis(500))? {
            continue;
        }
        if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
            match code {
                KeyCode::Esc | KeyCode::Char('q') => break,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => break,
                other => {
                    execute!(
                        out,
                        cursor::MoveTo(0, 2),
                        terminal::Clear(terminal::ClearType::CurrentLine),
                        style::Print(format!("key: {other:?}")),
                    )?;
                    out.flush()?;
                }
            }
        }
    }

    execute!(out, cursor::Show)?;
    Ok(())
}

/// Writes to the launcher's done-fifo on drop, unblocking the action script.
struct DoneSignal {
    fifo: Option<std::path::PathBuf>,
}

impl DoneSignal {
    fn from_env() -> Self {
        Self {
            fifo: std::env::var_os("WHICHKEY_DONE_FIFO").map(Into::into),
        }
    }
}

impl Drop for DoneSignal {
    fn drop(&mut self) {
        if let Some(path) = self.fifo.take() {
            let _ = std::fs::write(path, b"done\n");
        }
    }
}
