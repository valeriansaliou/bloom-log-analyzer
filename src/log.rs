//! Domain types for parsed log state.

use std::collections::HashMap;

/// All analysis data, pre-aggregated during a single streaming parse pass.
///
/// Memory is O(unique routes + unique identifiers), not O(total requests):
/// no per-request data is retained. To support a new analysis dimension
/// (user agents, IPs, time buckets, …), add a `HashMap` field here and one
/// line of collection in [`crate::parser`].
#[derive(Debug, Default)]
pub struct ParsedLog {
    pub total_requests: usize,
    pub route_counts: HashMap<RouteKey, usize>,
    /// Raw identifier string → occurrence count across all URLs.
    pub identifier_counts: HashMap<String, usize>,
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
