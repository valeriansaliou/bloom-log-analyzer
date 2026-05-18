//! Interactive terminal UI: analysis selection menu and result rendering.

use std::fmt::Write as _; // writeln! on String
use std::io::Write; // stdout.flush()

use anyhow::Result;
use colored::Colorize;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use crossterm::{
    cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute, queue,
    style::{Attribute as StyleAttr, Print, SetAttribute},
    terminal::{self, ClearType},
};
use dialoguer::{theme::ColorfulTheme, Select};

use crate::analysis::{AnalysisOutput, ListItem, SortableRow, Registry};
use crate::util::{fmt_count, truncate};

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
        AnalysisOutput::SortableTable { title, preamble, chart_data, chart_meta, columns, sortable, rows, summary } => {
            let meta = chart_meta.as_ref().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()));
            display_sortable_table(title, preamble.as_deref(), chart_data.as_deref(), meta, columns, sortable, rows, summary.as_deref());
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
// Sortable table
// ---------------------------------------------------------------------------

fn display_sortable_table(
    title: &str,
    preamble: Option<&str>,
    chart_data: Option<&[usize]>,
    chart_meta: Option<(&str, &str, &str)>,
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    summary: Option<&str>,
) {
    if rows.is_empty() {
        eprintln!("\n  No data.");
        return;
    }

    let mut sort_col = sortable.first().copied().unwrap_or(0);
    let mut sort_asc = false;
    let mut scroll: usize = 0;

    let mut stdout = std::io::stdout();
    if terminal::enable_raw_mode().is_err() {
        return;
    }
    let _ = execute!(stdout, terminal::EnterAlternateScreen, cursor::Hide, EnableMouseCapture);

    let _ = sortable_table_loop(title, preamble, chart_data, chart_meta, columns, sortable, rows, summary,
        &mut sort_col, &mut sort_asc, &mut scroll, &mut stdout);

    let _ = execute!(stdout, DisableMouseCapture, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

fn sortable_table_loop(
    title: &str,
    preamble: Option<&str>,
    chart_data: Option<&[usize]>,
    chart_meta: Option<(&str, &str, &str)>,
    columns: &[String],
    sortable: &[usize],
    rows: &[SortableRow],
    summary: Option<&str>,
    sort_col: &mut usize,
    sort_asc: &mut bool,
    scroll: &mut usize,
    stdout: &mut impl Write,
) -> Result<()> {
    let preamble_lines: Vec<&str> = preamble
        .map(|p| p.lines().collect::<Vec<_>>())
        .unwrap_or_default();
    let preamble_h = preamble_lines.len();

    let mut chart_mode = false;

    loop {
        let (term_cols, term_rows) = terminal::size().unwrap_or((160, 40));
        let term_rows = term_rows as usize;
        let term_cols = term_cols as usize;

        // ── Chart mode ───────────────────────────────────────────────────
        if chart_mode {
            if let Some(data) = chart_data {
                render_chart(data, title, chart_meta, stdout, term_rows, term_cols)?;
            }
            // Any input dismisses the chart back to the table.
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),
                        _ => { chart_mode = false; }
                    }
                }
                // Only a press (not release/move/scroll) dismisses the chart.
                Event::Mouse(me) if matches!(me.kind, MouseEventKind::Down(_)) => {
                    chart_mode = false;
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
            continue;
        }

        // ── Table mode ───────────────────────────────────────────────────

        // Sorted index array — rebuilt each frame.
        let sc = *sort_col;
        let sa = *sort_asc;
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by(|&a, &b| {
            let ka = rows[a].sort_keys.get(sc).and_then(|k| *k).unwrap_or(0);
            let kb = rows[b].sort_keys.get(sc).and_then(|k| *k).unwrap_or(0);
            if sa { ka.cmp(&kb) } else { kb.cmp(&ka) }
        });

        // Column widths (in display chars, not bytes).
        // all_cols: # + columns
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
        // Shrink route column (col 2 = widths[2]) if table too wide.
        let total_table_w: usize = widths.iter().map(|w| w + 3).sum::<usize>() + 1;
        if total_table_w > term_cols {
            let excess = total_table_w.saturating_sub(term_cols);
            widths[2] = widths[2].saturating_sub(excess).max(MIN_CELL_WIDTH);
        }

        // Column X ranges for mouse-click detection (the full cell including spaces).
        // Row layout: │ cell0 │ cell1 │ …
        // Cell i occupies x ∈ [start, start + widths[i] + 2).
        let mut col_x_ranges: Vec<(usize, usize)> = Vec::new();
        {
            let mut x = 1usize; // skip leading │
            for &w in &widths {
                col_x_ranges.push((x, x + w + 2));
                x += w + 3; // space + content + space + │
            }
        }

        // Layout: title(row 0), preamble(rows 1..preamble_h), header, separator, data…, footer.
        let header_y = 1 + preamble_h;
        let sep_y    = header_y + 1;
        let data_start_y = sep_y + 1;
        let data_height = term_rows.saturating_sub(data_start_y + 1 /* footer */);
        let max_scroll = rows.len().saturating_sub(data_height);
        *scroll = (*scroll).min(max_scroll);

        queue!(stdout, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;

        // Title.
        queue!(stdout, cursor::MoveTo(0, 0), Print(format!("  {title}")))?;

        // Preamble.
        for (i, line) in preamble_lines.iter().enumerate() {
            queue!(stdout, cursor::MoveTo(0, (1 + i) as u16), Print(line))?;
        }

        // Header row with sort indicators.
        let header_cells: Vec<String> = widths.iter().enumerate().map(|(ci, _)| {
            if ci == 0 {
                "#".to_string()
            } else {
                let di = ci - 1;
                let name = &columns[di];
                if sortable.contains(&di) {
                    if di == *sort_col {
                        format!("{} {}", name, if *sort_asc { "▲" } else { "▼" })
                    } else {
                        format!("{}  ", name) // same width as "name ▼"
                    }
                } else {
                    name.clone()
                }
            }
        }).collect();
        queue!(stdout, cursor::MoveTo(0, header_y as u16),
            Print(table_row(&header_cells, &widths)))?;

        // Separator.
        queue!(stdout, cursor::MoveTo(0, sep_y as u16),
            Print(table_separator(&widths)))?;

        // Data rows.
        for i in 0..data_height {
            let oi = *scroll + i;
            if oi >= order.len() { break; }
            let ri = order[oi];
            let mut cells = vec![(oi + 1).to_string()];
            cells.extend(rows[ri].cells.iter().cloned());
            // Truncate the route column to its assigned width.
            if cells.len() > 2 {
                cells[2] = truncate(&cells[2], widths[2]);
            }
            queue!(stdout, cursor::MoveTo(0, (data_start_y + i) as u16),
                Print(table_row(&cells, &widths)))?;
        }

        // Footer.
        let sort_name = columns.get(*sort_col).map(String::as_str).unwrap_or("?");
        let pct = if rows.len() <= data_height { 100 }
                  else { ((*scroll + data_height) * 100 / rows.len()).min(100) };
        let footer_text = format!(
            " analgun  │  click column header to sort  │  {sort_name} {}  │  ↑/↓ scroll  q/esc back  {pct}%",
            if *sort_asc { "▲" } else { "▼" },
        );
        if let Some(s) = summary {
            // Show summary on the line just above footer when there is room.
            let summary_y = term_rows.saturating_sub(2) as u16;
            queue!(stdout, cursor::MoveTo(0, summary_y), Print(format!("  {}", s)))?;
        }
        let footer = format!("{:<width$}", footer_text, width = term_cols);
        queue!(stdout,
            cursor::MoveTo(0, (term_rows - 1) as u16),
            SetAttribute(StyleAttr::Reverse), Print(&footer), SetAttribute(StyleAttr::Reset)
        )?;
        stdout.flush()?;

        // Events.
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), _) | (KeyCode::Esc, _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(()),
                    (KeyCode::Up, _)   => { *scroll = scroll.saturating_sub(1); }
                    (KeyCode::Down, _) => { *scroll = scroll.saturating_add(1).min(max_scroll); }
                    (KeyCode::PageUp, _)   => { *scroll = scroll.saturating_sub(data_height); }
                    (KeyCode::PageDown, _) => { *scroll = scroll.saturating_add(data_height).min(max_scroll); }
                    (KeyCode::Home, _) => *scroll = 0,
                    (KeyCode::End, _)  => *scroll = max_scroll,
                    _ => {}
                }
            }
            Event::Mouse(me) => match me.kind {
                // Left-click on the sparkline preamble → enter chart mode.
                MouseEventKind::Down(MouseButton::Left)
                    if chart_data.is_some()
                        && me.row as usize >= 1
                        && me.row as usize <= preamble_h =>
                {
                    chart_mode = true;
                }
                // Left-click on header or separator → change sort column.
                MouseEventKind::Down(MouseButton::Left)
                    if (me.row as usize == header_y || me.row as usize == sep_y) =>
                {
                    let cx = me.column as usize;
                    if let Some(ci) = col_x_ranges.iter().position(|&(s, e)| cx >= s && cx < e) {
                        if ci > 0 {
                            let di = ci - 1;
                            if sortable.contains(&di) {
                                if *sort_col == di {
                                    *sort_asc = !*sort_asc;
                                } else {
                                    *sort_col = di;
                                    *sort_asc = false;
                                }
                                *scroll = 0;
                            }
                        }
                    }
                }
                MouseEventKind::ScrollUp   => { *scroll = scroll.saturating_sub(3); }
                MouseEventKind::ScrollDown => { *scroll = scroll.saturating_add(3).min(max_scroll); }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Render a full-screen bar chart from `counts` (one value per time bucket).
///
/// `meta` = `(y_axis_label, x_start_label, x_end_label)` for the legend.
fn render_chart(
    counts: &[usize],
    title: &str,
    meta: Option<(&str, &str, &str)>,
    stdout: &mut impl Write,
    term_rows: usize,
    term_cols: usize,
) -> Result<()> {
    let n = counts.len();
    let max_val = counts.iter().copied().max().unwrap_or(1).max(1);
    let peak_val = max_val;

    // Layout rows (from top):
    //  0          title + hint
    //  1          y-axis title  (e.g. "  requests / 1 minute")
    //  2 .. 2+H   chart rows
    //  2+H+1      x-axis line  (└──────)
    //  2+H+2      x-axis time labels
    //  term_rows-1 footer (reversed)
    const Y_W: usize = 10; // " 12,345 │" = 10 chars

    let chart_w = term_cols.saturating_sub(Y_W);
    // rows consumed by fixed elements: title(1) + y_title(1) + x_line(1) + x_labels(1) + footer(1) = 5
    let chart_h = term_rows.saturating_sub(5 + 1); // +1 blank between title and y_title

    if chart_h == 0 || chart_w == 0 {
        return Ok(());
    }

    // Aggregate bucket counts to fit chart_w display columns.
    let agg = (n + chart_w - 1) / chart_w.max(1);
    let actual_cols = (n + agg - 1) / agg.max(1);
    let col_vals: Vec<usize> = (0..actual_cols)
        .map(|i| {
            let s = i * agg;
            let e = ((i + 1) * agg).min(n);
            counts[s..e].iter().sum::<usize>() / (e - s).max(1)
        })
        .collect();

    queue!(stdout, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;

    // Row 0: title + keyboard hint.
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        Print(format!("  {title}  ·  click or any key to return"))
    )?;

    // Row 1: y-axis title (left-aligned, indented to align with bars).
    let y_title = meta.map(|(y, _, _)| y).unwrap_or("requests / bucket");
    queue!(
        stdout,
        cursor::MoveTo(0, 1),
        Print(format!("{:>Y_W$}  {y_title}", "▲"))
    )?;

    // Rows 2 .. 2+chart_h: bars + y-axis labels.
    let chart_top_row: usize = 2;
    for row in 0..chart_h {
        let y = (chart_top_row + row) as u16;

        // Y-axis label at top, every quarter, and at the bottom.
        let show_label = row == 0
            || row == chart_h / 4
            || row == chart_h / 2
            || row == 3 * chart_h / 4
            || row + 1 == chart_h;
        let count_at_row = max_val * (chart_h - row) / chart_h;
        let label = if show_label {
            format!("{:>8} │", fmt_count(count_at_row))
        } else {
            "         │".to_string()
        };
        queue!(stdout, cursor::MoveTo(0, y), Print(&label))?;

        // Bar cells.
        let mut line = String::with_capacity(chart_w);
        for col in 0..chart_w {
            let val = col_vals.get(col).copied().unwrap_or(0);
            let bar_h = (val as f64 / max_val as f64 * chart_h as f64).round() as usize;
            line.push(if chart_h - row <= bar_h { '█' } else { ' ' });
        }
        queue!(stdout, cursor::MoveTo(Y_W as u16, y), Print(&line))?;
    }

    // X-axis separator line.
    let x_line_row = (chart_top_row + chart_h) as u16;
    queue!(
        stdout,
        cursor::MoveTo(0, x_line_row),
        Print(format!("         └{}▶", "─".repeat(chart_w.saturating_sub(1))))
    )?;

    // X-axis time labels: start (left) and end (right).
    let x_label_row = x_line_row + 1;
    if let Some((_, x_start, x_end)) = meta {
        let start_x = Y_W as u16;
        // Right-align the end label.
        let end_x = (term_cols.saturating_sub(x_end.len())) as u16;
        queue!(stdout, cursor::MoveTo(start_x, x_label_row), Print(x_start))?;
        if end_x > start_x + x_start.len() as u16 + 2 {
            queue!(stdout, cursor::MoveTo(end_x, x_label_row), Print(x_end))?;
        }
        // Time axis arrow label.
        let time_label_x = (start_x as usize + x_start.len() + 2) as u16;
        let end_label_x = end_x.saturating_sub(5) as u16;
        if time_label_x < end_label_x {
            queue!(stdout, cursor::MoveTo(time_label_x, x_label_row), Print("time →"))?;
        }
    }

    // Footer: peak value + bucket info + dismiss hint.
    let peak_info = meta
        .map(|(y, _, _)| format!("peak: {}  ·  y-axis: {}", fmt_count(peak_val), y))
        .unwrap_or_else(|| format!("peak: {}", fmt_count(peak_val)));
    let footer = format!(
        "{:<width$}",
        format!(" analgun  │  {peak_info}  │  click or any key to return"),
        width = term_cols
    );
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

/// Render one table row given display cells and column widths.
fn table_row(cells: &[String], widths: &[usize]) -> String {
    let mut s = String::from("│");
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(cell.chars().count());
        let clen = cell.chars().count();
        s.push(' ');
        s.push_str(cell);
        for _ in 0..w.saturating_sub(clen) { s.push(' '); }
        s.push_str(" │");
    }
    s
}

/// Render the separator line between the header and data rows.
fn table_separator(widths: &[usize]) -> String {
    let mut s = String::from("├");
    for (i, &w) in widths.iter().enumerate() {
        for _ in 0..w + 2 { s.push('─'); }
        s.push(if i + 1 < widths.len() { '┼' } else { '┤' });
    }
    s
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
        AnalysisOutput::SortableTable { .. } => String::new(), // handled by display_sortable_table
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
