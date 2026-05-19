// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

//! Shared scanning primitives used by the parser and on-demand re-scan
//! analyses: log-entry regex, URL normalization, and progress-bar styling.
//!
//! All normalization rules ([`normalize_url`] / [`normalize_url_counted`])
//! live here so changing the definition of "identifier" only requires editing
//! a single file.

use std::time::Duration;

use ahash::AHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use once_cell::sync::Lazy;
use regex::Regex;

// ─── Regexes ────────────────────────────────────────────────────────────────

/// First line of each log entry: `[timestamp] METHOD /url`.
/// Captures: 1=timestamp, 2=method, 3=raw_url.
pub static ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[([^\]]+)\]\s+([A-Z]+)\s+(/\S*)").expect("ENTRY_RE pattern is valid")
});

/// Plain UUID or `prefix_UUID` variants (e.g. `session_abc12345-...`).
static ID_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?:[a-zA-Z][a-zA-Z0-9_]*_)?[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
    )
    .expect("ID_RE pattern is valid")
});

/// Email addresses in URLs.  Handles `user@example.com` and the URL-encoded
/// `user%40example.com`.  Neither part may contain a `.`, `/`, or other URL
/// separators — so `/path` segments never appear inside a match.
static EMAIL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[^.@/\s?&=#%+]+(?:@|%40)[^.@/\s?&=#%+]+(?:\.[^.@/\s?&=#%+]+)+")
        .expect("EMAIL_RE pattern is valid")
});

/// Any standalone digit token is treated as an opaque identifier — page
/// numbers, numeric IDs, timestamps, phone numbers, etc.  Word boundaries
/// (`\b`) make sure version prefixes embedded in letters (`v1`, `s3`,
/// `api2025`) are preserved: only digit runs flanked by non-word characters
/// (`/`, `-`, `.`, start/end of string, …) are normalized.
static NUMBER_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b\d+\b").expect("NUMBER_RE pattern is valid"));

// ─── URL normalization ──────────────────────────────────────────────────────

/// Replacement token for every detected identifier.
pub const ANY_ID: &str = ":any_id";

/// Normalize `url` by replacing UUIDs, emails and standalone digit tokens
/// with [`ANY_ID`], and stripping the query string.  Routes differing only
/// in identifiers or query parameters collapse to the same key.
pub fn normalize_url(url: &str) -> String {
    let s = ID_RE.replace_all(url, ANY_ID);
    let s = EMAIL_RE.replace_all(&s, ANY_ID);
    let s = NUMBER_RE.replace_all(&s, ANY_ID);
    strip_query(s.into_owned())
}

/// Like [`normalize_url`] but also records every matched identifier in
/// `counts` (so the parser can populate `ParsedLog.identifier_counts`).
pub fn normalize_url_counted(url: &str, counts: &mut AHashMap<String, usize>) -> String {
    let s = ID_RE.replace_all(url, |c: &regex::Captures| {
        *counts.entry(c[0].to_string()).or_insert(0) += 1;
        ANY_ID
    });
    let s = EMAIL_RE.replace_all(&s, |c: &regex::Captures| {
        *counts.entry(c[0].to_string()).or_insert(0) += 1;
        ANY_ID
    });
    let s = NUMBER_RE.replace_all(&s, |c: &regex::Captures| {
        *counts.entry(c[0].to_string()).or_insert(0) += 1;
        ANY_ID
    });
    strip_query(s.into_owned())
}

fn strip_query(s: String) -> String {
    match s.find('?') {
        Some(pos) => s[..pos].to_string(),
        None => s,
    }
}

// ─── Progress bar ───────────────────────────────────────────────────────────

/// Each scanning thread flushes its byte counter to the shared progress bar
/// every 1 MB.  Keeps the bar responsive without per-line atomic contention.
pub const PROGRESS_FLUSH_BYTES: u64 = 1_024 * 1_024;
const TICK_INTERVAL: Duration = Duration::from_millis(80);

/// Standard progress bar: cyan bar, brail spinner, "msg eta X" suffix.
pub fn make_progress_bar(file_size: u64, initial_msg: &'static str) -> ProgressBar {
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:45.cyan/238}] {percent:>3}%  {msg}  eta {eta}",
        )
        .expect("progress template is valid")
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
        .progress_chars("█▓░"),
    );
    pb.set_message(initial_msg);
    pb.enable_steady_tick(TICK_INTERVAL);
    pb
}
