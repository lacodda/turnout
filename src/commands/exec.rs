use std::path::Path;

use anyhow::{Context, Result, bail};

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
    let dir = Path::new(&app.path);
    if !dir.is_dir() {
        bail!("project directory {} no longer exists", dir.display());
    }
    // Status goes to stderr so the command's own stdout stays clean for pipes.
    eprintln!("[{}] {command_line}", app.name);

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
    let status = command.current_dir(dir).status().with_context(|| format!("cannot run '{command_line}'"))?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Explicit name wins; otherwise the app whose path contains the current
/// directory (deepest match), so `turnout dev` works from inside a project.
fn resolve(apps: &[App], name: Option<String>) -> Result<&App> {
    match name {
        Some(name) => apps
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| anyhow::anyhow!("no app named '{name}' - see `turnout app list`")),
        None => {
            let cwd = std::env::current_dir()?;
            apps.iter()
                .filter(|a| cwd.starts_with(&a.path))
                .max_by_key(|a| a.path.len())
                .ok_or_else(|| anyhow::anyhow!("not inside a known app directory - pass the app name or see `turnout app list`"))
        }
    }
}
