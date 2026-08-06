mod cli;
mod commands;
mod paths;
mod store;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let result = match cli.command {
        cli::Command::Setup { assume_yes } => commands::setup::run(assume_yes),
        cli::Command::Status => commands::status::run(),
    };
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
