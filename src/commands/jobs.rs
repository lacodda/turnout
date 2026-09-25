//! Jobs in the background, and the console over all of them: `--detach`,
//! `ps`, `logs` and `stop`.
//!
//! A detached job is run by a *supervisor*: turnout started again, detached
//! from the terminal, as `turnout job-run`. It claims the job's record, runs
//! the command exactly the way a foreground run would - captured, logged,
//! watched for the server coming up - and writes down how it ended. Spawning
//! the command itself detached would be simpler and would know nothing: not
//! whether the server came up, not the exit code, and on Windows not even
//! how to take its whole process tree down again.

use std::io::{BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::job::{self, Mode, Program};
use crate::notify;
use crate::process::{self, Ending};
use crate::progress::{human_duration, human_span};
use crate::registry::{self, Entry, Status, Work};
use crate::store;

/// How long the supervisor gets to claim its record before the start is
/// called failed. It only has to start and write one small file.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(10);

/// How long `--detach` keeps watching after the job started, to catch the one
/// that fails at once.
///
/// A typo in a command line, a missing `node_modules`: those die within a
/// fraction of a second, and a `--detach` that answered "running in the
/// background" to them would send the user to `ps` to find out it never ran.
/// Longer than that and the console is no longer "free at once".
const GRACE: Duration = Duration::from_millis(700);

/// How long `stop` waits for a job to go before it stops asking.
const STOP_PATIENCE: Duration = Duration::from_secs(5);

/// How many lines a job that failed at once replays.
const TAIL_LINES: usize = 40;

/// Sends commands to the background without `--detach`: `all`, or their names.
pub const PREFER_ENV: &str = "TURNOUT_DETACH";

/// The commands turnout runs under these names whatever the apps call theirs.
pub const BUILT_IN: [&str; 5] = ["dev", "build", "test", "lint", "deploy"];

/// Where a job runs, and why there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// In this terminal.
    Here,
    /// In the background, because `--detach` said so.
    Asked,
    /// In the background, because [`PREFER_ENV`] covers the command.
    Preferred,
}

impl Placement {
    pub fn detached(self) -> bool {
        self != Placement::Here
    }
}

/// Where a job runs.
///
/// `--detach` decides when given, and `--foreground` and `-v` keep the job
/// here - streaming its output in full means being where it streams to.
/// Otherwise the preference does, but only where the console would have been
/// quiet anyway: on a terminal. A pipe, a CI job and a script reading the output
/// get the job in line as they always did, because a preference for the
/// background must not turn `turnout build && turnout deploy` in a script into
/// a race between the two. `is_command` says whether any app has a command by
/// that name, so a misspelt preference is reported instead of never matching.
pub fn placement(command: &str, flags: crate::cli::Console, console: Mode, is_command: &dyn Fn(&str) -> bool) -> Placement {
    if flags.detach {
        return Placement::Asked;
    }
    if flags.foreground || flags.verbose || console == Mode::Stream {
        return Placement::Here;
    }
    let preference = Preference::parse(&std::env::var(PREFER_ENV).unwrap_or_default());
    for name in preference.unknown(is_command) {
        crate::progress::warn(&format!(
            "{PREFER_ENV} names '{name}', which is not a command turnout runs or any app has - it is ignored"
        ));
    }
    if preference.covers(command) { Placement::Preferred } else { Placement::Here }
}

/// What [`PREFER_ENV`] asks for.
#[derive(Debug, PartialEq, Eq)]
struct Preference {
    all: bool,
    names: Vec<String>,
}

impl Preference {
    /// `all` (or `1`, `true`, `yes`, `on`) for every command; nothing for an
    /// empty value or `0`, `false`, `no`, `off`, `none` - the words
    /// `TURNOUT_UPDATE_CHECK` takes; otherwise command names, separated by
    /// commas or spaces. Names keep their case: npm scripts are case-sensitive.
    fn parse(value: &str) -> Self {
        let words: Vec<&str> = value.split(|c: char| c == ',' || c.is_whitespace()).filter(|word| !word.is_empty()).collect();
        let is = |word: &str, set: &[&str]| set.contains(&word.to_ascii_lowercase().as_str());
        if let [word] = words.as_slice() {
            if is(word, &["all", "1", "true", "yes", "on"]) {
                return Self { all: true, names: Vec::new() };
            }
            if is(word, &["0", "false", "no", "off", "none"]) {
                return Self { all: false, names: Vec::new() };
            }
        }
        Self {
            all: words.iter().any(|word| word.eq_ignore_ascii_case("all")),
            names: words
                .iter()
                .filter(|word| !word.eq_ignore_ascii_case("all"))
                .map(|word| word.to_string())
                .collect(),
        }
    }

