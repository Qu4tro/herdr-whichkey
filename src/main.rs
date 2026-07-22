//! herdr-whichkey — blezz/which-key-style single-keystroke action menu
//! for herdr. Runs inside a herdr plugin pane docked as a bottom split:
//! the menu tree from the user's keys file (the shipped one until `init`
//! writes it), settings from whichkey.toml, rendered as key hints in
//! columns, dispatched on single keystrokes.

mod config;
mod context;
mod dispatch;
mod keys;
mod layout;
mod model;
mod theme;
mod ui;

use anyhow::Result;

use crate::context::HerdrContext;
use crate::model::{Node, NodeKind};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("defaults") => match args.collect::<Vec<_>>().as_slice() {
            [] => print_defaults(false),
            [flag] if flag == "--shipped" => print_defaults(true),
            _ => anyhow::bail!("usage: herdr-whichkey defaults [--shipped]"),
        },
        Some("init") => init(),
        Some("--version") | Some("-V") => {
            println!("herdr-whichkey {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None | Some("menu") => run_menu(),
        Some(other) => anyhow::bail!("unknown subcommand: {other} (try: menu, defaults, init)"),
    }
}

fn run_menu() -> Result<()> {
    // Tell the launcher we're done on every exit path, so it can stop
    // keeping the invoking action alive — herdr may tear popup views down
    // when the invoking action exits (docs/spike-popup-panes.md).
    let _done = DoneSignal::from_env();

    // Drop doesn't run when we die by signal (e.g. herdr tearing the popup
    // down), which would leave the launcher blocked on the fifo until its
    // timeout — signal the fifo from a handler thread instead.
    std::thread::spawn(|| {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        let mut signals = match signal_hook::iterator::Signals::new([SIGTERM, SIGHUP, SIGINT]) {
            Ok(s) => s,
            Err(_) => return,
        };
        if signals.forever().next().is_some() {
            // Drop doesn't run, so undo mouse reporting by hand. Unconditional:
            // disabling modes that were never enabled is a no-op, and herdr
            // resets the host's own mouse state when our pane dies anyway.
            let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
            if let Some(path) = std::env::var_os("WHICHKEY_DONE_FIFO") {
                write_done_fifo(std::path::Path::new(&path));
            }
            std::process::exit(130);
        }
    });

    // Config first: the split fit below wants `[layout] height`, and
    // loading a local TOML is nothing next to the herdr calls that follow.
    let loaded = config::load();

    // Split placement opens at ratio 0.5 and can't be sized at open time
    // — shrink to strip height before the first frame paints (the
    // launcher tags the surface; popups size themselves at open).
    if std::env::var_os("WHICHKEY_SURFACE").is_some_and(|v| v == "split") {
        let h = loaded.as_ref().ok().and_then(|c| c.layout.height).unwrap_or(7);
        dispatch::fit_split_height(h.max(3));
    }

    // Resolve the palette before touching config errors: they render in
    // the strip, themed (falling back to herdr's theme or ANSI).
    let pal = match &loaded {
        Ok(cfg) => theme::resolve(&cfg.theme),
        Err(_) => theme::resolve(&Default::default()),
    };

    let (tree, ctx, lay, uic) = match loaded.and_then(load_tree) {
        Ok(v) => v,
        Err(e) => return ui::show_error(&pal, &format!("{e:#}")),
    };
    if tree.is_empty() {
        return ui::show_error(&pal, "menu is empty — every item was hidden or unavailable");
    }

    match ui::run(&tree, &pal, &lay, &uic, &ctx)? {
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

type Menu = (Vec<Node>, HerdrContext, layout::LayoutConfig, ui::UiConfig);

fn load_tree(cfg: config::Config) -> Result<Menu> {
    let ctx = HerdrContext::from_env()?;
    let tree = config::build_tree(&cfg.entries)?;

    let plugins = dispatch::installed_plugins();
    let have_bin = |b: &str| dispatch::binary_on_path(b);
    // Probe failure (no server?) keeps action items visible — hiding the
    // whole menu because one probe failed would be the surprising choice.
    let have_plugin = |p: &str| plugins.as_ref().map(|set| set.contains(p)).unwrap_or(true);
    Ok((model::prune_unavailable(tree, &have_bin, &have_plugin), ctx, cfg.layout, cfg.ui))
}

/// `herdr-whichkey init` — write the shipped menu into the keys file and a
/// commented settings stub into whichkey.toml. Existing files are never
/// clobbered; a pre-split whichkey.toml is moved aside rather than kept,
/// since its `[menu]` table would keep the menu from loading at all.
fn init() -> Result<()> {
    let mut ported = None;
    for wrote in config::init()? {
        match wrote {
            config::Wrote::Created(p) => println!("wrote {}", p.display()),
            config::Wrote::Kept(p) => println!("{} already exists — left alone", p.display()),
            config::Wrote::MovedAside { from, to } => {
                println!("moved {} → {}", from.display(), to.display());
                ported = Some(to);
            }
        }
    }
    if let Some(old) = ported {
        println!(
            "\n{} still holds your old [menu] table — nothing else reads it now.\n\
             Port the entries you want into the keys file above; `herdr-whichkey\n\
             defaults --shipped` prints what that file started as.",
            old.display()
        );
    }
    Ok(())
}

/// `herdr-whichkey defaults` — the live menu tree, annotated with what
/// adaptive detection would hide right now. `--shipped` prints the tree
/// `init` would write instead, so a customised keys file can be diffed
/// against the one the plugin ships.
fn print_defaults(shipped: bool) -> Result<()> {
    let (header, entries) = if shipped {
        (
            "# shipped menu (what `herdr-whichkey init` writes)".to_string(),
            config::shipped_entries(),
        )
    } else {
        let cfg = config::load()?;
        let header = match &cfg.menu_source {
            config::MenuSource::KeysFile(p) => format!("# menu from {}", p.display()),
            config::MenuSource::BuiltIn => {
                "# built-in menu — no keys file yet (`herdr-whichkey init` writes one)".to_string()
            }
        };
        (header, cfg.entries)
    };
    let tree = config::build_tree(&entries)?;

    let plugins = dispatch::installed_plugins();
    let have_bin = |b: &str| dispatch::binary_on_path(b);
    let have_plugin = |p: &str| plugins.as_ref().map(|set| set.contains(p)).unwrap_or(true);

    let mut out = format!("{header}\n");
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
                let _ =
                    writeln!(out, "{indent}{}  {} ›{hidden}", keys::display_key(n.key), n.label);
                print_nodes(out, children, &format!("{indent}  "), have_bin, have_plugin);
            }
            NodeKind::Leaf(_) => {
                let _ = writeln!(
                    out,
                    "{indent}{}  {}{stick}{hidden}",
                    keys::display_key(n.key),
                    n.label
                );
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
        Self { fifo: std::env::var_os("WHICHKEY_DONE_FIFO").map(Into::into) }
    }
}

impl Drop for DoneSignal {
    fn drop(&mut self) {
        if let Some(path) = self.fifo.take() {
            write_done_fifo(&path);
        }
    }
}

/// Non-blocking fifo write: a plain `fs::write` would block forever when
/// the launcher is already gone (reader-less fifo), hanging our own exit.
/// O_NONBLOCK without O_CREAT fails fast in every launcher-gone case.
fn write_done_fifo(path: &std::path::Path) {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    if let Ok(mut f) =
        std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(path)
    {
        let _ = f.write_all(b"done\n");
    }
}
