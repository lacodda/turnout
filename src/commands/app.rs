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
            env_var,
            env_file,
            dev_port,
            dist,
            commands,
            servers,
        } => add(name, path, port, env_var, env_file, dev_port, dist, commands, servers),
        AppCommand::List => list(),
        AppCommand::Show { name } => show(&resolve(name, "Show app")?),
        AppCommand::Edit {
            name,
            path,
            port,
            env_var,
            env_file,
            dev_port,
            dist,
            commands,
            add_servers,
            rm_servers,
        } => {
            let name = resolve(name, "Edit app")?;
            edit(&name, path, port, env_var, env_file, dev_port, dist, commands, add_servers, rm_servers)
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

#[allow(clippy::too_many_arguments)]
fn add(
    name: Option<String>,
    path: Option<PathBuf>,
    port: Option<u16>,
    env_var: Option<String>,
    env_file: Option<String>,
    dev_port: Option<u16>,
    dist: Option<String>,
    overrides: Vec<String>,
    servers: Vec<String>,
) -> Result<()> {
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

    if let Some(port) = port {
        ensure_port_is_free(&apps, port, &name)?;
    }
    let env_file = env_file.map(validate_env_file).transpose()?;
    let dev_port = dev_port.filter(|port| *port != 0);
    if let Some(port) = dev_port {
        crate::front::ensure_dev_port_is_free(&apps, port, &name)?;
    }
    let env_var = match env_var {
        Some(name) => Some(validate_env_name(name)?),
        None if wizard && port.is_some() => {
            let answer: String = Input::new()
                .with_prompt("Variable that carries the gateway URL to the app")
                .default(suggested_env_name(&path).to_string())
                .interact_text()?;
            Some(validate_env_name(answer)?)
        }
        None => None,
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
        gateway_env: env_var,
        env_file,
        dev_port,
        servers,
    };
    apps.push(app.clone());
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_apps(&apps)?;
    crate::journal::record("app.add", Some(&name), None, None);
    println!("App '{name}' added.");
    report_env_file(&app);
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
        let address = app.dev_port.map(|p| format!("  http://{}.localhost -> :{p}", app.name)).unwrap_or_default();
        println!("{:width$}  {}{}{}", app.name, app.path, port, address);
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
    println!("  Env:      {} -> {}", app.gateway_env_name(), app.env_file_name());
    let door = store::load_state()?
        .gateway
        .and_then(|gateway| gateway.front_port)
        .unwrap_or(crate::front::PORT);
    match app.dev_port {
        Some(port) => println!("  Address:  {} (dev server port {port})", crate::front::address(&app.name, door)),
        None => println!(
            "  Address:  {} (dev server port assigned on the first `turnout dev`)",
            crate::front::address(&app.name, door)
        ),
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
    env_var: Option<String>,
    env_file: Option<String>,
    dev_port: Option<u16>,
    dist: Option<String>,
    overrides: Vec<String>,
    add_servers: Vec<String>,
    rm_servers: Vec<String>,
) -> Result<()> {
    let mut apps = store::load_apps()?;
    let known = store::load_servers()?;
    let index = apps.iter().position(|a| a.name == name).ok_or_else(|| unknown_app(name))?;
    let no_flags = path.is_none()
        && port.is_none()
        && env_var.is_none()
        && env_file.is_none()
        && dev_port.is_none()
        && dist.is_none()
        && overrides.is_empty()
        && add_servers.is_empty()
        && rm_servers.is_empty();

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
        if let Some(port) = app.gateway_port {
            ensure_port_is_free(&apps, port, name)?;
        }
        let app = &mut apps[index];
        if app.gateway_port.is_some() {
            let env_var: String = Input::new()
                .with_prompt("Variable that carries the gateway URL to the app")
                .default(app.gateway_env_name().to_string())
                .interact_text()?;
            app.gateway_env = Some(validate_env_name(env_var)?);
        }
        if !known.is_empty() {
            let current = app.servers.clone();
            app.servers = pick_servers(&known, &current)?;
        }
        println!("Commands are edited with flags: `turnout app edit {name} --command NAME=CMD` (NAME= removes).");
    } else {
        if let Some(path) = path {
            let path = crate::utils::project_dir(&path)?;
            apps[index].path = path.display().to_string();
        }
        if let Some(port) = port {
            ensure_port_is_free(&apps, port, name)?;
            apps[index].gateway_port = Some(port);
        }
        if let Some(env_var) = env_var {
            apps[index].gateway_env = Some(validate_env_name(env_var)?);
        }
        if let Some(env_file) = env_file {
            apps[index].env_file = Some(validate_env_file(env_file)?);
        }
        if let Some(dev_port) = dev_port {
            // 0 hands the port back: the next `dev` assigns a fresh one.
            let wanted = (dev_port != 0).then_some(dev_port);
            if let Some(port) = wanted {
                crate::front::ensure_dev_port_is_free(&apps, port, name)?;
            }
            apps[index].dev_port = wanted;
        }
        let app = &mut apps[index];
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
    crate::journal::record("app.edit", Some(name), None, None);
    println!("App '{name}' updated.");
    report_env_file(&apps[index]);
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
    // A target names exactly one app; without it there is nothing left to
    // deploy, so it goes too.
    let mut targets = store::load_targets()?;
    let dropped: Vec<String> = targets.iter().filter(|t| t.app == name).map(|t| t.name.clone()).collect();
    if !dropped.is_empty() {
        targets.retain(|t| t.app != name);
        store::save_targets(&targets)?;
        println!("Removed targets: {}.", dropped.join(", "));
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

/// Bring the app's dotenv file in step and say what happened. A file that
/// cannot be written is reported, not fatal: the catalog is already saved.
fn report_env_file(app: &App) {
    use crate::envfile::Outcome;
    match crate::envfile::write(app) {
        Ok(Outcome::Written(assignment)) => println!("Wrote {} ({assignment}).", crate::envfile::path_of(app).display()),
        Ok(Outcome::Removed) => println!("Removed the gateway line from {}.", crate::envfile::path_of(app).display()),
        Ok(Outcome::Unchanged | Outcome::Nothing) => {}
        Err(error) => eprintln!("warning: {error:#}"),
    }
}

/// A variable name a shell and a dotenv parser both accept.
fn validate_env_name(name: String) -> Result<String> {
    let name = name.trim().to_string();
    let valid = !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        bail!("'{name}' is not a variable name: letters, digits and underscores, not starting with a digit");
    }
    Ok(name)
}

/// A dotenv file name inside the project: no directories, no absolute paths.
fn validate_env_file(file: String) -> Result<String> {
    let file = file.trim().to_string();
    if file.is_empty() || file.contains(['/', '\\']) {
        bail!("'{file}' is not a file name in the project directory - pass a bare name such as .env.development.local");
    }
    Ok(file)
}

/// The variable a framework can actually see: Vite exposes only `VITE_*` to
/// the client, so a Vite project gets that prefix by default.
fn suggested_env_name(path: &Path) -> &'static str {
    let vite = std::fs::read_to_string(path.join("package.json")).is_ok_and(|text| text.contains("\"vite\""));
    if vite { "VITE_API_URL" } else { crate::model::DEFAULT_GATEWAY_ENV }
}

/// A gateway port belongs to exactly one app.
///
/// Two apps on one port used to be accepted here and only surface as the
/// gateway dying on its second `bind` - after `start` had reported success.
/// `this` is the app being added or edited, so keeping its own port is fine.
fn ensure_port_is_free(apps: &[App], port: u16, this: &str) -> Result<()> {
    if let Some(other) = apps.iter().find(|a| a.name != this && a.gateway_port == Some(port)) {
        bail!(
            "port {port} is already used by app '{0}' - pick another, or move '{0}' first with `turnout app edit {0} --port PORT`",
            other.name
        );
    }
    Ok(())
}
