//! Interactive terminal UI: analysis selection menu and result rendering.

use std::fmt::Write as _; // writeln! on String
use std::io::Write; // stdout.flush()

use anyhow::Result;
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};
use dialoguer::{theme::ColorfulTheme, Select};

use crate::analysis::{AnalysisOutput, Registry};
use crate::util::truncate;

const MIN_CELL_WIDTH: usize = 20;
const FALLBACK_TERM_WIDTH: usize = 120;

/// User's choice from the analysis menu.
pub enum Selection {
    Analysis(usize),
    Quit,
    /// CTRL+C was pressed — caller decides whether to quit or warn.
    Interrupted,
}

/// Show the analysis menu and return the user's choice.
///
/// ESC → `Quit`; CTRL+C → `Interrupted` (caller tracks double-press); a
/// normal selection of the last "Quit" item → `Quit`.
pub fn select_analysis(registry: &Registry) -> Result<Selection> {
    let mut items = registry.names();
    items.push("Quit");

    eprintln!();
    eprintln!(
        "  {}  {}",
        "Available analyses".bold(),
        "· esc or ctrl+c to quit".dimmed(),
    );
    let result = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("")
        .items(&items)
        .default(0)
        .interact_opt();

    match result {
        Err(_) => Ok(Selection::Interrupted),
        Ok(None) => Ok(Selection::Quit),
        Ok(Some(idx)) => {
            if idx + 1 == items.len() {
                Ok(Selection::Quit)
            } else {
                Ok(Selection::Analysis(idx))
            }
        }
    }
}

/// Render an analysis output in a full-screen pager.
///
/// Returns normally on q / ESC / CTRL+C — never exits the process.
pub fn display_output(output: &AnalysisOutput) {
    colored::control::set_override(true);
    let content = format_output(output);
    colored::control::unset_override();

    if let Err(e) = run_pager(&content) {
        eprintln!("pager error: {e}");
        print!("{content}");
    }
}

// ---------------------------------------------------------------------------
// Pager
// ---------------------------------------------------------------------------

fn run_pager(content: &str) -> Result<()> {
    let lines: Vec<&str> = content.lines().collect();
    let mut stdout = std::io::stdout();

    terminal::enable_raw_mode()?;
    execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide)?;

    let result = pager_loop(&lines, &mut stdout);

    // Always restore the terminal, even on error.
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
        let content_rows = rows.saturating_sub(1); // bottom row reserved for footer
        let max_scroll = lines.len().saturating_sub(content_rows);

        // Render content lines.
        queue!(stdout, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;
        for i in 0..content_rows {
            if let Some(line) = lines.get(scroll + i) {
                queue!(stdout, Print(line), Print("\r\n"))?;
            }
        }

        // Render footer: reversed status bar spanning the full width.
        let pct = if lines.len() <= content_rows {
            100usize
        } else {
            ((scroll + content_rows) * 100 / lines.len()).min(100)
        };
        let footer_text = format!(
            " analgun  │  ↑/↓ row   fn+↑/↓ page   esc/q back   {pct}%"
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

        // Handle input — blocks until a key or resize event arrives.
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match (key.code, key.modifiers) {
                    // Exit pager → return to menu.
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
                }
            }
            Event::Resize(_, _) => {} // just re-render on next iteration
            _ => {}
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Table formatting
// ---------------------------------------------------------------------------

/// Build the full rendered string for `output` (kept separate so it can be
/// unit-tested without invoking the pager).
fn format_output(output: &AnalysisOutput) -> String {
    match output {
        AnalysisOutput::Table {
            title,
            columns,
            rows,
            summary,
        } => format_table(title, columns, rows, summary.as_deref()),
    }
}

fn format_table(
    title: &str,
    columns: &[String],
    rows: &[Vec<String>],
    summary: Option<&str>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}\n", title.bold().underline());

    if rows.is_empty() {
        let _ = writeln!(out, "{}", "No data found.".yellow());
    } else {
        let max_cell = max_cell_chars(columns.len());
        let mut table = Table::new();
        // Disabled arrangement: columns are exactly as wide as their widest
        // cell. We pre-truncate content to fit terminal width.
        table.set_content_arrangement(ContentArrangement::Disabled);
        table.set_header(columns.iter().map(header_cell).collect::<Vec<_>>());
        for row in rows {
            table.add_row(
                row.iter()
                    .map(|c| Cell::new(truncate(c, max_cell)))
                    .collect::<Vec<_>>(),
            );
        }
        let _ = writeln!(out, "{table}");
    }

    if let Some(s) = summary {
        let _ = writeln!(out, "\n{} {}", "→".cyan(), s.dimmed());
    }
    out
}

fn header_cell(label: &String) -> Cell {
    Cell::new(label)
        .add_attribute(Attribute::Bold)
        .fg(Color::Cyan)
}

/// Maximum characters per cell, sized to keep the rendered table within the
/// current terminal width.
fn max_cell_chars(num_cols: usize) -> usize {
    let term_width = terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(FALLBACK_TERM_WIDTH);
    let cols = num_cols.max(1);
    let per_col = term_width.saturating_sub(cols + 1) / cols;
    per_col.max(MIN_CELL_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_table_includes_title_and_summary() {
        let out = format_table(
            "Test Title",
            &["A".into(), "B".into()],
            &[vec!["1".into(), "2".into()]],
            Some("ok"),
        );
        assert!(out.contains("Test Title"));
        assert!(out.contains("ok"));
        assert!(out.contains('1'));
    }

    #[test]
    fn format_table_handles_empty_rows() {
        let out = format_table("Empty", &["X".into()], &[], None);
        assert!(out.contains("No data found"));
    }
}
