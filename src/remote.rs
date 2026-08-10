use std::io::Read;
use std::net::TcpStream;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::model::{App, DeployTarget, Server, Ssh};
use crate::{secrets, store};

pub struct Target {
    pub app: App,
    pub server: Server,
}

/// Resolve the app (argument or cwd) and the target server (flag or binding),
/// honoring the app's server allow-list.
pub fn resolve(app_name: Option<String>, server_name: Option<String>) -> Result<Target> {
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, app_name)?.clone();
    let server_name = match server_name {
        Some(name) => name,
        None => store::load_state()?
            .bindings
            .get(&app.name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no target: pass --server or bind one with `turnout use {} SERVER`", app.name))?,
    };
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    if !app.servers.is_empty() && !app.servers.contains(&server.name) {
        bail!("server '{server_name}' is not allowed for '{}' (allowed: {})", app.name, app.servers.join(", "));
    }
    Ok(Target { app, server })
}

pub fn require_ssh(server: &Server) -> Result<&Ssh> {
    server.ssh.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "server '{0}' has no SSH access - set it with `turnout server edit {0} --ssh USER@HOST[:PORT]`",
            server.name
        )
    })
}

pub fn require_deploy_target<'a>(server: &'a Server, app_name: &str) -> Result<&'a DeployTarget> {
    server.deploy.get(app_name).ok_or_else(|| {
        anyhow::anyhow!(
            "no deploy path for '{app_name}' on '{0}' - set it with `turnout server edit {0} --deploy-path {app_name}=DIR`",
            server.name
        )
    })
}

/// Configured key file first, then agent keys (ssh-agent / Pageant),
/// then the keyring password (kind `ssh`, falling back to `password`).
pub fn connect(ssh: &Ssh, server_name: &str) -> Result<Session> {
    let stream = TcpStream::connect((ssh.host.as_str(), ssh.port)).with_context(|| format!("cannot reach {}:{}", ssh.host, ssh.port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(stream);
    session.handshake().context("SSH handshake failed")?;

    if let Some(key) = &ssh.key {
        session
            .userauth_pubkey_file(&ssh.user, None, Path::new(key), None)
            .with_context(|| format!("key auth with {key} failed"))?;
        return Ok(session);
    }
    let _ = session.userauth_agent(&ssh.user);
    if !session.authenticated() {
        let password = secrets::get(server_name, "ssh")
            .or_else(|_| secrets::get(server_name, "password"))
            .map_err(|_| anyhow::anyhow!("agent auth failed and no password stored - save one with `turnout pass set {server_name} --kind ssh`"))?;
        session.userauth_password(&ssh.user, &password).context("SSH password authentication failed")?;
    }
    Ok(session)
}

/// Run a remote command and return its stdout; a non-zero exit becomes an error
/// carrying the remote stderr.
pub fn exec(session: &Session, command: &str) -> Result<String> {
    let mut channel = session.channel_session()?;
    channel.exec(command).with_context(|| format!("cannot run remote command '{command}'"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    let code = channel.exit_status()?;
    if code != 0 {
        bail!("remote command '{command}' exited with {code}: {}", stderr.trim());
    }
    Ok(output)
}

/// Single-quote a value for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Backups live next to the deploy directory: `{path}.backups/`.
pub fn backups_dir(deploy_path: &str) -> String {
    format!("{}.backups", deploy_path.trim_end_matches('/'))
}

/// Create a timestamped tar.gz of the deploy directory; prints the archive name.
pub fn backup_command(deploy_path: &str) -> String {
    let dir = shell_quote(deploy_path.trim_end_matches('/'));
    let backups = shell_quote(&backups_dir(deploy_path));
    format!("mkdir -p {backups} && ts=$(date +%Y%m%d-%H%M%S) && tar czf {backups}/\"$ts\".tar.gz -C {dir} . && echo \"$ts\".tar.gz")
}

/// Run the backup, explaining the one failure that surprises everyone.
///
/// Archives live *beside* the deploy directory, so creating them needs write
/// access to its parent - typically `/var/www`, owned by root. Uploading works
/// regardless, which makes a bare "permission denied" from `mkdir` look
/// arbitrary; the hint below is what actually unblocks it.
pub fn run_backup(session: &Session, deploy_path: &str) -> Result<String> {
    exec(session, &backup_command(deploy_path)).map_err(|err| {
        if mentions_permission_denial(&err) {
            anyhow::anyhow!(permission_hint(deploy_path))
        } else {
            err
        }
    })
}

/// Why the backup was refused and the one command that unblocks it.
fn permission_hint(deploy_path: &str) -> String {
    let backups = backups_dir(deploy_path);
    let parent = parent_dir(&backups);
    format!(
        "cannot write backups to {backups}: permission denied.\n\
         Backups are kept next to the deploy directory, so this needs write access to {parent} - \
         being able to upload into the deploy directory itself is not enough.\n\
         Create it once on the server with the right owner, e.g.\n  \
         sudo mkdir -p {backups} && sudo chown $USER {backups}"
    )
}

/// Whether a failed remote command was refused for lack of permission.
fn mentions_permission_denial(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_lowercase();
    text.contains("permission denied") || text.contains("cannot create directory")
}

/// The directory a path sits in; `/` when there is no parent to name.
fn parent_dir(path: &str) -> &str {
    match path.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/",
        Some((parent, _)) => parent,
    }
}

#[cfg(test)]
mod tests {
    use super::{backups_dir, mentions_permission_denial, parent_dir, shell_quote};

    #[test]
    fn backups_sit_beside_the_deploy_directory() {
        assert_eq!(backups_dir("/var/www/myapp"), "/var/www/myapp.backups");
        assert_eq!(backups_dir("/var/www/myapp/"), "/var/www/myapp.backups");
    }

    /// The parent is what the permission hint tells the user to fix, so it has
    /// to be right even for a directory sitting at the root.
    #[test]
    fn parent_of_a_backups_dir() {
        assert_eq!(parent_dir("/var/www/myapp.backups"), "/var/www");
        assert_eq!(parent_dir("/srv"), "/");
        assert_eq!(parent_dir("/"), "/");
    }

    #[test]
    fn recognizes_permission_failures_only() {
        let denied = anyhow::anyhow!("remote command 'mkdir -p /var/www/x.backups' exited with 1: mkdir: cannot create directory: Permission denied");
        assert!(mentions_permission_denial(&denied));

        let other = anyhow::anyhow!("remote command 'tar czf ...' exited with 2: tar: not found");
        assert!(!mentions_permission_denial(&other), "unrelated failures must keep their own message");
    }

    /// The message has one job: say why uploading works while backing up does
    /// not, and give the command that fixes it.
    #[test]
    fn permission_hint_names_the_parent_and_the_fix() {
        let hint = super::permission_hint("/var/www/myapp");
        assert!(hint.contains("/var/www/myapp.backups"), "{hint}");
        assert!(hint.contains("write access to /var/www"), "{hint}");
        assert!(hint.contains("sudo mkdir -p /var/www/myapp.backups"), "{hint}");
    }

    #[test]
    fn quotes_values_for_a_posix_shell() {
        assert_eq!(shell_quote("/var/www/my app"), "'/var/www/my app'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
