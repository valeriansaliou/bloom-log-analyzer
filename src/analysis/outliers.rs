//! Outlier request detection — five focused sub-analyses exposed through a
//! sub-menu.  Each sub-analysis re-scans the source file sequentially with a
//! shared state-machine scanner and a per-type detection closure.

use std::fs::File;
use std::time::Duration;

use ahash::AHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::analysis::{Analysis, AnalysisOutput, ListItem, DEFAULT_TOP_N};
use crate::log::{ParsedLog, RouteKey};
use crate::util::{fmt_bytes, fmt_count, truncate};

// ── Regexes ────────────────────────────────────────────────────────────────

static ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:[a-zA-Z][a-zA-Z0-9_]*_)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    )
    .expect("ID_RE valid")
});

static ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\]\s+([A-Z]+)\s+(/\S*)").expect("ENTRY_RE valid")
});

// ── Constants ──────────────────────────────────────────────────────────────

const MAX_HITS: usize = DEFAULT_TOP_N;
/// For RareUrl: max raw-entry examples collected per route pattern.
const MAX_EXAMPLES_PER_ROUTE: usize = 5;
/// content-length threshold for LargeRequest (bytes).
const LARGE_REQUEST_THRESHOLD: u64 = 100_000;
/// Single header-line length threshold for LargeHeader (bytes).
const LARGE_HEADER_THRESHOLD: usize = 2_000;
/// Cookie headers are legitimately large; only flag above this limit.
const LARGE_COOKIE_THRESHOLD: usize = 8_000;
/// Query-string length threshold for LargeQueryString (chars).
const LARGE_QUERY_THRESHOLD: usize = 200;
/// Max URL chars shown in navigation list labels.
const LABEL_URL_MAX: usize = 55;

const PROGRESS_FLUSH_BYTES: u64 = 1_024 * 1_024;
const TICK_INTERVAL: Duration = Duration::from_millis(80);

// ── Sub-menu entry point ────────────────────────────────────────────────────

pub struct OutlierRequests;

impl Analysis for OutlierRequests {
    fn name(&self) -> &'static str {
        "Outlier Requests (sub-menu)"
    }

    fn run(&self, _log: &ParsedLog) -> AnalysisOutput {
        AnalysisOutput::SubMenu {
            title: "Outlier Requests — detection type".into(),
            options: vec![
                (format!("Large Request       content-length > {} KB", LARGE_REQUEST_THRESHOLD / 1_000), Box::new(LargeRequest) as Box<dyn Analysis>),
                (format!("Large Header        single header line > {} KB", LARGE_HEADER_THRESHOLD / 1_000), Box::new(LargeHeader)),
                (format!("Large Query String  query part > {} chars", LARGE_QUERY_THRESHOLD), Box::new(LargeQueryString)),
                ("Anomalous Header    non-standard characters in header name".into(), Box::new(AnomalousHeaderName)),
                ("Rare URL            route pattern with unusually low traffic".into(), Box::new(RareUrl)),
            ],
        }
    }
}

// ── Sub-analysis: Large Request ─────────────────────────────────────────────

struct LargeRequest;

impl Analysis for LargeRequest {
    fn name(&self) -> &'static str { "Large Request" }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path { Some(p) => p.clone(), None => return no_source() };
        let hits = rescan_generic(
            &path,
            MAX_HITS,
            // Start collecting every entry; the content-length check runs in line_update.
            &mut |_ts, _method, _url| Some(String::new()),
            &mut |line, _in_headers, reason| {
                // content-length is always in headers, but we don't need to guard:
                // the pattern won't match body lines anyway.
                if reason.is_empty() {
                    if let Some(n) = parse_content_length(line) {
                        if n > LARGE_REQUEST_THRESHOLD {
                            *reason = format!(
                                "content-length: {} ({} KB)",
                                fmt_count(n as usize),
                                n / 1_000
                            );
                        }
                    }
                }
            },
        ).unwrap_or_default();

