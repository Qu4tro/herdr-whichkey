//! Config loading, split across two files: whichkey.toml holds settings and
//! stays sparse (anything undeclared falls back to a built-in default, and
//! those defaults keep improving with the plugin), while the menu tree lives
//! in a keys file that is the user's — written once by `init`, never touched
//! by us again. Flat key-paths from that file build the menu tree.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use serde::Deserialize;

use crate::keys;
use crate::layout::LayoutConfig;
use crate::model::{fallback_label, leaf_from_spec, ItemSpec, Node, NodeKind};
use crate::theme::ThemeOverrides;
use crate::ui::UiConfig;

/// The shipped menu tree: the tree that renders before `init` runs, and the
/// template `init` writes into the keys file (verbatim, uncommented).
pub const DEFAULTS_TOML: &str = include_str!("defaults.toml");

/// Default keys file name, alongside whichkey.toml unless `keys_path` says
/// otherwise.
pub const KEYS_FILE: &str = "keys.toml";

/// whichkey.toml — settings only, every field optional.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    /// Where the keys file lives. Relative paths resolve against this file's
    /// own directory; `~` and `$VARS` expand.
    keys_path: Option<String>,
    #[serde(default)]
    theme: ThemeOverrides,
    #[serde(default)]
    layout: LayoutConfig,
    #[serde(default)]
    ui: UiConfig,
    /// Where pre-split configs kept the menu. Parsed, never read: accepting it
    /// here is what lets one pass over the file tell the loader to raise the
    /// migration error *and* tell `init` whether the settings around it are
    /// sound — with toml's line and column on whatever else is wrong.
    ///
    /// A table, specifically. A pre-split file has a `[menu]` *table*; anything
    /// else spelled `menu` is a typo, and taking it for a menu would migrate a
    /// file that never had one. As a table it fails to deserialize instead, on
    /// the ordinary invalid-value path, which is where a typo belongs.
    #[serde(default)]
    menu: Option<toml::Table>,
}

/// The keys file — the whole menu, in file order.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct KeysFile {
    #[serde(default)]
    menu: toml::map::Map<String, toml::Value>,
}

/// Where the entries came from, for `defaults` to name.
#[derive(Debug, Clone)]
pub enum MenuSource {
    /// No keys file yet — the compiled-in tree is rendering.
    BuiltIn,
    KeysFile(PathBuf),
}

pub struct Config {
    /// The whole menu, flat, in file order — which is display order.
    pub entries: Vec<(String, ItemSpec)>,
    pub theme: ThemeOverrides,
    pub layout: LayoutConfig,
    pub ui: UiConfig,
    pub menu_source: MenuSource,
}

/// The user's settings file path: plugin config dir when running under herdr,
/// the same well-known location otherwise (so `herdr-whichkey defaults` works
/// from a plain shell).
pub fn user_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return PathBuf::from(dir).join("whichkey.toml");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
    });
    base.join("herdr/plugins/config/herdr-whichkey/whichkey.toml")
}

/// Resolve `keys_path` against the settings file it was declared in.
fn resolve_keys_path(config_path: &Path, keys_path: Option<&str>) -> PathBuf {
    let dir = config_path.parent().unwrap_or(Path::new("."));
    match keys_path {
        None => dir.join(KEYS_FILE),
        Some(raw) => {
            let expanded = PathBuf::from(crate::context::expand_env_tilde(raw));
            if expanded.is_absolute() {
                expanded
            } else {
                dir.join(expanded)
            }
        }
    }
}

pub fn load() -> Result<Config> {
    let path = user_config_path();
    let settings = load_settings(&path)?;
    let keys_path = resolve_keys_path(&path, settings.keys_path.as_deref());

    // No keys file is the normal pre-`init` state: the shipped tree renders so
    // the plugin works the moment herdr installs it. A `keys_path` the user
    // wrote by hand is an assertion that a file is there, though — falling back
    // silently would look like the plugin ignoring their config.
    let (text, menu_source) = match std::fs::read_to_string(&keys_path) {
        Ok(text) => (text, MenuSource::KeysFile(keys_path.clone())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if settings.keys_path.is_some() {
                // Short lines, not one long one: config errors render in the
                // strip, which clips each line to the pane width (ui::show_error).
                bail!(
                    "keys_path points at a file that does not exist:\n  \
                     {}\nCreate it, or run `herdr-whichkey init` to write the \
                     shipped menu there.",
                    keys_path.display()
                );
            }
            (DEFAULTS_TOML.to_string(), MenuSource::BuiltIn)
        }
        Err(e) => return Err(e).with_context(|| format!("could not read {}", keys_path.display())),
    };

    let keys: KeysFile = toml::from_str(&text)
        .with_context(|| format!("could not parse {}", keys_path.display()))?;
    let entries = flatten_menu(&keys.menu)?;

    Ok(Config {
        entries,
        theme: settings.theme,
        layout: settings.layout,
        ui: settings.ui,
        menu_source,
    })
}

