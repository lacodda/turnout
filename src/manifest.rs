//! `deploy --diff`: what a deploy would change on the stand, asked before it
//! writes anything.
//!
//! The answer comes from the files themselves, not from a list turnout keeps
//! about them: a list written by the last deploy goes stale the first time
//! someone edits a file on the server by hand, and then it is a second truth
//! that disagrees with the first. So the server is asked what it has - every
//! file and its size in one pass - and only the files that could be the same
//! (same path, same size) are hashed, on both sides. A deploy directory often
//! holds far more than the build sends: uploads, a compose file, a database's
//! volume. Hashing all of that to compare a few hundred files would be the
//! slow part of a command meant to be quick.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::artifact::Plan;
use crate::detached::Remote;
use crate::shell::{self, Dialect};

/// The archive a deploy leaves for a moment; never part of what is compared.
const UPLOAD: &str = ".turnout-upload.tar.gz";

/// How long a POSIX hash command may grow: well under what sshd and the
/// shell take, with the paths quoted.
const POSIX_COMMAND_BYTES: usize = 32 * 1024;

/// What the server has in the deploy directory.
#[derive(Debug, PartialEq, Eq)]
enum Listing {
    /// No directory yet: a first deploy.
    Missing,
    /// Every file, relative to the directory, with its size.
    Files(BTreeMap<String, u64>),
}

/// The command that lists every file under `dir` with its size.
///
/// POSIX asks `find -printf` where it exists (GNU) and `wc -c` where it does
/// not (BusyBox, BSD). Probed rather than tried: `find` stops at an
/// unreadable directory with a non-zero exit after printing the rest, and
/// trying the second form after the first would list everything twice.
fn listing_command(dialect: Dialect, dir: &str) -> String {
    match dialect {
        Dialect::Posix => {
            let script = format!(
                "d={}; if [ ! -d \"$d\" ]; then echo missing; exit 0; fi; cd \"$d\" || exit 1; echo present; \
                 if find . -maxdepth 0 -printf '' >/dev/null 2>&1; then find . -type f -printf '%s %p\\n' 2>/dev/null; \
                 else find . -type f -exec wc -c {{}} + 2>/dev/null; fi; exit 0",
                dialect.quote(dir)
            );
            format!("sh -c {}", dialect.quote(&script))
        }
        Dialect::Windows => shell::powershell(&format!(
            "{WINDOWS_PREAMBLE}\
             $d = '{}'\n\
             if (-not (Test-Path -LiteralPath $d -PathType Container)) {{ 'missing'; exit 0 }}\n\
             'present'\n\
             $base = (Get-Item -LiteralPath $d).FullName.TrimEnd([char]92) + [char]92\n\
             Get-ChildItem -LiteralPath $d -Recurse -File -Force | ForEach-Object {{ '' + $_.Length + ' ./' + $_.FullName.Substring($base.Length).Replace([char]92, [char]47) }}\n\
             exit 0\n",
            dir.replace('\'', "''")
        )),
    }
}

/// What every Windows script opens with: errors and progress kept out of
/// the reply. A PowerShell that loads a module for the first time on a
/// machine reports "Preparing modules for first use" as progress - on a fresh
/// CI runner that was the whole of stderr - and each script ends in an
/// explicit `exit 0`, because PowerShell otherwise exits with the success of
/// whatever statement ran last.
const WINDOWS_PREAMBLE: &str = "$ErrorActionPreference = 'SilentlyContinue'\n$ProgressPreference = 'SilentlyContinue'\n";

/// Read a listing: `missing`, or `present` and then `SIZE ./PATH` lines.
/// `wc -c` pads its numbers and ends each batch with a `total` line, which has
/// no `./` and so is no file.
fn read_listing(output: &str) -> Result<Listing> {
    let mut lines = output.lines();
    match lines.next().map(str::trim) {
        Some("missing") => return Ok(Listing::Missing),
        Some("present") => {}
        other => bail!("the server's file list began with '{}'", other.unwrap_or_default()),
    }
    let mut files = BTreeMap::new();
    for line in lines {
        let line = line.trim_start().trim_end_matches('\r');
        let Some((size, path)) = line.split_once(' ') else { continue };
        let (Ok(size), Some(path)) = (size.parse::<u64>(), path.trim_start().strip_prefix("./")) else {
            continue;
        };
        if path != UPLOAD {
            files.insert(path.to_string(), size);
        }
    }
    Ok(Listing::Files(files))
}

