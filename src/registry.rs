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
//! A key names a job slot, not a run: `myapp.dev`, `myapp.build`, `gateway`,
//! `tunnel@db`. The next run of the same command takes over the slot - its
//! record and its log - which bounds both by the number of commands rather
//! than by time.

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
    /// What the job has noticed and is still seeing - the gateway's word on
    /// the stands behind it. Written by the job itself, as it changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<crate::signal::Signal>,
    /// `stop` is ending it. Written before the first signal, so the supervisor
    /// knows the end it is about to see was asked for.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
    /// What keeps a running job from doing its work right now - a tunnel
    /// whose SSH connection is down. Written by the job as it changes, gone
    /// once the job works again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trouble: Option<Trouble>,
}

/// Why a running job does not do its work, and since when.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Trouble {
    /// Seconds since the epoch.
    pub since: u64,
    pub reason: String,
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
        /// Where the gateway takes requests about itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        control: Option<Control>,
    },
    /// A tunnel: a port here, carried over SSH to an address the server
    /// reaches (see [`crate::tunnel`]).
    Tunnel(crate::tunnel::Route),
}

/// The gateway's control channel: a loopback port, and the token a request
/// must carry. The token is what keeps a web page out - any page can send a
/// request to a localhost port, none can read this file (ADR 0021).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Control {
    pub port: u16,
    pub token: String,
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
    /// The exit code; `None` when the process died by a signal, or never ran.
    pub code: Option<i32>,
    /// The program never started: its directory was missing, or there was no
    /// shell to run it with. A record from before v0.27 says only `code: None`
    /// for this, and reads as killed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub never_ran: bool,
}

impl Ended {
    pub fn exit(&self) -> Exit {
        match (self.never_ran, self.code) {
            (true, _) => Exit::NeverRan,
            (false, Some(code)) => Exit::Code(code),
            (false, None) => Exit::Killed,
        }
    }
}

/// How a job that ended by itself ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exit {
    /// It ran and returned this code.
    Code(i32),
    /// It ran and was ended by a signal.
    Killed,
    /// It never ran at all.
    NeverRan,
}

impl Exit {
    pub fn code(self) -> Option<i32> {
        match self {
            Exit::Code(code) => Some(code),
            Exit::Killed | Exit::NeverRan => None,
        }
    }

    /// What `ps`, a notification and a failed `--detach` call it.
    pub fn describe(self) -> String {
        match self {
            Exit::Code(code) => format!("exit {code}"),
            Exit::Killed => "killed".to_string(),
            Exit::NeverRan => "did not start".to_string(),
        }
    }
}