fn load_settings(path: &Path) -> Result<FileConfig> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Sparse means the file need not exist at all.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileConfig::default()),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    // One parse decides everything, so `init` and the loader can never
    // disagree about what a given file is.
    let settings: FileConfig =
        toml::from_str(&text).with_context(|| format!("could not parse {}", path.display()))?;
    // Every config seeded before the split carries a live `[menu]` table. It is
    // accepted by the parser only to be rejected here, with somewhere to go.
    if settings.menu.is_some() {
        bail!(
            "[menu] moved out of whichkey.toml — the menu is its own file now:\n  \
             {}\nMove that table there, or run `herdr-whichkey init` (it keeps a copy).",
            resolve_keys_path(path, settings.keys_path.as_deref()).display()
        );
    }
    Ok(settings)
}

/// Flatten one keys file's `[menu]` table into entries, in file order.
fn flatten_menu(menu: &toml::map::Map<String, toml::Value>) -> Result<Vec<(String, ItemSpec)>> {
    let mut entries: Vec<(String, ItemSpec)> = Vec::new();
    // The key sequence each entry resolves to, against the spelling it was
    // written as, so a collision can name both sides.
    let mut seen: Vec<(Vec<char>, &str)> = Vec::new();

    for (raw, value) in menu {
        let path = normalize_path(raw);
        // Identity is the parsed sequence, not the spelling. TOML rejects
        // literally duplicate keys, but `"g s"`/`"g  s"` differ in whitespace
        // and `"G"`/`"shift+g"`, `"?"`/`"question"` are different spellings of
        // one key — all of them land on the same node. Checking the spelling
        // would let the aliases through to `insert`, where the later entry
        // overwrites the earlier: replace-in-place, back by the side door.
        let seq = keys::parse_path(&path).with_context(|| format!("in item \"{raw}\""))?;
        if seq.is_empty() {
            bail!("\"{raw}\": empty key path");
        }
        if let Some((_, first)) = seen.iter().find(|(s, _)| *s == seq) {
            bail!(
                "\"{first}\" and \"{raw}\" are the same key path ({}) — remove one",
                seq.iter().map(|&c| keys::display_key(c)).collect::<Vec<_>>().join(" ")
            );
        }
        let spec: ItemSpec = value
            .clone()
            .try_into()
            .with_context(|| format!("\"{raw}\": expected an item table, e.g. {{ … }}"))?;
        seen.push((seq, raw));
        entries.push((path, spec));
    }
    Ok(entries)
}

/// The shipped tree, for `defaults --shipped` and for the init tests.
pub fn shipped_entries() -> Vec<(String, ItemSpec)> {
    let keys: KeysFile = toml::from_str(DEFAULTS_TOML).expect("built-in defaults must parse");
    flatten_menu(&keys.menu).expect("built-in defaults must flatten")
}

