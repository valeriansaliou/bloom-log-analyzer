// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

//! Legacy `comfy-table`-based renderer for `AnalysisOutput::Table` content.
//! Produces a `String` that the [`pager`](super::pager) then scrolls.

use std::fmt::Write as _;

use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use crossterm::terminal;

use crate::analysis::AnalysisOutput;
use crate::util::truncate;

const MIN_CELL_WIDTH: usize = 20;
const FALLBACK_TERM_WIDTH: usize = 120;

/// Build the full rendered string for `output`.  Only `Table` produces content;
/// other variants return an empty string (they have dedicated displays).
pub(super) fn format_output(output: &AnalysisOutput) -> String {
    match output {
        AnalysisOutput::Table {
            title,
            columns,
            rows,
            summary,
        } => format_table(title, columns, rows, summary.as_deref()),
        _ => String::new(),
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
