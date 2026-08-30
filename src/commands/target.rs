//! Targets: the named deploy target - which app goes to which server, as whom,
//! and where.
//!
//! An entity of its own since v0.11.0 (ADR 0013). The relationship used to live
//! inside the server as a map from app to path, which meant it could not be
//! named, listed or reused - the same shape v0.9.0 pulled `user@host` out of the
//! server for.

use anyhow::{Result, bail};
use dialoguer::Input;

use crate::cli::TargetCommand;
use crate::model::{Target, unique_target_name, validate_name};
use crate::{pick, store};

pub fn run(command: TargetCommand) -> Result<()> {
    match command {
        TargetCommand::Add {
            name,
            app,
            server,
            credential,
            path,
        } => add(name, app, server, credential, path),
        TargetCommand::List => list(),
        TargetCommand::Show { name } => show(&resolve(name, "Show target")?),
        TargetCommand::Edit {
            name,
            server,
            credential,
            path,
        } => {
            let name = resolve(name, "Edit target")?;
            edit(&name, server, credential, path)
        }
        TargetCommand::Rename { name, to } => {
            let name = resolve(name, "Rename target")?;
            rename(&name, to)
        }
        TargetCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove target")?;
            remove(&name, assume_yes)
        }
    }
}

fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::target(&store::load_targets()?, prompt),
    }
}

fn add(name: Option<String>, app: Option<String>, server: Option<String>, credential: Option<String>, path: Option<String>) -> Result<()> {
    let mut targets = store::load_targets()?;
    let apps = store::load_apps()?;
    let servers = store::load_servers()?;
    let credentials = store::load_credentials()?;
    let paths = store::load_paths()?;
    let state = store::load_state()?;

    let app_name = match app {
        Some(name) => name,
        None => pick::app(&apps, &state, "Which app")?,
    };
    let app = apps
        .iter()
        .find(|a| a.name == app_name)
        .ok_or_else(|| anyhow::anyhow!("no app named '{app_name}' - see `turnout app list`"))?;

    let server_name = match server {
        Some(name) => name,
        None => pick::server_for_app(&servers, app, "Deploy to which server")?,
    };
    let server = servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    if !app.servers.is_empty() && !app.servers.iter().any(|s| s == &server_name) {
        bail!("server '{server_name}' is not allowed for '{app_name}' (allowed: {})", app.servers.join(", "));
    }

    // The server's own credential is the right default: it is what a deploy
    // there logged in as before named targets existed.
    let credential_name = match credential.or_else(|| server.credential.clone()) {
        Some(name) => name,
        None => pick::credential(&credentials, "Logs in with")?,
    };
    if !credentials.iter().any(|c| c.name == credential_name) {
        bail!("no credential named '{credential_name}' - see `turnout credential list`");
    }

    let path_name = match path {
        Some(name) => name,
        None => pick::path(&paths, "Files land in")?,
    };
    if !paths.iter().any(|p| p.name == path_name) {
        bail!("no path named '{path_name}' - see `turnout path list`");
    }

    // Generated from two names the user already chose, so it needs no prompt to
    // be recognizable - but it is offered rather than imposed.
    let name = match name {
        Some(name) => name,
        None => {
            let suggested = unique_target_name(&app_name, &server_name, &targets);
            if pick::interactive() {
                Input::new()
                    .with_prompt("Target name")
                    .default(suggested)
                    .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
                    .interact_text()?
            } else {
                suggested
            }
        }
    };
    validate_name(&name)?;
    if targets.iter().any(|t| t.name == name) {
        bail!("target '{name}' already exists");
    }

    targets.push(Target {
        name: name.clone(),
        app: app_name.clone(),
        server: server_name.clone(),
        credential: credential_name,
        path: path_name,
    });
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_targets(&targets)?;
    crate::journal::record("target.add", Some(&app_name), Some(&server_name), Some(&name));
    println!("Target '{name}' added - deploy with `turnout deploy {name}`.");
    Ok(())
}

/// Save a tuple that was just assembled interactively, so the next run has a
/// name to use. Returns the name when one was created.
///
/// Lives here rather than in `deploy` so that the offer and `target add` write
/// the same entity through the same generator.
pub fn offer_to_save(app: &str, server: &str, credential: &str, path: &str) -> Result<Option<String>> {
    if !pick::interactive() {
        return Ok(None);
    }
    let mut targets = store::load_targets()?;
    let suggested = unique_target_name(app, server, &targets);
    if !dialoguer::Confirm::new()
        .with_prompt(format!("Save this as a target, so `turnout deploy {suggested}` repeats it?"))
        .default(true)
        .interact()?
    {
        return Ok(None);
    }
    let name: String = Input::new()
        .with_prompt("Target name")
        .default(suggested)
        .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
        .interact_text()?;
    if targets.iter().any(|t| t.name == name) {
        bail!("target '{name}' already exists");
    }
    targets.push(Target {
        name: name.clone(),
        app: app.to_string(),
        server: server.to_string(),
        credential: credential.to_string(),
        path: path.to_string(),
    });
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_targets(&targets)?;
    crate::journal::record("target.add", Some(app), Some(server), Some(&name));
    Ok(Some(name))
}

