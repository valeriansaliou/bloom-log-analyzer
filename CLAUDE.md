# analgun

HTTP request log analyzer — written in Rust, designed for huge log files.

## Quick start

```bash
cd analgun
cargo run --release -- /path/to/requests.log
```

Use ↑/↓ to pick an analysis, ↵ to run, ↑/↓/j/k to scroll results in the pager, `q` to exit the pager.

## What it does

Parses HTTP request logs in a specific multi-line format (see [Log format](#log-format) below) and offers two interactive analyses today (extensible — see [Adding a new analysis](#adding-a-new-analysis)):

- **Heaviest Routes** — top routes by call count, per HTTP method. URLs containing UUIDs (or `prefix_UUID` like `session_<uuid>`) are normalized to `:any_id` so the same logical route groups together regardless of which specific resource it targeted.
- **Heaviest Identifiers** — top UUID values seen in URLs. Useful for finding the tenant/session that's generating the most traffic.

Results are shown in a paged ASCII table (arrow keys to scroll, `q` to exit).

## Architecture

### Memory model — streaming, single-pass

Logs can be multiple gigabytes. The parser uses `BufReader` line-by-line, so the file is **never fully in memory**. Aggregations are built incrementally during a single pass:

| In-memory structure | Bounded by |
|---|---|
| `ParsedLog.route_counts` | unique routes (typically hundreds) |
| `ParsedLog.identifier_counts` | unique identifiers (UUIDs × ~70 bytes each) |

**No per-request data is retained.** When a new analysis needs an additional dimension (user agents, IPs, time buckets, status codes…), add a `HashMap` field to `ParsedLog` in `src/log.rs` and one line of collection in `parser::record_request`.

### Parser fast path

Only the first line of each entry (`[timestamp] METHOD /url`) is parsed. Headers, body lines, blank lines, and the `---` separator are skipped at near-zero cost — the regex match fails on the first character when the line doesn't start with `[`. There is no block-buffer / multi-line state machine.

### Library + binary split

`src/lib.rs` exposes the full pipeline as a library (`analgun::run`, `analgun::parser::parse_file`, …). `src/main.rs` is a thin shim that just parses CLI args and calls `analgun::run`. This makes the analysis pipeline reusable and supports `tests/` integration tests.

## Module layout

```
src/
├── main.rs              ← CLI argv parsing (thin shim)
├── lib.rs               ← public API + top-level `run()` entry point
├── log.rs               ← ParsedLog, RouteKey (domain types)
├── parser.rs            ← streaming log parser + progress bar
├── analysis.rs          ← Analysis trait + Registry (module file)
├── analysis/
│   ├── routes.rs        ← HeaviestRoutes analyzer
│   └── identifiers.rs   ← HeaviestIdentifiers analyzer
├── ui.rs                ← interactive menu + paged table rendering
└── util.rs              ← fmt_count, truncate, fmt_pct
```

## Adding a new analysis

1. Create `src/analysis/my_analysis.rs`:
   ```rust
   use crate::analysis::{Analysis, AnalysisOutput, DEFAULT_TOP_N};
   use crate::log::ParsedLog;

   pub struct MyAnalysis;

   impl Analysis for MyAnalysis {
       fn name(&self) -> &'static str { "My Analysis" }
       fn run(&self, log: &ParsedLog) -> AnalysisOutput {
           // compute and return AnalysisOutput::Table { ... }
       }
   }
   ```

2. Declare the submodule in `src/analysis.rs`:
   ```rust
   pub mod my_analysis;
   ```

3. Register it in `Registry::default()` (also in `src/analysis.rs`):
   ```rust
   Box::new(my_analysis::MyAnalysis),
   ```

If the analysis needs a new pre-aggregated dimension, add the field to `ParsedLog` and collection logic in `parser::record_request` — the parser already gives you the method and normalized URL; pull additional fields off the captures inside `ENTRY_RE` (extend the regex if you need them).

## Log format

```
[2026-05-17T07:56:03Z] GET /v1/website/<uuid>/conversation/session_<uuid>/routing

x-real-ip: 1.2.3.4
host: app.crisp.chat
...

optional request body
```

Entries are separated by `---` lines but the parser doesn't require it — it keys off the `[timestamp] METHOD /url` regex, so any non-matching line is ignored.

### Identifier normalization

URLs are scanned for two patterns by `ID_RE` in `src/parser.rs`:

- Plain UUID: `9e578821-15f2-438b-b339-4126ea73abf3`
- Prefix-UUID: `session_becf8e02-f845-4336-9bcc-443aeac2183f`

Both are replaced with `:any_id` in the normalized URL. The raw identifier strings are simultaneously counted in `ParsedLog.identifier_counts` so they can be ranked independently.

## UI details

- **Progress bar** (`indicatif`): byte-based percentage from file size, request count and req/s shown in the message, animated spinner via `enable_steady_tick`.
- **Menu** (`dialoguer`): arrow-key navigation with the `ColorfulTheme`. "Quit" is always the last item. ESC (`interact_opt` returning `None`) → `Selection::Quit`. CTRL+C → `Selection::Interrupted`; the main loop requires two consecutive CTRL+C presses to exit (first press prints a warning).
- **Pager** (`minus` with `static_output` feature): receives a pre-rendered String containing ANSI codes. `colored::control::set_override(true)` is set before string building so colors aren't stripped (no TTY detected during build). Footer set via `pager.set_prompt(...)` showing navigation legend (`↑/↓ row  fn+↑/↓ page  esc back`).
- **Cell truncation** (`ui::max_cell_chars`): reads terminal width via `crossterm::terminal::size()` and clips each cell to `term_width / num_cols`, floored at 20 chars. Prevents the table from overflowing the terminal and corrupting borders.
- **Number formatting**: all count/occurrence columns use `util::fmt_count` for comma thousands separators.

## Dependencies

| Crate | Purpose |
|---|---|
| `clap` | CLI argument parsing |
| `regex` + `once_cell` | UUID detection in URLs (lazy-compiled regex) |
| `dialoguer` | Interactive analysis menu |
| `indicatif` | Progress bar during parsing |
| `minus` | `less`-style pager for results |
| `comfy-table` | ASCII table rendering |
| `colored` | ANSI color codes |
| `crossterm` | Terminal width detection |
| `anyhow` | Error context propagation |

## Tests

```bash
cargo test
```

Unit tests live alongside their modules under `#[cfg(test)] mod tests`. There are no integration tests yet — `tests/` is available and would `use analgun::*;` directly thanks to the lib/bin split.

## Constants and tuning

| Constant | Location | Value |
|---|---|---|
| `DEFAULT_TOP_N` | `src/analysis.rs` | `1_000` — max rows per analysis |
| `TICK_INTERVAL` | `src/parser.rs` | 80 ms — progress spinner tick rate |
| `MIN_CELL_WIDTH` | `src/ui.rs` | 20 chars — floor for table cells |
| `FALLBACK_TERM_WIDTH` | `src/ui.rs` | 120 — used when terminal size unknown |

## Performance characteristics

- **Parse throughput**: dominated by line decoding from `BufReader` (UTF-8 validation) plus one regex match per line. The regex bails on the first character for non-matching lines, so headers/body/blank lines are essentially free.
- **Aggregation cost**: O(1) HashMap insert per matched line; one normalized URL allocation per request; one identifier allocation per UUID seen.
- **Render cost**: O(N log N) sort over `route_counts` / `identifier_counts`, then O(top_n) row construction.

## Rust style conventions used here

- Modules follow Rust 2018 file-based layout (`analysis.rs` + `analysis/`, not `analysis/mod.rs`).
- Public domain types live in `log.rs`; internal types in their using module.
- Static regexes via `once_cell::Lazy` — they compile once on first use.
- `Default` impls instead of `new()` where there's no configuration.
- `&'static str` for fixed-name methods (avoids `String` allocation for menu items).
- Magic numbers are named constants at the top of each module.
- `#[cfg(test)] mod tests` colocated with code; no separate `tests.rs` files.
