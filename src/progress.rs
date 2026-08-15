//! Progress reporting for long remote operations.
//!
//! Deploy spends most of its wall time in places that produce no output on
//! their own: the SSH handshake, the archive going over the wire, tar on the
//! far end. Without feedback the command looks hung, which is the single most
//! common complaint about it.
//!
//! On a terminal the rendering is cliclack's clack-style checklist: `intro`
//! opens the frame, each phase runs as a spinner that settles into a checked
//! line, the upload is a live byte bar, and `outro` closes the frame.
//! Everything degrades to plain lines when stdout is not a terminal, so piped
//! output and CI logs stay readable.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use indicatif::HumanBytes;

/// The upload bar: percentage first, then the raw numbers a human checks when
/// the percentage stalls. cliclack's theme puts the state symbol in front.
const BAR_TEMPLATE: &str = "{msg} [{bar:24.cyan/blue}] {percent:>3}% · {bytes}/{total_bytes} · {bytes_per_sec} · eta {eta}";

fn is_terminal() -> bool {
    use std::io::IsTerminal;
    use std::sync::OnceLock;
    // Asked once per process: whether stdout is a terminal cannot change
    // mid-run, and every helper here consults it on every call.
    static TTY: OnceLock<bool> = OnceLock::new();
    *TTY.get_or_init(|| std::io::stdout().is_terminal())
}

/// Open the checklist frame around a multi-phase command.
pub fn intro(title: impl Into<Cow<'static, str>>) {
    let title = title.into();
    if is_terminal() {
        let _ = cliclack::intro(title);
    } else {
        println!("{title}");
    }
}

/// Close the frame after the last phase.
pub fn outro(message: impl Into<Cow<'static, str>>) {
    let message = message.into();
    if is_terminal() {
        let _ = cliclack::outro(message);
    } else {
        println!("{message}");
    }
}

/// Close the frame on a failure; the error itself prints after it.
pub fn outro_error(message: impl Into<Cow<'static, str>>) {
    let message = message.into();
    if is_terminal() {
        let _ = cliclack::outro_cancel(message);
    } else {
        eprintln!("{message}");
    }
}

/// A line of command output inside the frame (a restart command's stdout).
pub fn info(message: &str) {
    if is_terminal() {
        let _ = cliclack::log::info(message);
    } else {
        println!("{message}");
    }
}

/// A caveat that does not stop the command.
pub fn warn(message: &str) {
    if is_terminal() {
        let _ = cliclack::log::warning(message);
    } else {
        eprintln!("note: {message}");
    }
}

/// A phase that shows a spinner while it runs and a checked line when it ends.
///
/// On a non-terminal stdout the spinner is skipped and only the final line is
/// printed, so logs get one line per phase instead of a stream of redraws.
pub struct Step {
    bar: Option<cliclack::ProgressBar>,
    /// Off-terminal only: what the step announced at its start, printed if the
    /// step ends without a result line of its own.
    pending: Option<Cow<'static, str>>,
}

impl Step {
    /// Start a phase; `message` is phrased as an action in progress ("Connecting to ...").
    pub fn start(message: impl Into<Cow<'static, str>>) -> Self {
        let message = message.into();
        if !is_terminal() {
            // Held rather than printed: a step that finishes with `done` prints
            // only its result, so a log gets one line per step instead of two.
            return Self {
                bar: None,
                pending: Some(message),
            };
        }
        let bar = cliclack::spinner();
        bar.start(message);
        Self { bar: Some(bar), pending: None }
    }

    /// Replace the message of a running phase.
    pub fn update(&self, message: impl Into<Cow<'static, str>>) {
        if let Some(bar) = &self.bar {
            bar.set_message(message.into());
        }
    }

    /// Finish the phase, replacing the spinner with a checked line.
    pub fn done(self, message: impl Into<Cow<'static, str>>) {
        let message = message.into();
        match self.bar {
            Some(bar) => bar.stop(message),
            None => println!("{message}"),
        }
    }

    /// Finish the phase without a result line of its own; the caller prints
    /// something more specific in its place. Off-terminal the step still
    /// announces what it was doing, so the output that follows has context.
    pub fn clear(self) {
        match self.bar {
            Some(bar) => bar.clear(),
            None => {
                if let Some(message) = &self.pending {
                    println!("{message}");
                }
            }
        }
    }
}