/// The commands that hash `paths` under `dir`, split so that none outgrows
/// what the server's shell takes.
///
/// POSIX uses whichever of `sha256sum` (GNU, BusyBox) and `shasum -a 256`
/// (macOS) is there, and says `nohash` when neither is; a file it cannot read
/// is left out rather than failing the rest.
fn hash_commands(dialect: Dialect, dir: &str, paths: &[&str]) -> Vec<String> {
    let mut commands = Vec::new();
    let mut batch: Vec<String> = Vec::new();
    let mut size = 0;
    let limit = match dialect {
        Dialect::Posix => POSIX_COMMAND_BYTES,
        Dialect::Windows => shell::POWERSHELL_SCRIPT_CHARS - 300,
    };
    let flush = |batch: &mut Vec<String>, commands: &mut Vec<String>| {
        if batch.is_empty() {
            return;
        }
        commands.push(match dialect {
            Dialect::Posix => {
                let script = format!(
                    "cd {} || exit 1; if command -v sha256sum >/dev/null 2>&1; then sha256sum \"$@\" 2>/dev/null; \
                     elif command -v shasum >/dev/null 2>&1; then shasum -a 256 \"$@\" 2>/dev/null; else echo nohash; fi; exit 0",
                    dialect.quote(dir)
                );
                format!("sh -c {} turnout-hash {}", dialect.quote(&script), batch.join(" "))
            }
            // .NET's SHA-256 rather than `Get-FileHash`: in Windows
            // PowerShell 5.1 that is a function in a script module, loaded on
            // first use - which a fresh CI runner showed failing quietly.
            Dialect::Windows => shell::powershell(&format!(
                "{WINDOWS_PREAMBLE}\
                 $d = '{}'\n\
                 $sha = [Security.Cryptography.SHA256]::Create()\n\
                 foreach ($p in @({})) {{ try {{ $s = [IO.File]::OpenRead((Join-Path $d $p)); try {{ ([BitConverter]::ToString($sha.ComputeHash($s)) -replace '-', '').ToLower() + ' ./' + $p }} finally {{ $s.Dispose() }} }} catch {{ }} }}\n\
                 exit 0\n",
                dir.replace('\'', "''"),
                batch.join(",")
            )),
        });
        batch.clear();
    };
    for path in paths {
        let word = match dialect {
            Dialect::Posix => dialect.quote(&format!("./{path}")),
            Dialect::Windows => format!("'{}'", path.replace('\'', "''")),
        };
        if size + word.len() > limit {
            flush(&mut batch, &mut commands);
            size = 0;
        }
        size += word.len() + 1;
        batch.push(word);
    }
    flush(&mut batch, &mut commands);
    commands
}

/// Hashes the server printed, by path; `None` when it has no tool to hash with.
fn read_hashes(output: &str) -> Option<BTreeMap<String, String>> {
    if output.lines().any(|line| line.trim() == "nohash") {
        return None;
    }
    let mut hashes = BTreeMap::new();
    for line in output.lines() {
        let Some((hash, path)) = line.trim_end_matches('\r').split_once(' ') else {
            continue;
        };
        // `sha256sum` separates with two spaces, or a space and a `*` in
        // binary mode; PowerShell's line has one space.
        let path = path.trim_start().trim_start_matches('*');
        if let Some(path) = path.strip_prefix("./") {
            hashes.insert(path.to_string(), hash.to_ascii_lowercase());
        }
    }
    Some(hashes)
}

/// How the files were told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Compared {
    /// By SHA-256 of their contents.
    Content,
    /// By size alone: the server has nothing to hash with, so a changed file
    /// of the same size reads as unchanged.
    Size,
}

