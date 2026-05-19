// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

//! Interactive terminal UI: analysis selection menu and result rendering.
//!
//! The UI is split into focused submodules:
//! - [`pager`] — scrollable text viewer for `Table` output
//! - [`detail_viewer`] — full-screen viewer with ↑/↓ navigation between items
//! - [`selectable_list`] — dialoguer-based picker → detail viewer
//! - [`sortable_table`] — htop-style sortable table with click-to-sort columns
//! - [`chart`] — full-screen bar chart opened from a sortable-table preamble
//! - [`table_format`] — legacy `comfy-table` renderer for `Table` content

mod chart;
mod detail_viewer;
mod pager;
mod selectable_list;
mod sortable_table;
mod table_format;

pub(crate) use selectable_list::display_selectable_list_with_context;

/// Called after every TUI component exits to restore a clean normal-buffer state.
///
/// 1. Clears the normal terminal buffer (which may contain old progress bars or
///    menu history left behind when the alternate screen was active).
/// 2. Drains any input events that were buffered while in raw mode — most
///    importantly the Esc or 'q' that dismissed the TUI, which would otherwise
///    be silently consumed by the next `dialoguer::Select`, forcing the user to
///    press Enter an extra time before the menu appears.
///
/// Must be called *after* `LeaveAlternateScreen` and `disable_raw_mode`.
pub(super) fn restore_terminal() {
    use crossterm::{cursor, event, execute, terminal};
    use std::time::Duration;
    let mut stdout = std::io::stdout();
    let _ = execute!(
        stdout,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    );
    while event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}

use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::analysis::{AnalysisOutput, Registry};

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
        "What do you want to do?".bold(),
        "· esc or ctrl+c to quit".dimmed(),
    );
    let result = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("")
        .items(&items)
        .default(0)
        .interact_opt()
        .context("dialoguer interaction failed");

    match result {
        Err(_) => Ok(Selection::Interrupted),
        Ok(None) => Ok(Selection::Quit),
        Ok(Some(idx)) if idx + 1 == items.len() => Ok(Selection::Quit),
        Ok(Some(idx)) => Ok(Selection::Analysis(idx)),
    }
}

/// Render an analysis output. Returns normally — never exits the process.
///
/// - `Table` → scrollable pager.
/// - `SelectableList` → navigable list; Enter opens the item's detail.
/// - `SortableTable` → htop-style table; click a header to sort, click the
///   sparkline preamble to open a full-screen chart.
/// - `SubMenu` → handled by `lib.rs` before reaching this function.
pub fn display_output(output: &AnalysisOutput) {
    match output {
        AnalysisOutput::Table { .. } => {
            colored::control::set_override(true);
            let content = table_format::format_output(output);
            colored::control::unset_override();
            if let Err(e) = pager::run_pager(&content) {
                eprintln!("pager error: {e}");
                print!("{content}");
            }
        }
        AnalysisOutput::SelectableList {
            title,
            items,
            summary,
        } => {
            selectable_list::display_selectable_list(title, items, summary.as_deref());
        }
        AnalysisOutput::SortableTable {
            title,
            preamble,
            chart,
            columns,
            sortable,
            rows,
            summary,
        } => {
            sortable_table::display_sortable_table(
                title,
                preamble.as_deref(),
                chart.as_ref(),
                columns,
                sortable,
                rows,
                summary.as_deref(),
            );
        }
        AnalysisOutput::SubMenu { .. } => {} // routed by lib.rs before reaching here
    }
}
