//! Heaviest Routes: top routes by call count, grouped per HTTP method.

use crate::analysis::{Analysis, AnalysisOutput, DEFAULT_TOP_N};
use crate::log::ParsedLog;
use crate::util::{fmt_count, fmt_pct};

pub struct HeaviestRoutes {
    pub top_n: usize,
}

impl Default for HeaviestRoutes {
    fn default() -> Self {
        Self {
            top_n: DEFAULT_TOP_N,
        }
    }
}

impl Analysis for HeaviestRoutes {
    fn name(&self) -> &'static str {
        "Heaviest Routes (most called, per method)"
    }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let mut entries: Vec<_> = log.route_counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        entries.truncate(self.top_n);

        let total = log.total_requests;
        let shown = entries.len();

        let rows = entries
            .into_iter()
            .enumerate()
            .map(|(i, (key, count))| {
                vec![
                    (i + 1).to_string(),
                    key.method.clone(),
                    key.url.clone(),
                    fmt_count(*count),
                    fmt_pct(*count, total),
                ]
            })
            .collect();

        AnalysisOutput::Table {
            title: format!("Top {shown} Heaviest Routes"),
            columns: ["#", "Method", "Route", "Calls", "Share"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            rows,
            summary: Some(format!("Total requests analyzed: {total}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::RouteKey;

    #[test]
    fn ranks_routes_by_descending_count() {
        let mut log = ParsedLog::default();
        log.total_requests = 6;
        log.route_counts.insert(RouteKey::new("GET", "/a"), 5);
        log.route_counts.insert(RouteKey::new("POST", "/b"), 1);

        let AnalysisOutput::Table { rows, .. } = HeaviestRoutes::default().run(&log);
        assert_eq!(rows[0][1], "GET");
        assert_eq!(rows[0][2], "/a");
        assert_eq!(rows[1][1], "POST");
    }
}
