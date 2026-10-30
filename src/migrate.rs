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
/// 7 since v0.25.0: a target can say how many backups its stand keeps, and
/// every backup turnout leaves prunes down to that. Nothing is rewritten, but
/// an older turnout editing such a target would drop the number, and the next
/// prune would cut the stand back to the default against its owner's word.
///
/// 6 since v0.24.0: a target can carry a health check and an address of its
/// own. Nothing is rewritten - both are optional - but a turnout older than
/// this would read such a target, drop the check without a word and deploy
/// unchecked. The bump is what makes it refuse instead.
///
/// 5 since v0.20.0: the gateway's record left `state.json` for the job
/// registry (`jobs/gateway.json`), where every other job turnout runs is
/// recorded too. A gateway running through the upgrade keeps running and is
/// known by the new build.
///
/// 4 since v0.18.0: every app has a gateway port, because turnout hands them
/// out itself and no longer asks. Nothing changed shape - an app that already
/// had a port keeps exactly the number it had; the migration only fills the
/// apps that had none.
///
/// 3 since v0.11.0: the deploy target is a named entity in `targets.json`, and
/// the server's own `deploy` map - which held the app-to-path relationship - is
/// gone (ADR 0013).
pub const CURRENT_VERSION: u32 = 7;

/// The oldest schema a migration reads. Schema 1 is refused instead
/// ([`refuse_schema_1`]), and `setup` is the way out of it.
pub const FIRST_MIGRATABLE: u32 = 2;

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
    Step {
        from: 3,
        describes: "apps without a gateway port were given one - turnout assigns them now",
        apply: gateway_ports_for_every_app,
        rewrites: true,
    },
    Step {
        from: 4,
        describes: "the gateway's record moved into the job registry, next to every other job",
        apply: gateway_into_the_job_registry,
        rewrites: true,
    },
    Step {
        from: 5,
        describes: "targets can carry a health check now - an older turnout will refuse these settings rather than deploy unchecked",
        apply: nothing_to_rewrite,
        rewrites: false,
    },
    Step {
        from: 6,
        describes: "targets can say how many backups their stand keeps - an older turnout will refuse these settings rather than drop the number",
        apply: nothing_to_rewrite,
        rewrites: false,
    },
];

/// A step whose whole effect is the version it leaves behind: the shape it
/// introduces is optional, and what has to change is who may read it.
fn nothing_to_rewrite(_dir: &Path) -> Result<()> {
    Ok(())
}

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
                url: None,
                health: None,
                keep_backups: None,
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

/// Schema 3 -> 4: every app ends up with a gateway port.
///
/// Until v0.18.0 the port was a question in the wizard, and "empty for none"
/// was an answer - so a catalog can hold apps that have no address to hand to
/// their dotenv file, and whose `{gateway}` commands refuse to run. Now that
/// turnout assigns the port itself, those apps are the only ones that could
/// still be in that state, and there is no reason to leave them there.
///
/// **A port already written is taken as it stands** - including one outside
/// [`crate::front::GATEWAY_PORTS`], because the app's dotenv file and any
/// `.env` a developer copied from it already carry that number. Renumbering
/// would be a silent breaking change dressed up as a migration.
///
/// The same rule an import follows for the apps it brings
/// ([`crate::front::settle_arrivals`]): here the portless apps are the ones
/// arriving, and every app that has a port is already settled.
fn gateway_ports_for_every_app(dir: &Path) -> Result<()> {
    let apps_file = dir.join("apps.json");
    let mut apps: Vec<crate::model::App> = read_json_array(&apps_file)?;
    let portless: Vec<String> = apps.iter().filter(|app| app.gateway_port.is_none()).map(|app| app.name.clone()).collect();
    // Not fatal when the range runs out: a catalog this full is better off
    // keeping the apps it has than refusing to open, and the note names the
    // app that is short.
    let notes = crate::front::settle_arrivals(&mut apps, &portless);
    write_json(&apps_file, &apps)?;
    // Named rather than counted: the number goes into the app's dotenv file on
    // its next `app edit`, and the user may recognize a clash with something
    // they run by hand.
    for note in notes {
        eprintln!("    {note}");
    }
    Ok(())
}

