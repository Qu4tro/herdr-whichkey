//! The bottom-strip menu: flicker-free rendering (one synchronized frame
//! per keystroke, no full-screen clears between frames) and the
//! single-keystroke loop. We own the whole plugin pane surface.

use std::io::Write as _;

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, style, terminal};
use serde::Deserialize;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::context::HerdrContext;
use crate::dispatch;
use crate::keys::display_key;
use crate::layout::{self, LayoutConfig};
use crate::model::{Node, NodeKind};
use crate::theme::Palette;

/// `[ui]` section of whichkey.toml.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Click items to fire them. On by default: herdr forwards mouse into
    /// the pane whatever its own `ui.mouse_capture` is set to (see
    /// docs/spike-mouse.md). Turn it off to keep herdr's drag-to-select
    /// working over the strip's rows.
    #[serde(default = "yes")]
    pub mouse: bool,
}

fn yes() -> bool {
    true
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { mouse: true }
    }
}

/// How the menu ended.
pub enum Outcome {
    Closed,
    /// A non-stick leaf was chosen: run it after the pane is gone.
    /// Carries the item label for failure reporting.
    Deferred(crate::model::Leaf, String),
}

/// Transient message on the footer line.
enum Notice {
    None,
    Info(String),
    Error(String),
}

/// Raw-mode/cursor/wrap/mouse guard so every exit path restores the terminal.
struct TermGuard {
    mouse: bool,
}

