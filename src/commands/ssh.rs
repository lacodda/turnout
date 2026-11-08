use std::process::Command;

use anyhow::{Context, Result};

use crate::model::{Auth, Credential};
use crate::remote::{self, Host};
use crate::shell::Dialect;

/// Names the SSH client to run instead of the `ssh` on PATH.
pub const ENV_CLIENT: &str = "TURNOUT_SSH";

/// An interactive session on a server, with nothing to remember: host, port,
/// user and key come from the catalogs, and a target's session opens in its
/// deploy directory.
///
/// The session itself is the system `ssh` client's: a terminal is its job -
/// window size, signals, the raw mode dance - and every machine that deploys
/// anything has one. turnout composes the invocation and gets out of the way.
pub fn run(name: Option<String>, credential: Option<String>) -> Result<()> {
    let host = remote::resolve_host(name, credential)?;
    let client = std::env::var(ENV_CLIENT).unwrap_or_else(|_| "ssh".to_string());

    let mut args: Vec<String> = Vec::new();
    // One file of host keys for both: `ssh` reads ~/.ssh/known_hosts by itself,
    // and only a file named elsewhere has to be handed to it.
    if let Some(file) = std::env::var_os(crate::known_hosts::FILE_ENV) {
        args.push("-o".into());
        let file = std::path::Path::new(&file).display().to_string();
        // The option takes a list split on blanks, so a path with one is quoted.
        let file = if file.contains(char::is_whitespace) { format!("\"{file}\"") } else { file };
        args.push(format!("UserKnownHostsFile={file}"));
    }
    args.extend(["-p".into(), host.server.port.to_string()]);
    if host.credential.auth == Auth::Key
        && let Some(key) = &host.credential.key
    {
        args.push("-i".into());
        args.push(key.clone());
    }
    args.push(format!("{}@{}", host.credential.user, host.server.ssh_host()));
    if let Some(dir) = &host.dir {
        // Landing in the deploy directory needs the server's shell, which the
        // catalog knows after the first remote command; asking costs one
        // extra login, and a login that fails only costs the directory.
        match landing_shell(&host) {
            Some(dialect) => {
                remote::check_quotable(dialect, &[dir])?;
                args.push("-t".into());
                args.push(landing_command(dialect, dir));
            }
            None => eprintln!(
                "note: could not tell which shell '{}' runs - the session opens in the home directory",
                host.server.name
            ),
        }
    }

    // What the prompt will ask for is one paste away - and only that. A
    // stored secret is not always what it asks for: after `key setup` the
    // secret under a key credential is the old login password, kept as the
    // way back in. Up to v0.26 every login put it on the clipboard as a
    // "passphrase" the key never asked for.
    if let Some(what) = asks_for(&host.credential)
        && let Ok(secret) = crate::secrets::get(&host.credential.name)
    {
        match crate::clipboard::set(&secret) {
            Ok(()) => eprintln!("{what} for '{}' copied to the clipboard - paste it at the prompt.", host.credential.name),
            Err(_) => eprintln!("{what} for '{}' is in the keyring: `turnout pass copy {0}`.", host.credential.name),
        }
    }

    eprintln!("[{}] {client} {}", host.label(), args.join(" "));
    crate::journal::record("ssh", Some(&host.server.name), None, None);
    let mut child = Command::new(&client)
        .args(&args)
        .spawn()
        .with_context(|| format!("cannot run '{client}' - is the OpenSSH client installed? ({ENV_CLIENT} names another one)"))?;
    // While the session runs, Ctrl+C belongs to it.
    crate::term::child_begin();
    let status = child.wait();
    crate::term::child_end();
    let status = status.with_context(|| format!("cannot wait for '{client}'"))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// What the `ssh` prompt will ask for, if anything: a password credential's
/// password, a key's passphrase when the key is encrypted, and nothing for
/// an agent, which signs without asking.
fn asks_for(credential: &Credential) -> Option<&'static str> {
    match credential.auth {
        Auth::Password => Some("Password"),
        Auth::Key => credential.key.as_deref().filter(|key| key_is_encrypted(key)).map(|_| "Passphrase"),
        Auth::Agent => None,
    }
}

