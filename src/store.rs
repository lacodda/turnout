use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::{App, Credential, Group, Server, State, Target};
use crate::paths;

const META_FILE: &str = "meta.json";
const APPS_FILE: &str = "apps.json";
const SERVERS_FILE: &str = "servers.json";
const CREDENTIALS_FILE: &str = "credentials.json";
const PATHS_FILE: &str = "paths.json";
const TARGETS_FILE: &str = "targets.json";
const GROUPS_FILE: &str = "groups.json";
const STATE_FILE: &str = "state.json";
use crate::migrate::CURRENT_VERSION as SCHEMA_VERSION;

/// Marker written by `setup`; its presence means the data directory is initialized.
#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub schema_version: u32,
}

/// The schema recorded in `meta.json`.
///
/// A marker that cannot be read is an error, not a guess: read as schema 1
/// it would send every command to the v0.9 walkthrough, and `setup` would
/// offer to retire a catalog that is perfectly readable.
fn stored_version(dir: &Path) -> Result<u32> {
    let path = dir.join(META_FILE);
    let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str::<Meta>(&text).map(|meta| meta.schema_version).with_context(|| {
        format!(
            "{} does not say which schema these settings are in - if the catalogs beside it are from this turnout, \
             write {{\"schema_version\": {SCHEMA_VERSION}}} into it",
            path.display()
        )
    })
}

/// Write the marker for the current schema the way catalogs are written:
/// beside, then renamed into place, so a crash leaves the old marker or the
/// new one and never an empty file.
fn write_meta(dir: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(&Meta {
        schema_version: SCHEMA_VERSION,
    })?;
    let tmp = dir.join(format!("{META_FILE}.tmp"));
    fs::write(&tmp, json).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, dir.join(META_FILE)).context("cannot write meta.json")
}

/// Migrate the data directory if it predates this build, then record the new
/// version.
///
/// Runs before the first read of any catalog, so no command ever parses files
/// in a shape it does not understand.
fn ensure_current_schema(dir: &Path) -> Result<()> {
    let from = stored_version(dir)?;
    if from == SCHEMA_VERSION {
        return Ok(());
    }
    crate::migrate::run(dir, from)?;
    write_meta(dir).context("cannot update meta.json after migrating")
}

fn meta_path() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join(META_FILE))
}

pub fn is_initialized() -> Result<bool> {
    Ok(meta_path()?.exists())
}

/// Whether the data directory exists but holds a schema no migration reads.
///
/// Only [`crate::commands::setup`] asks: every other command funnels through
/// [`require_initialized`] and gets the refusal with its instructions. Setup is
/// what those instructions point at, so it has to see the state rather than
/// trip over it.
///
/// Only schema 1 qualifies. Every later one migrates by itself on the next
/// command - until v0.27 this asked "older than this build?", and `setup`
/// offered to retire a catalog from v0.20 that needed nothing but a migration.
/// A *newer* directory does not qualify either: the fix there is to update
/// turnout, and starting fresh would discard settings that are fine.
pub fn needs_reset() -> Result<bool> {
    let dir = paths::data_dir()?;
    if !dir.join(META_FILE).exists() {
        return Ok(false);
    }
    Ok(stored_version(&dir)? < crate::migrate::FIRST_MIGRATABLE)
}

/// Bring an initialized data directory up to this build's schema now, rather
/// than on the next command that reads it.
pub fn bring_up_to_date() -> Result<()> {
    require_initialized().map(|_| ())
}

/// Move the unreadable catalogs aside and start a catalog at the current schema.
///
/// Returns where the old files went. They are moved, not deleted: they are the
/// only record of what the user is about to re-enter.
pub fn reset_to_current_schema() -> Result<PathBuf> {
    let dir = paths::data_dir()?;
    let from = stored_version(&dir)?;
    let aside = crate::migrate::retire_catalogs(&dir, from)?;
    write_meta(&dir)?;
    Ok(aside)
}

pub fn initialize() -> Result<PathBuf> {
    let dir = paths::data_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    write_meta(&dir)?;
    Ok(dir)
}

fn require_initialized() -> Result<PathBuf> {
    let dir = paths::data_dir()?;
    if !dir.join(META_FILE).exists() {
        bail!("turnout is not set up on this machine - run `turnout setup` first");
    }
    // Every catalog read and write funnels through here, which makes it the one
    // place that can guarantee nothing parses an older shape.
    ensure_current_schema(&dir)?;
    Ok(dir)
}

/// Catalogs are plain pretty-printed JSON arrays, one file per entity kind (ADR 0007).
fn load_catalog<T: DeserializeOwned>(file: &str) -> Result<Vec<T>> {
    let path = require_initialized()?.join(file);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn save_catalog<T: Serialize>(file: &str, items: &[T]) -> Result<()> {
    let dir = require_initialized()?;
    let path = dir.join(file);
    let tmp = dir.join(format!("{file}.tmp"));
    let json = serde_json::to_string_pretty(items)?;
    fs::write(&tmp, json).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}

pub fn load_apps() -> Result<Vec<App>> {
    load_catalog(APPS_FILE)
}

pub fn save_apps(apps: &[App]) -> Result<()> {
    save_catalog(APPS_FILE, apps)
}

pub fn load_servers() -> Result<Vec<Server>> {
    load_catalog(SERVERS_FILE)
}

pub fn save_servers(servers: &[Server]) -> Result<()> {
    save_catalog(SERVERS_FILE, servers)
}

pub fn load_credentials() -> Result<Vec<Credential>> {
    load_catalog(CREDENTIALS_FILE)
}

pub fn save_credentials(credentials: &[Credential]) -> Result<()> {
    save_catalog(CREDENTIALS_FILE, credentials)
}

pub fn load_paths() -> Result<Vec<crate::model::Path>> {
    load_catalog(PATHS_FILE)
}

pub fn save_paths(paths: &[crate::model::Path]) -> Result<()> {
    save_catalog(PATHS_FILE, paths)
}

pub fn load_targets() -> Result<Vec<Target>> {
    load_catalog(TARGETS_FILE)
}

pub fn save_targets(targets: &[Target]) -> Result<()> {
    save_catalog(TARGETS_FILE, targets)
}

pub fn load_groups() -> Result<Vec<Group>> {
    load_catalog(GROUPS_FILE)
}

pub fn save_groups(groups: &[Group]) -> Result<()> {
    save_catalog(GROUPS_FILE, groups)
}

pub fn load_state() -> Result<State> {
    let path = require_initialized()?.join(STATE_FILE);
    if !path.exists() {
        return Ok(State::default());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

pub fn save_state(state: &State) -> Result<()> {
    let dir = require_initialized()?;
    let path = dir.join(STATE_FILE);
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(state)?).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("cannot replace {}", path.display()))?;
    Ok(())
}