    fn covers(&self, command: &str) -> bool {
        self.all || self.names.iter().any(|name| name == command)
    }

    fn unknown<'a>(&'a self, is_command: &'a dyn Fn(&str) -> bool) -> impl Iterator<Item = &'a str> {
        self.names
            .iter()
            .map(String::as_str)
            .filter(move |name| !BUILT_IN.contains(name) && !is_command(name))
    }
}

/// A job to hand to a supervisor.
pub struct Detach<'a> {
    pub app: &'a str,
    pub command: &'a str,
    pub dir: &'a Path,
    pub env: &'a [(&'a str, String)],
    pub label: &'a str,
    /// Watch for the server to come up.
    pub ready: bool,
    /// The address to report when the server names none.
    pub own: Option<String>,
    /// Open the front door once it is up.
    pub open: bool,
    /// `program` is turnout's own arguments rather than a command line.
    pub itself: bool,
    pub program: Vec<String>,
    /// Where the job's result can be seen once it succeeded: the stand a
    /// deploy went to. The notification leads there.
    pub link: Option<String>,
    /// Sent to the background by the preference rather than by `--detach`,
    /// which the message has to say - nobody typed the flag.
    pub preferred: bool,
}

/// Start a job in the background and return once it is running.
pub fn detach(spec: Detach<'_>) -> Result<()> {
    let key = registry::command_key(spec.app, spec.command);
    job::ensure_free(&key)?;
    let exe = std::env::current_exe().context("cannot locate the turnout binary")?;
    let mut command = Command::new(exe);
    command
        .args(["job-run", "--app", spec.app, "--command", spec.command, "--dir"])
        .arg(spec.dir)
        .args(["--label", spec.label]);
    if spec.ready {
        command.arg("--ready");
    }
    if let Some(own) = &spec.own {
        command.args(["--own", own]);
    }
    if spec.open {
        command.arg("--open");
    }
    if spec.itself {
        command.arg("--self");
    }
    if let Some(link) = &spec.link {
        command.args(["--link", link]);
    }
    command.arg("--").args(&spec.program);
    // Set on the supervisor, inherited by the job: the same variables a
    // foreground run hands over.
    for (name, value) in spec.env {
        command.env(name, value);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    process::detach(&mut command);
    crate::utils::stop_inheriting_stdio();
    let mut child = command.spawn().context("cannot start the background job")?;

    // Not the spawn: the record. Until the supervisor has claimed it, nothing
    // is running that `ps` or `stop` could find.
    let started = Instant::now();
    loop {
        if registry::load(&key)?.is_some_and(|entry| entry.pid == child.id()) {
            break;
        }
        if let Some(status) = child.try_wait().context("cannot check on the background job")? {
            bail!("the background job exited before it started ({status}) - run it without --detach to see why");
        }
        if started.elapsed() > CLAIM_TIMEOUT {
            let _ = process::end(child.id(), None, Ending::Kill);
            bail!("the background job did not start within {}s", CLAIM_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let title = format!("{} {}", spec.app, spec.command);
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        let Some(entry) = registry::load(&key)?.filter(|entry| entry.pid == child.id()) else {
            break;
        };
        let Some(ended) = entry.ended else { continue };
        if ended.code == Some(0) {
            println!("{title} finished at once - see its output with `turnout logs {title}`");
            return Ok(());
        }
        // The output is what a failure is for; the whole point of watching.
        // Read from the copy kept aside: it is the one that will still say
        // this after the next run.
        if let Some(log) = &entry.log {
            let kept = registry::failed_log(log);
            let log = if kept.is_file() { &kept } else { log };
            let mut stderr = std::io::stderr().lock();
            for line in tail(log, TAIL_LINES)? {
                let _ = writeln!(stderr, "{line}");
            }
            let _ = writeln!(stderr, "full output: {}", log.display());
        }
        eprintln!("error: {title} failed at once ({})", describe_code(ended.code));
        std::process::exit(ended.code.filter(|code| *code != 0).unwrap_or(1));
    }

    crate::journal::record("job.detach", Some(spec.app), None, Some(spec.command));
    if spec.preferred {
        println!(
            "{title} runs in the background (pid {}) - {PREFER_ENV} covers '{}'; --foreground keeps it here",
            child.id(),
            spec.command
        );
    } else {
        println!("{title} runs in the background (pid {})", child.id());
    }
    println!("  output: turnout logs {title} -f");
    println!("  stop:   turnout stop {title}");
    Ok(())
}

/// What the supervisor is told to run.
pub struct Supervised {
    pub app: String,
    pub command: String,
    pub dir: PathBuf,
    pub label: String,
    pub ready: bool,
    pub own: Option<String>,
    pub open: bool,
    pub itself: bool,
    pub program: Vec<String>,
    pub link: Option<String>,
}

/// The supervisor: `turnout job-run`, what [`detach`] spawns.
///
/// Says how the job went the one way a background job can: a notification
/// when a server comes up, and one when the job ends by itself - it finished,
/// it failed, it could not start at all. A job ended by `stop` says nothing:
/// the supervisor goes down with it, and the person who stopped it knows.
///
/// Its own troubles go into the job's log, since it has no stderr anybody
/// reads. Exits with the job's exit code - nobody reads that either, but a
/// supervisor that always said 0 would be one more place a failure goes quiet.
pub fn supervise(spec: Supervised) -> Result<()> {
    let mut job = job::Job::claim(&spec.app, &spec.command, true)?;
    let log = job.log_path().map(Path::to_path_buf);
    // The door as it is now: a gateway started later is picked up by `ps`,
    // which works the address out again when it shows it.
    let front_door = registry::gateway()?
        .and_then(|gateway| gateway.front_port)
        .map(|front| crate::front::address(&spec.app, front));
    // Filled by the ready callback, which runs while the job holds the log.
    let troubles: Arc<Mutex<Vec<String>>> = Arc::default();
    let was_ready = Arc::new(AtomicBool::new(false));
    let ready = spec.ready.then(|| {
        let open = spec.open.then(|| crate::commands::exec::opener(front_door.clone()));
        let (app, command, dir, log) = (spec.app.clone(), spec.command.clone(), spec.dir.clone(), log.clone());
        let (troubles, was_ready) = (Arc::clone(&troubles), Arc::clone(&was_ready));
        let on_ready: job::OnReady = Box::new(move |address, elapsed| {
            was_ready.store(true, Ordering::SeqCst);
            if let Some(open) = open {
                open(address.clone(), elapsed);
            }
            let places = notify::Places {
                log: log.as_deref(),
                dir: &dir,
            };
            if let Err(err) = notify::show(&notify::ready(&app, &command, address.as_deref(), elapsed, &places)) {
                troubles
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(format!("cannot show the notification: {err:#}"));
            }
        });
        job::Ready {
            app: &spec.app,
            door: front_door.clone(),
            own: spec.own.clone(),
            patience: crate::commands::exec::ready_patience(),
            on_ready: Some(on_ready),
        }
    });
    let line = spec.program.join(" ");
    let program = if spec.itself {
        Program::Turnout(&spec.program)
    } else {
        Program::Shell(&line)
    };
    let result = job::run(program, &spec.dir, &[], Mode::Log, &spec.label, &mut job, ready);
    for trouble in troubles.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).drain(..) {
        job.note(&trouble);
    }
    let (toast, code) = match result {
        Ok(outcome) => {
            let places = notify::Places {
                log: outcome.kept.as_deref().or(log.as_deref()),
                dir: &spec.dir,
            };
            let toast = if !outcome.failed() {
                ended_well(&spec, outcome.elapsed, was_ready.load(Ordering::SeqCst), &places)
            } else {
                let how = format!("{} after {}", describe_code(outcome.status.code()), human_duration(outcome.elapsed));
                notify::failed(&spec.app, &spec.command, &how, notify::reason(&outcome.tail).as_deref(), &places)
            };
            (toast, outcome.status.code().unwrap_or(1))
        }
        // The job never ran: the reason is turnout's own, and it goes where
        // the job's output would have been.
        Err(err) => {
            job.note(&format!("{err:#}"));
            let kept = job.keep();
            let places = notify::Places {
                log: kept.as_deref().or(log.as_deref()),
                dir: &spec.dir,
            };
            (notify::failed(&spec.app, &spec.command, "did not start", Some(&format!("{err:#}")), &places), 1)
        }
    };
    if let Err(err) = notify::show(&toast) {
        job.note(&format!("cannot show the notification: {err:#}"));
    }
    notify::settle();
    std::process::exit(code);
}

/// The notification for a job that ended with exit code 0.
///
/// A server that had come up and then ended by itself has *stopped* - the
/// news is that it no longer answers; anything else has *finished*.
fn ended_well(spec: &Supervised, elapsed: Duration, was_ready: bool, places: &notify::Places<'_>) -> notify::Toast {
    if was_ready {
        notify::stopped(&spec.app, &spec.command, &human_span(elapsed.as_secs()), places)
    } else {
        notify::finished(&spec.app, &spec.command, elapsed, spec.link.as_deref(), places)
    }
}

/// `turnout ps`.
pub fn ps(watch: bool) -> Result<()> {
    if !watch {
        print!("{}", table()?);
        return Ok(());
    }
    if !std::io::stdout().is_terminal() {
        bail!("--watch redraws a table on a terminal - without one, run `turnout ps` whenever you want a look");
    }
    let term = console::Term::stdout();
    loop {
        let frame = table()?;
        let _ = term.clear_screen();
        print!("{frame}");
        println!("\nevery second - Ctrl+C to quit");
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// The `ps` table, as text.
fn table() -> Result<String> {
    let mut entries = registry::list()?;
    if entries.is_empty() {
        return Ok("No jobs. Start one in the background with `turnout dev --detach`.\n".to_string());
    }
    // Running first - that is the question `ps` answers - then by name.
    entries.sort_by_key(|entry| (!entry.is_running(), entry.title()));
    let apps = store::load_apps()?;
    let gateway = registry::gateway()?;
    let now = registry::now();
    let mut rows = vec![["JOB", "STATE", "PID", "PORT", "ADDRESS", "TIME"].map(str::to_string)];
    for entry in &entries {
        let status = entry.status();
        let (port, address) = if status == Status::Running {
            reach(entry, &apps, gateway.as_ref())
        } else {
            (None, None)
        };
        let time = match (&entry.ended, status) {
            (_, Status::Running) => human_span(now.saturating_sub(entry.started)),
            (Some(ended), _) => format!("{} ago", human_span(now.saturating_sub(ended.at))),
            (None, _) => "-".to_string(),
        };
        let mut state = state_of(entry, status);
        if entry.detached && status == Status::Running {
            state.push_str(" (bg)");
        }
        rows.push([
            entry.title(),
            state,
            if status == Status::Running { entry.pid.to_string() } else { "-".to_string() },
            port.map_or_else(|| "-".to_string(), |port| port.to_string()),
            address.unwrap_or_else(|| "-".to_string()),
            time,
        ]);
    }
    let widths: Vec<usize> = (0..6)
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[column]));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// What the STATE column says.
fn state_of(entry: &Entry, status: Status) -> String {
    match status {
        Status::Running if entry.ready.is_some() => "ready".to_string(),
        Status::Running => "running".to_string(),
        Status::Exited(Some(0)) => "done".to_string(),
        Status::Exited(code) => format!("failed ({})", describe_code(code)),
        Status::Gone => "gone".to_string(),
    }
}

fn describe_code(code: Option<i32>) -> String {
    code.map_or_else(|| "killed".to_string(), |code| format!("exit {code}"))
}

/// Where a running job can be reached: its port and the address to use.
///
/// Worked out when shown rather than stored: the door opens and closes with
/// the gateway, and a dev server started before the gateway is still reachable
/// through the door once it opens.
fn reach(entry: &Entry, apps: &[crate::model::App], gateway: Option<&crate::model::Gateway>) -> (Option<u16>, Option<String>) {
    match &entry.work {
        Work::Gateway { ports, front_port } => (front_port.or_else(|| ports.keys().next().copied()), front_port.map(crate::front::door)),
        Work::Command { app, command } => {
            let seen = entry.ready.as_ref().and_then(|ready| ready.url.clone());
            if command == "dev"
                && let Some(port) = apps.iter().find(|a| &a.name == app).and_then(|a| a.dev_port)
            {
                let door = gateway.and_then(|gateway| gateway.front_port).map(|front| crate::front::address(app, front));
                return (Some(port), door.or(seen).or_else(|| Some(format!("http://localhost:{port}"))));
            }
            (seen.as_deref().and_then(port_of), seen)
        }
    }
}

/// The port an address names, if it names one.
fn port_of(url: &str) -> Option<u16> {
    let authority = url.split("://").nth(1)?.split('/').next()?;
    authority.rsplit_once(':')?.1.parse().ok()
}

/// The records a name (and a command) points at.
///
/// `gateway` alone is the gateway, even when an app has that name too - its
/// jobs are reached by naming the command as well: `turnout logs gateway dev`.
/// No name is the app of the current directory.
fn select(name: Option<String>, command: Option<String>) -> Result<(String, Vec<Entry>)> {
    if name.as_deref() == Some(registry::GATEWAY) && command.is_none() {
        let entries = registry::load(registry::GATEWAY)?.into_iter().collect();
        return Ok((registry::GATEWAY.to_string(), entries));
    }
    let app = match name {
        Some(name) => name,
        None => {
            let apps = store::load_apps()?;
            match crate::commands::exec::app_here(&apps)? {
                Some(app) => app.name.clone(),
                None => bail!("not inside a known app directory - name the job, see `turnout ps`"),
            }
        }
    };
    let entries: Vec<Entry> = match &command {
        Some(command) => registry::load(&registry::command_key(&app, command))?.into_iter().collect(),
        None => registry::list()?.into_iter().filter(|entry| entry.app() == Some(app.as_str())).collect(),
    };
    Ok((command.map_or_else(|| app.clone(), |command| format!("{app} {command}")), entries))
}

/// `turnout logs`.
///
/// `failed` reads the copy the last failure left aside instead of the latest
/// run's log - the output a notification about a failure pointed at, still
/// there after the job ran again.
pub fn logs(name: Option<String>, command: Option<String>, follow: bool, lines: Option<usize>, failed: bool) -> Result<()> {
    let (what, entries) = select(name, command)?;
    if failed {
        return failed_log(&what, &entries, lines);
    }
    // The running job, when there is one - that is whose output is wanted -
    // otherwise the one that ran last.
    let Some(entry) = entries.into_iter().max_by_key(|entry| (entry.is_running(), entry.started)) else {
        bail!("{what} has not run yet - see `turnout ps`");
    };
    let Some(log) = entry.log.clone() else {
        bail!("{} writes to the terminal it runs in and keeps no log", entry.title());
    };
    let mut stdout = std::io::stdout().lock();
    let mut position = match lines {
        Some(count) => {
            for line in tail(&log, count)? {
                writeln!(stdout, "{line}")?;
            }
            std::fs::metadata(&log).map(|meta| meta.len()).unwrap_or(0)
        }
        None => copy_from(&log, 0, &mut stdout)?,
    };
    if !follow {
        return Ok(());
    }
    // Until the job ends: a follow that outlived its job would be a second
    // Ctrl+C for nothing. The last read after it ends catches what it wrote
    // on the way out.
    loop {
        let current = registry::load(&entry.key())?;
        let alive = current.as_ref().is_some_and(|now| now.pid == entry.pid && now.is_running());
        // A new run of the same job truncates the file: start over with it.
        if std::fs::metadata(&log).is_ok_and(|meta| meta.len() < position) {
            position = 0;
        }
        position = copy_from(&log, position, &mut stdout)?;
        stdout.flush()?;
        if !alive {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// `turnout logs --failed`: the output of the last failure.
///
/// Of the jobs a name selects, the one whose failure is the latest - that is
/// the one somebody who just saw a failure means.
fn failed_log(what: &str, entries: &[Entry], lines: Option<usize>) -> Result<()> {
    let latest = entries
        .iter()
        .filter_map(|entry| entry.log.as_deref().map(registry::failed_log))
        .filter_map(|kept| std::fs::metadata(&kept).and_then(|meta| meta.modified()).ok().map(|at| (at, kept)))
        .max_by_key(|(at, _)| *at);
    let Some((_, kept)) = latest else {
        bail!("{what} has no failure on record - a failed run keeps its log in failed/ beside the others");
    };
    let mut stdout = std::io::stdout().lock();
    match lines {
        Some(count) => {
            for line in tail(&kept, count)? {
                writeln!(stdout, "{line}")?;
            }
        }
        None => {
            copy_from(&kept, 0, &mut stdout)?;
        }
    }
    Ok(())
}

/// Copy a file from `position` to the end; returns the new position.
///
/// Only whole lines: a line the job is halfway through writing waits for the
/// next round rather than being printed in two pieces.
fn copy_from(path: &Path, position: u64, out: &mut impl Write) -> Result<u64> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(position),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    file.seek(SeekFrom::Start(position))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let whole = buffer.iter().rposition(|byte| *byte == b'\n').map_or(0, |at| at + 1);
    out.write_all(&buffer[..whole])?;
    Ok(position + whole as u64)
}

/// The last `count` lines of a file.
fn tail(path: &Path, count: usize) -> Result<Vec<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut lines = std::collections::VecDeque::with_capacity(count);
    for line in BufReader::new(file).split(b'\n') {
        let line = line?;
        if lines.len() == count {
            lines.pop_front();
        }
        if count > 0 {
            lines.push_back(String::from_utf8_lossy(&line).trim_end_matches('\r').to_string());
        }
    }
    Ok(lines.into())
}

/// `turnout stop`, and `turnout gateway stop`.
pub fn stop(name: Option<String>, command: Option<String>) -> Result<()> {
    let (what, entries) = select(name, command)?;
    let is_gateway = what == registry::GATEWAY;
    let mut acted = false;
    for entry in entries {
        match entry.status() {
            Status::Running => {
                end_job(&entry)?;
                registry::remove(&entry.key())?;
                if is_gateway {
                    crate::journal::record("gateway.stop", None, None, None);
                    println!("Gateway stopped (pid {}).", entry.pid);
                } else {
                    let command = match &entry.work {
                        Work::Command { command, .. } => Some(command.as_str()),
                        Work::Gateway { .. } => None,
                    };
                    crate::journal::record("job.stop", entry.app(), None, command);
                    println!("Stopped {} (pid {}).", entry.title(), entry.pid);
                }
                acted = true;
            }
            // Killed from outside, crashed, or the machine rebooted: the record
            // is all that is left, and it is stale rather than an error.
            Status::Gone => {
                registry::remove(&entry.key())?;
                if is_gateway {
                    println!("The gateway (pid {}) was no longer running - cleared the stale record.", entry.pid);
                } else {
                    println!("{} (pid {}) was no longer running - cleared its record.", entry.title(), entry.pid);
                }
                acted = true;
            }
            // Ended by itself: its record is what `ps` shows about the last
            // run, and it goes when the next run takes the slot.
            Status::Exited(_) => {}
        }
    }
    if !acted {
        if is_gateway {
            println!("The gateway is not running.");
        } else {
            println!("Nothing of {what} is running.");
        }
    }
    Ok(())
}

/// End a running job and wait for it to go.
fn end_job(entry: &Entry) -> Result<()> {
    // A job in somebody's terminal is interrupted the way Ctrl+C there would,
    // so the turnout in that terminal gets to put it back; a detached one
    // has no terminal to put back and is terminated.
    let how = if entry.detached { Ending::Terminate } else { Ending::Interrupt };
    if cfg!(unix) && entry.group.is_none() && matches!(entry.work, Work::Command { .. }) {
        // turnout did not lead its process group, so the group is somebody
        // else's - a script's, a pipeline's - and signalling turnout alone
        // would leave the job's own process running without it.
        bail!(
            "{} runs in a terminal turnout does not own (pid {}) - stop it there with Ctrl+C",
            entry.title(),
            entry.pid
        );
    }
    signal(entry, how)?;
    let started = Instant::now();
    while process::is_alive(entry.pid, entry.birth) {
        if started.elapsed() > STOP_PATIENCE {
            // It was asked; it did not listen.
            signal(entry, Ending::Kill)?;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Signal a job, judging the outcome by the job rather than by the tool.
///
/// `taskkill /T` reports a failure whenever some process in the tree is gone
/// before it gets there - and in a job's tree that is the normal case: the
/// supervisor dies first, its job object takes the children with it, and
/// taskkill then fails to find them. Whether the stop worked is whether the
/// recorded process is still alive, so that is what is asked before an error
/// is believed.
fn signal(entry: &Entry, how: Ending) -> Result<()> {
    let Err(err) = process::end(entry.pid, entry.group, how) else {
        return Ok(());
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if !process::is_alive(entry.pid, entry.birth) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_reads_at_a_glance() {
        assert_eq!(human_span(0), "0s");
        assert_eq!(human_span(59), "59s");
        assert_eq!(human_span(60), "1m");
        assert_eq!(human_span(3599), "59m");
        assert_eq!(human_span(3600), "1h 00m");
        assert_eq!(human_span(3600 * 5 + 60 * 7), "5h 07m");
        assert_eq!(human_span(86400 * 2 + 3600 * 3), "2d 03h");
    }

    /// The preference reads the words `TURNOUT_UPDATE_CHECK` reads for all or
    /// nothing, and command names otherwise - with their case, because npm
    /// scripts have one.
    #[test]
    fn the_preference_reads_all_nothing_or_names() {
        let names = |value: &str| Preference::parse(value);
        for all in ["all", "ALL", "1", "true", "yes", "on", " all "] {
            assert!(names(all).all, "{all}");
            assert!(names(all).covers("dev") && names(all).covers("storybook"), "{all}");
        }
        for none in ["", "  ", "0", "false", "no", "off", "none"] {
            assert!(!names(none).covers("dev") && !names(none).covers("build"), "{none:?}");
        }
        let some = names("build, deploy storybook");
        assert!(some.covers("build") && some.covers("deploy") && some.covers("storybook"));
        assert!(!some.covers("dev"));
        assert!(!names("Build").covers("build"), "names keep their case");
        // `all` among names is still all.
        assert!(names("dev,all").covers("lint"));
    }

    /// A name no app has is reported, so a typo does not silently match
    /// nothing; the built-in commands are always known.
    #[test]
    fn a_misspelt_preference_is_reported() {
        let preference = Preference::parse("biuld,deploy,storybook,dev");
        let has = |name: &str| name == "storybook";
        assert_eq!(preference.unknown(&has).collect::<Vec<_>>(), ["biuld"]);
    }

    /// `--detach` and `--foreground` decide; the preference only fills in on
    /// a quiet console, never in front of a pipe or `-v`.
    ///
    /// Reads the environment, so the preference itself is not set here: the
    /// integration suite exercises it in a child process.
    #[test]
    fn the_flags_decide_before_the_preference() {
        use crate::cli::Console;
        let any = |_: &str| false;
        let detach = Console {
            detach: true,
            ..Console::default()
        };
        let foreground = Console {
            foreground: true,
            ..Console::default()
        };
        let verbose = Console {
            verbose: true,
            ..Console::default()
        };
        assert_eq!(placement("build", detach, Mode::Stream, &any), Placement::Asked);
        assert_eq!(placement("build", foreground, Mode::Quiet, &any), Placement::Here);
        // Forced quiet (`TURNOUT_CONSOLE`) still yields to `-v`.
        assert_eq!(placement("build", verbose, Mode::Quiet, &any), Placement::Here);
        assert_eq!(placement("build", Console::default(), Mode::Stream, &any), Placement::Here);
    }

    #[test]
    fn the_port_comes_out_of_an_address() {
        assert_eq!(port_of("http://localhost:5100/"), Some(5100));
        assert_eq!(port_of("http://127.0.0.1:3000"), Some(3000));
        assert_eq!(port_of("http://myapp.localhost"), None);
        assert_eq!(port_of("not a url"), None);
    }

    /// The tail keeps the end of the file, no more lines than asked, and
    /// drops the carriage return a Windows tool leaves on every line.
    #[test]
    fn the_tail_is_the_end_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        std::fs::write(&path, "one\r\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(tail(&path, 2).unwrap(), ["three", "four"]);
        assert_eq!(tail(&path, 10).unwrap(), ["one", "two", "three", "four"]);
        assert!(tail(&path, 0).unwrap().is_empty());
        assert!(tail(&dir.path().join("missing.log"), 3).unwrap().is_empty());
    }

    /// Following copies whole lines only, and picks up where it left off.
    #[test]
    fn following_copies_whole_lines_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("job.log");
        std::fs::write(&path, "first\nhalf").unwrap();
        let mut out = Vec::new();
        let position = copy_from(&path, 0, &mut out).unwrap();
        assert_eq!(out, b"first\n");
        std::fs::write(&path, "first\nhalf done\n").unwrap();
        let position = copy_from(&path, position, &mut out).unwrap();
        assert_eq!(out, b"first\nhalf done\n");
        assert_eq!(position, 16);
    }
}
