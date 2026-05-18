//! Interactive terminal UI: analysis selection menu and result rendering.

use std::fmt::Write as _;

use anyhow::Result;
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
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
    let result = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select analysis  (esc/ctrl+c to quit)")
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

/// Render an analysis output in a `less`-style pager (arrow-key navigation).
pub fn display_output(output: &AnalysisOutput) {
    // Force ANSI codes into the string — `minus` renders them in the pager, but
    // `colored` would strip them when writing to a String (no TTY detected on
    // the build path).
    colored::control::set_override(true);
    let content = format_output(output);
    colored::control::unset_override();

    let mut pager = minus::Pager::new();
    let _ = pager.set_prompt("analgun  │  ↑/↓ row   fn+↑/↓ page   esc back");
    let _ = std::fmt::Write::write_str(&mut pager, &content);
    let _ = minus::page_all(pager);
}

/// Build the full rendered string for `output` (used by [`display_output`];
/// kept separate so it can be unit-tested without invoking the pager).
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
        // Disabled arrangement: each column is exactly as wide as its widest
        // cell. We pre-truncate cell content to fit terminal width so the table
        // never overflows.
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
    let term_width = crossterm::terminal::size()
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
