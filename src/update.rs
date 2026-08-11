//! Quiet check for a newer release, and the hint that follows it.
//!
//! The rules this follows, in the order they matter:
//!
//! 1. **Never delay a command.** The network call happens in a detached
//!    background process (`turnout check-update`, hidden); the foreground only
//!    ever reads a cached file. A hint therefore describes what the *previous*
//!    run found, which is the price of never making the user wait.
//! 2. **Never fail a command.** Every step here is best-effort: no data
//!    directory, no network, garbage in the cache - all of it degrades to
//!    "say nothing", exactly like [`crate::journal`].
//! 3. **Never nag.** At most once a day, only on a terminal, and only when
//!    there is something newer to report.
//!
//! The tag comes from the `/releases/latest` redirect on github.com, not from
//! `api.github.com`: unauthenticated API calls are capped at 60 per hour per
//! IP address, so behind shared NAT the check would fail for everyone at once.
//! That is the same trap the installers hit in v0.4.1.

use std::io::IsTerminal;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Set to `0`/`false`/`no`/`off` to silence the check entirely.
pub const ENV_DISABLE: &str = "TURNOUT_UPDATE_CHECK";
/// Where the redirect is followed from; overridden by tests.
pub const ENV_URL: &str = "TURNOUT_UPDATE_URL";

const CACHE_FILE: &str = "update-check.json";
const LATEST_URL: &str = "https://github.com/lacodda/turnout/releases/latest";
/// One check a day: new releases are rarer than commands.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// The background process is not worth a long wait; it will try again tomorrow.
const TIMEOUT: Duration = Duration::from_secs(10);

/// What the last check found. `checked_at` is written even when the lookup
/// failed, so a machine that is offline retries tomorrow rather than on every
/// single command.
#[derive(Serialize, Deserialize, Default)]
pub struct Cache {
    /// Unix seconds of the last attempt, successful or not.
    pub checked_at: u64,
    /// Latest version seen upstream, without the `v` prefix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
}

/// Print the hint, then refresh the cache in the background if it is stale.
///
/// Called once, after a command has done its work and printed its own output:
/// the notice belongs at the bottom, and a command that failed should not be
/// crowned with unrelated news.
pub fn hint_and_refresh() {
    if !enabled() {
        return;
    }
    let cache = load().unwrap_or_default();
    // Only on a terminal: piped output is usually being parsed by something.
    if let Some(hint) = hint_for(&cache, current_version(), std::io::stderr().is_terminal()) {
        eprint!("{hint}");
    }
    if is_stale(&cache) {
        spawn_background_check();
    }
}

/// The notice to print, or `None` to stay quiet.
///
/// Split out from the printing so every reason for silence is testable
/// without a terminal: no cached version, nothing newer, or output that is
/// being piped somewhere.
fn hint_for(cache: &Cache, current: &str, is_terminal: bool) -> Option<String> {
    if !is_terminal {
        return None;
    }
    let latest = cache.latest.as_deref()?;
    is_newer(latest, current)
        .then(|| format!("\nturnout {latest} is available (you have {current}).\n  Update with `cargo install turnout` or see {LATEST_URL}\n"))
}

/// Perform the lookup and write the cache. This is the body of the hidden
/// `check-update` command the background process runs.
pub fn check_now() -> Result<()> {
    let latest = fetch_latest();
    // The timestamp is recorded either way - see [`Cache::checked_at`].
    let cache = Cache {
        checked_at: now_secs(),
        latest: latest.or_else(|| load().ok().and_then(|c| c.latest)),
    };
    save(&cache)
}

/// Whether the check is switched on. Off when explicitly disabled, and off in
/// CI, where nobody reads the hint and every run is a fresh machine.
fn enabled() -> bool {
    if std::env::var_os("CI").is_some() {
        return false;
    }
    match std::env::var(ENV_DISABLE) {
        Ok(value) => !matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"),
        Err(_) => true,
    }
}

fn is_stale(cache: &Cache) -> bool {
    now_secs().saturating_sub(cache.checked_at) >= INTERVAL.as_secs()
}

/// Re-run ourselves detached so the current command can exit immediately.
///
/// Failure is silent and harmless: the cache simply stays stale and the next
/// command tries again.
fn spawn_background_check() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = std::process::Command::new(exe);
    command
        .arg("check-update")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        // Without this the child flashes a console window on every check.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    let _ = command.spawn();
}

/// The latest tag, or `None` if anything at all went wrong.
fn fetch_latest() -> Option<String> {
    let url = std::env::var(ENV_URL).unwrap_or_else(|_| LATEST_URL.to_string());
    let location = redirect_target(&url).ok()?;
    let tag = location.rsplit('/').next()?;
    parse_tag(tag)
}

/// Ask for `/releases/latest` without following the redirect and read the tag
/// out of the `Location` header.
fn redirect_target(url: &str) -> Result<String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .user_agent(concat!("turnout/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let response = client.get(url).send().await.context("cannot reach github.com")?;
        let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
            bail!("no Location header on {url}");
        };
        Ok(location.to_str().context("Location is not valid text")?.to_string())
    })
}

