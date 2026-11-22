//! `turnout copy`: top up a folder on a server from a folder here (ADR 0038).
//!
//! Only what is new or changed goes, and nothing is ever removed on the
//! server: the folder there is somebody's inbox - a service takes the files
//! it finds and moves them on - and what it took is none of this command's
//! business. Which files differ is asked the way `deploy --diff` asks it: the
//! server lists the folder with sizes, the files that could be the same are
//! hashed on both sides.
//!
//! Two promises make the folder safe to watch:
//!
//! * a file appears whole or not at all - it travels under a dot-name of its
//!   own beside where it goes, and is renamed into place once it is all
//!   there, so a reader never takes half a file;
//! * on a POSIX server what the copy writes belongs to the group of the
//!   folder it lands in, and that group may write it - the service that
//!   takes the files often runs as another account, and a file it can read
//!   but not move stays in its inbox for ever.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};

use crate::artifact::{self, Plan};
use crate::manifest::{self, Compared};
use crate::progress::{self, Step, Transfer, human_bytes, human_duration, rate};
use crate::remote::{self, Host};
use crate::shell::Dialect;
use crate::ssh::Session;

/// What a file is called while it travels: after its own name, behind a dot.
const PART: &str = ".turnout-part";

/// How long one command that moves or shares files may grow: well under
/// `cmd.exe`'s 8191 characters, the tightest line of any server.
const LINE: usize = 6_000;

pub fn run(from: PathBuf, to: String, credential: Option<String>, diff_only: bool) -> Result<()> {
    let (name, dir) = to
        .split_once(':')
        .ok_or_else(|| anyhow!("'{to}' names no folder on a server - write it SERVER:DIR, as in `turnout copy ./inbox staging:/srv/app/inbox`"))?;
    if name.is_empty() {
        bail!("'{to}' names no server - write it SERVER:DIR");
    }
    let host = remote::resolve_machine(name, credential)?.ok_or_else(|| anyhow!("no server or target named '{name}' - see `turnout server list`"))?;
    let dir = folder(&host, name, dir)?;
    if !from.is_dir() {
        bail!("{} is not a folder here", from.display());
    }
    let plan = artifact::plan_upload(&from)?;

    progress::intro(format!("Copying {} → {name}:{dir}", from.display()));
    let step = Step::start(format!("Connecting to {} ...", host.label()));
    let session = remote::connect(&host.server, &host.credential)?;
    let dialect = remote::dialect(&session, &host.server)?;
    step.done(format!("Connected to {}", host.label()));
    remote::check_quotable(dialect, &[&dir])?;
    if plan.files == 0 {
        progress::outro(format!("Nothing to copy - {} has no files", from.display()));
        return Ok(());
    }

    let step = Step::start(format!("Comparing {} files with {dir} ...", plan.files));
    let diff = manifest::diff(&session, dialect, &dir, &plan, false)?;
    let choice = Choice::of(&diff, &plan);
    step.done(choice.summary(plan.files));
    for line in choice.lines() {
        progress::info(&line);
    }
    if diff_only {
        progress::outro(if choice.send.is_empty() {
            "Nothing was written - the folder there has every file already"
        } else {
            "Nothing was written - copy without --diff to send them"
        });
        return Ok(());
    }
    if choice.send.is_empty() {
        progress::outro(format!("{name}:{dir} has every file already"));
        return Ok(());
    }

    let part = plan.only(&choice.send.iter().map(String::as_str).collect());
    let sent = send(&session, dialect, &dir, &part)?;
    crate::journal::record("copy", None, Some(&host.server.name), Some(&format!("{sent} files to {dir}")));
    let plural = if sent == 1 { "" } else { "s" };
    progress::outro(format!("Copied {sent} file{plural} to {name}:{dir}"));
    Ok(())
}

