use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::progress::{Step, Transfer, human_bytes};
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
    if !no_build && let Some(build) = app.commands.get("build") {
        eprintln!("[{}] {build}", app.name);
        let status = crate::utils::run_in_dir(build, &project)?;
        if !status.success() {
            bail!("build failed with {status} - nothing uploaded");
        }
    }
    let local = project.join(dist);
    if !local.is_dir() {
        bail!("artifact directory {} does not exist - did the build produce it?", local.display());
    }

    // Walked up front so the upload bar can show a percentage and an ETA; this
    // only stats the tree, which is cheap next to sending it over the wire.
    let plan = plan_upload(&local)?;
    if plan.files == 0 {
        bail!("artifact directory {} is empty - nothing to upload", local.display());
    }

    let step = Step::start(format!("Connecting to {}@{}:{} ...", ssh.user, ssh.host, ssh.port));
    let session = remote::connect(ssh, &server.name)?;
    step.done(format!("Connected to {}@{}:{}", ssh.user, ssh.host, ssh.port));

    if backup {
        let step = Step::start(format!("Backing up {} ...", deploy.path));
        let name = remote::exec(&session, &remote::backup_command(&deploy.path))?;
        step.done(format!("Backup {} created in {}", name.trim(), remote::backups_dir(&deploy.path)));
    }
    if clear {
        let step = Step::start(format!("Clearing {} ...", deploy.path));
        let quoted = remote::shell_quote(&deploy.path);
        remote::exec(&session, &format!("find {quoted} -mindepth 1 -maxdepth 1 -exec rm -rf {{}} +"))?;
        step.done(format!("Cleared {}", deploy.path));
    }

    let plural = if plan.files == 1 { "" } else { "s" };
    let (files, bytes) = upload(&session, &deploy.path, &plan)?;
    println!("Uploaded {files} file{plural} ({}) to {}:{}", human_bytes(bytes), server.name, deploy.path);

    if let Some(restart) = &deploy.restart {
        let step = Step::start(format!("Running: {restart}"));
        let output = remote::exec(&session, restart)?;
        step.done(format!("Ran: {restart}"));
        if !output.trim().is_empty() {
            println!("{}", output.trim_end());
        }
    }
    crate::journal::record("deploy", Some(&app.name), Some(&server.name), Some(&format!("{files} files")));
    println!("Deploy of '{}' to '{}' finished.", app.name, server.name);
    Ok(())
}

/// What the upload is about to send: directories to create and files to copy,
/// each with the path relative to the artifact root.
struct Plan {
    dirs: Vec<String>,
    entries: Vec<(PathBuf, String)>,
    files: u64,
    bytes: u64,
}

/// Walk the artifact directory to learn its size before sending anything.
/// Directories come out parents-first so they can be created in order.
fn plan_upload(local_root: &Path) -> Result<Plan> {
    let mut plan = Plan {
        dirs: Vec::new(),
        entries: Vec::new(),
        files: 0,
        bytes: 0,
    };
    let mut queue = vec![local_root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(local_root).expect("entry under root").to_string_lossy().replace('\\', "/");
            let metadata = entry.metadata().with_context(|| format!("cannot stat {}", path.display()))?;
            if metadata.is_dir() {
                plan.dirs.push(relative);
                queue.push(path);
            } else {
                plan.files += 1;
                plan.bytes += metadata.len();
                plan.entries.push((path, relative));
            }
        }
    }
    // Shallow paths first: a child directory is never created before its parent.
    plan.dirs.sort_by_key(|d| d.matches('/').count());
    Ok(plan)
}

/// SFTP upload of a planned tree; remote directories are created as needed.
fn upload(session: &Session, remote_root: &str, plan: &Plan) -> Result<(u64, u64)> {
    let sftp = session.sftp().context("cannot open SFTP")?;
    let remote_root = remote_root.trim_end_matches('/');
    let _ = sftp.mkdir(Path::new(remote_root), 0o755);
    for dir in &plan.dirs {
        let _ = sftp.mkdir(Path::new(&format!("{remote_root}/{dir}")), 0o755);
    }

    let transfer = Transfer::start(plan.files, plan.bytes);
    let mut files = 0;
    let mut bytes = 0;
    for (path, relative) in &plan.entries {
        transfer.file(relative);
        let remote = format!("{remote_root}/{relative}");
        let mut local_file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let mut remote_file = sftp
            .create(Path::new(&remote))
            .with_context(|| format!("cannot create {remote} on the server"))?;
        let copied = std::io::copy(&mut local_file, &mut remote_file).with_context(|| format!("cannot upload {}", path.display()))?;
        transfer.advance(copied);
        bytes += copied;
        files += 1;
    }
    transfer.finish();
    Ok((files, bytes))
}

#[cfg(test)]
mod tests {
    use super::plan_upload;

    /// The plan drives both the progress total and the order of remote mkdirs,
    /// so it has to count every nested file and list parents before children.
    #[test]
    fn plans_a_nested_tree() {
        let root = tempfile::tempdir().expect("temp dir");
        let deep = root.path().join("assets").join("img");
        std::fs::create_dir_all(&deep).expect("create dirs");
        std::fs::write(root.path().join("index.html"), "hello").expect("write");
        std::fs::write(deep.join("logo.svg"), "12345678").expect("write");

        let plan = plan_upload(root.path()).expect("plan");

        assert_eq!(plan.files, 2);
        assert_eq!(plan.bytes, 13);
        assert_eq!(plan.dirs, vec!["assets", "assets/img"]);
        let mut relative: Vec<_> = plan.entries.iter().map(|(_, r)| r.as_str()).collect();
        relative.sort_unstable();
        assert_eq!(relative, vec!["assets/img/logo.svg", "index.html"]);
    }

    #[test]
    fn an_empty_tree_plans_nothing() {
        let root = tempfile::tempdir().expect("temp dir");
        let plan = plan_upload(root.path()).expect("plan");
        assert_eq!(plan.files, 0);
        assert!(plan.entries.is_empty());
    }
}