        hits_to_list(
            hits,
            &format!("Large Requests  (content-length > {} KB)", LARGE_REQUEST_THRESHOLD / 1_000),
        )
    }
}

// ── Sub-analysis: Large Header ──────────────────────────────────────────────

struct LargeHeader;

impl Analysis for LargeHeader {
    fn name(&self) -> &'static str { "Large Header" }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path { Some(p) => p.clone(), None => return no_source() };
        let hits = rescan_generic(
            &path,
            MAX_HITS,
            &mut |_ts, _method, _url| Some(String::new()),
            &mut |line, in_headers, reason| {
                if !in_headers || !reason.is_empty() || line.len() <= LARGE_HEADER_THRESHOLD {
                    return;
                }
                if let Some(col) = line.find(':') {
                    let name = &line[..col];
                    if name.is_empty() || name.contains(' ') {
                        return;
                    }
                    // Cookies are legitimately large; only flag above the higher threshold.
                    if name.eq_ignore_ascii_case("cookie")
                        && line.len() <= LARGE_COOKIE_THRESHOLD
                    {
                        return;
                    }
                    *reason = format!(
                        "header '{}': {} bytes",
                        truncate(name, 30),
                        fmt_count(line.len())
                    );
                }
            },
        ).unwrap_or_default();

        hits_to_list(
            hits,
            &format!("Large Headers  (single header line > {} KB)", LARGE_HEADER_THRESHOLD / 1_000),
        )
    }
}

// ── Sub-analysis: Large Query String ───────────────────────────────────────

struct LargeQueryString;

impl Analysis for LargeQueryString {
    fn name(&self) -> &'static str { "Large Query String" }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path { Some(p) => p.clone(), None => return no_source() };
        let hits = rescan_generic(
            &path,
            MAX_HITS,
            // Detect from the URL on the entry line — no need to inspect headers.
            &mut |_ts, _method, url| {
                if let Some(q) = url.find('?') {
                    let qlen = url.len() - q - 1;
                    if qlen > LARGE_QUERY_THRESHOLD {
                        return Some(format!(
                            "query string: {} chars",
                            fmt_count(qlen)
                        ));
                    }
                }
                None
            },
            &mut |_line, _in_headers, _reason| {}, // detected from URL, no header inspection
        ).unwrap_or_default();

        hits_to_list(
            hits,
            &format!("Large Query Strings  (query part > {} chars)", LARGE_QUERY_THRESHOLD),
        )
    }
}

// ── Sub-analysis: Anomalous Header Name ────────────────────────────────────

struct AnomalousHeaderName;

impl Analysis for AnomalousHeaderName {
    fn name(&self) -> &'static str { "Anomalous Header Name" }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path { Some(p) => p.clone(), None => return no_source() };
        let hits = rescan_generic(
            &path,
            MAX_HITS,
            &mut |_ts, _method, _url| Some(String::new()),
            &mut |line, in_headers, reason| {
                if !in_headers || !reason.is_empty() {
                    return; // skip body lines entirely
                }
                if let Some(col) = line.find(':') {
                    let name = &line[..col];
                    if name.is_empty() {
                        return;
                    }
                    if name.contains(' ') {
                        *reason = format!(
                            "header name '{}' contains spaces (possible injection)",
                            truncate(name, 40)
                        );
                    } else {
                        // Flag names with anything outside [a-zA-Z0-9\-_].
                        let has_anomaly = name.bytes().any(|b| {
                            !b.is_ascii_alphanumeric() && b != b'-' && b != b'_'
                        });
                        if has_anomaly {
                            *reason = format!(
                                "header name '{}' contains non-standard chars",
                                truncate(name, 40)
                            );
                        }
                    }
                }
            },
        ).unwrap_or_default();

        hits_to_list(hits, "Anomalous Header Names  (non-standard chars in header name)")
    }
}

// ── Sub-analysis: Rare URL ──────────────────────────────────────────────────
//
// Uses normalized route patterns (UUIDs replaced with :any_id) so that
// e.g. /v1/site/abc-uuid/debug and /v1/site/def-uuid/debug both count
// toward the same route.  Only the route PATTERN rarity matters.

