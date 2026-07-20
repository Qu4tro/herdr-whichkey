//! The bottom-strip menu: flicker-free rendering (one synchronized frame
//! per keystroke, no full-screen clears between frames) and the
//! single-keystroke loop. We own the whole popup pane surface.

use std::io::Write as _;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::{cursor, execute, queue, style, terminal};

use crate::context::HerdrContext;
use crate::dispatch;
use crate::keys::display_key;
use crate::model::{Node, NodeKind};
use crate::theme::Palette;

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

/// Raw-mode/cursor/wrap guard so every exit path restores the terminal.
struct TermGuard;

impl TermGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        execute!(std::io::stdout(), cursor::Hide, terminal::DisableLineWrap)?;
        Ok(Self)
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
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

pub fn run(tree: &[Node], pal: &Palette, ctx: &HerdrContext) -> Result<Outcome> {
    let _guard = TermGuard::enter()?;
    let mut stack: Vec<(&[Node], String)> = vec![(tree, "whichkey".into())];
    let mut notice = Notice::None;

    loop {
        let (level, _) = *stack.last().expect("stack never empty");
        render(pal, &stack, level, &notice)?;

        match event::read()? {
            Event::Resize(_, _) => continue,
            Event::Key(KeyEvent { code, modifiers, kind, .. })
                if kind == KeyEventKind::Press || kind == KeyEventKind::Repeat =>
            {
                match code {
                    KeyCode::Esc => return Ok(Outcome::Closed),
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(Outcome::Closed)
                    }
                    KeyCode::Backspace => {
                        if stack.len() > 1 {
                            stack.pop();
                        }
                        notice = Notice::None;
                    }
                    KeyCode::Char(c) => match level.iter().find(|n| n.key == c) {
                        Some(node) => match &node.kind {
                            NodeKind::Group(children) => {
                                stack.push((children, node.label.clone()));
                                notice = Notice::None;
                            }
                            NodeKind::Leaf(leaf) if node.stick => {
                                notice = match dispatch::execute(leaf, ctx) {
                                    Ok(()) => Notice::None,
                                    Err(e) => Notice::Error(format!("{e:#}")),
                                };
                            }
                            NodeKind::Leaf(leaf) => {
                                return Ok(Outcome::Deferred(leaf.clone(), node.label.clone()))
                            }
                        },
                        None => notice = Notice::Info(format!("no binding: {}", display_key(c))),
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Config errors render in the strip itself — a popup that opens and
/// instantly vanishes with no explanation is the worst outcome.
pub fn show_error(pal: &Palette, msg: &str) -> Result<()> {
    let _guard = TermGuard::enter()?;
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

fn render(pal: &Palette, stack: &[(&[Node], String)], level: &[Node], notice: &Notice) -> Result<()> {
    let mut out = std::io::stdout();
    let (cols, rows) = terminal::size().unwrap_or((80, 8));
    let cols_u = cols as usize;
    let item_rows = rows.saturating_sub(2).max(1);

    queue!(out, terminal::BeginSynchronizedUpdate, SetBackgroundColor(pal.bg))?;
    // Repaint every row from column 0; UntilNewLine fills the tail with the
    // strip background (BCE) — no Clear(All), no unpainted flash.
    for y in 0..rows {
        queue!(out, cursor::MoveTo(0, y), terminal::Clear(terminal::ClearType::UntilNewLine))?;
    }

    // Title: breadcrumb of where we are.
    let crumb = stack.iter().skip(1).map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(" › ");
    queue!(
        out,
        cursor::MoveTo(0, 0),
        SetForegroundColor(pal.accent),
        SetAttribute(Attribute::Bold),
        style::Print(" whichkey"),
        SetAttribute(Attribute::Reset),
        SetBackgroundColor(pal.bg)
    )?;
    if !crumb.is_empty() {
        queue!(
            out,
            SetForegroundColor(pal.dim),
            style::Print(" › "),
            SetForegroundColor(pal.fg),
            style::Print(clip(&crumb, cols_u.saturating_sub(13)))
        )?;
    }

    // Items, column-major.
    let texts: Vec<(String, String, bool)> = level
        .iter()
        .map(|n| (display_key(n.key), n.label.clone(), n.is_group()))
        .collect();
    let col_width = texts
        .iter()
        .map(|(k, l, g)| k.chars().count() + 2 + l.chars().count() + if *g { 2 } else { 0 })
        .max()
        .unwrap_or(0)
        + 3;
    for (i, (key, label, group)) in texts.iter().enumerate() {
        let (col, row) = column_major(i, item_rows as usize);
        let x = 1 + col * col_width;
        if x + col_width > cols_u + 1 {
            continue; // off-screen column; live validation decides if we page
        }
        queue!(
            out,
            cursor::MoveTo(x as u16, 1 + row as u16),
            SetForegroundColor(pal.accent),
            style::Print(key),
            SetForegroundColor(pal.dim),
            style::Print("  "),
            SetForegroundColor(pal.fg),
            style::Print(clip(label, cols_u.saturating_sub(x + key.chars().count() + 2)))
        )?;
        if *group {
            queue!(out, SetForegroundColor(pal.dim), style::Print(" ›"))?;
        }
    }

    // Footer: static hints left, transient notice after.
    queue!(
        out,
        cursor::MoveTo(0, rows.saturating_sub(1)),
        SetForegroundColor(pal.dim),
        style::Print(" esc close · bksp back")
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
            style::Print(clip(&text, cols_u.saturating_sub(26)))
        )?;
    }

    queue!(out, terminal::EndSynchronizedUpdate)?;
    out.flush()?;
    Ok(())
}

/// Index → (column, row) filling top-to-bottom, then left-to-right.
fn column_major(index: usize, rows: usize) -> (usize, usize) {
    (index / rows.max(1), index % rows.max(1))
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    t.push('…');
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_major_fills_down_first() {
        // 6 rows available: items 0..6 are column 0, 6..12 column 1.
        assert_eq!(column_major(0, 6), (0, 0));
        assert_eq!(column_major(5, 6), (0, 5));
        assert_eq!(column_major(6, 6), (1, 0));
        assert_eq!(column_major(13, 6), (2, 1));
    }

    #[test]
    fn clipping() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("exactly-ten", 11), "exactly-ten");
        assert_eq!(clip("much too long for this", 8), "much to…");
    }
}
