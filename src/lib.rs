//! analgun — HTTP request log analyzer.
//!
//! Parses HTTP request logs into pre-aggregated statistics, then offers
//! interactive analyses (heaviest routes, heaviest identifiers, …).
//!
//! See the README / `CLAUDE.md` for architecture, log format, and instructions
//! on how to add a new analysis.

pub mod analysis;
pub mod log;
pub mod parser;
pub mod ui;
pub mod util;

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::analysis::Registry;
use crate::log::ParsedLog;
use crate::ui::Selection;

/// Top-level entry point: parse a log file, then run the interactive menu loop
/// until the user picks "Quit".
pub fn run(log_file: &Path) -> Result<()> {
    let log = parser::parse_file(log_file)
        .with_context(|| format!("Failed to read log file: {}", log_file.display()))?;

    if log.total_requests == 0 {
        eprintln!("{}", "No requests found in log file.".yellow());
        return Ok(());
    }

    print_summary(&log, log_file);

    let registry = Registry::default();
    let mut last_was_interrupt = false;
    loop {
        match ui::select_analysis(&registry)? {
            Selection::Quit => break,
            Selection::Interrupted => {
                if last_was_interrupt {
                    break;
                }
                last_was_interrupt = true;
                eprintln!(
                    "\n{}",
                    "  Press Ctrl+C again to quit.".yellow()
                );
            }
            Selection::Analysis(idx) => {
                last_was_interrupt = false;
                if let Some(output) = registry.run(idx, &log) {
                    ui::display_output(&output);
                }
            }
        }
    }

    Ok(())
}

fn print_summary(log: &ParsedLog, log_file: &Path) {
    let filename = log_file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| log_file.display().to_string());

    let methods: BTreeSet<&str> = log.route_counts.keys().map(|k| k.method.as_str()).collect();
    let methods_str = methods.into_iter().collect::<Vec<_>>().join(", ");

    let term_width = crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80);
    // Rule spans available width, capped so it doesn't stretch on huge terminals.
    let rule_len = term_width.saturating_sub(4).min(68);
    let rule = "─".repeat(rule_len);

    // Label column — pad to align values. "identifiers" is the longest (11).
    const W: usize = 13;
    // Helper: dimmed label + bold value on one row.
    let row = |label: &str, value: &str| {
        eprintln!(
            "  {}  {}",
            format!("{label:<W$}").dimmed(),
            value.bold()
        );
    };

    eprintln!();
    eprintln!(
        "  {}  {}  {}",
        "analgun".bold().cyan(),
        "·".dimmed(),
        filename.bold()
    );
    eprintln!("  {}", rule.dimmed().cyan());
    eprintln!();
    row("requests", &util::fmt_count(log.total_requests));
    row(
        "identifiers",
        &format!("{} unique", util::fmt_count(log.identifier_counts.len())),
    );
    row("file size", &util::fmt_bytes(log.file_size));
    if let (Some(first), Some(last)) = (&log.first_timestamp, &log.last_timestamp) {
        eprintln!(
            "  {}  {}  {}  {}",
            format!("{:<W$}", "time span").dimmed(),
            first.bold(),
            "→".dimmed(),
            last.bold()
        );
    }
    row("bytes in", &util::fmt_bytes(log.total_bytes_in));
    row("methods", &methods_str);
    eprintln!();
}
