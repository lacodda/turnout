use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::progress::{Step, Transfer, human_bytes};
use crate::remote;
use crate::shell::Dialect;

/// Below this, packing is not worth the extra round trips: a handful of files
/// go over SFTP faster than tar + upload + untar can set itself up.
const ARCHIVE_THRESHOLD: u64 = 8;

/// Whether a tree of `files` files should travel as one archive.
///
/// Split out from the upload so the rule is testable without a server, and so
/// the opt-out and the threshold live in one readable place.
fn should_archive(files: u64, no_archive: bool) -> bool {
    !no_archive && files >= ARCHIVE_THRESHOLD
}

pub fn run(app_name: Option<String>, server_name: Option<String>, no_build: bool, backup: bool, clear: bool, no_archive: bool) -> Result<()> {
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

    // Asked once, before any command is built: everything below is phrased in
    // the dialect this answers with.
    let dialect = remote::dialect(&session, server);
    remote::check_quotable(dialect, &[&deploy.path])?;

    if backup {
        let step = Step::start(format!("Backing up {} ...", deploy.path));
        let name = remote::run_backup(&session, dialect, &deploy.path)?;
        step.done(format!("Backup {} created in {}", name.trim(), remote::backups_dir(&deploy.path)));
    }
    if clear {
        let step = Step::start(format!("Clearing {} ...", deploy.path));
        remote::exec(&session, &dialect.clear_dir(&deploy.path))?;
        step.done(format!("Cleared {}", deploy.path));
    }

    let plural = if plan.files == 1 { "" } else { "s" };
    let (files, bytes) = match archive_upload(&session, dialect, &deploy.path, &plan, no_archive)? {
        Some(counts) => counts,
        // Either the caller opted out, the tree is too small to be worth
        // packing, or the server has no tar - send the files one by one.
        None => upload(&session, &deploy.path, &plan)?,
    };
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

/// Pack the tree, send one file, unpack it on the server.
///
/// Returns `None` when this route does not apply and the caller should fall
/// back to sending files one at a time. A dist directory is usually thousands
/// of small files, and SFTP pays a round trip per file; one archive turns that
/// into a single stream plus one `tar` invocation.
///
/// The archive lands in a temporary file and is deleted afterwards, including
/// when unpacking fails - a stray multi-megabyte tarball next to the site is
/// its own kind of bug.
fn archive_upload(session: &Session, dialect: Dialect, remote_root: &str, plan: &Plan, no_archive: bool) -> Result<Option<(u64, u64)>> {
    if !should_archive(plan.files, no_archive) {
        return Ok(None);
    }
    if !remote_has_tar(session) {
        // Not an error: the deploy still works, just the slower way.
        eprintln!("note: {} - uploading file by file", no_tar_note(dialect));
        return Ok(None);
    }

    let step = Step::start(format!("Packing {} files ...", plan.files));
    let archive = pack(plan)?;
    step.clear();

    let remote_root = remote_root.trim_end_matches('/');
    // Inside the deploy directory, which is the one place we know is writable:
    // uploading works there by definition. Beside it would mean writing to the
    // parent - typically /var/www, owned by root - and that is exactly the
    // permission wall backups run into (see `remote::permission_hint`).
    //
    // A dotfile, so a web server serving this directory does not hand it out,
    // and it is removed as soon as it is unpacked.
    let remote_archive = remote::join_remote(remote_root, ".turnout-upload.tar.gz");

    // The archive goes inside the deploy directory, so it has to exist first -
    // on a first deploy it does not.
    remote::exec(session, &dialect.mkdir_p(remote_root))?;

    let transfer = Transfer::start(1, archive.len() as u64);
    transfer.file(&format!("{} (packed)", human_bytes(archive.len() as u64)));
    let sftp = session.sftp().context("cannot open SFTP")?;
    let sent = {
        use std::io::Write;
        let mut remote_file = sftp
            .create(Path::new(&remote_archive))
            .with_context(|| format!("cannot create {remote_archive} on the server"))?;
        remote_file
            .write_all(&archive)
            .with_context(|| format!("cannot upload the archive to {remote_archive}"))?;
        archive.len() as u64
    };
    transfer.advance(sent);
    transfer.finish();

    let step = Step::start("Unpacking on the server ...".to_string());
    let unpack = remote::exec(
        session,
        &dialect.and_then(&dialect.untar(&remote_archive, remote_root), &dialect.remove_file(&remote_archive)),
    );
    if let Err(err) = unpack {
        // Best-effort: the upload already failed, and a leftover archive would
        // outlive the error message.
        let _ = remote::exec(session, &dialect.remove_file(&remote_archive));
        step.clear();
        return Err(err).context("cannot unpack the archive on the server");
    }
    step.done(format!("Unpacked {} files", plan.files));
    Ok(Some((plan.files, plan.bytes)))
}

/// Whether the server can unpack what we would send.
///
/// Probed rather than assumed: `tar` is missing often enough on minimal
/// containers, and finding out mid-deploy would leave an archive behind and
/// nothing unpacked.
///
/// The probe is asked in the server's own dialect. The previous one was
/// `command -v tar >/dev/null 2>&1`, which `cmd.exe` cannot run at all - so a
/// Windows server always answered "no tar" no matter what it had installed. It
/// had bsdtar in System32 the whole time.
/// No dialect parameter on purpose: the fix was to stop asking in shell syntax
/// at all. `tar --version` is a plain program invocation that both bsdtar and
/// GNU tar answer, and running a program is the one thing every shell does the
/// same way.
fn remote_has_tar(session: &Session) -> bool {
    remote::exec(session, "tar --version").is_ok()
}

/// What to say when the archive route is off, without claiming more than we know.
///
/// The old text was `no usable tar on the server` for every failure, which
/// merged two different things: a fact about the server, and a defect on our
/// side. A field report cost a round of debugging and a feature request because
/// of it - the server had tar, and the message said it did not.
fn no_tar_note(dialect: Dialect) -> String {
    match dialect {
        Dialect::Posix => "the server has no usable tar".to_string(),
        // On Windows the probe is young enough that a failure is at least as
        // likely to be ours as the server's; say both.
        Dialect::Windows => {
            "tar did not answer on this Windows server (it ships with one since Windows 10 1803 - if it is there, this is a turnout bug)".to_string()
        }
    }
}

/// Build the gzipped tar in memory, entries relative to the artifact root so it
/// unpacks straight into the deploy directory.
fn pack(plan: &Plan) -> Result<Vec<u8>> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for (path, relative) in &plan.entries {
        let mut file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        builder
            .append_file(relative, &mut file)
            .with_context(|| format!("cannot add {} to the archive", path.display()))?;
    }
    let encoder = builder.into_inner().context("cannot finish the archive")?;
    encoder.finish().context("cannot compress the archive")
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

    /// A real dist directory is thousands of files and always packs; a couple
    /// of files are faster sent as they are. `--no-archive` overrides both.
    #[test]
    fn only_trees_worth_packing_are_packed() {
        assert!(super::should_archive(2_000, false), "a dist directory packs");
        assert!(super::should_archive(super::ARCHIVE_THRESHOLD, false), "the threshold itself packs");
        assert!(!super::should_archive(super::ARCHIVE_THRESHOLD - 1, false), "just below it does not");
        assert!(!super::should_archive(1, false), "a single file never packs");
        assert!(!super::should_archive(2_000, true), "--no-archive wins over any size");
    }

    /// The archive has to unpack straight into the deploy directory, so entries
    /// are stored relative to the artifact root - an absolute or `./`-prefixed
    /// path would land the files somewhere else entirely.
    #[test]
    fn the_archive_holds_relative_paths_and_content() {
        let root = tempfile::tempdir().expect("temp dir");
        let deep = root.path().join("assets").join("img");
        std::fs::create_dir_all(&deep).expect("create dirs");
        std::fs::write(root.path().join("index.html"), "<!doctype html>").expect("write");
        std::fs::write(deep.join("logo.svg"), "<svg/>").expect("write");

        let plan = super::plan_upload(root.path()).expect("plan");
        let archive = super::pack(&plan).expect("pack");

        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(&archive));
        let mut tar = tar::Archive::new(decoder);
        let mut found = std::collections::BTreeMap::new();
        for entry in tar.entries().expect("entries") {
            use std::io::Read;
            let mut entry = entry.expect("entry");
            let path = entry.path().expect("path").to_string_lossy().replace('\\', "/");
            let mut content = String::new();
            entry.read_to_string(&mut content).expect("read");
            found.insert(path, content);
        }

        assert_eq!(found.get("index.html").map(String::as_str), Some("<!doctype html>"));
        assert_eq!(found.get("assets/img/logo.svg").map(String::as_str), Some("<svg/>"));
        assert_eq!(found.len(), 2, "only files travel; directories come from the paths: {found:?}");
        assert!(
            !found.keys().any(|p| p.starts_with('/') || p.starts_with("./")),
            "paths must be relative: {found:?}"
        );
    }

    /// Compression is the point: a dist directory of text files should not
    /// cross the wire at full size.
    #[test]
    fn the_archive_is_smaller_than_the_tree() {
        let root = tempfile::tempdir().expect("temp dir");
        for index in 0..20 {
            std::fs::write(root.path().join(format!("chunk-{index}.js")), "console.log('hello world');\n".repeat(200)).expect("write");
        }
        let plan = super::plan_upload(root.path()).expect("plan");
        let archive = super::pack(&plan).expect("pack");
        assert!(
            (archive.len() as u64) < plan.bytes / 4,
            "expected real compression, got {} from {}",
            archive.len(),
            plan.bytes
        );
    }

    #[test]
    fn an_empty_tree_plans_nothing() {
        let root = tempfile::tempdir().expect("temp dir");
        let plan = plan_upload(root.path()).expect("plan");
        assert_eq!(plan.files, 0);
        assert!(plan.entries.is_empty());
    }
}
