//! Running a project command with a quiet console.
//!
//! Up to v0.18 every command turnout ran was a pass-through: the child's
//! stdout and stderr went straight to the terminal, and a build spent two
//! minutes filling the scrollback with progress the user never reads. What a
//! human wants from `turnout build` is one line that says it is building and
//! one that says it finished; what they want from the rare failure is the
//! whole output, right there, without running it again.
//!
//! So the child's output is always captured here and always written to a log
//! file, and only the *terminal* rendering changes with the mode:
//!
//! * [`Mode::Quiet`] - `build`, `test`, `lint`: a spinner while it runs, a
//!   checked line with the elapsed time when it ends.
//! * [`Mode::UntilReady`] - `dev` and `run`: a spinner until the server says
//!   it is up (see [`ready`]), then only the lines that look like errors or
//!   warnings. A server that never announces itself falls back to streaming,
//!   because silence from a spinner is worse than noise.
//! * [`Mode::Stream`] - `-v`, and everything off a terminal: the old
//!   behaviour, byte for byte.
//!
//! The log file is not a debugging aid bolted on: it is the half of the
//! background mode (v0.20) that has to exist before a job can be detached at
//! all. `turnout logs` will read exactly these files.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::progress::{self, Step, human_duration};

/// How much of the output a failure replays on the terminal.
///
/// Enough to hold a compiler's error with its context, short enough that the
/// command that caused it is still on screen above it. The full output is in
/// the log file either way, and the note under the tail says where.
const TAIL_LINES: usize = 40;

/// How the child's output reaches the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// A spinner, then one line with the elapsed time. Output only on failure.
    Quiet,
    /// A spinner until the server is ready, then errors and warnings only.
    UntilReady,
    /// Everything, as it arrives.
    Stream,
}

/// Forces the console mode regardless of the terminal: `quiet`, `ready` or
/// `stream`.
///
/// The quiet modes only ever happen on a terminal, and a test harness has
/// none - so without this the hiding, the failure tail and the ready detector
/// would be reasoned about and never run. Undocumented in the CLI on purpose:
/// `--verbose` is the user-facing half, and this exists so the other half is
/// exercised rather than assumed.
pub const MODE_ENV: &str = "TURNOUT_CONSOLE";

impl Mode {
    /// The mode a command actually runs in.
    ///
    /// `--verbose` and a non-terminal stdout both mean "stream": a log file, a
    /// CI job and a pipe all want the lines, and a spinner redrawing into a
    /// file is line noise. Asked once per run, at the top, so the child cannot
    /// be started under one mode and reported under another.
    pub fn resolve(wanted: Mode, verbose: bool) -> Mode {
        // The override outranks even `--verbose`: its whole purpose is to put
        // the run in a mode the environment would not otherwise produce.
        if let Some(forced) = Self::forced() {
            return forced;
        }
        if verbose || !is_terminal() { Mode::Stream } else { wanted }
    }

    fn forced() -> Option<Mode> {
        match std::env::var(MODE_ENV).ok()?.trim().to_ascii_lowercase().as_str() {
            "quiet" => Some(Mode::Quiet),
            "ready" => Some(Mode::UntilReady),
            "stream" => Some(Mode::Stream),
            _ => None,
        }
    }
}

fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// One line of the child's output, tagged with the stream it came from.
struct Line {
    text: String,
    /// stderr lines are the ones a quiet `dev` keeps showing; stdout is where
    /// a dev server usually announces itself.
    is_err: bool,
}

/// Where a job's output is kept, and how it is read back.
///
/// One file per app and command rather than one per run: the question a
/// developer asks is "what did the last build say", and a directory of
/// timestamped files answers a question nobody asked while growing without
/// bound. A run truncates the file it writes.
pub struct Log {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Log {
    /// Open (and truncate) the log for `app`'s `command`.
    ///
    /// A log that cannot be opened is a note, not a failure: the command the
    /// user asked for still runs, and it prints where the log would have gone.
    /// Losing a build over a read-only data directory would be absurd.
    pub fn open(app: &str, command: &str) -> Self {
        match Self::try_open(app, command) {
            Ok(log) => log,
            Err(err) => {
                progress::warn(&format!("cannot write the job log: {err:#}"));
                Self {
                    path: PathBuf::new(),
                    file: None,
                }
            }
        }
    }

