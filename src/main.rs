mod cli;
mod commands;
mod detect;
mod gateway;
mod journal;
mod model;
mod paths;
mod pick;
mod portable;
mod progress;
mod remote;
mod secrets;
mod store;
mod update;
mod utils;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    // Clear the binary a previous self-update left behind; it is only
    // deletable once it is no longer the running image.
    commands::self_update::sweep_backup();
    // The background refresh must not spawn another one: it *is* the check.
    // `self-update` is excluded too - it has just said its piece about
    // versions, and the cached notice would contradict what it did.
    let announces = !matches!(
        cli.command,
        cli::Command::CheckUpdate | cli::Command::Complete { .. } | cli::Command::SelfUpdate { .. }
    );
    let result = match cli.command {
        cli::Command::Setup { assume_yes } => commands::setup::run(assume_yes),
        cli::Command::Status => commands::status::run(),
        cli::Command::App { command } => commands::app::run(command),
        cli::Command::Server { command } => commands::server::run(command),
        cli::Command::Pass { command } => commands::pass::run(command),
        cli::Command::Use { app, server, no_check } => commands::use_cmd::run(app, server, no_check),
        cli::Command::Group { command } => commands::group::run(command),
        cli::Command::Gateway { command } => commands::gateway::run(command),
        cli::Command::Dev { app } => commands::exec::run("dev", app),
        cli::Command::Build { app } => commands::exec::run("build", app),
        cli::Command::Test { app } => commands::exec::run("test", app),
        cli::Command::Lint { app } => commands::exec::run("lint", app),
        cli::Command::Run { command, app } => commands::exec::run(&command, app),
        cli::Command::DeploySetup { app, server } => commands::deploy_setup::run(app, server),
        cli::Command::Deploy {
            app,
            server,
            no_build,
            backup,
            clear,
            no_archive,
        } => commands::deploy::run(app, server, no_build, backup, clear, no_archive),
        cli::Command::Backup { app, server } => commands::backup::backup(app, server),
        cli::Command::Restore { app, server, from, list } => commands::backup::restore(app, server, from, list),
        cli::Command::Completions { shell } => commands::completions::run(shell),
        cli::Command::Complete { what } => commands::complete::run(what),
        cli::Command::Export { output, with_secrets } => commands::transfer::export(output, with_secrets),
        cli::Command::Import { file, force } => commands::transfer::import(file, force),
        cli::Command::SelfUpdate { assume_yes, force } => commands::self_update::run(assume_yes, force),
        cli::Command::CheckUpdate => update::check_now(),
    };
    if let Err(err) = result {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
    // Last, and only after a command succeeded: news about turnout itself must
    // not push the actual output out of view, nor decorate a failure.
    if announces {
        update::hint_and_refresh();
    }
}