/// The folder `DIR` names on the machine: as written when it is absolute;
/// under the deploy directory when the name is a target; else where the
/// login lands, as `scp` takes it. A target alone (`TARGET:`) is its deploy
/// directory.
fn folder(host: &Host, name: &str, dir: &str) -> Result<String> {
    let absolute = dir.starts_with(['/', '\\']) || dir.as_bytes().get(1) == Some(&b':');
    match (&host.dir, dir) {
        (_, _) if absolute => Ok(dir.to_string()),
        (Some(deploy), "") => Ok(deploy.clone()),
        (Some(deploy), relative) => Ok(remote::join_remote(deploy, relative)),
        (None, "") => bail!("name the folder on '{name}' after the colon - `{name}:/srv/app/inbox`"),
        (None, relative) => Ok(relative.to_string()),
    }
}

/// Which files go, and why.
struct Choice {
    /// Every file that goes, by its relative path.
    send: BTreeSet<String>,
    new: Vec<String>,
    changed: Vec<String>,
    /// The ones that go again because nothing on the server can tell a
    /// same-size edit from no edit.
    again: usize,
    there: usize,
}

impl Choice {
    fn of(diff: &manifest::Diff, plan: &Plan) -> Self {
        let mut send: BTreeSet<String> = diff.added.iter().chain(&diff.updated).cloned().collect();
        let mut again = 0;
        if diff.compared == Compared::Size {
            // Sending a file again is harmless; leaving an edit behind
            // because it kept its size is not.
            for entry in &plan.entries {
                if send.insert(entry.relative.clone()) {
                    again += 1;
                }
            }
        }
        Self {
            send,
            new: diff.added.clone(),
            changed: diff.updated.clone(),
            again,
            there: diff.unchanged - again,
        }
    }

    fn summary(&self, files: u64) -> String {
        if self.send.is_empty() {
            return format!("All {files} files are there already");
        }
        let mut parts = Vec::new();
        if !self.new.is_empty() {
            parts.push(format!("{} new", self.new.len()));
        }
        if !self.changed.is_empty() {
            parts.push(format!("{} changed", self.changed.len()));
        }
        if self.again > 0 {
            parts.push(format!("{} that cannot be compared", self.again));
        }
        if self.there > 0 {
            parts.push(format!("{} there already", self.there));
        }
        format!("{} of {files} files to send: {}", self.send.len(), parts.join(", "))
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.extend(listed("new", &self.new));
        lines.extend(listed("changed", &self.changed));
        if self.again > 0 {
            lines.push("the server has no sha256sum or shasum - files of the same size go again".to_string());
        }
        lines
    }
}

/// How many names a report shows per kind before it counts the rest.
const SHOWN: usize = 8;

fn listed(kind: &str, paths: &[String]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let shown: Vec<&str> = paths.iter().take(SHOWN).map(String::as_str).collect();
    let more = paths.len().saturating_sub(SHOWN);
    let tail = if more > 0 { format!(", and {more} more") } else { String::new() };
    Some(format!("{kind}: {}{tail}", shown.join(", ")))
}

/// The name a file travels under: beside where it goes, hidden, and with a
/// suffix no reader takes for the file itself.
fn travelling(relative: &str) -> String {
    match relative.rsplit_once('/') {
        Some((parent, name)) => format!("{parent}/.{name}{PART}"),
        None => format!(".{relative}{PART}"),
    }
}

