//! Interactive fallbacks for arguments the user did not pass.
//!
//! The rule across the CLI: on a terminal a missing name opens a picker, and
//! everywhere else the command fails exactly as it did before, so scripts and
//! CI keep their old behaviour.
//!
//! Every prompt - the pickers here and the `dialoguer` prompts in the command
//! modules - must be preceded by [`ensure_interactive`]. `dialoguer` reads
//! stdin unconditionally: with no terminal it either blocks forever waiting for
//! input that will never come, or reads EOF and reports the empty value as if
//! the user had typed nothing. Both are worse than failing outright, and the
//! second one blames the wrong thing.

use std::io::IsTerminal;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Select};

use crate::model::{App, Credential, Group, Server, State};

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Fail with `reason` instead of prompting when there is nobody to answer.
///
/// `reason` states what is missing and how to supply it without a prompt -
/// which flag or argument - because that is what a script hitting this needs
/// to know. It is phrased as the missing thing, and this adds the context:
///
/// ```text
/// ensure_interactive("--login is required")
///   -> "--login is required: no terminal to prompt on"
/// ```
pub fn ensure_interactive(reason: &str) -> Result<()> {
    if !interactive() {
        bail!("{reason}: no terminal to prompt on");
    }
    Ok(())
}

/// Ask `prompt` before something destructive; `assume_yes` skips the question.
///
/// Without a terminal there is nobody to ask, and `dialoguer` would fail with a
/// bare "not a terminal" that says nothing about the way out - which is `--yes`.
pub fn confirm_destructive(prompt: String, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    ensure_interactive("this needs confirming; pass --yes to skip the prompt")?;
    Ok(Confirm::new().with_prompt(prompt).default(false).interact()?)
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
    ensure_interactive("app name is required")?;
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
    ensure_interactive("server name is required")?;
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
    ensure_interactive("group name is required")?;
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
    ensure_interactive("app or group name is required")?;
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

/// Pick a credential by name, showing who it logs in as and how.
pub fn credential(credentials: &[Credential], prompt: &str) -> Result<String> {
    if credentials.is_empty() {
        bail!("no credentials yet - run `turnout credential add` first");
    }
    // A single entry needs no picking, so it resolves without a terminal too.
    if let [only] = credentials {
        return Ok(only.name.clone());
    }
    ensure_interactive("credential name is required")?;
    let width = credentials.iter().map(|c| c.name.len()).max().unwrap_or(0);
    let labels: Vec<String> = credentials.iter().map(|c| format!("{:width$}  {}@  {}", c.name, c.user, c.auth)).collect();
    Ok(credentials[select(prompt, &labels)?].name.clone())
}

/// Pick a path by name, showing the directory so two roles are told apart.
pub fn path(paths: &[crate::model::Path], prompt: &str) -> Result<String> {
    if paths.is_empty() {
        bail!("no paths yet - run `turnout path add` first");
    }
    if let [only] = paths {
        return Ok(only.name.clone());
    }
    ensure_interactive("path name is required")?;
    let width = paths.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let labels: Vec<String> = paths.iter().map(|p| format!("{:width$}  {}", p.name, p.dir)).collect();
    Ok(paths[select(prompt, &labels)?].name.clone())
}