/// A determinate byte bar for an upload, ending in a summary line.
pub struct Transfer {
    bar: Option<cliclack::ProgressBar>,
    started: Instant,
}

impl Transfer {
    /// Start a transfer of `files` files totalling `bytes`.
    pub fn start(files: u64, bytes: u64) -> Self {
        let started = Instant::now();
        if !is_terminal() {
            let plural = if files == 1 { "" } else { "s" };
            println!("Uploading {files} file{plural} ({}) ...", human_bytes(bytes));
            return Self { bar: None, started };
        }
        let bar = cliclack::progress_bar(bytes).with_template(BAR_TEMPLATE);
        bar.start("Uploading");
        Self { bar: Some(bar), started }
    }

    /// Account for uploaded bytes.
    pub fn advance(&self, bytes: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(bytes);
        }
    }

    /// How long the transfer has been running, for the summary line.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Finish the bar, replacing it with a checked summary line.
    pub fn done(self, message: impl Into<Cow<'static, str>>) {
        let message = message.into();
        match self.bar {
            Some(bar) => bar.stop(message),
            None => println!("{message}"),
        }
    }
}

/// Byte count for humans: `5.96 MiB`, `812.00 KiB`, `93 B`.
///
/// Delegates to the same formatter the transfer bar uses, so the summary line
/// and the bar above it cannot drift into different units.
pub fn human_bytes(bytes: u64) -> String {
    HumanBytes(bytes).to_string()
}

/// Wall-clock time for the summary line: `4.1s`, `12s`, `1m 43s`.
///
/// Sub-second precision only where it is informative - below ten seconds a
/// digit after the point is the difference between fast and instant; above a
/// minute nobody reads tenths.
pub fn human_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 10.0 {
        return format!("{secs:.1}s");
    }
    let whole = secs.round() as u64;
    if whole < 60 {
        format!("{whole}s")
    } else {
        format!("{}m {:02}s", whole / 60, whole % 60)
    }
}

/// Mean transfer rate for the summary line: `1.25 MiB/s`.
pub fn rate(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= f64::EPSILON {
        // A transfer too fast to time meaningfully; showing the total as a
        // rate would claim precision the clock did not deliver.
        return format!("{}/s", HumanBytes(bytes));
    }
    format!("{}/s", HumanBytes((bytes as f64 / secs) as u64))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{human_bytes, human_duration, rate};

    /// Pins the units to the ones the transfer bar renders, so the summary
    /// line and the bar cannot drift apart.
    #[test]
    fn formats_byte_counts_like_the_bar() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(93), "93 B");
        assert!(human_bytes(831_488).ends_with("KiB"), "got {}", human_bytes(831_488));
        assert!(human_bytes(6_400_000).ends_with("MiB"), "got {}", human_bytes(6_400_000));
        assert!(human_bytes(3_221_225_472).ends_with("GiB"), "got {}", human_bytes(3_221_225_472));
    }

    /// The summary quotes wall time at the precision a human cares about:
    /// tenths under ten seconds, whole seconds under a minute, then minutes.
    #[test]
    fn formats_durations_at_a_human_scale() {
        assert_eq!(human_duration(Duration::from_millis(400)), "0.4s");
        assert_eq!(human_duration(Duration::from_millis(4_100)), "4.1s");
        assert_eq!(human_duration(Duration::from_secs(12)), "12s");
        assert_eq!(human_duration(Duration::from_secs(103)), "1m 43s");
        // Rounding must not produce the nonsense "60s".
        assert_eq!(human_duration(Duration::from_millis(59_700)), "1m 00s");
    }

    /// The rate is bytes over wall time in the bar's own units; a transfer too
    /// fast to time must not divide by (almost) zero.
    #[test]
    fn rates_use_the_bar_units_and_survive_zero_time() {
        let mib = rate(5 * 1024 * 1024, Duration::from_secs(4));
        assert_eq!(mib, "1.25 MiB/s");
        assert!(rate(1000, Duration::ZERO).ends_with("/s"));
    }
}
