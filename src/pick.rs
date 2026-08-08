//! Interactive fallbacks for arguments the user did not pass.
//!
//! The rule across the CLI: on a terminal a missing name opens a picker, and
//! everywhere else the command fails exactly as it did before, so scripts and
//! CI keep their old behaviour.

use std::io::IsTerminal;

use anyhow::{Result, bail};
use dialoguer::Select;

use crate::model::{App, Credential, Group, Server, State};

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Show `prompt` over `labels` and return the index the user picked.
fn select(prompt: &str, labels: &[String]) -> Result<usize> {
    Ok(Select::new().with_prompt(prompt).items(labels).default(0).interact()?)
}

/// Pick an app by name, listing where each one currently points.
pub fn app(apps: &[App], state: &State, prompt: &str) -> Result<String> {
    if apps.is_empty() {
        bail!("no apps in the catalog yet - run `turnout app add` first");
    }
    if !interactive() {
        bail!("app name is required outside an interactive terminal");
    }
    let width = apps.iter().map(|a| a.name.len()).max().unwrap_or(0);
    let labels: Vec<String> = apps
        .iter()
        .map(|app| match state.bindings.get(&app.name) {
            Some(server) => format!("{:width$}  -> {server}", app.name),
            None => format!("{:width$}     (unbound)", app.name),
        })
        .collect();
    Ok(apps[select(prompt, &labels)?].name.clone())
}

/// Pick a server by name, showing the URL so stands are told apart.
pub fn server(servers: &[Server], prompt: &str) -> Result<String> {
    if servers.is_empty() {
        bail!("no servers in the catalog yet - run `turnout server add` first");
    }
    if !interactive() {
        bail!("server name is required outside an interactive terminal");
    }
    let width = servers.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let labels: Vec<String> = servers.iter().map(|s| format!("{:width$}  {}", s.name, s.url)).collect();
    Ok(servers[select(prompt, &labels)?].name.clone())
}

/// Pick a server the app is allowed to use; unrestricted apps see them all.
pub fn server_for_app(servers: &[Server], app: &App, prompt: &str) -> Result<String> {
    let allowed: Vec<Server> = if app.servers.is_empty() {
        servers.to_vec()
    } else {
        servers.iter().filter(|s| app.servers.contains(&s.name)).cloned().collect()
    };
    if allowed.is_empty() {
        bail!(
            "no servers allowed for '{}' - add one with `turnout app edit {} --add-server NAME`",
            app.name,
            app.name
        );
    }
    server(&allowed, prompt)
}

/// Pick a group by name, listing its members.
pub fn group(groups: &[Group], prompt: &str) -> Result<String> {
    if groups.is_empty() {
        bail!("no groups yet - run `turnout group add` first");
    }
    if !interactive() {
        bail!("group name is required outside an interactive terminal");
    }
    let width = groups.iter().map(|g| g.name.len()).max().unwrap_or(0);
    let labels: Vec<String> = groups.iter().map(|g| format!("{:width$}  {}", g.name, g.apps.join(", "))).collect();
    Ok(groups[select(prompt, &labels)?].name.clone())
}

/// Pick an app or a group - what `use` accepts as its first argument.
pub fn app_or_group(apps: &[App], groups: &[Group], state: &State, prompt: &str) -> Result<String> {
    if groups.is_empty() {
        return app(apps, state, prompt);
    }
    if apps.is_empty() {
        bail!("no apps in the catalog yet - run `turnout app add` first");
    }
    if !interactive() {
        bail!("app or group name is required outside an interactive terminal");
    }
    let mut names: Vec<String> = Vec::with_capacity(apps.len() + groups.len());
    let mut labels: Vec<String> = Vec::with_capacity(apps.len() + groups.len());
    let width = apps.iter().map(|a| a.name.len()).chain(groups.iter().map(|g| g.name.len())).max().unwrap_or(0);
    for group in groups {
        names.push(group.name.clone());
        labels.push(format!("{:width$}  group: {}", group.name, group.apps.join(", ")));
    }
    for app in apps {
        names.push(app.name.clone());
        labels.push(match state.bindings.get(&app.name) {
            Some(server) => format!("{:width$}  -> {server}", app.name),
            None => format!("{:width$}     (unbound)", app.name),
        });
    }
    Ok(names[select(prompt, &labels)?].clone())
}

/// Pick a stored credential, which identifies both the server and the kind.
/// `kind` narrows the list when the user passed `--kind` explicitly.
pub fn credential(credentials: &[Credential], kind: Option<&str>, prompt: &str) -> Result<(String, String)> {
    let matching: Vec<&Credential> = credentials.iter().filter(|c| kind.is_none_or(|k| c.kind == k)).collect();
    if matching.is_empty() {
        bail!("no access saved yet - run `turnout pass set` first");
    }
    if !interactive() {
        bail!("server name is required outside an interactive terminal");
    }
    if let [only] = matching.as_slice() {
        return Ok((only.server.clone(), only.kind.clone()));
    }
    let width = matching.iter().map(|c| c.server.len()).max().unwrap_or(0);
    let labels: Vec<String> = matching
        .iter()
        .map(|c| format!("{:width$}  {:10} login {}", c.server, c.kind, c.login))
        .collect();
    let picked = matching[select(prompt, &labels)?];
    Ok((picked.server.clone(), picked.kind.clone()))
}
