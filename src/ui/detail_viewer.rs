//! Multi-item detail viewer: shows one [`ListItem`] at a time, with up/down
//! navigation between adjacent items.

use std::io::Write;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};

use crate::analysis::ListItem;

/// Open a full-screen viewer starting at `start_idx`.  Returns the index of
/// the last-viewed item so the calling list can restore the cursor position.
///
/// Key bindings:
/// - `↑` / `↓` — jump to previous / next item (resets scroll)
/// - `Page Up` / `Page Down` / `j` / `k` — scroll within the current item
/// - `Home` / `End` — top / bottom of current item
/// - `q` / `Esc` / `Ctrl+C` — return to the list
pub(super) fn run_detail_viewer(items: &[ListItem], start_idx: usize) -> Result<usize> {
    let mut idx = start_idx;
    let mut scroll: usize = 0;
    let mut stdout = std::io::stdout();

    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = detail_loop(items, &mut idx, &mut scroll, &mut stdout);

    let _ = execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result?;
    Ok(idx)
}

fn detail_loop(
    items: &[ListItem],
    idx: &mut usize,
    scroll: &mut usize,
    stdout: &mut impl Write,
) -> Result<()> {
    loop {
        let lines: Vec<&str> = items[*idx].detail.lines().collect();

        let (cols, rows) = terminal::size().unwrap_or((120, 40));
        let rows = rows as usize;
        let cols = cols as usize;
        let content_rows = rows.saturating_sub(1);
        let max_scroll = lines.len().saturating_sub(content_rows);
        *scroll = (*scroll).min(max_scroll);

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(ClearType::All)
        )?;
        for i in 0..content_rows {
            if let Some(line) = lines.get(*scroll + i) {
                queue!(stdout, Print(line), Print("\r\n"))?;
            }
        }

        let pct = if lines.len() <= content_rows {
            100
        } else {
            ((*scroll + content_rows) * 100 / lines.len()).min(100)
        };
        let footer_text = format!(
            " bloom-log-analyzer  │  req {cur}/{total}  │  ↑/↓ prev/next req   pgup/pgdn scroll   q/esc back   {pct}%",
            cur = *idx + 1,
            total = items.len(),
        );
        let footer = format!("{:<width$}", footer_text, width = cols);
        queue!(
            stdout,
            cursor::MoveTo(0, (rows - 1) as u16),
            SetAttribute(StyleAttr::Reverse),
            Print(&footer),
            SetAttribute(StyleAttr::Reset),
        )?;
        stdout.flush()?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _)
                | (KeyCode::Esc, _)
                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),

                (KeyCode::Up, _) => {
                    if *idx > 0 {
                        *idx -= 1;
                        *scroll = 0;
                    }
                }
                (KeyCode::Down, _) => {
                    if *idx + 1 < items.len() {
                        *idx += 1;
                        *scroll = 0;
                    }
                }
                (KeyCode::PageUp, _) | (KeyCode::Char('k'), _) => {
                    *scroll = scroll.saturating_sub(content_rows);
                }
                (KeyCode::PageDown, _) | (KeyCode::Char('j'), _) => {
                    *scroll = scroll.saturating_add(content_rows).min(max_scroll);
                }
                (KeyCode::Home, _) => *scroll = 0,
                (KeyCode::End, _) => *scroll = max_scroll,
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}
