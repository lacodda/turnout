use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};

use crate::cli::ServerCommand;
use crate::model::{Server, parse_host, validate_name, validate_url};
use crate::{pick, store};

pub fn run(command: ServerCommand) -> Result<()> {
    match command {
        ServerCommand::Add {
            name,
            url,
            label,
            host,
            credential,
            insecure,
        } => add(name, url, label, host, credential, insecure),
        ServerCommand::List => list(),
        ServerCommand::Show { name } => show(&resolve(name, "Show server")?),
        ServerCommand::Edit {
            name,
            url,
            label,
            host,
            credential,
            insecure,
            secure,
        } => {
            let name = resolve(name, "Edit server")?;
            edit(&name, url, label, host, credential, insecure, secure)
        }
        ServerCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove server")?;
            remove(&name, assume_yes)
        }
    }
}

/// Take the server name as given, or let the user pick one.
fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::server(&store::load_servers()?, prompt),
    }
}

/// Check a credential exists before a server is pointed at it.
fn check_credential(name: &str) -> Result<()> {
    if !store::load_credentials()?.iter().any(|c| c.name == name) {
        bail!("no credential named '{name}' - add one with `turnout credential add {name}`");
    }
    Ok(())
}

fn add(name: Option<String>, url: Option<String>, label: Option<String>, host: Option<String>, credential: Option<String>, insecure: bool) -> Result<()> {
    let mut servers = store::load_servers()?;
    let wizard = name.is_none() || url.is_none();
    if wizard {
        pick::ensure_interactive("server name and --url are required")?;
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

    // The URL's host is the common case, so the prompt says so and an empty
    // answer keeps it rather than demanding the same name twice.
    let (host, port) = match host {
        Some(spec) => {
            let (host, port) = parse_host(&spec)?;
            (Some(host), port)
        }
        None if wizard => {
            let answer: String = Input::new()
                .with_prompt("SSH host[:port] (empty to use the URL's host on 22)")
                .allow_empty(true)
                .interact_text()?;
            if answer.trim().is_empty() {
                (None, 22)
            } else {
                let (host, port) = parse_host(answer.trim())?;
                (Some(host), port)
            }
        }
        None => (None, 22),
    };

    let credential = match credential {
        Some(name) => {
            check_credential(&name)?;
            Some(name)
        }
        None if wizard => {
            let credentials = store::load_credentials()?;
            if credentials.is_empty() {
                println!("  No credentials yet - add one later with `turnout credential add`, then point this server at it.");
                None
            } else if Confirm::new().with_prompt("Point this server at a credential?").default(true).interact()? {
                Some(pick::credential(&credentials, "Logs in with")?)
            } else {
                None
            }
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
        host,
        port,
        accept_invalid_certs,
        credential,
        // Learned on the first command that needs a shell, not guessed here.
        shell: None,
    });
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_servers(&servers)?;
    crate::journal::record("server.add", None, Some(&name), None);
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
        let label = server.label.as_ref().map(|l| format!("  ({l})")).unwrap_or_default();
        println!("{:width$}  {}{}", server.name, server.url, label);
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let servers = store::load_servers()?;
    let server = servers.iter().find(|s| s.name == name).ok_or_else(|| unknown_server(name))?;
    println!("{}", server.name);
    if let Some(label) = &server.label {
        println!("  Label:      {label}");
    }
    println!("  URL:        {}", server.url);
    // Say when the host is inherited: it is the difference between "nothing is
    // configured" and "it is the same name as the URL".
    match &server.host {
        Some(host) => println!("  SSH:        {host}:{}", server.port),
        None => println!("  SSH:        {}:{} (from the URL)", server.ssh_host(), server.port),
    }
    match &server.credential {
        Some(credential) => println!("  Credential: {credential}"),
        None => println!("  Credential: none - set one with `turnout server edit {name} --credential NAME`"),
    }
    println!(
        "  TLS:        {}",
        if server.accept_invalid_certs {
            "accept invalid certificates"
        } else {
            "verify certificates"
        }
    );
    // Which deploy targets land here. The server used to own this list; since
    // v0.11.0 the targets do, and this reads it back the other way round.
    let targets = store::load_targets()?;
    let landing: Vec<&crate::model::Target> = targets.iter().filter(|t| t.server == name).collect();
    if !landing.is_empty() {
        let paths = store::load_paths()?;
        println!("  Targets:");
        for target in landing {
            match paths.iter().find(|p| p.name == target.path) {
                Some(path) => println!("    {}: {} → {}", target.name, target.app, path.dir),
                // A name with nothing behind it is worth showing rather than
                // hiding: it is exactly what a deploy would fail on.
                None => println!("    {}: {} → {} (missing from the path catalog)", target.name, target.app, target.path),
            }
        }
    }
    let apps = store::load_apps()?;
    let used_by: Vec<&str> = apps.iter().filter(|a| a.servers.iter().any(|s| s == name)).map(|a| a.name.as_str()).collect();
    if !used_by.is_empty() {
        println!("  Used by:    {}", used_by.join(", "));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edit(name: &str, url: Option<String>, label: Option<String>, host: Option<String>, credential: Option<String>, insecure: bool, secure: bool) -> Result<()> {
    let mut servers = store::load_servers()?;
    let index = servers.iter().position(|s| s.name == name).ok_or_else(|| unknown_server(name))?;
    let no_flags = url.is_none() && label.is_none() && host.is_none() && credential.is_none() && !insecure && !secure;

    if no_flags {
        pick::ensure_interactive("nothing to change: pass flags to edit non-interactively")?;
        let credentials = store::load_credentials()?;
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
        let current_host = server.host.as_ref().map(|h| format!("{h}:{}", server.port)).unwrap_or_default();
        let host: String = Input::new()
            .with_prompt("SSH host[:port] (empty to use the URL's host)")
            .default(current_host)
            .allow_empty(true)
            .interact_text()?;
        if host.trim().is_empty() {
            server.host = None;
            server.port = 22;
        } else {
            let (parsed, port) = parse_host(host.trim())?;
            server.host = Some(parsed);
            server.port = port;
        }
        if !credentials.is_empty() && Confirm::new().with_prompt("Change the credential?").default(false).interact()? {
            server.credential = Some(pick::credential(&credentials, "Logs in with")?);
        }
        server.accept_invalid_certs = Confirm::new()
            .with_prompt("Accept self-signed / invalid TLS certificates?")
            .default(server.accept_invalid_certs)
            .interact()?;
    } else {
        // Validated before the mutable borrow: both read from other catalogs.
        if let Some(credential) = credential.as_deref().filter(|c| !c.trim().is_empty()) {
            check_credential(credential)?;
        }
        let server = &mut servers[index];
        if let Some(url) = url {
            validate_url(&url)?;
            server.url = url;
        }
        if label.is_some() {
            server.label = label;
        }
        if let Some(spec) = host {
            if spec.trim().is_empty() {
                server.host = None;
                server.port = 22;
            } else {
                let (parsed, port) = parse_host(&spec)?;
                server.host = Some(parsed);
                server.port = port;
            }
        }
        if let Some(credential) = credential {
            server.credential = if credential.trim().is_empty() { None } else { Some(credential) };
        }
        if insecure {
            server.accept_invalid_certs = true;
        }
        if secure {
            server.accept_invalid_certs = false;
        }
    }
    store::save_servers(&servers)?;
    crate::journal::record("server.edit", None, Some(name), None);
    println!("Server '{name}' updated.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut servers = store::load_servers()?;
    if !servers.iter().any(|s| s.name == name) {
        return Err(unknown_server(name));
    }
    let confirmed = pick::confirm_destructive(format!("Remove server '{name}' from the catalog?"), assume_yes)?;
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

    // Targets do not outlive it: a target names exactly one server, so without
    // it there is no route left to describe.
    let mut targets = store::load_targets()?;
    let dropped: Vec<String> = targets.iter().filter(|t| t.server == name).map(|t| t.name.clone()).collect();
    if !dropped.is_empty() {
        targets.retain(|t| t.server != name);
        store::save_targets(&targets)?;
        println!("Removed targets: {}.", dropped.join(", "));
    }

    // Credentials and paths outlive the server on purpose: they are shared, and
    // deleting a login because one of the machines it reached went away would
    // take the others down with it.
    crate::journal::record("server.remove", None, Some(name), None);
    println!("Server '{name}' removed.");
    Ok(())
}

fn unknown_server(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no server named '{name}' - see `turnout server list`")
}
