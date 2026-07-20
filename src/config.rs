//! Config loading: built-in defaults overlaid with the user's whichkey.toml,
//! flattened key-paths built into the menu tree, first-run seeding.

use std::path::PathBuf;

use anyhow::{bail, Context as _, Result};
use serde::Deserialize;

use crate::keys;
use crate::model::{fallback_label, leaf_from_spec, Entry, ItemSpec, Node, NodeKind};
use crate::theme::ThemeOverrides;

pub const DEFAULTS_TOML: &str = include_str!("defaults.toml");

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    menu: toml::map::Map<String, toml::Value>,
    #[serde(default)]
    theme: ThemeOverrides,
}

pub struct Config {
    /// Merged flat entries in menu order (defaults first, user additions after).
    pub entries: Vec<(String, ItemSpec)>,
    pub theme: ThemeOverrides,
}

/// The user's config file path: plugin config dir when running under herdr,
/// the same well-known location otherwise (so `herdr-whichkey defaults` works
/// from a plain shell).
pub fn user_config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return PathBuf::from(dir).join("whichkey.toml");
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("herdr/plugins/config/herdr-whichkey/whichkey.toml")
}

/// `seed_if_missing`: the menu path seeds a commented starter config on
/// first run; read-only paths like `defaults` must not write anything.
pub fn load(seed_if_missing: bool) -> Result<Config> {
    let defaults: FileConfig = toml::from_str(DEFAULTS_TOML).expect("built-in defaults must parse");

    let path = user_config_path();
    let user: FileConfig = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text)
            .with_context(|| format!("could not parse {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if seed_if_missing {
                seed(&path).ok(); // best-effort; a read-only dir shouldn't break the menu
            }
            FileConfig::default()
        }
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };

    // Overlay, order-sensitive: same path replaces in place, `= false` removes
    // the entry and its whole subtree, new paths append.
    let mut entries: Vec<(String, ItemSpec)> = Vec::new();
    for (raw, value) in defaults.menu.iter().chain(user.menu.iter()) {
        let entry: Entry = value
            .clone()
            .try_into()
            .with_context(|| format!("\"{raw}\": not an item table or `false`"))?;
        let path = normalize_path(raw);
        match entry {
            Entry::Hide(true) => {
                bail!("\"{raw}\" = true has no meaning — use false to hide, or a {{ … }} table")
            }
            Entry::Hide(false) => {
                let prefix = format!("{path} ");
                entries.retain(|(p, _)| *p != path && !p.starts_with(&prefix));
            }
            Entry::Spec(spec) => match entries.iter_mut().find(|(p, _)| *p == path) {
                Some((_, slot)) => *slot = spec,
                None => entries.push((path, spec)),
            },
        }
    }

    Ok(Config { entries, theme: user.theme })
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

/// First-run seed: a short header plus every default as a commented line the
/// user can uncomment to override — discoverability without forking defaults.
fn seed(path: &std::path::Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = String::from(
        "# herdr-whichkey — your menu overlay.\n\
         #\n\
         # Entries here overlay the built-in defaults: same path replaces,\n\
         # `\"g g\" = false` hides, new paths add. Keys use herdr's format\n\
         # (\"g s\" = press g then s). Run `herdr-whichkey defaults` for the\n\
         # live resolved tree. Placeholders: {pane} {tab} {workspace} {cwd} {stamp}.\n\
         \n\
         [menu]\n\
         # \"d\"   = { label = \"dotfiles\", run = \"chezmoi edit\", in = \"pane\" }\n\
         \n\
         # ── shipped defaults (uncomment a line to change it) ──────────────\n",
    );
    for line in DEFAULTS_TOML.lines() {
        if line.starts_with('"') {
            out.push_str("# ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("# ── end shipped defaults ──────────────────────────────────────────\n");
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_of(toml_src: &str) -> Result<Vec<Node>> {
        let f: FileConfig = toml::from_str(toml_src).unwrap();
        let entries: Vec<(String, ItemSpec)> = f
            .menu
            .into_iter()
            .filter_map(|(k, v)| match v.try_into::<Entry>().unwrap() {
                Entry::Hide(_) => None,
                Entry::Spec(s) => Some((k, s)),
            })
            .collect();
        build_tree(&entries)
    }

    #[test]
    fn defaults_parse_and_build() {
        let f: FileConfig = toml::from_str(DEFAULTS_TOML).unwrap();
        let entries: Vec<(String, ItemSpec)> = f
            .menu
            .into_iter()
            .map(|(k, v)| (k, v.try_into::<ItemSpec>().unwrap()))
            .collect();
        let tree = build_tree(&entries).unwrap();
        assert!(tree.iter().any(|n| n.key == 'g'));
        let resize = tree.iter().find(|n| n.key == 'r').unwrap();
        assert!(resize.children().iter().all(|c| c.stick));
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
