//! The registry of jobs: what turnout has running, and what it ran last.
//!
//! One file per job under `jobs/` in the data directory, named after the job's
//! key. Not one shared file: a detached job writes its own record while it
//! runs - when it comes up, when it ends - and so do foreground jobs and the
//! gateway, all at once and from different processes. A shared file would be
//! a read-modify-write race between all of them, where the loser silently
//! drops somebody else's record. With a file each, every record has exactly
//! one writer at a time and a write is a rename.
//!
//! A key names a job slot, not a run: `myapp.dev`, `myapp.build`, `gateway`.
//! The next run of the same command takes over the slot - its record and its
//! log - which bounds both by the number of commands rather than by time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::process;

/// The key of the gateway's record.
pub const GATEWAY: &str = "gateway";

/// Where job logs go instead of `logs/` under the data directory.
pub const LOG_DIR_ENV: &str = "TURNOUT_LOG_DIR";

/// One job's record.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// What runs.
    pub work: Work,
    /// The process that owns the job: turnout itself for a foreground job,
    /// the supervisor for a detached one, the gateway for the gateway.
    pub pid: u32,
    /// When that process started, as the OS tells it - the only thing that
    /// makes the pid mean the same process tomorrow (see [`process`]).
    pub birth: u64,
    /// The process group to signal on Unix, when the job has one to itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<i32>,
    /// Seconds since the epoch.
    pub started: u64,
    /// Started with `--detach` (or by `gateway start`): nobody's terminal is
    /// waiting on it.
    #[serde(default)]
    pub detached: bool,
    /// Where the job's output goes; `None` when it goes to a terminal only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<PathBuf>,
    /// When a server job said it was up, and the address it settled on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<Ready>,
    /// How the job ended, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// What a job runs.
///
/// Externally tagged (`{"gateway": {...}}`), not internally (`"kind":
/// "gateway"`): serde reads an internally tagged enum through a buffer that
/// keeps every map key as a string, and the gateway's port map has numbers
/// for keys - the record would be written fine and never read back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Work {
    /// A command of an app - `dev`, `build`, a custom one, or `deploy`.
    Command { app: String, command: String },
    /// The gateway, with what it is listening on.
    Gateway {
        /// Listening port per app at the moment the gateway started.
        ports: BTreeMap<u16, String>,
        /// Where the front door opened, when it did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        front_port: Option<u16>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ready {
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Ended {
    pub at: u64,
    /// The exit code; `None` when the process died by a signal.
    pub code: Option<i32>,
}

/// Where a job stands right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The owning process is alive.
    Running,
    /// It ended by itself and said how.
    Exited(Option<i32>),
    /// The process is gone without saying how: killed from outside, a crash,
    /// a reboot.
    Gone,
}

impl Entry {
    /// A fresh record for this process, about to run `work`.
    pub fn own(work: Work, detached: bool, log: Option<PathBuf>) -> Self {
        Self {
            work,
            pid: std::process::id(),
            birth: process::own_birth(),
            group: process::own_group(),
            started: now(),
            detached,
            log,
            ready: None,
            ended: None,
        }
    }

    pub fn key(&self) -> String {
        match &self.work {
            Work::Command { app, command } => command_key(app, command),
            Work::Gateway { .. } => GATEWAY.to_string(),
        }
    }

    pub fn status(&self) -> Status {
        if let Some(ended) = &self.ended {
            return Status::Exited(ended.code);
        }
        if process::is_alive(self.pid, self.birth) {
            Status::Running
        } else {
            Status::Gone
        }
    }

    pub fn is_running(&self) -> bool {
        self.status() == Status::Running
    }

    /// The app a job belongs to; `None` for the gateway.
    pub fn app(&self) -> Option<&str> {
        match &self.work {
            Work::Command { app, .. } => Some(app),
            Work::Gateway { .. } => None,
        }
    }

    /// What `ps` and the messages call the job: `myapp dev`, `gateway`.
    pub fn title(&self) -> String {
        match &self.work {
            Work::Command { app, command } => format!("{app} {command}"),
            Work::Gateway { .. } => GATEWAY.to_string(),
        }
    }
}

