//! Interactive sortable table: htop-style column-header click to sort, plus
//! optional click-on-preamble to open a full-screen [chart](super::chart).

use std::io::Write;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEventKind,
    },
    execute, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};

use crate::analysis::{ChartConfig, SortableRow};
use crate::util::truncate;

use super::chart::render_chart;

const MIN_CELL_WIDTH: usize = 20;

/// Display a sortable table.  Blocks until the user dismisses it.
pub(super) fn display_sortable_table(
    title: &str,
    preamble: Option<&str>,
    chart: Option<&ChartConfig>,
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    summary: Option<&str>,
) {
    if rows.is_empty() {
        eprintln!("\n  No data.");
        return;
    }

    let mut state = TableState {
        sort_col: sortable.first().copied().unwrap_or(0),
        sort_asc: false,
        scroll: 0,
        chart_mode: false,
    };

    let mut stdout = std::io::stdout();
    if terminal::enable_raw_mode().is_err() {
        return;
    }
    let _ = execute!(
        stdout,
        terminal::EnterAlternateScreen,
        cursor::Hide,
        EnableMouseCapture
    );

    let _ = run_loop(
        title,
        preamble,
        chart,
        columns,
        sortable,
        rows,
        summary,
        &mut state,
        &mut stdout,
    );

    let _ = execute!(
        stdout,
        DisableMouseCapture,
        terminal::LeaveAlternateScreen,
        cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}

/// Mutable scroll/sort state preserved across event-loop iterations.
struct TableState {
    sort_col: usize,
    sort_asc: bool,
    scroll: usize,
    chart_mode: bool,
}

#[allow(clippy::too_many_arguments)]
fn run_loop(
    title: &str,
    preamble: Option<&str>,
    chart: Option<&ChartConfig>,
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    summary: Option<&str>,
    state: &mut TableState,
    stdout: &mut impl Write,
) -> Result<()> {
    let preamble_lines: Vec<&str> = preamble
        .map(|p| p.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let preamble_h = preamble_lines.len();

    loop {
        let (term_cols, term_rows) = terminal::size().unwrap_or((160, 40));
        let term_rows = term_rows as usize;
        let term_cols = term_cols as usize;

        // ── Chart mode ───────────────────────────────────────────────────
        if state.chart_mode {
            if let Some(cfg) = chart {
                render_chart(cfg, title, stdout, term_rows, term_cols)?;
            }
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),
                        _ => state.chart_mode = false,
                    }
                }
                Event::Mouse(me) if matches!(me.kind, MouseEventKind::Down(_)) => {
                    state.chart_mode = false;
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
            continue;
        }

        // ── Table mode ───────────────────────────────────────────────────
        let order = sort_indices(rows, state.sort_col, state.sort_asc);
        let widths = compute_column_widths(columns, sortable, rows, term_cols);
        let col_x_ranges = column_x_ranges(&widths);

        // Layout: title(row 0), preamble(rows 1..preamble_h), header, separator, data…, footer.
        let header_y = 1 + preamble_h;
        let sep_y = header_y + 1;
        let data_start_y = sep_y + 1;
        let data_height = term_rows.saturating_sub(data_start_y + 1 /* footer */);
        let max_scroll = rows.len().saturating_sub(data_height);
        state.scroll = state.scroll.min(max_scroll);

        render_frame(
            title,
            &preamble_lines,
            columns,
            sortable,
            rows,
            summary,
            &widths,
            &order,
            header_y,
            sep_y,
            data_start_y,
            data_height,
            term_rows,
            term_cols,
            state,
            stdout,
        )?;

        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if handle_key_event(key.code, key.modifiers, state, data_height, max_scroll) {
                    return Ok(());
                }
            }
            Event::Mouse(me) => {
                handle_mouse_event(
                    me,
                    state,
                    chart,
                    preamble_h,
                    header_y,
                    sep_y,
                    &col_x_ranges,
                    sortable,
                    data_height,
                    max_scroll,
                );
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Returns `true` if the table should exit (q/Esc/Ctrl+C).
fn handle_key_event(
    code: KeyCode,
    mods: KeyModifiers,
    state: &mut TableState,
    data_height: usize,
    max_scroll: usize,
) -> bool {
    match (code, mods) {
        (KeyCode::Char('q'), _)
        | (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            return true;
        }
        (KeyCode::Up, _) => state.scroll = state.scroll.saturating_sub(1),
        (KeyCode::Down, _) => state.scroll = state.scroll.saturating_add(1).min(max_scroll),
        (KeyCode::PageUp, _) => state.scroll = state.scroll.saturating_sub(data_height),
        (KeyCode::PageDown, _) => {
            state.scroll = state.scroll.saturating_add(data_height).min(max_scroll)
        }
        (KeyCode::Home, _) => state.scroll = 0,
        (KeyCode::End, _) => state.scroll = max_scroll,
        _ => {}
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse_event(
    me: event::MouseEvent,
    state: &mut TableState,
    chart: Option<&ChartConfig>,
    preamble_h: usize,
    header_y: usize,
    sep_y: usize,
    col_x_ranges: &[(usize, usize)],
    sortable: &[usize],
    data_height: usize,
    max_scroll: usize,
) {
    match me.kind {
        // Left-click on the sparkline preamble → enter chart mode.
        MouseEventKind::Down(MouseButton::Left)
            if chart.is_some() && (me.row as usize) >= 1 && (me.row as usize) <= preamble_h =>
        {
            state.chart_mode = true;
        }
        // Left-click on header / separator → change sort column.
        MouseEventKind::Down(MouseButton::Left)
            if (me.row as usize) == header_y || (me.row as usize) == sep_y =>
        {
            let cx = me.column as usize;
            if let Some(ci) = col_x_ranges.iter().position(|&(s, e)| cx >= s && cx < e) {
                if ci > 0 {
                    let di = ci - 1;
                    if sortable.contains(&di) {
                        if state.sort_col == di {
                            state.sort_asc = !state.sort_asc;
                        } else {
                            state.sort_col = di;
                            state.sort_asc = false;
                        }
                        state.scroll = 0;
                    }
                }
            }
        }
        MouseEventKind::ScrollUp => {
            state.scroll = state.scroll.saturating_sub(3);
        }
        MouseEventKind::ScrollDown => {
            state.scroll = state.scroll.saturating_add(3).min(max_scroll);
        }
        _ => {}
    }
    let _ = data_height; // currently unused but kept for symmetry
}

/// Build a permutation of row indices sorted by `sort_col` (asc if `sort_asc`).
fn sort_indices(rows: &[SortableRow], sort_col: usize, sort_asc: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| {
        let ka = rows[a]
            .sort_keys
            .get(sort_col)
            .and_then(|k| *k)
            .unwrap_or(0);
        let kb = rows[b]
            .sort_keys
            .get(sort_col)
            .and_then(|k| *k)
            .unwrap_or(0);
        if sort_asc {
            ka.cmp(&kb)
        } else {
            kb.cmp(&ka)
        }
    });
    order
}

/// Compute the display width (in chars) of every column including the leading `#`.
fn compute_column_widths(
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    term_cols: usize,
) -> Vec<usize> {
    let mut widths: Vec<usize> = std::iter::once("#".chars().count())
        .chain(columns.iter().map(|c| c.chars().count()))
        .collect();
    // Reserve room for sort indicator " ▼" on sortable columns.
    for &si in sortable {
        widths[si + 1] = widths[si + 1].max(columns[si].chars().count() + 2);
    }
    // Expand for data.
    let rank_width = rows.len().to_string().len();
    widths[0] = widths[0].max(rank_width);
    for row in rows {
        for (i, cell) in row.cells.iter().enumerate() {
            widths[i + 1] = widths[i + 1].max(cell.chars().count());
        }
    }
    // Shrink the route column (col 2 = widths[2]) if the table overflows.
    let total: usize = widths.iter().map(|w| w + 3).sum::<usize>() + 1;
    if total > term_cols && widths.len() > 2 {
        let excess = total.saturating_sub(term_cols);
        widths[2] = widths[2].saturating_sub(excess).max(MIN_CELL_WIDTH);
    }
    widths
}

/// X-coordinate ranges of each cell (used for mouse-click → column mapping).
fn column_x_ranges(widths: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::with_capacity(widths.len());
    let mut x = 1usize; // skip leading │
    for &w in widths {
        ranges.push((x, x + w + 2));
        x += w + 3; // space + content + space + │
    }
    ranges
}

#[allow(clippy::too_many_arguments)]
fn render_frame(
    title: &str,
    preamble_lines: &[&str],
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    summary: Option<&str>,
    widths: &[usize],
    order: &[usize],
    header_y: usize,
    sep_y: usize,
    data_start_y: usize,
    data_height: usize,
    term_rows: usize,
    term_cols: usize,
    state: &TableState,
    stdout: &mut impl Write,
) -> Result<()> {
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        terminal::Clear(ClearType::All)
    )?;

    // Title.
    queue!(stdout, cursor::MoveTo(0, 0), Print(format!("  {title}")))?;

    // Preamble.
    for (i, line) in preamble_lines.iter().enumerate() {
        queue!(stdout, cursor::MoveTo(0, (1 + i) as u16), Print(line))?;
    }

    // Header row with sort indicators.
    let header_cells: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(ci, _)| build_header_cell(ci, columns, sortable, state))
        .collect();
    queue!(
        stdout,
        cursor::MoveTo(0, header_y as u16),
        Print(table_row(&header_cells, widths))
    )?;

    // Separator.
    queue!(
        stdout,
        cursor::MoveTo(0, sep_y as u16),
        Print(table_separator(widths))
    )?;

    // Data rows.
    for i in 0..data_height {
        let oi = state.scroll + i;
        if oi >= order.len() {
            break;
        }
        let ri = order[oi];
        let mut cells = vec![(oi + 1).to_string()];
        cells.extend(rows[ri].cells.iter().cloned());
        if cells.len() > 2 {
            cells[2] = truncate(&cells[2], widths[2]);
        }
        queue!(
            stdout,
            cursor::MoveTo(0, (data_start_y + i) as u16),
            Print(table_row(&cells, widths))
        )?;
    }

    // Summary (one line above footer).
    if let Some(s) = summary {
        let summary_y = term_rows.saturating_sub(2) as u16;
        queue!(
            stdout,
            cursor::MoveTo(0, summary_y),
            Print(format!("  {s}"))
        )?;
    }

    // Footer.
    let sort_name = columns
        .get(state.sort_col)
        .map(String::as_str)
        .unwrap_or("?");
    let pct = if rows.len() <= data_height {
        100
    } else {
        ((state.scroll + data_height) * 100 / rows.len()).min(100)
    };
    let footer_text = format!(
        " bloom-log-analyzer  │  click column header to sort  │  {sort_name} {}  │  ↑/↓ scroll  q/esc back  {pct}%",
        if state.sort_asc { "▲" } else { "▼" },
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
    Ok(())
}

fn build_header_cell(
    ci: usize,
    columns: &[String],
    sortable: &[usize],
    state: &TableState,
) -> String {
    if ci == 0 {
        return "#".to_string();
    }
    let di = ci - 1;
    let name = &columns[di];
    if sortable.contains(&di) {
        if di == state.sort_col {
            format!("{} {}", name, if state.sort_asc { "▲" } else { "▼" })
        } else {
            // Same width as "name ▼" so column widths don't jitter on re-sort.
            format!("{name}  ")
        }
    } else {
        name.clone()
    }
}

fn table_row(cells: &[String], widths: &[usize]) -> String {
    let mut s = String::from("│");
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(cell.chars().count());
        let clen = cell.chars().count();
        s.push(' ');
        s.push_str(cell);
        for _ in 0..w.saturating_sub(clen) {
            s.push(' ');
        }
        s.push_str(" │");
    }
    s
}

fn table_separator(widths: &[usize]) -> String {
    let mut s = String::from("├");
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 {
            s.push('─');
        }
        s.push(if i + 1 < widths.len() { '┼' } else { '┤' });
    }
    s
}