/// Schema 4 -> 5: the gateway record in `state.json` becomes `jobs/gateway.json`.
///
/// A running gateway is carried over only when it still is one: its pid alive
/// *and* its first port answering. The pid alone is not enough - a record
/// left by a gateway that died long ago may name a pid the OS has handed to
/// some other program since, and adopting that program as the gateway would
/// hand it to the next `turnout gateway stop`. Anything short of that is
/// dropped: the next `gateway start` begins clean.
fn gateway_into_the_job_registry(dir: &Path) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Recorded {
        pid: u32,
        ports: std::collections::BTreeMap<u16, String>,
        #[serde(default)]
        front_port: Option<u16>,
    }

    let state_file = dir.join("state.json");
    if !state_file.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&state_file).with_context(|| format!("cannot read {}", state_file.display()))?;
    let mut state: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("cannot parse {}", state_file.display()))?;
    let Some(recorded) = state.as_object_mut().and_then(|state| state.remove("gateway")) else {
        return Ok(());
    };
    write_json(&state_file, &state)?;

    let Ok(recorded) = serde_json::from_value::<Recorded>(recorded) else {
        return Ok(());
    };
    let answers = recorded.ports.keys().next().is_some_and(|port| {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], *port));
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(300)).is_ok()
    });
    let birth = crate::process::birth(recorded.pid).filter(|_| answers);
    let Some(birth) = birth else {
        eprintln!(
            "    gateway: the recorded process (pid {}) is no longer running - its record was dropped",
            recorded.pid
        );
        return Ok(());
    };
    let entry = crate::registry::Entry {
        work: crate::registry::Work::Gateway {
            ports: recorded.ports,
            front_port: recorded.front_port,
            // A gateway from before v0.22 has no control channel; `cookie`
            // asks for a restart.
            control: None,
        },
        pid: recorded.pid,
        birth,
        // Started before gateways led a process group of their own: `stop`
        // signals the pid alone, which is all the gateway ever was.
        group: None,
        // Not recorded before; the upgrade is the earliest moment known.
        started: crate::registry::now(),
        detached: true,
        log: None,
        ready: None,
        ended: None,
        signals: Vec::new(),
    };
    let jobs = dir.join("jobs");
    std::fs::create_dir_all(&jobs).with_context(|| format!("cannot create {}", jobs.display()))?;
    write_json(&jobs.join(format!("{}.json", crate::registry::GATEWAY)), &entry)?;
    eprintln!("    gateway: pid {} carried over - `turnout ps` lists it", recorded.pid);
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
/// The journal, the update cache and the file keyring's secrets stay: they
/// are not catalogs, a user's history of what they did survives a reset, and
/// a secret moved aside is a login lost. The cache and the secrets are
/// `.json` like the catalogs, and moved with them until v0.27 - while this
/// comment already said they stayed. Secrets in the OS keyring are not in
/// this directory at all.
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
        if !is_settings_file(&path) {
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
    //
    // The heading goes out with the first step that succeeds rather than with
    // the copy: a step that rewrites nothing still changes what the directory
    // is, and a description under no heading reads like a stray line.
    let mut copied: Option<PathBuf> = None;
    let mut announced = false;
    let mut copy_note = None;
    for step in pending {
        if step.rewrites && copied.is_none() {
            let backup = safety_copy(dir, from)?;
            copy_note = Some(format!("  A copy of the old files is in {}", backup.display()));
            copied = Some(backup);
        }
        match (step.apply)(dir) {
            Ok(()) => {
                if !announced {
                    eprintln!("Migrating settings from schema {from} to {CURRENT_VERSION}.");
                    announced = true;
                }
                if let Some(note) = copy_note.take() {
                    eprintln!("{note}");
                }
                eprintln!("  {}", step.describes);
            }
            // A refusal is already a complete explanation; wrapping it in
            // "migration 1 -> 2 failed" would bury the part the user acts on.
            Err(err) if !step.rewrites => return Err(err),
            Err(err) => {
                let copy = copied.as_deref().map(Path::display);
                return Err(err).with_context(|| match copy {
                    Some(copy) => format!(
                        "migration {} -> {} failed - the files as they were before are in {copy}, and the next command tries again from them",
                        step.from,
                        step.from + 1
                    ),
                    None => format!("migration {} -> {} failed", step.from, step.from + 1),
                });
            }
        }
    }
    Ok(from)
}

