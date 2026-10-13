use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::artifact::{self, Artifact, Plan};
use crate::detached::Remote;
use crate::job;
use crate::model::{HealthCheck, Probe};
use crate::progress::{self, Step, Transfer, human_bytes, human_duration, rate};
use crate::remote::{self, Resolved};
use crate::shell::Dialect;
use crate::ssh::Session;
use crate::store;

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
    /// The deploy failed its check and the stand was put back as it was; the
    /// error says so itself.
    RolledBack,
}

impl Reached {
    /// What the user has to know about the state they were left in.
    fn note(self, path: &crate::model::Path) -> Option<String> {
        match self {
            Reached::BeforeUpload | Reached::RolledBack => None,
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

/// The switches of one deploy.
pub struct Flags {
    pub no_build: bool,
    pub backup: bool,
    pub clear: bool,
    pub no_archive: bool,
    /// Compare with the server and stop: nothing is written.
    pub diff: bool,
    /// With `diff`: the comparison as JSON.
    pub json: bool,
    /// Open each stand in the browser once its deploy is through.
    pub open: bool,
    pub console: crate::cli::Console,
}

pub fn run(names: Vec<String>, overrides: remote::Overrides, flags: Flags) -> Result<()> {
    let targets = resolve_all(names, overrides)?;
    let verbose = flags.console.verbose;
    let console = job::Mode::resolve(job::Mode::Quiet, verbose);
    let apps = store::load_apps()?;
    let is_command = |name: &str| apps.iter().any(|app| app.commands.contains_key(name));
    if !flags.diff {
        let placement = crate::commands::jobs::placement("deploy", flags.console, console, &is_command);
        if placement.detached() {
            let preferred = placement == crate::commands::jobs::Placement::Preferred;
            return in_background(&targets, &flags_for(&flags), preferred);
        }
    }

    let artifacts = prepare(&targets, flags.no_build, console)?;
    if flags.diff {
        return compare_all(&targets, &artifacts, flags.clear, flags.json);
    }

    let options = Options {
        backup: flags.backup,
        clear: flags.clear,
        no_archive: flags.no_archive,
    };
    for (index, target) in targets.iter().enumerate() {
        if let Err(err) = deploy_one(target, artifact_for(&artifacts, target), &options, flags.open) {
            let rest: Vec<String> = targets[index + 1..].iter().map(label).collect();
            if !rest.is_empty() {
                // A deploy is often staging first, production second: going
                // on after the first failure would ship what just failed.
                progress::warn(&format!("not attempted: {} - a deploy stops at the first target that fails", rest.join(", ")));
            }
            return Err(err);
        }
    }
    if targets.len() > 1 {
        println!("Deployed to {}.", targets.iter().map(label).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

/// The build a target sends.
fn artifact_for<'a>(artifacts: &'a [Artifact], target: &Resolved) -> &'a Artifact {
    artifacts
        .iter()
        .find(|artifact| artifact.app == target.app.name)
        .expect("every app was prepared")
}

/// What a target is called in a report: its name, or `app → server` for a
/// tuple nobody named.
fn label(target: &Resolved) -> String {
    target.target.clone().unwrap_or_else(|| format!("{} → {}", target.app.name, target.server.name))
}

/// Every target the command names, in the order it names them.
///
/// The overrides change one route for one run, so they take one target: on
/// several they would quietly send every one of them to the same place.
fn resolve_all(names: Vec<String>, overrides: remote::Overrides) -> Result<Vec<Resolved>> {
    if names.len() <= 1 {
        return Ok(vec![remote::resolve(names.into_iter().next(), overrides)?]);
    }
    if overrides.server.is_some() || overrides.credential.is_some() || overrides.path.is_some() {
        bail!("--server, --credential and --path change one deploy - name a single target with them");
    }
    let mut seen = Vec::new();
    for name in &names {
        if seen.contains(name) {
            bail!("'{name}' is named twice - each target is deployed once");
        }
        seen.push(name.clone());
    }
    names
        .into_iter()
        .map(|name| remote::resolve(Some(name), remote::Overrides::default()))
        .collect()
}

/// Build each app once, and walk what it built.
///
/// Every build runs before any server is touched: a second app that does not
/// build should stop the deploy before the first one is half out.
fn prepare(targets: &[Resolved], no_build: bool, console: job::Mode) -> Result<Vec<Artifact>> {
    let mut artifacts: Vec<Artifact> = Vec::new();
    for target in targets {
        let app = &target.app;
        if artifacts.iter().any(|artifact| artifact.app == app.name) {
            continue;
        }
        let Some(dist) = &app.dist_dir else {
            bail!("app '{0}' has no artifact directory - set it with `turnout app edit {0} --dist DIR`", app.name);
        };
        let project = crate::utils::project_dir(Path::new(&app.path))?;
        let build = app.commands.get("build").filter(|_| !no_build);
        if let Some(build) = build {
            eprintln!("[{}] {build}", app.name);
            // Same quiet console as `turnout build`: a loader with the elapsed
            // time, the output only when it fails. A deploy already renders a
            // checklist below this, and a build tool's own scrollback between the
            // two made the whole command look like two unrelated programs.
            let mut job = job::Job::claim(&app.name, "build", false)?;
            let outcome = job::run(
                job::Program::Shell(build),
                &project,
                &[],
                console,
                &format!("Building {}", app.name),
                &mut job,
                None,
            )?;
            if !outcome.status.success() {
                bail!("build failed with {} - nothing uploaded", outcome.status);
            }
        }
        let local = project.join(dist);
        if !local.is_dir() {
            bail!("artifact directory {} does not exist - did the build produce it?", local.display());
        }
        // Walked up front so the upload bar can show a percentage and an ETA;
        // this only stats the tree, which is cheap next to sending it.
        let plan = artifact::plan_upload(&local)?;
        if plan.files == 0 {
            bail!("artifact directory {} is empty - nothing to upload", local.display());
        }
        if build.is_none() {
            warn_if_stale(app, &project, &local, &plan);
        }
        artifacts.push(Artifact::new(&app.name, plan));
    }
    Ok(artifacts)
}

/// Say so when the build about to go out is older than its sources.
///
/// Only when this deploy did not build: then what goes out is whatever the
/// last build left, and a source saved since is a change the stand will not
/// get. Said, not refused - deploying a known older build is sometimes the
/// point.
fn warn_if_stale(app: &crate::model::App, project: &Path, local: &Path, plan: &Plan) {
    let Some(built) = plan.newest else { return };
    let Some(stale) = artifact::stale(project, local, built, &[Path::new(app.env_file_name())]) else {
        return;
    };
    let remedy = if app.commands.contains_key("build") {
        "deploy without --no-build to build it first".to_string()
    } else {
        format!("build it before deploying - '{}' has no build command for turnout to run", app.name)
    };
    progress::warn(&format!(
        "{} is older than its sources: {} changed {} after the last build - {remedy}",
        app.dist_dir.as_deref().unwrap_or("the build"),
        stale.source,
        crate::progress::human_span(stale.after.as_secs())
    ));
}

/// Deploy one target: the frame, the work, and what follows a success.
fn deploy_one(target: &Resolved, artifact: &Artifact, options: &Options, open: bool) -> Result<()> {
    let (app, server) = (&target.app, &target.server);
    // The frame opens after the build on purpose: the build tool's raw output
    // streams above it, and the checklist below covers only our own phases.
    let heading = match &target.target {
        Some(name) => format!("Deploying {name}: {} → {}", app.name, server.name),
        None => format!("Deploying {} → {}", app.name, server.name),
    };
    progress::intro(heading);
    if let Some(unchecked) = &target.unchecked {
        progress::warn(unchecked);
    }
    let connect = || remote::connect(&target.server, &target.credential);
    let mut reached = Reached::BeforeUpload;
    match deploy_over_ssh(target, artifact, options, &mut reached, &connect) {
        Ok(files) => {
            let what = target.target.clone().unwrap_or_else(|| app.name.clone());
            crate::journal::record("deploy", Some(&what), Some(&server.name), Some(&format!("{files} files")));
            progress::outro(format!("Deploy of '{}' to '{}' finished", app.name, server.name));
            // Only once it worked: offering to remember a target that just
            // failed would be saving a route nobody has driven.
            if target.target.is_none() {
                offer_target(target)?;
            }
            if open {
                println!("Opening {}", target.address);
                crate::front::open_in_browser(&target.address)?;
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

/// Hand the deploy to a background supervisor, as `turnout deploy` again.
///
/// Everything a picker would ask is settled here, in the terminal, and passed
/// on by name: the background has nobody to ask. A named target travels as
/// its name; anything else as the app plus every field spelled out, so the
/// supervisor's deploy resolves to exactly this tuple and not to whatever the
/// binding says by the time it runs.
///
/// The stand's address travels too: a deploy that went out says so with a
/// notification that opens the stand.
fn in_background(targets: &[Resolved], flags: &[&str], preferred: bool) -> Result<()> {
    let first = &targets[0];
    let app = &first.app;
    // A background job belongs to one app - it is what `ps`, `logs` and
    // `stop` find it by.
    if let Some(other) = targets.iter().find(|target| target.app.name != app.name) {
        bail!(
            "'{}' and '{}' deploy different apps, and a background deploy belongs to one - detach them one at a time",
            label(first),
            label(other)
        );
    }
    let mut args = vec!["deploy".to_string()];
    for target in targets {
        match &target.target {
            Some(name) => args.push(name.clone()),
            None => {
                // `deploy NAME` reads NAME as a target first, so an app that shares
                // its name with some target cannot be passed by name at all.
                if store::load_targets()?.iter().any(|t| t.name == app.name) {
                    bail!(
                        "a target is also named '{0}', so the background deploy cannot name the app - deploy a target by name, or without --detach",
                        app.name
                    );
                }
                args.extend([
                    app.name.clone(),
                    "--server".into(),
                    target.server.name.clone(),
                    "--credential".into(),
                    target.credential.name.clone(),
                    "--path".into(),
                    target.path.name.clone(),
                ]);
            }
        }
    }
    args.extend(flags.iter().map(|flag| flag.to_string()));
    let project = crate::utils::project_dir(Path::new(&app.path))?;
    crate::commands::jobs::detach(crate::commands::jobs::Detach {
        app: &app.name,
        command: "deploy",
        dir: &project,
        env: &[],
        label: &format!("Deploying {}", app.name),
        ready: false,
        own: None,
        open: false,
        itself: true,
        program: args,
        link: Some(first.address.clone()),
        preferred,
    })
}

/// The deploy switches, spelled the way the CLI reads them.
fn flags_for(flags: &Flags) -> Vec<&'static str> {
    [
        (flags.no_build, "--no-build"),
        (flags.backup, "--backup"),
        (flags.clear, "--clear"),
        (flags.no_archive, "--no-archive"),
    ]
    .into_iter()
    .filter_map(|(on, flag)| on.then_some(flag))
    .collect()
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

/// Why a backup is taken, which decides what becomes of it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kept {
    /// Asked for with `--backup`: it stays.
    Asked,
    /// Taken only so a failed check has something to roll back to: removed
    /// once the check passes, or every checked deploy would leave an archive
    /// behind until the disk is full.
    ForRollback,
}

/// Every phase that talks to the server, in checklist order.
///
/// `reached` is written as the deploy passes each point of no return, so the
/// caller can describe the state a failure left behind. `connect` opens the
/// session, and opens it again if it drops while the restart runs.
fn deploy_over_ssh(target: &Resolved, artifact: &Artifact, options: &Options, reached: &mut Reached, connect: &dyn Fn() -> Result<Session>) -> Result<u64> {
    let (server, path) = (&target.server, &target.path);
    let where_to = target.connection_label();
    let step = Step::start(format!("Connecting to {where_to} ..."));
    let mut session = connect()?;
    // Asked once, before any command is built: everything below is phrased in
    // the dialect this answers with. Under the same step - it is a round trip
    // on a first contact, and a silent one would look like a hang.
    step.update("Detecting the server shell ...");
    let dialect = remote::dialect(&session, server);
    step.done(format!("Connected to {where_to}"));
    remote::check_quotable(dialect, &[&path.dir])?;

    let kept = if options.backup {
        Some(Kept::Asked)
    } else {
        target.health.as_ref().map(|_| Kept::ForRollback)
    };
    let backup = match kept {
        Some(kept) => back_up(&session, dialect, &path.dir, kept)?,
        None => None,
    };
    if options.clear {
        let step = Step::start(format!("Clearing {} ...", path.dir));
        remote::exec(&session, &dialect.clear_dir(&path.dir))?;
        step.done(format!("Cleared {}", path.dir));
        // Emptying the directory is itself a change to the stand: failing after
        // this leaves it serving nothing, which the note has to cover.
        *reached = Reached::Uploaded;
    }

    let files = match archive_upload(&session, dialect, &path.dir, artifact, options.no_archive)? {
        Some(files) => files,
        // Either the caller opted out, the tree is too small to be worth
        // packing, or the server has no tar - send the files one by one.
        None => upload(&session, &path.dir, &artifact.plan)?,
    };
    *reached = Reached::Uploaded;

    let restarted = match &path.restart {
        Some(restart) => run_restart(&mut session, dialect, path, restart, connect),
        None => Ok(()),
    };
    let Some(check) = &target.health else {
        return restarted.map(|()| files);
    };
    // A check is the owner's word for "this works"; a restart that failed is
    // as much a failed deploy as a check that does not pass.
    let verdict = match restarted {
        Ok(()) => {
            let step = Step::start(format!("Checking {} ...", check.describe(&target.address)));
            match wait_healthy(check, target, &session, dialect, |answer| {
                step.update(format!("Checking {} - {answer}", check.describe(&target.address)))
            }) {
                Ok(answer) => {
                    step.done(format!("Healthy: {} - {answer}", check.describe(&target.address)));
                    Ok(())
                }
                Err(last) => {
                    step.clear();
                    Err(format!(
                        "the health check failed: {} {last} after {}s of asking",
                        check.describe(&target.address),
                        check.wait().as_secs()
                    ))
                }
            }
        }
        Err(err) => Err(format!("{err:#}")),
    };
    match (verdict, backup) {
        (Ok(()), Some(name)) if kept == Some(Kept::ForRollback) => {
            // Best-effort: the deploy is through, and a leftover archive is
            // the next deploy's to tidy, not a reason to fail this one.
            if let Err(err) = remote::remove_backup(&session, dialect, &path.dir, &name) {
                progress::warn(&format!("could not remove the rollback backup {name}: {err:#}"));
            }
            Ok(files)
        }
        (Ok(()), _) => Ok(files),
        (Err(why), None) => bail!("{why} - and this was the first deploy to {}, so there is nothing to roll back to", path.dir),
        (Err(why), Some(name)) => {
            progress::warn(&why);
            let after = roll_back(&mut session, dialect, target, &name, connect).with_context(|| {
                format!(
                    "{why}, and rolling back to {name} failed too - the stand is in between; the backup is in {}",
                    remote::backups_dir(&path.dir)
                )
            })?;
            *reached = Reached::RolledBack;
            bail!("{why} - rolled back to {name}: {after}")
        }
    }
}

/// Take the backup a deploy needs, or say why there is none.
fn back_up(session: &Session, dialect: Dialect, dir: &str, kept: Kept) -> Result<Option<String>> {
    if !remote::dir_exists(session, dialect, dir) {
        // A first deploy: nothing there to keep, and tar would fail on a
        // directory that does not exist.
        if kept == Kept::Asked {
            progress::warn(&format!("nothing to back up yet - {dir} is not there; the deploy creates it"));
        }
        return Ok(None);
    }
    let step = Step::start(format!("Backing up {dir} ..."));
    let name = remote::run_backup(session, dialect, dir)?;
    let name = name.trim().to_string();
    match kept {
        Kept::Asked => step.done(format!("Backup {name} created in {}", remote::backups_dir(dir))),
        Kept::ForRollback => step.done(format!("Backup {name} kept for a rollback until the check passes")),
    }
    Ok(Some(name))
}

/// Run the path's post-write command, with its newest line beside the spinner.
fn run_restart(session: &mut Session, dialect: Dialect, path: &crate::model::Path, restart: &str, connect: &dyn Fn() -> Result<Session>) -> Result<()> {
    let step = Step::start(format!("Running: {restart}"));
    let output = remote::restart(session, dialect, path, restart, connect, |line| {
        step.update(format!("Running: {restart} · {line}"))
    })?;
    step.done(format!("Restarted: {restart}"));
    if !output.trim().is_empty() {
        progress::info(output.trim_end());
    }
    Ok(())
}

/// Put the stand back on `backup` and ask it again; says how it answers.
///
/// The second ask is what tells the two failures apart: a stand that works
/// on the old files was broken by this deploy, and one that does not was
/// broken before it.
fn roll_back(session: &mut Session, dialect: Dialect, target: &Resolved, backup: &str, connect: &dyn Fn() -> Result<Session>) -> Result<String> {
    let path = &target.path;
    let step = Step::start(format!("Rolling back to {backup} ..."));
    remote::restore_backup(session, dialect, &path.dir, backup)?;
    step.done(format!("Rolled back {} to {backup}", path.dir));
    if let Some(restart) = &path.restart {
        run_restart(session, dialect, path, restart, connect)?;
    }
    let Some(check) = &target.health else {
        return Ok("restored".to_string());
    };
    let step = Step::start(format!("Checking {} again ...", check.describe(&target.address)));
    Ok(match wait_healthy(check, target, session, dialect, |_| {}) {
        Ok(answer) => {
            step.done(format!("Healthy again: {} - {answer}", check.describe(&target.address)));
            format!("the stand works again ({answer})")
        }
        Err(last) => {
            step.clear();
            format!("and it still fails the check ({last}) - the cause is not only in this deploy")
        }
    })
}

/// Between two asks of a health check.
const HEALTH_EVERY: Duration = if cfg!(test) { Duration::from_millis(100) } else { Duration::from_secs(2) };

/// Ask the stand until it says it works or the check's wait runs out. Ok
/// carries the answer that passed, Err the last one that did not. At least
/// one ask, whatever the wait.
fn wait_healthy(check: &HealthCheck, target: &Resolved, remote: &impl Remote, dialect: Dialect, mut heard: impl FnMut(&str)) -> Result<String, String> {
    let started = Instant::now();
    loop {
        let answer = match &check.probe {
            Probe::Url(url) => crate::health::ask(&crate::model::health_url(url, &target.address), target.server.accept_invalid_certs),
            Probe::Command(command) => match remote.exec_bytes(&dialect.run_in(&target.path.dir, command)) {
                Ok(_) => Ok("exit 0".to_string()),
                Err(err) => Err(last_words(&format!("{err:#}"))),
            },
        };
        match answer {
            Ok(answer) => return Ok(answer),
            Err(last) if started.elapsed() + HEALTH_EVERY > check.wait() => return Err(last),
            Err(last) => {
                heard(&last);
                std::thread::sleep(HEALTH_EVERY);
            }
        }
    }
}

/// The part of a failed command's error worth repeating: from its exit code
/// on, without the command line it already quoted.
fn last_words(error: &str) -> String {
    match error.split_once("exited with ") {
        Some((_, rest)) => format!("exited with {}", rest.trim()),
        None => error.to_string(),
    }
}

/// One target's comparison, as `--diff --json` prints it.
#[derive(Serialize)]
struct Compared {
    target: String,
    server: String,
    dir: String,
    #[serde(flatten)]
    diff: crate::manifest::Diff,
}

/// `--diff`: compare the build with every target, write nothing.
fn compare_all(targets: &[Resolved], artifacts: &[Artifact], clear: bool, json: bool) -> Result<()> {
    let mut all = Vec::new();
    for target in targets {
        let artifact = artifact_for(artifacts, target);
        let path = &target.path;
        if json {
            let session = remote::connect(&target.server, &target.credential)?;
            let dialect = remote::dialect(&session, &target.server);
            remote::check_quotable(dialect, &[&path.dir])?;
            all.push(Compared {
                target: label(target),
                server: target.server.name.clone(),
                dir: path.dir.clone(),
                diff: crate::manifest::diff(&session, dialect, &path.dir, &artifact.plan, clear)?,
            });
            continue;
        }
        progress::intro(format!("Comparing {}: {} → {}", label(target), target.app.name, target.server.name));
        let where_to = target.connection_label();
        let step = Step::start(format!("Connecting to {where_to} ..."));
        let session = remote::connect(&target.server, &target.credential)?;
        let dialect = remote::dialect(&session, &target.server);
        step.done(format!("Connected to {where_to}"));
        remote::check_quotable(dialect, &[&path.dir])?;
        let step = Step::start(format!("Comparing {} files with {} ...", artifact.plan.files, path.dir));
        let diff = crate::manifest::diff(&session, dialect, &path.dir, &artifact.plan, clear)?;
        step.done(format!("Compared {} files with {}", artifact.plan.files, path.dir));
        for line in diff.lines() {
            progress::info(&line);
        }
        progress::outro(if diff.changes() {
            "Nothing was written - deploy without --diff to make these changes"
        } else {
            "Nothing was written - the stand already has this build"
        });
    }
    if json {
        println!("{}", crate::output::json(&all)?);
    }
    Ok(())
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
fn archive_upload(session: &Session, dialect: Dialect, remote_root: &str, artifact: &Artifact, no_archive: bool) -> Result<Option<u64>> {
    let plan = &artifact.plan;
    if !artifact::should_archive(plan.files, no_archive) {
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
    let archive = artifact.archive()?;

    let remote_root = remote_root.trim_end_matches(['/', '\\']);
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
        .upload_bytes(archive, &remote_archive, |chunk| transfer.advance(chunk))
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
    Ok(Some(plan.files))
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

/// SFTP upload of a planned tree; remote directories are created as needed.
fn upload(session: &Session, remote_root: &str, plan: &Plan) -> Result<u64> {
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
    for entry in &plan.entries {
        let remote = format!("{remote_root}/{}", entry.relative);
        let copied = session
            .upload(&entry.path, &remote, |chunk| transfer.advance(chunk))
            .with_context(|| format!("cannot upload {}", entry.path.display()))?;
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
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// send the user looking for changes on a server that has none. Nor must
    /// one that was rolled back: its own error says what it left.
    #[test]
    fn a_failure_before_the_upload_claims_nothing() {
        assert!(Reached::BeforeUpload.note(&a_path(Some("systemctl restart site"))).is_none());
        assert!(Reached::BeforeUpload.note(&a_path(None)).is_none());
        assert!(Reached::RolledBack.note(&a_path(Some("systemctl restart site"))).is_none());
    }

    #[test]
    fn a_failed_command_is_quoted_from_its_code_on() {
        assert_eq!(
            last_words("remote command 'cd /x && curl -f localhost' exited with 22: curl: (22) 503"),
            "exited with 22: curl: (22) 503"
        );
        assert_eq!(last_words("connection reset"), "connection reset");
    }

    /// A stand that answers 503 twice and then 200 - a service coming up.
    fn a_stand_that_wakes_up(failures: usize) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        use std::sync::atomic::Ordering;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let seen = counter.fetch_add(1, Ordering::SeqCst);
                let status = if seen < failures { "503 Service Unavailable" } else { "200 OK" };
                let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        });
        (port, asked)
    }

    fn a_target(dir: &str, restart: Option<&str>, health: Option<HealthCheck>, address: &str) -> Resolved {
        Resolved {
            app: crate::model::App {
                name: "web".into(),
                path: ".".into(),
                commands: Default::default(),
                dist_dir: Some("dist".into()),
                gateway_port: None,
                gateway_env: None,
                env_file: None,
                dev_port: None,
                servers: Vec::new(),
            },
            server: crate::model::Server {
                name: "stand".into(),
                label: None,
                url: address.into(),
                host: Some("127.0.0.1".into()),
                port: 22,
                accept_invalid_certs: false,
                shell: None,
                credential: None,
            },
            credential: crate::model::Credential {
                name: "deploy".into(),
                user: "deploy".into(),
                auth: crate::model::Auth::Password,
                key: None,
            },
            path: crate::model::Path {
                name: "site".into(),
                dir: dir.into(),
                restart: restart.map(str::to_string),
            },
            target: Some("web-stand".into()),
            address: address.into(),
            health,
            unchecked: None,
        }
    }

    /// A service that takes a moment to come up passes; one that never does
    /// fails with the last thing it said, once the wait is spent.
    #[test]
    fn the_check_waits_for_a_stand_coming_up() {
        let (port, asked) = a_stand_that_wakes_up(2);
        let address = format!("http://127.0.0.1:{port}");
        let check = HealthCheck {
            probe: Probe::Url("/health".into()),
            wait: Some(5),
        };
        let target = a_target("/srv", None, Some(check.clone()), &address);
        let stand = crate::ssh::tests::Stand::spawn();
        let session = stand.session();
        let mut heard = Vec::new();
        let answer = wait_healthy(&check, &target, &session, Dialect::Posix, |line| heard.push(line.to_string())).unwrap();
        assert!(answer.starts_with("200 in "), "{answer}");
        assert_eq!(heard, vec!["answered 503", "answered 503"]);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 3);

        let (port, _) = a_stand_that_wakes_up(usize::MAX);
        let never = HealthCheck {
            probe: Probe::Url(format!("http://127.0.0.1:{port}/health")),
            wait: Some(0),
        };
        let started = Instant::now();
        assert_eq!(wait_healthy(&never, &target, &session, Dialect::Posix, |_| {}).unwrap_err(), "answered 503");
        assert!(started.elapsed() < Duration::from_secs(2), "a zero wait asks once");
    }

    fn a_build(files: &[(&str, &str)]) -> (tempfile::TempDir, Artifact) {
        let root = tempfile::tempdir().unwrap();
        for (path, content) in files {
            std::fs::write(root.path().join(path), content).unwrap();
        }
        // Enough files to take the archive route, as a real dist does.
        for index in 0..artifact::ARCHIVE_THRESHOLD {
            std::fs::write(root.path().join(format!("chunk-{index}.js")), "x").unwrap();
        }
        let plan = artifact::plan_upload(root.path()).unwrap();
        let artifact = Artifact::new("web", plan);
        (root, artifact)
    }

    fn backups(dir: &Path) -> Vec<String> {
        let backups = std::path::PathBuf::from(remote::backups_dir(&dir.display().to_string()));
        match std::fs::read_dir(backups) {
            Ok(entries) => entries.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The whole of a checked deploy, through a real SSH session into this
    /// machine's own shell: a first deploy that passes, a second whose check
    /// fails and is rolled back to the first, a third that passes and leaves
    /// no archive behind.
    #[test]
    fn a_deploy_that_fails_its_check_is_rolled_back() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let connect = || Ok(stand.session());
        let server = tempfile::tempdir().unwrap();
        let site = server.path().join("site");
        let dir = site.display().to_string();
        let (restart, probe) = if cfg!(windows) {
            ("echo restarted> restarted.txt", "findstr ok health.txt")
        } else {
            ("echo restarted > restarted.txt", "grep -q ok health.txt")
        };
        let check = HealthCheck {
            probe: Probe::Command(probe.into()),
            wait: Some(1),
        };
        let target = a_target(&dir, Some(restart), Some(check), "http://127.0.0.1:1");
        let options = Options {
            backup: false,
            clear: false,
            no_archive: false,
        };

        let (_v1, first) = a_build(&[("health.txt", "ok"), ("index.html", "v1")]);
        let mut reached = Reached::BeforeUpload;
        deploy_over_ssh(&target, &first, &options, &mut reached, &connect).expect("a healthy first deploy");
        assert!(site.join("restarted.txt").is_file(), "the restart ran in the deploy directory");
        assert!(backups(&site).is_empty(), "a first deploy has nothing to back up: {:?}", backups(&site));

        let (_v2, broken) = a_build(&[("health.txt", "down"), ("index.html", "v2"), ("only-in-v2.js", "x")]);
        let mut reached = Reached::BeforeUpload;
        let err = format!("{:#}", deploy_over_ssh(&target, &broken, &options, &mut reached, &connect).unwrap_err());
        assert!(err.contains("the health check failed"), "{err}");
        assert!(err.contains("rolled back to"), "{err}");
        assert!(err.contains("the stand works again"), "the old build passes the check: {err}");
        assert!(reached == Reached::RolledBack);
        assert_eq!(std::fs::read_to_string(site.join("index.html")).unwrap(), "v1", "the first build is back");
        assert!(!site.join("only-in-v2.js").exists(), "nothing of the failed deploy stays");
        assert_eq!(backups(&site).len(), 1, "the archive the stand was rolled back to is kept");

        let (_v3, fixed) = a_build(&[("health.txt", "ok"), ("index.html", "v3")]);
        let mut reached = Reached::BeforeUpload;
        deploy_over_ssh(&target, &fixed, &options, &mut reached, &connect).expect("a healthy deploy");
        assert_eq!(std::fs::read_to_string(site.join("index.html")).unwrap(), "v3");
        assert_eq!(backups(&site).len(), 1, "a rollback backup goes once the check passes: {:?}", backups(&site));
    }

    /// A first deploy that fails its check has nothing to go back to, and
    /// says so instead of pretending.
    #[test]
    fn a_first_deploy_that_fails_its_check_says_there_is_nothing_to_go_back_to() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let connect = || Ok(stand.session());
        let server = tempfile::tempdir().unwrap();
        let dir = server.path().join("site").display().to_string();
        let probe = if cfg!(windows) { "findstr ok health.txt" } else { "grep -q ok health.txt" };
        let check = HealthCheck {
            probe: Probe::Command(probe.into()),
            wait: Some(0),
        };
        let target = a_target(&dir, None, Some(check), "http://127.0.0.1:1");
        let (_build, broken) = a_build(&[("health.txt", "down")]);
        let options = Options {
            backup: true,
            clear: false,
            no_archive: false,
        };
        let mut reached = Reached::BeforeUpload;
        let err = format!("{:#}", deploy_over_ssh(&target, &broken, &options, &mut reached, &connect).unwrap_err());
        assert!(err.contains("nothing to roll back to"), "{err}");
        assert!(reached == Reached::Uploaded, "the files are there, and the note must say so");
    }
}