/// `v0.4.1` -> `0.4.1`; anything that is not a version tag is rejected so a
/// redirect to a login page never becomes a "new version".
fn parse_tag(tag: &str) -> Option<String> {
    let version = tag.strip_prefix('v')?;
    let mut parts = version.split('.');
    let mut numbers = 0;
    for part in parts.by_ref().take(3) {
        // Trailing pre-release markers (`1-rc.1`) keep the tag usable.
        let digits = part.split(['-', '+']).next().unwrap_or_default();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        numbers += 1;
    }
    (numbers == 3).then(|| version.to_string())
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Compare `major.minor.patch` numerically. A version carrying a pre-release
/// suffix loses to the same numbers without one, per semver.
fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_semver(candidate), parse_semver(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// `(major, minor, patch, is_final)` - the flag makes `1.0.0` sort above `1.0.0-rc.1`.
fn parse_semver(version: &str) -> Option<(u64, u64, u64, bool)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, !version.contains('-')))
}

fn cache_path() -> Result<std::path::PathBuf> {
    Ok(paths::data_dir()?.join(CACHE_FILE))
}

fn load() -> Result<Cache> {
    let text = std::fs::read_to_string(cache_path()?)?;
    Ok(serde_json::from_str(&text)?)
}

fn save(cache: &Cache) -> Result<()> {
    let dir = paths::data_dir()?;
    // No data directory means turnout is not set up yet; nothing to cache.
    if !dir.is_dir() {
        return Ok(());
    }
    let path = dir.join(CACHE_FILE);
    let tmp = dir.join(format!("{CACHE_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_string_pretty(cache)?).with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_become_versions() {
        assert_eq!(parse_tag("v0.4.1").as_deref(), Some("0.4.1"));
        assert_eq!(parse_tag("v1.0.0-rc.1").as_deref(), Some("1.0.0-rc.1"));
    }

    /// A redirect that does not end in a version tag must not be mistaken for
    /// a release - that is how a login or error page becomes a fake update.
    #[test]
    fn non_tags_are_rejected() {
        assert_eq!(parse_tag("login"), None);
        assert_eq!(parse_tag("v0.4"), None);
        assert_eq!(parse_tag("0.4.1"), None);
        assert_eq!(parse_tag("vNEXT.0.0"), None);
        assert_eq!(parse_tag(""), None);
    }

    #[test]
    fn newer_versions_win() {
        assert!(is_newer("0.5.0", "0.4.1"));
        assert!(is_newer("0.4.2", "0.4.1"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.4.1", "0.4.1"));
        assert!(!is_newer("0.4.0", "0.4.1"));
    }

    /// Numbers are compared as numbers; a string comparison would rank 0.10.0
    /// below 0.9.0 and go quiet exactly when an update matters.
    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.4.10", "0.4.9"));
    }

    #[test]
    fn a_release_beats_its_own_prerelease() {
        assert!(is_newer("1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
    }

    /// Garbage on either side means "say nothing" rather than a wrong hint.
    #[test]
    fn unparsable_versions_never_announce() {
        assert!(!is_newer("next", "0.4.1"));
        assert!(!is_newer("0.5.0", "unknown"));
        assert!(!is_newer("0.5.0.1", "0.4.1"));
    }

    fn cache_with(latest: &str) -> Cache {
        Cache {
            checked_at: now_secs(),
            latest: Some(latest.to_string()),
        }
    }

    #[test]
    fn a_newer_release_is_announced() {
        let hint = hint_for(&cache_with("0.5.0"), "0.4.1", true).expect("hint");
        assert!(hint.contains("0.5.0 is available"), "{hint}");
        assert!(hint.contains("you have 0.4.1"), "{hint}");
    }

    /// Piped output is usually being read by a script or another tool, and a
    /// version notice in the middle of it is noise at best.
    #[test]
    fn piped_output_stays_clean() {
        assert!(hint_for(&cache_with("0.5.0"), "0.4.1", false).is_none());
    }

    #[test]
    fn nothing_to_report_stays_quiet() {
        assert!(hint_for(&cache_with("0.4.1"), "0.4.1", true).is_none());
        assert!(hint_for(&cache_with("0.4.0"), "0.4.1", true).is_none());
        // Never checked yet: no version cached, nothing to say.
        assert!(hint_for(&Cache::default(), "0.4.1", true).is_none());
    }

    #[test]
    fn staleness_follows_the_interval() {
        let fresh = Cache {
            checked_at: now_secs(),
            latest: None,
        };
        assert!(!is_stale(&fresh));
        let old = Cache {
            checked_at: now_secs() - INTERVAL.as_secs() - 1,
            latest: None,
        };
        assert!(is_stale(&old));
        // A cache that was never written must trigger the first check.
        assert!(is_stale(&Cache::default()));
    }
}
