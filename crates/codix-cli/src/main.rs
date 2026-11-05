//! The `codix` command-line interface.

use clap::Parser;

/// Headless code index for LLM agents.
#[derive(Parser)]
#[command(name = "codix", version)]
struct Cli {}

fn main() {
    Cli::parse();
}