/// Where a job stands right now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// The owning process is alive.
    Running,
    /// It ended by itself and said how.
    Exited(Exit),
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
            signals: Vec::new(),
            stopping: false,
            trouble: None,
        }
    }

    pub fn key(&self) -> String {
        match &self.work {
            Work::Command { app, command } => command_key(app, command),
            Work::Gateway { .. } => GATEWAY.to_string(),
            Work::Tunnel(route) => tunnel_key(&route.name),
        }
    }

    pub fn status(&self) -> Status {
        if let Some(ended) = &self.ended {
            return Status::Exited(ended.exit());
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

    /// The app a job belongs to; `None` for the gateway and a tunnel.
    pub fn app(&self) -> Option<&str> {
        match &self.work {
            Work::Command { app, .. } => Some(app),
            Work::Gateway { .. } | Work::Tunnel(_) => None,
        }
    }

    /// The tunnel a record is, when it is one.
    pub fn tunnel(&self) -> Option<&crate::tunnel::Route> {
        match &self.work {
            Work::Tunnel(route) => Some(route),
            Work::Command { .. } | Work::Gateway { .. } => None,
        }
    }

    /// What `ps` and the messages call the job: `myapp dev`, `gateway`,
    /// `tunnel db` - the words `logs` and `stop` take.
    pub fn title(&self) -> String {
        match &self.work {
            Work::Command { app, command } => format!("{app} {command}"),
            Work::Gateway { .. } => GATEWAY.to_string(),
            Work::Tunnel(route) => format!("{TUNNEL} {}", route.name),
        }
    }
}

/// The word `ps`, `logs` and `stop` put in front of a tunnel's name.
pub const TUNNEL: &str = "tunnel";

/// The key of a tunnel's record.
///
/// No app's key can be one: an app's key always has its dot, and `@` is
/// escaped in it. Tunnel names are `[a-z0-9-]`, like the names of the
/// catalogs, so nothing here needs escaping.
pub fn tunnel_key(name: &str) -> String {
    format!("{TUNNEL}@{name}")
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

/// Where the copy of a failed run's log is kept: `failed/` beside the log.
///
/// A directory rather than a suffix: command names may hold dots, and
/// `myapp.build.failed.log` is also the log of a command called
/// `build.failed`. Beside the log rather than under [`logs_dir`], so a log
/// written before `TURNOUT_LOG_DIR` changed keeps its failure next to it.
pub fn failed_log(log: &Path) -> PathBuf {
    let name = log.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    log.parent().unwrap_or(Path::new("")).join("failed").join(name)
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
pub(crate) fn load_in(dir: &Path, key: &str) -> Result<Option<Entry>> {
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
    save_in(&jobs_dir()?, entry)
}

pub(crate) fn save_in(dir: &Path, entry: &Entry) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let key = entry.key();
    let path = record_path(dir, &key);
    let aside = dir.join(format!("{key}.json.{}.tmp", std::process::id()));
    std::fs::write(&aside, serde_json::to_string_pretty(entry)?).with_context(|| format!("cannot write {}", aside.display()))?;
    std::fs::rename(&aside, &path).with_context(|| format!("cannot write {}", path.display()))
}

/// How long a turnout waits for another to let go of a job slot. The slot is
/// held for a look and a write, milliseconds; this is for a machine under load.
const SLOT_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// A job slot held by this process for a moment - while it is, no other
/// turnout claims the slot or takes its record away. Let go when dropped.
///
/// Each record has one writer at a time, but deciding whether to write is a
/// read first: "is this job running?", then the claim. Two `turnout dev web`
/// started together both read "no", both truncated the same log, and the
/// second record overwrote the first. A look and the write that depends on
/// it happen under this lock.
///
/// An OS lock on `jobs/KEY.lock` (`flock` on Unix, `LockFileEx` on Windows)
/// rather than a file whose existence is the lock: the OS lets go of it when
/// its process dies, however it dies, so no lock is ever left stale for the
/// next turnout to judge by a pid. The file itself stays; removing it while
/// another process waits on it would hand two processes two different locks.
pub(crate) struct Slot {
    _file: std::fs::File,
}

pub(crate) fn lock_in(dir: &Path, key: &str) -> Result<Slot> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(format!("{key}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Slot { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < SLOT_PATIENCE => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
                "the record of {key} stayed locked by another turnout for {}s ({})",
                SLOT_PATIENCE.as_secs(),
                path.display()
            ),
            Err(std::fs::TryLockError::Error(err)) => return Err(err).with_context(|| format!("cannot lock {}", path.display())),
        }
    }
}

/// Write `entry` as the record of its slot, unless the slot's job is running.
///
/// The look and the write under the slot's lock, as [`crate::job::Job`]
/// claims a command's: two tunnels opened under one name at once must not
/// both believe they hold it.
pub fn claim(entry: &Entry) -> Result<()> {
    claim_in(&jobs_dir()?, entry)
}

pub(crate) fn claim_in(dir: &Path, entry: &Entry) -> Result<()> {
    let key = entry.key();
    let _slot = lock_in(dir, &key)?;
    if let Some(running) = load_in(dir, &key)?.filter(Entry::is_running) {
        anyhow::bail!("{} is already running (pid {})", running.title(), running.pid);
    }
    save_in(dir, entry)
}