fn list() -> Result<()> {
    let targets = store::load_targets()?;
    if targets.is_empty() {
        println!("No targets yet - run `turnout target add`.");
        return Ok(());
    }
    let width = targets.iter().map(|t| t.name.len()).max().unwrap_or(0);
    for target in targets {
        println!(
            "{:width$}  {} -> {}  ({}@, {})",
            target.name, target.app, target.server, target.credential, target.path
        );
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let targets = store::load_targets()?;
    let target = targets.iter().find(|t| t.name == name).ok_or_else(|| unknown(name))?;
    println!("{}", target.name);
    println!("  App:        {}", target.app);
    println!("  Server:     {}", target.server);
    println!("  Logs in as: {}", target.credential);
    // The directory itself, not just the path's name: "where does this land" is
    // the question `show` is asked, and answering it with another name to look
    // up is half an answer.
    match store::load_paths()?.iter().find(|p| p.name == target.path) {
        Some(path) => println!("  Lands in:   {} ({})", target.path, path.dir),
        None => println!("  Lands in:   {} (missing from the catalog)", target.path),
    }
    Ok(())
}

fn edit(name: &str, server: Option<String>, credential: Option<String>, path: Option<String>) -> Result<()> {
    let mut targets = store::load_targets()?;
    let index = targets.iter().position(|t| t.name == name).ok_or_else(|| unknown(name))?;
    if server.is_none() && credential.is_none() && path.is_none() {
        pick::ensure_interactive("nothing to change: pass flags to edit non-interactively")?;
        let apps = store::load_apps()?;
        let app = apps
            .iter()
            .find(|a| a.name == targets[index].app)
            .ok_or_else(|| anyhow::anyhow!("target '{name}' names app '{}', which is not in the catalog", targets[index].app))?;
        let server = pick::server_for_app(&store::load_servers()?, app, "Deploy to which server")?;
        let credential = pick::credential(&store::load_credentials()?, "Logs in with")?;
        let path = pick::path(&store::load_paths()?, "Files land in")?;
        let target = &mut targets[index];
        target.server = server;
        target.credential = credential;
        target.path = path;
    } else {
        if let Some(server) = &server
            && !store::load_servers()?.iter().any(|s| &s.name == server)
        {
            bail!("no server named '{server}' - see `turnout server list`");
        }
        if let Some(credential) = &credential
            && !store::load_credentials()?.iter().any(|c| &c.name == credential)
        {
            bail!("no credential named '{credential}' - see `turnout credential list`");
        }
        if let Some(path) = &path
            && !store::load_paths()?.iter().any(|p| &p.name == path)
        {
            bail!("no path named '{path}' - see `turnout path list`");
        }
        let target = &mut targets[index];
        if let Some(server) = server {
            target.server = server;
        }
        if let Some(credential) = credential {
            target.credential = credential;
        }
        if let Some(path) = path {
            target.path = path;
        }
    }
    store::save_targets(&targets)?;
    crate::journal::record("target.edit", None, None, Some(name));
    println!("Target '{name}' updated.");
    Ok(())
}

fn rename(name: &str, to: Option<String>) -> Result<()> {
    let mut targets = store::load_targets()?;
    let index = targets.iter().position(|t| t.name == name).ok_or_else(|| unknown(name))?;
    let to = match to {
        Some(to) => to,
        None => {
            pick::ensure_interactive("the new name is required")?;
            Input::new()
                .with_prompt("New name")
                .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
                .interact_text()?
        }
    };
    validate_name(&to)?;
    if to == name {
        println!("Target '{name}' already has that name.");
        return Ok(());
    }
    if targets.iter().any(|t| t.name == to) {
        bail!("target '{to}' already exists");
    }
    targets[index].name = to.clone();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_targets(&targets)?;
    crate::journal::record("target.rename", None, None, Some(&format!("{name} -> {to}")));
    println!("Target '{name}' renamed to '{to}'.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut targets = store::load_targets()?;
    if !targets.iter().any(|t| t.name == name) {
        return Err(unknown(name));
    }
    if !pick::confirm_destructive(format!("Remove target '{name}'?"), assume_yes)? {
        println!("Cancelled.");
        return Ok(());
    }
    targets.retain(|t| t.name != name);
    store::save_targets(&targets)?;
    crate::journal::record("target.remove", None, None, Some(name));
    // The four entities it named are catalogs of their own and outlive it; only
    // the connection between them is gone.
    println!("Target '{name}' removed (the app, server, credential and path are untouched).");
    Ok(())
}

fn unknown(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no target named '{name}' - see `turnout target list`")
}
