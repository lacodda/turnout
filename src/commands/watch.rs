//! `turnout watch`: deploy an app again whenever its build changes (ADR 0039).
//!
//! A watch is a job of the app like `dev` or `build` - `myapp watch` in `ps`,
//! `logs` and `stop` - that runs in the background and owns its own record,
//! the way a tunnel does: there is no command line under it for a supervisor
//! to watch, only turnout looking at files and deploying.
//!
//! It looks rather than listens. Every half second it notes each file's size
//! and modification time, which works the same on every OS and every file
//! system - a network share, a WSL mount, a bind mount into a container -
//! where change notifications are missing or late, and asks nothing of a
//! crate that would have to be told to ignore the build's own writes. A
//! change counts once the files have stayed as they are for a moment: a
//! build writes in bursts, and half a build must never go out.
//!
//! By default it watches the artifact directory: whatever writes the build -
//! `turnout build`, a bundler's own `--watch`, an IDE - the stand gets it.
//! While `turnout build` of the app runs, the watch waits for it, and a build
//! that failed is not deployed. `--build` watches the sources instead, builds
//! on every change, and deploys only a green build.
//!
//! Each deploy says how it went on the desktop, and a failed one shows in
//! `ps` until the next one goes through.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, anyhow, bail};

use crate::commands::deploy::{self, After, Options, Shipment};
use crate::job;
use crate::notify;
use crate::process;
use crate::registry::{self, Entry, Work};
use crate::remote::{self, Overrides, Resolved};

/// The command a watch is recorded under.
pub const COMMAND: &str = "watch";

/// How often the watch looks at its files.
const POLL: Duration = if cfg!(test) { Duration::from_millis(40) } else { Duration::from_millis(500) };

/// How long the files must stay as they are before a change counts as done.
const QUIET: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_millis(1500)
};

/// How long the background watch gets to start and claim its record.
const START_TIMEOUT: Duration = Duration::from_secs(20);

/// What `turnout watch` was asked.
pub struct Spec {
    pub target: Option<String>,
    pub overrides: Overrides,
    /// Watch the sources and build before each deploy.
    pub build: bool,
    /// Stay in this terminal.
    pub foreground: bool,
    /// The log the background watch writes; marks the run as that watch.
    pub log: Option<PathBuf>,
}

pub fn run(spec: Spec) -> Result<()> {
    let target = remote::resolve(spec.target, spec.overrides)?;
    let app = &target.app;
    if app.dist_dir.is_none() {
        bail!("app '{0}' has no artifact directory - set it with `turnout app edit {0} --dist DIR`", app.name);
    }
    if spec.build && !app.commands.contains_key("build") {
        bail!(
            "'{0}' has no build command for --build to run - add one with `turnout app edit {0} --command build=CMD`",
            app.name
        );
    }
    if spec.foreground || spec.log.is_some() {
        return watch(&target, spec.build, spec.log);
    }
    in_background(&target, spec.build)
}

/// What a watch deploys on, in words: `dist changes`, `a source changes`.
fn trigger(target: &Resolved, build: bool) -> String {
    if build {
        "a change to the sources builds it, and a green build".to_string()
    } else {
        format!("a change to {}", target.app.dist_dir.as_deref().unwrap_or("the build"))
    }
}

/// Start the watch in the background and return once it watches - or with
/// its reason when it cannot.
fn in_background(target: &Resolved, build: bool) -> Result<()> {
    let app = &target.app.name;
    let key = registry::command_key(app, COMMAND);
    job::ensure_free(&key)?;
    let log = registry::log_path(&key)?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let output = std::fs::File::create(&log).with_context(|| format!("cannot write {}", log.display()))?;
    let mut args = vec![COMMAND.to_string()];
    args.extend(deploy::route_words(target, COMMAND)?);
    if build {
        args.push("--build".to_string());
    }
    let exe = std::env::current_exe().context("cannot locate the turnout binary")?;
    let mut command = Command::new(exe);
    command.args(&args).arg("--log").arg(&log);
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone().context("cannot share the watch's log")?)
        .stderr(output);
    process::detach(&mut command);
    crate::utils::stop_inheriting_stdio();
    let mut child = command.spawn().context("cannot start the watch")?;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("cannot check on the watch")? {
            return Err(why_not(&log, &status.to_string()));
        }
        if registry::load(&key)?.is_some_and(|entry| entry.pid == child.id()) {
            break;
        }
        if started.elapsed() > START_TIMEOUT {
            let _ = process::end(child.id(), None, process::Ending::Kill);
            return Err(why_not(&log, &format!("it did not start within {}s", START_TIMEOUT.as_secs())));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    crate::journal::record("watch.start", Some(app), Some(&target.server.name), None);
    println!(
        "{app} watch runs in the background (pid {}): {} deploys it to {}",
        child.id(),
        trigger(target, build),
        deploy::label(target)
    );
    println!("  output: turnout logs {app} {COMMAND} -f");
    println!("  stop:   turnout stop {app} {COMMAND}");
    Ok(())
}

