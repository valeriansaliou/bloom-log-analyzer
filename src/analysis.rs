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
pub mod outliers;
pub mod request_sizes;
pub mod routes;
pub mod timeline;

use crate::log::ParsedLog;

/// Default cap on rows returned by an analysis. Keeps the pager responsive even
/// when the underlying data has millions of unique entries.
pub const DEFAULT_TOP_N: usize = 1_000;

/// Output of an analysis run.
pub enum AnalysisOutput {
    /// Flat table rendered in a scrollable pager.
    Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        summary: Option<String>,
    },
    /// Navigable list where each item can be "opened" to show a detail view.
    SelectableList {
        title: String,
        items: Vec<ListItem>,
        summary: Option<String>,
    },
    /// An interactive table whose columns can be sorted by clicking their headers.
    /// The `#` rank column is added automatically at render time.
    SortableTable {
        title: String,
        /// Optional multi-line text rendered between the title and the column
        /// header row (used e.g. for sparklines).
        preamble: Option<String>,
        /// Raw counts per time bucket used to render the full-screen chart when
        /// the user clicks on the sparkline preamble.  `None` disables chart mode.
        chart_data: Option<Vec<usize>>,
        /// Legend metadata for the chart: `(y_axis_label, x_start_label, x_end_label)`.
        chart_meta: Option<(String, String, String)>,
        /// Column names (no `#`).
        columns: Vec<String>,
        /// Indices into `columns` that support sorting.
        sortable: Vec<usize>,
        rows: Vec<SortableRow>,
        summary: Option<String>,
    },
    /// A sub-menu of named analyses; handled by the top-level orchestration
    /// loop in `lib.rs` (not by `ui::display_output`).
    SubMenu {
        title: String,
        options: Vec<(String, Box<dyn Analysis>)>,
    },
}

/// One row in a [`AnalysisOutput::SelectableList`].
pub struct ListItem {
    /// Short line shown in the navigation list.
    pub label: String,
    /// Full text shown in the pager when the item is selected.
    pub detail: String,
}

/// One row in a [`AnalysisOutput::SortableTable`].
pub struct SortableRow {
    /// Display cells, parallel to the table's `columns` (no leading `#` rank cell).
    pub cells: Vec<String>,
    /// Sort key per column — `None` for non-sortable columns, `Some(u64)` otherwise.
    pub sort_keys: Vec<Option<u64>>,
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
                Box::new(request_sizes::HeaviestRequestsBySize::default()),
                Box::new(timeline::TrafficTimeline),
                Box::new(outliers::OutlierRequests),
            ],
        }
    }
}