/// Send `part` into `dir`: every file under its travelling name, then all of
/// them into place, then to the folder's group. Returns how many went.
fn send(session: &Session, dialect: Dialect, dir: &str, part: &Plan) -> Result<u64> {
    // Asked before anything is made: a folder this creates must go to the
    // group of the one it is made in, not to this account's own.
    let group = match dialect {
        Dialect::Posix => Some(group_of(session, dir)?),
        Dialect::Windows => None,
    };
    let mut made = session.mkdir_all_made(dir)?;
    for sub in &part.dirs {
        made.extend(session.mkdir_all_made(&crate::commands::deploy::remote_path(dir, sub))?);
    }

    let transfer = Transfer::start(part.files, part.bytes);
    let mut moves: Vec<(String, String)> = Vec::new();
    let mut bytes = 0;
    for entry in &part.entries {
        let to = crate::commands::deploy::remote_path(dir, &entry.relative);
        let through = crate::commands::deploy::remote_path(dir, &travelling(&entry.relative));
        moves.push((through.clone(), to));
        match session.upload(&entry.path, &through, |chunk| transfer.advance(chunk)) {
            Ok(copied) => bytes += copied,
            Err(err) => {
                // Best-effort: what travelled so far is of no use to anyone
                // under names nobody reads, and the next copy sends it again.
                let left: Vec<String> = moves.iter().map(|(through, _)| through.clone()).collect();
                let _ = remote::remove_paths(session, dialect, &left);
                return Err(err).with_context(|| format!("cannot send {}", entry.path.display()));
            }
        }
    }
    let elapsed = transfer.elapsed();
    let files = part.files;
    let plural = if files == 1 { "" } else { "s" };
    transfer.done(format!(
        "Sent {files} file{plural} ({}) in {} · {}",
        human_bytes(bytes),
        human_duration(elapsed),
        rate(bytes, elapsed)
    ));

    let step = Step::start(format!("Putting {files} file{plural} in place ..."));
    if let Err(err) = put_in_place(session, dialect, &moves) {
        step.clear();
        let left: Vec<String> = moves.iter().map(|(through, _)| through.clone()).collect();
        let _ = remote::remove_paths(session, dialect, &left);
        return Err(err).context("cannot put the files in place");
    }
    step.done(format!("Put {files} file{plural} in place"));

    if let Some(group) = group {
        let mut written: Vec<String> = made;
        written.extend(moves.into_iter().map(|(_, to)| to));
        let refused = share(session, dialect, &group, &written)?;
        if !refused.is_empty() {
            progress::warn(&format!(
                "{} of what was written could not be given to the folder's group (gid {group}) - this account is not in it, \
                 so a service that runs in that group cannot move them: {}",
                refused.len(),
                refused.join(", ")
            ));
        }
    }
    Ok(files)
}

/// The group of `dir`, or of the nearest folder above it that is there: the
/// group a folder this copy makes is given.
fn group_of(session: &Session, dir: &str) -> Result<String> {
    let posix = Dialect::Posix;
    let script = "d=$1; while [ ! -d \"$d\" ]; do p=$(dirname -- \"$d\"); [ \"$p\" = \"$d\" ] && break; d=$p; done; \
                  stat -c %g -- \"$d\" 2>/dev/null || stat -f %g \"$d\"";
    let output = remote::exec(session, &format!("sh -c {} turnout-group {}", posix.quote(script), posix.quote(dir)))?;
    let group = output.trim();
    if group.is_empty() || !group.chars().all(|c| c.is_ascii_digit()) {
        bail!("the server did not say which group owns {dir} ('{group}')");
    }
    Ok(group.to_string())
}

/// Rename every travelling file over the one it becomes: on POSIX `mv`
/// within a folder, which a reader sees as the old file or the new one and
/// never as half of either.
fn put_in_place(session: &Session, dialect: Dialect, moves: &[(String, String)]) -> Result<()> {
    for chunk in chunks(moves.iter().map(|(from, to)| from.len() + to.len() + 24), moves.len()) {
        let pairs = &moves[chunk];
        let command = match dialect {
            Dialect::Posix => {
                let words: Vec<String> = pairs.iter().flat_map(|(from, to)| [dialect.quote(from), dialect.quote(to)]).collect();
                let script = "while [ $# -gt 1 ]; do mv -f -- \"$1\" \"$2\" || exit 1; shift 2; done";
                format!("sh -c {} turnout-move {}", dialect.quote(script), words.join(" "))
            }
            Dialect::Windows => pairs
                .iter()
                .map(|(from, to)| {
                    format!(
                        "(move /y {} {} >nul)",
                        dialect.quote(&from.replace('/', "\\")),
                        dialect.quote(&to.replace('/', "\\"))
                    )
                })
                .reduce(|chain, next| dialect.and_then(&chain, &next))
                .expect("a chunk is never empty"),
        };
        remote::exec(session, &command)?;
    }
    Ok(())
}

