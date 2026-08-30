use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::progress::{self, Step, Transfer, human_bytes, human_duration, rate};
use crate::remote::{self, Resolved};
use crate::shell::Dialect;
use crate::ssh::Session;

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

/// How far a deploy got before it failed.
///
/// A deploy that dies between the upload and the restart leaves the stand in a
/// state that looks fine from outside: the files are new, the service is still
/// running whatever it loaded last. That was a field report from lyrid - the
/// image was rebuilt from new sources that never took effect, and the only way
/// to find out was to log in and look. The error has to say which side of the
/// upload it fell on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reached {
    /// Nothing was written; the remote directory is as it was.
    BeforeUpload,
    /// The files are on the server, and the post-write command did not finish.
    Uploaded,
}

impl Reached {
    /// What the user has to know about the state they were left in.
    fn note(self, path: &crate::model::Path) -> Option<String> {
        match self {
            Reached::BeforeUpload => None,
            Reached::Uploaded => Some(match &path.restart {
                Some(restart) => format!(
                    "The files are already in {} - only `{restart}` did not finish, so the service is still running what it had.\n\
                     Re-run it there, or deploy again with --no-build once the cause is fixed.",
                    path.dir
                ),
                None => format!("The files are already in {}; the deploy failed after writing them.", path.dir),
            }),
        }
    }
}

pub fn run(target_name: Option<String>, overrides: remote::Overrides, no_build: bool, backup: bool, clear: bool, no_archive: bool) -> Result<()> {
    let target = remote::resolve(target_name, overrides)?;
    let (app, server) = (&target.app, &target.server);
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

    // The frame opens after the build on purpose: the build tool's raw output
    // streams above it, and the checklist below covers only our own phases.
    let heading = match &target.target {
        Some(name) => format!("Deploying {name}: {} → {}", app.name, server.name),
        None => format!("Deploying {} → {}", app.name, server.name),
    };
    progress::intro(heading);
    let mut reached = Reached::BeforeUpload;
    match deploy_over_ssh(&target, &plan, &Options { backup, clear, no_archive }, &mut reached) {
        Ok(files) => {
            let what = target.target.clone().unwrap_or_else(|| app.name.clone());
            crate::journal::record("deploy", Some(&what), Some(&server.name), Some(&format!("{files} files")));
            progress::outro(format!("Deploy of '{}' to '{}' finished", app.name, server.name));
            // Only once it worked: offering to remember a target that just
            // failed would be saving a route nobody has driven.
            if target.target.is_none() {
                offer_target(&target)?;
            }
            Ok(())
        }
        Err(err) => {
            // Closes the frame so the error prints on a clean line below it.
            progress::outro_error(format!("Deploy of '{}' to '{}' failed", app.name, server.name));
            match reached.note(&target.path) {
                Some(note) => Err(err.context(note)),
                None => Err(err),
            }
        }
    }
}

