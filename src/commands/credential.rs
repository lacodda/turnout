//! Credentials: who logs in, and whether with a key or a password.
//!
//! Free-standing since v0.9.0. Before that a login was a field on a server, so
//! the same deploy account had to be re-entered for every stand it reached;
//! here it is one entry the servers point at.

use anyhow::{Result, bail};
use dialoguer::{Input, Select};

use crate::cli::CredentialCommand;
use crate::model::{Auth, Credential, validate_name};
use crate::{pick, secrets, store};

/// The auth kinds the wizards offer, and the labels for them.
///
/// One list, used by `add` and `edit` alike and index-matched by position, so
/// the two wizards cannot drift into offering different choices - or, worse,
/// into mapping the same position to different kinds.
pub const AUTH_KINDS: [Auth; 3] = [Auth::Password, Auth::Key, Auth::Agent];
pub const AUTH_LABELS: [&str; 3] = ["a password", "a private key file", "a key from the SSH agent"];

pub fn run(command: CredentialCommand) -> Result<()> {
    match command {
        CredentialCommand::Add { name, user, auth, key } => add(name, user, auth, key),
        CredentialCommand::List => list(),
        CredentialCommand::Show { name } => show(&resolve(name, "Show credential")?),
        CredentialCommand::Edit { name, user, auth, key } => {
            let name = resolve(name, "Edit credential")?;
            edit(&name, user, auth, key)
        }
        CredentialCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove credential")?;
            remove(&name, assume_yes)
        }
    }
}

fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::credential(&store::load_credentials()?, prompt),
    }
}

/// Parse the `--auth` value, or infer it: passing a key file means key auth.
fn parse_auth(auth: Option<String>, key: &Option<String>) -> Result<Option<Auth>> {
    match auth {
        Some(text) => Ok(Some(text.parse()?)),
        None if key.as_ref().is_some_and(|k| !k.trim().is_empty()) => Ok(Some(Auth::Key)),
        None => Ok(None),
    }
}

