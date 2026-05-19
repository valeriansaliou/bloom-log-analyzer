//! Traffic Timeline: requests per time bucket, designed to surface bursts.
//!
//! Re-scans the log file in parallel, buckets each entry by its timestamp,
//! then shows a sparkline overview and a `SortableTable` of busiest buckets
//! with the dominant route for each.  Default sort is burst-first (total
//! requests descending); click the `time` header to switch to chronological.

use std::fs::File;

use ahash::AHashMap;
use indicatif::ProgressBar;
use memmap2::Mmap;
use rayon::prelude::*;

use crate::analysis::{Analysis, AnalysisOutput, ChartConfig, SortableRow, DEFAULT_TOP_N};
use crate::log::{ParsedLog, RouteKey};
use crate::scanner::{normalize_url, ENTRY_RE, PROGRESS_FLUSH_BYTES};
use crate::util::{fmt_count, fmt_pct, split_into_chunks};

/// Width of the sparkline in terminal characters.
const SPARKLINE_WIDTH: usize = 72;
/// Unicode block characters for 8-level bar (▁ … █).
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

// ── Analysis ────────────────────────────────────────────────────────────────

pub struct TrafficTimeline;

impl Analysis for TrafficTimeline {
    fn name(&self) -> &'static str {
        "Traffic Timeline (burst detection)"
    }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let source_path = match &log.source_path {
            Some(p) => p.clone(),
            None => return error_output("No source file available for re-scan."),
        };

        // Determine time span from already-parsed first/last timestamps.
        let first_secs = match log.first_timestamp.as_deref().and_then(parse_timestamp) {
            Some(s) => s,
            None => return error_output("Could not parse timestamps from log."),
        };
        let last_secs = match log.last_timestamp.as_deref().and_then(parse_timestamp) {
            Some(s) => s,
            None => return error_output("Could not parse last timestamp from log."),
        };

        let span_secs = last_secs.saturating_sub(first_secs).max(1);
        let (bucket_size, bucket_label) = choose_bucket_size(span_secs);

        let buckets = match rescan(&source_path, first_secs, bucket_size) {
            Ok(b) => b,
            Err(e) => return error_output(&format!("Re-scan failed: {e}")),
        };
        if buckets.is_empty() {
            return error_output("No timestamped entries found.");
        }

        // Build sparkline from all bucket totals in chronological order.
        let max_bucket_idx = *buckets.keys().max().unwrap_or(&0);
        let counts: Vec<usize> = (0..=max_bucket_idx)
            .map(|i| buckets.get(&i).map(|b| b.total).unwrap_or(0))
            .collect();
        let spark = sparkline(&counts, SPARKLINE_WIDTH);

        let first_ts = log.first_timestamp.as_deref().unwrap_or("?");
        let last_ts = log.last_timestamp.as_deref().unwrap_or("?");
        let preamble = format!(
            "  {first_ts}  →  {last_ts}  ·  bucket: {bucket_label}  ·  {} buckets\n  {spark}  ← click to expand",
            fmt_count(buckets.len()),
        );

        // Build rows — pre-sorted burst-first.
        let mut bucket_vec: Vec<(u64, BucketData)> = buckets.into_iter().collect();
        bucket_vec.sort_by(|a, b| b.1.total.cmp(&a.1.total));
        bucket_vec.truncate(DEFAULT_TOP_N);

        let rows: Vec<SortableRow> = bucket_vec
            .into_iter()
            .map(|(bucket_idx, data)| {
                let bucket_start = first_secs + bucket_idx * bucket_size;
                let time_label = format_bucket_time(bucket_start, bucket_size);

                let (top_route, top_count) = data
                    .routes
                    .iter()
                    .max_by_key(|(_, &c)| c)
                    .map(|(k, &c)| (format!("{} {}", k.method, k.url), c))
                    .unwrap_or_else(|| ("—".into(), 0));

                let pct_scaled = (top_count as f64 / data.total.max(1) as f64 * 1_000_000.0) as u64;

                SortableRow {
                    cells: vec![
                        time_label,
                        fmt_count(data.total),
                        top_route,
                        fmt_count(top_count),
                        fmt_pct(top_count, data.total),
                    ],
                    sort_keys: vec![
                        Some(bucket_start),      // time (unix secs — chronological sort)
                        Some(data.total as u64), // total req
                        None,                    // top route (text)
                        Some(top_count as u64),  // route req
                        Some(pct_scaled),        // route %
                    ],
                }
            })
            .collect();

        let shown = rows.len();
        AnalysisOutput::SortableTable {
            title: format!("Traffic Timeline  —  top {shown} busiest buckets"),
            preamble: Some(preamble),
            chart: Some(ChartConfig {
                counts,
                y_axis_label: format!("requests / {bucket_label}"),
                x_start_label: format_bucket_time(first_secs, bucket_size),
                x_end_label: format_bucket_time(last_secs, bucket_size),
            }),
            columns: ["time", "total req", "top route", "route req", "route %"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sortable: vec![0, 1, 3, 4],
            rows,
            summary: Some("default: burst-first  ·  click 'time' to sort chronologically".into()),
        }
    }
}