    fn try_open(app: &str, command: &str) -> Result<Self> {
        let dir = logs_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
        let path = dir.join(format!("{}-{}.log", slug(app), slug(command)));
        let file = std::fs::File::create(&path).with_context(|| format!("cannot write {}", path.display()))?;
        Ok(Self { path, file: Some(file) })
    }

    /// Where the log lives, for the note under a failure.
    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(|_| self.path.as_path())
    }

    fn write_line(&mut self, line: &str) {
        if let Some(file) = &mut self.file {
            // A log that starts failing mid-run stops being a log: dropping the
            // handle keeps `path()` honest instead of advertising half a file.
            if writeln!(file, "{line}").is_err() {
                self.file = None;
            }
        }
    }
}

/// The directory holding every job log.
pub fn logs_dir() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("logs"))
}

/// A name safe for a file on every platform turnout runs on.
///
/// App and command names come from the catalog, where nothing stops a command
/// being called `test:e2e` - a colon is a valid file name on Unix and an
/// alternate data stream on Windows.
fn slug(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_string();
    if cleaned.is_empty() { "job".to_string() } else { cleaned }
}

/// What a finished job leaves behind.
pub struct Outcome {
    pub status: std::process::ExitStatus,
}

/// Run a shell command line in a directory under the chosen console mode.
///
/// `label` names the job for the spinner ("Building myapp"); `ready` is the
/// detector a [`Mode::UntilReady`] job uses to decide the server is up, and
/// what to print when it is.
pub fn run(command_line: &str, dir: &Path, env: &[(&str, String)], mode: Mode, label: &str, log: &mut Log, mut ready: Option<Ready<'_>>) -> Result<Outcome> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", command_line]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("sh");
        command.args(["-c", command_line]);
        command
    };
    command.current_dir(dir);
    for (name, value) in env {
        command.env(name, value);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| format!("cannot run '{command_line}'"))?;
    crate::term::confine(&child);
    crate::term::child_begin();

    // Both pipes feed one channel, so the interleaving the reader sees is the
    // one the terminal would have shown. Reading them on separate threads is
    // not an optimisation: a child that fills stderr while we drain stdout
    // deadlocks on the unread pipe otherwise.
    let (sender, receiver) = channel::<Line>();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let out_thread = spawn_reader(stdout, sender.clone(), false);
    let err_thread = spawn_reader(stderr, sender, true);

    let started = Instant::now();
    let mut tail: Vec<String> = Vec::new();
    let mut shown_any = false;
    let mut step = match mode {
        Mode::Stream => None,
        Mode::Quiet | Mode::UntilReady => Some(Step::start(format!("{label} ..."))),
    };

    // The detector runs in every mode, because `--open` hangs off it: a
    // `turnout dev -v --open` still wants its browser, and nothing else in
    // the process knows when the server started answering. Only the *console*
    // changes with the mode - `streaming` says the lines go out as they come,
    // `watching` says the detector is still looking for the signal.
    let mut watching = ready.is_some();
    let mut streaming = mode == Mode::Stream;

    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                log.write_line(&line.text);
                remember(&mut tail, &line.text);
                let mut settled = false;
                if watching
                    && let Some(detector) = ready.as_mut()
                    && let Some(address) = detector.sees(&line.text)
                {
                    watching = false;
                    settled = true;
                    if let Some(step) = step.take() {
                        step.done(detector.settled(address, started.elapsed()));
                    }
                    if let Some(open) = detector.on_ready.take() {
                        open();
                    }
                }
                if streaming {
                    emit(&line);
                    shown_any = true;
                    continue;
                }
                // The line that announced the server has been said better by
                // the step above it; the rest of a running server's chatter is
                // noise except where it is not - a failed compile, a warning.
                if !settled && !watching && mode == Mode::UntilReady && is_noteworthy(&line) {
                    if let Some(step) = step.take() {
                        step.clear();
                    }
                    emit(&line);
                    shown_any = true;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A server that says nothing recognisable: rather than spin
                // forever over one that is running fine, hand the console back
                // and let it speak for itself from here on.
                if watching
                    && let Some(detector) = ready.as_ref()
                    && started.elapsed() >= detector.patience
                {
                    watching = false;
                    if !streaming {
                        streaming = true;
                        if let Some(step) = step.take() {
                            step.clear();
                        }
                        replay(&tail);
                        shown_any = !tail.is_empty();
                    }
                }
                if let Some(step) = &step {
                    step.update(format!("{label} ... {}", human_duration(started.elapsed())));
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let status = child.wait();
    crate::term::child_end();
    let _ = out_thread.join();
    let _ = err_thread.join();
    let status = status.with_context(|| format!("cannot run '{command_line}'"))?;
    let elapsed = started.elapsed();

    match step {
        Some(step) if status.success() => step.done(format!("{label} finished in {}", human_duration(elapsed))),
        Some(step) => {
            step.clear();
            progress::outro_error(format!("{label} failed after {}", human_duration(elapsed)));
        }
        None => {}
    }
    // The point of hiding the output is that a failure gets to show it. Only
    // what was not already on screen: a streamed run has printed it once.
    if !status.success() && !shown_any {
        replay(&tail);
        if let Some(path) = log.path() {
            eprintln!("full output: {}", path.display());
        }
    }
    Ok(Outcome { status })
}

/// Print a captured line on the stream it came from.
fn emit(line: &Line) {
    if line.is_err {
        eprintln!("{}", line.text);
    } else {
        println!("{}", line.text);
    }
}

/// Put the held-back output on screen, oldest first.
fn replay(tail: &[String]) {
    if tail.is_empty() {
        return;
    }
    let mut stderr = std::io::stderr().lock();
    for line in tail {
        let _ = writeln!(stderr, "{line}");
    }
    let _ = stderr.flush();
}

/// Keep the last [`TAIL_LINES`] lines, so a failure has something to show.
fn remember(tail: &mut Vec<String>, line: &str) {
    if tail.len() == TAIL_LINES {
        tail.remove(0);
    }
    tail.push(line.to_string());
}

/// Whether a line survives the quiet filter of a running dev server.
///
/// Deliberately generous on stderr and narrow on stdout: a dev server writes
/// its problems to stderr, and the one thing that must never be swallowed is
/// the reason a page went blank. A blank stderr line is the separator around
/// such a message, not content, so it goes.
fn is_noteworthy(line: &Line) -> bool {
    if line.text.trim().is_empty() {
        return false;
    }
    if line.is_err {
        return true;
    }
    let lower = line.text.to_ascii_lowercase();
    ["error", "err!", "warn", "failed", "fail:", "cannot", "unable to", "exception"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Read one pipe line by line into the shared channel.
///
/// Lossy UTF-8 rather than a hard error: a tool that emits a stray byte (a
/// Windows console code page, a progress bar's control sequence) must not take
/// the run down with it.
fn spawn_reader(pipe: impl Read + Send + 'static, sender: Sender<Line>, is_err: bool) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while buffer.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                        buffer.pop();
                    }
                    let text = String::from_utf8_lossy(&buffer).into_owned();
                    if sender.send(Line { text, is_err }).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// The detector that decides a dev server is up, and says so.
pub struct Ready<'a> {
    /// What the app is called, for the line the detector settles on.
    pub app: &'a str,
    /// The front door address to print once the server answers, when there is
    /// a gateway to print one.
    pub door: Option<String>,
    /// Where the dev server itself listens, for when there is no door.
    ///
    /// Vite announces "ready in 590 ms" one line *before* it prints its
    /// `Local:` address, so the signal that settles the loader often carries
    /// no address at all. Without this the line would read "myapp ready in
    /// 0.8s" and leave the reader to guess where - while turnout has known
    /// the port since it handed it out.
    pub own: Option<String>,
    /// How long to wait for a signal before giving up and streaming.
    pub patience: Duration,
    /// What to do the moment the server answers - `--open` hangs its browser
    /// here. Runs once, from the reader loop, so nothing has to poll a port to
    /// guess when the page would load.
    pub on_ready: Option<Box<dyn Fn() + Send>>,
}

impl Ready<'_> {
    /// Whether this line says the server is up, and the address it named.
    fn sees(&self, line: &str) -> Option<Option<String>> {
        ready(line)
    }

    /// The line that replaces the spinner.
    ///
    /// The front door first - it is the address turnout wants people using,
    /// and it survives the dev server taking a different port tomorrow. Then
    /// whatever the signal line itself named, then the port turnout handed
    /// out. A line that says "ready" and nothing else has hidden the one
    /// thing the reader was waiting for.
    fn settled(&self, address: Option<String>, elapsed: Duration) -> String {
        let where_to = self.door.clone().or(address).or_else(|| self.own.clone());
        match where_to {
            Some(url) => format!("{} ready in {} - {url}", self.app, human_duration(elapsed)),
            None => format!("{} ready in {}", self.app, human_duration(elapsed)),
        }
    }
}

/// Whether a line of a dev server's output announces that it is up, and the
/// address it named if it named one.
///
/// Three shapes cover the servers turnout is pointed at. Vite prints `ready in
/// 431 ms` and then its `Local:` line; Next prints `compiled` or `Ready in`;
/// everything else is recognised by the first line that carries a `http://`
/// address, which is what a server prints when it starts listening and almost
/// never before. The returned address is the one to show when turnout has no
/// front door of its own to offer.
///
/// `Option<Option<String>>`: the outer says whether this line is the signal,
/// the inner whether the signal carried an address.
pub fn ready(line: &str) -> Option<Option<String>> {
    let plain = strip_ansi(line);
    let text = plain.trim();
    if text.is_empty() {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    // "ready in", "Ready in", "compiled successfully", "compiled in 1.2s".
    let announced = lower.contains("ready in") || lower.contains("compiled ") || lower.ends_with("compiled");
    let address = url_in(text);
    if announced || address.is_some() {
        return Some(address);
    }
    None
}

/// The first `http://`/`https://` address in a line, trimmed of the
/// punctuation a server wraps it in.
fn url_in(text: &str) -> Option<String> {
    let at = text.find("http://").or_else(|| text.find("https://"))?;
    let rest = &text[at..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == ')')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ';']);
    (!url.is_empty()).then(|| url.to_string())
}