fn normalize_path(path: &str) -> String {
    path.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Build the menu tree from flat entries, validating keys and leaf shapes.
pub fn build_tree(entries: &[(String, ItemSpec)]) -> Result<Vec<Node>> {
    let mut roots: Vec<Node> = Vec::new();

    // Two passes: groups/labels first so children can attach in any file order,
    // then leaves — but a single ordered pass with on-demand group creation
    // keeps file order, so do that and fix up leaf/group conflicts as we go.
    for (path, spec) in entries {
        let seq = keys::parse_path(path).with_context(|| format!("in item \"{path}\""))?;
        if seq.is_empty() {
            bail!("\"{path}\": empty key path");
        }
        insert(&mut roots, path, &seq, spec)?;
    }

    // A group entry that never got children and has no action is a config slip.
    fn check(nodes: &[Node], path: &str) -> Result<()> {
        for n in nodes {
            let p = if path.is_empty() {
                keys::display_key(n.key)
            } else {
                format!("{path} {}", keys::display_key(n.key))
            };
            if let NodeKind::Group(children) = &n.kind {
                if children.is_empty() {
                    bail!("\"{p}\": has neither an action nor child items");
                }
                check(children, &p)?;
            }
        }
        Ok(())
    }
    check(&roots, "")?;
    Ok(roots)
}

fn insert(roots: &mut Vec<Node>, path: &str, seq: &[char], spec: &ItemSpec) -> Result<()> {
    let mut level = roots;
    for (i, &key) in seq.iter().enumerate() {
        let last = i == seq.len() - 1;
        let pos = level.iter().position(|n| n.key == key);

        if last {
            let node = make_node(path, key, spec)?;
            match pos {
                Some(idx) => {
                    let existing = &mut level[idx];
                    if spec.is_group_only() {
                        // Label-only entry on an existing node: relabel/decorate.
                        if let Some(l) = &spec.label {
                            existing.label = l.clone();
                        }
                        existing.requires = spec.requires.clone().or(existing.requires.take());
                        // …and re-anchor it here. The node only exists already
                        // because a deeper item sprang the group into being at
                        // that earlier line; this line is the group's own
                        // declaration, so it is the one that decides where the
                        // group sits. Without this, file order is display order
                        // for every item except a group declared below its
                        // children — moving that line would do nothing.
                        // Everything after it appends, so "here" is the end.
                        let node = level.remove(idx);
                        level.push(node);
                    } else if existing.is_group() && !existing.children().is_empty() {
                        bail!(
                            "\"{path}\": is a group (deeper items exist) but also sets an action — move the action to a child key"
                        );
                    } else {
                        level[idx] = node;
                    }
                }
                None => level.push(node),
            }
            return Ok(());
        }

        let idx = match pos {
            Some(idx) => {
                if !level[idx].is_group() {
                    bail!(
                        "\"{path}\": parent key '{}' is already an action item — a key can't be both",
                        keys::display_key(key)
                    );
                }
                idx
            }
            None => {
                level.push(Node {
                    key,
                    label: format!("+{}", keys::display_key(key)),
                    stick: false,
                    requires: None,
                    requires_plugin: None,
                    kind: NodeKind::Group(Vec::new()),
                });
                level.len() - 1
            }
        };
        level = match &mut level[idx].kind {
            NodeKind::Group(children) => children,
            NodeKind::Leaf(_) => unreachable!(),
        };
    }
    Ok(())
}

fn make_node(path: &str, key: char, spec: &ItemSpec) -> Result<Node> {
    let kind = if spec.is_group_only() {
        // stick is per-action and never inherited, so on a group it did
        // nothing at all — refuse it instead of silently dropping it.
        if spec.stick {
            bail!(
                "\"{path}\": stick = true needs an action on the same item — it isn't inherited, so set it on each child item that should repeat"
            );
        }
        NodeKind::Group(Vec::new())
    } else {
        NodeKind::Leaf(leaf_from_spec(path, spec)?)
    };
    let requires_plugin = spec
        .action
        .as_ref()
        .map(|a| a.rsplit_once('.').map(|(p, _)| p.to_string()).unwrap_or_default());
    Ok(Node {
        key,
        label: fallback_label(spec),
        stick: spec.stick,
        requires: spec.requires.clone(),
        requires_plugin,
        kind,
    })
}

/// The settings stub `init` writes. Section headers are live and every knob
/// is commented: an empty `[layout]` resolves exactly like an absent one, and
/// live headers mean uncommenting a single line just works — under a
/// commented-out `# [theme]`, an uncommented `name =` would land at top level
/// and be rejected.
///
/// Note the asymmetry with the keys file, which `init` writes uncommented: a
/// commented knob keeps tracking the built-in default and improves with the
/// plugin, whereas the menu is frozen at init because it becomes the user's.
const SETTINGS_STUB: &str = r##"# herdr-whichkey — settings.
#
# Everything here is optional, and sparse: a knob you leave commented keeps
# tracking its built-in default, and those defaults improve as the plugin
# updates. Uncomment one to pin it.
#
# Your menu is not here — it lives in the keys file (keys.toml next to this
# one), which is yours: written once by `herdr-whichkey init`, never touched
# by the plugin again.

# keys_path = "keys.toml"     # where the menu lives (relative to this file)

[layout]                      # where the menu opens and how items spread
# placement = "bottom"        #   bottom (split below) | right (split beside) | popup (centered float)
# height  = 7                 #   bottom: strip rows · popup: float rows (~2 are chrome either way)
# width   = 32                #   right: strip columns · popup: float columns
# justify = "space-evenly"    #   columns: start | center | end | space-between | space-around | space-evenly
# align   = "space-around"    #   rows: same keywords (both default per placement)
# columns = 4                 #   pin the column count (default: fit the width)
# gutter  = 9                 #   cells between columns (default: half an item)

[theme]                       # colors, detected from herdr's own config by default
# name   = "gruvbox"          #   force a palette, skipping detection
# accent = "#fe8019"          #   per-role: bg, surface, fg, dim, accent, warn

[ui]
# mouse = false               #   keyboard-only; the strip stops requesting mouse input
"##;

/// What `init` did with one file.
#[derive(Debug)]
pub enum Wrote {
    Created(PathBuf),
    /// Left alone — an existing file is the user's, and the keys file in
    /// particular is frozen once it exists.
    Kept(PathBuf),
    /// A pre-split whichkey.toml renamed out of the way, its `[menu]` intact
    /// for the user to port by hand.
    MovedAside {
        from: PathBuf,
        to: PathBuf,
    },
}

/// Suffix given to a pre-split whichkey.toml moved aside by `init`.
const LEGACY_SUFFIX: &str = ".pre-split";

/// `init`: materialise the shipped menu into the keys file, and drop a
/// settings stub beside it. Existing files are never clobbered.
pub fn init() -> Result<Vec<Wrote>> {
    let config_path = user_config_path();
    // Deliberately not `load_settings`: that *fails* on a pre-split file, and
    // the error it raises tells the user to run this command. init has to be
    // the one thing that shape cannot break, or the migration deadlocks.
    let text = std::fs::read_to_string(&config_path).unwrap_or_default();
    let plan = init_plan(&config_path, &text)?;
    let keys_path = resolve_keys_path(&config_path, plan.declared.as_deref());

    init_at(&config_path, &keys_path, plan.legacy_menu, plan.declared.as_deref())
}

/// What `init` makes of the settings file it found.
#[derive(Debug)]
struct InitPlan {
    /// `keys_path` as the user wrote it, to resolve against and to carry over.
    declared: Option<String>,
    /// The file is pre-split and has to move aside before the stub lands.
    legacy_menu: bool,
}

fn init_plan(config_path: &Path, text: &str) -> Result<InitPlan> {
    match toml::from_str::<FileConfig>(text) {
        Ok(settings) => {
            Ok(InitPlan { declared: settings.keys_path, legacy_menu: settings.menu.is_some() })
        }
        // Only the `[menu]` half is ours to move. Migrating a file whose
        // settings we could not read would move them aside and hand back a
        // stub — replacing knobs we never managed to parse, and reporting
        // success. Refuse, and pass on toml's own line and column.
        Err(e) if has_legacy_menu(text) => Err(e).with_context(|| {
            format!(
                "{} has a [menu] table to migrate, but its settings do not\n\
                     parse — fix them, then run `herdr-whichkey init` again.",
                config_path.display()
            )
        }),
        // Broken for some other reason, or not TOML at all: the user's own
        // typo, and no migration to get wrong. Leave the file alone — it will
        // be `Kept` below — and let the menu report it.
        Err(_) => Ok(InitPlan { declared: None, legacy_menu: false }),
    }
}

/// Whether `text` is a pre-split config: one carrying a `[menu]` **table**.
/// Used only when the file is too broken to deserialize, so the typed `menu`
/// field cannot answer; it has to agree with that field about what counts.
fn has_legacy_menu(text: &str) -> bool {
    toml::from_str::<toml::Table>(text)
        .is_ok_and(|t| t.get("menu").is_some_and(toml::Value::is_table))
}

fn init_at(
    config_path: &Path,
    keys_path: &Path,
    legacy_menu: bool,
    declared: Option<&str>,
) -> Result<Vec<Wrote>> {
    let mut done = vec![write_new(keys_path, DEFAULTS_TOML)?];
    // A pre-split whichkey.toml would keep failing to load with the stub
    // sitting unwritten behind it, so move it aside — renamed, not rewritten,
    // and never deleted: its `[menu]` is the only record of what the user had.
    if legacy_menu {
        done.push(move_aside(config_path)?);
    }
    done.push(write_new(config_path, &settings_stub(declared))?);
    Ok(done)
}

/// The stub, carrying over a `keys_path` the replaced config declared.
///
/// Live, not commented: the menu was just written to *that* path, so a stub
/// falling back to the commented default would point the next load at
/// `keys.toml` and orphan the file it had only just created.
fn settings_stub(keys_path: Option<&str>) -> String {
    let Some(raw) = keys_path else {
        return SETTINGS_STUB.to_string();
    };
    // Through toml's own serializer, so a path with a quote or a backslash in
    // it comes back out as the same string it went in as.
    let declaration = format!(
        "keys_path = {}   # where the menu lives (kept from your previous config)",
        toml::Value::String(raw.to_string())
    );
    let mut out: Vec<&str> = SETTINGS_STUB.lines().collect();
    for line in out.iter_mut() {
        if line.starts_with("# keys_path") {
            *line = &declaration;
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// Copy `path` to `<name>.pre-split` and remove the original, refusing to
/// overwrite an earlier backup.
fn move_aside(path: &Path) -> Result<Wrote> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(LEGACY_SUFFIX);
    let to = path.with_file_name(name);

    // Copy-then-remove rather than `fs::rename`, which overwrites silently: a
    // second migration must not bury the first backup. Through `write_new`, so
    // the backup lands whole or not at all — a half-written one would both
    // lose the config it is preserving and block every later attempt.
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    if matches!(write_new(&to, &contents)?, Wrote::Kept(_)) {
        bail!(
            "{} already exists — move or delete it, then run `herdr-whichkey init` again",
            to.display()
        );
    }
    std::fs::remove_file(path).with_context(|| format!("could not remove {}", path.display()))?;
    Ok(Wrote::MovedAside { from: path.to_path_buf(), to })
}

/// Write `contents` to `path` unless something is already there.
///
/// Two guarantees, and they need the same mechanism. The file is the user's,
/// so it must never be clobbered; and it is about to be read back as config,
/// so a write that dies partway must never leave a truncated file sitting
/// where the loader will find it — a retry would see it, call it the user's,
/// and keep it forever. So build the whole thing beside the target and
/// `link(2)` it into place: link fails when the target exists, which makes
/// "don't clobber" and "publish" one atomic step, and a failure anywhere
/// before it abandons nothing but the temp file.
fn write_new(path: &Path, contents: &str) -> Result<Wrote> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
    }
    let (mut file, tmp) = new_temp_beside(path)?;
    let published =
        fill(&mut file, contents, &tmp).and_then(|()| match std::fs::hard_link(&tmp, path) {
            Ok(()) => Ok(Wrote::Created(path.to_path_buf())),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Ok(Wrote::Kept(path.to_path_buf()))
            }
            Err(e) => Err(e).with_context(|| format!("could not write {}", path.display())),
        });
    // Scratch either way: linked into place under its real name, or abandoned.
    let _ = std::fs::remove_file(&tmp);
    published
}

