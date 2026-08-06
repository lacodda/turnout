use std::io::Read;
use std::net::TcpStream;
use std::path::Path;

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::model::Ssh;
use crate::{secrets, store};

pub fn run(app_name: Option<String>, server_name: Option<String>, no_build: bool, clear: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let app = super::exec::resolve(&apps, app_name)?;
    let servers = store::load_servers()?;

    let server_name = match server_name {
        Some(name) => name,
        None => store::load_state()?
            .bindings
            .get(&app.name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no target: pass --server or bind one with `turnout use {} SERVER`", app.name))?,
    };
    let server = servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    if !app.servers.is_empty() && !app.servers.contains(&server_name) {
        bail!("server '{server_name}' is not allowed for '{}' (allowed: {})", app.name, app.servers.join(", "));
    }
    let Some(ssh) = &server.ssh else {
        bail!("server '{server_name}' has no SSH access - set it with `turnout server edit {server_name} --ssh USER@HOST[:PORT]`");
    };
    let Some(target) = server.deploy.get(&app.name) else {
        bail!(
            "no deploy path for '{}' on '{server_name}' - set it with `turnout server edit {server_name} --deploy-path {}=DIR`",
            app.name,
            app.name
        );
    };
    let Some(dist) = &app.dist_dir else {
        bail!("app '{0}' has no artifact directory - set it with `turnout app edit {0} --dist DIR`", app.name);
    };

    if !no_build {
        if let Some(build) = app.commands.get("build") {
            eprintln!("[{}] {build}", app.name);
            let status = crate::utils::run_in_dir(build, Path::new(&app.path))?;
            if !status.success() {
                bail!("build failed with {status} - nothing uploaded");
            }
        }
    }
    let local = Path::new(&app.path).join(dist);
    if !local.is_dir() {
        bail!("artifact directory {} does not exist - did the build produce it?", local.display());
    }

    println!("Connecting to {}@{}:{} ...", ssh.user, ssh.host, ssh.port);
    let session = connect(ssh, &server_name)?;

    if clear {
        println!("Clearing {} ...", target.path);
        let quoted = shell_quote(&target.path);
        remote_exec(&session, &format!("find {quoted} -mindepth 1 -maxdepth 1 -exec rm -rf {{}} +"))?;
    }

    let (files, bytes) = upload(&session, &local, &target.path)?;
    println!("Uploaded {files} files ({} KB) to {}:{}", bytes / 1024, server_name, target.path);

    if let Some(restart) = &target.restart {
        println!("Running: {restart}");
        remote_exec(&session, restart)?;
    }
    println!("Deploy of '{}' to '{server_name}' finished.", app.name);
    Ok(())
}

/// Configured key file first, then agent keys (ssh-agent / Pageant),
/// then the keyring password (kind `ssh`, falling back to `password`).
fn connect(ssh: &Ssh, server_name: &str) -> Result<Session> {
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

fn remote_exec(session: &Session, command: &str) -> Result<()> {
    let mut channel = session.channel_session()?;
    channel.exec(command).with_context(|| format!("cannot run remote command '{command}'"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    let code = channel.exit_status()?;
    if !output.trim().is_empty() {
        println!("{}", output.trim_end());
    }
    if code != 0 {
        bail!("remote command '{command}' exited with {code}: {}", stderr.trim());
    }
    Ok(())
}

/// Recursive SFTP upload; remote directories are created as needed.
fn upload(session: &Session, local_root: &Path, remote_root: &str) -> Result<(u64, u64)> {
    let sftp = session.sftp().context("cannot open SFTP")?;
    let remote_root = remote_root.trim_end_matches('/');
    let _ = sftp.mkdir(Path::new(remote_root), 0o755);
    let mut files = 0;
    let mut bytes = 0;
    let mut stack = vec![local_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(local_root).expect("entry under root");
            let remote = format!("{remote_root}/{}", relative.to_string_lossy().replace('\\', "/"));
            if path.is_dir() {
                let _ = sftp.mkdir(Path::new(&remote), 0o755);
                stack.push(path);
            } else {
                let mut local_file = std::fs::File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
                let mut remote_file = sftp
                    .create(Path::new(&remote))
                    .with_context(|| format!("cannot create {remote} on the server"))?;
                bytes += std::io::copy(&mut local_file, &mut remote_file).with_context(|| format!("cannot upload {}", path.display()))?;
                files += 1;
            }
        }
    }
    Ok((files, bytes))
}

/// Single-quote a value for a POSIX shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