/// The error of a watch that did not start: its own error line when its log
/// has one.
fn why_not(log: &Path, how: &str) -> anyhow::Error {
    let lines = crate::commands::jobs::tail(log, 20).unwrap_or_default();
    let error = lines.iter().rev().find_map(|line| line.strip_prefix("error: ")).map(str::to_string);
    match error {
        Some(error) => anyhow!("{error}"),
        None => anyhow!("the watch did not start ({how}) - see `{}`", log.display()),
    }
}

/// The watch itself, until Ctrl+C or `stop` ends it.
fn watch(target: &Resolved, build: bool, log: Option<PathBuf>) -> Result<()> {
    let app = &target.app;
    let project = crate::utils::project_dir(Path::new(&app.path))?;
    let dist = project.join(app.dist_dir.as_deref().expect("checked before"));
    let watched = if build {
        Watched::Sources {
            project: project.clone(),
            dist: dist.clone(),
            skip: PathBuf::from(app.env_file_name()),
        }
    } else {
        Watched::Build(dist.clone())
    };
    let key = registry::command_key(&app.name, COMMAND);
    let work = Work::Command {
        app: app.name.clone(),
        command: COMMAND.to_string(),
    };
    // The first look before the record: once `turnout watch` has said the
    // watch runs, a change saved the moment after is one it sees.
    let first = watched.look();
    registry::claim(&Entry::own(work, log.is_some(), log.clone()))?;
    // Ctrl+C is the watch's to handle: a deploy under way finishes first,
    // and the record goes with the watch.
    crate::term::hand_over_interrupts();
    say(&format!(
        "watching {} of {}: {} deploys it to {}",
        if build { "the sources".to_string() } else { dist.display().to_string() },
        app.name,
        trigger(target, build),
        deploy::label(target)
    ));

    let gate = |seen: u64| if build { Gate::Go } else { build_gate(&app.name, seen) };
    let mut cycle = || deploy_once(target, build, &project, log.as_deref());
    follow(&watched, first, gate, &mut cycle, crate::term::interrupted);

    let _ = registry::remove_own(&key);
    say(&format!("the watch of {} ended", app.name));
    Ok(())
}

/// What the watch looks at.
enum Watched {
    /// The artifact directory.
    Build(PathBuf),
    /// What the project is built from.
    Sources { project: PathBuf, dist: PathBuf, skip: PathBuf },
}

/// One look: every file with its size and when it was written.
type Look = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

impl Watched {
    fn look(&self) -> Look {
        match self {
            Watched::Build(dist) => walk(dist),
            Watched::Sources { project, dist, skip } => crate::artifact::sources(project, dist, &[skip.as_path()])
                .into_iter()
                .filter_map(|relative| {
                    let metadata = std::fs::metadata(project.join(&relative)).ok()?;
                    Some((relative, (metadata.len(), metadata.modified().ok())))
                })
                .collect(),
        }
    }
}

/// Every file under `dir`; nothing when it is not there - a build that
/// empties its directory before it writes.
fn walk(dir: &Path) -> Look {
    let mut look = Look::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else { continue };
            if metadata.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(dir) {
                look.insert(relative.to_path_buf(), (metadata.len(), metadata.modified().ok()));
            }
        }
    }
    look
}

/// Whether a change that has settled goes out now.
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    Go,
    /// Not yet: something is still writing.
    Wait,
    /// Not at all, for this reason; the next change is looked at afresh.
    Skip(String),
}