struct RareUrl;

impl Analysis for RareUrl {
    fn name(&self) -> &'static str { "Rare URL" }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let path = match &log.source_path { Some(p) => p.clone(), None => return no_source() };

        let threshold = outlier_threshold(log);
        // Build a map: normalized RouteKey → global call count, for outlier routes only.
        let outlier_keys: AHashMap<RouteKey, usize> = log
            .route_counts
            .iter()
            .filter(|(_, &count)| count <= threshold)
            .map(|(key, &count)| (key.clone(), count))
            .collect();

        if outlier_keys.is_empty() {
            return no_results("No outlier routes detected.");
        }

        let hits = rescan_rare_url(&path, &outlier_keys, log.total_requests)
            .unwrap_or_default();

        hits_to_list(
            hits,
            &format!(
                "Rare URLs  (route pattern with ≤{threshold} occurrences — bottom 5th percentile)"
            ),
        )
    }
}

// ── Scanner: generic sequential re-scan ────────────────────────────────────

/// A single matched entry returned by the scanner.
struct ScanHit {
    timestamp: String,
    method: String,
    raw_url: String,
    hit_reason: String,
    full_entry: String,
}

/// Sequential single-pass file scanner.  Uses a two-state machine:
/// - **Scanning**: calls `entry_start(ts, method, url)` on each entry header.
///   Returns `Some(initial_reason)` to start collecting; `None` to skip.
/// - **Collecting**: calls `line_update(line, in_headers, &mut reason)` on
///   every subsequent line.  `in_headers` is `true` until the blank line that
///   separates HTTP headers from the body; detectors must guard on it to avoid
///   treating body content as headers.  The entry is saved only when `reason`
///   is non-empty at flush time.
fn rescan_generic(
    path: &std::path::Path,
    max_hits: usize,
    entry_start: &mut impl FnMut(&str, &str, &str) -> Option<String>,
    line_update: &mut impl FnMut(&str, bool, &mut String),
) -> anyhow::Result<Vec<ScanHit>> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let pb = make_progress_bar(file_size);

    // SAFETY: read-only mapping; log file is not modified during analysis.
    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    let mut hits: Vec<ScanHit> = Vec::new();
    // Active-collection state: (ts, method, raw_url, lines, hit_reason, bytes, in_headers).
    let mut active: Option<(String, String, String, Vec<String>, String, usize, bool)> = None;

    let mut pending_bytes: u64 = 0;
    let mut done_bytes: usize = 0;

    for line_bytes in data.split(|&b| b == b'\n') {
        pending_bytes += line_bytes.len() as u64 + 1;
        if pending_bytes >= PROGRESS_FLUSH_BYTES {
            pb.inc(pending_bytes);
            pending_bytes = 0;
            pb.set_message(progress_msg(hits.len(), done_bytes));
        }

        let is_sep = line_bytes.starts_with(b"---");
        let is_entry = !is_sep && line_bytes.first() == Some(&b'[');

        // ── Flush on separator or next entry header ─────────────────────
        if active.is_some() && (is_sep || is_entry) {
            let (ts, method, raw_url, lines, hit_reason, _bytes, _) = active.take().unwrap();
            if !hit_reason.is_empty() && hits.len() < max_hits {
                let full_entry = lines.join("\n");
                done_bytes += full_entry.len();
                hits.push(ScanHit { timestamp: ts, method, raw_url, hit_reason, full_entry });
            }
        }

        if is_sep {
            continue;
        }

        if let Some((_, _, _, ref mut lines, ref mut reason, ref mut bytes, ref mut in_headers)) =
            active
        {
            // ── Collecting: track header/body boundary, run detection ────
            let is_blank = line_bytes.is_empty() || line_bytes == b"\r";
            if is_blank {
                *in_headers = false;
            }
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                *bytes += s.len() + 1;
                line_update(s, *in_headers, reason);
                lines.push(s.to_string());
            }
        } else if is_entry && hits.len() < max_hits {
            // ── Scanning: test entry header ──────────────────────────────
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                if let Some(caps) = ENTRY_RE.captures(s) {
                    if let Some(initial_reason) = entry_start(&caps[1], &caps[2], &caps[3]) {
                        active = Some((
                            caps[1].to_string(),
                            caps[2].to_string(),
                            caps[3].to_string(),
                            vec![s.to_string()],
                            initial_reason,
                            s.len() + 1,
                            true, // start in header section
                        ));
                    }
                }
            }
        }
    }

    // Flush the last entry if the file has no trailing separator.
    if let Some((ts, method, raw_url, lines, hit_reason, _, _)) = active.take() {
        if !hit_reason.is_empty() && hits.len() < max_hits {
            let full_entry = lines.join("\n");
            done_bytes += full_entry.len();
            hits.push(ScanHit { timestamp: ts, method, raw_url, hit_reason, full_entry });
        }
    }

    pb.inc(pending_bytes);
    pb.set_message(progress_msg(hits.len(), done_bytes));
    pb.finish_and_clear();

    Ok(hits)
}

