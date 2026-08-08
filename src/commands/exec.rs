use std::path::Path;

use anyhow::{Result, bail};

use crate::model::App;
use crate::store;

/// Run a named command of an app in its project directory, streaming output.
/// Exits with the child's exit code, so turnout is transparent in scripts.
pub fn run(command_name: &str, app_name: Option<String>) -> Result<()> {
    let apps = store::load_apps()?;
    let app = resolve(&apps, app_name)?;
    let Some(command_line) = app.commands.get(command_name) else {
        bail!(
            "app '{0}' has no '{1}' command - add it with `turnout app edit {0} --command {1}=CMD`",
            app.name,
            command_name
        );
    };
    let dir = crate::utils::project_dir(Path::new(&app.path))?;
    // Status goes to stderr so the command's own stdout stays clean for pipes.
    eprintln!("[{}] {command_line}", app.name);
    let status = crate::utils::run_in_dir(command_line, &dir)?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Explicit name wins; otherwise the app whose path contains the current
/// directory (deepest match), so `turnout dev` works from inside a project.
/// Outside any known project a terminal gets a picker instead of an error.
pub(crate) fn resolve(apps: &[App], name: Option<String>) -> Result<&App> {
    match name {
        Some(name) => apps
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow::anyhow!("no app named '{name}' - see `turnout app list`")),
        None => {
            let cwd = std::env::current_dir()?;
            // Canonicalize both sides: on macOS temp paths reach the app through
            // symlinks (/var -> /private/var), so raw prefix comparison lies.
            let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
            let here = apps
                .iter()
                .filter(|a| {
                    let path = Path::new(&a.path);
                    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
                    cwd.starts_with(&path)
                })
                .max_by_key(|a| a.path.len());
            if let Some(app) = here {
                return Ok(app);
            }
            if !crate::pick::interactive() {
                bail!("not inside a known app directory - pass the app name or see `turnout app list`");
            }
            let picked = crate::pick::app(apps, &store::load_state()?, "App")?;
            apps.iter().find(|a| a.name == picked).ok_or_else(|| anyhow::anyhow!("no app named '{picked}'"))
        }
    }
}
