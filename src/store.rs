use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::{App, Credential, Server};
use crate::paths;

const META_FILE: &str = "meta.json";
const APPS_FILE: &str = "apps.json";
const SERVERS_FILE: &str = "servers.json";
const CREDENTIALS_FILE: &str = "credentials.json";
const SCHEMA_VERSION: u32 = 1;

/// Marker written by `setup`; its presence means the data directory is initialized.
#[derive(Serialize, Deserialize)]
pub struct Meta {
    pub schema_version: u32,
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