/// Whether the key file at `path` is locked by a passphrase.
///
/// Read without one: the OpenSSH format says so outright, and the PEM forms
/// (`BEGIN ENCRYPTED PRIVATE KEY`, `Proc-Type: 4,ENCRYPTED`) carry the word in
/// the file. A file that cannot be read at all is taken as not encrypted:
/// putting a secret on the clipboard is what has to be sure.
fn key_is_encrypted(path: &str) -> bool {
    match russh::keys::load_secret_key(path, None) {
        Ok(_) => false,
        Err(russh::keys::Error::KeyIsEncrypted) => true,
        Err(_) => std::fs::read_to_string(path).is_ok_and(|text| text.contains("ENCRYPTED")),
    }
}

/// The dialect the session will land in: known from the catalog, or learned
/// with one extra login; `None` when the server cannot be asked.
fn landing_shell(host: &Host) -> Option<Dialect> {
    if let Some(known) = host.server.shell {
        return Some(known);
    }
    let session = remote::connect(&host.server, &host.credential).ok()?;
    remote::dialect(&session, &host.server).ok()
}

/// The command that turns a login into a shell in `dir`.
///
/// `exec $SHELL -l` gives the account's own login shell, not `sh`; on
/// Windows sshd runs the command under `cmd.exe`, and a bare `cmd` at the
/// end of it stays open for input.
pub fn landing_command(dialect: Dialect, dir: &str) -> String {
    match dialect {
        Dialect::Posix => format!("cd {} && exec $SHELL -l", dialect.quote(dir)),
        Dialect::Windows => format!("cd /d {} && cmd", dialect.quote(dir)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_landing_command_speaks_the_servers_shell() {
        assert_eq!(landing_command(Dialect::Posix, "/srv/app"), "cd '/srv/app' && exec $SHELL -l");
        assert_eq!(landing_command(Dialect::Windows, r"C:\site"), r#"cd /d "C:\site" && cmd"#);
    }

    fn credential(auth: Auth, key: Option<&std::path::Path>) -> Credential {
        Credential {
            name: "deploy".into(),
            user: "deploy".into(),
            auth,
            key: key.map(|path| path.display().to_string()),
        }
    }

    /// Only what the prompt asks for goes on the clipboard: a key's
    /// passphrase when it has one, never the password a key credential keeps
    /// after `key setup`, nothing for the agent.
    #[test]
    fn only_what_the_prompt_asks_for_is_copied() {
        use russh::keys::ssh_key::LineEnding;
        use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
        let scratch = tempfile::tempdir().unwrap();
        let key = russh::keys::PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[9; 32])), "test").unwrap();
        let open = scratch.path().join("id_open");
        std::fs::write(&open, key.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();
        let locked = scratch.path().join("id_locked");
        let encrypted = key.encrypt(&mut rand::rng(), "passphrase").unwrap();
        std::fs::write(&locked, encrypted.to_openssh(LineEnding::LF).unwrap().as_bytes()).unwrap();

        assert_eq!(asks_for(&credential(Auth::Password, None)), Some("Password"));
        assert_eq!(asks_for(&credential(Auth::Key, Some(&locked))), Some("Passphrase"));
        assert_eq!(asks_for(&credential(Auth::Key, Some(&open))), None, "a key without a passphrase asks nothing");
        assert_eq!(asks_for(&credential(Auth::Key, Some(&scratch.path().join("missing")))), None);
        assert_eq!(asks_for(&credential(Auth::Agent, None)), None);
    }

    /// A directory that cannot be quoted for the shell is refused before it
    /// reaches the command line, not sent as something else.
    #[test]
    fn an_unquotable_directory_is_refused() {
        assert!(remote::check_quotable(Dialect::Windows, &[r#"C:\odd"dir"#]).is_err());
        assert!(remote::check_quotable(Dialect::Posix, &["/srv/it's"]).is_ok());
    }
}
