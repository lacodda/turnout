//! Targets: the named deploy target - which app goes to which server, as whom,
//! and where.
//!
//! An entity of its own since v0.11.0 (ADR 0013). The relationship used to live
//! inside the server as a map from app to path, which meant it could not be
//! named, listed or reused - the same shape v0.9.0 pulled `user@host` out of the
//! server for.

use anyhow::{Result, bail};
use dialoguer::Input;

use serde::Serialize;

use crate::cli::{Format, HealthArgs, TargetCommand};
use crate::model::{HealthCheck, Probe, Target, unique_target_name, validate_health_url, validate_name, validate_url};
use crate::{output, pick, store};

pub fn run(command: TargetCommand) -> Result<()> {
    match command {
        TargetCommand::Add {
            name,
            app,
            server,
            credential,
            path,
            url,
            health,
        } => add(name, Parts { app, server, credential, path }, url, &health),
        TargetCommand::List { format } => list(format),
        TargetCommand::Show { name, format } => show(&resolve(name, "Show target")?, format),
        TargetCommand::Edit {
            name,
            server,
            credential,
            path,
            url,
            health,
        } => {
            let name = resolve(name, "Edit target")?;
            edit(&name, server, credential, path, url, &health)
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

/// The four entities a new target joins, each picked when not named.
struct Parts {
    app: Option<String>,
    server: Option<String>,
    credential: Option<String>,
    path: Option<String>,
}

/// A target's own address as typed: empty clears it, anything else has to be
/// a URL.
fn url_from(url: String) -> Result<Option<String>> {
    if url.is_empty() {
        return Ok(None);
    }
    validate_url(&url)?;
    Ok(Some(url.trim_end_matches('/').to_string()))
}

/// The health check a target ends up with, from what it had and the flags.
///
/// An empty probe clears the check; a new probe replaces the old one and keeps
/// its wait unless a wait is given too; a wait alone changes the check that is
/// there. A wait with no check to belong to is refused rather than dropped.
fn health_from(current: Option<HealthCheck>, args: &HealthArgs) -> Result<Option<HealthCheck>> {
    let probe = match (&args.health, &args.health_command) {
        (Some(url), _) if url.is_empty() => Some(None),
        (_, Some(command)) if command.is_empty() => Some(None),
        (Some(url), _) => {
            validate_health_url(url)?;
            // `//health` is how a Git Bash user keeps the path from being
            // rewritten; it means `/health`, and is stored that way.
            Some(Some(Probe::Url(crate::model::normalize_remote_path(url))))
        }
        (_, Some(command)) => Some(Some(Probe::Command(command.clone()))),
        (None, None) => None,
    };
    let orphan_wait = || anyhow::anyhow!("--health-wait belongs to a check: pass --health URL or --health-command CMD as well");
    match (probe, current) {
        (Some(None), _) if args.health_wait.is_some() => Err(orphan_wait()),
        (Some(None), _) => Ok(None),
        (Some(Some(probe)), current) => Ok(Some(HealthCheck {
            probe,
            wait: args.health_wait.or(current.and_then(|check| check.wait)),
        })),
        (None, Some(mut check)) => {
            if let Some(wait) = args.health_wait {
                check.wait = Some(wait);
            }
            Ok(Some(check))
        }
        (None, None) if args.health_wait.is_some() => Err(orphan_wait()),
        (None, None) => Ok(None),
    }
}

fn add(name: Option<String>, parts: Parts, url: Option<String>, health: &HealthArgs) -> Result<()> {
    let Parts { app, server, credential, path } = parts;
    // Read before anything is asked: a bad flag must fail at once, not after
    // three pickers.
    let url = url.map(url_from).transpose()?.flatten();
    let health = health_from(None, health)?;
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
        url,
        health,
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
        url: None,
        health: None,
    });
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_targets(&targets)?;
    crate::journal::record("target.add", Some(app), Some(server), Some(&name));
    Ok(Some(name))
}

/// A deploy target as `target list` and `target show` report it.
#[derive(Serialize)]
pub struct TargetView {
    pub name: String,
    pub app: String,
    pub server: String,
    pub credential: String,
    pub path: String,
    /// The directory the path names - "where does this land" is the question
    /// `show` is asked, and another name to look up is half an answer.
    /// `null` when the path is gone from the catalog.
    pub dir: Option<String>,
    /// The target's own address; `null` when it answers at the server's URL.
    pub url: Option<String>,
    /// Where the deployed app answers - the target's address or the server's
    /// URL; `null` when neither is known (the server is gone).
    pub address: Option<String>,
    /// How a deploy proves it worked; `null` when it is not checked.
    pub health: Option<HealthView>,
}

/// A health check as the views report it: every key always there, and the
/// wait as the seconds a deploy will actually spend, the default filled in.
#[derive(Serialize)]
pub struct HealthView {
    /// The URL asked, as the target names it; `null` for a command check.
    pub url: Option<String>,
    /// The command run on the server; `null` for a URL check.
    pub command: Option<String>,
    pub wait: u64,
    #[serde(skip)]
    check: HealthCheck,
}

impl From<HealthCheck> for HealthView {
    fn from(check: HealthCheck) -> Self {
        let (url, command) = match &check.probe {
            Probe::Url(url) => (Some(url.clone()), None),
            Probe::Command(command) => (None, Some(command.clone())),
        };
        Self {
            url,
            command,
            wait: check.wait().as_secs(),
            check,
        }
    }
}

/// Every target as a view, in catalog order.
pub fn views() -> Result<Vec<TargetView>> {
    let paths = store::load_paths()?;
    let servers = store::load_servers()?;
    Ok(store::load_targets()?
        .into_iter()
        .map(|target| TargetView {
            dir: paths.iter().find(|p| p.name == target.path).map(|p| p.dir.clone()),
            address: target
                .url
                .clone()
                .or_else(|| servers.iter().find(|s| s.name == target.server).map(|s| s.url.clone())),
            name: target.name,
            app: target.app,
            server: target.server,
            credential: target.credential,
            path: target.path,
            url: target.url,
            health: target.health.map(HealthView::from),
        })
        .collect())
}

fn list(format: Format) -> Result<()> {
    output::emit(format, &views()?, |targets| {
        if targets.is_empty() {
            return "No targets yet - run `turnout target add`.".to_string();
        }
        let width = targets.iter().map(|t| t.name.len()).max().unwrap_or(0);
        let lines: Vec<String> = targets
            .iter()
            .map(|target| {
                format!(
                    "{:width$}  {} -> {}  ({}@, {})",
                    target.name, target.app, target.server, target.credential, target.path
                )
            })
            .collect();
        lines.join("\n")
    })
}

fn show(name: &str, format: Format) -> Result<()> {
    let target = views()?.into_iter().find(|t| t.name == name).ok_or_else(|| unknown(name))?;
    output::emit(format, &target, |target| {
        [
            target.name.clone(),
            format!("  App:        {}", target.app),
            format!("  Server:     {}", target.server),
            format!("  Logs in as: {}", target.credential),
            match &target.dir {
                Some(dir) => format!("  Lands in:   {} ({dir})", target.path),
                None => format!("  Lands in:   {} (missing from the catalog)", target.path),
            },
            match (&target.url, &target.address) {
                (Some(url), _) => format!("  Answers at: {url}"),
                (None, Some(address)) => format!("  Answers at: {address} (the server's)"),
                (None, None) => "  Answers at: unknown - the server is missing from the catalog".to_string(),
            },
            match &target.health {
                Some(view) => format!(
                    "  Health:     {}, asked for up to {}s - a deploy that fails it is rolled back",
                    view.check.describe(target.address.as_deref().unwrap_or_default()),
                    view.wait
                ),
                None => format!("  Health:     not checked - turnout target edit {} --health /health", target.name),
            },
        ]
        .join("\n")
    })
}

fn edit(name: &str, server: Option<String>, credential: Option<String>, path: Option<String>, url: Option<String>, health: &HealthArgs) -> Result<()> {
    let mut targets = store::load_targets()?;
    let index = targets.iter().position(|t| t.name == name).ok_or_else(|| unknown(name))?;
    // Read before anything is written: a bad flag must not leave half an edit.
    let url = url.map(url_from).transpose()?;
    let check = health.any().then(|| health_from(targets[index].health.clone(), health)).transpose()?;
    if server.is_none() && credential.is_none() && path.is_none() && url.is_none() && check.is_none() {
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
        if let Some(url) = url {
            target.url = url;
        }
        if let Some(check) = check {
            target.health = check;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(health: Option<&str>, command: Option<&str>, wait: Option<u64>) -> HealthArgs {
        HealthArgs {
            health: health.map(str::to_string),
            health_command: command.map(str::to_string),
            health_wait: wait,
        }
    }

    fn check(probe: Probe, wait: Option<u64>) -> Option<HealthCheck> {
        Some(HealthCheck { probe, wait })
    }

    /// Each flag changes its own part of the check and nothing else: a new
    /// probe keeps the wait, a wait keeps the probe, an empty probe clears.
    #[test]
    fn the_health_flags_edit_one_part_each() {
        let current = check(Probe::Url("/health".into()), Some(90));
        assert_eq!(
            health_from(current.clone(), &flags(None, Some("curl -f localhost"), None)).unwrap(),
            check(Probe::Command("curl -f localhost".into()), Some(90))
        );
        assert_eq!(
            health_from(current.clone(), &flags(None, None, Some(30))).unwrap(),
            check(Probe::Url("/health".into()), Some(30))
        );
        assert_eq!(health_from(current.clone(), &flags(Some(""), None, None)).unwrap(), None);
        assert_eq!(health_from(current.clone(), &flags(None, Some(""), None)).unwrap(), None);
        assert_eq!(
            health_from(current, &flags(None, None, None)).unwrap(),
            check(Probe::Url("/health".into()), Some(90))
        );
        assert_eq!(
            health_from(None, &flags(Some("/up"), None, None)).unwrap(),
            check(Probe::Url("/up".into()), None)
        );
    }

    /// A wait with no check would be dropped on the floor; a bare word would
    /// be joined into an address nobody meant.
    #[test]
    fn a_check_that_cannot_mean_anything_is_refused() {
        assert!(health_from(None, &flags(None, None, Some(30))).is_err());
        assert!(health_from(check(Probe::Url("/h".into()), None), &flags(Some(""), None, Some(30))).is_err());
        assert!(health_from(None, &flags(Some("health"), None, None)).is_err());
        // What Git Bash makes of `/health` and `/`, named as such; and the
        // doubled slash that keeps it from doing so, read as the path.
        let rewritten = health_from(None, &flags(Some("C:/Program Files/Git/health"), None, None))
            .unwrap_err()
            .to_string();
        assert!(
            rewritten.contains("Git Bash rewrote '/health'") && rewritten.contains("MSYS_NO_PATHCONV"),
            "{rewritten}"
        );
        let root = health_from(None, &flags(Some("C:/Program Files/Git/"), None, None)).unwrap_err().to_string();
        assert!(root.contains("rewrote '/'"), "{root}");
        assert_eq!(
            health_from(None, &flags(Some("//health"), None, None)).unwrap(),
            check(Probe::Url("/health".into()), None)
        );
        assert_eq!(health_from(None, &flags(Some("//"), None, None)).unwrap(), check(Probe::Url("/".into()), None));
        assert!(url_from("admin.example.com".into()).is_err());
        assert_eq!(url_from(String::new()).unwrap(), None);
        assert_eq!(
            url_from("https://admin.example.com/".into()).unwrap().as_deref(),
            Some("https://admin.example.com")
        );
    }
}