/// How many scratch names to try before giving up rather than spinning.
const TEMP_ATTEMPTS: u32 = 16;

/// Create a scratch file beside `path`, never reopening one already there.
///
/// A run killed between the link and the cleanup leaves a scratch file that is
/// still the same inode as the published one, and the name is predictable
/// enough for a later run to land on it. Opening that truncating would rewrite
/// the user's file through the alias, and the no-clobber link would then report
/// the file `Kept` — destroying it while saying it left it alone. `create_new`
/// makes a leftover a name to step over instead.
fn new_temp_beside(path: &Path) -> Result<(std::fs::File, PathBuf)> {
    for attempt in 0..TEMP_ATTEMPTS {
        let tmp = temp_candidate(path, attempt);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((file, tmp)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("could not write {}", tmp.display())),
        }
    }
    bail!(
        "could not create a scratch file next to {}: {TEMP_ATTEMPTS} candidates \
         all exist — delete the leftover .{}.*.tmp files and try again",
        path.display(),
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

fn fill(file: &mut std::fs::File, contents: &str, tmp: &Path) -> Result<()> {
    use std::io::Write as _;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("could not write {}", tmp.display()))
}

/// The `attempt`-th scratch name in `path`'s own directory — same filesystem,
/// so the link into place can succeed. Hidden and self-describing, in case one
/// is ever left behind by a kill between filling it and the link.
fn temp_candidate(path: &Path, attempt: u32) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.{attempt}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries_of(toml_src: &str) -> Result<Vec<(String, ItemSpec)>> {
        let f: KeysFile = toml::from_str(toml_src).unwrap();
        flatten_menu(&f.menu)
    }

    fn tree_of(toml_src: &str) -> Result<Vec<Node>> {
        build_tree(&entries_of(toml_src)?)
    }

    #[test]
    fn defaults_parse_and_build() {
        let tree = build_tree(&shipped_entries()).unwrap();
        assert!(tree.iter().any(|n| n.key == 'g'));
        let resize = tree.iter().find(|n| n.key == 'r').unwrap();
        assert!(resize.children().iter().all(|c| c.stick));
    }

    /// File order is display order — the whole point of the user-owned keys
    /// file. Reordering the lines reorders the strip.
    #[test]
    fn file_order_is_display_order() {
        let tree = tree_of(
            r#"[menu]
"z" = { label = "zed", run = "zed" }
"a" = { label = "alpha", run = "alpha" }
"#,
        )
        .unwrap();
        assert_eq!(tree.iter().map(|n| n.key).collect::<Vec<_>>(), vec!['z', 'a']);
    }

    /// …including for a group whose children appear above its own declaration.
    /// The child line springs the group into existence early; the declaration
    /// is still the group's own line, so it decides where the group sits and
    /// moving it moves the group.
    #[test]
    fn explicit_group_declaration_anchors_it() {
        let src = |decl_last: bool| {
            let decl = "\"g\"   = { label = \"git\" }\n";
            let rest = "\"g x\" = { run = \"a\" }\n\"a\"   = { label = \"alpha\", run = \"b\" }\n";
            if decl_last {
                format!("[menu]\n{rest}{decl}")
            } else {
                format!("[menu]\n{decl}{rest}")
            }
        };

        let below = tree_of(&src(true)).unwrap();
        assert_eq!(below.iter().map(|n| n.key).collect::<Vec<_>>(), vec!['a', 'g']);

        // Same three lines, declaration moved to the top: the strip follows.
        let above = tree_of(&src(false)).unwrap();
        assert_eq!(above.iter().map(|n| n.key).collect::<Vec<_>>(), vec!['g', 'a']);

        // Re-anchoring is a move, not a rebuild — the label and children survive.
        let git = below.iter().find(|n| n.key == 'g').unwrap();
        assert_eq!(git.label, "git");
        assert_eq!(git.children().len(), 1);
    }

    /// TOML catches literal duplicate keys; these two only collide after
    /// normalization, so `flatten_menu` has to.
    #[test]
    fn duplicate_normalized_path_rejected() {
        let err = entries_of(
            r#"[menu]
"g s"  = { label = "status", run = "git status" }
"g  s" = { label = "stash", run = "git stash" }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("same key path"), "{err}");
        assert!(err.contains("\"g s\"") && err.contains("\"g  s\""), "{err}");
    }

    /// Whitespace is the easy half. These spellings differ as *text* and are
    /// the same key once parsed, so a check on the spelling waves them through
    /// to `insert`, where the second silently overwrites the first — the
    /// replace-in-place this ticket deleted, back by the side door.
    #[test]
    fn alias_spellings_of_one_key_rejected() {
        // The last pair collides in its *second* key, so only parsing the
        // whole sequence catches it.
        for (a, b) in [("G", "shift+g"), ("?", "question"), ("g ?", "g question")] {
            let src = format!(
                "[menu]\n{a:?} = {{ label = \"first\", run = \"a\" }}\n\
                 {b:?} = {{ label = \"second\", run = \"b\" }}\n"
            );
            let err = entries_of(&src).unwrap_err().to_string();
            assert!(err.contains("same key path"), "{a} vs {b}: {err}");
            assert!(err.contains(a) && err.contains(b), "{a} vs {b}: {err}");
        }
    }

    /// The other direction: `s` and `S` are two keys the menu can tell apart,
    /// and the collision check must not fold them together.
    #[test]
    fn distinct_keys_still_accepted() {
        let entries = entries_of(
            r#"[menu]
"g s" = { run = "lower" }
"g S" = { run = "upper" }
"#,
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        let tree = build_tree(&entries).unwrap();
        assert_eq!(tree[0].children().len(), 2, "s and S are different keys");
    }

    /// `= false` is gone with the overlay. It must not parse to a silent
    /// no-op in a file that is the whole menu.
    #[test]
    fn hide_false_rejected() {
        let err = entries_of(
            r#"[menu]
"g g" = false
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("expected an item table"), "{err}");
    }

    /// A whichkey.toml seeded before the split carries a live `[menu]`.
    #[test]
    fn menu_in_settings_file_rejected() {
        let dir = temp_dir("settings-menu");
        let config = dir.join("whichkey.toml");
        std::fs::write(&config, "[menu]\n\"d\" = { run = \"true\" }\n").unwrap();
        let err = load_settings(&config).unwrap_err().to_string();
        assert!(err.contains("[menu] moved out of whichkey.toml"), "{err}");
        assert!(err.contains("keys.toml"), "{err}");
        assert!(err.contains("herdr-whichkey init"), "{err}");
        // Config errors render in the strip, which clips each line to the pane
        // width and shows about three of them (ui::show_error, default height
        // 7 less chrome, header and footer). One long line would put the
        // actionable half off-screen; a fourth line would drop it.
        assert!(err.lines().count() <= 3, "{err}");
        assert!(err.lines().all(|l| l.chars().count() < 80 || l.contains(".toml")), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn keys_path_resolution() {
        let config = Path::new("/cfg/herdr-whichkey/whichkey.toml");
        assert_eq!(
            resolve_keys_path(config, None),
            Path::new("/cfg/herdr-whichkey/keys.toml"),
            "default: alongside whichkey.toml"
        );
        assert_eq!(
            resolve_keys_path(config, Some("menu/mine.toml")),
            Path::new("/cfg/herdr-whichkey/menu/mine.toml"),
            "relative: against the settings file's own directory"
        );
        assert_eq!(
            resolve_keys_path(config, Some("/dotfiles/keys.toml")),
            Path::new("/dotfiles/keys.toml"),
        );
        // Read HOME rather than setting it: env is process-global and the
        // test runner is threaded.
        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(
                resolve_keys_path(config, Some("~/dotfiles/keys.toml")),
                PathBuf::from(home).join("dotfiles/keys.toml"),
            );
        }
    }

    /// Sparse settings: an absent file, and the stub with everything still
    /// commented, must resolve identically — that is what "leaving it
    /// commented keeps tracking built-in defaults" means.
    #[test]
    fn settings_stub_parses_as_all_defaults() {
        let stub: FileConfig = toml::from_str(SETTINGS_STUB).unwrap();
        assert!(stub.keys_path.is_none());
        assert_eq!(stub.layout.height, None);
        assert_eq!(stub.layout.justify, None);
        assert!(stub.theme.name.is_none());
        assert_eq!(stub.ui.mouse, UiConfig::default().mouse);
    }

    /// …and uncommenting one line takes effect, with the rest still tracking.
    #[test]
    fn uncommenting_a_stub_knob_takes_effect() {
        let uncommented: String = SETTINGS_STUB
            .lines()
            .map(|l| l.strip_prefix("# height").map(|r| format!("height{r}")).unwrap_or(l.into()))
            .collect::<Vec<_>>()
            .join("\n");
        let cfg: FileConfig = toml::from_str(&uncommented).unwrap();
        assert_eq!(cfg.layout.height, Some(7));
        assert_eq!(cfg.layout.gutter, None);
    }

    #[test]
    fn init_writes_both_files_and_never_clobbers() {
        let dir = temp_dir("init");
        let config = dir.join("whichkey.toml");
        let keys = dir.join(KEYS_FILE);

        let wrote = init_at(&config, &keys, false, None).unwrap();
        assert!(matches!(wrote[0], Wrote::Created(_)) && matches!(wrote[1], Wrote::Created(_)));

        // The keys file is the shipped tree, uncommented and authoritative.
        let text = std::fs::read_to_string(&keys).unwrap();
        assert_eq!(text, DEFAULTS_TOML);
        let entries = entries_of(&text).unwrap();
        let paths = |e: &[(String, ItemSpec)]| e.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&entries), paths(&shipped_entries()));
        build_tree(&entries).unwrap();

        // The settings stub is the commented one, and still parses.
        let stub = std::fs::read_to_string(&config).unwrap();
        assert!(stub.contains("# keys_path"));
        toml::from_str::<FileConfig>(&stub).unwrap();

        // Second run leaves the user's edits alone.
        std::fs::write(&keys, "[menu]\n\"x\" = { run = \"mine\" }\n").unwrap();
        let wrote = init_at(&config, &keys, false, None).unwrap();
        assert!(matches!(wrote[0], Wrote::Kept(_)) && matches!(wrote[1], Wrote::Kept(_)));
        assert!(std::fs::read_to_string(&keys).unwrap().contains("mine"));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole pre-split upgrade, end to end: a v0.2.x config errors, the
    /// error says to run init, init runs, and the menu loads afterwards. The
    /// error used to name a command that reproduced the error.
    #[test]
    fn legacy_menu_migration_is_not_a_deadlock() {
        let dir = temp_dir("legacy");
        let config = dir.join("whichkey.toml");
        let keys = dir.join(KEYS_FILE);
        let legacy =
            "[layout]\nheight = 9\n\n[menu]\n\"d\" = { run = \"chezmoi edit\" }\n\"g g\" = false\n";
        std::fs::write(&config, legacy).unwrap();

        // 1. The pre-split config fails to load, pointing at init.
        let err = load_settings(&config).unwrap_err().to_string();
        assert!(err.contains("herdr-whichkey init"), "{err}");

        // 2. init runs anyway — this is the step that used to fail.
        let plan = init_plan(&config, legacy).unwrap();
        assert!(plan.legacy_menu);
        let wrote = init_at(&config, &keys, plan.legacy_menu, None).unwrap();
        assert!(matches!(wrote[1], Wrote::MovedAside { .. }), "the old config moves aside");

        // 3. Nothing is lost: the old file is intact under its new name.
        let aside = dir.join(format!("whichkey.toml{LEGACY_SUFFIX}"));
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), legacy);

        // 4. …and the menu loads: shipped tree, stub settings.
        let settings = load_settings(&config).unwrap();
        assert_eq!(settings.layout.height, None, "the stub tracks defaults again");
        let entries = entries_of(&std::fs::read_to_string(&keys).unwrap()).unwrap();
        assert!(build_tree(&entries).unwrap().iter().any(|n| n.key == 'g'));

        // A later re-run must not bury that backup.
        std::fs::write(&config, legacy).unwrap();
        let err = init_at(&config, &keys, true, None).unwrap_err().to_string();
        assert!(err.contains("already exists"), "{err}");
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), legacy);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only the `[menu]` table is ours to migrate. Settings we could not parse
    /// must not be swept into the backup and replaced by the stub — that is a
    /// silent edit to knobs the user is still editing.
    #[test]
    fn broken_settings_beside_a_legacy_menu_are_not_migrated() {
        let config = Path::new("/cfg/whichkey.toml");
        let broken = "[layout]\nheight = \"invalid\"\n\n[menu]\n\"d\" = { run = \"x\" }\n";

        let err = init_plan(config, broken).unwrap_err();
        let chain = format!("{err:#}");
        assert!(chain.contains("do not\nparse"), "{chain}");
        // toml's own diagnostic survives, line and column included.
        assert!(chain.contains("line 2"), "{chain}");

        // Broken *without* a [menu]: nothing to migrate, so init leaves the
        // file alone rather than refusing to run at all.
        let plan = init_plan(config, "[layout]\nheight = \"invalid\"\n").unwrap();
        assert!(!plan.legacy_menu);
        assert!(plan.declared.is_none());

        // Not TOML at all — same.
        assert!(init_plan(config, "this is not toml {{{").is_ok());
    }

    /// The migration writes the menu to a declared `keys_path`, so the stub it
    /// leaves behind has to keep pointing there. A commented-out default would
    /// send the next load to keys.toml and orphan the file just written.
    #[test]
    fn migration_keeps_a_declared_keys_path_live() {
        let dir = temp_dir("keys-path-carry");
        let config = dir.join("whichkey.toml");
        let elsewhere = dir.join("mine.toml");
        let legacy = "keys_path = \"mine.toml\"\n\n[menu]\n\"d\" = { run = \"x\" }\n";
        std::fs::write(&config, legacy).unwrap();

        let plan = init_plan(&config, legacy).unwrap();
        assert_eq!(plan.declared.as_deref(), Some("mine.toml"));
        let keys_path = resolve_keys_path(&config, plan.declared.as_deref());
        assert_eq!(keys_path, elsewhere);
        init_at(&config, &keys_path, plan.legacy_menu, plan.declared.as_deref()).unwrap();

        // The menu went to the declared path…
        assert!(elsewhere.exists());
        assert!(!dir.join(KEYS_FILE).exists());
        // …and the regenerated settings still resolve to it.
        let settings = load_settings(&config).unwrap();
        assert_eq!(settings.keys_path.as_deref(), Some("mine.toml"));
        assert_eq!(resolve_keys_path(&config, settings.keys_path.as_deref()), elsewhere);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A path needing TOML escaping has to survive the round trip verbatim —
    /// it is the user's string, not ours to re-spell.
    #[test]
    fn carried_keys_path_is_escaped() {
        for raw in ["~/dot files/keys.toml", "od\"d\\keys.toml", "$XDG_CONFIG_HOME/k.toml"] {
            let stub = settings_stub(Some(raw));
            let parsed: FileConfig = toml::from_str(&stub).unwrap();
            assert_eq!(parsed.keys_path.as_deref(), Some(raw));
        }
        // No declaration: the stub keeps its commented default.
        let plain: FileConfig = toml::from_str(&settings_stub(None)).unwrap();
        assert!(plain.keys_path.is_none());
        assert_eq!(settings_stub(None), SETTINGS_STUB);
    }

    /// `write_new` must be all-or-nothing at the destination: a write that dies
    /// partway cannot leave a truncated file for the next run to adopt as the
    /// user's. Approximated here by checking the destination is never the temp
    /// it was built in, and that a failure leaves no scratch behind.
    #[test]
    fn write_new_publishes_atomically() {
        let dir = temp_dir("atomic");
        let path = dir.join("keys.toml");

        assert!(matches!(write_new(&path, DEFAULTS_TOML).unwrap(), Wrote::Created(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULTS_TOML);

        // A second write finds the file there and neither truncates nor
        // rewrites it — and cleans up after itself.
        assert!(matches!(write_new(&path, "clobbered").unwrap(), Wrote::Kept(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULTS_TOML);

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Only a `[menu]` *table* means pre-split. `menu = false` beside settings
    /// that parse is a typo in a perfectly modern file — migrating it would
    /// move a config aside over a menu that was never there.
    #[test]
    fn a_non_table_menu_is_not_a_legacy_config() {
        let config = Path::new("/cfg/whichkey.toml");
        for text in ["menu = false\n\n[layout]\nheight = 9\n", "menu = \"keys.toml\"\n"] {
            assert!(!has_legacy_menu(text), "{text}");
            let plan = init_plan(config, text).unwrap();
            assert!(!plan.legacy_menu, "{text}");
            // …and the loader rejects it on the ordinary invalid-value path,
            // spans included, rather than sending the user to `init`.
            let err = format!("{:#}", toml::from_str::<FileConfig>(text).unwrap_err());
            assert!(err.contains("invalid type"), "{err}");
        }

        // The old seed wrote `[menu]` with every entry commented out, so an
        // empty table still has to migrate.
        let empty = "[layout]\nheight = 9\n\n[menu]\n";
        assert!(has_legacy_menu(empty));
        assert!(init_plan(config, empty).unwrap().legacy_menu);
    }

    /// A run killed between the link and the cleanup leaves a scratch file that
    /// is still the *same inode* as the published one. Reopening that name
    /// truncating would rewrite the user's file through the alias — before the
    /// no-clobber link ever gets a say. The scratch file has to be created
    /// exclusively, so a leftover is stepped over rather than opened.
    #[test]
    fn a_stale_temp_is_never_reopened_in_place() {
        let dir = temp_dir("stale-temp");
        let path = dir.join("keys.toml");
        std::fs::write(&path, "the user's own menu").unwrap();
        let stale = temp_candidate(&path, 0);
        std::fs::hard_link(&path, &stale).unwrap();

        assert!(matches!(write_new(&path, DEFAULTS_TOML).unwrap(), Wrote::Kept(_)));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "the user's own menu",
            "the published file was rewritten through a leftover scratch name"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whichkey-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn implicit_groups_and_order() {
        let tree = tree_of(
            r#"[menu]
"g s" = { label = "status", run = "git status" }
"g"   = { label = "git" }
"#,
        )
        .unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].label, "git");
        assert_eq!(tree[0].children().len(), 1);
    }

    #[test]
    fn leaf_with_children_rejected() {
        let err = tree_of(
            r#"[menu]
"g"   = { label = "git", run = "lazygit" }
"g s" = { label = "status", run = "git status" }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("already an action item"), "{err}");
    }

    #[test]
    fn group_with_action_rejected() {
        let err = tree_of(
            r#"[menu]
"g s" = { label = "status", run = "git status" }
"g"   = { label = "git", run = "lazygit" }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("move the action to a child key"), "{err}");
    }

    #[test]
    fn stick_on_group_rejected() {
        let err = tree_of(
            r#"[menu]
"r"   = { label = "resize", stick = true }
"r h" = { label = "left", herdr = "pane resize --direction left", stick = true }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"r\": stick = true"), "{err}");
    }

    /// Same rejection when the group node already exists — this ordering
    /// takes the relabel branch in `insert`, which used to drop stick.
    #[test]
    fn stick_on_existing_group_rejected() {
        let err = tree_of(
            r#"[menu]
"r h" = { label = "left", herdr = "pane resize --direction left", stick = true }
"r"   = { label = "resize", stick = true }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("\"r\": stick = true"), "{err}");
    }

    #[test]
    fn stick_on_leaf_accepted() {
        let tree = tree_of(
            r#"[menu]
"r h" = { label = "left", herdr = "pane resize --direction left", stick = true }
"#,
        )
        .unwrap();
        assert!(tree[0].children()[0].stick);
        assert!(!tree[0].stick); // the implicit parent group stays unstuck
    }

    #[test]
    fn multiple_leaf_kinds_rejected() {
        let err = tree_of(
            r#"[menu]
"x" = { run = "ls", herdr = "pane list" }
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("pick one"), "{err}");
    }
}
