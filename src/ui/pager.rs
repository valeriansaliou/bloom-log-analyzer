//! Simple full-screen pager.  Used for `AnalysisOutput::Table` content.

use std::io::Write;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};

/// Show `content` in a scrollable pager.  Returns when the user presses
/// `q`, `Esc`, or `Ctrl+C`.
pub(super) fn run_pager(content: &str) -> Result<()> {
    let lines: Vec<&str> = content.lines().collect();
    let mut stdout = std::io::stdout();

    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = pager_loop(&lines, &mut stdout);

    let _ = execute!(stdout, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result
}

fn pager_loop(lines: &[&str], stdout: &mut impl Write) -> Result<()> {
    let mut scroll: usize = 0;

    loop {
        let (cols, rows) = terminal::size().unwrap_or((120, 40));
        let rows = rows as usize;
        let cols = cols as usize;
        let content_rows = rows.saturating_sub(1);
        let max_scroll = lines.len().saturating_sub(content_rows);

        queue!(
            stdout,
            cursor::MoveTo(0, 0),
            terminal::Clear(ClearType::All)
        )?;
        for i in 0..content_rows {
            if let Some(line) = lines.get(scroll + i) {
                queue!(stdout, Print(line), Print("\r\n"))?;
            }
        }

        let pct = scroll_pct(scroll, content_rows, lines.len());
        let footer_text =
            format!(" bloom-log-analyzer  │  ↑/↓ row   fn+↑/↓ page   esc/q back   {pct}%");
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
                | (KeyCode::Char('c'), KeyModifiers::CONTROL) => break,
                (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                    scroll = scroll.saturating_sub(1);
                }
                (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                    scroll = scroll.saturating_add(1).min(max_scroll);
                }
                (KeyCode::PageUp, _) => {
                    scroll = scroll.saturating_sub(content_rows);
                }
                (KeyCode::PageDown, _) => {
                    scroll = scroll.saturating_add(content_rows).min(max_scroll);
                }
                (KeyCode::Home, _) => scroll = 0,
                (KeyCode::End, _) => scroll = max_scroll,
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
    Ok(())
}

fn scroll_pct(scroll: usize, content_rows: usize, total: usize) -> usize {
    if total <= content_rows {
        100
    } else {
        ((scroll + content_rows) * 100 / total).min(100)
    }
}
