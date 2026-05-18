//! Streaming parser for HTTP request log files.
//!
//! Reads line-by-line via `BufReader` so the file is never fully in memory.
//! Only the first line of each entry (`[timestamp] METHOD /path`) is parsed;
//! every other line (headers, body, blanks, `---` separators) is skipped at
//! near-zero cost — the only check is a regex match that fails on the first
//! character when the line doesn't start with `[`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::log::{ParsedLog, RouteKey};
use crate::util::fmt_count;

/// Plain UUID or `prefix_UUID` variants (e.g. `session_abc12345-...`).
static ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:[a-zA-Z][a-zA-Z0-9_]*_)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    )
    .expect("ID_RE pattern is valid")
});

/// First line of each log entry: `[timestamp] METHOD /url`.
static ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\]\s+([A-Z]+)\s+(/\S*)").expect("ENTRY_RE pattern is valid")
});

const TICK_INTERVAL: Duration = Duration::from_millis(80);

/// Parse `path` into a [`ParsedLog`], showing a progress bar on stderr.
pub fn parse_file(path: &Path) -> Result<ParsedLog> {
    let file_size = fs::metadata(path)?.len();
    let file = File::open(path)?;
    let pb = make_progress_bar(file_size);

    let reader = BufReader::new(pb.wrap_read(file));
    let mut log = ParsedLog::default();
    let start = Instant::now();

    for line in reader.lines() {
        let line = line?;
        if let Some(caps) = ENTRY_RE.captures(&line) {
            record_request(&mut log, &caps[2], &caps[3]);
            update_progress(&pb, &log, start);
        }
    }

    pb.finish_and_clear();
    Ok(log)
}

fn record_request(log: &mut ParsedLog, method: &str, raw_url: &str) {
    let normalized = normalize_url(raw_url, &mut log.identifier_counts);
    *log.route_counts
        .entry(RouteKey::new(method, normalized))
        .or_insert(0) += 1;
    log.total_requests += 1;
}

fn normalize_url(url: &str, id_counts: &mut HashMap<String, usize>) -> String {
    ID_RE
        .replace_all(url, |c: &regex::Captures| {
            *id_counts.entry(c[0].to_string()).or_insert(0) += 1;
            ":any_id"
        })
        .into_owned()
}

fn make_progress_bar(file_size: u64) -> ProgressBar {
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:45.cyan/238}] {percent:>3}%  {msg}  eta {eta}",
        )
        .expect("progress template is valid")
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
        .progress_chars("█▓░"),
    );
    pb.set_message("0 req  0 req/s");
    pb.enable_steady_tick(TICK_INTERVAL);
    pb
}

fn update_progress(pb: &ProgressBar, log: &ParsedLog, start: Instant) {
    let elapsed = start.elapsed().as_secs_f64();
    let rps = if elapsed > 0.1 {
        (log.total_requests as f64 / elapsed) as usize
    } else {
        0
    };
    pb.set_message(format!(
        "{} req  {} req/s",
        fmt_count(log.total_requests),
        fmt_count(rps),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_uuid_and_prefix_uuid() {
        let mut counts = HashMap::new();
        let normalized = normalize_url(
            "/v1/website/9e578821-15f2-438b-b339-4126ea73abf3/conversation/session_becf8e02-f845-4336-9bcc-443aeac2183f/routing",
            &mut counts,
        );
        assert_eq!(
            normalized,
            "/v1/website/:any_id/conversation/:any_id/routing"
        );
        assert_eq!(counts.len(), 2);
        assert_eq!(counts.get("9e578821-15f2-438b-b339-4126ea73abf3"), Some(&1));
        assert_eq!(
            counts.get("session_becf8e02-f845-4336-9bcc-443aeac2183f"),
            Some(&1)
        );
    }

    #[test]
    fn record_request_aggregates_by_route() {
        let mut log = ParsedLog::default();
        record_request(&mut log, "GET", "/v1/foo");
        record_request(&mut log, "GET", "/v1/foo");
        record_request(&mut log, "POST", "/v1/foo");

        assert_eq!(log.total_requests, 3);
        assert_eq!(log.route_counts.len(), 2);
        assert_eq!(
            log.route_counts
                .get(&RouteKey::new("GET", "/v1/foo")),
            Some(&2)
        );
    }

    #[test]
    fn entry_regex_matches_sample_line() {
        let caps = ENTRY_RE
            .captures("[2026-05-17T07:56:03Z] GET /v1/website/abc/routing")
            .expect("should match");
        assert_eq!(&caps[2], "GET");
        assert_eq!(&caps[3], "/v1/website/abc/routing");
    }
}
