//! `turnout key rotate` - replace a credential's key on every server it
//! signs in to.
//!
//! A key is rotated because the laptop it was on is gone, because someone
//! who had it left, or because a year has passed. By hand that is one new
//! key, and then every server in turn: sign in, add a line, check it, remove
//! the old one - with the catalog pointing at whichever key the servers done
//! so far no longer take. Half-way through, some stands admit only the old
//! key and some only the new.
//!
//! The order is what keeps access whole at every point (ADR 0036):
//!
//! 1. the new key is authorized on *every* server, signing in with the old;
//! 2. the new key signs in to *every* server, by itself, in fresh sessions;
//! 3. only then is the credential switched to it;
//! 4. and only then is the old key taken off the servers - last.
//!
//! Anything that fails before step 3 is undone: the new key is withdrawn
//! from the servers that got it, its file is removed, and the credential
//! never moved. After step 3 nothing can lock the credential out: a server
//! that keeps the old key's line is named, and that is all it is - a line
//! to remove.

use anyhow::{Context, Result, bail};

use crate::commands::key::{authorize, comment, deauthorize, locate, write_generated};
use crate::keysetup::{self, AuthorizedKeysFile};
use crate::model::{Auth, Credential, Server};
use crate::progress::{self, Step};
use crate::shell::Dialect;
use crate::ssh::Session;
use crate::{pick, remote, secrets, store};

/// One server of the rotation, signed in to with the old key.
struct Stand {
    server: Server,
    dialect: Dialect,
    placed: AuthorizedKeysFile,
    session: Session,
}

impl Stand {
    fn who(&self, credential: &Credential) -> String {
        format!("{}@{}", credential.user, self.server.ssh_host())
    }
}

pub fn run(credential_name: Option<String>, assume_yes: bool) -> Result<()> {
    let credentials = store::load_credentials()?;
    let credential_name = match credential_name {
        Some(name) => name,
        None => pick::credential(&credentials, "Rotate the key of")?,
    };
    let credential = credentials
        .iter()
        .find(|c| c.name == credential_name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no credential named '{credential_name}' - see `turnout credential list`"))?;
    let old = old_key(&credential)?;
    let servers = servers_of(&credential.name)?;
    if servers.is_empty() {
        bail!(
            "no server signs in with '{0}' - nothing to rotate it on. A server takes a credential with `turnout server edit SERVER --credential {0}`, a target with `turnout target edit TARGET --credential {0}`",
            credential.name
        );
    }
    let names: Vec<&str> = servers.iter().map(|server| server.name.as_str()).collect();
    let confirmed = pick::confirm_destructive(
        format!(
            "Replace the key of '{}' on {}? Whatever else signs in there with {} loses access.",
            credential.name,
            names.join(", "),
            old.path
        ),
        assume_yes,
    )?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }

    progress::intro(format!("Rotating the key of '{}' on {}", credential.name, names.join(", ")));
    let new_path = free_key_path(&credential.name)?;
    let new = keysetup::generate(&comment())?;
    keysetup::check_line(&new.public)?;

    // 1. The new key onto every server, through the old one.
    let mut stands: Vec<Stand> = Vec::new();
    for server in servers {
        match admit(&server, &credential, &new.public) {
            Ok(stand) => stands.push(stand),
            Err(err) => {
                withdraw(&stands, &credential, &new.public);
                return Err(err).with_context(|| {
                    format!(
                        "'{}' could not be given the new key, so nothing was rotated: the credential and every server are on {} as before",
                        server.name, old.path
                    )
                });
            }
        }
    }

    // 2. The new key signs in everywhere, by itself. It has to be a file for
    //    that; one a failed check leaves behind is removed again.
    write_generated(&new_path, &new.private, &new.public)?;
    let mut fresh: Vec<Session> = Vec::new();
    for stand in &stands {
        let who = stand.who(&credential);
        let step = Step::start(format!("Checking that the new key signs in as {who}"));
        match Session::open_with_key(&stand.server, &credential.user, &new_path, None) {
            Ok(session) => {
                step.done(format!("The new key signs in as {who}"));
                fresh.push(session);
            }
            Err(err) => {
                step.clear();
                withdraw(&stands, &credential, &new.public);
                discard(&new_path);
                return Err(err).with_context(|| {
                    format!(
                        "'{}' was given the new key and still refuses it, so nothing was rotated: the credential and every server are on {} as before",
                        stand.server.name, old.path
                    )
                });
            }
        }
    }

    // 3. The credential moves over. From here on it signs in everywhere.
    let mut credentials = store::load_credentials()?;
    let entry = credentials
        .iter_mut()
        .find(|c| c.name == credential.name)
        .ok_or_else(|| anyhow::anyhow!("credential '{}' vanished while its key was being rotated", credential.name))?;
    entry.auth = Auth::Key;
    entry.key = Some(new_path.clone());
    store::save_credentials(&credentials)?;
    if old.passphrase.is_some() {
        // What is stored under the credential's name was the old key's
        // passphrase; the new key has none, and a passphrase offered to an
        // open key is at best ignored.
        secrets::delete(&credential.name)?;
        progress::info("The stored passphrase was the old key's and is removed: the new key has none.");
    }
    crate::journal::record("key.rotate", None, None, Some(&credential.name));

    // 4. The old key comes off, last - through the new one.
    let mut still_admitted = Vec::new();
    for (stand, session) in stands.iter().zip(&fresh) {
        let who = stand.who(&credential);
        let step = Step::start(format!("Removing the old key from {}", stand.placed.file));
        match deauthorize(session, stand.dialect, &stand.placed, &old.public) {
            Ok(true) => step.done(format!("The old key no longer signs in as {who}")),
            Ok(false) => step.done(format!("The old key was not among the keys of {who}")),
            Err(err) => {
                step.clear();
                progress::warn(&format!(
                    "the old key is still authorized as {who}: {err:#} - remove its line from {} there",
                    stand.placed.file
                ));
                still_admitted.push(stand.server.name.clone());
            }
        }
    }

    progress::outro(format!("'{}' now signs in with {new_path}", credential.name));
    progress::info(&format!(
        "{} is the old key: no server here takes it any more - delete the file once nothing else uses it.",
        old.path
    ));
    if !still_admitted.is_empty() {
        bail!(
            "the key was rotated, and the old one is still authorized on {} - see above",
            still_admitted.join(", ")
        );
    }
    Ok(())
}

