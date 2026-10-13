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
    if cfg!(test) {
        return Ok(sandbox());
    }
    let dirs = BaseDirs::new().context("cannot resolve the user data directory")?;
    Ok(dirs.data_local_dir().join("lacodda").join("turnout"))
}

/// Where the unit tests keep data when none is named: one scratch directory
/// per test process.
///
/// Without it, any code path a unit test drives that reads a catalog reads
/// the developer's own - and a read migrates. That happened in v0.24: a
/// deploy test learned a server's shell, `remote::dialect` went to remember
/// it, and the load moved the real data directory to a schema the installed
/// turnout refused. The integration tests always name a directory; this is
/// for everything else.
fn sandbox() -> PathBuf {
    static SANDBOX: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SANDBOX
        .get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("turnout-unit-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            dir
        })
        .clone()
}

#[cfg(test)]
mod tests {
    /// A unit test that reaches the catalogs reaches a scratch directory,
    /// never the one the installed turnout uses.
    #[test]
    fn unit_tests_never_reach_the_real_data_directory() {
        if std::env::var_os(super::DATA_DIR_ENV).is_some() {
            return;
        }
        let dir = super::data_dir().unwrap();
        let real = directories::BaseDirs::new().unwrap().data_local_dir().join("lacodda").join("turnout");
        assert_ne!(dir, real);
        assert!(dir.starts_with(std::env::temp_dir()), "{}", dir.display());
    }
}
