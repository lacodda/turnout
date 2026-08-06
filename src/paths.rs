use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

/// Environment override for the data directory; primarily for tests and scripting.
pub const DATA_DIR_ENV: &str = "TURNOUT_DATA_DIR";

/// Root directory for all turnout data (catalogs, state).
/// Resolves to the platform user data directory, e.g. `%LOCALAPPDATA%\lacodda\turnout` on Windows.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let dirs = ProjectDirs::from("", "lacodda", "turnout").context("cannot resolve the user data directory")?;
    Ok(dirs.data_local_dir().to_path_buf())
}
