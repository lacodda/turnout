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
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::job::{self, Mode, Program};
use crate::process::{self, Ending};
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
        if let Some(log) = &entry.log {
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
    println!("{title} runs in the background (pid {})", child.id());
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
}

/// The supervisor: `turnout job-run`, what [`detach`] spawns.
///
/// Exits with the job's exit code - nobody reads it, but a supervisor that
/// always said 0 would be one more place where a failure goes quiet.
pub fn supervise(spec: Supervised) -> Result<()> {
    let mut job = job::Job::claim(&spec.app, &spec.command, true)?;
    // The door as it is now: a gateway started later is picked up by `ps`,
    // which works the address out again when it shows it.
    let front_door = registry::gateway()?
        .and_then(|gateway| gateway.front_port)
        .map(|front| crate::front::address(&spec.app, front));
    let ready = spec.ready.then(|| job::Ready {
        app: &spec.app,
        door: front_door.clone(),
        own: spec.own.clone(),
        patience: crate::commands::exec::ready_patience(),
        on_ready: spec.open.then(|| crate::commands::exec::opener(front_door.clone())),
    });
    let line = spec.program.join(" ");
    let program = if spec.itself {
        Program::Turnout(&spec.program)
    } else {
        Program::Shell(&line)
    };
    let outcome = job::run(program, &spec.dir, &[], Mode::Log, &spec.label, &mut job, ready)?;
    std::process::exit(outcome.status.code().unwrap_or(1));
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
            (_, Status::Running) => uptime(now.saturating_sub(entry.started)),
            (Some(ended), _) => format!("{} ago", uptime(now.saturating_sub(ended.at))),
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

/// A span of seconds the way a person reads it at a glance.
fn uptime(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h {:02}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d {:02}h", seconds / 86400, seconds % 86400 / 3600),
    }
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
pub fn logs(name: Option<String>, command: Option<String>, follow: bool, lines: Option<usize>) -> Result<()> {
    let (what, entries) = select(name, command)?;
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
        assert_eq!(uptime(0), "0s");
        assert_eq!(uptime(59), "59s");
        assert_eq!(uptime(60), "1m");
        assert_eq!(uptime(3599), "59m");
        assert_eq!(uptime(3600), "1h 00m");
        assert_eq!(uptime(3600 * 5 + 60 * 7), "5h 07m");
        assert_eq!(uptime(86400 * 2 + 3600 * 3), "2d 03h");
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
