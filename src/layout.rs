//! Item placement: the strip width picks the column count (policy CSS
//! doesn't have — it wraps greedily, we divide evenly), then taffy, the
//! CSS grid engine, positions everything, steered by the `[layout]`
//! knobs in whichkey.toml.

use anyhow::Result;
use serde::Deserialize;
use taffy::prelude::*;

/// `[layout]` section of whichkey.toml — CSS names, CSS meanings.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LayoutConfig {
    /// Where the menu lives. Default bottom (the footer strip).
    #[serde(default)]
    pub placement: Placement,
    /// Horizontal distribution of the columns. Default per placement.
    pub justify: Option<Distribute>,
    /// Vertical distribution of the rows. Default per placement.
    pub align: Option<Distribute>,
    /// Size along the vertical axis, in rows — see [`Placement`] for what
    /// that means per placement. Default [`Placement::height`].
    pub height: Option<u16>,
    /// Size along the horizontal axis, in columns. Default
    /// [`Placement::width`].
    pub width: Option<u16>,
    /// Pin the column count; omit (or 0) to fit the width.
    pub columns: Option<usize>,
    /// Cells between columns: the spacing floor when fitting columns,
    /// the actual gap under start/center/end justify. Default half an
    /// item, at least 4.
    pub gutter: Option<usize>,
}

/// Where the menu surface sits. herdr 0.7.5's `plugin pane open --direction`
/// still takes only `down` and `right` (`up`/`left` are rejected outright),
/// so top and left are not expressible without a split-then-move that
/// reflows the focused pane twice — see docs/spike-popup-panes.md.
#[derive(Debug, Default, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Placement {
    /// Today's surface: a full-width strip split below the focused pane.
    #[default]
    Bottom,
    /// A tall narrow column split to the right of the focused pane.
    Right,
    /// A centered float. Never reflows the focused pane; herdr floats
    /// popups centered only, so there is no docked variant of this.
    Popup,
}

impl Placement {
    /// The distribution defaults this surface reads well at. The knobs
    /// override; without them, space-evenly/space-around — tuned for a
    /// 4×2 footer — would scatter eight items down a 40-row column.
    fn distribution(self) -> (Distribute, Distribute) {
        match self {
            // Wide and shallow: columns spread across, a blank row above each.
            Placement::Bottom => (Distribute::SpaceEvenly, Distribute::SpaceAround),
            // Tall and narrow: a list, packed at the top-left.
            Placement::Right => (Distribute::Start, Distribute::Start),
            // A boxed grid, centered in its float like which-key.nvim's.
            Placement::Popup => (Distribute::Center, Distribute::Center),
        }
    }

    /// Default `[layout] height`, in this placement's meaning of it.
    pub fn height(self) -> u16 {
        match self {
            Placement::Bottom => 7,
            // Unused: a right split sizes itself on the other axis.
            Placement::Right => 0,
            Placement::Popup => 10,
        }
    }

    /// Default `[layout] width`, in this placement's meaning of it.
    pub fn width(self) -> u16 {
        match self {
            // Unused: a bottom strip is as wide as the pane it splits.
            Placement::Bottom => 0,
            Placement::Right => 32,
            Placement::Popup => 60,
        }
    }

    /// The surface string the launcher tags `WHICHKEY_SURFACE` with, and
    /// what `herdr-whichkey surface` prints back to it.
    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Bottom => "bottom",
            Placement::Right => "right",
            Placement::Popup => "popup",
        }
    }

    /// Parse a `WHICHKEY_SURFACE` value; `None` for anything unknown, so a
    /// surface we don't recognise falls back to the config rather than
    /// silently rendering as the wrong one.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bottom" => Some(Placement::Bottom),
            "right" => Some(Placement::Right),
            "popup" => Some(Placement::Popup),
            _ => None,
        }
    }

    /// Rows of the body this surface spends on chrome above the items.
    /// A split pane carries the breadcrumb in its border title (herdr
    /// `pane rename`), so it spends none; a popup has no pane id to
    /// rename and its border title is the manifest's, so it draws the
    /// breadcrumb itself.
    pub fn header_rows(self) -> u16 {
        match self {
            Placement::Bottom | Placement::Right => 0,
            Placement::Popup => 1,
        }
    }
}

/// The CSS content-distribution keywords we expose.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Distribute {
    Start,
    Center,
    End,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

impl Distribute {
    fn content(self) -> AlignContent {
        match self {
            Distribute::Start => AlignContent::START,
            Distribute::Center => AlignContent::CENTER,
            Distribute::End => AlignContent::END,
            Distribute::SpaceBetween => AlignContent::SPACE_BETWEEN,
            Distribute::SpaceAround => AlignContent::SPACE_AROUND,
            Distribute::SpaceEvenly => AlignContent::SPACE_EVENLY,
        }
    }