/// Give what this account wrote to `group`, and let the group write it.
/// Returns what could not be given - this account is not in the group.
///
/// Only what the account owns is touched: a folder that was there before
/// keeps whatever its owner set on it.
fn share(session: &Session, dialect: Dialect, group: &str, paths: &[String]) -> Result<Vec<String>> {
    let mut refused = Vec::new();
    for chunk in chunks(paths.iter().map(|path| path.len() + 3), paths.len()) {
        let words: Vec<String> = paths[chunk].iter().map(|path| dialect.quote(path)).collect();
        let script = "g=$1; shift; for p in \"$@\"; do [ -O \"$p\" ] || continue; \
                      chgrp \"$g\" \"$p\" 2>/dev/null || echo \"$p\"; chmod g+w \"$p\" 2>/dev/null; done; exit 0";
        let output = remote::exec(session, &format!("sh -c {} turnout-share {} {}", dialect.quote(script), group, words.join(" ")))?;
        refused.extend(output.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string));
    }
    Ok(refused)
}

/// Split `count` items of the given lengths into runs that fit on one line.
fn chunks(lengths: impl Iterator<Item = usize>, count: usize) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let (mut start, mut length) = (0, 0);
    for (index, item) in lengths.enumerate() {
        if index > start && length + item > LINE {
            runs.push(start..index);
            start = index;
            length = 0;
        }
        length += item;
    }
    if start < count {
        runs.push(start..count);
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_host(dir: Option<&str>) -> Host {
        Host {
            server: crate::model::Server {
                name: "stand".into(),
                label: None,
                url: "http://127.0.0.1".into(),
                host: Some("127.0.0.1".into()),
                port: 22,
                accept_invalid_certs: false,
                shell: None,
                credential: None,
            },
            credential: crate::model::Credential {
                name: "deploy".into(),
                user: "deploy".into(),
                auth: crate::model::Auth::Password,
                key: None,
            },
            dir: dir.map(str::to_string),
        }
    }

    /// `DIR` reads as `scp` reads it on a server, and from the deploy
    /// directory on a target.
    #[test]
    fn the_folder_is_read_as_scp_reads_it() {
        let server = a_host(None);
        assert_eq!(folder(&server, "stand", "/srv/inbox").unwrap(), "/srv/inbox");
        assert_eq!(folder(&server, "stand", "inbox").unwrap(), "inbox", "where the login lands");
        assert!(folder(&server, "stand", "").is_err(), "a server has no folder of its own");
        let target = a_host(Some("/var/www/site"));
        assert_eq!(folder(&target, "web", "").unwrap(), "/var/www/site");
        assert_eq!(folder(&target, "web", "uploads").unwrap(), "/var/www/site/uploads");
        assert_eq!(folder(&target, "web", "/srv/other").unwrap(), "/srv/other");
        let windows = a_host(Some("C:\\site"));
        assert_eq!(folder(&windows, "web", "inbox").unwrap(), "C:\\site\\inbox");
        assert_eq!(folder(&windows, "web", "D:\\drop").unwrap(), "D:\\drop");
    }

    #[test]
    fn a_file_travels_hidden_beside_where_it_goes() {
        assert_eq!(travelling("a.xml"), ".a.xml.turnout-part");
        assert_eq!(travelling("me@example.com/a.xml"), "me@example.com/.a.xml.turnout-part");
    }

    #[test]
    fn long_lists_are_cut_to_the_line() {
        assert_eq!(chunks([10, 10].into_iter(), 2), vec![0..2]);
        let runs = chunks(std::iter::repeat_n(LINE / 3, 7), 7);
        assert_eq!(runs, vec![0..3, 3..6, 6..7]);
        assert!(chunks(std::iter::empty(), 0).is_empty());
        assert_eq!(chunks([LINE * 2].into_iter(), 1), vec![0..1], "an item longer than a line still goes");
    }

    fn a_folder(files: &[(&str, &str)]) -> (tempfile::TempDir, Plan) {
        let root = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let plan = artifact::plan_upload(root.path()).unwrap();
        (root, plan)
    }

    /// Through a real SSH session into this machine's shell: the new and the
    /// changed files go, the unchanged and the server's own stay as they
    /// are, nothing is removed, and no travelling file is left behind.
    #[test]
    fn a_copy_sends_what_is_new_or_changed_and_removes_nothing() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        let server = tempfile::tempdir().unwrap();
        let inbox = server.path().join("inbox");
        std::fs::create_dir_all(inbox.join("me@example.com").join("done")).unwrap();
        std::fs::write(inbox.join("me@example.com").join("old.xml"), "taken").unwrap();
        std::fs::write(inbox.join("me@example.com").join("done").join("older.xml"), "taken long ago").unwrap();
        std::fs::write(inbox.join("me@example.com").join("same.xml"), "same").unwrap();
        std::fs::write(inbox.join("me@example.com").join("edited.xml"), "v1").unwrap();
        let dir = inbox.display().to_string();

        let (_here, plan) = a_folder(&[
            ("me@example.com/same.xml", "same"),
            ("me@example.com/edited.xml", "v2"),
            ("me@example.com/new.xml", "fresh"),
            ("other@example.com/acct-1/first.xml", "deep"),
        ]);
        let diff = manifest::diff(&session, dialect, &dir, &plan, false).unwrap();
        let choice = Choice::of(&diff, &plan);
        assert_eq!(
            choice.send.iter().map(String::as_str).collect::<Vec<_>>(),
            vec!["me@example.com/edited.xml", "me@example.com/new.xml", "other@example.com/acct-1/first.xml"]
        );
        assert_eq!(choice.summary(plan.files), "3 of 4 files to send: 2 new, 1 changed, 1 there already");
        let part = plan.only(&choice.send.iter().map(String::as_str).collect());
        assert_eq!(send(&session, dialect, &dir, &part).unwrap(), 3);

        let read = |path: &str| std::fs::read_to_string(inbox.join(path)).unwrap();
        assert_eq!(read("me@example.com/edited.xml"), "v2");
        assert_eq!(read("me@example.com/new.xml"), "fresh");
        assert_eq!(read("other@example.com/acct-1/first.xml"), "deep");
        assert_eq!(read("me@example.com/old.xml"), "taken", "nothing on the server is removed");
        assert_eq!(read("me@example.com/done/older.xml"), "taken long ago");
        let mut travelling = Vec::new();
        for entry in walk(&inbox) {
            if entry.ends_with(PART) {
                travelling.push(entry);
            }
        }
        assert!(travelling.is_empty(), "every file is in place: {travelling:?}");

        // Nothing new: nothing goes.
        let again = manifest::diff(&session, dialect, &dir, &plan, false).unwrap();
        assert!(Choice::of(&again, &plan).send.is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &str| std::fs::metadata(inbox.join(path)).unwrap().permissions().mode() & 0o070;
            assert_eq!(mode("me@example.com/new.xml") & 0o020, 0o020, "the group may write what came");
            assert_eq!(mode("other@example.com") & 0o020, 0o020, "and the folders the copy made");
            assert_eq!(mode("other@example.com/acct-1") & 0o020, 0o020);
            let before = std::fs::metadata(inbox.join("me@example.com/done")).unwrap().permissions().mode();
            assert_eq!(before & 0o020, 0, "a folder that was there keeps its mode");
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<String> {
        let mut found = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    found.push(path.display().to_string());
                }
            }
        }
        found
    }
}
