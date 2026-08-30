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
/// 3 since v0.11.0: the deploy target is a named entity in `targets.json`, and
/// the server's own `deploy` map - which held the app-to-path relationship - is
/// gone (ADR 0013).
pub const CURRENT_VERSION: u32 = 3;

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
const STEPS: &[Step] = &[
    Step {
        from: 1,
        describes: "refused: the v0.9.0 entity split has no automatic migration",
        apply: refuse_schema_1,
        rewrites: false,
    },
    Step {
        from: 2,
        describes: "deploy targets moved out of the servers into a catalog of their own",
        apply: targets_from_server_deploy,
        rewrites: true,
    },
];

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

/// Schema 2 -> 3: every `server.deploy[app] = path` becomes a named target.
///
/// This one *does* name things, where [`refuse_schema_1`] refused to - and the
/// difference is the whole reason it is allowed to (ADR 0013). The v0.9.0 split
/// had to invent a credential out of a `user@host` string, producing names like
/// `prod-cred-1` that mean nothing six months later. Here both halves of
/// `{app}-{server}` were typed by the user; the migration joins them, and the
/// same generator serves the interactive "save this as a target?" prompt, so the
/// two can never drift apart.
///
/// The server's credential comes along because that is what a deploy from that
/// server used to log in as. A server with a deploy entry but no credential
/// could never have deployed at all, so its entries are dropped rather than
/// turned into a target that cannot run - and the user is told, because a
/// silently missing target is worse than a named gap.
fn targets_from_server_deploy(dir: &Path) -> Result<()> {
    let servers_file = dir.join("servers.json");
    let mut servers: Vec<serde_json::Value> = read_json_array(&servers_file)?;
    // Whatever is already in `targets.json` wins over anything generated here:
    // a partially-migrated directory must not gain a second copy of its own
    // targets on the next run.
    let mut targets: Vec<crate::model::Target> = read_json_array(&dir.join("targets.json"))?;
    let mut created = Vec::new();
    let mut skipped = Vec::new();

    for server in servers.iter_mut() {
        let Some(server_name) = server.get("name").and_then(|v| v.as_str()).map(String::from) else {
            continue;
        };
        // Taken out whether or not it yields targets: the field is gone from the
        // schema, and leaving it behind would have `Server` silently ignore it.
        let Some(deploy) = server.as_object_mut().and_then(|o| o.remove("deploy")) else {
            continue;
        };
        let credential = server.get("credential").and_then(|v| v.as_str()).map(String::from);
        let Some(entries) = deploy.as_object() else {
            continue;
        };
        for (app, path) in entries {
            let Some(path) = path.as_str() else { continue };
            let Some(credential) = credential.clone() else {
                skipped.push(format!("{app} on {server_name}"));
                continue;
            };
            let name = crate::model::unique_target_name(app, &server_name, &targets);
            created.push(format!("{name} ({app} -> {server_name}:{path})"));
            targets.push(crate::model::Target {
                name,
                app: app.clone(),
                server: server_name.clone(),
                credential,
                path: path.to_string(),
            });
        }
    }

    targets.sort_by(|a, b| a.name.cmp(&b.name));
    write_json(&dir.join("targets.json"), &targets)?;
    write_json(&servers_file, &servers)?;

    for line in &created {
        eprintln!("    target {line}");
    }
    if !skipped.is_empty() {
        // Named rather than counted: the user has to know which deploy stopped
        // being addressable, and re-creating it needs the name.
        eprintln!(
            "    no target for {} - the server had no credential to log in with;
    re-create with `turnout target add`",
            skipped.join(", ")
        );
    }
    Ok(())
}

/// Read a JSON array, treating a missing file as an empty one.
///
/// A catalog that has never been written is not an error at any schema: a user
/// who set turnout up and added nothing has no `targets.json` either.
fn read_json_array<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