/// What a deploy would do to the deploy directory.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Diff {
    /// The directory does not exist yet: everything is new.
    pub first: bool,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: usize,
    /// Files on the server the build does not have: removed by `--clear`,
    /// left where they are without it.
    pub extra: Vec<String>,
    pub clear: bool,
    /// How many bytes the directory gains (negative: loses).
    pub delta: i64,
    pub compared: Compared,
}

impl Diff {
    /// Whether the deploy would change anything at all.
    pub fn changes(&self) -> bool {
        !self.added.is_empty() || !self.updated.is_empty() || (self.clear && !self.extra.is_empty())
    }

    /// The report, a line per kind of change.
    pub fn lines(&self) -> Vec<String> {
        let mut counts = Vec::new();
        if !self.updated.is_empty() {
            counts.push(format!("{} to update", self.updated.len()));
        }
        if !self.added.is_empty() {
            counts.push(format!("{} to add", self.added.len()));
        }
        if self.clear && !self.extra.is_empty() {
            counts.push(format!("{} to remove", self.extra.len()));
        }
        counts.push(format!("{} unchanged", self.unchanged));
        let mut lines = vec![format!("{} · {}", counts.join(" · "), signed_bytes(self.delta))];
        if self.first {
            lines.push("the directory does not exist yet - the deploy creates it".to_string());
        }
        lines.extend(listed("update", &self.updated));
        lines.extend(listed("add", &self.added));
        if !self.extra.is_empty() {
            if self.clear {
                lines.extend(listed("remove", &self.extra));
            } else {
                let what = if self.extra.len() == 1 { "file" } else { "files" };
                lines.push(format!(
                    "{} {what} on the server are not in the build - they stay (--clear removes them)",
                    self.extra.len()
                ));
            }
        }
        if self.compared == Compared::Size {
            lines.push("compared by size: the server has no sha256sum or shasum, so a same-size edit reads as unchanged".to_string());
        }
        lines
    }
}

/// How many file names a report shows per kind before it counts the rest.
const SHOWN: usize = 8;

fn listed(verb: &str, paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let shown: Vec<&str> = paths.iter().take(SHOWN).map(String::as_str).collect();
    let more = paths.len().saturating_sub(SHOWN);
    let tail = if more > 0 { format!(", and {more} more") } else { String::new() };
    Some(format!("{verb}: {}{tail}", shown.join(", ")))
}

fn signed_bytes(delta: i64) -> String {
    let size = crate::progress::human_bytes(delta.unsigned_abs());
    match delta {
        0 => "no change in size".to_string(),
        d if d > 0 => format!("+{size}"),
        _ => format!("-{size}"),
    }
}

/// Sort the build's files against the server's. `same` says whether a file
/// present on both sides with the same size has the same content.
fn compare(plan: &Plan, remote: &BTreeMap<String, u64>, same: impl Fn(&str) -> bool, clear: bool, compared: Compared) -> Diff {
    let mut diff = Diff {
        first: false,
        added: Vec::new(),
        updated: Vec::new(),
        unchanged: 0,
        extra: Vec::new(),
        clear,
        delta: 0,
        compared,
    };
    let mut local: Vec<(&str, u64)> = plan.entries.iter().map(|entry| (entry.relative.as_str(), entry.size)).collect();
    local.sort_unstable();
    for (path, size) in &local {
        match remote.get(*path) {
            None => {
                diff.added.push(path.to_string());
                diff.delta += *size as i64;
            }
            Some(theirs) if theirs == size && same(path) => diff.unchanged += 1,
            Some(theirs) => {
                diff.updated.push(path.to_string());
                diff.delta += *size as i64 - *theirs as i64;
            }
        }
    }
    for (path, size) in remote {
        if !local.iter().any(|(ours, _)| ours == path) {
            diff.extra.push(path.clone());
            if clear {
                diff.delta -= *size as i64;
            }
        }
    }
    diff
}

