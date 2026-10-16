//! The record of every deploy: what went where, built from what, and how it
//! ended - and the builds a rollback sends again.
//!
//! One file per deploy under `deploys/`, named by its number, and the number
//! is claimed by creating that file: two background deploys finishing at once
//! cannot take the same one, the way the job registry keeps a file per job
//! (ADR 0018). The builds sit beside them in `deploys/builds/`, named by the
//! SHA-256 of the archive, so a build sent to staging and then to production
//! is kept once, and a rollback's record points at the very build it sent
//! again (ADR 0027).
//!
//! Unlike the journal, a record keeps command output - the restart's - the way
//! a job's log does: it stays on this machine, in the data directory.

use std::collections::{BTreeSet, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

/// Under the data directory.
const DIR: &str = "deploys";
const BUILDS: &str = "builds";

/// Builds kept for each stand: those of its newest deploys that wrote files.
/// Enough to go back a week or two of daily deploys; the records themselves
/// are kept for good - they are small, and the history is the point.
pub const KEEP_BUILDS: usize = 10;

/// A build younger than this is never pruned: a deploy writes its build just
/// before its record, and a prune running in another process in between must
/// not take a build nothing points at yet.
const FRESH: Duration = Duration::from_secs(3600);

/// What a record keeps of a restart's output: its last lines - enough to see
/// what it said or why it failed, bounded so a chatty image build does not
/// turn a record into a log.
const OUTPUT_LINES: usize = 100;
const OUTPUT_BYTES: usize = 16 * 1024;

/// How a deploy ended, as far as the stand is concerned.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Through: the stand runs this build.
    Deployed,
    /// The check failed, and the stand was put back on what it ran before.
    RolledBack,
    /// The files are on the server, and what had to follow them did not
    /// finish - the restart, or a check with nothing to roll back to. From
    /// outside this looks like a deploy that worked and a stand that did not
    /// pick it up.
    Partial,
    /// Nothing was written.
    Failed,
}

impl Outcome {
    /// Whether the deploy left its files on the server.
    pub fn wrote(self) -> bool {
        matches!(self, Outcome::Deployed | Outcome::Partial)
    }

    pub fn word(self) -> &'static str {
        match self {
            Outcome::Deployed => "deployed",
            Outcome::RolledBack => "rolled_back",
            Outcome::Partial => "partial",
            Outcome::Failed => "failed",
        }
    }
}

/// The commit a build was made from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    /// The working tree had changes the commit does not hold.
    pub dirty: bool,
}

impl Commit {
    /// `1a2b3c4`, or `1a2b3c4-dirty` the way `git describe --dirty` says it.
    pub fn short(&self) -> String {
        let sha: String = self.sha.chars().take(7).collect();
        if self.dirty { format!("{sha}-dirty") } else { sha }
    }
}

/// Where a build came from: the project's version and commit when it was made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    pub version: Option<String>,
    pub commit: Option<Commit>,
}

/// What the post-write command did.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Restart {
    pub command: String,
    pub ok: bool,
    /// The last lines it printed; when it failed, the error that says so.
    pub output: String,
}

impl Restart {
    pub fn new(command: &str, result: &Result<String>) -> Self {
        let (ok, output) = match result {
            Ok(output) => (true, output.clone()),
            Err(err) => (false, format!("{err:#}")),
        };
        Self {
            command: command.to_string(),
            ok,
            output: tail(&output),
        }
    }
}

/// What the health check answered: the answer that passed, or the last one
/// that did not.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub ok: bool,
    pub answer: String,
}

/// One deploy, as it is kept.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Record {
    pub id: u64,
    /// When the deploy reached for the server, UTC.
    pub at: String,
    pub app: String,
    /// The named target, when the deploy went through one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub server: String,
    pub credential: String,
    pub path: String,
    /// The directory on the server: with the server, what names the stand.
    pub dir: String,
    pub files: u64,
    pub bytes: u64,
    /// How long the server's part took - from connecting to the check.
    pub millis: u64,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<Commit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<Restart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Check>,
    /// SHA-256 of the build's archive, kept under `builds/` while it is among
    /// the stand's recent ones; only a deploy that wrote files has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// The deploy whose build this one sent again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback_of: Option<u64>,
}

impl Record {
    /// Whether this deploy went to the stand `server` + `dir`.
    pub fn on(&self, server: &str, dir: &str) -> bool {
        self.server == server && same_dir(&self.dir, dir)
    }
}