/// Remove the record under `key`, if any. Never by the key alone from outside
/// this module: [`remove_if`] and [`remove_own`] say whose record it is.
fn remove_in(dir: &Path, key: &str) -> Result<()> {
    let path = record_path(dir, key);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("cannot remove {}", path.display())),
    }
}

/// Remove the record under `key` if it still is the one that was judged -
/// the process `pid` born at `birth`; returns whether it was.
///
/// What removes a record decided on it a while before: `stop` waits for the
/// job to go, the doctor runs its checks first. A new run may claim the slot
/// in that time, and removing by the key alone took the new run's record
/// away - a running job `ps` no longer showed and `stop` could not reach.
/// The look and the removal happen under the slot's lock, so no claim comes
/// between them.
pub fn remove_if(key: &str, pid: u32, birth: u64) -> Result<bool> {
    remove_if_in(&jobs_dir()?, key, pid, birth)
}

fn remove_if_in(dir: &Path, key: &str, pid: u32, birth: u64) -> Result<bool> {
    let _slot = lock_in(dir, key)?;
    match load_in(dir, key)? {
        Some(entry) if entry.pid == pid && entry.birth == birth => remove_in(dir, key).map(|()| true),
        _ => Ok(false),
    }
}

/// Say that the job `pid` born at `birth` is being stopped, if the record
/// under `key` still is that job's.
///
/// On Windows `stop` ends the job's whole tree at once, in no order it can
/// choose: when the child goes before the supervisor, the supervisor saw its
/// job end and sent the notification of an end nobody asked to hear about - a
/// job ended by `stop` says nothing (ADR 0019). Marking the record before the
/// first signal gives the supervisor something to read that is already there
/// whichever process dies first.
pub fn mark_stopping(key: &str, pid: u32, birth: u64) -> Result<()> {
    mark_stopping_in(&jobs_dir()?, key, pid, birth)
}

fn mark_stopping_in(dir: &Path, key: &str, pid: u32, birth: u64) -> Result<()> {
    let _slot = lock_in(dir, key)?;
    match load_in(dir, key)? {
        Some(mut entry) if entry.pid == pid && entry.birth == birth => {
            entry.stopping = true;
            save_in(dir, &entry)
        }
        _ => Ok(()),
    }
}

/// Whether this process's own job is being ended by `stop`: its record says
/// so, or is gone - `stop` removes it once the job is down.
pub fn being_stopped(key: &str) -> bool {
    jobs_dir().is_ok_and(|dir| being_stopped_in(&dir, key, std::process::id(), process::own_birth()))
}

fn being_stopped_in(dir: &Path, key: &str, pid: u32, birth: u64) -> bool {
    match load_in(dir, key) {
        Ok(Some(entry)) if entry.pid == pid && entry.birth == birth => entry.stopping,
        // Another run's record, or none: this job's was taken away by a stop.
        Ok(_) => true,
        // Unreadable: no grounds to keep quiet about how the job ended.
        Err(_) => false,
    }
}

/// Change this process's own record, if it still is this process's.
///
/// `stop` removes a record before (or while) its process dies, and the
/// process must not write it back on the way out. Checking the pid and the
/// birth first is what keeps a stopped job stopped - under the slot's lock,
/// so the removal cannot fall between the check and the write.
pub fn update_own(key: &str, change: impl FnOnce(&mut Entry)) -> Result<()> {
    let dir = jobs_dir()?;
    let _slot = lock_in(&dir, key)?;
    let Some(mut entry) = load_in(&dir, key)? else { return Ok(()) };
    if entry.pid != std::process::id() || entry.birth != process::own_birth() {
        return Ok(());
    }
    change(&mut entry);
    save_in(&dir, &entry)
}

