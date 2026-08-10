//! Progress reporting for long remote operations.
//!
//! Deploy spends most of its wall time in two places that produce no output on
//! their own: the SSH handshake and the SFTP upload loop. Without feedback the
//! command looks hung, which is the single most common complaint about it.
//!
//! Everything here degrades to plain lines when stdout is not a terminal, so
//! piped output and CI logs stay readable.

use std::borrow::Cow;
use std::time::Duration;

use indicatif::{HumanBytes, ProgressBar, ProgressStyle};

/// Tick interval for the spinners; fast enough to look alive, slow enough to
/// stay cheap over a slow link.
const TICK: Duration = Duration::from_millis(100);

fn is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// A step that shows a spinner while it runs and a checked line when it ends.
///
/// On a non-terminal stdout the spinner is hidden and only the final line is
/// printed, so logs get one line per step instead of a stream of redraws.
pub struct Step {
    bar: ProgressBar,
    quiet: bool,
    /// Off-terminal only: what the step announced at its start, printed if the
    /// step ends without a result line of its own.
    pending: Option<Cow<'static, str>>,
}

impl Step {
    /// Start a step; `message` is phrased as an action in progress ("Connecting to ...").
    pub fn start(message: impl Into<Cow<'static, str>>) -> Self {
        let message = message.into();
        if !is_terminal() {
            // Held rather than printed: a step that finishes with `done` prints
            // only its result, so a log gets one line per step instead of two.
            return Self {
                bar: ProgressBar::hidden(),
                quiet: true,
                pending: Some(message),
            };
        }
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            // The last tick char is what `finish_with_message` leaves behind,
            // so a finished step reads as a checkmark rather than a frozen frame.
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .expect("valid template")
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏✓"),
        );
        bar.set_message(message);
        bar.enable_steady_tick(TICK);
        Self {
            bar,
            quiet: false,
            pending: None,
        }
    }

    /// Replace the message of a running step.
    pub fn update(&self, message: impl Into<Cow<'static, str>>) {
        if !self.quiet {
            self.bar.set_message(message);
        }
    }

    /// Finish the step, replacing the spinner with a final line.
    pub fn done(self, message: impl Into<Cow<'static, str>>) {
        let message = message.into();
        if self.quiet {
            println!("{message}");
        } else {
            self.bar.finish_with_message(message);
        }
    }

    /// Finish the step without a result line of its own; the caller prints
    /// something more specific in its place. Off-terminal the step still
    /// announces what it was doing, so the output that follows has context.
    pub fn clear(self) {
        if self.quiet {
            if let Some(message) = &self.pending {
                println!("{message}");
            }
        } else {
            self.bar.finish_and_clear();
        }
    }
}

/// A determinate bar over a byte total, with a per-file message.
pub struct Transfer {
    bar: ProgressBar,
    quiet: bool,
}

impl Transfer {
    /// Start a transfer of `files` files totalling `bytes`.
    pub fn start(files: u64, bytes: u64) -> Self {
        let plural = if files == 1 { "" } else { "s" };
        if !is_terminal() {
            println!("Uploading {files} file{plural} ({}) ...", human_bytes(bytes));
            return Self {
                bar: ProgressBar::hidden(),
                quiet: true,
            };
        }
        let bar = ProgressBar::new(bytes);
        bar.set_style(
            ProgressStyle::with_template("{bar:28.cyan/blue} {bytes}/{total_bytes} · {bytes_per_sec} · eta {eta} {wide_msg}")
                .expect("valid template")
                .progress_chars("=> "),
        );
        bar.enable_steady_tick(TICK);
        Self { bar, quiet: false }
    }

    /// Name the file currently going over the wire.
    pub fn file(&self, name: &str) {
        if !self.quiet {
            self.bar.set_message(name.to_string());
        }
    }

    /// Account for uploaded bytes.
    pub fn advance(&self, bytes: u64) {
        if !self.quiet {
            self.bar.inc(bytes);
        }
    }

    /// Remove the bar; the caller prints the summary.
    pub fn finish(self) {
        if !self.quiet {
            self.bar.finish_and_clear();
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

#[cfg(test)]
mod tests {
    use super::human_bytes;

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
}
