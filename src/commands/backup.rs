use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;
use crate::progress::Step;
use crate::{output, remote};

/// The newest backup in a directory listing.
///
/// Backup names are `YYYYMMDD-HHMMSS.tar.gz`, so the newest is simply the
/// largest string - no date parsing, and no dependence on the order the server
/// happened to list them in (`dir /b` and `ls -1` do not agree on that).
///
/// Anything that is not a backup name is ignored rather than trusted: the
/// directory belongs to the user, and a stray note in it must not be restored
/// as if it were an archive.
fn newest(listing: &str) -> Option<&str> {
    listing.lines().map(str::trim).filter(|line| is_backup_name(line)).max()
}

/// Whether a listing entry is one of our archives: `YYYYMMDD-HHMMSS.tar.gz`.
fn is_backup_name(entry: &str) -> bool {
    let Some(stem) = entry.strip_suffix(".tar.gz") else {
        return false;
    };
    let bytes = stem.as_bytes();
    bytes.len() == 15 && bytes[8] == b'-' && stem.chars().enumerate().all(|(i, c)| if i == 8 { c == '-' } else { c.is_ascii_digit() })
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
    Ok(())
}

/// The backups `restore --list` reports: the archives in a target's backup
/// directory, newest first.
#[derive(Serialize)]
struct Backups {
    dir: String,
    backups: Vec<String>,
}

/// Our archives in a directory listing, newest first; anything else in the
/// directory is the user's and stays out.
fn archives(listing: &str) -> Vec<String> {
    let mut names: Vec<String> = listing.lines().map(str::trim).filter(|line| is_backup_name(line)).map(str::to_string).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
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
            backups: archives(&listing),
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
            newest(&listing)
                .ok_or_else(|| anyhow::anyhow!("no backups in {backups_raw} - create one with `turnout backup {}`", target.app.name))?
                .to_string()
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
    use super::{is_backup_name, newest};

    /// The newest backup is picked from the listing, not from its order: `ls -1`
    /// sorts, `dir /b` does not, and restore must land on the same archive
    /// either way.
    #[test]
    fn the_newest_backup_wins_regardless_of_listing_order() {
        let shuffled = "20260810-120000.tar.gz\n20260812-181500.tar.gz\n20260811-090000.tar.gz\n";
        assert_eq!(newest(shuffled), Some("20260812-181500.tar.gz"));

        // Windows line endings, since the listing may come from cmd.exe.
        let crlf = "20260810-120000.tar.gz\r\n20260812-181500.tar.gz\r\n";
        assert_eq!(newest(crlf), Some("20260812-181500.tar.gz"));
    }

    /// Same day, different times: the sort has to reach into the time field.
    #[test]
    fn backups_from_the_same_day_compare_by_time() {
        let listing = "20260812-090000.tar.gz\n20260812-181500.tar.gz\n20260812-093000.tar.gz";
        assert_eq!(newest(listing), Some("20260812-181500.tar.gz"));
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
        assert_eq!(newest(mixed), Some("20260812-181500.tar.gz"));
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
