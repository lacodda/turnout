//! Append-only log of what turnout did, one JSON object per line.
//!
//! The foundation for `status` and the future `report`: a command records the
//! fact it happened, never the values it handled. Secrets, logins and command
//! output stay out by construction - entries carry an action and the entity
//! names involved.
//!
//! Writing is best-effort: a journal that cannot be written must never fail
//! the command the user actually asked for.

use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths;

const JOURNAL_FILE: &str = "journal.jsonl";
/// Rotated at this size so the file stays readable and bounded; one previous
/// generation is kept as `journal.jsonl.1`.
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub struct Entry {
    /// UTC, ISO 8601 with second precision.
    pub at: String,
    /// What happened: `use`, `deploy`, `app.add`, ...
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Short outcome note - never command output or secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Record an action. Failures are swallowed: journaling is never the point of
/// the command being run.
pub fn record(action: &str, app: Option<&str>, server: Option<&str>, detail: Option<&str>) {
    let entry = Entry {
        at: now_iso8601(),
        action: action.to_string(),
        app: app.map(str::to_string),
        server: server.map(str::to_string),
        detail: detail.map(str::to_string),
    };
    let _ = append(&entry);
}

fn append(entry: &Entry) -> anyhow::Result<()> {
    let dir = paths::data_dir()?;
    // No data directory means turnout is not set up yet; nothing to journal.
    if !dir.is_dir() {
        return Ok(());
    }
    let path = dir.join(JOURNAL_FILE);
    if path.metadata().map(|m| m.len() >= MAX_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join(format!("{JOURNAL_FILE}.1")));
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// The most recent entries, oldest first. Unparsable lines are skipped so a
/// truncated write never breaks the reader.
pub fn tail(limit: usize) -> Vec<Entry> {
    let Ok(dir) = paths::data_dir() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(dir.join(JOURNAL_FILE)) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    entries
}

/// `YYYY-MM-DDTHH:MM:SSZ` from the system clock, without pulling in a date
/// library: days are converted with the shared civil-from-days arithmetic.
fn now_iso8601() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = crate::utils::civil_from_days(days as i64);
    let (hour, minute, second) = (rem / 3_600, (rem % 3_600) / 60, rem % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_have_the_expected_shape() {
        let stamp = now_iso8601();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(stamp.as_bytes()[4], b'-');
        assert_eq!(stamp.as_bytes()[10], b'T');
    }
}