/// Two spellings of one remote directory: a trailing separator is not a
/// different place.
pub fn same_dir(a: &str, b: &str) -> bool {
    a.trim_end_matches(['/', '\\']) == b.trim_end_matches(['/', '\\'])
}

/// `12` or `#12`, the way a person copies it out of the table.
pub fn parse_id(text: &str) -> Result<u64> {
    text.trim()
        .trim_start_matches('#')
        .parse()
        .map_err(|_| anyhow::anyhow!("'{text}' is not a deploy number - `turnout history` lists them"))
}

/// The records and builds of this machine's deploys.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// The store in the data directory.
    pub fn open() -> Result<Self> {
        Ok(Self::at(paths::data_dir()?.join(DIR)))
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Every record, oldest first. A file that does not read - one being
    /// written this very moment, or one somebody edited by hand - is left out
    /// rather than failing the whole history.
    pub fn all(&self) -> Result<Vec<Record>> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err).with_context(|| format!("cannot read {}", self.dir.display())),
        };
        let mut records: Vec<Record> = entries
            .flatten()
            .filter(|entry| number_of(&entry.path()).is_some())
            .filter_map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok())
            .collect();
        records.sort_by_key(|record| record.id);
        Ok(records)
    }

    /// Keep `record` under the next free number, and return that number.
    pub fn add(&self, record: &mut Record) -> Result<u64> {
        std::fs::create_dir_all(&self.dir).with_context(|| format!("cannot create {}", self.dir.display()))?;
        let mut id = self.highest()? + 1;
        loop {
            record.id = id;
            let path = self.dir.join(format!("{id:06}.json"));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    file.write_all(&serde_json::to_vec_pretty(record)?)
                        .with_context(|| format!("cannot write {}", path.display()))?;
                    return Ok(id);
                }
                // Another deploy took this number between the scan and now.
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => id += 1,
                Err(err) => return Err(err).with_context(|| format!("cannot create {}", path.display())),
            }
        }
    }

    /// The highest number taken so far; 0 when there is none.
    fn highest(&self) -> Result<u64> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err).with_context(|| format!("cannot read {}", self.dir.display())),
        };
        Ok(entries.flatten().filter_map(|entry| number_of(&entry.path())).max().unwrap_or(0))
    }

    fn builds(&self) -> PathBuf {
        self.dir.join(BUILDS)
    }

    /// Where the build with this digest is kept, if it still is.
    pub fn build(&self, digest: &str) -> Option<PathBuf> {
        let path = self.builds().join(format!("{digest}.tar.gz"));
        path.is_file().then_some(path)
    }

    /// Keep a build's archive under its digest. A build already kept - the
    /// second target of one deploy, a rollback - is not written again.
    pub fn keep_build(&self, digest: &str, archive: &[u8]) -> Result<()> {
        let builds = self.builds();
        let path = builds.join(format!("{digest}.tar.gz"));
        if path.is_file() {
            return Ok(());
        }
        std::fs::create_dir_all(&builds).with_context(|| format!("cannot create {}", builds.display()))?;
        // Written aside and renamed: a build cut short by a full disk must not
        // sit under a name that promises the whole archive.
        let partial = builds.join(format!("{digest}.{}.partial", std::process::id()));
        std::fs::write(&partial, archive).with_context(|| format!("cannot write {}", partial.display()))?;
        std::fs::rename(&partial, &path).with_context(|| format!("cannot keep the build as {}", path.display()))
    }

    /// Drop the builds no stand needs any more: each stand keeps the builds of
    /// its newest [`KEEP_BUILDS`] deploys that wrote files. Returns how many
    /// went.
    pub fn prune_builds(&self, records: &[Record]) -> Result<usize> {
        let keep = kept_builds(records);
        let entries = match std::fs::read_dir(self.builds()) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err).context("cannot read the kept builds"),
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(digest) = name.strip_suffix(".tar.gz") else { continue };
            let fresh = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| SystemTime::now().duration_since(modified).unwrap_or_default() < FRESH);
            if keep.contains(digest) || fresh {
                continue;
            }
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// The record file's number: `000012.json` is 12.
fn number_of(path: &Path) -> Option<u64> {
    if path.extension()? != "json" {
        return None;
    }
    path.file_stem()?.to_str()?.parse().ok()
}