/// What `turnout build` of the app says about a change to its build first
/// seen at `seen` (seconds since the epoch): a build that runs is still
/// writing, and one that failed after the change began wrote it - half a
/// build, and no deploy.
fn build_gate(app: &str, seen: u64) -> Gate {
    let Ok(Some(entry)) = registry::load(&registry::command_key(app, "build")) else {
        return Gate::Go;
    };
    if entry.is_running() {
        return Gate::Wait;
    }
    match &entry.ended {
        Some(ended) if ended.code != Some(0) && ended.at + 1 >= seen => Gate::Skip(format!(
            "`turnout build` of {app} failed ({}) - its output is not deployed",
            ended.exit().describe()
        )),
        _ => Gate::Go,
    }
}

/// Look at `watched` until `stopped` says so - `first` is the look a change
/// is a change from - and run `cycle` once for each change that settled and
/// `gate` lets through.
///
/// The look after a cycle is taken afresh: what the cycle itself wrote - the
/// build of `--build` - is not a change to deploy again.
fn follow(watched: &Watched, first: Look, gate: impl Fn(u64) -> Gate, cycle: &mut dyn FnMut(), stopped: impl Fn() -> bool) {
    let mut last = first;
    // When the files last changed, and when this change began.
    let mut changed: Option<(Instant, u64)> = None;
    while !stopped() {
        std::thread::sleep(POLL);
        let now = watched.look();
        if now != last {
            last = now;
            changed = Some((Instant::now(), changed.map_or_else(registry::now, |(_, seen)| seen)));
            continue;
        }
        let Some((at, seen)) = changed else { continue };
        if at.elapsed() < QUIET || last.is_empty() {
            continue;
        }
        match gate(seen) {
            Gate::Wait => continue,
            Gate::Skip(why) => {
                say(&why);
                changed = None;
                continue;
            }
            Gate::Go => {}
        }
        changed = None;
        cycle();
        last = watched.look();
    }
}

/// One deploy of what changed, told on the desktop and in the record.
fn deploy_once(target: &Resolved, build: bool, project: &Path, log: Option<&Path>) {
    let app = &target.app.name;
    say(&format!(
        "{} changed - deploying to {}",
        if build {
            "the sources"
        } else {
            target.app.dist_dir.as_deref().unwrap_or("the build")
        },
        deploy::label(target)
    ));
    let started = Instant::now();
    let console = job::Mode::resolve(job::Mode::Quiet, false, job::on_terminal());
    let result = deploy::prepare(std::slice::from_ref(target), !build, false, console).and_then(|artifacts| {
        let shipment = Shipment {
            env: deploy::stand_env(target)?,
            ..Shipment::of(&artifacts[0])
        };
        deploy::deploy_one(target, &shipment, &Options::default(), After { open: false, offer: false })
    });
    let key = registry::command_key(app, COMMAND);
    let toast = match result {
        Ok(()) => {
            let _ = registry::update_own(&key, |entry| entry.trouble = None);
            let places = notify::Places { log, dir: project };
            notify::finished(app, "deploy", started.elapsed(), Some(&target.address), &places)
        }
        Err(err) => {
            let text = format!("{err:#}");
            eprintln!("error: {text}");
            let reason = text.lines().next().unwrap_or_default().to_string();
            let _ = registry::update_own(&key, |entry| {
                entry.trouble = Some(registry::Trouble {
                    since: registry::now(),
                    reason: format!("the last deploy failed: {reason}"),
                })
            });
            notify::watch_failed(app, &target.server.name, &reason, log)
        }
    };
    if let Err(err) = notify::show(&toast) {
        say(&format!("cannot show the notification: {err:#}"));
    }
}