/// Offer to save an unnamed tuple as a target, so the next deploy is one word.
///
/// Best-effort: the deploy already succeeded, and failing to write the catalog
/// must not turn a finished deploy into an error. It says so and moves on.
fn offer_target(target: &Resolved) -> Result<()> {
    match crate::commands::target::offer_to_save(&target.app.name, &target.server.name, &target.credential.name, &target.path.name) {
        Ok(Some(name)) => {
            println!("Saved as target '{name}' - next time: `turnout deploy {name}`.");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(err) => {
            progress::warn(&format!("could not save the target: {err:#}"));
            Ok(())
        }
    }
}

struct Options {
    backup: bool,
    clear: bool,
    no_archive: bool,
}

/// Every phase that talks to the server, in checklist order.
///
/// `reached` is written as the deploy passes each point of no return, so the
/// caller can describe the state a failure left behind.
fn deploy_over_ssh(target: &Resolved, plan: &Plan, options: &Options, reached: &mut Reached) -> Result<u64> {
    let (server, credential, path) = (&target.server, &target.credential, &target.path);
    let where_to = target.connection_label();
    let step = Step::start(format!("Connecting to {where_to} ..."));
    let session = remote::connect(server, credential)?;
    // Asked once, before any command is built: everything below is phrased in
    // the dialect this answers with. Under the same step - it is a round trip
    // on a first contact, and a silent one would look like a hang.
    step.update("Detecting the server shell ...");
    let dialect = remote::dialect(&session, server);
    step.done(format!("Connected to {where_to}"));
    remote::check_quotable(dialect, &[&path.dir])?;

    if options.backup {
        let step = Step::start(format!("Backing up {} ...", path.dir));
        let name = remote::run_backup(&session, dialect, &path.dir)?;
        step.done(format!("Backup {} created in {}", name.trim(), remote::backups_dir(&path.dir)));
    }
    if options.clear {
        let step = Step::start(format!("Clearing {} ...", path.dir));
        remote::exec(&session, &dialect.clear_dir(&path.dir))?;
        step.done(format!("Cleared {}", path.dir));
        // Emptying the directory is itself a change to the stand: failing after
        // this leaves it serving nothing, which the note has to cover.
        *reached = Reached::Uploaded;
    }

    let (files, _bytes) = match archive_upload(&session, dialect, &path.dir, plan, options.no_archive)? {
        Some(counts) => counts,
        // Either the caller opted out, the tree is too small to be worth
        // packing, or the server has no tar - send the files one by one.
        None => upload(&session, &path.dir, plan)?,
    };
    *reached = Reached::Uploaded;

    if let Some(restart) = &path.restart {
        let step = Step::start(format!("Running: {restart}"));
        remote::check_quotable(dialect, &[&path.dir])?;
        let output = remote::exec(&session, &restart_command(dialect, &path.dir, restart))?;
        step.done(format!("Restarted: {restart}"));
        if !output.trim().is_empty() {
            progress::info(output.trim_end());
        }
    }
    Ok(files)
}

/// The post-write command, phrased to run where the files just landed.
///
/// In the deploy directory, not the home one: a path's post-write command is
/// defined as "after writing *here*", and up to v0.10 it ran wherever the SSH
/// session dropped the user - so every path had to open with a `cd` of its own
/// to make its own definition true.
///
/// A named function rather than a call spliced into the deploy, because the
/// thing worth asserting is that the `cd` is *there*: a mutation that dropped
/// it went unnoticed while only `Dialect::run_in` was covered.
fn restart_command(dialect: Dialect, deploy_dir: &str, restart: &str) -> String {
    dialect.run_in(deploy_dir, restart)
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

    // The tar probe, the packing and the remote mkdir are one visible phase:
    // each is a wait with no output of its own, and three blinking lines would
    // say less than one.
    let step = Step::start(format!("Packing {} files ...", plan.files));
    if !remote_has_tar(session) {
        step.clear();
        // Not an error: the deploy still works, just the slower way.
        progress::warn(&format!("{} - uploading file by file", no_tar_note(dialect)));
        return Ok(None);
    }
    let archive = pack(plan)?;

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
    step.done(format!("Packed {} files → {}", plan.files, human_bytes(archive.len() as u64)));

    let sent = archive.len() as u64;
    let transfer = Transfer::start(1, sent);
    // Chunked so the bar earns its keep: crediting every byte after the fact
    // would leave it at zero for exactly as long as the upload takes.
    session
        .upload_bytes(&archive, &remote_archive, |chunk| transfer.advance(chunk))
        .with_context(|| format!("cannot upload the archive to {remote_archive}"))?;
    let elapsed = transfer.elapsed();
    transfer.done(format!(
        "Uploaded {} in {} · {}",
        human_bytes(sent),
        human_duration(elapsed),
        rate(sent, elapsed)
    ));

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
    step.done(format!("Unpacked {} files into {}", plan.files, remote_root));
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
    let remote_root = remote_root.trim_end_matches('/');
    // Parents before children (the plan is sorted that way), so no directory is
    // created before the one it sits in.
    session.mkdir(remote_root)?;
    for dir in &plan.dirs {
        session.mkdir(&format!("{remote_root}/{dir}"))?;
    }

    let transfer = Transfer::start(plan.files, plan.bytes);
    let mut files = 0;
    let mut bytes = 0;
    for (path, relative) in &plan.entries {
        let remote = format!("{remote_root}/{relative}");
        let copied = session
            .upload(path, &remote, |chunk| transfer.advance(chunk))
            .with_context(|| format!("cannot upload {}", path.display()))?;
        bytes += copied;
        files += 1;
    }
    let elapsed = transfer.elapsed();
    let plural = if files == 1 { "" } else { "s" };
    transfer.done(format!(
        "Uploaded {files} file{plural} ({}) to {} in {} · {}",
        human_bytes(bytes),
        remote_root,
        human_duration(elapsed),
        rate(bytes, elapsed)
    ));
    Ok((files, bytes))
}

#[cfg(test)]
mod tests {
    use super::{Reached, plan_upload};

    fn a_path(restart: Option<&str>) -> crate::model::Path {
        crate::model::Path {
            name: "wwwroot".into(),
            dir: "/var/www/site".into(),
            restart: restart.map(str::to_string),
        }
    }

    /// The field report from lyrid: the upload landed, the rebuild did not, and
    /// the stand kept serving the old image off new sources. From outside that
    /// reads as "the deploy worked but the version is old", and the only way to
    /// find out was to log in. The failure has to name the state it left.
    #[test]
    fn a_failure_after_the_upload_says_the_files_are_already_there() {
        let note = Reached::Uploaded.note(&a_path(Some("docker compose up -d"))).expect("a note");
        assert!(note.contains("/var/www/site"), "it must name where the files landed: {note}");
        assert!(note.contains("docker compose up -d"), "and which command did not finish: {note}");
        assert!(
            note.contains("still running what it had"),
            "the point is that the service did not pick them up: {note}"
        );
    }

    /// Without a post-write command there is nothing half-done to explain, but
    /// the files are still on the server and saying so is still the difference
    /// between "nothing happened" and "something did".
    #[test]
    fn a_failure_after_the_upload_reports_even_without_a_restart() {
        let note = Reached::Uploaded.note(&a_path(None)).expect("a note");
        assert!(note.contains("/var/www/site"), "{note}");
        assert!(!note.contains("did not finish"), "there was no command to not finish: {note}");
    }

    /// A deploy that never wrote anything must not claim it did - that would
    /// send the user looking for changes on a server that has none.
    #[test]
    fn a_failure_before_the_upload_claims_nothing() {
        assert!(Reached::BeforeUpload.note(&a_path(Some("systemctl restart site"))).is_none());
        assert!(Reached::BeforeUpload.note(&a_path(None)).is_none());
    }

    /// The post-write command has to arrive at the server already anchored to
    /// the deploy directory. Asserting `Dialect::run_in` alone was not enough:
    /// a mutation that sent the bare command survived every test, which is the
    /// exact shape of the defect this release fixes.
    #[test]
    fn the_post_write_command_carries_the_deploy_directory() {
        let posix = super::restart_command(crate::shell::Dialect::Posix, "/var/www/site", "docker compose up -d");
        assert_eq!(posix, "cd '/var/www/site' && docker compose up -d");

        let windows = super::restart_command(crate::shell::Dialect::Windows, "C:\\inetpub\\site", "iisreset");
        assert!(windows.starts_with("cd /d "), "cmd.exe needs /d to change drive: {windows}");
        assert!(windows.ends_with("&& iisreset"), "{windows}");
    }

    /// A path whose command already opens with its own `cd` - the shape every
    /// pre-0.11 setup had to use - still works: the second `cd` lands in the
    /// directory the first one already chose.
    #[test]
    fn a_command_that_still_cds_itself_keeps_working() {
        let command = super::restart_command(crate::shell::Dialect::Posix, "/srv/app", "cd /srv/app && docker compose up -d");
        assert_eq!(command, "cd '/srv/app' && cd /srv/app && docker compose up -d");
    }

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
