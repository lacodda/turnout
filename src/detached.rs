//! A command on the server that outlives the connection that started it.
//!
//! A path's post-write command is often the longest part of a deploy:
//! `docker compose up -d --build` rebuilds an image for minutes. Run as a
//! plain SSH exec, it belongs to the session, and when the session goes - a
//! laptop lid, a Wi-Fi hop, Ctrl+C - the command goes with it. The field
//! report from lyrid: the upload landed, the rebuild was killed half-way, and
//! the stand kept serving the old image off new sources. `nohup` did not save
//! it: compose's children share the session, and the hangup reached them
//! anyway. `setsid` gives the command a session of its own, which the SSH one
//! ending does not touch.
//!
//! So on a POSIX server the command starts detached, writes its output and
//! then its exit code to files under `~/.turnout/runs`, and turnout reads them
//! back until the code appears. The connection is only a window onto the run:
//! when it drops, turnout dials again and reads on; when it cannot, the run
//! carries on without it and the error says where its output is.
//!
//! A Windows server runs the command attached, as before: OpenSSH for Windows
//! ends the session's whole job object on disconnect, and leaving that takes a
//! WMI process launch - its own piece of work, for the day a Windows stand
//! runs something long.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::shell::Dialect;
use crate::ssh::Session;

/// Where runs keep their output and exit code, under the remote home: the one
/// directory an account can always write to. The deploy directory may be
/// served to the web, and its parent is usually root's.
const RUNS: &str = ".turnout/runs";

/// How long turnout keeps dialling a server that dropped mid-run. A test
/// cannot wait a minute to watch it give up.
const REDIAL_FOR: Duration = if cfg!(test) { Duration::from_millis(300) } else { Duration::from_secs(60) };
const REDIAL_EVERY: Duration = if cfg!(test) { Duration::from_millis(50) } else { Duration::from_secs(3) };

/// What runs a remote command: a live session, or in tests one that can be
/// made to drop.
pub trait Remote {
    fn exec_bytes(&self, command: &str) -> Result<Vec<u8>>;
}

impl Remote for Session {
    fn exec_bytes(&self, command: &str) -> Result<Vec<u8>> {
        Session::exec_bytes(self, command)
    }
}

/// A run started on the server, named so that its files can be found again
/// from any connection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Run {
    id: String,
}

