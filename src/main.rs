mod agent;
mod alias;
mod artifact;
mod cli;
mod commands;
mod control;
mod detached;
mod detect;
mod envfile;
mod front;
mod gateway;
mod health;
mod history;
mod jar;
mod job;
mod journal;
mod keysetup;
mod manifest;
mod migrate;
mod model;
mod notify;
mod output;
mod paths;
mod pick;
mod portable;
mod ports;
mod process;
mod progress;
mod registry;
mod remote;
mod secrets;
mod shell;
mod signal;
mod ssh;
mod store;
mod style;
mod term;
mod update;
mod utils;

use clap::Parser;

fn main() {
    let cli = cli::Cli::parse();
    // A command asked for JSON owns stdout: a progress line in front of the
    // JSON breaks whatever it is piped to.
    if progress::asks_for_json(std::env::args_os()) {
        progress::keep_stdout_for_data();
    }
    // Before anything touches the terminal: capture its state and take over
    // Ctrl+C, so an interrupt never leaves the console half-broken.
    term::init();
    // Clear the binary a previous self-update left behind; it is only
    // deletable once it is no longer the running image.
    commands::self_update::sweep_backup();
    // The background refresh must not spawn another one: it *is* the check.
    // `self-update` is excluded too - it has just said its piece about
    // versions, and the cached notice would contradict what it did. And a
    // prompt runs `status --short` before every line it draws: that is no
    // place for news, nor for a lookup.
    let announces = !matches!(
        cli.command,
        cli::Command::CheckUpdate
            | cli::Command::Complete { .. }
            | cli::Command::SelfUpdate { .. }
            | cli::Command::JobRun { .. }
            | cli::Command::Status { short: true, .. }
    );
    let result = match cli.command {
        cli::Command::Setup { assume_yes } => commands::setup::run(assume_yes),
        cli::Command::Doctor {
            server,
            fix,
            assume_yes,
            format,
        } => commands::doctor::run(format, fix, assume_yes, server),
        cli::Command::Status { format, short } => commands::status::run(format, short),
        cli::Command::App { command } => commands::app::run(command),
        cli::Command::Server { command } => commands::server::run(command),
        cli::Command::Credential { command } => commands::credential::run(command),
        cli::Command::Path { command } => commands::path::run(command),
        cli::Command::Target { command } => commands::target::run(command),
        cli::Command::Pass { command } => commands::pass::run(command),
        cli::Command::Key { command } => match command {
            cli::KeyCommand::Setup { server, credential, key } => commands::key::setup(server, credential, key),
            cli::KeyCommand::Check { server, credential } => commands::key::check(server, credential),
        },
        cli::Command::Use { app, server, no_check } => commands::use_cmd::run(app, server, no_check),
        cli::Command::Group { command } => commands::group::run(command),
        cli::Command::Gateway { command } => commands::gateway::run(command),
        cli::Command::Cookie { command } => commands::cookie::run(command),
        cli::Command::Dev { app, console, open } => commands::exec::run("dev", app, commands::exec::Options { console, open }),
        cli::Command::Open { name } => commands::open::run(name),
        cli::Command::Build { app, console } => commands::exec::run("build", app, commands::exec::Options { console, open: false }),
        cli::Command::Test { app, console } => commands::exec::run("test", app, commands::exec::Options { console, open: false }),
        cli::Command::Lint { app, console } => commands::exec::run("lint", app, commands::exec::Options { console, open: false }),
        cli::Command::Run { command, app, console, open } => commands::exec::run(&command, app, commands::exec::Options { console, open }),
        cli::Command::Ps { watch, format } => commands::jobs::ps(watch, format),
        cli::Command::Logs {
            name,
            command,
            follow,
            lines,
            failed,
        } => commands::jobs::logs(name, command, follow, lines, failed),
        cli::Command::Stop { name, command } => commands::jobs::stop(name, command),
        cli::Command::JobRun {
            app,
            command,
            dir,
            label,
            ready,
            own,
            open,
            itself,
            link,
            program,
        } => commands::jobs::supervise(commands::jobs::Supervised {
            app,
            command,
            dir,
            label,
            ready,
            own,
            open,
            itself,
            program,
            link,
        }),
        cli::Command::Ssh { name, credential } => commands::ssh::run(name, credential),
        cli::Command::Exec {
            name,
            credential,
            dir,
            command,
        } => commands::remote_exec::run(name, credential, dir, command),
        cli::Command::DeploySetup { app, server } => commands::deploy_setup::run(app, server),
        cli::Command::Deploy {
            targets,
            server,
            credential,
            path,
            no_build,
            backup,
            clear,
            no_archive,
            diff,
            json,
            open,
            console,
        } => commands::deploy::run(
            targets,
            remote::Overrides { server, credential, path },
            commands::deploy::Flags {
                no_build,
                backup,
                clear,
                no_archive,
                diff,
                json,
                open,
                console,
            },
        ),
        cli::Command::Backup {
            command:
                Some(cli::BackupCommand::Prune {
                    target,
                    server,
                    credential,
                    path,
                    keep,
                    format,
                }),
            ..
        } => commands::backup::prune(target, remote::Overrides { server, credential, path }, keep, format),
        cli::Command::Backup {
            command: None,
            target,
            server,
            credential,
            path,
        } => commands::backup::backup(target, remote::Overrides { server, credential, path }),
        cli::Command::History {
            name,
            limit,
            show,
            diff,
            format,
        } => commands::history::run(name, limit, show, diff, format),
        cli::Command::Rollback { target, to } => commands::rollback::run(target, to),
        cli::Command::Restore {
            target,
            server,
            credential,
            path,
            from,
            list,
            json,
        } => commands::backup::restore(
            target,
            remote::Overrides { server, credential, path },
            from,
            list.then_some(cli::Format { json }),
        ),
        cli::Command::Completions { shell } => commands::completions::run(shell),
        cli::Command::Complete { what } => commands::complete::run(what),
        cli::Command::Export { output, with_secrets } => commands::transfer::export(output, with_secrets),
        cli::Command::Import { file, force } => commands::transfer::import(file, force),
        cli::Command::SelfUpdate { assume_yes, force } => commands::self_update::run(assume_yes, force),
        cli::Command::CheckUpdate => update::check_now(),
    };
    // Whatever the command did to the terminal - a spinner mid-error, a
    // picker abandoned with Esc - the prompt the user gets back must work.
    term::restore();
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
