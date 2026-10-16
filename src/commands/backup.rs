use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;
use crate::progress::Step;
use crate::shell::Dialect;
use crate::ssh::Session;
use crate::{output, remote};

/// The newest backup in a directory listing; anything that is not one of our
/// archives is the user's and is never a candidate.
fn newest(listing: &str) -> Option<String> {
    remote::backups_in(listing).into_iter().next()
}

pub fn backup(target_name: Option<String>, overrides: remote::Overrides) -> Result<()> {
    let target = remote::resolve(target_name, overrides)?;
    let dir = &target.path.dir;
    let step = Step::start(format!("Connecting to {} ...", target.connection_label()));
    let session = remote::connect(&target.server, &target.credential)?;
    let dialect = remote::dialect(&session, &target.server);
    step.update(format!("Backing up {dir} ..."));
    let name = remote::run_backup(&session, dialect, dir)?;
    step.done(format!("Backup {} created in {}", name.trim(), remote::backups_dir(dir)));
    crate::journal::record("backup", Some(&target.app.name), Some(&target.server.name), Some(name.trim()));
    keep_only(&session, dialect, &target);
    Ok(())
}

/// Prune a stand's backups down to what it keeps, after a backup was left
/// there - by `backup`, `deploy --backup`, or a deploy rolled back to one.
///
/// Best-effort: the backup that was just taken is the point of the command,
/// and an old one that would not go is a warning, not a failure of it.
pub fn keep_only(session: &Session, dialect: Dialect, target: &remote::Resolved) {
    let keep = target.keep_backups;
    match remote::prune_backups(session, dialect, &target.path.dir, keep) {
        Ok(pruned) if pruned.removed.is_empty() => {}
        Ok(pruned) => crate::progress::info(&format!(
            "Removed {} - the stand keeps the newest {keep} backup{}{}",
            describe(&pruned.removed),
            if keep == 1 { "" } else { "s" },
            match &target.target {
                Some(name) => format!(" (`turnout target edit {name} --keep-backups N` changes it)"),
                None => String::new(),
            }
        )),
        Err(err) => crate::progress::warn(&format!("could not prune the old backups: {err:#}")),
    }
}

/// `the backup 20261001-120000.tar.gz`, or `3 old backups`.
fn describe(removed: &[String]) -> String {
    match removed {
        [one] => format!("the old backup {one}"),
        many => format!("{} old backups", many.len()),
    }
}

/// What `backup prune --json` reports.
#[derive(Serialize)]
struct PruneView {
    dir: String,
    keep: u32,
    removed: Vec<String>,
    kept: usize,
}

/// `backup prune`: remove all but the newest backups of a stand.
pub fn prune(target_name: Option<String>, overrides: remote::Overrides, keep: Option<u32>, format: Format) -> Result<()> {
    let target = remote::resolve(target_name, overrides)?;
    let keep = keep.unwrap_or(target.keep_backups);
    let dir = remote::backups_dir(&target.path.dir);
    let step = Step::start(format!("Connecting to {} ...", target.connection_label()));
    let session = remote::connect(&target.server, &target.credential)?;
    let dialect = remote::dialect(&session, &target.server);
    step.update(format!("Pruning {dir} ..."));
    let pruned = remote::prune_backups(&session, dialect, &target.path.dir, keep)?;
    step.clear();
    if !pruned.removed.is_empty() {
        crate::journal::record(
            "backup.prune",
            Some(&target.app.name),
            Some(&target.server.name),
            Some(&format!("{} removed", pruned.removed.len())),
        );
    }
    let view = PruneView {
        dir,
        keep,
        removed: pruned.removed,
        kept: pruned.kept,
    };
    output::emit(format, &view, |view| {
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        if view.removed.is_empty() {
            return format!(
                "Nothing to prune: {} backup{} in {}, and it keeps {}.",
                view.kept,
                plural(view.kept),
                view.dir,
                view.keep
            );
        }
        let mut lines = vec![format!(
            "Removed {} backup{} from {}, kept the newest {}:",
            view.removed.len(),
            plural(view.removed.len()),
            view.dir,
            view.kept
        )];
        lines.extend(view.removed.iter().map(|name| format!("  {name}")));
        lines.join("\n")
    })
}

/// The backups `restore --list` reports: the archives in a target's backup
/// directory, newest first.
#[derive(Serialize)]
struct Backups {
    dir: String,
    backups: Vec<String>,
}

