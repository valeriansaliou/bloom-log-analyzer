//! Selectable list: dialoguer-based picker that opens each item in the
//! [`detail_viewer`](super::detail_viewer).

use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::analysis::ListItem;

use super::detail_viewer::run_detail_viewer;

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

pub(super) fn display_selectable_list(
    title: &str,
    items: &[ListItem],
    summary: Option<&str>,
) {
    display_selectable_list_impl(title, items, summary, None);
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