fn add(name: Option<String>, user: Option<String>, auth: Option<String>, key: Option<String>) -> Result<()> {
    let mut credentials = store::load_credentials()?;
    let wizard = name.is_none() || user.is_none();
    if wizard {
        pick::ensure_interactive("credential name and --user are required")?;
    }

    let name = match name {
        Some(name) => name,
        None => Input::new()
            .with_prompt("Credential name")
            .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_name(&name)?;
    if credentials.iter().any(|c| c.name == name) {
        bail!("credential '{name}' already exists");
    }

    let user = match user {
        Some(user) => user,
        None => Input::new().with_prompt("Logs in as (remote user)").interact_text()?,
    };
    if user.trim().is_empty() {
        bail!("the remote user cannot be empty");
    }

    let auth = match parse_auth(auth, &key)? {
        Some(auth) => auth,
        None if wizard => AUTH_KINDS[Select::new().with_prompt("Authenticates with").items(AUTH_LABELS).default(0).interact()?],
        None => Auth::Password,
    };

    let key = match key {
        // Only key auth reads a key file. Keeping the path on a password or
        // agent credential would store something nothing uses, and
        // `credential show` would print a `Key:` line for a login that never
        // opens it - a live run caught exactly that with `--auth agent --key`.
        _ if auth != Auth::Key => None,
        Some(key) if !key.trim().is_empty() => Some(key.trim().to_string()),
        _ if wizard => {
            let answer: String = Input::new().with_prompt("Private key file").interact_text()?;
            Some(answer.trim().to_string())
        }
        _ => None,
    };
    if auth == Auth::Key && key.is_none() {
        bail!("key authentication needs a key file - pass --key PATH");
    }

    credentials.push(Credential {
        name: name.clone(),
        user: user.trim().to_string(),
        auth,
        key,
    });
    credentials.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_credentials(&credentials)?;
    crate::journal::record("credential.add", None, None, Some(&name));
    println!("Credential '{name}' added.");
    match auth {
        Auth::Password => println!("Store its secret with `turnout pass set {name}`."),
        Auth::Agent => println!("It signs in with a key from the running SSH agent - add one with `ssh-add PATH`."),
        Auth::Key => {}
    }
    Ok(())
}

fn list() -> Result<()> {
    let credentials = store::load_credentials()?;
    if credentials.is_empty() {
        println!("No credentials yet - run `turnout credential add`.");
        return Ok(());
    }
    let width = credentials.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for credential in credentials {
        println!("{:width$}  {}@  {}", credential.name, credential.user, credential.auth);
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let credentials = store::load_credentials()?;
    let credential = credentials.iter().find(|c| c.name == name).ok_or_else(|| unknown(name))?;
    println!("{}", credential.name);
    println!("  User:     {}", credential.user);
    println!("  Auth:     {}", credential.auth);
    if let Some(key) = &credential.key {
        println!("  Key:      {key}");
    }
    // Whether a secret exists is useful; the secret itself never is.
    let stored = secrets::get(name).is_ok();
    println!(
        "  Secret:   {}",
        match credential.auth {
            _ if stored => "stored in the OS keyring",
            Auth::Key => "none (the key file is unprotected)",
            // An agent credential has no secret here by design: the passphrase
            // was given to the agent, not to turnout. Telling the user to run
            // `pass set` would be advice for a problem they do not have.
            Auth::Agent => "none (the SSH agent holds the key)",
            Auth::Password => "none - save one with `turnout pass set`",
        }
    );
    let servers = store::load_servers()?;
    let used_by: Vec<&str> = servers
        .iter()
        .filter(|s| s.credential.as_deref() == Some(name))
        .map(|s| s.name.as_str())
        .collect();
    if !used_by.is_empty() {
        println!("  Used by:  {}", used_by.join(", "));
    }
    Ok(())
}

fn edit(name: &str, user: Option<String>, auth: Option<String>, key: Option<String>) -> Result<()> {
    let mut credentials = store::load_credentials()?;
    let index = credentials.iter().position(|c| c.name == name).ok_or_else(|| unknown(name))?;
    let parsed_auth = parse_auth(auth, &key)?;
    let no_flags = user.is_none() && parsed_auth.is_none() && key.is_none();

    if no_flags {
        pick::ensure_interactive("nothing to change: pass flags to edit non-interactively")?;
        let credential = &mut credentials[index];
        credential.user = Input::new()
            .with_prompt("Logs in as (remote user)")
            .default(credential.user.clone())
            .interact_text()?;
        credential.auth = AUTH_KINDS[Select::new()
            .with_prompt("Authenticates with")
            .items(AUTH_LABELS)
            .default(AUTH_KINDS.iter().position(|&a| a == credential.auth).unwrap_or(0))
            .interact()?];
        if credential.auth == Auth::Key {
            let answer: String = Input::new()
                .with_prompt("Private key file")
                .default(credential.key.clone().unwrap_or_default())
                .interact_text()?;
            credential.key = Some(answer.trim().to_string());
        } else {
            // Password and agent alike: no file is read, so none is kept.
            credential.key = None;
        }
    } else {
        let credential = &mut credentials[index];
        if let Some(user) = user {
            if user.trim().is_empty() {
                bail!("the remote user cannot be empty");
            }
            credential.user = user.trim().to_string();
        }
        if let Some(auth) = parsed_auth {
            credential.auth = auth;
        }
        if let Some(key) = key {
            credential.key = if key.trim().is_empty() { None } else { Some(key.trim().to_string()) };
        }
        // Switching away from key auth drops the file with it, for the same
        // reason `add` never stores one: nothing would read it afterwards.
        if credential.auth != Auth::Key {
            credential.key = None;
        }
    }

    let credential = &credentials[index];
    if credential.auth == Auth::Key && credential.key.is_none() {
        bail!("key authentication needs a key file - pass --key PATH");
    }
    store::save_credentials(&credentials)?;
    crate::journal::record("credential.edit", None, None, Some(name));
    println!("Credential '{name}' updated.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut credentials = store::load_credentials()?;
    if !credentials.iter().any(|c| c.name == name) {
        return Err(unknown(name));
    }
    // Servers pointing at it would be left with a dangling name, so say so
    // before the question rather than after the deletion.
    let servers = store::load_servers()?;
    let used_by: Vec<&str> = servers
        .iter()
        .filter(|s| s.credential.as_deref() == Some(name))
        .map(|s| s.name.as_str())
        .collect();
    // Targets name it too, and a target without a login cannot deploy at all -
    // unlike a server, which is still a routable stand.
    let targets = store::load_targets()?;
    let target_users: Vec<&str> = targets.iter().filter(|t| t.credential == name).map(|t| t.name.as_str()).collect();
    if !used_by.is_empty() {
        println!("Used by servers: {}. They will be left without a credential.", used_by.join(", "));
    }
    if !target_users.is_empty() {
        println!("Used by targets: {}. They will be removed with it.", target_users.join(", "));
    }
    let confirmed = pick::confirm_destructive(format!("Remove credential '{name}' and its stored secret?"), assume_yes)?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    if let Err(err) = secrets::delete(name) {
        eprintln!("warning: {err:#}");
    }
    credentials.retain(|c| c.name != name);
    store::save_credentials(&credentials)?;

    let mut servers = servers;
    let mut touched = Vec::new();
    for server in servers.iter_mut() {
        if server.credential.as_deref() == Some(name) {
            server.credential = None;
            touched.push(server.name.clone());
        }
    }
    if !touched.is_empty() {
        store::save_servers(&servers)?;
        println!("Cleared it from servers: {}.", touched.join(", "));
    }
    let mut targets = targets;
    let before = targets.len();
    targets.retain(|t| t.credential != name);
    if targets.len() != before {
        store::save_targets(&targets)?;
    }
    crate::journal::record("credential.remove", None, None, Some(name));
    println!("Credential '{name}' removed.");
    Ok(())
}

fn unknown(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no credential named '{name}' - see `turnout credential list`")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live run caught this: `--auth agent --key PATH` stored the path and
    /// `credential show` then printed a `Key:` line for a login that never
    /// opens a key file. The flag combination is a contradiction, and the auth
    /// kind is what settles it - the file is dropped, not kept as dead data.
    ///
    /// Asserted through `parse_auth` plus the rule it feeds, because the rule
    /// is the part that was wrong: the kind decides whether a key is kept.
    #[test]
    fn only_key_authentication_keeps_a_key_file() {
        let path = Some("~/.ssh/id_ed25519".to_string());
        // The kind that reads a file keeps it.
        assert_eq!(parse_auth(Some("key".into()), &path).unwrap(), Some(Auth::Key));
        // The kinds that do not read one must not be given one to store.
        for kind in ["password", "agent"] {
            let auth = parse_auth(Some(kind.into()), &path).unwrap().expect("an explicit kind");
            assert_ne!(auth, Auth::Key, "{kind} does not authenticate with a key file");
        }
    }

    /// The two wizard lists are index-matched: `AUTH_KINDS[i]` is what picking
    /// `AUTH_LABELS[i]` stores. Nothing in the type system holds them together,
    /// and a mismatch would silently save the wrong kind of login.
    #[test]
    fn every_offered_label_maps_to_the_kind_it_describes() {
        assert_eq!(AUTH_KINDS.len(), AUTH_LABELS.len());
        assert!(AUTH_LABELS[AUTH_KINDS.iter().position(|&a| a == Auth::Password).expect("password is offered")].contains("password"));
        assert!(AUTH_LABELS[AUTH_KINDS.iter().position(|&a| a == Auth::Key).expect("a key file is offered")].contains("key file"));
        assert!(AUTH_LABELS[AUTH_KINDS.iter().position(|&a| a == Auth::Agent).expect("the agent is offered")].contains("agent"));
    }

    /// Passing a key file is enough to mean key auth: making the user also type
    /// `--auth key` would be a second way to say the same thing, and forgetting
    /// it would store a key that never gets used.
    #[test]
    fn a_key_file_implies_key_authentication() {
        assert_eq!(parse_auth(None, &Some("~/.ssh/id_ed25519".into())).unwrap(), Some(Auth::Key));
        assert_eq!(parse_auth(None, &Some("  ".into())).unwrap(), None, "an empty key is a removal, not a mode");
        assert_eq!(parse_auth(None, &None).unwrap(), None);
        // An explicit flag always wins over the inference.
        assert_eq!(parse_auth(Some("password".into()), &Some("k".into())).unwrap(), Some(Auth::Password));
        // Since v0.13.0 the agent is a real kind, and naming it explicitly is
        // the only way to reach it: there is no key file to infer it from.
        assert_eq!(parse_auth(Some("agent".into()), &None).unwrap(), Some(Auth::Agent));
        // A key file next to --auth agent is a contradiction the flag wins:
        // the inference must not quietly turn an agent login into a file one.
        assert_eq!(parse_auth(Some("agent".into()), &Some("k".into())).unwrap(), Some(Auth::Agent));
        assert!(parse_auth(Some("smartcard".into()), &None).is_err());
    }
}