/// `restore`; `list` is `Some` with how to print the list when only listing.
pub fn restore(target_name: Option<String>, overrides: remote::Overrides, from: Option<String>, list: Option<Format>) -> Result<()> {
    let target = remote::resolve(target_name, overrides)?;
    let deploy_dir = target.path.dir.clone();
    let backups_raw = remote::backups_dir(&deploy_dir);
    let step = Step::start(format!("Connecting to {} ...", target.connection_label()));
    let session = remote::connect(&target.server, &target.credential)?;
    let dialect = remote::dialect(&session, &target.server);
    remote::check_quotable(dialect, &[&deploy_dir, &backups_raw])?;
    step.clear();

    if let Some(format) = list {
        let listing = remote::exec(&session, &dialect.list_dir(&backups_raw))?;
        let backups = Backups {
            backups: remote::backups_in(&listing),
            dir: backups_raw,
        };
        let app = target.app.name.clone();
        return output::emit(format, &backups, |backups| {
            if backups.backups.is_empty() {
                return format!("No backups yet in {} - create one with `turnout backup {app}`.", backups.dir);
            }
            backups.backups.join("\n")
        });
    }

    let name = match from {
        Some(name) => name,
        None => {
            // Sorted here rather than by piping through `sort | tail` on the
            // server: that pipeline is POSIX-only, and the names are fixed-width
            // timestamps, so lexicographic order is chronological order.
            let listing = remote::exec(&session, &dialect.list_dir(&backups_raw))?;
            newest(&listing).ok_or_else(|| anyhow::anyhow!("no backups in {backups_raw} - create one with `turnout backup {}`", target.app.name))?
        }
    };
    let step = Step::start(format!("Restoring {name} into {deploy_dir} ..."));
    remote::restore_backup(&session, dialect, &deploy_dir, &name)?;
    step.done(format!("Restored {name} into {deploy_dir}"));
    crate::journal::record("restore", Some(&target.app.name), Some(&target.server.name), Some(&name));
    if let Some(restart) = &target.path.restart {
        // The same post-write command a deploy runs, run the same way: in the
        // deploy directory, and detached from this connection. Until v0.24 a
        // restore ran it in the home directory, which a deploy stopped doing
        // in v0.11.
        let mut session = session;
        let connect = || remote::connect(&target.server, &target.credential);
        let step = Step::start(format!("Running: {restart}"));
        let output = remote::restart(&mut session, dialect, &target.path, restart, &connect, |line| {
            step.update(format!("Running: {restart} · {line}"))
        })?;
        step.done(format!("Restarted: {restart}"));
        if !output.trim().is_empty() {
            println!("{}", output.trim_end());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::newest;
    use crate::remote::is_backup_name;

    /// The newest backup is picked from the listing, not from its order: `ls -1`
    /// sorts, `dir /b` does not, and restore must land on the same archive
    /// either way.
    #[test]
    fn the_newest_backup_wins_regardless_of_listing_order() {
        let shuffled = "20260810-120000.tar.gz\n20260812-181500.tar.gz\n20260811-090000.tar.gz\n";
        assert_eq!(newest(shuffled).as_deref(), Some("20260812-181500.tar.gz"));

        // Windows line endings, since the listing may come from cmd.exe.
        let crlf = "20260810-120000.tar.gz\r\n20260812-181500.tar.gz\r\n";
        assert_eq!(newest(crlf).as_deref(), Some("20260812-181500.tar.gz"));
    }

    /// Same day, different times: the sort has to reach into the time field.
    #[test]
    fn backups_from_the_same_day_compare_by_time() {
        let listing = "20260812-090000.tar.gz\n20260812-181500.tar.gz\n20260812-093000.tar.gz";
        assert_eq!(newest(listing).as_deref(), Some("20260812-181500.tar.gz"));
    }

    /// The backups directory belongs to the user. A file they left in it must
    /// never be handed to `tar` as if turnout had written it.
    #[test]
    fn only_our_own_archives_are_candidates() {
        assert!(is_backup_name("20260812-181500.tar.gz"));
        assert!(!is_backup_name("notes.txt"));
        assert!(!is_backup_name("site-final.tar.gz"), "not a timestamp");
        assert!(!is_backup_name("20260812-181500.zip"));
        assert!(!is_backup_name("2026081-181500.tar.gz"), "wrong width");

        let mixed = "readme.txt\n20260812-181500.tar.gz\nold-backup.tar.gz\n";
        assert_eq!(newest(mixed).as_deref(), Some("20260812-181500.tar.gz"));
    }

    /// An empty or backup-less listing is a normal state - it means nobody has
    /// taken one yet, and the caller turns that into an explanation.
    #[test]
    fn nothing_to_restore_is_not_a_backup() {
        assert_eq!(newest(""), None);
        assert_eq!(newest("   \n\n"), None);
        assert_eq!(newest("readme.txt\n"), None);
    }
}
