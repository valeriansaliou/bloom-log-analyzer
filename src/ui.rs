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

use crate::analysis::{AnalysisOutput, ListItem, Registry};
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

/// Render an analysis output. Returns normally — never exits the process.
///
/// - `Table` → scrollable pager.
/// - `SelectableList` → navigable list; Enter opens the item's detail in the
///   pager; ESC returns to the list; ESC from the list returns to the menu.
pub fn display_output(output: &AnalysisOutput) {
    match output {
        AnalysisOutput::Table { .. } => {
            colored::control::set_override(true);
            let content = format_output(output);
            colored::control::unset_override();
            if let Err(e) = run_pager(&content) {
                eprintln!("pager error: {e}");
                print!("{content}");
            }
        }
        AnalysisOutput::SelectableList { title, items, summary } => {
            display_selectable_list_impl(title, items, summary.as_deref(), None);
        }
        AnalysisOutput::SubMenu { .. } => {} // routed by lib.rs before reaching here
    }
}

// ---------------------------------------------------------------------------
// Selectable list
// ---------------------------------------------------------------------------

/// Display a selectable list with an optional breadcrumb header above the
/// title (used by the outlier sub-menu to show which detection type is active).
pub(crate) fn display_selectable_list_with_context(
    title: &str,
    items: &[ListItem],
    summary: Option<&str>,
    context: &str,
) {
    display_selectable_list_impl(title, items, summary, Some(context));
}

fn display_selectable_list_impl(
    title: &str,
    items: &[ListItem],
    summary: Option<&str>,
    context: Option<&str>,
) {
    if items.is_empty() {
        eprintln!("\n  No results.");
        return;
    }

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    let mut last_idx: usize = 0;

    loop {
        eprintln!();
        // Sticky breadcrumb: always visible above the list on every render.
        if let Some(ctx) = context {
            eprintln!("  {}", ctx.bold().cyan());
        }
        eprintln!(
            "  {}  {}",
            title.bold(),
            "· enter to inspect   ↑/↓ in viewer to jump req   esc back".dimmed(),
        );
        if let Some(s) = summary {
            eprintln!("  {}", s.dimmed());
        }

        let result = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("")
            .items(&labels)
            .default(last_idx)
            .interact_opt();

        match result {
            Ok(None) | Err(_) => break,
            Ok(Some(idx)) => {
                last_idx = run_detail_viewer(items, idx).unwrap_or(idx);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Detail viewer — multi-item navigation
// ---------------------------------------------------------------------------

/// Open a full-screen viewer starting at `start_idx`.  Returns the index of
/// the last-viewed item so the list can restore the cursor position.
///
/// Key bindings:
/// - `↑` / `↓` — jump to previous / next item (resets scroll)
/// - `Page Up` / `Page Down` — scroll within the current item
/// - `Home` / `End` — top / bottom of current item
/// - `q` / `Esc` / `Ctrl+C` — return to the list
fn run_detail_viewer(items: &[ListItem], start_idx: usize) -> Result<usize> {
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
        // Clamp scroll when switching to a shorter item.
        *scroll = (*scroll).min(max_scroll);

        queue!(stdout, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;
        for i in 0..content_rows {
            if let Some(line) = lines.get(*scroll + i) {
                queue!(stdout, Print(line), Print("\r\n"))?;
            }
        }

        let pct = if lines.len() <= content_rows {
            100usize
        } else {
            ((*scroll + content_rows) * 100 / lines.len()).min(100)
        };
        let footer_text = format!(
            " analgun  │  req {cur}/{total}  │  ↑/↓ prev/next req   pgup/pgdn scroll   q/esc back   {pct}%",
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
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _)
                    | (KeyCode::Esc, _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),

                    // Navigate between items — reset scroll on each jump.
                    (KeyCode::Up, _) => {
                        if *idx > 0 { *idx -= 1; *scroll = 0; }
                    }
                    (KeyCode::Down, _) => {
                        if *idx + 1 < items.len() { *idx += 1; *scroll = 0; }
                    }

                    // Scroll within the current item.
                    (KeyCode::PageUp, _) | (KeyCode::Char('k'), _) => {
                        *scroll = scroll.saturating_sub(content_rows);
                    }
                    (KeyCode::PageDown, _) | (KeyCode::Char('j'), _) => {
                        *scroll = scroll.saturating_add(content_rows).min(max_scroll);
                    }
                    (KeyCode::Home, _) => *scroll = 0,
                    (KeyCode::End, _) => *scroll = max_scroll,
                    _ => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
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
        AnalysisOutput::SelectableList { .. } => String::new(), // handled by display_selectable_list
        AnalysisOutput::SubMenu { .. } => String::new(), // handled by lib.rs
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
