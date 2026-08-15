//! Bringing an older data directory up to the current schema.
//!
//! `schema_version` has been written since the first release but never read,
//! which was fine while there was only one shape. This is the machinery that
//! reads it, so a breaking change has somewhere to be handled rather than
//! becoming a parse error nobody can act on.
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
//!
//! Schema 1 -> 2 is the exception, and deliberately so: the v0.9.0 entity split
//! has no automatic migration ([`refuse_schema_1`]). A server's single
//! `user@host` became a server plus a credential, and its per-app directories
//! became named paths - choices about what to call things that turnout would
//! have to invent. Inventing them is how a catalog ends up full of `prod-cred-1`
//! entries nobody recognizes. The data is left untouched and the user is told
//! exactly what to do instead.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The schema this build reads and writes.
///
/// 2 since v0.9.0: servers hold host/port and point at named credentials and
/// paths, which are catalogs of their own.
pub const CURRENT_VERSION: u32 = 2;

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
    /// Whether this step rewrites files, and therefore needs the safety copy.
    ///
    /// A step that only refuses (see [`refuse_schema_1`]) writes nothing, so
    /// backing the directory up first would leave a folder the user has to
    /// clean up after an operation that never happened.
    rewrites: bool,
}

/// Every known migration, in order.
///
/// Schema 2 is current and has no successor yet, so the only entry is a
/// refusal. That leaves the `rewrites: true` machinery in [`run`] - the safety
/// copy and the error wrapping - unexercised until a schema 3 exists: it is
/// future plumbing kept warm, not dead code to delete.
const STEPS: &[Step] = &[Step {
    from: 1,
    describes: "refused: the v0.9.0 entity split has no automatic migration",
    apply: refuse_schema_1,
    rewrites: false,
}];

/// The v0.9.0 split, explained instead of guessed at.
///
/// Deciding *not* to migrate is the migration here. Every server used to carry
/// one `user@host` and a directory per app; those are now three entities that
/// each need a name, and turnout has no way to pick names a user will recognize
/// six months later. What it can do is say precisely what changed and leave the
/// files exactly as they were - a user who downgrades finds their old turnout
/// still working.
///
/// The instructions deliberately do not say "run `turnout export` first":
/// export reads the catalogs, so it hits this very refusal. The old files are
/// already plain JSON, and the copy is made here rather than asked for.
fn refuse_schema_1(dir: &Path) -> Result<()> {
    let saved = copy_for_reference(dir);
    let where_to_read = match &saved {
        Ok(path) => format!("A copy to read the old values from is in\n  {}\n", path.display()),
        // Not fatal: the originals are untouched either way, and the user can
        // open them directly.
        Err(err) => format!("(could not set a copy aside: {err:#} - the originals in that directory are still readable)\n"),
    };
    bail!(
        "these settings were written by turnout 0.8 or older (schema 1), and v0.9.0 changed how they are stored.\n\
         \n\
         What changed: a server used to hold the login and every app's remote directory. Now\n\
         logins are credentials and directories are paths - both named entities you can reuse\n\
         across servers.\n\
         \n\
         There is no automatic conversion: the new entities need names, and turnout would have\n\
         to invent them. Your files in {} are untouched.\n\
         {where_to_read}\n\
         To move over:\n  \
           1. turnout setup\n  \
           2. turnout server add / credential add / path add, reading the old values from the copy\n\
         \n\
         See https://lacodda.github.io/turnout/guides/upgrading-to-0-9/ for the walkthrough.",
        dir.display()
    )
}

/// Put the pre-split catalogs somewhere the user can read them while
/// re-entering, without touching the originals.
///
/// Idempotent by way of [`backup_dir`]: running a command twice does not
/// overwrite the first copy, and does not pile up a new one each time either -
/// an existing copy is reported as-is.
fn copy_for_reference(dir: &Path) -> Result<PathBuf> {
    let existing = dir.join("settings-backup-v1");
    if existing.is_dir() {
        return Ok(existing);
    }
    let backup = backup_dir(dir, 1);
    std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
    copy_data_files(dir, &backup)?;
    Ok(backup)
}

