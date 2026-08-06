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
        cli::Command::Dev { app } => commands::exec::run("dev", app),
        cli::Command::Build { app } => commands::exec::run("build", app),
        cli::Command::Test { app } => commands::exec::run("test", app),
        cli::Command::Lint { app } => commands::exec::run("lint", app),
        cli::Command::Run { command, app } => commands::exec::run(&command, app),
    };
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
