use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::Server;

/// Normalize a project directory for storing and for spawning tools in it.
/// Canonicalized via dunce so the result carries no `\\?\` prefix; this also
/// fixes a lowercase drive letter, which would otherwise reach dev servers as
/// a lowercase cwd and break them (Vite resolves imports against it).
pub fn project_dir(path: &Path) -> Result<PathBuf> {
    let path = std::path::absolute(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    if !path.is_dir() {
        bail!("directory {} does not exist", path.display());
    }
    dunce::canonicalize(&path).with_context(|| format!("cannot canonicalize {}", path.display()))
}

/// Run a shell command line in a directory, streaming output to the terminal.
pub fn run_in_dir(command_line: &str, dir: &Path) -> Result<std::process::ExitStatus> {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", command_line]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", command_line]);
        command
    };
    command.current_dir(dir).status().with_context(|| format!("cannot run '{command_line}'"))
}

/// Best-effort reachability probe used by `use` and `status`.
pub fn check_reachable(server: &Server) -> Result<reqwest::StatusCode> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(server.accept_invalid_certs)
            .timeout(std::time::Duration::from_secs(4))
            .build()?;
        let response = client.get(&server.url).send().await.with_context(|| format!("{} is unreachable", server.url))?;
        Ok(response.status())
    })
}