impl Run {
    /// A name no other run of this or another turnout takes: the time, this
    /// process, and a count within it - a deploy that rolls back starts two.
    fn new() -> Self {
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let stamp = crate::remote::backup_name();
        let stamp = stamp.trim_end_matches(".tar.gz");
        Self {
            id: format!("{stamp}-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed)),
        }
    }

    /// The run's files without their extension, as the shell spells it.
    fn base(&self) -> String {
        format!("\"$HOME/{RUNS}/{}\"", self.id)
    }

    /// The log, as a person types it.
    fn log(&self) -> String {
        format!("~/{RUNS}/{}.log", self.id)
    }
}

/// The command that starts `command` in `dir`, detached from this session.
///
/// The outer script is plain `sh`, whatever the login shell is. The command
/// itself runs in the login shell (`$SHELL`), as it did when sshd ran it
/// directly - a `source` or a `[[` in it must keep working. Its exit code is
/// written to a side file and renamed into place, so a reader never sees half
/// a number. Without `setsid` (macOS has none) `nohup` is the honest fallback:
/// it keeps the command from the hangup, if not from its session.
///
/// The log and the file that keeps the run's process id are created here, in
/// the foreground, before anything starts: up to v0.26 the log was opened by
/// the background job itself, so a run that could not create it never
/// started - and `launched` was printed all the same, and turnout waited for
/// an exit code that could not come. The process id is what lets a poll tell
/// a run still going from one that is gone (see [`poll_command`]); the run's
/// id opens the script, so that the process can be told apart from another
/// that took its number later, in the first words `ps` shows of it.
fn launch_command(run: &Run, dir: &str, command: &str) -> String {
    let posix = Dialect::Posix;
    let inner = format!(
        ": {id}; \"${{SHELL:-sh}}\" -c {}; c=$?; echo $c > \"$1.part\" && mv \"$1.part\" \"$1.code\"",
        posix.quote(command),
        id = run.id,
    );
    let script = format!(
        "mkdir -p \"$HOME/{RUNS}\" || exit 1; f={base}; cd {dir} || exit 1; \
         : > \"$f.log\" || exit 1; : > \"$f.pid\" || exit 1; \
         if command -v setsid >/dev/null 2>&1; then d=setsid; else d=nohup; fi; \
         $d sh -c {inner} turnout-run \"$f\" >> \"$f.log\" 2>&1 < /dev/null & \
         echo $! > \"$f.pid\" || exit 1; echo launched",
        base = run.base(),
        dir = posix.quote(dir),
        inner = posix.quote(&inner),
    );
    format!("sh -c {}", posix.quote(&script))
}

/// The command that reports on a run: `exit=N`, `running` or `gone` on the
/// first line, then the log from byte `offset` on.
///
/// The code is read before the log: a run that ends between the two reads is
/// reported as running, and its last output comes with the next poll - the
/// other order could report it ended and lose what it wrote last.
///
/// `gone` is a run with no exit code whose process is not there any more:
/// killed (the OOM killer, a reboot) or unable to write its code. Up to v0.26
/// such a run read as `running` for ever, and the deploy waited for ever. A
/// run that is merely slow still has its process, and is waited for as long
/// as it takes - an image build may take many minutes. The process is the one
/// whose number the launch kept, and whose command line carries this run's id:
/// a number taken over by another process after this one ended does not count.
/// Where `ps` cannot say, `kill -0` alone answers. The code is looked for once
/// more after the process: a run that ends between the two looks has one.
fn poll_command(run: &Run, offset: u64) -> String {
    let script = format!(
        "f={base}; p=$(cat \"$f.pid\" 2>/dev/null); \
         if [ -f \"$f.code\" ]; then echo \"exit=$(cat \"$f.code\")\"; \
         elif [ -n \"$p\" ] && kill -0 \"$p\" 2>/dev/null && {{ a=$(ps -p \"$p\" -o args= 2>/dev/null) || a={id}; case \"$a\" in *{id}*) true;; *) false;; esac; }}; then echo running; \
         elif [ -f \"$f.code\" ]; then echo \"exit=$(cat \"$f.code\")\"; \
         else echo gone; fi; \
         tail -c +{from} \"$f.log\" 2>/dev/null; exit 0",
        base = run.base(),
        id = run.id,
        from = offset + 1,
    );
    format!("sh -c {}", Dialect::Posix.quote(&script))
}

/// The command that removes a finished run's files.
fn forget_command(run: &Run) -> String {
    let script = format!("f={}; rm -f \"$f.log\" \"$f.code\" \"$f.pid\"", run.base());
    format!("sh -c {}", Dialect::Posix.quote(&script))
}

/// Where a run stands, as a poll found it.
#[derive(Debug, PartialEq, Eq)]
enum State {
    Running,
    Exited(i32),
    /// No exit code, and no process to write one.
    Gone,
}

/// What a poll said.
#[derive(Debug, PartialEq, Eq)]
struct Polled {
    state: State,
    /// Log bytes past the offset asked for.
    output: Vec<u8>,
}

fn read_poll(reply: &[u8]) -> Result<Polled> {
    let split = reply.iter().position(|byte| *byte == b'\n').unwrap_or(reply.len());
    let head = String::from_utf8_lossy(&reply[..split]);
    let output = reply.get(split + 1..).unwrap_or_default().to_vec();
    let state = match head.trim() {
        "running" => State::Running,
        "gone" => State::Gone,
        status => match status.strip_prefix("exit=").and_then(|code| code.trim().parse().ok()) {
            Some(code) => State::Exited(code),
            None => bail!("the server answered '{status}' about a run it should know"),
        },
    };
    Ok(Polled { state, output })
}

/// The last line with anything on it, cut to fit beside a spinner.
fn last_line(output: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let line = text
        .lines()
        .rev()
        .map(|line| crate::job::strip_ansi(line).trim().to_string())
        .find(|line| !line.is_empty())?;
    const WIDTH: usize = 60;
    Some(if line.chars().count() > WIDTH {
        format!("{}…", line.chars().take(WIDTH - 1).collect::<String>())
    } else {
        line
    })
}

/// Run `command` in `dir` on the server and wait for it, returning its output.
///
/// A non-zero exit is an error carrying the output - the same contract as a
/// plain exec, so callers do not care which way the command ran. `reconnect`
/// opens a fresh connection when the one in `remote` drops mid-run; the
/// replacement is left in `remote` for whatever the caller asks next.
/// `progress` hears the newest line of output as it arrives.
pub fn run<R: Remote>(
    remote: &mut R,
    dialect: Dialect,
    dir: &str,
    command: &str,
    mut reconnect: impl FnMut() -> Result<R>,
    mut progress: impl FnMut(&str),
) -> Result<String> {
    if dialect == Dialect::Windows {
        let stdout = remote.exec_bytes(&dialect.run_in(dir, command))?;
        return Ok(String::from_utf8_lossy(&stdout).into_owned());
    }
    let run = Run::new();
    let launched = remote.exec_bytes(&launch_command(&run, dir, command))?;
    if !String::from_utf8_lossy(&launched).contains("launched") {
        bail!("`{command}` did not start on the server: {}", String::from_utf8_lossy(&launched).trim());
    }
    let mut output = Vec::new();
    let mut pause = Duration::from_millis(100);
    loop {
        let reply = match remote.exec_bytes(&poll_command(&run, output.len() as u64)) {
            Ok(reply) => reply,
            Err(lost) => {
                progress("the connection dropped - dialling again; the command runs on");
                *remote = redial(&mut reconnect).map_err(|_| {
                    anyhow::anyhow!(
                        "the connection dropped while `{command}` ran ({lost:#}), and it could not be opened again.\n\
                         The command keeps running on the server; its output is in {}",
                        run.log()
                    )
                })?;
                continue;
            }
        };
        let polled = read_poll(&reply)?;
        output.extend_from_slice(&polled.output);
        if !polled.output.is_empty()
            && let Some(line) = last_line(&output)
        {
            progress(&line);
        }
        match polled.state {
            State::Exited(code) => {
                // Best-effort: the result is in hand, and three stray files in
                // a dot directory are not worth failing a deploy over.
                let _ = remote.exec_bytes(&forget_command(&run));
                let text = String::from_utf8_lossy(&output).into_owned();
                if code != 0 {
                    bail!("`{command}` exited with {code}: {}", text.trim());
                }
                return Ok(text);
            }
            // Its files stay: the log is all there is to say what happened.
            State::Gone => bail!(
                "`{command}` ended on the server without an exit code - it was killed, or could not write its code; \
                 what it printed is in {}",
                run.log()
            ),
            State::Running => {}
        }
        std::thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_secs(1));
    }
}

