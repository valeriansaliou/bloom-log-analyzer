//! Parallel parser for HTTP request log files.
//!
//! Memory-maps the file once, splits it into N CPU-aligned byte chunks at
//! newline boundaries, then processes each chunk on a separate rayon thread.
//! Partial results are merged into a single [`ParsedLog`] after all threads
//! complete.
//!
//! Per-line `String` allocations are eliminated: non-entry lines (anything
//! not starting with `[`) are rejected with a single byte comparison before
//! UTF-8 validation or regex matching are attempted.

use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Result;
use indicatif::ProgressBar;
use rayon::prelude::*;

use crate::log::{ParsedLog, RouteKey};
use crate::scanner::{normalize_url_counted, ENTRY_RE, PROGRESS_FLUSH_BYTES};
use crate::util::fmt_count;

/// Parse `path` into a [`ParsedLog`], using all available CPU cores.
///
/// The file is memory-mapped read-only. Modifying the file externally while
/// parsing produces undefined results — acceptable for log analysis.
pub fn parse_file(path: &Path) -> Result<ParsedLog> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();

    if file_size == 0 {
        return Ok(ParsedLog::default());
    }

    let pb = make_progress_bar(file_size);
    let start = Instant::now();

    // Zero-copy: the OS pages in only what we actually touch.
    // SAFETY: we hold an exclusive analysis session; the file is read-only here.
    let mmap = unsafe { memmap2::Mmap::map(&file)? };
    let data: &[u8] = &mmap;

    let n_threads = rayon::current_num_threads().max(1);
    let chunks = crate::util::split_into_chunks(data, n_threads);

    // Shared counter for live req/s display; updated atomically per chunk.
    let req_counter = AtomicUsize::new(0);

    // Each thread accumulates into its own ParsedLog — no locking during parse.
    let partial_logs: Vec<ParsedLog> = chunks
        .into_par_iter()
        .map(|chunk| {
            let mut local = ParsedLog::default();
            let mut pending_bytes: u64 = 0;
            let mut pending_reqs: usize = 0;

            for line in chunk.split(|&b| b == b'\n') {
                // +1 re-adds the '\n' that split() consumed.
                pending_bytes += line.len() as u64 + 1;

                match line.first() {
                    // Entry lines: `[timestamp] METHOD /url`
                    Some(&b'[') => {
                        if let Ok(s) = std::str::from_utf8(line) {
                            if let Some(caps) = ENTRY_RE.captures(s) {
                                record_request(&mut local, &caps[2], &caps[3], &caps[1]);
                                pending_reqs += 1;
                            }
                        }
                    }
                    // Header lines: fast-path for `content-length:` (any case)
                    Some(&b'c') | Some(&b'C') => {
                        local.total_bytes_in += parse_content_length(line).unwrap_or(0);
                    }
                    _ => {}
                }

                // Flush bytes + reqs every 1 MB so both the bar and the
                // req/s message update smoothly at ~80 ms intervals.
                if pending_bytes >= PROGRESS_FLUSH_BYTES {
                    pb.inc(pending_bytes);
                    pending_bytes = 0;
                    let total = req_counter.fetch_add(pending_reqs, Ordering::Relaxed)
                        + pending_reqs;
                    pending_reqs = 0;
                    let elapsed = start.elapsed().as_secs_f64();
                    let rps = if elapsed > 0.1 {
                        (total as f64 / elapsed) as usize
                    } else {
                        0
                    };
                    pb.set_message(format!(
                        "{} req  {} req/s",
                        fmt_count(total),
                        fmt_count(rps)
                    ));
                }
            }

            // Final flush for any bytes/reqs not yet reported.
            let total = req_counter.fetch_add(pending_reqs, Ordering::Relaxed) + pending_reqs;
            let elapsed = start.elapsed().as_secs_f64();
            let rps = if elapsed > 0.1 {
                (total as f64 / elapsed) as usize
            } else {
                0
            };
            pb.inc(pending_bytes);
            pb.set_message(format!(
                "{} req  {} req/s",
                fmt_count(total),
                fmt_count(rps)
            ));
            local
        })
        .collect();

    // Merge all partial maps into the final log.
    let mut log = ParsedLog {
        file_size,
        source_path: Some(path.to_path_buf()),
        ..ParsedLog::default()
    };
    for mut partial in partial_logs {
        log.total_requests += partial.total_requests;
        log.total_bytes_in += partial.total_bytes_in;
        for (key, count) in partial.route_counts {
            *log.route_counts.entry(key).or_insert(0) += count;
        }
        for (id, count) in partial.identifier_counts {
            *log.identifier_counts.entry(id).or_insert(0) += count;
        }
        // Keep the earliest first_timestamp and latest last_timestamp.
        if let Some(ts) = partial.first_timestamp.take() {
            match &log.first_timestamp {
                None => log.first_timestamp = Some(ts),
                Some(cur) if ts < *cur => log.first_timestamp = Some(ts),
                _ => {}
            }
        }
        if let Some(ts) = partial.last_timestamp.take() {
            match &log.last_timestamp {
                None => log.last_timestamp = Some(ts),
                Some(cur) if ts > *cur => log.last_timestamp = Some(ts),
                _ => {}
            }
        }
    }

    pb.finish_and_clear();
    Ok(log)
}


