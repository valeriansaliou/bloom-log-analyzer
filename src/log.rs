//! Domain types for parsed log state.

use ahash::AHashMap;

/// All analysis data, pre-aggregated during a single streaming parse pass.
///
/// Memory is O(unique routes + unique identifiers), not O(total requests):
/// no per-request data is retained. To support a new analysis dimension
/// (user agents, IPs, time buckets, …), add an `AHashMap` field here and one
/// line of collection in [`crate::parser`].
#[derive(Debug, Default)]
pub struct ParsedLog {
    pub total_requests: usize,
    pub route_counts: AHashMap<RouteKey, usize>,
    /// Raw identifier string → occurrence count across all URLs.
    pub identifier_counts: AHashMap<String, usize>,

    // --- file-level metadata set by the parser ---
    /// On-disk size of the log file in bytes.
    pub file_size: u64,
    /// Raw timestamp string of the earliest log entry.
    pub first_timestamp: Option<String>,
    /// Raw timestamp string of the latest log entry.
    pub last_timestamp: Option<String>,
    /// Sum of all `content-length` header values seen across all requests.
    pub total_bytes_in: u64,
}

/// Identity of a route: HTTP method paired with a normalized URL.
///
/// "Normalized" means UUIDs and `prefix_UUID` segments have been replaced with
/// `:any_id`, so e.g. `GET /v1/website/<uuid>/messages` collapses to one key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteKey {
    pub method: String,
    pub url: String,
}

impl RouteKey {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
        }
    }
}