/// Remove this process's own record, if it still is this process's.
pub fn remove_own(key: &str) -> Result<()> {
    remove_if(key, std::process::id(), process::own_birth()).map(|_| ())
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
        Work::Gateway { ports, front_port, control } => Some(crate::model::Gateway {
            pid: entry.pid,
            started: entry.started,
            ports,
            front_port,
            control,
            signals: entry.signals,
        }),
        Work::Command { .. } | Work::Tunnel(_) => None,
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
        // Nor is a tunnel's: an app's key never holds a bare `@`.
        assert_eq!(tunnel_key("db"), "tunnel@db");
        assert_ne!(command_key("tunnel", "db"), tunnel_key("db"));
        assert!(!command_key("tunnel@db", "x").contains('@'));
    }

    /// A slot is claimed only while nobody runs in it: a second tunnel under
    /// a running one's name is refused, and the record of one that is gone
    /// is taken over.
    #[test]
    fn a_slot_is_claimed_only_while_nobody_runs_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let route = crate::tunnel::Route {
            name: "db".into(),
            server: "staging".into(),
            credential: "deploy".into(),
            local_port: 5432,
            host: "localhost".into(),
            port: 5432,
        };
        let first = Entry::own(Work::Tunnel(route.clone()), true, None);
        claim_in(dir.path(), &first).unwrap();
        let mut other = route.clone();
        other.local_port = 15432;
        let error = claim_in(dir.path(), &Entry::own(Work::Tunnel(other.clone()), true, None))
            .unwrap_err()
            .to_string();
        assert!(error.contains("tunnel db is already running"), "{error}");
        assert_eq!(load_in(dir.path(), "tunnel@db").unwrap().unwrap().tunnel(), Some(&route));

        // The first one's process is gone: its slot is free again.
        let mut gone = first.clone();
        gone.birth = gone.birth.wrapping_add(1);
        save_in(dir.path(), &gone).unwrap();
        claim_in(dir.path(), &Entry::own(Work::Tunnel(other.clone()), true, None)).unwrap();
        assert_eq!(load_in(dir.path(), "tunnel@db").unwrap().unwrap().tunnel(), Some(&other));
    }

    /// Every kind of record reads back as what was written - the gateway's
    /// included, whose port map has numbers for keys.
    #[test]
    fn records_read_back_as_written() {
        let gateway = Work::Gateway {
            ports: BTreeMap::from([(7100, "web".to_string()), (7101, "api".to_string())]),
            front_port: Some(80),
            control: Some(Control {
                port: 51234,
                token: "0f".repeat(32),
            }),
        };
        let command = Work::Command {
            app: "myapp".into(),
            command: "test:e2e".into(),
        };
        let tunnel = Work::Tunnel(crate::tunnel::Route {
            name: "db".into(),
            server: "staging".into(),
            credential: "deploy".into(),
            local_port: 15432,
            host: "db.internal".into(),
            port: 5432,
        });
        for work in [gateway, command, tunnel] {
            let mut entry = Entry::own(work.clone(), true, Some(PathBuf::from("x.log")));
            entry.ready = Some(Ready {
                at: 5,
                url: Some("http://a.localhost".into()),
            });
            entry.ended = Some(Ended {
                at: 9,
                code: None,
                never_ran: false,
            });
            entry.signals = vec![crate::signal::Signal {
                kind: crate::signal::Kind::StandDown,
                app: "web".into(),
                server: "prod".into(),
                status: None,
                since: 7,
            }];
            let text = serde_json::to_string(&entry).unwrap();
            let back: Entry = serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"));
            assert_eq!(back.work, work);
            assert_eq!((back.pid, back.birth, back.detached), (entry.pid, entry.birth, true));
            assert_eq!(back.ready.unwrap().url.as_deref(), Some("http://a.localhost"));
            assert_eq!(back.ended.unwrap().code, None);
            assert_eq!(back.signals, entry.signals);
            assert_eq!(back.trouble, None);
        }
        // A trouble reads back too, and a record without one never writes it.
        let mut entry = Entry::own(
            Work::Command {
                app: "myapp".into(),
                command: "dev".into(),
            },
            true,
            None,
        );
        assert!(!serde_json::to_string(&entry).unwrap().contains("trouble"));
        entry.trouble = Some(Trouble {
            since: 4,
            reason: "staging.example.com:22 did not answer".into(),
        });
        let back: Entry = serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(back.trouble, entry.trouble);
    }

    /// A record goes only while it is the one that was judged. `stop` waits
    /// for a job before it takes the record away, and a new run that claimed
    /// the slot in the meantime used to lose its record with it.
    #[test]
    fn a_record_is_removed_only_while_it_is_the_one_judged() {
        let dir = tempfile::tempdir().unwrap();
        let work = Work::Command {
            app: "myapp".into(),
            command: "dev".into(),
        };
        let judged = Entry::own(work, true, None);
        save_in(dir.path(), &judged).unwrap();
        // A new run took the slot while the old one was being stopped.
        let mut newer = judged.clone();
        newer.pid = judged.pid.wrapping_add(1);
        newer.birth = judged.birth.wrapping_add(1);
        save_in(dir.path(), &newer).unwrap();

        let key = judged.key();
        assert!(!remove_if_in(dir.path(), &key, judged.pid, judged.birth).unwrap());
        let left = load_in(dir.path(), &key).unwrap().expect("the new run's record was removed");
        assert_eq!((left.pid, left.birth), (newer.pid, newer.birth));
        // The same pid with another birth is not the judged record either.
        assert!(!remove_if_in(dir.path(), &key, newer.pid, judged.birth).unwrap());
        assert!(load_in(dir.path(), &key).unwrap().is_some());

        assert!(remove_if_in(dir.path(), &key, newer.pid, newer.birth).unwrap());
        assert!(load_in(dir.path(), &key).unwrap().is_none());
        // Nothing to remove is not an error.
        assert!(!remove_if_in(dir.path(), &key, newer.pid, newer.birth).unwrap());
    }

    /// `stop` marks the job before it signals, and the supervisor reads the
    /// mark whichever process of the tree dies first; only the judged job's
    /// record is marked, and a record taken away counts as a stop too.
    #[test]
    fn a_stop_is_known_to_the_job_before_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let work = Work::Command {
            app: "myapp".into(),
            command: "serve".into(),
        };
        let own = Entry::own(work, true, None);
        let key = own.key();
        save_in(dir.path(), &own).unwrap();
        assert!(
            !being_stopped_in(dir.path(), &key, own.pid, own.birth),
            "a job nobody stops is not being stopped"
        );

        // Another process's mark does not land on this record.
        mark_stopping_in(dir.path(), &key, own.pid.wrapping_add(1), own.birth).unwrap();
        assert!(!being_stopped_in(dir.path(), &key, own.pid, own.birth));

        mark_stopping_in(dir.path(), &key, own.pid, own.birth).unwrap();
        assert!(being_stopped_in(dir.path(), &key, own.pid, own.birth));

        remove_in(dir.path(), &key).unwrap();
        assert!(being_stopped_in(dir.path(), &key, own.pid, own.birth), "a record stop took away");
        // A mark on a record that is gone writes nothing back.
        mark_stopping_in(dir.path(), &key, own.pid, own.birth).unwrap();
        assert!(load_in(dir.path(), &key).unwrap().is_none());
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
        entry.ended = Some(Ended {
            at: now(),
            code: Some(3),
            never_ran: false,
        });
        assert_eq!(entry.status(), Status::Exited(Exit::Code(3)));
        entry.ended = Some(Ended {
            at: now(),
            code: None,
            never_ran: false,
        });
        assert_eq!(entry.status(), Status::Exited(Exit::Killed));
        entry.ended = Some(Ended {
            at: now(),
            code: None,
            never_ran: true,
        });
        assert_eq!(entry.status(), Status::Exited(Exit::NeverRan));
        assert_eq!(Exit::NeverRan.describe(), "did not start");

        // A record of a turnout before v0.27 has no `never_ran`, and one that
        // ran never writes it.
        let old: Ended = serde_json::from_str(r#"{"at": 1, "code": null}"#).unwrap();
        assert_eq!(old.exit(), Exit::Killed);
        let ran = serde_json::to_string(&Ended {
            at: 1,
            code: Some(0),
            never_ran: false,
        })
        .unwrap();
        assert!(!ran.contains("never_ran"), "{ran}");
    }
}