/// A line in the watch's log, stamped with the moment: a watch runs for
/// days. A log that cannot be written is not worth the watch.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{} {line}", crate::output::iso8601(registry::now()));
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A watch on a directory, in a thread of its own: how many cycles ran,
    /// and the switch that ends it.
    struct Running {
        cycles: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Running {
        fn start(watched: Watched, gate: impl Fn(u64) -> Gate + Send + 'static) -> Self {
            let cycles = Arc::new(AtomicUsize::new(0));
            let stop = Arc::new(AtomicBool::new(false));
            let (counted, stopping) = (cycles.clone(), stop.clone());
            let thread = std::thread::spawn(move || {
                let mut cycle = || {
                    counted.fetch_add(1, Ordering::SeqCst);
                };
                let first = watched.look();
                follow(&watched, first, gate, &mut cycle, || stopping.load(Ordering::SeqCst));
            });
            // The first look is taken before anything below changes.
            std::thread::sleep(POLL * 3);
            Self {
                cycles,
                stop,
                thread: Some(thread),
            }
        }

        fn cycles(&self) -> usize {
            self.cycles.load(Ordering::SeqCst)
        }

        /// Wait for the watch to settle on what it was given.
        fn settle(&self) {
            std::thread::sleep(QUIET + POLL * 8);
        }

        /// Wait for the watch to have run `cycles` cycles, and to stay there.
        ///
        /// A look at the sources asks git each time, which on a loaded
        /// machine takes longer than a test should guess at.
        fn reaches(&self, cycles: usize) {
            let waited = Instant::now();
            while self.cycles() < cycles && waited.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(POLL);
            }
            self.settle();
            assert_eq!(self.cycles(), cycles);
        }
    }

    impl Drop for Running {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// A build that writes in bursts goes out once, when it is done; a quiet
    /// directory goes out never; an emptied one waits for what comes next.
    #[test]
    fn a_change_goes_out_once_it_has_settled() {
        let dist = tempfile::tempdir().unwrap();
        write(&dist.path().join("index.html"), "v1");
        let watch = Running::start(Watched::Build(dist.path().to_path_buf()), |_| Gate::Go);
        watch.settle();
        assert_eq!(watch.cycles(), 0, "nothing changed");

        for burst in 0..3 {
            write(&dist.path().join(format!("assets/chunk-{burst}.js")), "x");
            std::thread::sleep(POLL * 2);
        }
        write(&dist.path().join("index.html"), "v2 - longer");
        assert_eq!(watch.cycles(), 0, "the build is still writing");
        watch.reaches(1);

        // Emptied, as a bundler empties its directory first: nothing to send.
        std::fs::remove_dir_all(dist.path().join("assets")).unwrap();
        std::fs::remove_file(dist.path().join("index.html")).unwrap();
        watch.settle();
        assert_eq!(watch.cycles(), 1, "an empty build is not deployed");
        write(&dist.path().join("index.html"), "v3");
        watch.reaches(2);
    }

    /// A gate that waits holds the change; one that skips drops it, and the
    /// next change is looked at afresh.
    #[test]
    fn the_gate_holds_or_drops_a_change() {
        let dist = tempfile::tempdir().unwrap();
        write(&dist.path().join("index.html"), "v1");
        let answer = Arc::new(Mutex::new(Gate::Wait));
        let asked = answer.clone();
        let watch = Running::start(Watched::Build(dist.path().to_path_buf()), move |_| match &*asked.lock().unwrap() {
            Gate::Go => Gate::Go,
            Gate::Wait => Gate::Wait,
            Gate::Skip(why) => Gate::Skip(why.clone()),
        });
        write(&dist.path().join("index.html"), "v2");
        watch.settle();
        assert_eq!(watch.cycles(), 0, "the build is still running");
        *answer.lock().unwrap() = Gate::Skip("the build failed".into());
        watch.settle();
        assert_eq!(watch.cycles(), 0, "a failed build is not deployed");
        *answer.lock().unwrap() = Gate::Go;
        watch.settle();
        assert_eq!(watch.cycles(), 0, "the change was dropped, not kept for later");
        write(&dist.path().join("index.html"), "v3");
        watch.reaches(1);
    }

    /// The sources a git repository tracks or would track: an ignored file
    /// and the build's own directory are not sources, and writing them does
    /// not deploy.
    #[test]
    fn only_a_source_wakes_a_watch_on_the_sources() {
        let project = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(project.path())
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            return; // No git here: the walk is covered by the artifact tests.
        }
        write(&project.path().join(".gitignore"), "node_modules/\n");
        write(&project.path().join("src/main.ts"), "v1");
        let watched = Watched::Sources {
            project: project.path().to_path_buf(),
            dist: project.path().join("dist"),
            skip: PathBuf::from(".env.development.local"),
        };
        let watch = Running::start(watched, |_| Gate::Go);
        write(&project.path().join("dist/index.html"), "built");
        write(&project.path().join("node_modules/x/index.js"), "installed");
        write(&project.path().join(".env.development.local"), "TURNOUT_GATEWAY_URL=x");
        watch.settle();
        assert_eq!(watch.cycles(), 0, "none of that is a source");
        write(&project.path().join("src/main.ts"), "v2 - edited");
        watch.reaches(1);
    }
}
