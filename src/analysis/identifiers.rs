//! Most Seen URL Identifiers: top UUID / `prefix_UUID` / email / numeric
//! identifier values seen across all URLs (tenants, sessions, items, …).

use crate::analysis::{Analysis, AnalysisOutput, SortableRow, DEFAULT_TOP_N};
use crate::log::ParsedLog;
use crate::util::{fmt_count, fmt_pct};

pub struct HeaviestIdentifiers {
    pub top_n: usize,
}

impl Default for HeaviestIdentifiers {
    fn default() -> Self {
        Self {
            top_n: DEFAULT_TOP_N,
        }
    }
}

impl Analysis for HeaviestIdentifiers {
    fn name(&self) -> &'static str {
        "Most Seen URL Identifiers"
    }

    fn run(&self, log: &ParsedLog) -> AnalysisOutput {
        let mut entries: Vec<_> = log.identifier_counts.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        entries.truncate(self.top_n);

        let total: usize = log.identifier_counts.values().sum();
        let unique = log.identifier_counts.len();
        let shown = entries.len();

        let rows = entries
            .into_iter()
            .map(|(id, count)| {
                let pct_scaled = (*count as f64 / total.max(1) as f64 * 1_000_000.0) as u64;
                SortableRow {
                    cells: vec![id.clone(), fmt_count(*count), fmt_pct(*count, total)],
                    sort_keys: vec![
                        None,                // identifier (text)
                        Some(*count as u64), // occurrences
                        Some(pct_scaled),    // share
                    ],
                }
            })
            .collect();

        AnalysisOutput::SortableTable {
            title: format!("Top {shown} Most Seen URL Identifiers"),
            preamble: None,
            chart: None,
            columns: ["identifier", "occurrences", "share"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sortable: vec![1, 2],
            rows,
            summary: Some(format!(
                "Total identifier occurrences: {}  ·  Unique identifiers: {}",
                fmt_count(total),
                fmt_count(unique),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_identifiers_by_descending_count() {
        let mut log = ParsedLog::default();
        log.identifier_counts.insert("alpha".into(), 10);
        log.identifier_counts.insert("beta".into(), 3);
        log.identifier_counts.insert("gamma".into(), 7);

        let AnalysisOutput::SortableTable { rows, .. } = HeaviestIdentifiers::default().run(&log)
        else {
            panic!("expected SortableTable")
        };
        assert_eq!(rows[0].cells[0], "alpha");
        assert_eq!(rows[1].cells[0], "gamma");
        assert_eq!(rows[2].cells[0], "beta");
    }
}
