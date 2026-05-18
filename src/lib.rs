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

use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::analysis::Registry;
use crate::ui::Selection;

/// Top-level entry point: parse a log file, then run the interactive menu loop
/// until the user picks "Quit".
pub fn run(log_file: &Path) -> Result<()> {
    let log = parser::parse_file(log_file)
        .with_context(|| format!("Failed to read log file: {}", log_file.display()))?;

    eprintln!(
        "{}  {} requests parsed, {} unique identifiers found",
        "✓".green().bold(),
        util::fmt_count(log.total_requests).bold(),
        util::fmt_count(log.identifier_counts.len()).bold(),
    );

    if log.total_requests == 0 {
        eprintln!("{}", "No requests found in log file.".yellow());
        return Ok(());
    }

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