/// A line without the colour codes a dev server wraps its words in.
///
/// Vite writes `ready in` with the number in green; matching on the raw bytes
/// would work for that one and break on the next tool that colours the word
/// itself. Only CSI sequences - the only kind that appears in this output.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        // A CSI sequence runs to the first byte in @-~.
        for c in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&c) {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three shapes the detector promises to know, and the chatter that
    /// must not be mistaken for a server coming up.
    #[test]
    fn ready_recognises_the_servers_it_claims_to() {
        // Vite, colours and all - including a theme that dims the word "in"
        // on its own, which splits the phrase the detector matches on.
        assert_eq!(ready("  \x1b[32mVITE v5.4.0\x1b[0m  \x1b[2mready in\x1b[0m 431 ms"), Some(None));
        assert_eq!(ready("  \x1b[2mready\x1b[0m \x1b[2min\x1b[0m 431 ms"), Some(None));
        assert_eq!(ready("  ➜  Local:   http://localhost:5100/"), Some(Some("http://localhost:5100/".to_string())));
        // Next.
        assert_eq!(ready("✓ Compiled in 1.2s (412 modules)"), Some(None));
        assert_eq!(ready("- Ready in 2.3s"), Some(None));
        // Anything else: the first line with an address.
        assert_eq!(
            ready("Server listening on http://127.0.0.1:3000"),
            Some(Some("http://127.0.0.1:3000".to_string()))
        );
        // Not a signal.
        assert_eq!(ready(""), None);
        assert_eq!(ready("  "), None);
        assert_eq!(ready("$ vite --port 5100"), None);
        assert_eq!(ready("watching for file changes"), None);
        assert_eq!(ready("error: cannot resolve ./missing"), None);
    }

    /// The address is what a browser could open: no trailing bracket, quote or
    /// sentence punctuation dragged in from the line around it.
    #[test]
    fn the_address_comes_out_clean() {
        assert_eq!(url_in("see http://localhost:5100, then"), Some("http://localhost:5100".to_string()));
        assert_eq!(url_in("open (http://a.localhost) now"), Some("http://a.localhost".to_string()));
        assert_eq!(url_in("\"https://example.test/path\""), Some("https://example.test/path".to_string()));
        assert_eq!(url_in("no address here"), None);
    }

    /// The quiet filter's promise: nothing that reads like a problem is
    /// swallowed, and a running server's ordinary chatter is.
    #[test]
    fn the_quiet_filter_keeps_problems_and_drops_chatter() {
        let out = |text: &str| Line {
            text: text.to_string(),
            is_err: false,
        };
        let err = |text: &str| Line {
            text: text.to_string(),
            is_err: true,
        };
        assert!(is_noteworthy(&err("anything on stderr")));
        assert!(is_noteworthy(&out("Error: Failed to resolve import")));
        assert!(is_noteworthy(&out("[plugin:vite] warning: unused")));
        assert!(is_noteworthy(&out("build failed with 1 error")));
        assert!(!is_noteworthy(&out("hmr update /src/App.vue")));
        assert!(!is_noteworthy(&out("page reload src/main.ts")));
        // A blank line is the padding around a message, not a message.
        assert!(!is_noteworthy(&err("   ")));
    }

    /// The tail is bounded and keeps the *end* of the output - a compiler puts
    /// its summary last, and an unbounded buffer is a leak on a dev server
    /// that runs for a day.
    #[test]
    fn the_tail_keeps_the_last_lines_and_no_more() {
        let mut tail = Vec::new();
        for i in 0..(TAIL_LINES + 10) {
            remember(&mut tail, &format!("line {i}"));
        }
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail.first().unwrap(), &format!("line {}", 10));
        assert_eq!(tail.last().unwrap(), &format!("line {}", TAIL_LINES + 9));
    }

    /// The settled line always names somewhere to go when there is somewhere
    /// to name - the front door first, then whatever the server said, then
    /// the port turnout handed out. Vite announces "ready in 590 ms" a line
    /// before it prints its address, so the last fallback is the usual case
    /// without a gateway, not an exotic one.
    #[test]
    fn the_ready_line_names_an_address_whenever_one_exists() {
        let detector = |door: Option<&str>, own: Option<&str>| Ready {
            app: "myapp",
            door: door.map(str::to_string),
            own: own.map(str::to_string),
            patience: Duration::from_secs(1),
            on_ready: None,
        };
        let at = Duration::from_millis(800);
        // The door outranks the address the server printed about itself.
        assert_eq!(
            detector(Some("http://myapp.localhost"), Some("http://localhost:5100")).settled(Some("http://localhost:5173".into()), at),
            "myapp ready in 0.8s - http://myapp.localhost"
        );
        // No door: what the signal line named.
        assert_eq!(
            detector(None, Some("http://localhost:5100")).settled(Some("http://localhost:5173".into()), at),
            "myapp ready in 0.8s - http://localhost:5173"
        );
        // No door and a signal that carried no address - Vite's usual shape.
        assert_eq!(
            detector(None, Some("http://localhost:5100")).settled(None, at),
            "myapp ready in 0.8s - http://localhost:5100"
        );
        // Nothing known anywhere: say so rather than invent an address.
        assert_eq!(detector(None, None).settled(None, at), "myapp ready in 0.8s");
    }

    /// A command name is not a file name until this says it is.
    #[test]
    fn log_names_survive_a_command_called_test_e2e() {
        assert_eq!(slug("myapp"), "myapp");
        assert_eq!(slug("test:e2e"), "test-e2e");
        assert_eq!(slug("../etc/passwd"), "..-etc-passwd");
        assert_eq!(slug("a b"), "a-b");
        assert_eq!(slug("--"), "job");
        assert_eq!(slug(""), "job");
    }

    /// Verbose and a pipe both mean "stream"; the wanted mode only survives on
    /// a terminal, and a test process has none.
    ///
    /// Reads the environment, so it runs alone - `cargo test` shares one
    /// process across threads, and a neighbour setting `TURNOUT_CONSOLE`
    /// would decide this test's answer. The integration suite exercises the
    /// override for real, in a child process.
    #[test]
    fn verbosity_and_pipes_both_force_streaming() {
        assert!(
            std::env::var(MODE_ENV).is_err(),
            "{MODE_ENV} is set in this process; the resolve tests describe the unforced path"
        );
        assert_eq!(Mode::resolve(Mode::Quiet, true), Mode::Stream);
        assert_eq!(Mode::resolve(Mode::UntilReady, true), Mode::Stream);
        // No terminal under `cargo test`, so even the quiet mode streams.
        assert_eq!(Mode::resolve(Mode::Quiet, false), Mode::Stream);
    }
}
