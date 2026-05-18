//! Heaviest Identifiers: top UUID / `prefix_UUID` values seen across all URLs.

use crate::analysis::{Analysis, AnalysisOutput, DEFAULT_TOP_N};
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
        "Heaviest Identifiers (most seen in URLs)"
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
            .enumerate()
            .map(|(i, (id, count))| {
                vec![
                    (i + 1).to_string(),
                    id.clone(),
                    fmt_count(*count),
                    fmt_pct(*count, total),
                ]
            })
            .collect();

        AnalysisOutput::Table {
            title: format!("Top {shown} Heaviest Identifiers"),
            columns: ["#", "Identifier", "Occurrences", "Share"]
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            rows,
            summary: Some(format!(
                "Total identifier occurrences: {total} | Unique identifiers: {unique}"
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

        let AnalysisOutput::Table { rows, .. } = HeaviestIdentifiers::default().run(&log);
        assert_eq!(rows[0][1], "alpha");
        assert_eq!(rows[1][1], "gamma");
        assert_eq!(rows[2][1], "beta");
    }
}
