use std::path::Path;

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::remote;

pub fn run(app_name: Option<String>, server_name: Option<String>, no_build: bool, backup: bool, clear: bool) -> Result<()> {
    let target = remote::resolve(app_name, server_name)?;
    let (app, server) = (&target.app, &target.server);
    let ssh = remote::require_ssh(server)?;
    let deploy = remote::require_deploy_target(server, &app.name)?;
    let Some(dist) = &app.dist_dir else {
        bail!("app '{0}' has no artifact directory - set it with `turnout app edit {0} --dist DIR`", app.name);
    };

    let project = crate::utils::project_dir(Path::new(&app.path))?;
    if !no_build {
        if let Some(build) = app.commands.get("build") {
            eprintln!("[{}] {build}", app.name);
            let status = crate::utils::run_in_dir(build, &project)?;
            if !status.success() {
                bail!("build failed with {status} - nothing uploaded");
            }
        }
    }
    let local = project.join(dist);
    if !local.is_dir() {
        bail!("artifact directory {} does not exist - did the build produce it?", local.display());
    }

    println!("Connecting to {}@{}:{} ...", ssh.user, ssh.host, ssh.port);
    let session = remote::connect(ssh, &server.name)?;

    if backup {
        let name = remote::exec(&session, &remote::backup_command(&deploy.path))?;
        println!("Backup {} created in {}", name.trim(), remote::backups_dir(&deploy.path));
    }
    if clear {
        println!("Clearing {} ...", deploy.path);
        let quoted = remote::shell_quote(&deploy.path);
        remote::exec(&session, &format!("find {quoted} -mindepth 1 -maxdepth 1 -exec rm -rf {{}} +"))?;
    }

    let (files, bytes) = upload(&session, &local, &deploy.path)?;
    println!("Uploaded {files} files ({} KB) to {}:{}", bytes / 1024, server.name, deploy.path);

    if let Some(restart) = &deploy.restart {
        println!("Running: {restart}");
        let output = remote::exec(&session, restart)?;
        if !output.trim().is_empty() {
            println!("{}", output.trim_end());
        }
    }
    crate::journal::record("deploy", Some(&app.name), Some(&server.name), Some(&format!("{files} files")));
    println!("Deploy of '{}' to '{}' finished.", app.name, server.name);
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