impl TermGuard {
    fn enter(mouse: bool) -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(std::io::stdout(), cursor::Hide, terminal::DisableLineWrap)?;
        if mouse {
            execute!(std::io::stdout(), EnableMouseCapture)?;
        }
        Ok(Self { mouse })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        if self.mouse {
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
        }
        let _ = execute!(
            std::io::stdout(),
            SetAttribute(Attribute::Reset),
            style::ResetColor,
            terminal::EnableLineWrap,
            cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

pub fn run(
    tree: &[Node],
    pal: &Palette,
    lay: &LayoutConfig,
    uic: &UiConfig,
    ctx: &HerdrContext,
) -> Result<Outcome> {
    let _guard = TermGuard::enter(uic.mouse)?;
    let mut stack: Vec<(&[Node], String)> = vec![(tree, "whichkey".into())];
    let mut notice = Notice::None;
    // The breadcrumb lives in the pane border title, not the body —
    // starts as "whichkey" from the manifest, follows the group path.
    let mut title = String::from("whichkey");
    let mut hover: Option<usize> = None;
    // Any-motion mouse reporting means events we ignore arrive in bulk;
    // repaint only when something actually changed.
    let mut dirty = true;
    // Geometry of the frame on screen. Clicks resolve against this, never
    // against a fresh terminal::size(): a resize and a click can both be
    // pending, and hit-testing the click against a layout that was never
    // drawn fires whatever the user didn't aim at. Resize sets dirty, so
    // the next turn re-renders and refreshes this before the next event.
    let mut frame = (0u16, 0u16);

    loop {
        let (level, _) = *stack.last().expect("stack never empty");
        if dirty {
            let want = std::iter::once("whichkey")
                .chain(stack.iter().skip(1).map(|(_, l)| l.as_str()))
                .collect::<Vec<_>>()
                .join(" › ");
            if want != title {
                dispatch::set_pane_title(&want);
                title = want;
            }
            frame = render(pal, lay, &stack, level, &notice, hover)?;
            dirty = false;
        }

        match event::read()? {
            Event::Resize(_, _) => dirty = true,
            Event::Key(KeyEvent { code, modifiers, kind, .. })
                if kind == KeyEventKind::Press || kind == KeyEventKind::Repeat =>
            {
                dirty = true;
                match code {
                    // Esc walks back up and only closes from the root —
                    // ctrl+c (and the launcher's toggle) always closes.
                    KeyCode::Esc => {
                        if stack.len() > 1 {
                            ascend(&mut stack, &mut notice, &mut hover);
                        } else {
                            return Ok(Outcome::Closed);
                        }
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(Outcome::Closed)
                    }
                    KeyCode::Backspace => ascend(&mut stack, &mut notice, &mut hover),
                    KeyCode::Char(c) => match level.iter().find(|n| n.key == c) {
                        Some(node) => {
                            if let Some(out) =
                                choose(node, &mut stack, &mut notice, &mut hover, ctx)
                            {
                                return Ok(out);
                            }
                        }
                        None => notice = Notice::Info(format!("no binding: {}", display_key(c))),
                    },
                    _ => {}
                }
            }
            Event::Mouse(MouseEvent { kind, column, row, .. }) => {
                let (cols, rows) = frame;
                match kind {
                    // Act on the press: the release lands right behind it
                    // and must not fire a sticky item a second time.
                    MouseEventKind::Down(MouseButton::Left) => {
                        dirty = true;
                        match hit(level, lay, cols, rows, column, row)? {
                            Some(i) => {
                                if let Some(out) =
                                    choose(&level[i], &mut stack, &mut notice, &mut hover, ctx)
                                {
                                    return Ok(out);
                                }
                            }
                            // Dead space is the ascend gesture — Backspace,
                            // not close: a stray click must not be destructive.
                            None => ascend(&mut stack, &mut notice, &mut hover),
                        }
                    }
                    // Only reaches us when herdr isn't eating it for its own
                    // pane menu (ui.mouse_capture = false, or a
                    // right_click_passthrough_modifier). Same ascend.
                    MouseEventKind::Down(MouseButton::Right) => {
                        dirty = true;
                        ascend(&mut stack, &mut notice, &mut hover);
                    }
                    MouseEventKind::Moved => {
                        let at = hit(level, lay, cols, rows, column, row)?;
                        if at != hover {
                            hover = at;
                            dirty = true;
                        }
                    }
                    // Scroll has no meaning in a menu that fits on screen,
                    // and herdr hands us the wheel instead of scrolling its
                    // own scrollback — ignore it rather than invent one.
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// One level up, or nothing at the root. Backspace, empty-space click and
/// right click all land here.
fn ascend(stack: &mut Vec<(&[Node], String)>, notice: &mut Notice, hover: &mut Option<usize>) {
    if stack.len() > 1 {
        stack.pop();
        *hover = None;
    }
    *notice = Notice::None;
}

/// Pick an item: descend a group, run a sticky leaf in place, hand a plain
/// leaf back to the caller to run after the pane is gone. Shared by the key
/// and click paths so a click behaves exactly like the key.
fn choose<'a>(
    node: &'a Node,
    stack: &mut Vec<(&'a [Node], String)>,
    notice: &mut Notice,
    hover: &mut Option<usize>,
    ctx: &HerdrContext,
) -> Option<Outcome> {
    match &node.kind {
        NodeKind::Group(children) => {
            stack.push((children, node.label.clone()));
            *notice = Notice::None;
            *hover = None; // indices belong to the level that just left
            None
        }
        NodeKind::Leaf(leaf) if node.stick => {
            *notice = match dispatch::execute(leaf, ctx) {
                Ok(()) => Notice::None,
                Err(e) => Notice::Error(format!("{e:#}")),
            };
            None
        }
        NodeKind::Leaf(leaf) => Some(Outcome::Deferred(leaf.clone(), node.label.clone())),
    }
}

/// Config errors render in the strip itself — a popup that opens and
/// instantly vanishes with no explanation is the worst outcome.
pub fn show_error(pal: &Palette, msg: &str) -> Result<()> {
    let _guard = TermGuard::enter(false)?;
    let mut out = std::io::stdout();
    let (cols, rows) = terminal::size().unwrap_or((80, 8));
    queue!(out, terminal::BeginSynchronizedUpdate, SetBackgroundColor(pal.bg))?;
    for y in 0..rows {
        queue!(out, cursor::MoveTo(0, y), terminal::Clear(terminal::ClearType::UntilNewLine))?;
    }
    queue!(
        out,
        cursor::MoveTo(0, 0),
        SetForegroundColor(pal.warn),
        style::Print("whichkey: config error")
    )?;
    for (i, line) in msg.lines().take(rows.saturating_sub(2).max(1) as usize).enumerate() {
        queue!(
            out,
            cursor::MoveTo(0, 1 + i as u16),
            SetForegroundColor(pal.fg),
            style::Print(clip(line, cols as usize))
        )?;
    }
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        SetForegroundColor(pal.dim),
        style::Print("press any key to close"),
        terminal::EndSynchronizedUpdate
    )?;
    out.flush()?;
    loop {
        if let Event::Key(KeyEvent { kind: KeyEventKind::Press, .. }) = event::read()? {
            return Ok(());
        }
    }
}

/// Terminal cells `s` draws in. Not `chars().count()`: that counts Unicode
/// scalar values, which is not what the cursor advances by — CJK and most
/// emoji take two cells, combining marks take none, and a ZWJ sequence is
/// several scalars in one two-cell glyph. Every width in this module comes
/// from here so the strip's arithmetic is in the terminal's own units.
fn width(s: &str) -> usize {
    s.width()
}

/// The item cells of a level: each item's drawn parts, the uniform cell
/// width, and the (x, y) taffy puts each cell at. Rendering and hit-testing
/// must agree down to the cell, so both come from here.
#[allow(clippy::type_complexity)]
fn grid(
    level: &[Node],
    lay: &LayoutConfig,
    cols: usize,
    item_rows: usize,
) -> Result<(Vec<(String, String, bool)>, usize, Vec<(usize, usize)>)> {
    let texts: Vec<(String, String, bool)> =
        level.iter().map(|n| (display_key(n.key), n.label.clone(), n.is_group())).collect();
    let item_width = texts
        .iter()
        .map(|(k, l, g)| width(k) + 2 + width(l) + if *g { 2 } else { 0 })
        .max()
        .unwrap_or(0);
    let places = layout::positions(texts.len(), item_width, cols, item_rows, lay)?;
    Ok((texts, item_width, places))
}

/// Off-screen cells are drawn by nobody, so they are clickable by nobody.
fn drawn(x: usize, y: usize, item_width: usize, cols: usize, item_rows: usize) -> bool {
    x + item_width <= cols + 1 && y < item_rows
}

/// Which item covers pane cell (col, row), if any. The whole `item_width`
/// cell is the target, not just the drawn text — the gutters between
/// columns and the footer line stay dead space, which is the ascend gesture.
fn hit(
    level: &[Node],
    lay: &LayoutConfig,
    cols: u16,
    rows: u16,
    col: u16,
    row: u16,
) -> Result<Option<usize>> {
    // No `.max(1)`: in a 0- or 1-row pane the footer covers everything, so
    // there are no item rows and `row >= item_rows` rejects every click.
    // Clamping here instead would put a phantom item under the footer text.
    let (cols, item_rows) = (cols as usize, rows.saturating_sub(1) as usize);
    let (col, row) = (col as usize, row as usize);
    if row >= item_rows {
        return Ok(None);
    }
    let (_, item_width, places) = grid(level, lay, cols, item_rows)?;
    Ok(places.iter().position(|&(x, y)| {
        drawn(x, y, item_width, cols, item_rows) && row == y && col >= x && col < x + item_width
    }))
}

/// Draws a frame and reports the (cols, rows) it drew at, so hit-testing
/// can use the geometry the user is actually looking at.
fn render(
    pal: &Palette,
    lay: &LayoutConfig,
    stack: &[(&[Node], String)],
    level: &[Node],
    notice: &Notice,
    hover: Option<usize>,
) -> Result<(u16, u16)> {
    let mut out = std::io::stdout();
    let (cols, rows) = terminal::size().unwrap_or((80, 8));
    let cols_u = cols as usize;
    // No title row — the breadcrumb is the pane border title. Only the
    // footer line is carved off the item area. No `.max(1)`: a 0- or 1-row
    // pane is all footer, and `hit()` counts it as having no item rows —
    // drawing an item there would paint something no click can reach.
    let item_rows = rows.saturating_sub(1);

    queue!(out, terminal::BeginSynchronizedUpdate, SetBackgroundColor(pal.bg))?;
    // Repaint every row from column 0; UntilNewLine fills the tail with the
    // strip background (BCE) — no Clear(All), no unpainted flash.
    for y in 0..rows {
        queue!(out, cursor::MoveTo(0, y), terminal::Clear(terminal::ClearType::UntilNewLine))?;
    }

    // Items form a footer-style grid: the strip width picks the column
    // count (unless `columns` pins it), taffy positions the grid with
    // the `[layout]` distribution knobs — space-evenly both ways by
    // default.
    let (texts, item_width, places) = grid(level, lay, cols_u, item_rows as usize)?;
    for (i, ((key, label, group), (x, y))) in texts.iter().zip(places).enumerate() {
        if !drawn(x, y, item_width, cols_u, item_rows as usize) {
            continue; // off-screen; live validation decides if we page
        }
        // The hovered cell is painted whole, so the click target is visible
        // — with the raised background, or reverse video when the palette
        // has none of its own (ANSI mode).
        let hot = hover == Some(i);
        if hot {
            queue!(out, cursor::MoveTo(x as u16, y as u16))?;
            if pal.surface == pal.bg {
                queue!(out, SetAttribute(Attribute::Reverse))?;
            } else {
                queue!(out, SetBackgroundColor(pal.surface))?;
            }
            queue!(out, style::Print(" ".repeat(item_width.min(cols_u.saturating_sub(x)))))?;
        }
        queue!(
            out,
            cursor::MoveTo(x as u16, y as u16),
            SetForegroundColor(pal.accent),
            style::Print(key),
            SetForegroundColor(pal.dim),
            style::Print("  "),
            SetForegroundColor(pal.fg),
            style::Print(clip(label, cols_u.saturating_sub(x + width(key) + 2)))
        )?;
        if *group {
            queue!(out, SetForegroundColor(pal.dim), style::Print(" ›"))?;
        }
        if hot {
            queue!(out, SetAttribute(Attribute::Reset), SetBackgroundColor(pal.bg))?;
        }
    }

    // Footer: hints for where we are, transient notice after.
    let hints = if stack.len() > 1 { " esc back · ctrl+c close" } else { " esc close" };
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        SetForegroundColor(pal.dim),
        style::Print(hints)
    )?;
    let (text, color) = match notice {
        Notice::None => (String::new(), pal.dim),
        Notice::Info(m) => (m.clone(), pal.dim),
        Notice::Error(m) => (m.clone(), pal.warn),
    };
    if !text.is_empty() {
        queue!(
            out,
            SetForegroundColor(pal.dim),
            style::Print(" · "),
            SetForegroundColor(color),
            style::Print(clip(&text, cols_u.saturating_sub(width(hints) + 3)))
        )?;
    }

    queue!(out, terminal::EndSynchronizedUpdate)?;
    out.flush()?;
    Ok((cols, rows))
}

/// `s` cut to `max` cells, ellipsis included in the count. The cut lands on
/// a grapheme boundary, never inside one: half a ZWJ sequence or a combining
/// mark parted from its base is not something the terminal can draw. A
/// double-width grapheme with one cell left is dropped whole, so the result
/// can come up a cell short — the alternative is drawing past the edge.
fn clip(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let budget = max.saturating_sub(1); // the '…' takes a cell of its own
    let mut t = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = width(g);
        if used + w > budget {
            break;
        }
        t.push_str(g);
        used += w;
    }
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Leaf, RunIn};

    fn leaf(key: char, label: &str) -> Node {
        Node {
            key,
            label: label.into(),
            stick: false,
            requires: None,
            requires_plugin: None,
            kind: NodeKind::Leaf(Leaf::Run {
                cmd: "true".into(),
                run_in: RunIn::Background,
                cwd: None,
            }),
        }
    }

    /// 8 leaves 15 chars wide: item_width 18, the live strip's root level.
    fn level() -> Vec<Node> {
        "abcdefgh".chars().map(|c| leaf(c, "fifteen-chars-x")).collect()
    }

    // Labels whose cell count is not their scalar count, one per way that
    // happens: wide glyphs, a mark that rides on its base, and a sequence
    // the terminal joins into a single glyph.
    const CJK: &str = "文件浏览器"; // 5 scalars, 10 cells
    const ACUTE: &str = "e\u{301}cran"; // 6 scalars, 5 cells
    const FAMILY: &str = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"; // 5 scalars, 2 cells

    #[test]
    fn clicks_map_back_to_the_item_under_them() {
        let (lay, level) = (LayoutConfig::default(), level());
        // Same geometry as layout::tests::default_positions_footer_grid:
        // 179 cols, 5 rows (4 item rows + footer), 4×2 cells 18 wide.
        let at = |col, row| hit(&level, &lay, 179, 5, col, row).unwrap();
        assert_eq!(at(21, 1), Some(0)); // first cell, first column
        assert_eq!(at(30, 1), Some(0)); // mid-cell
        assert_eq!(at(38, 1), Some(0)); // last column of the cell
        assert_eq!(at(21, 3), Some(1)); // column-major: second item is below
        assert_eq!(at(61, 1), Some(2));
        assert_eq!(at(140, 3), Some(7));
        assert_eq!(at(20, 1), None); // one column left of the first cell
        assert_eq!(at(39, 1), None); // gutter between columns
        assert_eq!(at(21, 0), None); // blank row above (space-around)
        assert_eq!(at(21, 2), None); // blank row between the two item rows
    }

    #[test]
    fn the_same_click_resolves_differently_at_a_different_size() {
        // Why the loop hit-tests against the frame it drew instead of a
        // fresh terminal::size(): resolving a click with the wrong geometry
        // is not a near miss, it is a different item — or none at all.
        let (lay, level) = (LayoutConfig::default(), level());
        let (col, row) = (21, 3);
        assert_eq!(hit(&level, &lay, 179, 5, col, row).unwrap(), Some(1)); // the drawn frame
        assert_eq!(hit(&level, &lay, 100, 5, col, row).unwrap(), Some(2)); // narrower: other item
        assert_eq!(hit(&level, &lay, 179, 8, col, row).unwrap(), None); // taller: dead space
    }

    #[test]
    fn footer_row_is_dead_space() {
        let (lay, level) = (LayoutConfig::default(), level());
        // Row 4 of 5 is the footer, so nothing there is clickable — that is
        // what makes clicking it ascend instead of firing an item.
        for col in 0..179 {
            assert_eq!(hit(&level, &lay, 179, 5, col, 4).unwrap(), None);
        }
    }

    #[test]
    fn a_pane_with_no_item_rows_draws_and_hits_nothing() {
        // 1 row is all footer. Nothing may be clickable there — a click
        // must ascend, not fire whatever item row 0 would have held — and
        // nothing may be drawn there either, or the strip would show items
        // that reject every click. Both sides derive item_rows the same way.
        let (lay, level) = (LayoutConfig::default(), level());
        for rows in 0..=1u16 {
            for col in 0..179u16 {
                assert_eq!(hit(&level, &lay, 179, rows, col, 0).unwrap(), None);
            }
            let item_rows = rows.saturating_sub(1) as usize; // as render() computes it
            let (_, item_width, places) = grid(&level, &lay, 179, item_rows).unwrap();
            let shown: Vec<_> =
                places.iter().filter(|&&(x, y)| drawn(x, y, item_width, 179, item_rows)).collect();
            assert!(shown.is_empty(), "{rows}-row pane draws unclickable cells {shown:?}");
        }
    }

    #[test]
    fn cells_the_renderer_skips_are_not_clickable() {
        // Strip too narrow for the columns: cells past the right edge are
        // never drawn, so clicks that land in them must miss.
        let (lay, level) = (LayoutConfig::default(), level());
        let (_, item_width, places) = grid(&level, &lay, 20, 4).unwrap();
        let skipped: Vec<_> =
            places.iter().filter(|&&(x, y)| !drawn(x, y, item_width, 20, 4)).collect();
        assert!(!skipped.is_empty(), "expected overflow at 20 columns, got {places:?}");
        for col in 0..20u16 {
            for row in 0..4u16 {
                if let Some(i) = hit(&level, &lay, 20, 5, col, row).unwrap() {
                    let (x, y) = places[i];
                    assert!(drawn(x, y, item_width, 20, 4), "hit an undrawn cell at {col},{row}");
                }
            }
        }
    }

    #[test]
    fn group_cells_are_wider_by_the_marker() {
        let lay = LayoutConfig::default();
        let mut level = vec![leaf('a', "same-label-here")];
        let (_, leaf_width, _) = grid(&level, &lay, 179, 4).unwrap();
        level[0].kind = NodeKind::Group(Vec::new());
        let (_, group_width, _) = grid(&level, &lay, 179, 4).unwrap();
        assert_eq!(group_width, leaf_width + 2); // the " ›" is part of the target
    }

    #[test]
    fn item_width_counts_cells_not_scalars() {
        let lay = LayoutConfig::default();
        let cell = |label: &str| grid(&[leaf('a', label)], &lay, 179, 4).unwrap().1;
        // One-cell key plus the two-space gap, then the label's own cells.
        assert_eq!(cell("files"), 3 + 5);
        assert_eq!(cell(CJK), 3 + 10); // two cells per glyph
        assert_eq!(cell(ACUTE), 3 + 5); // the acute draws on the 'e', in no cell of its own
        assert_eq!(cell(FAMILY), 3 + 2); // five scalars, one two-cell glyph
    }

    #[test]
    fn wide_labels_keep_the_columns_apart() {
        // The misalignment itself: a packed row of CJK labels. At the scalar
        // count each cell would be 8 wide and the columns 10 apart, with 13
        // cells of text drawn into each — every label bleeding into its
        // neighbour. The cell count is what keeps them clear of each other.
        let cfg: LayoutConfig =
            toml::from_str("justify = \"start\"\ngutter = 2\ncolumns = 4\n").unwrap();
        let level: Vec<Node> = "abcd".chars().map(|c| leaf(c, CJK)).collect();
        let (_, item_width, places) = grid(&level, &cfg, 100, 4).unwrap();
        assert_eq!(item_width, 13); // 1 key + 2 gap + 10 label
        let mut xs: Vec<usize> = places.iter().map(|&(x, _)| x).collect();
        xs.sort_unstable();
        for w in xs.windows(2) {
            assert!(w[1] - w[0] >= 3 + width(CJK), "columns {xs:?} overlap the drawn labels");
        }
    }

    #[test]
    fn every_cell_of_a_non_ascii_menu_hits_its_own_item() {
        // Clicking any item still fires that item: the hit-test measures the
        // cells the renderer laid out, whatever the labels are made of.
        let lay = LayoutConfig::default();
        let mut level = vec![leaf('a', CJK), leaf('b', ACUTE), leaf('c', FAMILY), leaf('d', CJK)];
        level[3].kind = NodeKind::Group(Vec::new()); // the widest: label plus " ›"
        let (_, item_width, places) = grid(&level, &lay, 179, 4).unwrap();
        assert_eq!(item_width, 15);
        for (i, &(x, y)) in places.iter().enumerate() {
            assert!(drawn(x, y, item_width, 179, 4), "item {i} at {x},{y} is never drawn");
            for col in x..x + item_width {
                assert_eq!(hit(&level, &lay, 179, 5, col as u16, y as u16).unwrap(), Some(i));
            }
        }
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("exactly-ten", 11), "exactly-ten");
        assert_eq!(clip("much too long for this", 8), "much to…");
    }

    #[test]
    fn clipping_cuts_on_cell_and_grapheme_boundaries() {
        // The budget is cells: five glyphs fill ten of them, and an odd
        // budget leaves one spare rather than half-drawing a wide glyph.
        assert_eq!(clip(CJK, 10), CJK);
        assert_eq!(clip(CJK, 9), "文件浏览…");
        assert_eq!(clip(CJK, 8), "文件浏…");
        // A combining mark leaves with its base or not at all — never left
        // behind to draw itself on whatever precedes it.
        assert_eq!(clip(ACUTE, 3), "e\u{301}c…");
        assert_eq!(clip(ACUTE, 2), "e\u{301}…");
        // One glyph, five scalars: kept whole or dropped whole, never cut
        // into a lone man and a dangling joiner.
        let family = format!("{FAMILY} family");
        assert_eq!(clip(&family, 9), family); // 2 + 1 + 6 cells, nothing to cut
        assert_eq!(clip(&family, 3), format!("{FAMILY}…"));
        assert_eq!(clip(&family, 2), "…");
    }
}