// ── Scanner: RareUrl (needs per-route limiting) ─────────────────────────────

fn rescan_rare_url(
    path: &std::path::Path,
    outlier_keys: &AHashMap<RouteKey, usize>,
    total_requests: usize,
) -> anyhow::Result<Vec<ScanHit>> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let pb = make_progress_bar(file_size);

    let mmap = unsafe { Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    let mut hits: Vec<ScanHit> = Vec::new();
    let mut per_route: AHashMap<String, usize> = AHashMap::new();
    let mut active: Option<(String, String, String, String, usize, Vec<String>, usize)> = None;
    // (ts, method, raw_url, normalized_url, route_total, lines, bytes)

    let mut pending_bytes: u64 = 0;
    let mut done_bytes: usize = 0;

    for line_bytes in data.split(|&b| b == b'\n') {
        pending_bytes += line_bytes.len() as u64 + 1;
        if pending_bytes >= PROGRESS_FLUSH_BYTES {
            pb.inc(pending_bytes);
            pending_bytes = 0;
            pb.set_message(progress_msg(hits.len(), done_bytes));
        }

        let is_sep = line_bytes.starts_with(b"---");
        let is_entry = !is_sep && line_bytes.first() == Some(&b'[');

        if active.is_some() && (is_sep || is_entry) {
            let (ts, method, raw_url, norm_url, route_total, lines, _) = active.take().unwrap();
            let share = fmt_share(route_total, total_requests);
            let full_entry = lines.join("\n");
            done_bytes += full_entry.len();
            hits.push(ScanHit {
                timestamp: ts,
                method: method.clone(),
                raw_url,
                hit_reason: format!(
                    "route {} {} seen {} of {} ({} of traffic)",
                    method,
                    norm_url,
                    fmt_count(route_total),
                    fmt_count(total_requests),
                    share,
                ),
                full_entry,
            });
        }

        if is_sep { continue; }

        if let Some((_, _, _, _, _, ref mut lines, ref mut bytes)) = active {
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                *bytes += s.len() + 1;
                lines.push(s.to_string());
            }
        } else if is_entry && hits.len() < MAX_HITS {
            if let Ok(s) = std::str::from_utf8(line_bytes) {
                if let Some(caps) = ENTRY_RE.captures(s) {
                    let method = &caps[2];
                    let raw_url = &caps[3];
                    let normalized = normalize(raw_url);
                    let key = RouteKey::new(method, &normalized);
                    if let Some(&route_total) = outlier_keys.get(&key) {
                        let route_id = format!("{method} {normalized}");
                        let cnt = per_route.entry(route_id).or_insert(0);
                        if *cnt < MAX_EXAMPLES_PER_ROUTE {
                            *cnt += 1;
                            active = Some((
                                caps[1].to_string(),
                                method.to_string(),
                                raw_url.to_string(),
                                normalized,
                                route_total,
                                vec![s.to_string()],
                                s.len() + 1,
                            ));
                        }
                    }
                }
            }
        }
    }

    if let Some((ts, method, raw_url, norm_url, route_total, lines, _)) = active.take() {
        let share = fmt_share(route_total, total_requests);
        let full_entry = lines.join("\n");
        done_bytes += full_entry.len();
        hits.push(ScanHit {
            timestamp: ts,
            method: method.clone(),
            raw_url,
            hit_reason: format!(
                "route {} {} seen {} of {} ({} of traffic)",
                method,
                norm_url,
                fmt_count(route_total),
                fmt_count(total_requests),
                share,
            ),
            full_entry,
        });
    }

    pb.inc(pending_bytes);
    pb.set_message(progress_msg(hits.len(), done_bytes));
    pb.finish_and_clear();

    Ok(hits.into_iter().take(MAX_HITS).collect())
}

