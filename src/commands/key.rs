//! `turnout key setup` - put a key on a server and switch the credential to it.
//!
//! Its own top-level command rather than a branch of `server` or `credential`,
//! for the reason ADR 0013 gave `target` one: the operation joins two entities
//! and belongs to neither. It reads a server (where to install) and writes a
//! credential (what it authenticates with), and a subcommand of either would be
//! quietly editing the other.
//!
//! The order of the steps is the whole design. Signing in with the password is
//! the *only* way to install the key in the first place, and the check
//! afterwards opens a second, separate session that offers nothing but the key:
//! a check riding on the already-authenticated connection would pass whether or
//! not the key works.

use anyhow::{Context, Result, bail};
use dialoguer::Input;

use crate::keysetup::{self, AuthorizedKeysFile};
use crate::model::{Auth, Credential, Server};
use crate::progress::Step;
use crate::shell::Dialect;
use crate::ssh::Session;
use crate::{pick, progress, remote, secrets, store};

/// Install the key of `credential` on `server`.
pub fn setup(server_name: Option<String>, credential_name: Option<String>, key_path: Option<String>) -> Result<()> {
    let servers = store::load_servers()?;
    let server_name = match server_name {
        Some(name) => name,
        None => pick::server(&servers, "Set up key access to")?,
    };
    let server = servers
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;

    // The server's own credential is the default, because "set up key access to
    // this stand" almost always means the account already configured for it.
    let credentials = store::load_credentials()?;
    let credential_name = match credential_name.or_else(|| server.credential.clone()) {
        Some(name) => name,
        None => pick::credential(&credentials, "Set up key access for")?,
    };
    let mut credential = credentials
        .iter()
        .find(|c| c.name == credential_name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no credential named '{credential_name}' - see `turnout credential list`"))?;

    progress::intro(format!("Key access for {}@{}", credential.user, server.ssh_host()));

    // 1. Can we get in at all? Asked before anything is created, because
    //    generating first is what a live run caught: with no password stored,
    //    the command failed *after* writing a key pair to disk, and the next
    //    run then refused to start because that key was in the way. Nothing is
    //    written until the way in is known to exist.
    check_first_sign_in(&credential)?;

    // 2. The key itself, still before the network: a key that cannot be
    //    produced is not worth a round trip.
    let key = obtain_key(&credential, key_path)?;

    // 3. Sign in the way that still works. This is the last time the password
    //    is needed, which is the point of the whole command.
    let step = Step::start(format!("Connecting to {}:{}", server.ssh_host(), server.port));
    let session = connect_for_setup(&server, &credential)?;
    step.done(format!("Connected to {}:{}", server.ssh_host(), server.port));

    // 4. Where the key goes. Both answers come from the server rather than
    //    from a guess: the shell it speaks, and the home of the account.
    let dialect = remote::dialect(&session, &server);
    keysetup::check_line(dialect, &key.public)?;
    let placed = locate(&session, dialect)?;
    if placed.administrators {
        progress::info("This account is an administrator: OpenSSH reads only C:\\ProgramData\\ssh\\administrators_authorized_keys for it.");
    }

    // 5. Install, unless it is already there.
    install(&session, dialect, &placed, &key.public)?;

    // 6. Prove it. A fresh session, key only.
    let step = Step::start("Checking that the key signs in");
    match Session::open_with_key(&server.ssh_host(), server.port, &credential.user, &key.path, key.passphrase.as_deref()) {
        Ok(_) => step.done("The key signs in"),
        Err(err) => {
            step.clear();
            // The key is installed either way; what failed is the sign-in. The
            // credential is deliberately *not* switched, so the working
            // password stays in place.
            return Err(err).context(key_sign_in_failed(dialect, &placed));
        }
    }

    // 7. Only now is the credential moved over.
    credential.auth = Auth::Key;
    credential.key = Some(key.path.clone());
    let mut credentials = store::load_credentials()?;
    let index = credentials
        .iter()
        .position(|c| c.name == credential.name)
        .ok_or_else(|| anyhow::anyhow!("credential '{}' vanished while it was being set up", credential.name))?;
    credentials[index] = credential.clone();
    store::save_credentials(&credentials)?;

    progress::outro(format!("'{}' now signs in with {}", credential.name, key.path));
    if secrets::get(&credential.name).is_ok() && key.passphrase.is_none() {
        // The stored password is now unused but still valid - and still the way
        // back in if the key is ever lost. Removing it is the user's call.
        progress::info(&format!(
            "The stored password is no longer used; remove it with `turnout pass remove {}` once you are sure.",
            credential.name
        ));
    }
    Ok(())
}

/// The key this setup will install: where it lives and how it is unlocked.
struct KeyInHand {
    path: String,
    public: String,
    passphrase: Option<String>,
}

/// Find the key to install, generating one when there is none.
///
/// A credential that already names a key file uses it - re-running the command
/// to authorize an existing key on one more server is a normal thing to want,
/// and generating a second key for it would be surprising.
fn obtain_key(credential: &Credential, key_path: Option<String>) -> Result<KeyInHand> {
    let existing = key_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .or_else(|| credential.key.clone());

    if let Some(path) = existing {
        if !std::path::Path::new(&path).exists() {
            bail!("the key file {path} does not exist - point --key at one, or leave it out to generate a key");
        }
        // A passphrase-protected key has its passphrase under the credential's
        // name; an unprotected one has nothing stored, which is not an error.
        let passphrase = secrets::get(&credential.name).ok().filter(|_| credential.auth == Auth::Key);
        let public = keysetup::public_line(&path, passphrase.as_deref(), &comment())?;
        progress::info(&format!("Using the existing key {path}"));
        return Ok(KeyInHand { path, public, passphrase });
    }

    let default = keysetup::default_key_path(&credential.name)?;
    let path = if pick::interactive() {
        let answer: String = Input::new()
            .with_prompt("Generate a key at")
            .default(default.display().to_string())
            .interact_text()?;
        answer.trim().to_string()
    } else {
        default.display().to_string()
    };

    let generated = keysetup::generate(&comment())?;
    keysetup::write_private_key(std::path::Path::new(&path), &generated.private)?;
    // The `.pub` sibling is what every other tool reads; writing it means a key
    // turnout generated is an ordinary key, not a turnout-only one.
    let pub_path = format!("{path}.pub");
    std::fs::write(&pub_path, format!("{}\n", generated.public)).with_context(|| format!("cannot write {pub_path}"))?;
    progress::info(&format!("Generated an ed25519 key at {path}"));
    Ok(KeyInHand {
        path,
        public: generated.public,
        passphrase: None,
    })
}

/// The comment written into the public key: who made it, on which machine.
fn comment() -> String {
    let host = hostname().unwrap_or_else(|| "unknown".to_string());
    format!("turnout@{host}")
}

/// This machine's name, for the key comment only.
fn hostname() -> Option<String> {
    // Not worth a dependency: both platforms hand it to the environment, and a
    // missing value costs a comment, not a key.
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
}

/// Whether there is anything to sign in with at all.
///
/// Checked before a key is generated, not when the connection is opened. A live
/// run made the difference plain: with no password stored, the command wrote a
/// key pair to disk and *then* failed, and the next attempt refused to start
/// because that key was now in the way. The user had to go and delete a file
/// they never asked for.
fn check_first_sign_in(credential: &Credential) -> Result<()> {
    // An agent credential already signs in by key - through the agent - so
    // there is nothing for this command to set up, and running it would trade
    // a working login for a key file on disk. That is a downgrade, not a
    // setup, so it is refused rather than done quietly.
    if credential.auth == Auth::Agent {
        bail!(
            "credential '{0}' already signs in with a key from the SSH agent - to authorize that key on this server, add it with `ssh-copy-id`, or point '{0}' at a key file first",
            credential.name
        );
    }
    if credential.auth == Auth::Password && secrets::get(&credential.name).is_err() {
        bail!(
            "no password stored for '{0}', and the first sign-in needs one - save it with `turnout pass set {0}`",
            credential.name
        );
    }
    Ok(())
}

/// Open the session the installation rides on.
///
/// Password auth, because a key credential with no key on the server yet cannot
/// sign in - which is the situation this command exists to end. A credential
/// already set to `key` is honoured as-is: it is being authorized on one more
/// server, and its key already works elsewhere.
fn connect_for_setup(server: &Server, credential: &Credential) -> Result<Session> {
    remote::connect(server, credential).with_context(|| {
        format!(
            "cannot sign in as '{}' to install the key - the first connection still uses the password",
            credential.user
        )
    })
}

/// Ask the server where its authorized-keys file is.
fn locate(session: &Session, dialect: Dialect) -> Result<AuthorizedKeysFile> {
    let home = remote::exec(session, dialect.home_dir())
        .context("cannot read the remote home directory")?
        .trim()
        .to_string();
    if home.is_empty() {
        bail!("the server did not report a home directory for this account");
    }
    let administrator = match dialect {
        Dialect::Posix => false,
        Dialect::Windows => keysetup::reads_as_administrator(remote::exec(session, keysetup::WINDOWS_ADMIN_PROBE)),
    };
    Ok(AuthorizedKeysFile::locate(dialect, &home, administrator))
}

/// Append the public key to the server's authorized-keys file, unless it is
/// already there, and give the file the permissions sshd requires.
fn install(session: &Session, dialect: Dialect, placed: &AuthorizedKeysFile, public: &str) -> Result<()> {
    let step = Step::start(format!("Installing the key in {}", placed.file));
    let existing = remote::exec(session, &dialect.read_file(&placed.file)).unwrap_or_default();
    if keysetup::already_authorized(&existing, public) {
        step.done(format!("The key is already authorized in {}", placed.file));
        return Ok(());
    }

    remote::exec(session, &dialect.mkdir_p(&placed.dir)).with_context(|| format!("cannot create {} on the server", placed.dir))?;
    remote::exec(session, &dialect.append_line(&placed.file, public)).with_context(|| format!("cannot write the key to {}", placed.file))?;
    // sshd ignores a key file that others can write, and says so only in its
    // own log - so the permissions are part of installing, not an afterthought.
    remote::exec(session, &dialect.restrict_to_owner(&placed.dir, &placed.file))
        .with_context(|| format!("the key was written to {} but its permissions could not be set", placed.file))?;
    step.done(format!("Key installed in {}", placed.file));
    Ok(())
}

/// What to say when the key was installed but does not sign in.
///
/// Every line here is a cause that actually produces this exact symptom, and
/// each one names where to look - the point is that the user does not have to
/// go and read someone's blog post about it.
fn key_sign_in_failed(dialect: Dialect, placed: &AuthorizedKeysFile) -> String {
    let mut reasons = vec![format!("the key was installed in {} but the server still refuses it", placed.file)];
    reasons.push("likely causes:".to_string());
    reasons.push("  - sshd has `PubkeyAuthentication no` (check /etc/ssh/sshd_config and restart sshd)".to_string());
    match dialect {
        Dialect::Posix => {
            reasons.push("  - the home directory or ~/.ssh is group-writable, which sshd refuses (chmod go-w ~ ~/.ssh)".to_string());
            reasons.push("  - sshd's AuthorizedKeysFile points somewhere else than ~/.ssh/authorized_keys".to_string());
        }
        Dialect::Windows if placed.administrators => {
            reasons.push("  - the file's ACL still grants more than the owner and SYSTEM, which Windows sshd refuses".to_string());
            reasons.push("  - the `Match Group administrators` block in %ProgramData%\\ssh\\sshd_config was removed or repointed".to_string());
        }
        Dialect::Windows => {
            reasons.push("  - this account is in the administrators group after all, in which case only administrators_authorized_keys is read".to_string());
        }
    }
    reasons.push("the credential was left on its password, so access is unchanged".to_string());
    reasons.join("\n")
}

/// `turnout key check` - does key sign-in work for this pair?
pub fn check(server_name: Option<String>, credential_name: Option<String>) -> Result<()> {
    let servers = store::load_servers()?;
    let server_name = match server_name {
        Some(name) => name,
        None => pick::server(&servers, "Check key access to")?,
    };
    let server = servers
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    let credentials = store::load_credentials()?;
    let credential_name = match credential_name.or_else(|| server.credential.clone()) {
        Some(name) => name,
        None => pick::credential(&credentials, "Check key access for")?,
    };
    let credential = credentials
        .into_iter()
        .find(|c| c.name == credential_name)
        .ok_or_else(|| anyhow::anyhow!("no credential named '{credential_name}' - see `turnout credential list`"))?;

    // Named by account and machine rather than by the two entity names: a
    // credential and a server often share a name, and "'pi-host' signs in to
    // 'pi-host'" reads like a bug in the sentence rather than a result.
    let who = format!("{}@{}", credential.user, server.ssh_host());

    // An agent credential has no key file to name, and checking it means
    // asking the agent to sign - the same route the daily commands take. It
    // goes through the credential rather than around it, which for `agent` is
    // exactly what proves the thing under test.
    if credential.auth == Auth::Agent {
        let step = Step::start(format!("Signing in as {who} with a key from the SSH agent"));
        return match remote::connect(&server, &credential) {
            Ok(_) => {
                step.done(format!("{who} signs in with an agent key ({})", credential.name));
                Ok(())
            }
            Err(err) => {
                step.clear();
                Err(err).with_context(|| format!("{who} cannot sign in through the SSH agent (credential '{}')", credential.name))
            }
        };
    }

    let Some(path) = credential.key.clone() else {
        bail!(
            "credential '{0}' has no key file - set one up with `turnout key setup {1} --credential {0}`",
            credential.name,
            server.name
        );
    };
    let passphrase = secrets::get(&credential.name).ok();
    let step = Step::start(format!("Signing in as {who} with {path}"));
    match Session::open_with_key(&server.ssh_host(), server.port, &credential.user, &path, passphrase.as_deref()) {
        Ok(_) => {
            step.done(format!("{who} signs in with its key ({}, key {path})", credential.name));
            Ok(())
        }
        Err(err) => {
            step.clear();
            Err(err).with_context(|| format!("{who} cannot sign in with {path} (credential '{}')", credential.name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A credential that already has its key needs no password: it is being
    /// authorized on one more server, and the key it already has is what signs
    /// in there.
    #[test]
    fn a_key_credential_needs_no_stored_password() {
        let credential = Credential {
            name: "already-on-a-key".into(),
            user: "deploy".into(),
            auth: Auth::Key,
            key: Some("/home/me/.ssh/id_ed25519".into()),
        };
        assert!(check_first_sign_in(&credential).is_ok());
    }

    /// The failure message has to name the file it wrote to and leave the user
    /// pointed at a cause, not at a search engine.
    #[test]
    fn a_refused_key_names_the_file_and_the_usual_causes() {
        let placed = AuthorizedKeysFile::locate(Dialect::Posix, "/home/pi", false);
        let message = key_sign_in_failed(Dialect::Posix, &placed);
        assert!(message.contains("/home/pi/.ssh/authorized_keys"), "{message}");
        assert!(message.contains("PubkeyAuthentication no"), "{message}");
        assert!(message.contains("group-writable"), "{message}");
        // The most important line: nothing was taken away.
        assert!(message.contains("access is unchanged"), "{message}");
    }

    /// On a Windows administrator the advice is about the ACL and the Match
    /// block, because the POSIX advice would send the user to files that do
    /// not exist there.
    #[test]
    fn windows_advice_is_about_windows() {
        let placed = AuthorizedKeysFile::locate(Dialect::Windows, "C:\\Users\\deploy", true);
        let message = key_sign_in_failed(Dialect::Windows, &placed);
        assert!(message.contains("administrators_authorized_keys"), "{message}");
        assert!(message.contains("ACL"), "{message}");
        assert!(!message.contains("chmod"), "chmod means nothing on Windows: {message}");
    }

    /// A non-administrator Windows account that is refused is most often an
    /// administrator after all - that is the one cause worth naming there.
    #[test]
    fn an_ordinary_windows_account_is_told_about_the_group() {
        let placed = AuthorizedKeysFile::locate(Dialect::Windows, "C:\\Users\\deploy", false);
        let message = key_sign_in_failed(Dialect::Windows, &placed);
        assert!(message.contains("administrators group"), "{message}");
    }

    /// The comment is what a human reads in `authorized_keys` a year later; it
    /// must say the key came from turnout and from which machine.
    #[test]
    fn the_key_comment_names_turnout_and_the_machine() {
        let comment = comment();
        assert!(comment.starts_with("turnout@"), "{comment}");
        assert!(!comment.ends_with('@'), "an empty host leaves a dangling at-sign: {comment}");
        // It goes into a single-line file, and onto a cmd.exe command line.
        assert_eq!(comment.lines().count(), 1);
        assert!(!comment.contains(' '), "{comment}");
    }
}
