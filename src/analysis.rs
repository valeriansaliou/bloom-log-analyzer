//! Pluggable analyses over a parsed log.
//!
//! Each analysis implements the [`Analysis`] trait. The [`Registry`] holds all
//! registered analyses and dispatches them by index, hiding the boxing behind
//! [`Registry::names`] and [`Registry::run`].
//!
//! To add a new analysis:
//! 1. Create `src/analysis/my_analysis.rs` and implement [`Analysis`].
//! 2. Declare the submodule below.
//! 3. Push it onto the vec in [`Registry::default`].

pub mod identifiers;
pub mod routes;

use crate::log::ParsedLog;

/// Default cap on rows returned by an analysis. Keeps the pager responsive even
/// when the underlying data has millions of unique entries.
pub const DEFAULT_TOP_N: usize = 1_000;

/// Output of an analysis run.
///
/// Currently only a `Table` variant; future variants (charts, time-series,
/// JSON export, …) can be added without breaking the trait.
pub enum AnalysisOutput {
    Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        summary: Option<String>,
    },
}

/// One analysis: produces an [`AnalysisOutput`] from a [`ParsedLog`].
pub trait Analysis: Send + Sync {
    /// Human-readable name shown in the interactive menu.
    fn name(&self) -> &'static str;

    /// Run the analysis against pre-aggregated log data.
    fn run(&self, log: &ParsedLog) -> AnalysisOutput;
}

/// Holds all analyses available in the interactive menu.
pub struct Registry {
    analyses: Vec<Box<dyn Analysis>>,
}

impl Registry {
    /// Display names for all registered analyses, in stable order.
    pub fn names(&self) -> Vec<&'static str> {
        self.analyses.iter().map(|a| a.name()).collect()
    }

    /// Run the analysis at `index`, returning `None` if `index` is out of bounds.
    pub fn run(&self, index: usize, log: &ParsedLog) -> Option<AnalysisOutput> {
        self.analyses.get(index).map(|a| a.run(log))
    }

    pub fn len(&self) -> usize {
        self.analyses.len()
    }

    pub fn is_empty(&self) -> bool {
        self.analyses.is_empty()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            analyses: vec![
                Box::new(routes::HeaviestRoutes::default()),
                Box::new(identifiers::HeaviestIdentifiers::default()),
            ],
        }
    }
}
