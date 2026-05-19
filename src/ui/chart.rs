// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

//! Full-screen bar chart drawn over the alternate screen.  Opened from a
//! [`SortableTable`](crate::analysis::AnalysisOutput::SortableTable) when the
//! user clicks the sparkline preamble.

use std::io::Write;

use anyhow::Result;
use crossterm::{
    cursor, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};

use crate::analysis::ChartConfig;
use crate::util::fmt_count;

/// Width of the y-axis label area in characters: `" 12,345 │"`.
const Y_W: usize = 10;
/// Total rows reserved by fixed layout elements (title, y-title, x-line,
/// x-labels, footer, blank line above y-title).
const FIXED_ROWS: usize = 5 + 1;

/// Render a full-screen bar chart based on `cfg`.
pub(super) fn render_chart(
    cfg: &ChartConfig,
    title: &str,
    stdout: &mut impl Write,
    term_rows: usize,
    term_cols: usize,
) -> Result<()> {
    let counts = &cfg.counts;
    let n = counts.len();
    let max_val = counts.iter().copied().max().unwrap_or(1).max(1);
    let peak_val = max_val;

    let chart_w = term_cols.saturating_sub(Y_W);
    let chart_h = term_rows.saturating_sub(FIXED_ROWS);

    if chart_h == 0 || chart_w == 0 {
        return Ok(());
    }

    // Aggregate bucket counts to fit chart_w display columns.
    let col_vals = aggregate(counts, chart_w);

    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;

    // Row 0: title + keyboard hint.
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        Print(format!("  {title}  ·  click or any key to return"))
    )?;

    // Row 1: y-axis title.
    queue!(
        stdout,
        cursor::MoveTo(0, 1),
        Print(format!("{:>Y_W$}  {}", "▲", cfg.y_axis_label))
    )?;

    // Rows 2..2+chart_h: bars + y-axis labels.
    let chart_top: usize = 2;
    for row in 0..chart_h {
        let y = (chart_top + row) as u16;
        queue!(
            stdout,
            cursor::MoveTo(0, y),
            Print(y_axis_label(row, chart_h, max_val))
        )?;
        queue!(
            stdout,
            cursor::MoveTo(Y_W as u16, y),
            Print(bar_row(&col_vals, chart_w, row, chart_h, max_val))
        )?;
    }

    // X-axis separator line.
    let x_line_row = (chart_top + chart_h) as u16;
    queue!(
        stdout,
        cursor::MoveTo(0, x_line_row),
        Print(format!(
            "         └{}▶",
            "─".repeat(chart_w.saturating_sub(1))
        ))
    )?;

    // X-axis time labels.
    render_x_axis_labels(stdout, cfg, x_line_row + 1, term_cols)?;

    // Footer.
    let footer_text = format!(
        " bloom-log-analyzer  │  peak: {}  ·  y-axis: {}  │  click or any key to return",
        fmt_count(peak_val),
        cfg.y_axis_label,
    );
    let footer = format!("{:<width$}", footer_text, width = term_cols);
    queue!(
        stdout,
        cursor::MoveTo(0, (term_rows - 1) as u16),
        SetAttribute(StyleAttr::Reverse),
        Print(&footer),
        SetAttribute(StyleAttr::Reset),
    )?;

    stdout.flush()?;
    let _ = n; // silence unused — kept for clarity at top
    Ok(())
}

/// Aggregate `counts` into at most `width` display values, averaging each
/// bucket of `agg = ceil(len / width)` raw counts.
fn aggregate(counts: &[usize], width: usize) -> Vec<usize> {
    let n = counts.len();
    if n == 0 || width == 0 {
        return Vec::new();
    }
    let agg = n.div_ceil(width);
    let cols = n.div_ceil(agg);
    (0..cols)
        .map(|i| {
            let s = i * agg;
            let e = ((i + 1) * agg).min(n);
            counts[s..e].iter().sum::<usize>() / (e - s).max(1)
        })
        .collect()
}

/// Y-axis label for chart row `row` (0 = top). Shown only at top, quarters,
/// and bottom; other rows just show the vertical bar `│`.
fn y_axis_label(row: usize, chart_h: usize, max_val: usize) -> String {
    let show = row == 0
        || row == chart_h / 4
        || row == chart_h / 2
        || row == 3 * chart_h / 4
        || row + 1 == chart_h;
    let count_at_row = max_val * (chart_h - row) / chart_h;
    if show {
        format!("{:>8} │", fmt_count(count_at_row))
    } else {
        "         │".to_string()
    }
}

/// Build the bar segment for one chart row (left to right).
fn bar_row(col_vals: &[usize], width: usize, row: usize, chart_h: usize, max_val: usize) -> String {
    let mut line = String::with_capacity(width);
    for col in 0..width {
        let val = col_vals.get(col).copied().unwrap_or(0);
        let bar_h = (val as f64 / max_val as f64 * chart_h as f64).round() as usize;
        line.push(if chart_h - row <= bar_h { '█' } else { ' ' });
    }
    line
}

/// Draw the x-axis time labels (start left, "time →" middle, end right).
fn render_x_axis_labels(
    stdout: &mut impl Write,
    cfg: &ChartConfig,
    row: u16,
    term_cols: usize,
) -> Result<()> {
    let start_x = Y_W as u16;
    let end_x = term_cols.saturating_sub(cfg.x_end_label.len()) as u16;

    queue!(
        stdout,
        cursor::MoveTo(start_x, row),
        Print(&cfg.x_start_label)
    )?;
    if end_x > start_x + cfg.x_start_label.len() as u16 + 2 {
        queue!(stdout, cursor::MoveTo(end_x, row), Print(&cfg.x_end_label))?;
    }
    let time_label_x = (start_x as usize + cfg.x_start_label.len() + 2) as u16;
    let end_label_x = end_x.saturating_sub(5);
    if time_label_x < end_label_x {
        queue!(stdout, cursor::MoveTo(time_label_x, row), Print("time →"))?;
    }
    Ok(())
}