/// Dial until a connection opens or [`REDIAL_FOR`] runs out.
fn redial<R>(reconnect: &mut impl FnMut() -> Result<R>) -> Result<R> {
    let started = Instant::now();
    loop {
        match reconnect() {
            Ok(remote) => return Ok(remote),
            Err(err) if started.elapsed() >= REDIAL_FOR => return Err(err),
            Err(_) => std::thread::sleep(REDIAL_EVERY),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    /// The command reaches the server detached from the session, in the
    /// deploy directory, with its code written where the poll reads it.
    #[test]
    fn the_command_starts_in_a_session_of_its_own() {
        let run = Run { id: "r1".into() };
        let launch = launch_command(&run, "/srv/my app", "docker compose up -d --build");
        assert!(launch.starts_with("sh -c '"), "the outer script is sh, whatever the login shell: {launch}");
        assert!(launch.contains("setsid"), "{launch}");
        assert!(launch.contains("/srv/my app"), "{launch}");
        assert!(launch.contains("docker compose up -d --build"), "{launch}");
        assert!(launch.contains("< /dev/null &"), "nothing may hold the channel open: {launch}");
        assert!(
            poll_command(&run, 41).contains("tail -c +42"),
            "the log is read on from the byte after the offset"
        );
    }

    #[test]
    fn a_poll_reads_the_state_then_the_log() {
        assert_eq!(
            read_poll(b"running\nstep 1\nstep 2\n").unwrap(),
            Polled {
                state: State::Running,
                output: b"step 1\nstep 2\n".to_vec()
            }
        );
        assert_eq!(
            read_poll(b"exit=3\n").unwrap(),
            Polled {
                state: State::Exited(3),
                output: Vec::new()
            }
        );
        assert_eq!(
            read_poll(b"gone\nhalf a line").unwrap(),
            Polled {
                state: State::Gone,
                output: b"half a line".to_vec()
            }
        );
        assert!(read_poll(b"who knows\n").is_err(), "a reply nobody wrote must not read as running forever");
    }

    /// A server that answers like a run whose process went without a code:
    /// started, a line of output, then gone.
    struct Vanishing {
        polls: Cell<u32>,
    }

    impl Remote for Vanishing {
        fn exec_bytes(&self, command: &str) -> Result<Vec<u8>> {
            if command.contains("echo launched") {
                return Ok(b"launched\n".to_vec());
            }
            if command.contains("tail -c") {
                self.polls.set(self.polls.get() + 1);
                return Ok(match self.polls.get() {
                    1 => b"running\nbuilding\n".to_vec(),
                    _ => b"gone\n".to_vec(),
                });
            }
            bail!("nothing else is asked of a run that is gone: {command}")
        }
    }

    /// A run whose process went without an exit code ends the wait, with the
    /// log to look in. Up to v0.26 it read as running, and the deploy waited
    /// for a code that could never come.
    #[test]
    fn a_run_gone_without_a_code_ends_the_wait() {
        let mut remote = Vanishing { polls: Cell::new(0) };
        let mut heard = Vec::new();
        let err = run(
            &mut remote,
            Dialect::Posix,
            "/srv/app",
            "docker compose up -d --build",
            || -> Result<Vanishing> { bail!("the connection never drops here") },
            |line| heard.push(line.to_string()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("without an exit code"), "{err}");
        assert!(err.contains("~/.turnout/runs/"), "the log is named: {err}");
        assert_eq!(heard, ["building"]);
    }

    /// The poll asks after the run's process, by the number the launch kept
    /// and by the run's id in its command line, before it says `gone`; and
    /// the launch writes both files in the foreground.
    #[test]
    fn the_poll_asks_after_the_process_and_the_launch_keeps_its_number() {
        let run = Run { id: "r7".into() };
        let poll = poll_command(&run, 0);
        assert!(poll.contains("kill -0"), "{poll}");
        assert!(poll.contains("*r7*"), "{poll}");
        assert!(poll.contains("echo gone"), "{poll}");
        let launch = launch_command(&run, "/srv/app", "make");
        assert!(launch.contains(": > \"$f.log\" || exit 1"), "the log exists before the run starts: {launch}");
        assert!(launch.contains("echo $! > \"$f.pid\" || exit 1"), "{launch}");
        assert!(launch.contains(": r7;"), "the run's id opens its command line: {launch}");
    }

    #[test]
    fn the_spinner_shows_the_newest_line() {
        assert_eq!(last_line(b"one\ntwo\n\n").as_deref(), Some("two"));
        assert_eq!(last_line(b"\x1b[32mbuilt\x1b[0m\n").as_deref(), Some("built"));
        assert_eq!(last_line(b"").as_deref(), None);
        assert_eq!(last_line(&[b'x'; 200]).map(|line| line.chars().count()), Some(60));
    }

    /// A session that fails the first `drops` polls, as a connection does
    /// when the network goes.
    #[cfg(unix)]
    struct Flaky {
        session: Session,
        drops: Cell<u32>,
    }

    #[cfg(unix)]
    impl Remote for Flaky {
        fn exec_bytes(&self, command: &str) -> Result<Vec<u8>> {
            if command.contains("tail -c") && self.drops.get() > 0 {
                self.drops.set(self.drops.get() - 1);
                bail!("connection reset by peer");
            }
            self.session.exec_bytes(command)
        }
    }

    /// The point of the module: the command finishes on the server whatever
    /// happens to the connection that started it, and its output and code
    /// come back through the next one.
    #[cfg(unix)]
    #[test]
    fn a_run_survives_the_connection_that_started_it() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let dir = tempfile::tempdir().unwrap();
        let mut remote = Flaky {
            session: stand.session(),
            drops: Cell::new(2),
        };
        let redials = Cell::new(0);
        let mut lines = Vec::new();
        let output = run(
            &mut remote,
            Dialect::Posix,
            &dir.path().display().to_string(),
            "sleep 1; pwd; echo done > marker",
            || {
                redials.set(redials.get() + 1);
                Ok(Flaky {
                    session: stand.session(),
                    drops: Cell::new(0),
                })
            },
            |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(redials.get(), 1, "one drop, one new connection: {lines:?}");
        assert!(output.contains(&dir.path().display().to_string()), "it ran in the deploy directory: {output}");
        assert!(dir.path().join("marker").is_file(), "and it ran to the end");
        let runs = stand.home().join(RUNS);
        assert_eq!(std::fs::read_dir(&runs).unwrap().count(), 0, "a finished run leaves nothing behind");
    }

    /// A failing command fails the run with its own words and code, the way a
    /// plain exec did.
    #[cfg(unix)]
    #[test]
    fn a_failing_command_reports_its_code_and_output() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let dir = tempfile::tempdir().unwrap();
        let mut session = stand.session();
        let err = run(
            &mut session,
            Dialect::Posix,
            &dir.path().display().to_string(),
            "echo the image did not build; exit 3",
            || Ok(stand.session()),
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("exited with 3"), "{err}");
        assert!(err.contains("the image did not build"), "{err}");
    }

    /// A run whose process is killed before it can write its code - here the
    /// command kills the shell that would have written it - ends the wait.
    /// Up to v0.26 this deploy waited for ever.
    #[cfg(unix)]
    #[test]
    fn a_killed_run_ends_the_wait_in_a_real_shell() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let dir = tempfile::tempdir().unwrap();
        let mut session = stand.session();
        let err = run(
            &mut session,
            Dialect::Posix,
            &dir.path().display().to_string(),
            "echo about to go; kill -9 $PPID",
            || Ok(stand.session()),
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("without an exit code"), "{err}");
        let runs = stand.home().join(RUNS);
        let log = std::fs::read_dir(&runs)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "log"))
            .expect("the log stays");
        assert!(std::fs::read_to_string(log).unwrap().contains("about to go"));
    }

    /// A run that cannot create its log does not start, and the launch says
    /// so. Up to v0.26 the background job opened the log, failed, and never
    /// ran - while the launch printed `launched` and the deploy waited for
    /// ever for its exit code.
    #[cfg(unix)]
    #[test]
    fn a_run_that_cannot_keep_its_log_does_not_launch() {
        if unsafe { libc::geteuid() } == 0 {
            // root writes through any permission.
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let runs = stand.home().join(RUNS);
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::set_permissions(&runs, std::fs::Permissions::from_mode(0o555)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut session = stand.session();
        let outcome = run(
            &mut session,
            Dialect::Posix,
            &dir.path().display().to_string(),
            "echo ran > marker",
            || Ok(stand.session()),
            |_| {},
        );
        std::fs::set_permissions(&runs, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(outcome.is_err(), "{outcome:?}");
        assert!(!dir.path().join("marker").exists(), "nothing ran");
    }

    /// With no way back to the server, the error says the command is still
    /// running and where its output goes - not that it failed.
    #[cfg(unix)]
    #[test]
    fn a_connection_that_cannot_be_reopened_says_where_the_output_is() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let dir = tempfile::tempdir().unwrap();
        let mut remote = Flaky {
            session: stand.session(),
            drops: Cell::new(1),
        };
        let err = run(
            &mut remote,
            Dialect::Posix,
            &dir.path().display().to_string(),
            "sleep 2; echo late > marker",
            || -> Result<Flaky> { bail!("no route to host") },
            |_| {},
        );
        let err = format!("{:#}", err.unwrap_err());
        assert!(err.contains("keeps running on the server"), "{err}");
        assert!(err.contains("~/.turnout/runs/"), "{err}");
    }
}
