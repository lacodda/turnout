use std::process::Command;

use anyhow::{Context, Result};

use crate::model::Auth;
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

    let mut args: Vec<String> = vec!["-p".into(), host.server.port.to_string()];
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

    // What the prompt will ask for is one paste away. A key that needs no
    // passphrase has nothing stored, and asking the keyring is not an error.
    if let Ok(secret) = crate::secrets::get(&host.credential.name) {
        let what = match host.credential.auth {
            Auth::Password => "Password",
            Auth::Key => "Passphrase",
            Auth::Agent => "Secret",
        };
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(secret)) {
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

/// The dialect the session will land in: known from the catalog, or learned
/// with one extra login; `None` when the server cannot be asked.
fn landing_shell(host: &Host) -> Option<Dialect> {
    if let Some(known) = host.server.shell {
        return Some(known);
    }
    let session = remote::connect(&host.server, &host.credential).ok()?;
    Some(remote::dialect(&session, &host.server))
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

    /// A directory that cannot be quoted for the shell is refused before it
    /// reaches the command line, not sent as something else.
    #[test]
    fn an_unquotable_directory_is_refused() {
        assert!(remote::check_quotable(Dialect::Windows, &[r#"C:\odd"dir"#]).is_err());
        assert!(remote::check_quotable(Dialect::Posix, &["/srv/it's"]).is_ok());
    }
}