/// Move catalogs this build cannot read out of the way, so a fresh one can be
/// started in their place. Returns where they went.
///
/// Moved rather than deleted, and moved rather than copied: leaving the old
/// `servers.json` next to a new one would mean two files claiming to be the
/// catalog, and the next release that *can* migrate would find the wrong shape.
///
/// The journal and the update cache stay: they are not catalogs, and a user's
/// history of what they did survives a reset. Secrets in the OS keyring are not
/// touched at all - they are not in this directory.
pub fn retire_catalogs(dir: &Path, from: u32) -> Result<PathBuf> {
    // Reuse the folder an earlier refusal already made, rather than opening a
    // second one: the user was told where to read the old values, and splitting
    // them across `settings-backup-v1` and `-v1-2` would make that a lie.
    let existing = dir.join(format!("settings-backup-v{from}"));
    let aside = if existing.is_dir() { existing } else { backup_dir(dir, from) };
    std::fs::create_dir_all(&aside).with_context(|| format!("cannot create {}", aside.display()))?;
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = entry.file_name();
        let target = aside.join(&name);
        // A reference copy from an earlier refusal may already hold this file;
        // the original is the same content, so keeping the first is fine.
        if target.exists() {
            std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
            continue;
        }
        std::fs::rename(&path, &target).with_context(|| format!("cannot move {} aside", path.display()))?;
    }
    Ok(aside)
}

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

    // Only when something is actually about to be rewritten: a run that ends in
    // a refusal must leave the directory exactly as it found it, backup folder
    // included.
    if pending.iter().any(|step| step.rewrites) {
        let backup = backup_dir(dir, from);
        std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
        copy_data_files(dir, &backup)?;
        eprintln!("Migrating settings from schema {from} to {CURRENT_VERSION}.");
        eprintln!("  A copy of the old files is in {}", backup.display());
    }

    for step in pending {
        match (step.apply)(dir) {
            Ok(()) => eprintln!("  {}", step.describes),
            // A refusal is already a complete explanation; wrapping it in
            // "migration 1 -> 2 failed" would bury the part the user acts on.
            Err(err) if !step.rewrites => return Err(err),
            Err(err) => return Err(err).with_context(|| format!("migration {} -> {} failed", step.from, step.from + 1)),
        }
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

    /// The v0.9.0 split: a schema-1 directory is refused with instructions, the
    /// originals are left exactly as they were, and a readable copy is set
    /// aside to re-enter the values from.
    #[test]
    fn a_schema_1_directory_is_refused_with_a_way_forward() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[{\"name\":\"prod\"}]").unwrap();

        let err = run(dir.path(), 1).unwrap_err().to_string();
        assert!(err.contains("schema 1"), "{err}");
        assert!(err.contains("turnout setup"), "{err}");
        assert!(err.contains("credential") && err.contains("path"), "it must name what changed: {err}");
        assert!(
            !err.contains("migration 1 -> 2 failed"),
            "the explanation must not be buried in a wrapper: {err}"
        );
        // `export` reads the catalogs, so it hits this same refusal - telling
        // the user to run it first would be a closed loop.
        assert!(
            !err.contains("turnout export"),
            "the way out must not go through a command that also fails: {err}"
        );

        assert_eq!(
            std::fs::read_to_string(dir.path().join("servers.json")).unwrap(),
            "[{\"name\":\"prod\"}]",
            "the originals must be untouched"
        );
        let copy = dir.path().join("settings-backup-v1").join("servers.json");
        assert!(copy.is_file(), "a readable copy must be set aside");
        assert!(err.contains("settings-backup-v1"), "and the message must say where it is: {err}");
    }

    /// Running any command twice must not pile up copies, nor overwrite the
    /// first one - which is the only place the old values still exist if the
    /// user has started re-entering them.
    #[test]
    fn the_reference_copy_is_made_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[{\"name\":\"prod\"}]").unwrap();

        let _ = run(dir.path(), 1);
        std::fs::write(dir.path().join("settings-backup-v1").join("servers.json"), "edited by hand").unwrap();
        let _ = run(dir.path(), 1);

        let copies: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("settings-backup"))
            .collect();
        assert_eq!(copies, vec!["settings-backup-v1"], "{copies:?}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings-backup-v1").join("servers.json")).unwrap(),
            "edited by hand",
            "an existing copy must not be overwritten"
        );
    }

    /// `setup` is what every refusal points at, so it has to be able to clear
    /// the way: the old catalogs move aside, the journal stays, and the folder
    /// is the same one the refusal already named.
    #[test]
    fn retiring_catalogs_moves_them_into_the_folder_the_user_was_told_about() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[{\"name\":\"pi\"}]").unwrap();
        std::fs::write(dir.path().join("apps.json"), "[]").unwrap();
        std::fs::write(dir.path().join("journal.jsonl"), "{}").unwrap();
        // The refusal ran first and already made a reference copy.
        let _ = run(dir.path(), 1);

        let aside = retire_catalogs(dir.path(), 1).unwrap();

        assert_eq!(aside, dir.path().join("settings-backup-v1"), "a second folder would strand half the files");
        assert!(!dir.path().join("servers.json").exists(), "catalogs must be gone from the data dir");
        assert!(dir.path().join("journal.jsonl").exists(), "the journal is not a catalog and stays");
        assert_eq!(
            std::fs::read_to_string(aside.join("servers.json")).unwrap(),
            "[{\"name\":\"pi\"}]",
            "the old values must survive - they are what gets re-entered"
        );
        let folders: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("settings-backup"))
            .collect();
        assert_eq!(folders, vec!["settings-backup-v1"], "{folders:?}");
    }

    /// Without a prior refusal there is nothing to reuse, and the catalogs
    /// still have to land somewhere named.
    #[test]
    fn retiring_catalogs_works_without_a_previous_copy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[]").unwrap();

        let aside = retire_catalogs(dir.path(), 1).unwrap();

        assert!(aside.join("servers.json").is_file());
        assert!(!dir.path().join("servers.json").exists());
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
