//! The secrets credentials authenticate with, kept in the OS keyring.
//!
//! Since v0.9.0 a secret belongs to a credential rather than to a
//! (server, kind) pair: the same deploy account reaches several stands, and its
//! password is one thing, not one per stand.

use std::io::Read;

use anyhow::{Context, Result, bail};
use dialoguer::Password;

use crate::cli::PassCommand;
use crate::model::Auth;
use crate::{pick, secrets, store};

pub fn run(command: PassCommand) -> Result<()> {
    match command {
        PassCommand::Set { credential } => set(credential),
        PassCommand::Copy { credential, user, show } => copy(credential, user, show),
        PassCommand::List => list(),
        PassCommand::Remove { credential, assume_yes } => remove(credential, assume_yes),
    }
}

/// Resolve the credential a command works on, checking it exists.
fn resolve(name: Option<String>, prompt: &str) -> Result<crate::model::Credential> {
    let credentials = store::load_credentials()?;
    let name = match name {
        Some(name) => name,
        None => pick::credential(&credentials, prompt)?,
    };
    credentials
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| anyhow::anyhow!("no credential named '{name}' - see `turnout credential list`"))
}

fn set(name: Option<String>) -> Result<()> {
    let credential = resolve(name, "Set the secret of")?;
    let interactive = pick::interactive();

    // An agent credential keeps nothing here: the agent already holds the key
    // and its passphrase. Storing a secret under its name would sit unused and
    // read, on `credential show`, as though the login needed it.
    if credential.auth == Auth::Agent {
        bail!(
            "credential '{0}' signs in through the SSH agent and has no secret to store - add the key to the agent with `ssh-add PATH`",
            credential.name
        );
    }
    let prompt = if credential.auth == Auth::Key {
        format!("Passphrase for the key of '{}'", credential.name)
    } else {
        format!("Secret for '{}'", credential.name)
    };
    let secret = if interactive {
        Password::new()
            .with_prompt(prompt)
            .with_confirmation("Repeat to confirm", "Values do not match")
            .interact()?
    } else {
        // Scripted use: the secret arrives on stdin so it never lands in shell history.
        let mut buffer = String::new();
        std::io::stdin().read_to_string(&mut buffer).context("cannot read the secret from stdin")?;
        buffer.trim_end_matches(['\r', '\n']).to_string()
    };
    if secret.is_empty() {
        if interactive {
            bail!("empty secret - nothing saved");
        }
        // Off-terminal the secret comes from stdin, so an empty one usually
        // means nothing was piped in rather than that an empty value was meant.
        bail!(
            "no secret on stdin - pipe it in, e.g. `echo \"$PASSWORD\" | turnout pass set {}`",
            credential.name
        );
    }

    secrets::set(&credential.name, &secret)?;
    println!("Secret for '{}' saved to the OS keyring.", credential.name);
    Ok(())
}

fn copy(name: Option<String>, user: bool, show: bool) -> Result<()> {
    let credential = resolve(name, "Copy the secret of")?;
    let (value, what) = if user {
        (credential.user.clone(), "User")
    } else {
        (secrets::get(&credential.name)?, "Secret")
    };
    if show {
        println!("{value}");
        return Ok(());
    }
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(value))
        .context("cannot access the clipboard (use --show to print instead)")?;
    println!("{what} for '{}' copied to the clipboard.", credential.name);
    Ok(())
}

fn list() -> Result<()> {
    let credentials = store::load_credentials()?;
    if credentials.is_empty() {
        println!("No credentials yet - run `turnout credential add`.");
        return Ok(());
    }
    let width = credentials.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for credential in &credentials {
        let stored = if secrets::get(&credential.name).is_ok() { "stored" } else { "-" };
        println!("{:width$}  {}@  {:8}  {stored}", credential.name, credential.user, credential.auth);
    }
    println!("Secrets live in the OS keyring; copy one with `turnout pass copy NAME`.");
    Ok(())
}

fn remove(name: Option<String>, assume_yes: bool) -> Result<()> {
    let credential = resolve(name, "Remove the secret of")?;
    if secrets::get(&credential.name).is_err() {
        bail!("no secret stored for '{}'", credential.name);
    }
    let confirmed = pick::confirm_destructive(format!("Remove the stored secret of '{}'?", credential.name), assume_yes)?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    secrets::delete(&credential.name)?;
    // The credential itself stays: dropping it too would turn "I rotated my
    // password" into "I lost the account".
    println!("Secret for '{}' removed; the credential itself is unchanged.", credential.name);
    Ok(())
}
