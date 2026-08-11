use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, MultiSelect};

use crate::cli::AppCommand;
use crate::detect;
use crate::model::{App, Server, validate_name};
use crate::{pick, store};

pub fn run(command: AppCommand) -> Result<()> {
    match command {
        AppCommand::Add {
            name,
            path,
            port,
            dist,
            commands,
            servers,
        } => add(name, path, port, dist, commands, servers),
        AppCommand::List => list(),
        AppCommand::Show { name } => show(&resolve(name, "Show app")?),
        AppCommand::Edit {
            name,
            path,
            port,
            dist,
            commands,
            add_servers,
            rm_servers,
        } => {
            let name = resolve(name, "Edit app")?;
            edit(&name, path, port, dist, commands, add_servers, rm_servers)
        }
        AppCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove app")?;
            remove(&name, assume_yes)
        }
    }
}

/// Take the app name as given, or let the user pick one.
fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::app(&store::load_apps()?, &store::load_state()?, prompt),
    }
}

fn add(name: Option<String>, path: Option<PathBuf>, port: Option<u16>, dist: Option<String>, overrides: Vec<String>, servers: Vec<String>) -> Result<()> {
    let mut apps = store::load_apps()?;
    let known = store::load_servers()?;
    let wizard = name.is_none() || path.is_none();
    if wizard {
        pick::ensure_interactive("app name and --path are required")?;
    }

    let name = match name {
        Some(name) => name,
        None => Input::new()
            .with_prompt("App name")
            .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_name(&name)?;
    if apps.iter().any(|a| a.name == name) {
        bail!("app '{name}' already exists");
    }
    if store::load_groups()?.iter().any(|g| g.name == name) {
        bail!("a group named '{name}' already exists - app names must not clash with group names");
    }

    let path = match path {
        Some(path) => path,
        None => {
            let cwd = std::env::current_dir()?.display().to_string();
            PathBuf::from(Input::<String>::new().with_prompt("Project directory").default(cwd).interact_text()?)
        }
    };
    let path = crate::utils::project_dir(&path)?;

    let kind = detect::detect(&path);
    let mut commands = detect::commands_for(&path, kind);
    if wizard && !commands.is_empty() {
        let source = if path.join("package.json").exists() { " (from package.json)" } else { "" };
        println!("Detected a {} project{source}; proposed commands:", kind.label());
        print_commands(&commands);
        if !Confirm::new().with_prompt("Use these commands?").default(true).interact()? {
            commands.clear();
            println!("Skipped. Set commands later with `turnout app edit {name} --command NAME=CMD`.");
        }
    }
    apply_overrides(&mut commands, &overrides)?;

    let port = if wizard && port.is_none() {
        let suggestion = 7100 + apps.len() as u16;
        let answer: String = Input::new()
            .with_prompt("Local gateway port (empty to skip)")
            .default(suggestion.to_string())
            .allow_empty(true)
            .interact_text()?;
        if answer.trim().is_empty() {
            None
        } else {
            Some(answer.trim().parse().context("port must be a number")?)
        }
    } else {
        port
    };

    let servers = if wizard && servers.is_empty() && !known.is_empty() {
        pick_servers(&known, &[])?
    } else {
        validate_servers(&servers, &known)?;
        servers
    };

    let app = App {
        name: name.clone(),
        path: path.display().to_string(),
        commands,
        dist_dir: dist,
        gateway_port: port,
        servers,
    };
    apps.push(app);
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_apps(&apps)?;
    crate::journal::record("app.add", Some(&name), None, None);
    println!("App '{name}' added.");
    Ok(())
}

fn list() -> Result<()> {
    let apps = store::load_apps()?;
    if apps.is_empty() {
        println!("No apps yet - run `turnout app add`.");
        return Ok(());
    }
    let width = apps.iter().map(|a| a.name.len()).max().unwrap_or(0);
    for app in apps {
        let port = app.gateway_port.map(|p| format!(":{p}")).unwrap_or_default();
        println!("{:width$}  {}{}", app.name, app.path, port);
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let apps = store::load_apps()?;
    let app = find(&apps, name)?;
    println!("{}", app.name);
    println!("  Path:     {}", app.path);
    if !Path::new(&app.path).is_dir() {
        println!("            (warning: directory no longer exists)");
    }
    match app.gateway_port {
        Some(port) => println!("  Gateway:  localhost:{port}"),
        None => println!("  Gateway:  port not set"),
    }
    if let Some(dist) = &app.dist_dir {
        println!("  Dist:     {dist}");
    }
    if app.commands.is_empty() {
        println!("  Commands: none");
    } else {
        println!("  Commands:");
        print_commands(&app.commands);
    }
    if app.servers.is_empty() {
        println!("  Servers:  none allowed yet");
    } else {
        println!("  Servers:  {}", app.servers.join(", "));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edit(
    name: &str,
    path: Option<PathBuf>,
    port: Option<u16>,
    dist: Option<String>,
    overrides: Vec<String>,
    add_servers: Vec<String>,
    rm_servers: Vec<String>,
) -> Result<()> {
    let mut apps = store::load_apps()?;
    let known = store::load_servers()?;
    let index = apps.iter().position(|a| a.name == name).ok_or_else(|| unknown_app(name))?;
    let no_flags = path.is_none() && port.is_none() && dist.is_none() && overrides.is_empty() && add_servers.is_empty() && rm_servers.is_empty();

    if no_flags {
        pick::ensure_interactive("nothing to change: pass flags to edit non-interactively")?;
        let app = &mut apps[index];
        let path: String = Input::new().with_prompt("Project directory").default(app.path.clone()).interact_text()?;
        let path = crate::utils::project_dir(Path::new(&path))?;
        app.path = path.display().to_string();
        let port: String = Input::new()
            .with_prompt("Local gateway port (empty to unset)")
            .default(app.gateway_port.map(|p| p.to_string()).unwrap_or_default())
            .allow_empty(true)
            .interact_text()?;
        app.gateway_port = if port.trim().is_empty() {
            None
        } else {
            Some(port.trim().parse().context("port must be a number")?)
        };
        if !known.is_empty() {
            let current = app.servers.clone();
            app.servers = pick_servers(&known, &current)?;
        }
        println!("Commands are edited with flags: `turnout app edit {name} --command NAME=CMD` (NAME= removes).");
    } else {
        let app = &mut apps[index];
        if let Some(path) = path {
            let path = crate::utils::project_dir(&path)?;
            app.path = path.display().to_string();
        }
        if port.is_some() {
            app.gateway_port = port;
        }
        if dist.is_some() {
            app.dist_dir = dist;
        }
        apply_overrides(&mut app.commands, &overrides)?;
        validate_servers(&add_servers, &known)?;
        for server in add_servers {
            if !app.servers.contains(&server) {
                app.servers.push(server);
            }
        }
        app.servers.retain(|s| !rm_servers.contains(s));
    }
    store::save_apps(&apps)?;
    println!("App '{name}' updated.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut apps = store::load_apps()?;
    find(&apps, name)?;
    let confirmed = pick::confirm_destructive(format!("Remove app '{name}' from the catalog?"), assume_yes)?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    apps.retain(|a| a.name != name);
    store::save_apps(&apps)?;

    let mut groups = store::load_groups()?;
    let mut touched = Vec::new();
    for group in groups.iter_mut() {
        if group.apps.iter().any(|a| a == name) {
            group.apps.retain(|a| a != name);
            touched.push(group.name.clone());
        }
    }
    if !touched.is_empty() {
        // A group emptied by this removal disappears with it.
        groups.retain(|g| !g.apps.is_empty());
        store::save_groups(&groups)?;
        println!("Removed '{name}' from groups: {}.", touched.join(", "));
    }
    crate::journal::record("app.remove", Some(name), None, None);
    println!("App '{name}' removed. The project on disk is untouched.");
    Ok(())
}

fn find<'a>(apps: &'a [App], name: &str) -> Result<&'a App> {
    apps.iter().find(|a| a.name == name).ok_or_else(|| unknown_app(name))
}

fn unknown_app(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no app named '{name}' - see `turnout app list`")
}

fn print_commands(commands: &BTreeMap<String, String>) {
    // Script names come from package.json and can be longer than the roles.
    let width = commands.keys().map(|n| n.len()).max().unwrap_or(0).max(8);
    for (name, cmd) in commands {
        println!("    {name:width$} {cmd}");
    }
}

/// Apply NAME=CMD pairs; an empty CMD removes the command.
fn apply_overrides(commands: &mut BTreeMap<String, String>, overrides: &[String]) -> Result<()> {
    for spec in overrides {
        let (name, cmd) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid command '{spec}': expected NAME=CMD"))?;
        if name.is_empty() {
            bail!("invalid command '{spec}': expected NAME=CMD");
        }
        if cmd.is_empty() {
            commands.remove(name);
        } else {
            commands.insert(name.to_string(), cmd.to_string());
        }
    }
    Ok(())
}

fn validate_servers(names: &[String], known: &[Server]) -> Result<()> {
    for name in names {
        if !known.iter().any(|s| &s.name == name) {
            bail!("no server named '{name}' - see `turnout server list`");
        }
    }
    Ok(())
}

fn pick_servers(known: &[Server], current: &[String]) -> Result<Vec<String>> {
    let items: Vec<&str> = known.iter().map(|s| s.name.as_str()).collect();
    let defaults: Vec<bool> = known.iter().map(|s| current.contains(&s.name)).collect();
    let picked = MultiSelect::new()
        .with_prompt("Allowed servers (space to toggle, enter to accept)")
        .items(&items)
        .defaults(&defaults)
        .interact()?;
    Ok(picked.into_iter().map(|i| known[i].name.clone()).collect())
}