// ── Parallel re-scan ────────────────────────────────────────────────────────

#[derive(Default)]
struct BucketData {
    total: usize,
    routes: AHashMap<RouteKey, usize>,
}

fn rescan(
    path: &std::path::Path,
    first_secs: u64,
    bucket_size: u64,
) -> anyhow::Result<AHashMap<u64, BucketData>> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let pb = make_progress_bar(file_size);

    // SAFETY: read-only; log file is not modified during analysis.
    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    let n = rayon::current_num_threads().max(1);
    let chunks = split_into_chunks(data, n);

    let partial: Vec<AHashMap<u64, BucketData>> = chunks
        .into_par_iter()
        .map(|chunk| {
            let mut local: AHashMap<u64, BucketData> = AHashMap::new();
            let mut pending: u64 = 0;

            for line in chunk.split(|&b| b == b'\n') {
                pending += line.len() as u64 + 1;
                if pending >= PROGRESS_FLUSH_BYTES {
                    pb.inc(pending);
                    pending = 0;
                }
                if line.first() != Some(&b'[') {
                    continue;
                }
                if let Ok(s) = std::str::from_utf8(line) {
                    if let Some(caps) = ENTRY_RE.captures(s) {
                        if let Some(secs) = parse_timestamp(&caps[1]) {
                            let bucket_idx = secs.saturating_sub(first_secs) / bucket_size;
                            let norm = normalize_url(&caps[3]);
                            let key = RouteKey::new(&caps[2], norm);
                            let b = local.entry(bucket_idx).or_default();
                            *b.routes.entry(key).or_insert(0) += 1;
                            b.total += 1;
                        }
                    }
                }
            }
            pb.inc(pending);
            local
        })
        .collect();

    // Merge partial maps.
    let mut all: AHashMap<u64, BucketData> = AHashMap::new();
    for local in partial {
        for (idx, data) in local {
            let e = all.entry(idx).or_default();
            e.total += data.total;
            for (route, count) in data.routes {
                *e.routes.entry(route).or_insert(0) += count;
            }
        }
    }

    pb.finish_and_clear();
    Ok(all)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Choose a bucket granularity that gives a meaningful number of data points.
fn choose_bucket_size(span_secs: u64) -> (u64, &'static str) {
    match span_secs {
        0..=600 => (1, "1 second"),
        601..=7_200 => (10, "10 seconds"),
        7_201..=86_400 => (60, "1 minute"),
        86_401..=604_800 => (300, "5 minutes"),
        _ => (3_600, "1 hour"),
    }
}

/// Parse an ISO 8601 UTC timestamp string to a Unix timestamp (seconds).
/// Supports the format `YYYY-MM-DDTHH:MM:SSZ` (ignores sub-seconds and tz offset).
fn parse_timestamp(ts: &str) -> Option<u64> {
    if ts.len() < 19 {
        return None;
    }
    let year: i64 = ts[0..4].parse().ok()?;
    let month: i64 = ts[5..7].parse().ok()?;
    let day: i64 = ts[8..10].parse().ok()?;
    let hour: i64 = ts[11..13].parse().ok()?;
    let min: i64 = ts[14..16].parse().ok()?;
    let sec: i64 = ts[17..19].parse().ok()?;

    // Gregorian JDN formula (works for all dates after 1582-10-15).
    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    let jdn = day + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;

    // JDN of 1970-01-01 is 2440588.
    let unix_days = jdn - 2440588;
    if unix_days < 0 {
        return None;
    }
    Some(unix_days as u64 * 86400 + hour as u64 * 3600 + min as u64 * 60 + sec as u64)
}

