//! Heaviest Requests: aggregates estimated byte sizes (headers + body) per
//! normalized route, ordered heaviest-total first.
//!
//! Tracks up to MAX_SIZES_SAMPLE individual request sizes per route to compute
//! the heaviest single request seen and the p95 size estimate.

use std::fs::File;
use std::time::Duration;

use ahash::AHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::analysis::{Analysis, AnalysisOutput, SortableRow, DEFAULT_TOP_N};
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

static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[^.@/\s?&=#%+]+(?:@|%40)[^.@/\s?&=#%+]+(?:\.[^.@/\s?&=#%+]+)+")
        .expect("EMAIL_RE valid")
});

static LONG_NUMBER_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{10,}").expect("LONG_NUMBER_RE valid")
});

const PROGRESS_FLUSH_BYTES: u64 = 1_024 * 1_024;
const TICK_INTERVAL: Duration = Duration::from_millis(80);
/// Max individual sizes kept per route for percentile computation.
const MAX_SIZES_SAMPLE: usize = 1_000;
/// Minimum samples required before reporting a p95 estimate.
const MIN_SAMPLES_FOR_P95: usize = 20;

struct RouteStats {
    total: usize,
    count: usize,
    max: usize,
    /// Up to MAX_SIZES_SAMPLE individual request sizes (first-seen sample).
    sizes: Vec<usize>,
}

impl RouteStats {
    fn record(&mut self, bytes: usize) {
        self.total += bytes;
        self.count += 1;
        if bytes > self.max {
            self.max = bytes;
        }
        if self.sizes.len() < MAX_SIZES_SAMPLE {
            self.sizes.push(bytes);
        }
    }
}

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

        let mut stats = match compute_sizes(&path) {
            Ok(s) => s,
            Err(e) => return error_output(&format!("Re-scan failed: {e}")),
        };

        // Grand total for % share column (before truncation).
        let grand_total: usize = stats.values().map(|s| s.total).sum();

        let mut entries: Vec<(RouteKey, RouteStats)> = stats.drain().collect();
        entries.sort_by(|a, b| b.1.total.cmp(&a.1.total));
        entries.truncate(self.top_n);

        let shown = entries.len();

        // columns does NOT include # — it is added automatically by the sortable table renderer.
        // sortable indices: total=2, %ofAll=3, requests=4, avg=5, heaviest=6, p95=7
        let rows = entries
            .into_iter()
            .map(|(key, mut s)| {
                let avg = if s.count > 0 { s.total / s.count } else { 0 };
                let p95_val = p95(&mut s.sizes);
                let pct_scaled = (s.total as f64 / grand_total.max(1) as f64 * 1_000_000.0) as u64;
                SortableRow {
                    cells: vec![
                        key.method,
                        key.url,
                        fmt_bytes(s.total as u64),
                        fmt_pct(s.total, grand_total),
                        fmt_count(s.count),
                        fmt_bytes(avg as u64),
                        fmt_bytes(s.max as u64),
                        p95_val.map_or_else(|| "n/a".into(), |v| fmt_bytes(v as u64)),
                    ],
                    sort_keys: vec![
                        None,                            // method
                        None,                            // route
                        Some(s.total as u64),            // total
                        Some(pct_scaled),                // % of all (scaled to u64)
                        Some(s.count as u64),            // requests
                        Some(avg as u64),                // avg / req
                        Some(s.max as u64),              // heaviest
                        Some(p95_val.unwrap_or(0) as u64), // p95
                    ],
                }
            })
            .collect();

        AnalysisOutput::SortableTable {
            title: format!("Top {shown} Heaviest Requests  (estimated headers + body)"),
            preamble: None,
            chart_data: None,
            chart_meta: None,
            columns: ["method", "route", "total", "% of all", "requests", "avg / req", "heaviest", "p95"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sortable: vec![2, 3, 4, 5, 6, 7],
            rows,
            summary: Some(format!(
                "sorted by total desc by default  ·  click any highlighted column header to re-sort  ·  p95 requires ≥{MIN_SAMPLES_FOR_P95} samples"
            )),
        }
    }
}

/// Compute p95 from a sample of sizes.  Returns `None` if there are fewer than
/// `MIN_SAMPLES_FOR_P95` data points (not enough for a meaningful estimate).
fn p95(sizes: &mut Vec<usize>) -> Option<usize> {
    if sizes.len() < MIN_SAMPLES_FOR_P95 {
        return None;
    }
    sizes.sort_unstable();
    let idx = ((sizes.len() as f64 - 1.0) * 0.95) as usize;
    Some(sizes[idx.min(sizes.len() - 1)])
}

/// Sequential single-pass scan.  Accumulates line bytes into a per-route map
/// using a two-state machine: scanning (looking for entry headers) and
/// collecting (accumulating lines until `---` or the next entry).
fn compute_sizes(path: &std::path::Path) -> anyhow::Result<AHashMap<RouteKey, RouteStats>> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let pb = make_progress_bar(file_size);

    // SAFETY: read-only mapping; log file is not modified during analysis.
    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    let mut sizes: AHashMap<RouteKey, RouteStats> = AHashMap::new();
    // Collecting state: (route_key, bytes_accumulated_so_far)
    let mut active: Option<(RouteKey, usize)> = None;

    let mut pending_bytes: u64 = 0;
    let mut req_count: usize = 0;

    for line_bytes in data.split(|&b| b == b'\n') {
        let line_len = line_bytes.len() + 1;
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

        if active.is_some() && (is_sep || is_entry) {
            let (key, bytes) = active.take().unwrap();
            sizes
                .entry(key)
                .or_insert_with(|| RouteStats { total: 0, count: 0, max: 0, sizes: Vec::new() })
                .record(bytes);
            req_count += 1;
        }

        if is_sep {
            continue;
        }

        if let Some((_, ref mut bytes)) = active {
            *bytes += line_len;
        } else if is_entry {
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                if let Some(caps) = ENTRY_RE.captures(s) {
                    let normalized = normalize(&caps[3]);
                    let key = RouteKey::new(&caps[2], normalized);
                    active = Some((key, line_len));
                }
            }
        }
    }

    if let Some((key, bytes)) = active.take() {
        sizes
            .entry(key)
            .or_insert_with(|| RouteStats { total: 0, count: 0, max: 0, sizes: Vec::new() })
            .record(bytes);
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
    let after_ids = ID_RE.replace_all(url, ":any_id");
    let after_emails = EMAIL_RE.replace_all(&after_ids, ":any_id");
    let after_numbers = LONG_NUMBER_RE.replace_all(&after_emails, ":any_id");
    let s = after_numbers.into_owned();
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
