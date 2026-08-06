use std::io::IsTerminal;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};

use crate::cli::ServerCommand;
use crate::model::{Server, parse_ssh, validate_name, validate_url};
use crate::{secrets, store};

pub fn run(command: ServerCommand) -> Result<()> {
    match command {
        ServerCommand::Add {
            name,
            url,
            label,
            ssh,
            insecure,
        } => add(name, url, label, ssh, insecure),
        ServerCommand::List => list(),
        ServerCommand::Show { name } => show(&name),
        ServerCommand::Edit {
            name,
            url,
            label,
            ssh,
            insecure,
            secure,
            deploy_paths,
            restart_cmds,
        } => edit(&name, url, label, ssh, insecure, secure, deploy_paths, restart_cmds),
        ServerCommand::Remove { name, assume_yes } => remove(&name, assume_yes),
    }
}

fn add(name: Option<String>, url: Option<String>, label: Option<String>, ssh: Option<String>, insecure: bool) -> Result<()> {
    let mut servers = store::load_servers()?;
    let wizard = name.is_none() || url.is_none();
    if wizard && !std::io::stdin().is_terminal() {
        bail!("server name and --url are required outside an interactive terminal");
    }

    let name = match name {
        Some(name) => name,
        None => Input::new()
            .with_prompt("Server name")
            .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_name(&name)?;
    if servers.iter().any(|s| s.name == name) {
        bail!("server '{name}' already exists");
    }

    let url = match url {
        Some(url) => url,
        None => Input::new()
            .with_prompt("Base URL (e.g. https://staging.example.com)")
            .validate_with(|s: &String| validate_url(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_url(&url)?;

    let label = match label {
        Some(label) => Some(label),
        None if wizard => {
            let answer: String = Input::new().with_prompt("Label (empty to skip)").allow_empty(true).interact_text()?;
            if answer.trim().is_empty() { None } else { Some(answer.trim().to_string()) }
        }
        None => None,
    };

    let ssh = match ssh {
        Some(spec) => Some(parse_ssh(&spec)?),
        None if wizard => {
            let answer: String = Input::new()
                .with_prompt("SSH access as user@host[:port] (empty to skip)")
                .allow_empty(true)
                .interact_text()?;
            if answer.trim().is_empty() { None } else { Some(parse_ssh(answer.trim())?) }
        }
        None => None,
    };

    let accept_invalid_certs = if wizard && !insecure && url.starts_with("https://") {
        Confirm::new()
            .with_prompt("Accept self-signed / invalid TLS certificates for this server?")
            .default(false)
            .interact()?
    } else {
        insecure
    };

    servers.push(Server {
        name: name.clone(),
        label,
        url,
        ssh,
        accept_invalid_certs,
        deploy: Default::default(),
    });
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_servers(&servers)?;
    println!("Server '{name}' added.");
    Ok(())
}

fn list() -> Result<()> {
    let servers = store::load_servers()?;
    if servers.is_empty() {
        println!("No servers yet - run `turnout server add`.");
        return Ok(());
    }
    let width = servers.iter().map(|s| s.name.len()).max().unwrap_or(0);
    for server in servers {
        let label = server.label.map(|l| format!("  ({l})")).unwrap_or_default();
        println!("{:width$}  {}{}", server.name, server.url, label);
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let servers = store::load_servers()?;
    let server = servers.iter().find(|s| s.name == name).ok_or_else(|| unknown_server(name))?;
    println!("{}", server.name);
    if let Some(label) = &server.label {
        println!("  Label:    {label}");
    }
    println!("  URL:      {}", server.url);
    match &server.ssh {
        Some(ssh) => println!("  SSH:      {}@{}:{}", ssh.user, ssh.host, ssh.port),
        None => println!("  SSH:      not configured"),
    }
    println!(
        "  TLS:      {}",
        if server.accept_invalid_certs {
            "accept invalid certificates"
        } else {
            "verify certificates"
        }
    );
    let credentials = store::load_credentials()?;
    let kinds: Vec<&str> = credentials.iter().filter(|c| c.server == name).map(|c| c.kind.as_str()).collect();
    if !kinds.is_empty() {
        println!("  Access:   {} (secrets in the OS keyring)", kinds.join(", "));
    }
    if !server.deploy.is_empty() {
        println!("  Deploy:");
        for (app, target) in &server.deploy {
            match &target.restart {
                Some(restart) => println!("    {app}: {} (then: {restart})", target.path),
                None => println!("    {app}: {}", target.path),
            }
        }
    }
    let apps = store::load_apps()?;
    let used_by: Vec<&str> = apps.iter().filter(|a| a.servers.iter().any(|s| s == name)).map(|a| a.name.as_str()).collect();
    if !used_by.is_empty() {
        println!("  Used by:  {}", used_by.join(", "));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edit(
    name: &str,
    url: Option<String>,
    label: Option<String>,
    ssh: Option<String>,
    insecure: bool,
    secure: bool,
    deploy_paths: Vec<String>,
    restart_cmds: Vec<String>,
) -> Result<()> {
    let mut servers = store::load_servers()?;
    let index = servers.iter().position(|s| s.name == name).ok_or_else(|| unknown_server(name))?;
    let no_flags = url.is_none() && label.is_none() && ssh.is_none() && !insecure && !secure && deploy_paths.is_empty() && restart_cmds.is_empty();

    if no_flags {
        if !std::io::stdin().is_terminal() {
            bail!("nothing to change: pass flags, or run interactively for the wizard");
        }
        let server = &mut servers[index];
        let url: String = Input::new()
            .with_prompt("Base URL")
            .default(server.url.clone())
            .validate_with(|s: &String| validate_url(s).map_err(|e| e.to_string()))
            .interact_text()?;
        server.url = url;
        let label: String = Input::new()
            .with_prompt("Label (empty to unset)")
            .default(server.label.clone().unwrap_or_default())
            .allow_empty(true)
            .interact_text()?;
        server.label = if label.trim().is_empty() { None } else { Some(label.trim().to_string()) };
        let current_ssh = server.ssh.as_ref().map(|s| format!("{}@{}:{}", s.user, s.host, s.port)).unwrap_or_default();
        let ssh: String = Input::new()
            .with_prompt("SSH as user@host[:port] (empty to unset)")
            .default(current_ssh)
            .allow_empty(true)
            .interact_text()?;
        server.ssh = if ssh.trim().is_empty() { None } else { Some(parse_ssh(ssh.trim())?) };
        server.accept_invalid_certs = Confirm::new()
            .with_prompt("Accept self-signed / invalid TLS certificates?")
            .default(server.accept_invalid_certs)
            .interact()?;
    } else {
        let server = &mut servers[index];
        if let Some(url) = url {
            validate_url(&url)?;
            server.url = url;
        }
        if label.is_some() {
            server.label = label;
        }
        if let Some(spec) = ssh {
            server.ssh = Some(parse_ssh(&spec)?);
        }
        if insecure {
            server.accept_invalid_certs = true;
        }
        if secure {
            server.accept_invalid_certs = false;
        }
        let known_apps = store::load_apps()?;
        for spec in &deploy_paths {
            let (app, dir) = split_spec(spec)?;
            if dir.is_empty() {
                server.deploy.remove(app);
            } else {
                if !known_apps.iter().any(|a| a.name == app) {
                    bail!("no app named '{app}' - see `turnout app list`");
                }
                server
                    .deploy
                    .entry(app.to_string())
                    .or_insert_with(|| crate::model::DeployTarget {
                        path: String::new(),
                        restart: None,
                    })
                    .path = dir.to_string();
            }
        }
        for spec in &restart_cmds {
            let (app, cmd) = split_spec(spec)?;
            let Some(target) = server.deploy.get_mut(app) else {
                bail!("app '{app}' has no deploy path on '{name}' - set it first with --deploy-path {app}=DIR");
            };
            target.restart = if cmd.is_empty() { None } else { Some(cmd.to_string()) };
        }
    }
    store::save_servers(&servers)?;
    println!("Server '{name}' updated.");
    Ok(())
}

fn split_spec(spec: &str) -> Result<(&str, &str)> {
    let (app, value) = spec
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("invalid value '{spec}': expected APP=VALUE"))?;
    if app.is_empty() {
        bail!("invalid value '{spec}': expected APP=VALUE");
    }
    Ok((app, value))
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut servers = store::load_servers()?;
    if !servers.iter().any(|s| s.name == name) {
        return Err(unknown_server(name));
    }
    let confirmed = assume_yes
        || Confirm::new()
            .with_prompt(format!("Remove server '{name}' from the catalog?"))
            .default(false)
            .interact()?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    servers.retain(|s| s.name != name);
    store::save_servers(&servers)?;

    let mut apps = store::load_apps()?;
    let mut touched = Vec::new();
    for app in apps.iter_mut() {
        if app.servers.iter().any(|s| s == name) {
            app.servers.retain(|s| s != name);
            touched.push(app.name.clone());
        }
    }
    if !touched.is_empty() {
        store::save_apps(&apps)?;
        println!("Removed '{name}' from apps: {}.", touched.join(", "));
    }

    let mut credentials = store::load_credentials()?;
    let kinds: Vec<String> = credentials.iter().filter(|c| c.server == name).map(|c| c.kind.clone()).collect();
    if !kinds.is_empty() {
        for kind in &kinds {
            if let Err(err) = secrets::delete(name, kind) {
                eprintln!("warning: {err:#}");
            }
        }
        credentials.retain(|c| c.server != name);
        store::save_credentials(&credentials)?;
        println!("Removed stored access: {}.", kinds.join(", "));
    }
    println!("Server '{name}' removed.");
    Ok(())
}

fn unknown_server(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no server named '{name}' - see `turnout server list`")
}
