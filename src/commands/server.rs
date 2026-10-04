use anyhow::{Result, bail};
use dialoguer::{Confirm, Input};

use serde::Serialize;

use crate::cli::{Format, ServerCommand};
use crate::model::{Server, parse_host, validate_name, validate_url};
use crate::{output, pick, store};

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
        ServerCommand::List { format } => list(format),
        ServerCommand::Show { name, format } => show(&resolve(name, "Show server")?, format),
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

/// A server as `server list` and `server show` report it.
#[derive(Serialize)]
pub struct ServerView {
    pub name: String,
    pub label: Option<String>,
    pub url: String,
    /// Where SSH connects, and whether the host is the URL's own.
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_host_from_url: bool,
    pub credential: Option<String>,
    /// Self-signed and otherwise invalid certificates are accepted here.
    pub accept_invalid_certs: bool,
    /// The deploy targets that land on this server.
    pub targets: Vec<Landing>,
    /// Apps whose allow-list names this server.
    pub allowed_for: Vec<String>,
    /// Apps bound to this server right now.
    pub bound: Vec<String>,
}

/// A deploy target as seen from the server it lands on.
#[derive(Serialize)]
pub struct Landing {
    pub target: String,
    pub app: String,
    pub path: String,
    /// The directory the path names; `null` when the path is gone from the
    /// catalog - exactly what a deploy would fail on.
    pub dir: Option<String>,
}

/// Every server as a view, in catalog order.
pub fn views() -> Result<Vec<ServerView>> {
    let targets = store::load_targets()?;
    let paths = store::load_paths()?;
    let apps = store::load_apps()?;
    let state = store::load_state()?;
    Ok(store::load_servers()?
        .iter()
        .map(|server| ServerView {
            name: server.name.clone(),
            label: server.label.clone(),
            url: server.url.clone(),
            ssh_host: server.ssh_host(),
            ssh_port: server.port,
            ssh_host_from_url: server.host.is_none(),
            credential: server.credential.clone(),
            accept_invalid_certs: server.accept_invalid_certs,
            // Which deploy targets land here. The server used to own this
            // list; since v0.11.0 the targets do, and this reads it back the
            // other way round.
            targets: targets
                .iter()
                .filter(|target| target.server == server.name)
                .map(|target| Landing {
                    target: target.name.clone(),
                    app: target.app.clone(),
                    path: target.path.clone(),
                    dir: paths.iter().find(|path| path.name == target.path).map(|path| path.dir.clone()),
                })
                .collect(),
            allowed_for: apps
                .iter()
                .filter(|app| app.servers.contains(&server.name))
                .map(|app| app.name.clone())
                .collect(),
            bound: state
                .bindings
                .iter()
                .filter(|(_, bound)| **bound == server.name)
                .map(|(app, _)| app.clone())
                .collect(),
        })
        .collect())
}

fn list(format: Format) -> Result<()> {
    output::emit(format, &views()?, |servers| {
        if servers.is_empty() {
            return "No servers yet - run `turnout server add`.".to_string();
        }
        let width = servers.iter().map(|s| s.name.len()).max().unwrap_or(0);
        let lines: Vec<String> = servers
            .iter()
            .map(|server| {
                let label = server.label.as_ref().map(|l| format!("  ({l})")).unwrap_or_default();
                format!("{:width$}  {}{}", server.name, server.url, label)
            })
            .collect();
        lines.join("\n")
    })
}

fn show(name: &str, format: Format) -> Result<()> {
    let server = views()?.into_iter().find(|s| s.name == name).ok_or_else(|| unknown_server(name))?;
    output::emit(format, &server, |server| {
        let mut lines = vec![server.name.clone()];
        if let Some(label) = &server.label {
            lines.push(format!("  Label:      {label}"));
        }
        lines.push(format!("  URL:        {}", server.url));
        // Say when the host is inherited: it is the difference between
        // "nothing is configured" and "it is the same name as the URL".
        lines.push(if server.ssh_host_from_url {
            format!("  SSH:        {}:{} (from the URL)", server.ssh_host, server.ssh_port)
        } else {
            format!("  SSH:        {}:{}", server.ssh_host, server.ssh_port)
        });
        lines.push(match &server.credential {
            Some(credential) => format!("  Credential: {credential}"),
            None => format!("  Credential: none - set one with `turnout server edit {} --credential NAME`", server.name),
        });
        lines.push(format!(
            "  TLS:        {}",
            if server.accept_invalid_certs {
                "accept invalid certificates"
            } else {
                "verify certificates"
            }
        ));
        if !server.targets.is_empty() {
            lines.push("  Targets:".to_string());
            for landing in &server.targets {
                lines.push(match &landing.dir {
                    Some(dir) => format!("    {}: {} → {dir}", landing.target, landing.app),
                    // A name with nothing behind it is worth showing rather
                    // than hiding: it is exactly what a deploy would fail on.
                    None => format!("    {}: {} → {} (missing from the path catalog)", landing.target, landing.app, landing.path),
                });
            }
        }
        if !server.allowed_for.is_empty() {
            lines.push(format!("  Used by:    {}", server.allowed_for.join(", ")));
        }
        if !server.bound.is_empty() {
            lines.push(format!("  Bound:      {}", server.bound.join(", ")));
        }
        lines.join("\n")
    })
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
    // An app bound to a server that is gone would make the gateway answer
    // every request with "gone from the catalog"; unbound, `use` says what to do.
    let mut state = store::load_state()?;
    let unbound = state.forget_server(name);
    store::save_state(&state)?;
    if !unbound.is_empty() {
        println!("Unbound: {} - point them elsewhere with `turnout use APP SERVER`.", unbound.join(", "));
    }
    crate::journal::record("server.remove", None, Some(name), None);
    println!("Server '{name}' removed.");
    Ok(())
}

fn unknown_server(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no server named '{name}' - see `turnout server list`")
}