/// The key being replaced.
struct OldKey {
    path: String,
    public: String,
    /// Its passphrase, when it has one: what the keyring holds under the
    /// credential's name.
    passphrase: Option<String>,
}

/// The credential's key as it is now - the one thing that still signs in.
fn old_key(credential: &Credential) -> Result<OldKey> {
    let path = match (credential.auth, &credential.key) {
        (Auth::Key, Some(path)) => path.clone(),
        (Auth::Agent, _) => bail!(
            "credential '{}' signs in with a key the SSH agent holds - turnout never sees it, and cannot replace it. Rotate it where it lives, or point the credential at a key file first",
            credential.name
        ),
        _ => bail!(
            "credential '{0}' has no key to rotate - give it one with `turnout key setup SERVER --credential {0}`",
            credential.name
        ),
    };
    if !std::path::Path::new(&path).exists() {
        bail!(
            "the key file {path} of '{}' does not exist - the old key is what signs in to add the new one",
            credential.name
        );
    }
    // A key that opens bare has no passphrase, whatever is stored: after
    // `key setup` the keyring may still hold the password the key replaced.
    let passphrase = match russh::keys::load_secret_key(&path, None) {
        Ok(_) => None,
        Err(_) => Some(secrets::get(&credential.name).with_context(|| format!("the key {path} is locked"))?),
    };
    let public = keysetup::public_line(&path, passphrase.as_deref(), &comment())?;
    Ok(OldKey { path, public, passphrase })
}

/// Every server the credential signs in to: the ones that name it as their
/// own, and the ones a target reaches with it - in name order, each once.
fn servers_of(credential: &str) -> Result<Vec<Server>> {
    let targets = store::load_targets()?;
    let mut servers: Vec<Server> = store::load_servers()?
        .into_iter()
        .filter(|server| {
            server.credential.as_deref() == Some(credential) || targets.iter().any(|target| target.credential == credential && target.server == server.name)
        })
        .collect();
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(servers)
}

/// Where the new key is written: beside the others, under a name nothing
/// has. A key file is never overwritten - the old one least of all.
fn free_key_path(credential: &str) -> Result<String> {
    let first = keysetup::default_key_path(credential)?;
    let taken = |path: &std::path::Path| path.exists() || std::path::Path::new(&format!("{}.pub", path.display())).exists();
    if !taken(&first) {
        return Ok(first.display().to_string());
    }
    let free = (2..)
        .map(|n| first.with_file_name(format!("id_ed25519_{credential}_{n}")))
        .find(|path| !taken(path))
        .expect("an unused suffix exists");
    Ok(free.display().to_string())
}

/// Sign in to `server` with the credential as it is and authorize `public`.
fn admit(server: &Server, credential: &Credential, public: &str) -> Result<Stand> {
    let who = format!("{}@{}", credential.user, server.ssh_host());
    let step = Step::start(format!("Connecting to {}:{} with the old key", server.ssh_host(), server.port));
    match sign_in_and_authorize(server, credential, public, &step) {
        Ok(stand) => {
            step.done(format!("The new key is authorized as {who} ({})", stand.placed.file));
            Ok(stand)
        }
        Err(err) => {
            step.clear();
            Err(err)
        }
    }
}

fn sign_in_and_authorize(server: &Server, credential: &Credential, public: &str, step: &Step) -> Result<Stand> {
    let session = remote::connect(server, credential)?;
    let dialect = remote::dialect(&session, server)?;
    let placed = locate(&session, dialect)?;
    step.update(format!("Authorizing the new key in {}", placed.file));
    authorize(&session, dialect, &placed, public)?;
    Ok(Stand {
        server: server.clone(),
        dialect,
        placed,
        session,
    })
}

/// Take the new key back off the servers that got it: a rotation that stops
/// leaves no key behind that nothing uses. Best-effort - the old key still
/// signs in everywhere, and a line that stays is named.
fn withdraw(stands: &[Stand], credential: &Credential, public: &str) {
    for stand in stands {
        if let Err(err) = deauthorize(&stand.session, stand.dialect, &stand.placed, public) {
            progress::warn(&format!(
                "the new key stays authorized as {}: {err:#} - its line in {} is the one ending in {}",
                stand.who(credential),
                stand.placed.file,
                comment()
            ));
        }
    }
}

/// Remove the key pair a rotation wrote and could not use.
fn discard(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{path}.pub"));
}
