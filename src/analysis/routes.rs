//! Most Called Routes: top routes by call count, grouped per HTTP method.

use crate::analysis::{Analysis, AnalysisOutput, SortableRow, DEFAULT_TOP_N};
use crate::log::ParsedLog;
use crate::util::{fmt_count, fmt_pct};

pub struct HeaviestRoutes {
    pub top_n: usize,
}

impl Default for HeaviestRoutes {
    fn default() -> Self {
        Self { top_n: DEFAULT_TOP_N }
    }
}

impl Analysis for HeaviestRoutes {
    fn name(&self) -> &'static str {
        "Most Called Routes (per method)"
    }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let mut entries: Vec<_> = log.route_counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        entries.truncate(self.top_n);

        let total = log.total_requests;
        let shown = entries.len();

        let rows = entries
            .into_iter()
            .map(|(key, count)| {
                let pct_scaled = (*count as f64 / total.max(1) as f64 * 1_000_000.0) as u64;
                SortableRow {
                    cells: vec![
                        key.method.clone(),
                        key.url.clone(),
                        fmt_count(*count),
                        fmt_pct(*count, total),
                    ],
                    sort_keys: vec![
                        None,                   // method
                        None,                   // route
                        Some(*count as u64),    // calls
                        Some(pct_scaled),       // share
                    ],
                }
            })
            .collect();

        AnalysisOutput::SortableTable {
            title: format!("Top {shown} Most Called Routes"),
            preamble: None,
            chart: None,
            columns: ["method", "route", "calls", "share"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sortable: vec![2, 3],
            rows,
            summary: Some(format!("Total requests analyzed: {}", fmt_count(total))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::RouteKey;

    #[test]
    fn ranks_routes_by_descending_count() {
        let mut log = ParsedLog {
            total_requests: 6,
            ..ParsedLog::default()
        };
        log.route_counts.insert(RouteKey::new("GET", "/a"), 5);
        log.route_counts.insert(RouteKey::new("POST", "/b"), 1);

        let AnalysisOutput::SortableTable { rows, .. } = HeaviestRoutes::default().run(&log)
        else { panic!("expected SortableTable") };
        assert_eq!(rows[0].cells[0], "GET");
        assert_eq!(rows[0].cells[1], "/a");
        assert_eq!(rows[1].cells[0], "POST");
    }
}