// ── Output helpers ──────────────────────────────────────────────────────────

fn hits_to_list(hits: Vec<ScanHit>, title: &str) -> AnalysisOutput {
    if hits.is_empty() {
        return no_results("No matching requests found.");
    }
    let shown = hits.len();
    let sep = "─".repeat(64);
    let items = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| {
            let label = format!(
                "{:>4}  {}  {}  {}  {}",
                i + 1,
                h.hit_reason,
                h.method,
                truncate(&h.raw_url, LABEL_URL_MAX),
                h.timestamp,
            );
            let detail = format!(
                "anomaly\n{sep}\n  {}\n\noriginal request\n{sep}\n{}",
                h.hit_reason,
                h.full_entry,
            );
            ListItem { label, detail }
        })
        .collect();
    AnalysisOutput::SelectableList {
        title: title.into(),
        items,
        summary: Some(format!("{} requests found", fmt_count(shown))),
    }
}

fn no_source() -> AnalysisOutput {
    no_results("No source file path available for re-scan.")
}

fn no_results(msg: &str) -> AnalysisOutput {
    AnalysisOutput::Table {
        title: "Outlier Requests".into(),
        columns: vec![],
        rows: vec![],
        summary: Some(msg.into()),
    }
}

// ── Utilities ───────────────────────────────────────────────────────────────

fn outlier_threshold(log: &ParsedLog) -> usize {
    if log.route_counts.is_empty() { return 0; }
    let mut counts: Vec<usize> = log.route_counts.values().copied().collect();
    counts.sort_unstable();
    counts[counts.len() / 20].min(20)
}

fn normalize(url: &str) -> String {
    let s = ID_RE.replace_all(url, ":any_id").into_owned();
    match s.find('?') {
        Some(pos) => s[..pos].to_string(),
        None => s,
    }
}

/// Parse `content-length: <n>` (case-insensitive), returning the numeric value.
/// Operates on bytes to avoid panicking on non-UTF-8 char boundaries.
fn parse_content_length(line: &str) -> Option<u64> {
    const PREFIX: &[u8] = b"content-length:";
    let bytes = line.as_bytes();
    if bytes.len() <= PREFIX.len() { return None; }
    if !bytes[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) { return None; }
    std::str::from_utf8(&bytes[PREFIX.len()..]).ok()?.trim().parse().ok()
}

fn fmt_share(n: usize, total: usize) -> String {
    if total == 0 { return "?%".into(); }
    let pct = n as f64 * 100.0 / total as f64;
    if pct >= 1.0 { format!("{:.1}%", pct) }
    else if pct >= 0.01 { format!("{:.3}%", pct) }
    else if pct >= 0.0001 { format!("{:.6}%", pct) }
    else { format!("1 in {}", fmt_count((total as f64 / n as f64).round() as usize)) }
}

fn progress_msg(entries: usize, mem_bytes: usize) -> String {
    format!("{} hits  {} in memory", fmt_count(entries), fmt_bytes(mem_bytes as u64))
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
    pb.set_message("0 hits  0 B in memory");
    pb.enable_steady_tick(TICK_INTERVAL);
    pb
}
