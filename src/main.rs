mod cli;
mod commands;
mod detect;
mod gateway;
mod model;
mod paths;
mod secrets;
mod store;
mod utils;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    let result = match cli.command {
        cli::Command::Setup { assume_yes } => commands::setup::run(assume_yes),
        cli::Command::Status => commands::status::run(),
        cli::Command::App { command } => commands::app::run(command),
        cli::Command::Server { command } => commands::server::run(command),
        cli::Command::Pass { command } => commands::pass::run(command),
        cli::Command::Use { app, server, no_check } => commands::use_cmd::run(&app, &server, no_check),
        cli::Command::Gateway { command } => commands::gateway::run(command),
    };
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
