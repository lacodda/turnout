//! `turnout env` - the variables an app runs with on a stand.
//!
//! The store is [`crate::envset`]; this is the console over it. A set
//! belongs to an app and a server, and every command names the pair the same
//! way: `--app` or the app of the current directory, `--server` or the stand
//! the app points at - the pair `dev` is about to run with.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use dialoguer::Password;
use serde::Serialize;

use crate::cli::{EnvCommand, Format, Stand};
use crate::model::App;
use crate::{envset, output, pick, store};

pub fn run(command: EnvCommand) -> Result<()> {
    match command {
        EnvCommand::Set { variables, stand } => set(variables, stand),
        EnvCommand::List { stand, show, format } => {
            if show {
                self::show(stand)
            } else {
                list(stand, format)
            }
        }
        EnvCommand::Remove { names, all, assume_yes, stand } => remove(names, all, assume_yes, stand),
        EnvCommand::Import { file, stand } => import(file, stand),
    }
}

/// The app and the server a command works on.
fn resolve(stand: Stand) -> Result<(App, String)> {
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, stand.app)?.clone();
    let server = match stand.server {
        Some(server) => {
            if !store::load_servers()?.iter().any(|known| known.name == server) {
                bail!("no server named '{server}' - see `turnout server list`");
            }
            app.check_allowed(&server)?;
            server
        }
        None => store::load_state()?.bindings.get(&app.name).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "'{0}' points at no stand - name the one these variables are for with --server, or bind it with `turnout use {0} SERVER`",
                app.name
            )
        })?,
    };
    Ok((app, server))
}

/// The gateway's address is turnout's to set: it is this machine's, and a
/// stored copy would go to the stand with a deploy.
fn refuse_the_gateway_variable(app: &App, name: &str) -> Result<()> {
    if name == app.gateway_env_name() {
        bail!(
            "{name} is the variable turnout hands '{}' the gateway's address in - it is set for every run and is not stored",
            app.name
        );
    }
    Ok(())
}

fn set(variables: Vec<String>, stand: Stand) -> Result<()> {
    let (app, server) = resolve(stand)?;
    let asked: Vec<&String> = variables.iter().filter(|variable| !variable.contains('=')).collect();
    if asked.len() > 1 && !pick::interactive() {
        bail!(
            "standard input carries one value, and {} names came without one - give them as NAME=VALUE, or one at a time",
            asked.len()
        );
    }
    let mut pairs = Vec::new();
    for variable in &variables {
        let (name, value) = match variable.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            None => (variable.clone(), String::new()),
        };
        envset::validate_name(&name)?;
        refuse_the_gateway_variable(&app, &name)?;
        let value = if variable.contains('=') { value } else { ask(&name, &app.name, &server)? };
        pairs.push((name, value));
    }
    let stored = envset::put(&app.name, &server, &pairs)?;
    crate::journal::record("env.set", Some(&app.name), Some(&server), Some(&names_of(&pairs).join(" ")));
    println!("{}", said(&stored, &app.name, &server));
    Ok(())
}

/// A value nobody typed on the command line: asked for without an echo on a
/// terminal, read whole from standard input otherwise.
fn ask(name: &str, app: &str, server: &str) -> Result<String> {
    if pick::interactive() {
        return Ok(Password::new()
            .with_prompt(format!("{name} for '{app}' on '{server}'"))
            .allow_empty_password(true)
            .interact()?);
    }
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer).context("cannot read the value from stdin")?;
    // One line break is the pipe's, not the value's; a value of several lines
    // keeps the rest.
    let value = buffer.strip_suffix('\n').unwrap_or(&buffer);
    let value = value.strip_suffix('\r').unwrap_or(value);
    if value.is_empty() {
        bail!("no value on stdin for {name} - pipe it in, or give it as {name}=VALUE");
    }
    Ok(value.to_string())
}

fn names_of(pairs: &[(String, String)]) -> Vec<String> {
    pairs.iter().map(|(name, _)| name.clone()).collect()
}

/// What a `set` or an `import` did, in a line.
fn said(stored: &envset::Stored, app: &str, server: &str) -> String {
    let mut parts = Vec::new();
    if !stored.added.is_empty() {
        parts.push(format!("stored {}", stored.added.join(", ")));
    }
    if !stored.replaced.is_empty() {
        parts.push(format!("replaced {}", stored.replaced.join(", ")));
    }
    let mut line = parts.join("; ");
    if let Some(first) = line.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    format!("{line} for '{app}' on '{server}', in the OS keyring.")
}

/// A set as `env list` reports it: names, never values.
#[derive(Serialize)]
struct SetView {
    app: String,
    server: String,
    /// The stand the app points at now - the set `dev` and `run` hand over.
    current: bool,
    variables: Vec<String>,
}

