//! Bringing an older data directory up to the current schema.
//!
//! `schema_version` has been written since the first release but never read,
//! which was fine while there was only one shape. This is the machinery that
//! starts reading it, so the breaking entity split in v0.6.0 has something to
//! migrate *with* rather than a hard failure to apologize for.
//!
//! Three rules shape everything here:
//!
//! 1. **Never guess.** A directory from a *newer* turnout is refused outright.
//!    Its files may parse and still mean something different; importing half of
//!    that is worse than stopping.
//! 2. **Never lose data.** Every migration backs up what it is about to rewrite,
//!    into a timestamped folder the user can copy back by hand.
//! 3. **Never surprise.** Migrations run automatically because a tool that
//!    refuses to start until you type a magic command is just a worse error
//!    message - but they say what they did.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The schema this build reads and writes.
pub const CURRENT_VERSION: u32 = 1;

/// One step from `from` to `from + 1`.
///
/// Steps are deliberately single-version hops: a directory two versions behind
/// runs two of them in order, and each step only has to know about the shape
/// immediately before it.
struct Step {
    from: u32,
    /// What this step changes, shown to the user when it runs.
    describes: &'static str,
    apply: fn(&Path) -> Result<()>,
}

/// Every known migration, in order.
///
/// Empty until the first breaking change (the v0.6.0 entity split). The
/// machinery ships first on purpose: a migration is only useful if the version
/// that *precedes* the break already knows how to run one.
const STEPS: &[Step] = &[];

/// Bring `dir` up to [`CURRENT_VERSION`], reporting anything it does.
///
/// Returns the version the directory was on before, so callers can tell a
/// no-op from real work.
pub fn run(dir: &Path, from: u32) -> Result<u32> {
    if from == CURRENT_VERSION {
        return Ok(from);
    }
    if from > CURRENT_VERSION {
        bail!(
            "this data directory was written by a newer turnout (schema {from}, this build reads {CURRENT_VERSION}).\n\
             Update turnout with `turnout self-update`, or point TURNOUT_DATA_DIR at a different directory."
        );
    }

    let pending: Vec<&Step> = STEPS.iter().filter(|step| step.from >= from).collect();
    if pending.len() as u32 != CURRENT_VERSION - from {
        bail!(
            "cannot migrate this data directory from schema {from} to {CURRENT_VERSION}: no upgrade path.\n\
             Back up {} and run `turnout setup` to start fresh.",
            dir.display()
        );
    }

    let backup = backup_dir(dir, from);
    std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
    copy_data_files(dir, &backup)?;
    eprintln!("Migrating settings from schema {from} to {CURRENT_VERSION}.");
    eprintln!("  A copy of the old files is in {}", backup.display());

    for step in pending {
        (step.apply)(dir).with_context(|| format!("migration {} -> {} failed", step.from, step.from + 1))?;
        eprintln!("  {}", step.describes);
    }
    Ok(from)
}

/// `settings-backup-v1-2` next to the data, not inside a temp dir: a user who
/// needs it should find it where the data lives.
fn backup_dir(dir: &Path, from: u32) -> PathBuf {
    let mut candidate = dir.join(format!("settings-backup-v{from}"));
    // Two migrations of the same directory must not overwrite each other's
    // safety net.
    let mut suffix = 2;
    while candidate.exists() {
        candidate = dir.join(format!("settings-backup-v{from}-{suffix}"));
        suffix += 1;
    }
    candidate
}

/// Copy the JSON turnout owns. Journals, caches and previous backups stay put -
/// they are not what a migration rewrites.
fn copy_data_files(dir: &Path, backup: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_json = path.extension().and_then(|e| e.to_str()) == Some("json");
        if !is_json {
            continue;
        }
        let name = entry.file_name();
        std::fs::copy(&path, backup.join(&name)).with_context(|| format!("cannot back up {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_current_directory_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("apps.json"), "[]").unwrap();
        assert_eq!(run(dir.path(), CURRENT_VERSION).unwrap(), CURRENT_VERSION);
        // No backup folder for a no-op: the directory must look untouched.
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries.len(), 1, "{entries:?}");
    }

    /// The case this whole module exists to handle safely: data written by a
    /// build that knows more than this one.
    #[test]
    fn a_newer_directory_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(dir.path(), CURRENT_VERSION + 1).unwrap_err().to_string();
        assert!(err.contains("newer turnout"), "{err}");
        assert!(err.contains("self-update"), "the error must say how to move forward: {err}");
    }

    /// A version with no path to the present must say so rather than silently
    /// doing nothing and letting the parse fail later with a confusing error.
    #[test]
    fn a_gap_in_the_upgrade_path_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(dir.path(), 0).unwrap_err().to_string();
        assert!(err.contains("no upgrade path"), "{err}");
        assert!(err.contains("setup"), "the error must offer a way out: {err}");
    }

    #[test]
    fn backups_never_overwrite_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let first = backup_dir(dir.path(), 1);
        std::fs::create_dir_all(&first).unwrap();
        let second = backup_dir(dir.path(), 1);
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with("-2"), "{}", second.display());
    }

    /// The backup is a safety net for the files a migration rewrites - the
    /// journal and caches are neither rewritten nor worth duplicating.
    #[test]
    fn only_the_json_turnout_owns_is_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("apps.json"), "[]").unwrap();
        std::fs::write(dir.path().join("meta.json"), "{}").unwrap();
        std::fs::write(dir.path().join("journal.jsonl"), "{}").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();

        let backup = dir.path().join("backup");
        std::fs::create_dir(&backup).unwrap();
        copy_data_files(dir.path(), &backup).unwrap();

        let mut copied: Vec<String> = std::fs::read_dir(&backup)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into())
            .collect();
        copied.sort();
        assert_eq!(copied, vec!["apps.json", "meta.json"], "journals and directories stay put");
    }
}
