Bloom Log Analyzer
==================

**Log analysis CLI for Bloom, the REST API caching middleware. It helps you uncover patterns in your Bloom logs.**

Bloom Log Analyzer is a CLI for [Bloom](https://github.com/valeriansaliou/bloom).

A very large request log file from a Bloom server can be analyzed using all your CPU cores in a few seconds (2M requests / second analyzed on a M4 Pro CPU), with low to no memory impact. Traffic pattern reports can then be visualized from within your terminal.

Request logs can be saved in Bloom by enabling the `proxy.request_log` option.

👉 Not using Bloom? **[Check out Bloom here](https://github.com/valeriansaliou/bloom)**.

_Tested at Rust version: `rustc 1.94.0 (4a4ef493e 2026-03-02)`_

**🇫🇷 Crafted in Nantes, France.**

## How to use?

The `bloom-log-analyzer` binary needs to be compiled on your machine before you can run it:

```sh
# Make sure Rust is installed first
# I recommend installing using: https://rustup.rs

# Pull bloom-log-analyzer:
git clone https://github.com/valeriansaliou/bloom-log-analyzer.git
cd ./bloom-log-analyzer

# Build a release version of bloom-log-analyzer:
cargo build --release

# Run the release binary of bloom-log-analyzer on your Bloom log file:
cargo run --release /path/to/your/bloom-requests.log 
```

## Features

As soon as you run `bloom-log-analyzer` on a Bloom requests log file, a quick analysis will be ran, which can take some time.

Then, you'll enter a menu where you can pick which analysis report to see. Some reports require further log file analysis to be ran, that takes a bit more time.

- ✅ **Most Called Routes** (per method)
- ✅ **Most Seen URL Identifiers** (largest tenants)
- ✅ **Heaviest Requests** (headers + body byte size)
- ✅ **Traffic Timeline** (burst detection)
- ✅ **Outlier Requests** (weird requests)
  - Large Request       _content-length > 100 KB_
  - Large Header        _single header line > 2 KB_
  - Large Query String  _query part > 512 chars_
  - Anomalous Header    _non-standard characters in header name_
  - Rare URL            _route pattern with unusually low traffic_

Reports are typically shown in a table format. You can click on the table headers to sort in ascending or descending order, if the column value is of numerical form.

To navigate between the menu or results, you can use the following keyboard shortcuts:

- ↕️ **Navigate up/down**: `ARROW UP` / `ARROW DOWN`
- ↩️ **Inspect original request**: `ENTER`
- ⏪ **Go back**: `Q` / `ESCAPE`

## Disclaimer

Bloom Log Analyzer has been built entirely using Claude Code. Whilst Bloom is hand-crafted (and will remain so), this CLI tool is solely maintained using agentic coding.

If you want to add a new feature, please do not modify with the source code by hand. You can open an issue and ask for the desired feature, provide some sample log file from your Bloom server, and I will ask Claude Code to implement it for you on my end.

To keep things simple, there are no releases. The latest stable `bloom-log-analyzer` is available at `HEAD` from the Git `master` branch.
