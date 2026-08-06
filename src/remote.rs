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