/// Inverse JDN: convert a Unix timestamp back to a formatted date/time string.
fn format_bucket_time(unix_secs: u64, bucket_size: u64) -> String {
    let days = unix_secs / 86400;
    let tod = unix_secs % 86400;
    let h = tod / 3600;
    let m = (tod % 3600) / 60;
    let s = tod % 60;

    // Gregorian calendar from JDN.
    let jdn = days + 2440588;
    let l = jdn + 68569;
    let n = 4 * l / 146097;
    #[allow(clippy::manual_div_ceil)] // Standard JDN-inverse algorithm — not a div_ceil.
    let l = l - (146097 * n + 3) / 4;
    let i = 4000 * (l + 1) / 1461001;
    let l = l - 1461 * i / 4 + 31;
    let j = 80 * l / 2447;
    let dd = l - 2447 * j / 80;
    let l = j / 11;
    let mm = j + 2 - 12 * l;
    let yy = 100 * (n - 49) + i + l;

    if bucket_size >= 3600 {
        format!("{yy:04}-{mm:02}-{dd:02} {h:02}:00")
    } else if bucket_size >= 60 {
        format!("{yy:04}-{mm:02}-{dd:02} {h:02}:{m:02}")
    } else {
        format!("{yy:04}-{mm:02}-{dd:02} {h:02}:{m:02}:{s:02}")
    }
}

/// Render `counts` as a Unicode sparkline of at most `max_width` characters.
fn sparkline(counts: &[usize], max_width: usize) -> String {
    let n = counts.len();
    if n == 0 {
        return String::new();
    }
    let max_val = *counts.iter().max().unwrap_or(&1);
    if max_val == 0 {
        return "─".repeat(n.min(max_width));
    }
    // How many buckets to fold into each display character.
    let n_chars = n.min(max_width).max(1);
    let agg = n.div_ceil(n_chars);
    // Actual displayable chars may be < n_chars when n_chars * agg > n,
    // which would push start past the end of the slice.
    let actual = n.div_ceil(agg);
    (0..actual)
        .map(|i| {
            let start = i * agg;
            let end = ((i + 1) * agg).min(n);
            let avg = counts[start..end].iter().sum::<usize>() / (end - start).max(1);
            let level = (avg as f64 / max_val as f64 * 7.0) as usize;
            BARS[level.min(7)]
        })
        .collect()
}

fn make_progress_bar(file_size: u64) -> ProgressBar {
    crate::scanner::make_progress_bar(file_size, "building timeline")
}

fn error_output(msg: &str) -> AnalysisOutput {
    AnalysisOutput::Table {
        title: "Traffic Timeline".into(),
        columns: vec![],
        rows: vec![],
        summary: Some(msg.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparkline_no_out_of_bounds() {
        // Regression: counts.len() not a multiple of agg caused start > len.
        let counts: Vec<usize> = (0..1228).map(|i| i % 100).collect();
        let s = sparkline(&counts, 72);
        assert!(!s.is_empty());
        assert!(s.chars().count() <= 72);
    }

    #[test]
    fn sparkline_fewer_buckets_than_width() {
        let counts = vec![10usize, 50, 30, 80, 20];
        let s = sparkline(&counts, 72);
        assert_eq!(s.chars().count(), 5);
    }

    #[test]
    fn parse_timestamp_known_date() {
        let secs = parse_timestamp("2026-05-17T07:56:03Z").unwrap();
        // 2026-05-17T00:00:00Z = 20590 days * 86400 = 1_778_976_000
        assert_eq!(secs, 1_778_976_000 + 7 * 3600 + 56 * 60 + 3);
    }
}
