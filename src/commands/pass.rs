use std::io::Read;

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, Password};

use crate::cli::PassCommand;
use crate::model::Credential;
use crate::{pick, secrets, store};

pub fn run(command: PassCommand) -> Result<()> {
    match command {
        PassCommand::Set { server, kind, login } => set(server, &kind, login),
        PassCommand::Copy { server, kind, login, show } => copy(server, kind, login, show),
        PassCommand::Show { server } => show(server),
        PassCommand::List => list(),
        PassCommand::Remove { server, kind, assume_yes } => remove(server, kind, assume_yes),
    }
}

/// Resolve the (server, kind) pair a command works on. A given server narrows
/// the choice to its own kinds; with neither argument the picker lists
/// everything stored.
fn resolve(server: Option<String>, kind: Option<String>, prompt: &str) -> Result<(String, String)> {
    let credentials = store::load_credentials()?;
    match (server, kind) {
        (Some(server), Some(kind)) => Ok((server, kind)),
        (Some(server), None) => {
            let matching: Vec<&Credential> = credentials.iter().filter(|c| c.server == server).collect();
            match matching.as_slice() {
                [] => bail!("no access saved for '{server}' - run `turnout pass set {server}`"),
                [only] => Ok((server.clone(), only.kind.clone())),
                _ => {
                    let owned: Vec<Credential> = matching.into_iter().cloned().collect();
                    pick::credential(&owned, None, prompt)
                }
            }
        }
        (None, kind) => pick::credential(&credentials, kind.as_deref(), prompt),
    }
}

fn set(server: Option<String>, kind: &str, login: Option<String>) -> Result<()> {
    let servers = store::load_servers()?;
    if servers.is_empty() {
        bail!("no servers in the catalog yet - run `turnout server add` first");
    }
    let interactive = pick::interactive();

    let server = match server {
        Some(server) => server,
        None => pick::server(&servers, "Server")?,
    };
    if !servers.iter().any(|s| s.name == server) {
        bail!("no server named '{server}' - see `turnout server list`");
    }

    let mut credentials = store::load_credentials()?;
    let existing = credentials.iter().position(|c| c.server == server && c.kind == kind);

    let login = match login {
        Some(login) => login,
        None => {
            if !interactive {
                bail!("--login is required outside an interactive terminal");
            }
            let current = existing.map(|i| credentials[i].login.clone()).unwrap_or_default();
            let mut input = Input::new().with_prompt("Login");
            if !current.is_empty() {
                input = input.default(current);
            }
            input.interact_text()?
        }
    };

    let secret = if interactive {
        Password::new()
            .with_prompt(format!("Secret ({kind})"))
            .with_confirmation("Repeat to confirm", "Values do not match")
            .interact()?
    } else {
        // Scripted use: the secret arrives on stdin so it never lands in shell history.
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer).context("cannot read the secret from stdin")?;
        buffer.trim_end_matches(['\r', '\n']).to_string()
    };
    if secret.is_empty() {
        bail!("empty secret - nothing saved");
    }

    secrets::set(&server, kind, &secret)?;
    match existing {
        Some(index) => credentials[index].login = login,
        None => credentials.push(Credential {
            server: server.clone(),
            kind: kind.to_string(),
            login,
        }),
    }
    credentials.sort_by(|a, b| (a.server.as_str(), a.kind.as_str()).cmp(&(b.server.as_str(), b.kind.as_str())));
    store::save_credentials(&credentials)?;
    println!("Access to '{server}' ({kind}) saved.");
    Ok(())
}

fn copy(server: Option<String>, kind: Option<String>, login: bool, show: bool) -> Result<()> {
    let (server, kind) = resolve(server, kind, "Copy access for")?;
    let (server, kind) = (server.as_str(), kind.as_str());
    let credentials = store::load_credentials()?;
    let credential = credentials
        .iter()
        .find(|c| c.server == server && c.kind == kind)
        .ok_or_else(|| anyhow::anyhow!("no access saved for '{server}' ({kind}) - run `turnout pass set {server}`"))?;
    let (value, what) = if login {
        (credential.login.clone(), "Login")
    } else {
        (secrets::get(server, kind)?, "Secret")
    };
    if show {
        println!("{value}");
        return Ok(());
    }
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(value))
        .context("cannot access the clipboard (use --show to print instead)")?;
    println!("{what} for '{server}' ({kind}) copied to the clipboard.");
    Ok(())
}

fn show(server: Option<String>) -> Result<()> {
    let credentials = store::load_credentials()?;
    let server = match server {
        Some(server) => server,
        // Only servers that actually have access saved are worth showing.
        None => pick::credential(&credentials, None, "Show access for")?.0,
    };
    let server = server.as_str();
    let matching: Vec<&Credential> = credentials.iter().filter(|c| c.server == server).collect();
    if matching.is_empty() {
        println!("No access saved for '{server}'.");
        return Ok(());
    }
    println!("{server}");
    for credential in matching {
        println!("  {:10} login {}", credential.kind, credential.login);
    }
    println!("Secrets are stored in the OS keyring; copy with `turnout pass copy {server}`.");
    Ok(())
}

fn list() -> Result<()> {
    let credentials = store::load_credentials()?;
    if credentials.is_empty() {
        println!("No access saved yet - run `turnout pass set`.");
        return Ok(());
    }
    let width = credentials.iter().map(|c| c.server.len()).max().unwrap_or(0);
    for credential in credentials {
        println!("{:width$}  {:10} login {}", credential.server, credential.kind, credential.login);
    }
    Ok(())
}

fn remove(server: Option<String>, kind: Option<String>, assume_yes: bool) -> Result<()> {
    let (server, kind) = resolve(server, kind, "Remove access for")?;
    let (server, kind) = (server.as_str(), kind.as_str());
    let mut credentials = store::load_credentials()?;
    if !credentials.iter().any(|c| c.server == server && c.kind == kind) {
        bail!("no access saved for '{server}' ({kind})");
    }
    let confirmed = assume_yes
        || Confirm::new()
            .with_prompt(format!("Remove access to '{server}' ({kind})?"))
            .default(false)
            .interact()?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    secrets::delete(server, kind)?;
    credentials.retain(|c| !(c.server == server && c.kind == kind));
    store::save_credentials(&credentials)?;
    println!("Access to '{server}' ({kind}) removed.");
    Ok(())
}