/// The key of an app's command.
///
/// App names are `[a-z0-9-]`, so the first dot always ends the app and two
/// apps can never collide - `a` running `b-c` and `a-b` running `c` used to
/// share `a-b-c.log`. The command is escaped rather than squashed: `test:e2e`
/// and `test-e2e` are two npm scripts, and they get two keys.
pub fn command_key(app: &str, command: &str) -> String {
    format!("{}.{}", escape(app), escape(command))
}

/// A name made safe for a file name on every platform turnout runs on,
/// without losing what it was.
///
/// Anything outside `[A-Za-z0-9._-]` becomes `%XX` per byte - a colon is an
/// alternate data stream on Windows, a slash a directory everywhere. A name
/// Windows reserves for a device (`con`, `nul`, `com1`) keeps that meaning
/// even with an extension on older systems, so its first letter is escaped.
fn escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    if out.is_empty() {
        return "%".to_string();
    }
    let lower = out.to_ascii_lowercase();
    let reserved = matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || (lower.len() == 4 && (lower.starts_with("com") || lower.starts_with("lpt")) && lower.as_bytes()[3].is_ascii_digit());
    if reserved {
        out = format!("%{:02X}{}", out.as_bytes()[0], &out[1..]);
    }
    out
}

pub fn jobs_dir() -> Result<PathBuf> {
    Ok(crate::paths::data_dir()?.join("jobs"))
}

/// The directory job logs go to: `TURNOUT_LOG_DIR`, or `logs/` under the data
/// directory.
pub fn logs_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(LOG_DIR_ENV).filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(crate::paths::data_dir()?.join("logs"))
}

/// The log file of the job with `key`.
pub fn log_path(key: &str) -> Result<PathBuf> {
    Ok(logs_dir()?.join(format!("{key}.log")))
}

fn record_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// The record under `key`, if there is a readable one.
pub fn load(key: &str) -> Result<Option<Entry>> {
    load_in(&jobs_dir()?, key)
}