    /// The space-* modes make their own gaps; the fixed modes need the
    /// gutter as a real gap or adjacent columns would touch.
    fn wants_gutter_gap(self) -> bool {
        matches!(self, Distribute::Start | Distribute::Center | Distribute::End)
    }
}

/// Item-area cell (x, y) for each item, column-major. Items may land
/// past the right or bottom edge when nothing fits — callers clip.
pub fn positions(
    n: usize,
    item_width: usize,
    area_cols: usize,
    area_rows: usize,
    cfg: &LayoutConfig,
) -> Result<Vec<(usize, usize)>> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let (def_justify, def_align) = cfg.placement.distribution();
    let justify = cfg.justify.unwrap_or(def_justify);
    let align = cfg.align.unwrap_or(def_align);
    let gutter = cfg.gutter.unwrap_or((item_width / 2).max(4));
    let (ncols, nrows) = match cfg.columns.filter(|&c| c > 0) {
        Some(c) => {
            let c = c.min(n);
            (c, n.div_ceil(c))
        }
        None => balance(n, area_rows, item_width, area_cols, gutter),
    };

    let mut tree: TaffyTree<()> = TaffyTree::new();
    let children =
        (0..n).map(|_| tree.new_leaf(Style::default())).collect::<Result<Vec<_>, _>>()?;
    let gap = if justify.wants_gutter_gap() { gutter } else { 0 };
    let root = tree.new_with_children(
        Style {
            display: Display::Grid,
            size: Size { width: length(area_cols as f32), height: length(area_rows as f32) },
            grid_template_columns: vec![length(item_width as f32); ncols],
            grid_template_rows: vec![length(1.0_f32); nrows],
            grid_auto_flow: GridAutoFlow::Column,
            gap: Size { width: length(gap as f32), height: length(0.0_f32) },
            justify_content: Some(justify.content()),
            align_content: Some(align.content()),
            ..Default::default()
        },
        &children,
    )?;
    tree.compute_layout(root, Size::MAX_CONTENT)?;
    children
        .iter()
        .map(|&c| {
            let l = tree.layout(c)?;
            Ok((l.location.x.max(0.0) as usize, l.location.y.max(0.0) as usize))
        })
        .collect()
}

