//! Heaviest Requests: aggregates estimated byte sizes (headers + body) per
//! normalized route, ordered heaviest-total first.
//!
//! For every request entry the scanner sums every line's byte count — the
//! entry header line, all header lines, blank separator, and body — until the
//! `---` log separator or the next entry.  URLs are normalized so different
//! IDs collapse to the same route key.  The result is presented as a table
//! with total bytes, request count, average per request, and share of all
//! scanned bytes.

use std::fs::File;
use std::time::Duration;

use ahash::AHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::analysis::{Analysis, AnalysisOutput, DEFAULT_TOP_N};
use crate::log::{ParsedLog, RouteKey};
use crate::util::{fmt_bytes, fmt_count, fmt_pct};

static ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\]\s+([A-Z]+)\s+(/\S*)").expect("ENTRY_RE valid")
});

static ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:[a-zA-Z][a-zA-Z0-9_]*_)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    )
    .expect("ID_RE valid")
});

const PROGRESS_FLUSH_BYTES: u64 = 1_024 * 1_024;
const TICK_INTERVAL: Duration = Duration::from_millis(80);

pub struct HeaviestRequestsBySize {
    pub top_n: usize,
}

impl Default for HeaviestRequestsBySize {
    fn default() -> Self {
        Self { top_n: DEFAULT_TOP_N }
    }
}

impl Analysis for HeaviestRequestsBySize {
    fn name(&self) -> &'static str {
        "Heaviest Requests (headers + body byte size)"
    }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path {
            Some(p) => p.clone(),
            None => return error_output("No source file path available for re-scan."),
        };

        let sizes = match compute_sizes(&path) {
            Ok(s) => s,
            Err(e) => return error_output(&format!("Re-scan failed: {e}")),
        };

        // Total bytes across ALL routes (for % share column).
        let grand_total: usize = sizes.values().map(|(b, _)| b).sum();

        let mut entries: Vec<(RouteKey, usize, usize)> = sizes
            .into_iter()
            .map(|(key, (total, count))| (key, total, count))
            .collect();
        entries.sort_by(|a, b| b.1.cmp(&a.1)); // heaviest total first
        entries.truncate(self.top_n);

        let shown = entries.len();

        let rows = entries
            .into_iter()
            .enumerate()
            .map(|(i, (key, total, count))| {
                let avg = if count > 0 { total / count } else { 0 };
                vec![
                    (i + 1).to_string(),
                    key.method,
                    key.url,
                    fmt_bytes(total as u64),
                    fmt_pct(total, grand_total),
                    fmt_count(count),
                    fmt_bytes(avg as u64),
                ]
            })
            .collect();

        AnalysisOutput::Table {
            title: format!("Top {shown} Heaviest Requests  (estimated headers + body)"),
            columns: ["#", "method", "route", "total", "% of all", "requests", "avg / req"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            rows,
            summary: Some(format!(
                "Grand total estimated bytes (all routes): {}",
                fmt_bytes(grand_total as u64)
            )),
        }
    }
}

/// Sequential single-pass scan.  Accumulates line bytes into a per-route map
/// using a two-state machine: scanning (looking for entry headers) and
/// collecting (accumulating lines until `---` or the next entry).
fn compute_sizes(path: &std::path::Path) -> anyhow::Result<AHashMap<RouteKey, (usize, usize)>> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let pb = make_progress_bar(file_size);

    // SAFETY: read-only mapping; log file is not modified during analysis.
    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    // AHashMap<RouteKey, (total_bytes, request_count)>
    let mut sizes: AHashMap<RouteKey, (usize, usize)> = AHashMap::new();
    // Collecting state: (route_key, bytes_accumulated_so_far)
    let mut active: Option<(RouteKey, usize)> = None;

    let mut pending_bytes: u64 = 0;
    let mut req_count: usize = 0;

    for line_bytes in data.split(|&b| b == b'\n') {
        let line_len = line_bytes.len() + 1; // +1 for the consumed '\n'
        pending_bytes += line_len as u64;

        if pending_bytes >= PROGRESS_FLUSH_BYTES {
            pb.inc(pending_bytes);
            pending_bytes = 0;
            pb.set_message(format!(
                "{} reqs  {} routes",
                fmt_count(req_count),
                fmt_count(sizes.len()),
            ));
        }

        let is_sep = line_bytes.starts_with(b"---");
        let is_entry = !is_sep && line_bytes.first() == Some(&b'[');

        // ── Flush on separator or next entry header ─────────────────────
        if active.is_some() && (is_sep || is_entry) {
            let (key, bytes) = active.take().unwrap();
            let e = sizes.entry(key).or_insert((0, 0));
            e.0 += bytes;
            e.1 += 1;
            req_count += 1;
        }

        if is_sep {
            continue;
        }

        if let Some((_, ref mut bytes)) = active {
            // ── Collecting: accumulate header/body line ──────────────────
            *bytes += line_len;
        } else if is_entry {
            // ── Scanning: start collecting if entry line matches ─────────
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                if let Some(caps) = ENTRY_RE.captures(s) {
                    let normalized = normalize(&caps[3]);
                    let key = RouteKey::new(&caps[2], normalized);
                    active = Some((key, line_len));
                }
            }
        }
    }

    // Flush last entry (file may not end with `---`).
    if let Some((key, bytes)) = active.take() {
        let e = sizes.entry(key).or_insert((0, 0));
        e.0 += bytes;
        e.1 += 1;
        req_count += 1;
    }

    pb.inc(pending_bytes);
    pb.set_message(format!(
        "{} reqs  {} routes",
        fmt_count(req_count),
        fmt_count(sizes.len()),
    ));
    pb.finish_and_clear();

    Ok(sizes)
}

fn normalize(url: &str) -> String {
    let s = ID_RE.replace_all(url, ":any_id").into_owned();
    match s.find('?') {
        Some(pos) => s[..pos].to_string(),
        None => s,
    }
}

fn make_progress_bar(file_size: u64) -> ProgressBar {
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:45.cyan/238}] {percent:>3}%  {msg}  eta {eta}",
        )
        .expect("progress template valid")
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
        .progress_chars("█▓░"),
    );
    pb.set_message("0 reqs  0 routes");
    pb.enable_steady_tick(TICK_INTERVAL);
    pb
}

fn error_output(msg: &str) -> AnalysisOutput {
    AnalysisOutput::Table {
        title: "Heaviest Requests".into(),
        columns: vec![],
        rows: vec![],
        summary: Some(msg.into()),
    }
}
