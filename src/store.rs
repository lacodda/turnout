use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::{App, Credential, Group, Server, State};
use crate::paths;

const META_FILE: &str = "meta.json";
const APPS_FILE: &str = "apps.json";
const SERVERS_FILE: &str = "servers.json";
const CREDENTIALS_FILE: &str = "credentials.json";
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
/// A directory from before the marker existed, or one whose marker is
/// unreadable, is treated as version 1 - that is the only shape that could
/// have produced it.
fn stored_version(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join(META_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Meta>(&text).ok())
        .map(|meta| meta.schema_version)
        .unwrap_or(1)
}

/// Migrate the data directory if it predates this build, then record the new
/// version.
///
/// Runs before the first read of any catalog, so no command ever parses files
/// in a shape it does not understand.
fn ensure_current_schema(dir: &Path) -> Result<()> {
    let from = stored_version(dir);
    if from == SCHEMA_VERSION {
        return Ok(());
    }
    crate::migrate::run(dir, from)?;
    let json = serde_json::to_string_pretty(&Meta {
        schema_version: SCHEMA_VERSION,
    })?;
    fs::write(dir.join(META_FILE), json).context("cannot update meta.json after migrating")
}

fn meta_path() -> Result<PathBuf> {
    Ok(paths::data_dir()?.join(META_FILE))
}

pub fn is_initialized() -> Result<bool> {
    Ok(meta_path()?.exists())
}

pub fn initialize() -> Result<PathBuf> {
    let dir = paths::data_dir()?;
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let meta = Meta {
        schema_version: SCHEMA_VERSION,
    };
    let json = serde_json::to_string_pretty(&meta)?;
    fs::write(meta_path()?, json).context("cannot write meta.json")?;
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
