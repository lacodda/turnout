use std::path::Path;

use anyhow::{Result, bail};

use crate::model::App;
use crate::store;

/// Run a named command of an app in its project directory, streaming output.
/// Exits with the child's exit code, so turnout is transparent in scripts.
pub fn run(command_name: &str, app_name: Option<String>) -> Result<()> {
    let mut apps = store::load_apps()?;
    let index = {
        let app = resolve(&apps, app_name)?;
        apps.iter().position(|a| a.name == app.name).expect("resolved from the same list")
    };
    // `dev` is where the app gets its port: fixed once, saved, and from then
    // on the front door knows where `{name}.localhost` goes.
    if command_name == "dev" && apps[index].dev_port.is_none() {
        let Some(port) = crate::front::dev_port_for(&apps, &apps[index].name) else {
            bail!(
                "no free dev port left in {:?} - pin one with `turnout app edit {} --dev-port PORT`",
                crate::front::DEV_PORTS,
                apps[index].name
            );
        };
        apps[index].dev_port = Some(port);
        store::save_apps(&apps)?;
    }
    let app = &apps[index];
    let Some(command_line) = app.commands.get(command_name) else {
        bail!(
            "app '{0}' has no '{1}' command - add it with `turnout app edit {0} --command {1}=CMD`",
            app.name,
            command_name
        );
    };
    let dir = crate::utils::project_dir(Path::new(&app.path))?;
    let command_line = crate::envfile::substitute(command_line, app)?;
    // The gateway address rides along as a variable for `dev` and for any
    // custom command - but not for `build`, `test` or `lint`. A variable in
    // the process environment overrides every dotenv file whatever the mode,
    // and a production bundle must not bake in localhost.
    let mut env = Vec::new();
    if !matches!(command_name, "build" | "test" | "lint")
        && let Some(url) = app.gateway_url()
    {
        env.push((app.gateway_env_name(), url));
    }
    // Status goes to stderr so the command's own stdout stays clean for pipes.
    eprintln!("[{}] {command_line}", app.name);
    if command_name == "dev"
        && let Some(port) = app.dev_port
    {
        env.push(("PORT", port.to_string()));
        let door = store::load_state()?.gateway.and_then(|gateway| gateway.front_port);
        match door {
            Some(front) => eprintln!("[{}] {} -> dev server port {port}", app.name, crate::front::address(&app.name, front)),
            None => eprintln!(
                "[{}] dev server port {port} (PORT, {{port}}) - start the gateway for http://{}.localhost",
                app.name, app.name
            ),
        }
        if !command_line.contains(&port.to_string()) && !app.commands["dev"].contains("{port}") {
            eprintln!(
                "[{}] note: the dev command does not mention {{port}} - a server that ignores PORT stays on its own port",
                app.name
            );
        }
    }
    let status = crate::utils::run_in_dir_with(&command_line, &dir, &env)?;
    // 130 is the conventional "interrupted" exit; the raw Windows status for
    // Ctrl+C is a negative NTSTATUS nobody's scripts check for.
    let code = if crate::term::interrupted() { 130 } else { status.code().unwrap_or(1) };
    std::process::exit(code);
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
            crate::pick::ensure_interactive("not inside a known app directory - pass the app name or see `turnout app list`")?;
            let picked = crate::pick::app(apps, &store::load_state()?, "App")?;
            apps.iter().find(|a| a.name == picked).ok_or_else(|| anyhow::anyhow!("no app named '{picked}'"))
        }
    }
}
