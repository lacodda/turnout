use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};

use crate::job::{self, Mode};
use crate::model::App;
use crate::store;

/// How long `dev` waits for a server to announce itself before deciding it
/// never will and handing the console over to it.
///
/// Generous on purpose: a cold Next build on a big project takes its time, and
/// falling back to streaming early would defeat the quiet console for exactly
/// the slow starts it exists for. When the server does answer sooner - and
/// Vite answers in under a second - none of this waiting happens.
const READY_PATIENCE: Duration = Duration::from_secs(90);

/// Shortens the wait above, in milliseconds.
///
/// Ninety seconds is the right answer for a person and an impossible one for a
/// test: without this the fallback to streaming would be argued for and never
/// run. Undocumented in the CLI, for the same reason as [`job::MODE_ENV`].
const PATIENCE_ENV: &str = "TURNOUT_READY_PATIENCE_MS";

fn ready_patience() -> Duration {
    std::env::var(PATIENCE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(READY_PATIENCE, Duration::from_millis)
}

/// What the caller wants from the console.
#[derive(Clone, Copy, Default)]
pub struct Options {
    /// Stream the command's output in full instead of hiding it behind a loader.
    pub verbose: bool,
    /// Open the app's front door once the dev server is up.
    pub open: bool,
}

/// Run a named command of an app in its project directory.
/// Exits with the child's exit code, so turnout is transparent in scripts.
pub fn run(command_name: &str, app_name: Option<String>, options: Options) -> Result<()> {
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
    // A long-running server gets the ready detector; a command that finishes
    // gets a spinner and its elapsed time. `run` is the open case: a custom
    // command is as likely to be `storybook` as `codegen`, and waiting for a
    // ready signal that never comes costs nothing but the patience above,
    // after which it streams like the old pass-through did.
    let long_running = !matches!(command_name, "build" | "test" | "lint");
    let mode = Mode::resolve(if long_running { Mode::UntilReady } else { Mode::Quiet }, options.verbose);

    // Status goes to stderr so the command's own stdout stays clean for pipes.
    // Under a loader it is the one line that says what is being run at all.
    eprintln!("[{}] {command_line}", app.name);
    let door = store::load_state()?.gateway.and_then(|gateway| gateway.front_port);
    if command_name == "dev"
        && let Some(port) = app.dev_port
    {
        env.push(("PORT", port.to_string()));
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
    if options.open && !long_running {
        // Not an error: the flag is harmless here, and refusing `turnout build
        // --open` would only make a habit of typing it a failure.
        eprintln!("[{}] note: --open waits for a server to come up; '{command_name}' is not one", app.name);
    }

    let label = label_for(command_name, &app.name);
    let front_door = door.map(|front| crate::front::address(&app.name, front));
    let ready = long_running.then(|| job::Ready {
        app: &app.name,
        door: front_door.clone(),
        own: app.dev_port.map(|port| format!("http://localhost:{port}")),
        patience: ready_patience(),
        // The browser opens the moment the server answers, not a poll later:
        // the detector is the only thing in the process that knows when that
        // is. Every long-running command gets this, not `dev` alone - a
        // custom `storybook` command is as much a server as `dev` is, and
        // that is exactly why `run` carries the flag too.
        on_ready: options.open.then(|| {
            let url = front_door;
            Box::new(move || match &url {
                Some(url) => {
                    if let Err(err) = crate::front::open_in_browser(url) {
                        eprintln!("note: cannot open {url}: {err:#}");
                    }
                }
                None => eprintln!("note: --open needs the gateway's front door - start it with `turnout gateway start`"),
            }) as Box<dyn Fn() + Send>
        }),
    });
    let mut log = job::Log::open(&app.name, command_name);
    let outcome = job::run(&command_line, &dir, &env, mode, &label, &mut log, ready)?;

    // 130 is the conventional "interrupted" exit; the raw Windows status for
    // Ctrl+C is a negative NTSTATUS nobody's scripts check for.
    let code = if crate::term::interrupted() {
        130
    } else {
        outcome.status.code().unwrap_or(1)
    };
    std::process::exit(code);
}

/// What the loader calls the job: an action in progress, named after the
/// command rather than after the shell line, which is already on screen above.
fn label_for(command_name: &str, app: &str) -> String {
    let verb = match command_name {
        "build" => "Building",
        "test" => "Testing",
        "lint" => "Linting",
        "dev" => "Starting",
        other => return format!("Running {other} for {app}"),
    };
    format!("{verb} {app}")
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

#[cfg(test)]
mod tests {
    use super::label_for;

    /// The loader line reads as an action, and a custom command that has no
    /// verb of its own still gets a sentence rather than a bare name.
    #[test]
    fn the_loader_names_the_job_as_an_action() {
        assert_eq!(label_for("build", "myapp"), "Building myapp");
        assert_eq!(label_for("test", "myapp"), "Testing myapp");
        assert_eq!(label_for("lint", "myapp"), "Linting myapp");
        assert_eq!(label_for("dev", "myapp"), "Starting myapp");
        assert_eq!(label_for("storybook", "myapp"), "Running storybook for myapp");
    }
}
