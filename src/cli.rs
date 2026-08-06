use clap::{Parser, Subcommand};

/// Point local apps at any backend stand, keep servers and secrets at hand,
/// build and deploy - from any directory.
#[derive(Parser)]
#[command(name = "turnout", version, about, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize the data directory and walk through first-run setup
    Setup {
        /// Skip confirmation prompts and accept defaults
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
    /// Show what turnout knows: apps, servers, bindings, gateway state
    Status,
}