/// A record that does not parse is reported and treated as absent: it is one
/// job's problem, and refusing every command over it would be the wrong
/// trade - but saying nothing would hide a running job from `ps` and `stop`.
fn load_in(dir: &Path, key: &str) -> Result<Option<Entry>> {
    let path = record_path(dir, key);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(entry) => Ok(Some(entry)),
            Err(err) => {
                crate::progress::warn(&format!("ignoring the unreadable job record {}: {err}", path.display()));
                Ok(None)
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Every record, in key order.
pub fn list() -> Result<Vec<Entry>> {
    let dir = jobs_dir()?;
    let reader = match std::fs::read_dir(&dir) {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut keys: Vec<String> = reader
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().to_str().and_then(|name| name.strip_suffix(".json")).map(str::to_string))
        .collect();
    keys.sort();
    let mut entries = Vec::new();
    for key in keys {
        if let Some(entry) = load_in(&dir, &key)? {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// Write a record, replacing whatever the slot held.
///
/// Written aside and renamed over, so a reader never sees half a record and a
/// crash mid-write leaves the previous one intact.
pub fn save(entry: &Entry) -> Result<()> {
    let dir = jobs_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let key = entry.key();
    let path = record_path(&dir, &key);
    let aside = dir.join(format!("{key}.json.{}.tmp", std::process::id()));
    std::fs::write(&aside, serde_json::to_string_pretty(entry)?).with_context(|| format!("cannot write {}", aside.display()))?;
    std::fs::rename(&aside, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// Remove the record under `key`, if any.
pub fn remove(key: &str) -> Result<()> {
    let path = record_path(&jobs_dir()?, key);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("cannot remove {}", path.display())),
    }
}

/// Change this process's own record, if it still is this process's.
///
/// `stop` removes a record before (or while) its process dies, and the
/// process must not write it back on the way out. Checking the pid and the
/// birth first is what keeps a stopped job stopped.
pub fn update_own(key: &str, change: impl FnOnce(&mut Entry)) -> Result<()> {
    let Some(mut entry) = load(key)? else { return Ok(()) };
    if entry.pid != std::process::id() || entry.birth != process::own_birth() {
        return Ok(());
    }
    change(&mut entry);
    save(&entry)
}

/// Remove this process's own record, if it still is this process's.
pub fn remove_own(key: &str) -> Result<()> {
    match load(key)? {
        Some(entry) if entry.pid == std::process::id() && entry.birth == process::own_birth() => remove(key),
        _ => Ok(()),
    }
}

/// The running gateway, as the rest of turnout sees it.
///
/// A record the gateway left behind by dying is not a gateway: callers that
/// want to route through it or print its door get `None`.
pub fn gateway() -> Result<Option<crate::model::Gateway>> {
    Ok(load(GATEWAY)?.filter(Entry::is_running).and_then(as_gateway))
}

/// A record as the gateway it describes, whether or not it still runs.
pub fn as_gateway(entry: Entry) -> Option<crate::model::Gateway> {
    match entry.work {
        Work::Gateway { ports, front_port } => Some(crate::model::Gateway {
            pid: entry.pid,
            ports,
            front_port,
        }),
        Work::Command { .. } => None,
    }
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key is a file name on every platform, and different names never
    /// share one.
    #[test]
    fn keys_are_file_names_and_never_collide() {
        assert_eq!(command_key("myapp", "dev"), "myapp.dev");
        assert_eq!(command_key("myapp", "test:e2e"), "myapp.test%3Ae2e");
        assert_ne!(command_key("myapp", "test:e2e"), command_key("myapp", "test-e2e"));
        // The collision the old `{app}-{command}` naming had.
        assert_ne!(command_key("a", "b-c"), command_key("a-b", "c"));
        assert_eq!(command_key("myapp", "../etc/passwd"), "myapp...%2Fetc%2Fpasswd");
        assert_eq!(command_key("myapp", "a b"), "myapp.a%20b");
        assert_eq!(command_key("myapp", ""), "myapp.%");
        // Device names Windows reserves stop being device names.
        assert_eq!(escape("nul"), "%6Eul");
        assert_eq!(escape("COM1"), "%43OM1");
        assert_eq!(escape("com"), "com");
        assert_eq!(escape("console"), "console");
        // The gateway's key is not one any app can produce: app keys carry a dot.
        assert!(!command_key("gateway", "x").eq(GATEWAY));
    }

    /// Every kind of record reads back as what was written - the gateway's
    /// included, whose port map has numbers for keys.
    #[test]
    fn records_read_back_as_written() {
        let gateway = Work::Gateway {
            ports: BTreeMap::from([(7100, "web".to_string()), (7101, "api".to_string())]),
            front_port: Some(80),
        };
        let command = Work::Command {
            app: "myapp".into(),
            command: "test:e2e".into(),
        };
        for work in [gateway, command] {
            let mut entry = Entry::own(work.clone(), true, Some(PathBuf::from("x.log")));
            entry.ready = Some(Ready {
                at: 5,
                url: Some("http://a.localhost".into()),
            });
            entry.ended = Some(Ended { at: 9, code: None });
            let text = serde_json::to_string(&entry).unwrap();
            let back: Entry = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"));
            assert_eq!(back.work, work);
            assert_eq!((back.pid, back.birth, back.detached), (entry.pid, entry.birth, true));
            assert_eq!(back.ready.unwrap().url.as_deref(), Some("http://a.localhost"));
            assert_eq!(back.ended.unwrap().code, None);
        }
    }

    /// A record whose process is this one reads as running; one whose birth
    /// is off reads as gone; one that says it ended reads as ended, whatever
    /// its process is doing.
    #[test]
    fn a_record_knows_whether_its_process_is_the_one_it_names() {
        let work = Work::Command {
            app: "myapp".into(),
            command: "dev".into(),
        };
        let mut entry = Entry::own(work, false, None);
        assert_eq!(entry.status(), Status::Running);
        entry.birth = entry.birth.wrapping_add(1);
        assert_eq!(entry.status(), Status::Gone);
        entry.ended = Some(Ended { at: now(), code: Some(3) });
        assert_eq!(entry.status(), Status::Exited(Some(3)));
    }
}
