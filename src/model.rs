//! Menu tree model: item specs as written in TOML, and the resolved tree.

use anyhow::{bail, Result};
use serde::Deserialize;

/// Where a `run` command executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunIn {
    #[default]
    Background,
    Pane,
    Tab,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemSpec {
    pub label: Option<String>,
    /// herdr CLI call (no shell), e.g. "pane split {pane} --direction right".
    pub herdr: Option<String>,
    /// Qualified plugin action id, e.g. "worktrunk.open".
    pub action: Option<String>,
    /// Shell command, run via `sh -c`.
    pub run: Option<String>,
    #[serde(rename = "in", default)]
    pub run_in: Option<RunIn>,
    pub cwd: Option<String>,
    /// Keep the menu open after this action fires (hydra-style repeat).
    #[serde(default)]
    pub stick: bool,
    /// Binary that must exist on PATH for this item to show.
    pub requires: Option<String>,
}

impl ItemSpec {
    pub fn leaf_kind_count(&self) -> usize {
        [self.herdr.is_some(), self.action.is_some(), self.run.is_some()]
            .iter()
            .filter(|b| **b)
            .count()
    }

    pub fn is_group_only(&self) -> bool {
        self.leaf_kind_count() == 0
    }
}

#[derive(Debug, Clone)]
pub enum Leaf {
    Herdr(String),
    Action(String),
    Run { cmd: String, run_in: RunIn, cwd: Option<String> },
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Group(Vec<Node>),
    Leaf(Leaf),
}

#[derive(Debug, Clone)]
pub struct Node {
    pub key: char,
    pub label: String,
    pub stick: bool,
    pub requires: Option<String>,
    /// The plugin id an `action` leaf depends on (auto-derived).
    pub requires_plugin: Option<String>,
    pub kind: NodeKind,
}

impl Node {
    pub fn children(&self) -> &[Node] {
        match &self.kind {
            NodeKind::Group(c) => c,
            NodeKind::Leaf(_) => &[],
        }
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, NodeKind::Group(_))
    }
}

/// Why a node would be hidden by adaptive detection (for `defaults` output).
pub fn unavailable_reason(
    node: &Node,
    have_bin: &dyn Fn(&str) -> bool,
    have_plugin: &dyn Fn(&str) -> bool,
) -> Option<String> {
    if let Some(bin) = &node.requires {
        if !have_bin(bin) {
            return Some(format!("needs `{bin}` on PATH"));
        }
    }
    if let Some(plugin) = &node.requires_plugin {
        if !have_plugin(plugin) {
            return Some(format!("needs plugin `{plugin}`"));
        }
    }
    None
}

/// Adaptive auto-hide: drop items whose required binary or plugin is
/// missing, then drop groups that end up empty.
pub fn prune_unavailable(
    nodes: Vec<Node>,
    have_bin: &dyn Fn(&str) -> bool,
    have_plugin: &dyn Fn(&str) -> bool,
) -> Vec<Node> {
    nodes
        .into_iter()
        .filter_map(|mut n| {
            if unavailable_reason(&n, have_bin, have_plugin).is_some() {
                return None;
            }
            if let NodeKind::Group(children) = n.kind {
                let kept = prune_unavailable(children, have_bin, have_plugin);
                if kept.is_empty() {
                    return None;
                }
                n.kind = NodeKind::Group(kept);
            }
            Some(n)
        })
        .collect()
}

/// Build a leaf from a validated spec (spec must have exactly one leaf kind).
pub fn leaf_from_spec(path: &str, spec: &ItemSpec) -> Result<Leaf> {
    match spec.leaf_kind_count() {
        0 => bail!("\"{path}\": no action — set one of herdr=, action=, run= (or add children)"),
        1 => {}
        _ => bail!("\"{path}\": more than one of herdr=, action=, run= — pick one"),
    }
    if let Some(h) = &spec.herdr {
        return Ok(Leaf::Herdr(h.clone()));
    }
    if let Some(a) = &spec.action {
        if !a.contains('.') {
            bail!("\"{path}\": action = \"{a}\" — expected a qualified id like \"plugin.action\"");
        }
        return Ok(Leaf::Action(a.clone()));
    }
    Ok(Leaf::Run {
        cmd: spec.run.clone().unwrap(),
        run_in: spec.run_in.unwrap_or_default(),
        cwd: spec.cwd.clone(),
    })
}

/// Default label for an unlabeled item: its command, trimmed.
pub fn fallback_label(spec: &ItemSpec) -> String {
    spec.label
        .clone()
        .or_else(|| spec.herdr.clone())
        .or_else(|| spec.action.clone())
        .or_else(|| spec.run.clone())
        .unwrap_or_else(|| "…".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(key: char, requires: Option<&str>, plugin: Option<&str>) -> Node {
        Node {
            key,
            label: key.to_string(),
            stick: false,
            requires: requires.map(Into::into),
            requires_plugin: plugin.map(Into::into),
            kind: NodeKind::Leaf(Leaf::Run {
                cmd: "true".into(),
                run_in: RunIn::Background,
                cwd: None,
            }),
        }
    }

    #[test]
    fn prune_drops_missing_and_empty_groups() {
        let tree = vec![Node {
            key: 'g',
            label: "git".into(),
            stick: false,
            requires: None,
            requires_plugin: None,
            kind: NodeKind::Group(vec![
                leaf('a', Some("present"), None),
                leaf('b', Some("missing"), None),
                leaf('c', None, Some("gone-plugin")),
            ]),
        }];
        let have_bin = |b: &str| b == "present";

        let kept = prune_unavailable(tree.clone(), &have_bin, &|_| true);
        assert_eq!(kept[0].children().len(), 2); // 'b' dropped, 'c' kept

        let kept = prune_unavailable(tree, &|_| false, &|_| false);
        assert!(kept.is_empty()); // all children gone → group gone
    }
}
