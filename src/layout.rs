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
    /// Horizontal distribution of the columns. Default space-evenly.
    pub justify: Option<Distribute>,
    /// Vertical distribution of the rows. Default space-around: at the
    /// default height the root's two rows get a blank above each, the
    /// second sitting on the footer.
    pub align: Option<Distribute>,
    /// Strip height the split fits itself to, in split rows — herdr's
    /// pane chrome takes ~2 of them, the rest is menu. Default 7.
    pub height: Option<u16>,
    /// Pin the column count; omit (or 0) to fit the width.
    pub columns: Option<usize>,
    /// Cells between columns: the spacing floor when fitting columns,
    /// the actual gap under start/center/end justify. Default half an
    /// item, at least 4.
    pub gutter: Option<usize>,
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
    let justify = cfg.justify.unwrap_or(Distribute::SpaceEvenly);
    let align = cfg.align.unwrap_or(Distribute::SpaceAround);
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