/// Ask the server and sort what the build would change.
pub fn diff(remote: &impl Remote, dialect: Dialect, dir: &str, plan: &Plan, clear: bool) -> Result<Diff> {
    let listing = String::from_utf8_lossy(&remote.exec_bytes(&listing_command(dialect, dir))?).into_owned();
    let files = match read_listing(&listing)? {
        Listing::Missing => {
            let mut diff = compare(plan, &BTreeMap::new(), |_| false, clear, Compared::Content);
            diff.first = true;
            return Ok(diff);
        }
        Listing::Files(files) => files,
    };
    let candidates: Vec<&crate::artifact::Entry> = plan.entries.iter().filter(|entry| files.get(&entry.relative) == Some(&entry.size)).collect();
    let paths: Vec<&str> = candidates.iter().map(|entry| entry.relative.as_str()).collect();
    let mut theirs = Some(BTreeMap::new());
    for command in hash_commands(dialect, dir, &paths) {
        let output = String::from_utf8_lossy(&remote.exec_bytes(&command)?).into_owned();
        match (read_hashes(&output), theirs.as_mut()) {
            (Some(hashes), Some(all)) => all.extend(hashes),
            _ => theirs = None,
        }
    }
    let Some(theirs) = theirs else {
        return Ok(compare(plan, &files, |_| true, clear, Compared::Size));
    };
    let mut ours = BTreeMap::new();
    for entry in candidates {
        ours.insert(entry.relative.as_str(), crate::artifact::sha256(&entry.path)?);
    }
    Ok(compare(
        plan,
        &files,
        |path| matches!((ours.get(path), theirs.get(path)), (Some(a), Some(b)) if a == b),
        clear,
        Compared::Content,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_of(files: &[(&str, &str)]) -> (tempfile::TempDir, Plan) {
        let root = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let plan = crate::artifact::plan_upload(root.path()).unwrap();
        (root, plan)
    }

    /// GNU's `-printf` and BusyBox's `wc -c` read the same, padding and the
    /// `total` line included; the deploy's own transient archive never counts.
    #[test]
    fn listings_from_every_find_read_the_same() {
        let gnu = "present\n12 ./index.html\n3400 ./assets/app.js\n9 ./.turnout-upload.tar.gz\n";
        let busybox = "present\n     12 ./index.html\n   3400 ./assets/app.js\n   3412 total\n";
        let expected = Listing::Files(BTreeMap::from([("assets/app.js".to_string(), 3400), ("index.html".to_string(), 12)]));
        assert_eq!(read_listing(gnu).unwrap(), expected);
        assert_eq!(read_listing(busybox).unwrap(), expected);
        assert_eq!(
            read_listing("present\r\n12 ./my file.txt\r\n").unwrap(),
            Listing::Files(BTreeMap::from([("my file.txt".to_string(), 12)]))
        );
        assert_eq!(read_listing("missing\n").unwrap(), Listing::Missing);
        assert!(read_listing("Permission denied\n").is_err());
    }

    #[test]
    fn hashes_from_every_tool_read_the_same() {
        let gnu = "ba78  ./index.html\nCAFE *./a b.js\n";
        let hashes = read_hashes(gnu).unwrap();
        assert_eq!(hashes.get("index.html").map(String::as_str), Some("ba78"));
        assert_eq!(hashes.get("a b.js").map(String::as_str), Some("cafe"), "binary mode and case");
        assert_eq!(read_hashes("nohash\n"), None);
    }

    /// The report the deploy prints, from the case the plan names:
    /// 34 to update, 2 to remove, the size it grows by.
    #[test]
    fn the_diff_sorts_every_file_once() {
        let (_root, plan) = plan_of(&[("index.html", "new"), ("assets/a.js", "same"), ("assets/b.js", "fresh file")]);
        let remote = BTreeMap::from([
            ("index.html".to_string(), 7),
            ("assets/a.js".to_string(), 4),
            ("old.js".to_string(), 100),
            ("uploads/cat.png".to_string(), 50),
        ]);
        let diff = compare(&plan, &remote, |path| path == "assets/a.js", false, Compared::Content);
        assert_eq!(diff.updated, vec!["index.html"]);
        assert_eq!(diff.added, vec!["assets/b.js"]);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.extra, vec!["old.js", "uploads/cat.png"]);
        assert_eq!(diff.delta, (3 - 7) + 10, "extra files stay without --clear");
        assert!(diff.lines()[0].starts_with("1 to update · 1 to add · 1 unchanged · +6 B"), "{:?}", diff.lines());
        assert!(
            diff.lines()
                .iter()
                .any(|line| line.contains("2 files on the server are not in the build - they stay"))
        );

        let cleared = compare(&plan, &remote, |path| path == "assets/a.js", true, Compared::Content);
        assert_eq!(cleared.delta, 6 - 150);
        assert!(cleared.lines()[0].contains("2 to remove"), "{:?}", cleared.lines());
        assert!(cleared.lines()[0].ends_with("-144 B"), "{:?}", cleared.lines());
    }

    /// Same size is not same content: only the hash says unchanged.
    #[test]
    fn a_same_size_edit_is_an_update() {
        let (_root, plan) = plan_of(&[("index.html", "abc")]);
        let remote = BTreeMap::from([("index.html".to_string(), 3)]);
        let diff = compare(&plan, &remote, |_| false, false, Compared::Content);
        assert_eq!(diff.updated, vec!["index.html"]);
        assert!(diff.changes());
        let unchanged = compare(&plan, &remote, |_| true, false, Compared::Content);
        assert!(!unchanged.changes());
    }

    /// No command may outgrow the shell that runs it, however many files
    /// there are - and none of the files may be dropped on the way.
    #[test]
    fn hash_commands_split_long_lists() {
        let names: Vec<String> = (0..3000).map(|i| format!("assets/chunk-{i:04}-abcdef.js")).collect();
        let paths: Vec<&str> = names.iter().map(String::as_str).collect();
        let posix = hash_commands(Dialect::Posix, "/var/www/site", &paths);
        assert!(posix.len() > 1);
        assert!(posix.iter().all(|command| command.len() < POSIX_COMMAND_BYTES + 1024));
        assert_eq!(posix.iter().map(|command| command.matches("'./assets/chunk-").count()).sum::<usize>(), 3000);
        let windows = hash_commands(Dialect::Windows, "C:\\site", &paths[..200]);
        assert!(windows.iter().all(|command| command.len() < 8191), "cmd.exe caps a command line at 8191");
    }

    /// The listing and hash scripts run for real through this machine's shell
    /// (sh on Linux and macOS, cmd and PowerShell on Windows), and a deploy
    /// directory compared with a build gives the diff it should.
    #[test]
    fn the_server_side_answers_through_a_real_shell() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        let (_root, plan) = plan_of(&[("index.html", "<h1>new</h1>"), ("assets/a b.js", "same"), ("assets/c.js", "brand new")]);
        let server = tempfile::tempdir().unwrap();
        let site = server.path().join("site");
        std::fs::create_dir_all(site.join("assets")).unwrap();
        std::fs::write(site.join("index.html"), "<h1>old</h1>").unwrap();
        std::fs::write(site.join("assets").join("a b.js"), "same").unwrap();
        std::fs::write(site.join("stray.txt"), "left over").unwrap();
        let dir = site.display().to_string();

        let diff = diff(&session, dialect, &dir, &plan, false).unwrap();
        assert_eq!(diff.compared, Compared::Content, "{diff:?}");
        assert_eq!(diff.updated, vec!["index.html"], "same size, other content: {diff:?}");
        assert_eq!(diff.added, vec!["assets/c.js"], "{diff:?}");
        assert_eq!(diff.unchanged, 1, "{diff:?}");
        assert_eq!(diff.extra, vec!["stray.txt"], "{diff:?}");
        assert!(!diff.first);

        let missing = server.path().join("not-yet").display().to_string();
        let first = super::diff(&session, dialect, &missing, &plan, false).unwrap();
        assert!(first.first && first.added.len() == 3, "{first:?}");
    }
}