/// The digests the stands still need: per stand, the builds of its newest
/// [`KEEP_BUILDS`] deploys that wrote files.
fn kept_builds(records: &[Record]) -> HashSet<String> {
    let mut seen: Vec<(&str, &str, usize)> = Vec::new();
    let mut keep = HashSet::new();
    for record in records.iter().rev() {
        let Some(digest) = &record.build else { continue };
        if !record.outcome.wrote() {
            continue;
        }
        let index = match seen.iter().position(|(server, dir, _)| record.on(server, dir)) {
            Some(index) => index,
            None => {
                seen.push((&record.server, &record.dir, 0));
                seen.len() - 1
            }
        };
        if seen[index].2 < KEEP_BUILDS {
            seen[index].2 += 1;
            keep.insert(digest.clone());
        }
    }
    keep
}

/// The deploy whose build a record sent: itself, or - through any number of
/// rollbacks - the original one.
pub fn origin<'a>(records: &'a [Record], record: &'a Record) -> &'a Record {
    let mut current = record;
    // Bounded: a hand-edited cycle must not hang a history.
    for _ in 0..records.len() {
        let Some(of) = current.rollback_of else { break };
        match records.iter().find(|candidate| candidate.id == of) {
            Some(earlier) => current = earlier,
            None => break,
        }
    }
    current
}

/// The deploy a plain `rollback` goes back to on a stand: the newest one that
/// went through before the deploy whose build the stand runs now.
///
/// What the stand runs is the newest deploy there that wrote files - a
/// partial one too, since its files are what is on the server. A deploy that
/// was itself a rollback stands for the deploy it sent again, so rolling back
/// twice goes two steps back rather than to and fro.
pub fn previous<'a>(records: &'a [Record], server: &str, dir: &str) -> Result<&'a Record> {
    let on_stand: Vec<&Record> = records.iter().filter(|record| record.on(server, dir)).collect();
    let Some(live) = on_stand.iter().rev().find(|record| record.outcome.wrote()) else {
        bail!("no deploy to {dir} on '{server}' is recorded on this machine - nothing to roll back");
    };
    let running = origin(records, live);
    on_stand
        .iter()
        .rev()
        .find(|record| record.id < running.id && record.outcome == Outcome::Deployed && record.rollback_of.is_none())
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the stand runs the build of #{}, and no earlier deploy there went through - name one with --to, see `turnout history`",
                running.id
            )
        })
}

/// The files deploys after `record` wrote to its stand that `record`'s build
/// does not have - what sending its build again would otherwise leave behind.
///
/// Only deploys that wrote count: one that was rolled back left nothing, and
/// one that failed before the upload never wrote. The second list names the
/// later deploys whose builds are no longer kept, whose files cannot be known.
pub fn added_since(store: &Store, records: &[Record], record: &Record, own: &BTreeSet<String>) -> Result<(Vec<String>, Vec<u64>)> {
    let mut added = BTreeSet::new();
    let mut unknown = Vec::new();
    for later in records
        .iter()
        .filter(|later| later.id > record.id && later.on(&record.server, &record.dir) && later.outcome.wrote())
    {
        let Some(path) = later.build.as_deref().and_then(|digest| store.build(digest)) else {
            unknown.push(later.id);
            continue;
        };
        let archive = std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        added.extend(crate::artifact::files_in(&archive)?.into_iter().filter(|file| !own.contains(file)));
    }
    Ok((added.into_iter().collect(), unknown))
}

/// The project's version and commit, read before the build.
///
/// The version is `package.json`'s, else the `[package]` version of a
/// `Cargo.toml` - the two kinds of project turnout recognizes. The commit is
/// git's `HEAD`, dirty when the working tree holds anything it does not.
pub fn source_of(project: &Path) -> Source {
    Source {
        version: package_version(project).or_else(|| cargo_version(project)),
        commit: head_commit(project),
    }
}

fn package_version(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get("version")?.as_str().map(str::to_string)
}

/// The `version` of the `[package]` table, when it is written out there
/// (`version.workspace = true` names none).
fn cargo_version(project: &Path) -> Option<String> {
    let text = std::fs::read_to_string(project.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim() == "version"
        {
            let value = value.trim();
            return value
                .strip_prefix('"')
                .and_then(|value| value.split_once('"'))
                .map(|(version, _)| version.to_string());
        }
    }
    None
}

