use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

const META_FILE: &str = "meta.json";
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
