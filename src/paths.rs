use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::BaseDirs;

/// Environment override for the data directory; primarily for tests and scripting.
pub const DATA_DIR_ENV: &str = "TURNOUT_DATA_DIR";

/// Root directory for all turnout data (catalogs, state):
/// `%LOCALAPPDATA%\lacodda\turnout` on Windows, `~/Library/Application Support/lacodda/turnout`
/// on macOS, `~/.local/share/lacodda/turnout` on Linux.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    let dirs = BaseDirs::new().context("cannot resolve the user data directory")?;
    Ok(dirs.data_local_dir().join("lacodda").join("turnout"))
}
