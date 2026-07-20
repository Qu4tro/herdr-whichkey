//! herdr-whichkey — blezz/which-key-style single-keystroke action menu
//! for herdr. Runs inside a herdr popup pane (bottom strip): built-in
//! defaults overlaid with the user's whichkey.toml, rendered as key hints
//! in columns, dispatched on single keystrokes.

mod config;
mod context;
mod dispatch;
mod keys;
mod model;
mod theme;
mod ui;

use anyhow::Result;

use crate::context::HerdrContext;
use crate::model::{Node, NodeKind};

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

fn run_menu() -> Result<()> {
    // Tell the launcher we're done on every exit path, so it can stop
    // keeping the invoking action alive — herdr may tear popup views down
    // when the invoking action exits (docs/spike-popup-panes.md).
    let _done = DoneSignal::from_env();

    // Resolve the palette before touching config errors: they render in
    // the strip, themed (falling back to herdr's theme or ANSI).
    let loaded = config::load(true);
    let pal = match &loaded {
        Ok(cfg) => theme::resolve(&cfg.theme),
        Err(_) => theme::resolve(&Default::default()),
    };

    let (tree, ctx) = match loaded.and_then(|cfg| load_tree(cfg)) {
        Ok(v) => v,
        Err(e) => return ui::show_error(&pal, &format!("{e:#}")),
    };
    if tree.is_empty() {
        return ui::show_error(&pal, "menu is empty — every item was hidden or unavailable");
    }

    match ui::run(&tree, &pal, &ctx)? {
        ui::Outcome::Closed => {}
        // Deferred leaves run after the terminal is restored; the popup
        // stays visible for the few ms this takes, which beats dispatching
        // from inside raw mode and racing our own teardown.
        ui::Outcome::Deferred(leaf, label) => {
            if let Err(e) = dispatch::execute(&leaf, &ctx) {
                dispatch::notify_failure(&label, &e);
            }
        }
    }
    Ok(())
}

fn load_tree(cfg: config::Config) -> Result<(Vec<Node>, HerdrContext)> {
    let ctx = HerdrContext::from_env()?;
    let tree = config::build_tree(&cfg.entries)?;

    let plugins = dispatch::installed_plugins();
    let have_bin = |b: &str| dispatch::binary_on_path(b);
    // Probe failure (no server?) keeps action items visible — hiding the
    // whole menu because one probe failed would be the surprising choice.
    let have_plugin = |p: &str| plugins.as_ref().map(|set| set.contains(p)).unwrap_or(true);
    Ok((model::prune_unavailable(tree, &have_bin, &have_plugin), ctx))
}

/// `herdr-whichkey defaults` — the live resolved tree (defaults + user
/// overlay), annotated with what adaptive detection would hide right now.
fn print_defaults() -> Result<()> {
    let cfg = config::load(false)?;
    let tree = config::build_tree(&cfg.entries)?;

    let plugins = dispatch::installed_plugins();
    let have_bin = |b: &str| dispatch::binary_on_path(b);
    let have_plugin = |p: &str| plugins.as_ref().map(|set| set.contains(p)).unwrap_or(true);

    let mut out = format!(
        "# resolved menu (defaults + {})\n",
        config::user_config_path().display()
    );
    print_nodes(&mut out, &tree, "", &have_bin, &have_plugin);
    // One buffered write, EPIPE ignored — `defaults | head` must not panic.
    use std::io::Write as _;
    let _ = std::io::stdout().write_all(out.as_bytes());
    Ok(())
}

fn print_nodes(
    out: &mut String,
    nodes: &[Node],
    indent: &str,
    have_bin: &dyn Fn(&str) -> bool,
    have_plugin: &dyn Fn(&str) -> bool,
) {
    use std::fmt::Write as _;
    for n in nodes {
        let hidden = model::unavailable_reason(n, have_bin, have_plugin)
            .map(|r| format!("   (hidden: {r})"))
            .unwrap_or_default();
        let stick = if n.stick { "   (stick)" } else { "" };
        match &n.kind {
            NodeKind::Group(children) => {
                let _ = writeln!(out, "{indent}{}  {} ›{hidden}", keys::display_key(n.key), n.label);
                print_nodes(out, children, &format!("{indent}  "), have_bin, have_plugin);
            }
            NodeKind::Leaf(_) => {
                let _ = writeln!(out, "{indent}{}  {}{stick}{hidden}", keys::display_key(n.key), n.label);
            }
        }
    }
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