fn list(stand: Stand, format: Format) -> Result<()> {
    // No app named and none around: every app's sets. A picker here would ask
    // which app before showing what there is to pick from.
    let apps = store::load_apps()?;
    let app = match &stand.app {
        Some(name) => Some(crate::commands::exec::resolve(&apps, Some(name.clone()))?.name.clone()),
        None => crate::commands::exec::app_here(&apps)?.map(|app| app.name.clone()),
    };
    if let Some(server) = &stand.server
        && !store::load_servers()?.iter().any(|known| &known.name == server)
    {
        bail!("no server named '{server}' - see `turnout server list`");
    }
    let bindings = store::load_state()?.bindings;
    let views: Vec<SetView> = envset::sets()?
        .into_iter()
        .filter(|set| app.as_ref().is_none_or(|app| &set.app == app))
        .filter(|set| stand.server.as_ref().is_none_or(|server| &set.server == server))
        .map(|set| SetView {
            current: bindings.get(&set.app) == Some(&set.server),
            variables: set.variables.into_iter().map(|variable| variable.name).collect(),
            app: set.app,
            server: set.server,
        })
        .collect();
    output::emit(format, &views, |views| {
        if views.is_empty() {
            return match &app {
                Some(app) => {
                    format!("No variables stored for '{app}' - add one with `turnout env set NAME=VALUE`, or a file of them with `turnout env import`.")
                }
                None => "No variables stored - add one with `turnout env set NAME=VALUE --app APP`.".to_string(),
            };
        }
        let mut lines = Vec::new();
        for view in views {
            let count = view.variables.len();
            let plural = if count == 1 { "" } else { "s" };
            let current = if view.current { " - where it points now" } else { "" };
            lines.push(format!("{} on {} ({count} variable{plural}){current}", view.app, view.server));
            for name in &view.variables {
                lines.push(format!("  {name}"));
            }
        }
        lines.push("Values live in the OS keyring; print one set with `turnout env list --show`.".to_string());
        lines.join("\n")
    })
}

/// `env list --show`: one set with its values, as the dotenv lines a file
/// would hold.
fn show(stand: Stand) -> Result<()> {
    let (app, server) = resolve(stand)?;
    let pairs = envset::values(&app.name, &server)?;
    if pairs.is_empty() {
        bail!("no variables stored for '{}' on '{server}' - see `turnout env list`", app.name);
    }
    print!("{}", envset::render(&pairs));
    Ok(())
}

fn remove(names: Vec<String>, all: bool, assume_yes: bool, stand: Stand) -> Result<()> {
    let (app, server) = resolve(stand)?;
    let have = envset::names(&app.name, &server)?;
    for name in &names {
        if !have.contains(name) {
            bail!("'{}' has no variable {name} on '{server}' - see `turnout env list`", app.name);
        }
    }
    let going = if all { have } else { names };
    if going.is_empty() {
        println!("No variables stored for '{}' on '{server}'.", app.name);
        return Ok(());
    }
    let confirmed = pick::confirm_destructive(
        format!("Remove {} of '{}' on '{server}' from the keyring?", going.join(", "), app.name),
        assume_yes,
    )?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    let gone = envset::take(&app.name, &server, Some(&going))?;
    crate::journal::record("env.remove", Some(&app.name), Some(&server), Some(&gone.join(" ")));
    println!("Removed {} of '{}' on '{server}'.", gone.join(", "), app.name);
    Ok(())
}

fn import(file: Option<PathBuf>, stand: Stand) -> Result<()> {
    let (app, server) = resolve(stand)?;
    let path = file.unwrap_or_else(|| std::path::Path::new(&app.path).join(".env"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut pairs = envset::parse(&text).with_context(|| format!("{} is not a dotenv file turnout can read", path.display()))?;
    // The gateway's address written by hand into the file, as it was before
    // turnout kept it: this machine's, not the stand's.
    let before = pairs.len();
    pairs.retain(|(name, _)| name != app.gateway_env_name());
    if pairs.len() < before {
        println!(
            "Left out {} - turnout hands '{}' the gateway's address in it on every run.",
            app.gateway_env_name(),
            app.name
        );
    }
    if pairs.is_empty() {
        bail!("{} holds no variables", path.display());
    }
    let stored = envset::put(&app.name, &server, &pairs)?;
    crate::journal::record("env.import", Some(&app.name), Some(&server), Some(&format!("{} variables", pairs.len())));
    println!("{}", said(&stored, &app.name, &server));
    println!(
        "{} still holds them in plain text - `turnout dev` and `turnout run` hand them over without it.",
        path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_is_reported_by_what_it_did() {
        let stored = envset::Stored {
            added: vec!["A".into(), "B".into()],
            replaced: vec!["C".into()],
        };
        assert_eq!(
            said(&stored, "web", "staging"),
            "Stored A, B; replaced C for 'web' on 'staging', in the OS keyring."
        );
        let only = envset::Stored {
            added: Vec::new(),
            replaced: vec!["C".into()],
        };
        assert_eq!(said(&only, "web", "staging"), "Replaced C for 'web' on 'staging', in the OS keyring.");
    }
}