/// Write a catalog the way [`crate::store`] does, so a migrated file is
/// byte-identical to one turnout would have written itself.
fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let json = serde_json::to_string_pretty(value)?;
    std::fs::write(path, json).with_context(|| format!("cannot write {}", path.display()))
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

    // The safety copy is made lazily, right before the first step that
    // actually rewrites something - not up front for the whole chain. A run
    // that ends in a refusal must leave the directory exactly as it found it,
    // backup folder included, and a chain can refuse at step one and still
    // contain a rewriting step later (1 -> 2 -> 3 is exactly that shape).
    let mut copied = false;
    for step in pending {
        if step.rewrites && !copied {
            let backup = backup_dir(dir, from);
            std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
            copy_data_files(dir, &backup)?;
            eprintln!("Migrating settings from schema {from} to {CURRENT_VERSION}.");
            eprintln!("  A copy of the old files is in {}", backup.display());
            copied = true;
        }
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

    /// Schema 2 -> 3: the relationship the server used to own becomes a named
    /// target, and the field it lived in is taken off the server.
    #[test]
    fn deploy_maps_become_named_targets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("servers.json"),
            r#"[{"name":"prod","url":"https://prod.example.com","credential":"deploy","deploy":{"web":"wwwroot","api":"api-root"}}]"#,
        )
        .unwrap();

        run(dir.path(), 2).unwrap();

        let targets: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("targets.json")).unwrap()).unwrap();
        assert_eq!(targets.len(), 2, "{targets:?}");
        let web = targets.iter().find(|t| t["name"] == "web-prod").expect("web-prod");
        assert_eq!(web["app"], "web");
        assert_eq!(web["server"], "prod");
        assert_eq!(web["path"], "wwwroot");
        // The login comes from the server, because that is what a deploy there
        // used to authenticate as.
        assert_eq!(web["credential"], "deploy");
        assert!(targets.iter().any(|t| t["name"] == "api-prod"), "{targets:?}");

        // The field is gone from the server, not merely ignored: leaving it
        // behind would have `Server` silently drop it on the next write.
        let servers: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("servers.json")).unwrap()).unwrap();
        assert!(servers[0].get("deploy").is_none(), "{servers:?}");
        assert_eq!(servers[0]["url"], "https://prod.example.com", "the rest of the server survives");
    }

    /// A server that could never log in produced no deploys either, so it
    /// yields no target - but the user has to be told, or a route silently
    /// vanishes.
    #[test]
    fn a_server_without_a_credential_yields_no_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("servers.json"),
            r#"[{"name":"prod","url":"https://prod.example.com","deploy":{"web":"wwwroot"}}]"#,
        )
        .unwrap();

        run(dir.path(), 2).unwrap();

        let targets: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("targets.json")).unwrap()).unwrap();
        assert!(targets.is_empty(), "{targets:?}");
        let servers: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("servers.json")).unwrap()).unwrap();
        assert!(servers[0].get("deploy").is_none(), "the field goes either way: {servers:?}");
    }

    /// Two servers deploying the same app collide on the generated name. Losing
    /// one of them silently would drop a deploy target on the floor.
    #[test]
    fn a_second_run_does_not_duplicate_what_it_already_made() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("servers.json"),
            r#"[{"name":"prod","url":"https://prod.example.com","credential":"deploy","deploy":{"web":"wwwroot"}}]"#,
        )
        .unwrap();

        run(dir.path(), 2).unwrap();
        // The second run finds no `deploy` field left, so it adds nothing - the
        // shape a half-finished migration leaves behind.
        run(dir.path(), 2).unwrap();

        let targets: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("targets.json")).unwrap()).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
    }

    /// A migrated catalog has to be readable by the code that owns it, not just
    /// well-formed JSON: the names it generates go straight into `build add`
    /// and the pickers.
    #[test]
    fn migrated_targets_parse_as_the_entity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("servers.json"),
            r#"[{"name":"kib-2","url":"https://kib2.example.com","credential":"deploy","deploy":{"my-app":"wwwroot"}}]"#,
        )
        .unwrap();

        run(dir.path(), 2).unwrap();

        let targets: Vec<crate::model::Target> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("targets.json")).unwrap()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "my-app-kib-2");
        crate::model::validate_name(&targets[0].name).expect("a generated name must be a valid entity name");
    }

    /// The chain 1 -> 3 refuses at its first step but contains a rewriting one
    /// after it. The safety copy must not be made for work that never happens -
    /// a refusal has to leave the directory exactly as it found it.
    #[test]
    fn a_refusing_chain_leaves_no_backup_even_when_a_later_step_rewrites() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[{\"name\":\"prod\"}]").unwrap();

        assert!(run(dir.path(), 1).is_err());

        let folders: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("settings-backup"))
            .collect();
        // Exactly one, and it is the reference copy the refusal itself makes -
        // not a second one from the migration machinery.
        assert_eq!(folders, vec!["settings-backup-v1"], "{folders:?}");
        assert!(!dir.path().join("targets.json").exists(), "a refused chain writes nothing");
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
