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
fn launch_command(run: &Run, dir: &str, command: &str) -> String {
    let posix = Dialect::Posix;
    let inner = format!(
        "\"${{SHELL:-sh}}\" -c {}; c=$?; echo $c > \"$1.part\" && mv \"$1.part\" \"$1.code\"",
        posix.quote(command)
    );
    let script = format!(
        "mkdir -p \"$HOME/{RUNS}\" || exit 1; f={base}; cd {dir} || exit 1; \
         if command -v setsid >/dev/null 2>&1; then d=setsid; else d=nohup; fi; \
         $d sh -c {inner} turnout-run \"$f\" > \"$f.log\" 2>&1 < /dev/null & \
         echo launched",
        base = run.base(),
        dir = posix.quote(dir),
        inner = posix.quote(&inner),
    );
    format!("sh -c {}", posix.quote(&script))
}

/// The command that reports on a run: `exit=N` or `running` on the first
/// line, then the log from byte `offset` on.
///
/// The code is read before the log: a run that ends between the two reads is
/// reported as running, and its last output comes with the next poll - the
/// other order could report it ended and lose what it wrote last.
fn poll_command(run: &Run, offset: u64) -> String {
    let script = format!(
        "f={base}; if [ -f \"$f.code\" ]; then echo \"exit=$(cat \"$f.code\")\"; else echo running; fi; \
         tail -c +{from} \"$f.log\" 2>/dev/null; exit 0",
        base = run.base(),
        from = offset + 1,
    );
    format!("sh -c {}", Dialect::Posix.quote(&script))
}

/// The command that removes a finished run's files.
fn forget_command(run: &Run) -> String {
    let script = format!("f={}; rm -f \"$f.log\" \"$f.code\"", run.base());
    format!("sh -c {}", Dialect::Posix.quote(&script))
}

/// What a poll said.
#[derive(Debug, PartialEq, Eq)]
struct Polled {
    /// The exit code, once the run has one.
    exit: Option<i32>,
    /// Log bytes past the offset asked for.
    output: Vec<u8>,
}

fn read_poll(reply: &[u8]) -> Result<Polled> {
    let split = reply.iter().position(|byte| *byte == b'\n').unwrap_or(reply.len());
    let head = String::from_utf8_lossy(&reply[..split]);
    let output = reply.get(split + 1..).unwrap_or_default().to_vec();
    let exit = match head.trim() {
        "running" => None,
        status => match status.strip_prefix("exit=").and_then(|code| code.trim().parse().ok()) {
            Some(code) => Some(code),
            None => bail!("the server answered '{status}' about a run it should know"),
        },
    };
    Ok(Polled { exit, output })
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
        if let Some(code) = polled.exit {
            // Best-effort: the result is in hand, and two stray files in a
            // dot directory are not worth failing a deploy over.
            let _ = remote.exec_bytes(&forget_command(&run));
            let text = String::from_utf8_lossy(&output).into_owned();
            if code != 0 {
                bail!("`{command}` exited with {code}: {}", text.trim());
            }
            return Ok(text);
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
    #[cfg(unix)]
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
                exit: None,
                output: b"step 1\nstep 2\n".to_vec()
            }
        );
        assert_eq!(
            read_poll(b"exit=3\n").unwrap(),
            Polled {
                exit: Some(3),
                output: Vec::new()
            }
        );
        assert!(read_poll(b"who knows\n").is_err(), "a reply nobody wrote must not read as running forever");
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