fn head_commit(project: &Path) -> Option<Commit> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(project)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let sha = git(&["rev-parse", "HEAD"])?.trim().to_string();
    if sha.is_empty() {
        return None;
    }
    let dirty = git(&["status", "--porcelain"]).is_some_and(|status| !status.trim().is_empty());
    Some(Commit { sha, dirty })
}

/// An error as a record keeps it: whole when it is short, and its beginning
/// when it is not - where the cause is named.
pub fn brief(error: &str) -> String {
    const MOST: usize = 2_000;
    if error.len() <= MOST {
        return error.to_string();
    }
    let mut cut = MOST;
    while !error.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} ...", &error[..cut])
}

/// The last [`OUTPUT_LINES`] lines of `text`, at most [`OUTPUT_BYTES`] of them.
fn tail(text: &str) -> String {
    let text = text.trim_end();
    let lines: Vec<&str> = text.lines().collect();
    let mut kept = lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n");
    if kept.len() > OUTPUT_BYTES {
        let mut cut = kept.len() - OUTPUT_BYTES;
        while !kept.is_char_boundary(cut) {
            cut += 1;
        }
        kept = kept[cut..].to_string();
    }
    kept
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn a_record(id: u64, dir: &str, outcome: Outcome, build: Option<&str>) -> Record {
        Record {
            id,
            at: "2026-10-16T12:00:00Z".into(),
            app: "web".into(),
            target: Some("web-stand".into()),
            server: "stand".into(),
            credential: "deploy".into(),
            path: "site".into(),
            dir: dir.into(),
            files: 3,
            bytes: 30,
            millis: 1200,
            outcome,
            error: None,
            version: None,
            commit: None,
            restart: None,
            check: None,
            build: build.map(str::to_string),
            rollback_of: None,
        }
    }

    /// Numbers are claimed by creating the file: a number some other deploy
    /// took between the scan and the write is skipped, never overwritten.
    #[test]
    fn each_record_takes_the_next_free_number() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("deploys"));
        assert!(store.all().unwrap().is_empty(), "no directory yet is no history");
        let mut first = a_record(0, "/srv/site", Outcome::Deployed, None);
        assert_eq!(store.add(&mut first).unwrap(), 1);
        let mut second = a_record(0, "/srv/site", Outcome::Failed, None);
        assert_eq!(store.add(&mut second).unwrap(), 2);
        std::fs::write(dir.path().join("deploys/000003.json"), "{ half a record").unwrap();
        let mut third = a_record(0, "/srv/site", Outcome::Deployed, None);
        assert_eq!(store.add(&mut third).unwrap(), 4, "a taken number is skipped, even an unreadable one");
        let ids: Vec<u64> = store.all().unwrap().iter().map(|record| record.id).collect();
        assert_eq!(ids, vec![1, 2, 4], "an unreadable record is left out, not fatal");
    }

    /// Each stand keeps its own newest builds; a build two stands share stays
    /// while either needs it, and a deploy that wrote nothing keeps nothing.
    #[test]
    fn builds_are_kept_per_stand() {
        let mut records = Vec::new();
        for id in 1..=12 {
            records.push(a_record(id, "/srv/a", Outcome::Deployed, Some(&format!("a{id}"))));
        }
        records.push(a_record(13, "/srv/b", Outcome::Deployed, Some("a1")));
        records.push(a_record(14, "/srv/a", Outcome::RolledBack, Some("never")));
        let keep = kept_builds(&records);
        assert!(!keep.contains("a2"), "the oldest builds of a stand go");
        assert!(keep.contains("a3") && keep.contains("a12"), "its ten newest stay: {keep:?}");
        assert!(keep.contains("a1"), "another stand still runs it");
        assert!(!keep.contains("never"), "a rolled-back deploy left nothing to go back to");

        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf());
        store.keep_build("a2", b"old").unwrap();
        store.keep_build("a12", b"new").unwrap();
        let old = std::fs::File::options().write(true).open(store.build("a2").unwrap()).unwrap();
        old.set_modified(SystemTime::now() - FRESH * 2).unwrap();
        drop(old);
        assert_eq!(store.prune_builds(&records).unwrap(), 1);
        assert!(store.build("a2").is_none() && store.build("a12").is_some());

        store.keep_build("a1", b"fresh").unwrap();
        assert_eq!(store.prune_builds(&records[..2]).unwrap(), 0, "a build written within the hour is never taken");
    }

    /// A plain rollback goes to the deploy before the one the stand runs:
    /// past failures, past rollbacks, and from a partial deploy to the last
    /// one that went through.
    #[test]
    fn the_previous_deploy_is_the_one_before_what_runs() {
        let mut records = vec![
            a_record(1, "/srv/a", Outcome::Deployed, Some("b1")),
            a_record(2, "/srv/b", Outcome::Deployed, Some("other")),
            a_record(3, "/srv/a/", Outcome::Deployed, Some("b3")),
            a_record(4, "/srv/a", Outcome::RolledBack, None),
            a_record(5, "/srv/a", Outcome::Failed, None),
        ];
        assert_eq!(previous(&records, "stand", "/srv/a").unwrap().id, 1, "#3 runs; #4 and #5 changed nothing");

        records.push(a_record(6, "/srv/a", Outcome::Partial, Some("b6")));
        assert_eq!(previous(&records, "stand", "/srv/a").unwrap().id, 3, "#6's files are there; #3 went through");

        let mut rollback = a_record(7, "/srv/a", Outcome::Deployed, Some("b3"));
        rollback.rollback_of = Some(3);
        records.push(rollback);
        assert_eq!(previous(&records, "stand", "/srv/a").unwrap().id, 1, "a second rollback goes further back");

        let err = previous(&records, "stand", "/srv/b").unwrap_err().to_string();
        assert!(err.contains("#2") && err.contains("--to"), "{err}");
        assert!(previous(&records, "other", "/srv/a").is_err());
    }

    #[test]
    fn rollbacks_lead_back_to_the_build_they_sent() {
        let mut records = vec![a_record(1, "/a", Outcome::Deployed, Some("x"))];
        let mut second = a_record(2, "/a", Outcome::Deployed, Some("x"));
        second.rollback_of = Some(1);
        let mut third = a_record(3, "/a", Outcome::Deployed, Some("x"));
        third.rollback_of = Some(2);
        records.extend([second, third]);
        assert_eq!(origin(&records, &records[2]).id, 1);
        // A cycle somebody wrote by hand ends instead of hanging.
        records[0].rollback_of = Some(3);
        let _ = origin(&records, &records[2]);
    }

    #[test]
    fn deploy_numbers_read_with_or_without_the_hash() {
        assert_eq!(parse_id("12").unwrap(), 12);
        assert_eq!(parse_id("#12").unwrap(), 12);
        assert!(parse_id("web").is_err());
    }

    #[test]
    fn versions_come_from_the_project_manifest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(source_of(dir.path()).version, None);
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nversion = \"9.9.9\"\n\n[package]\nname = \"x\"\nversion = \"0.3.1\" # pinned\n",
        )
        .unwrap();
        assert_eq!(source_of(dir.path()).version.as_deref(), Some("0.3.1"));
        std::fs::write(dir.path().join("package.json"), r#"{ "name": "x", "version": "1.4.0" }"#).unwrap();
        assert_eq!(source_of(dir.path()).version.as_deref(), Some("1.4.0"), "package.json first");
    }

    /// The commit is HEAD; a change git sees makes it dirty.
    #[test]
    fn the_commit_and_whether_the_tree_matched_it() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(source_of(dir.path()).commit, None, "no repository, no commit");
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
        };
        git(&["init", "-q"]);
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "--no-verify", "-m", "first"]);
        let commit = source_of(dir.path()).commit.expect("a commit");
        assert_eq!(commit.sha.len(), 40);
        assert!(!commit.dirty);
        std::fs::write(dir.path().join("a.txt"), "changed").unwrap();
        let commit = source_of(dir.path()).commit.unwrap();
        assert!(commit.dirty);
        assert!(commit.short().ends_with("-dirty") && commit.short().len() == 13);
    }

    #[test]
    fn a_restart_keeps_the_end_of_what_it_said() {
        let long: String = (0..500).map(|line| format!("line {line}\n")).collect();
        let kept = Restart::new("make", &Ok(long));
        assert!(kept.ok);
        assert!(kept.output.starts_with("line 400\n") && kept.output.ends_with("line 499"), "{}", kept.output);
        let failed = Restart::new("make", &Err(anyhow::anyhow!("`make` exited with 2: boom")));
        assert!(!failed.ok && failed.output.contains("boom"));
        let wide = "é".repeat(OUTPUT_BYTES);
        assert!(tail(&wide).len() <= OUTPUT_BYTES, "cut on a character boundary, under the bound");
    }
}