/// Marks a safety copy whose migration has not finished. Not `.json`: a user
/// copying the catalogs back by hand takes the `.json` files, and this is not
/// one of them.
const UNFINISHED: &str = "MIGRATION-UNFINISHED.txt";

/// The safety copy for a migration from `from`: the one an unfinished
/// attempt left, or a new one.
///
/// A migration that failed after its first rewriting step left `meta.json`
/// as it was, so every later command - Tab completion and a prompt's `status
/// --short` among them - ran it again, and each run copied the whole
/// directory into a new `settings-backup-vN-K` before failing the same way.
/// The copy an unfinished attempt made holds the files from before any step
/// touched them, which is exactly what a retry needs; a fresh copy would hold
/// half-migrated ones. So it is reused, and marked until the attempt is
/// [`complete`].
fn safety_copy(dir: &Path, from: u32) -> Result<PathBuf> {
    if let Some(unfinished) = unfinished_copy(dir, from) {
        return Ok(unfinished);
    }
    let backup = backup_dir(dir, from);
    std::fs::create_dir_all(&backup).with_context(|| format!("cannot create {}", backup.display()))?;
    copy_data_files(dir, &backup)?;
    // Written last: a copy cut short by a crash carries no mark, and is never
    // taken for a complete one.
    std::fs::write(
        backup.join(UNFINISHED),
        format!(
            "turnout copied the settings here before migrating them from schema {from}, and that migration has not finished.\n\
             Until it does, every attempt starts from this copy: it holds the files as they were.\n"
        ),
    )
    .with_context(|| format!("cannot mark {}", backup.display()))?;
    Ok(backup)
}

/// The marked copy an unfinished migration from `from` left, if there is one.
fn unfinished_copy(dir: &Path, from: u32) -> Option<PathBuf> {
    let base = format!("settings-backup-v{from}");
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `-v1` must not find `-v10`: the name is the base, or the base
            // and a numeric suffix.
            name == base || name.strip_prefix(&format!("{base}-")).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        })
        .map(|entry| entry.path())
        .filter(|path| path.join(UNFINISHED).is_file())
        .collect();
    found.sort();
    found.into_iter().next()
}

/// The migration from `from` is over and recorded: its safety copy stops
/// being the one a retry starts from.
///
/// Called by [`crate::store`] once `meta.json` says the new version, not at
/// the end of [`run`]: until the version is recorded, the next command
/// migrates again, and has to find the originals where the first attempt
/// left them. Best effort - a mark that stays only means one copy is reused
/// by a migration that is never run again.
pub fn complete(dir: &Path, from: u32) {
    if let Some(copy) = unfinished_copy(dir, from) {
        let _ = std::fs::remove_file(copy.join(UNFINISHED));
    }
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

/// Copy the JSON turnout owns. Journals, caches, secrets and previous backups
/// stay put - they are not what a migration rewrites, and a copy of the file
/// keyring would be a second plaintext copy of every password, in a folder
/// nothing ever cleans up.
fn copy_data_files(dir: &Path, backup: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if !is_settings_file(&path) {
            continue;
        }
        let name = entry.file_name();
        std::fs::copy(&path, backup.join(&name)).with_context(|| format!("cannot back up {}", path.display()))?;
    }
    Ok(())
}

/// Files in the data directory that are `.json` and still not settings: the
/// update check's cache, and the secrets of the file keyring.
const NOT_SETTINGS: [&str; 2] = [crate::update::CACHE_FILE, crate::secrets::FILE_STORE];

