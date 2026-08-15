use std::future::Future;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::Server;

/// Normalize a project directory for storing and for spawning tools in it.
/// Canonicalized via dunce so the result carries no `\\?\` prefix; this also
/// fixes a lowercase drive letter, which would otherwise reach dev servers as
/// a lowercase cwd and break them (Vite resolves imports against it).
pub fn project_dir(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    if !path.is_dir() {
        bail!("directory {} does not exist", path.display());
    }
    dunce::canonicalize(&path).with_context(|| format!("cannot canonicalize {}", path.display()))
}

/// Run a shell command line in a directory, streaming output to the terminal.
pub fn run_in_dir(command_line: &str, dir: &Path) -> Result<std::process::ExitStatus> {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", command_line]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", command_line]);
        command
    };
    command.current_dir(dir);
    let mut child = command.spawn().with_context(|| format!("cannot run '{command_line}'"))?;
    // The whole tree dies with turnout, not only the direct child; and while
    // the child runs, Ctrl+C belongs to it (see `term`).
    crate::term::confine(&child);
    crate::term::child_begin();
    let status = child.wait();
    crate::term::child_end();
    status.with_context(|| format!("cannot run '{command_line}'"))
}

/// Days since 1970-01-01 to a calendar date, by Howard Hinnant's `civil_from_days`.
///
/// Written out rather than pulled in: a date crate would be a dependency for
/// two timestamps - the backup names in `remote` and the journal entries - and
/// this is the same arithmetic every one of them performs.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    // March is month 0 in this scheme; roll it back to the calendar.
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Drive one async operation to completion from synchronous code.
///
/// The sync command layer needs the network a handful of times per run at
/// most (a reachability probe, an update check, a release download); each
/// call gets a fresh current-thread runtime rather than the process keeping a
/// global one alive for work this rare. The SSH transport and the gateway own
/// their runtimes separately - theirs live as long as a connection does.
pub fn run_blocking<T>(future: impl Future<Output = Result<T>>) -> Result<T> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start a runtime")?;
    runtime.block_on(future)
}

/// Best-effort reachability probe used by `use` before binding.
pub fn check_reachable(server: &Server) -> Result<reqwest::StatusCode> {
    run_blocking(async {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(server.accept_invalid_certs)
            .timeout(std::time::Duration::from_secs(4))
            .build()?;
        let response = client.get(&server.url).send().await.with_context(|| format!("{} is unreachable", server.url))?;
        Ok(response.status())
    })
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    /// Dates the tooling will actually see, plus the leap-year cases that
    /// catch a wrong civil-date conversion.
    #[test]
    fn days_since_the_epoch_become_calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29: a leap year despite being divisible by 100.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        // 2000-03-01, just past that leap day.
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        // 2024-02-29, the ordinary leap case.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_313), (2025, 8, 13));
    }
}