fn record_request(log: &mut ParsedLog, method: &str, raw_url: &str, timestamp: &str) {
    let normalized = normalize_url_counted(raw_url, &mut log.identifier_counts);
    *log.route_counts
        .entry(RouteKey::new(method, normalized))
        .or_insert(0) += 1;
    log.total_requests += 1;
    if log.first_timestamp.is_none() {
        log.first_timestamp = Some(timestamp.to_string());
    }
    log.last_timestamp = Some(timestamp.to_string());
}

/// Parse a `content-length: <n>` header line (case-insensitive prefix).
/// Returns `None` if the line is not a content-length header or the value
/// cannot be parsed as a `u64`.
fn parse_content_length(line: &[u8]) -> Option<u64> {
    const PREFIX: &[u8] = b"content-length:";
    if line.len() <= PREFIX.len() {
        return None;
    }
    if !line[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    std::str::from_utf8(&line[PREFIX.len()..])
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn make_progress_bar(file_size: u64) -> ProgressBar {
    crate::scanner::make_progress_bar(file_size, "0 req  0 req/s")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahash::AHashMap;

    #[test]
    fn normalizes_uuid_and_prefix_uuid() {
        let mut counts = AHashMap::new();
        let normalized = normalize_url_counted(
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
    fn normalizes_email_in_url() {
        let mut counts = AHashMap::new();
        let n = normalize_url_counted("/v1/users/bob@example.com/profile", &mut counts);
        assert_eq!(n, "/v1/users/:any_id/profile");
        assert!(counts.contains_key("bob@example.com"));
    }

    #[test]
    fn normalizes_percent_encoded_email() {
        let mut counts = AHashMap::new();
        let n = normalize_url_counted("/v1/users/bob%40example.com/profile", &mut counts);
        assert_eq!(n, "/v1/users/:any_id/profile");
        assert!(counts.contains_key("bob%40example.com"));
    }

    #[test]
    fn normalizes_any_digit_token() {
        // Both short and long numeric tokens are normalized.
        let mut counts = AHashMap::new();
        let n1 = normalize_url_counted("/v1/items/42/get", &mut counts);
        assert_eq!(n1, "/v1/items/:any_id/get");
        assert!(counts.contains_key("42"));

        let mut counts = AHashMap::new();
        let n2 = normalize_url_counted("/v1/phone/1234567890/verify", &mut counts);
        assert_eq!(n2, "/v1/phone/:any_id/verify");
        assert!(counts.contains_key("1234567890"));
    }

    #[test]
    fn preserves_version_prefix() {
        // Digits embedded in a letter token (e.g. `v1`, `api2025`) stay intact.
        let mut counts = AHashMap::new();
        let n = normalize_url_counted("/v1/api2025/users/42", &mut counts);
        assert_eq!(n, "/v1/api2025/users/:any_id");
        assert!(counts.contains_key("42"));
        assert!(!counts.contains_key("1"));
        assert!(!counts.contains_key("2025"));
    }

    #[test]
    fn record_request_aggregates_by_route() {
        let mut log = ParsedLog::default();
        record_request(&mut log, "GET", "/v1/foo", "2026-01-01T00:00:00Z");
        record_request(&mut log, "GET", "/v1/foo", "2026-01-01T00:00:01Z");
        record_request(&mut log, "POST", "/v1/foo", "2026-01-01T00:00:02Z");

        assert_eq!(log.total_requests, 3);
        assert_eq!(log.route_counts.len(), 2);
        assert_eq!(
            log.route_counts.get(&RouteKey::new("GET", "/v1/foo")),
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

    #[test]
    fn parse_content_length_handles_cases() {
        assert_eq!(parse_content_length(b"content-length: 1234"), Some(1234));
        assert_eq!(parse_content_length(b"Content-Length: 0"), Some(0));
        assert_eq!(parse_content_length(b"Content-Length:512"), Some(512));
        assert_eq!(parse_content_length(b"x-real-ip: 1.2.3.4"), None);
        assert_eq!(parse_content_length(b"content-length: abc"), None);
    }

    #[test]
    fn record_request_tracks_timestamps() {
        let mut log = ParsedLog::default();
        record_request(&mut log, "GET", "/a", "2026-01-01T01:00:00Z");
        record_request(&mut log, "GET", "/b", "2026-01-01T02:00:00Z");
        record_request(&mut log, "GET", "/c", "2026-01-01T00:30:00Z");
        // first_timestamp stays as the first-seen within this chunk
        assert_eq!(log.first_timestamp.as_deref(), Some("2026-01-01T01:00:00Z"));
        assert_eq!(log.last_timestamp.as_deref(), Some("2026-01-01T00:30:00Z"));
    }

}