/// A catalog, the state or the schema marker: a `.json` file in the data
/// directory that is not one of [`NOT_SETTINGS`].
fn is_settings_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    path.is_file() && name.ends_with(".json") && !NOT_SETTINGS.contains(&name)
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

    /// Schema 5 -> 6 changes who may read the directory, not what is in it:
    /// the files stay byte for byte, and no safety copy is made of nothing.
    #[test]
    fn schema_5_moves_on_without_touching_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let targets = r#"[{"name":"web-prod","app":"web","server":"prod","credential":"deploy","path":"wwwroot"}]"#;
        std::fs::write(dir.path().join("targets.json"), targets).unwrap();
        assert_eq!(run(dir.path(), 5).unwrap(), 5);
        assert_eq!(std::fs::read_to_string(dir.path().join("targets.json")).unwrap(), targets);
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(entries.len(), 1, "no backup folder for a step that rewrites nothing: {entries:?}");
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
            r#"[{"name":"staging","url":"https://staging.example.com","credential":"deploy","deploy":{"my-app":"wwwroot"}}]"#,
        )
        .unwrap();

        run(dir.path(), 2).unwrap();

        let targets: Vec<crate::model::Target> = serde_json::from_str(&std::fs::read_to_string(dir.path().join("targets.json")).unwrap()).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].name, "my-app-staging");
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
        std::fs::write(dir.path().join("servers.json"), "[{\"name\":\"staging\"}]").unwrap();
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
            "[{\"name\":\"staging\"}]",
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

    /// A migration that fails after a rewriting step leaves `meta.json` as it
    /// was, so every command runs it again. Each run used to copy the whole
    /// directory into a new folder before failing the same way; the copy the
    /// first attempt made - of the files before anything touched them - is
    /// the one every retry needs, and the only one made.
    #[test]
    fn a_failing_migration_reuses_its_safety_copy() {
        let dir = tempfile::tempdir().unwrap();
        let servers = r#"[{"name":"prod","url":"https://prod.example.com","credential":"deploy","deploy":{"web":"wwwroot"}}]"#;
        std::fs::write(dir.path().join("servers.json"), servers).unwrap();
        // Broken by hand: step 2 -> 3 rewrites the servers, then 3 -> 4
        // cannot read the apps.
        std::fs::write(dir.path().join("apps.json"), "[{").unwrap();

        for _ in 0..3 {
            let err = run(dir.path(), 2).unwrap_err().to_string();
            assert!(err.contains("migration 3 -> 4 failed"), "{err}");
            assert!(err.contains("settings-backup-v2"), "the error names where the originals are: {err}");
        }
        let folders = || -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with("settings-backup"))
                .collect();
            names.sort();
            names
        };
        assert_eq!(folders(), vec!["settings-backup-v2"]);
        let copy = dir.path().join("settings-backup-v2");
        assert_eq!(
            std::fs::read_to_string(copy.join("servers.json")).unwrap(),
            servers,
            "the copy holds the files from before the first attempt, not half-migrated ones"
        );

        // Repaired, the migration finishes and the copy stops being reused.
        std::fs::write(dir.path().join("apps.json"), "[]").unwrap();
        run(dir.path(), 2).unwrap();
        complete(dir.path(), 2);
        assert!(!copy.join(UNFINISHED).exists());
        assert_eq!(folders(), vec!["settings-backup-v2"]);
        // A later migration from the same schema is another one, with a copy
        // of its own.
        run(dir.path(), 2).unwrap();
        assert_eq!(folders(), vec!["settings-backup-v2", "settings-backup-v2-2"]);
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
        // `.json` as well, and still not settings: the cache, and the file
        // keyring - whose copy would be every password in plain text again.
        std::fs::write(dir.path().join("update-check.json"), "{}").unwrap();
        std::fs::write(dir.path().join("insecure-secrets.json"), "{}").unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();

        let backup = dir.path().join("backup");
        std::fs::create_dir(&backup).unwrap();
        copy_data_files(dir.path(), &backup).unwrap();

        let mut copied: Vec<String> = std::fs::read_dir(&backup)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into())
            .collect();
        copied.sort();
        assert_eq!(copied, vec!["apps.json", "meta.json"], "journals, caches, secrets and directories stay put");
    }

    /// Starting over moves the catalogs aside and nothing else: the cache and
    /// the file keyring's secrets are `.json` too, and moved with the catalogs
    /// took every stored password out of reach of the fresh catalog.
    #[test]
    fn retiring_catalogs_leaves_the_cache_and_the_secrets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("servers.json"), "[]").unwrap();
        std::fs::write(dir.path().join("update-check.json"), "{}").unwrap();
        std::fs::write(dir.path().join("insecure-secrets.json"), r#"{"deploy":"s3cret"}"#).unwrap();

        let aside = retire_catalogs(dir.path(), 1).unwrap();

        assert!(aside.join("servers.json").is_file());
        for stays in ["update-check.json", "insecure-secrets.json"] {
            assert!(dir.path().join(stays).is_file(), "{stays} must stay where turnout reads it");
            assert!(!aside.join(stays).exists(), "{stays} must not be copied aside");
        }
    }
}
