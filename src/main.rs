// Bloom Log Analyzer
//
// Log analysis CLI for the Bloom HTTP REST API caching middleware
// Copyright: 2026, Valerian Saliou <valerian@valeriansaliou.name>
// License: Mozilla Public License v2.0 (MPL v2.0)

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "bloom-log-analyzer",
    about = "HTTP request log analyzer — fast analysis for large request logs",
    version
)]
struct Cli {
    /// Path to the log file to analyze
    log_file: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    bloom_log_analyzer::run(&cli.log_file)
}