/// (columns, rows per column): as many columns as the width fits with
/// the gutter, wide and shallow rather than tall and narrow — 8 items
/// in a wide strip become 4×2, not 2×4 — then items divided evenly so
/// no column trails nearly empty. `max_rows` still wins when the strip
/// is too narrow for the height-minimal column count.
fn balance(
    n: usize,
    max_rows: usize,
    item_width: usize,
    width: usize,
    gutter: usize,
) -> (usize, usize) {
    if n == 0 {
        return (1, 0);
    }
    let fit = (width.saturating_sub(gutter) / (item_width + gutter).max(1)).max(1);
    let ncols = fit.max(n.div_ceil(max_rows.max(1))).min(n);
    let rows = n.div_ceil(ncols);
    (n.div_ceil(rows), rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_flows_wide_and_shallow() {
        // Wide strip: width decides the columns, footer-style.
        assert_eq!(balance(8, 6, 18, 179, 9), (4, 2)); // not 2 columns of 4
        assert_eq!(balance(5, 6, 15, 179, 7), (5, 1)); // small level: one line
        assert_eq!(balance(9, 6, 18, 179, 9), (5, 2)); // not 6 cols with one empty
                                                       // Narrow strip: falls back toward tall columns.
        assert_eq!(balance(8, 6, 18, 78, 9), (2, 4));
        assert_eq!(balance(8, 6, 18, 20, 9), (2, 4)); // too narrow: height-minimal
        assert_eq!(balance(0, 6, 10, 80, 5), (1, 0)); // empty level draws nothing
    }

    #[test]
    fn default_positions_footer_grid() {
        let cfg = LayoutConfig::default();
        // 8 root items at the live strip: 4×2 with columns spaced evenly
        // across, a blank over each row (space-around). The default 7-row
        // split leaves 5 content rows; only the footer is carved off, so
        // the item area is 4 tall.
        let pos = positions(8, 18, 179, 4, &cfg).unwrap();
        assert_eq!(pos[0], (21, 1));
        assert_eq!(pos[1], (21, 3)); // column-major: second item under first
        assert_eq!(pos[2], (61, 1));
        assert_eq!(pos[7], (140, 3));
    }

    /// The whole point of the per-placement defaults: the same eight items,
    /// the same `positions()`, laid out for three surfaces. Only the
    /// distribution defaults differ — `balance()` already picks the column
    /// count from the area it is given.
    #[test]
    fn each_placement_lays_the_same_items_out_for_its_surface() {
        let at = |p: Placement, cols, rows| {
            let cfg = LayoutConfig { placement: p, ..Default::default() };
            positions(8, 18, cols, rows, &cfg).unwrap()
        };

        // Bottom: unchanged from today — 4×2 across a 179-wide strip, a
        // blank row above each row of items.
        let bottom = at(Placement::Bottom, 179, 4);
        assert_eq!((bottom[0], bottom[1], bottom[7]), ((21, 1), (21, 3), (140, 3)));

        // Right: one 30-wide column, so a single list of 8 — packed at the
        // top-left of the tall area rather than spread down all 20 rows.
        let right = at(Placement::Right, 30, 20);
        assert_eq!(right, (0..8).map(|y| (0, y)).collect::<Vec<_>>());

        // Popup: a boxed grid centered in the float's body. 58×6 (a 60×10
        // popup less herdr's border and our breadcrumb and footer rows)
        // takes 2 columns of 4, centered on both axes.
        let popup = at(Placement::Popup, 58, 6);
        assert_eq!((popup[0], popup[3]), ((7, 1), (7, 4))); // first column, 4 tall
        assert_eq!((popup[4], popup[7]), ((34, 1), (34, 4))); // second, one gutter over
                                                              // Centered means centered: the margins match on both axes, give or
                                                              // take the odd cell an odd remainder leaves over.
        assert_eq!(popup[0].0.abs_diff(58 - (popup[4].0 + 18)), 1); // 7 left, 6 right
        assert_eq!(popup[0].1, 6 - (popup[3].1 + 1)); // 1 row either side
    }

    /// A knob the user set beats the placement's default, on either axis.
    #[test]
    fn placement_defaults_yield_to_the_knobs() {
        let cfg: LayoutConfig =
            toml::from_str("placement = \"popup\"\njustify = \"start\"\n").unwrap();
        assert_eq!(cfg.placement, Placement::Popup);
        let pos = positions(2, 10, 100, 4, &cfg).unwrap();
        assert_eq!(pos[0].0, 0); // start, not the popup's centered default
        assert_eq!(pos[0].1, 2); // align still the popup's own center
    }

    #[test]
    fn placement_parses_and_round_trips() {
        let of = |s: &str| {
            toml::from_str::<LayoutConfig>(&format!("placement = \"{s}\"\n")).map(|c| c.placement)
        };
        for p in [Placement::Bottom, Placement::Right, Placement::Popup] {
            assert_eq!(of(p.as_str()).unwrap(), p);
            assert_eq!(Placement::parse(p.as_str()), Some(p));
        }
        assert_eq!(LayoutConfig::default().placement, Placement::Bottom);
        // top/left are not expressible on herdr 0.7.5 (docs/spike-popup-panes.md)
        // — rejected at parse time rather than quietly treated as bottom.
        assert!(of("top").is_err());
        assert!(of("left").is_err());
        assert_eq!(Placement::parse("split"), None);
    }

    #[test]
    fn knobs_pin_columns_and_distribution() {
        let cfg: LayoutConfig = toml::from_str(
            "justify = \"space-between\"\nalign = \"start\"\ncolumns = 2\nheight = 6\n",
        )
        .unwrap();
        assert_eq!(cfg.height, Some(6)); // consumed by the split fit, not positions()
        let pos = positions(4, 10, 100, 6, &cfg).unwrap();
        // Two pinned columns pushed to the edges, rows packed at the top.
        assert_eq!(pos[0], (0, 0));
        assert_eq!(pos[1], (0, 1));
        assert_eq!(pos[2], (90, 0));
        assert_eq!(pos[3], (90, 1));
    }

    #[test]
    fn fixed_justify_keeps_a_gutter() {
        let cfg: LayoutConfig = toml::from_str("justify = \"center\"\ngutter = 6\n").unwrap();
        let pos = positions(2, 10, 100, 6, &cfg).unwrap();
        // Two 10-wide columns + 6 gutter centered in 100: starts at 37.
        assert_eq!(pos[0].0, 37);
        assert_eq!(pos[1].0, 53);
    }

    #[test]
    fn unknown_keyword_rejected() {
        assert!(toml::from_str::<LayoutConfig>("justify = \"middle\"\n").is_err());
        assert!(toml::from_str::<LayoutConfig>("wobble = 3\n").is_err());
    }
}
