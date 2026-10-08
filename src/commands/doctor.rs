//! `turnout doctor`: "why does it not work" answered by a command.
//!
//! The usual answers are a handful, and each one costs ten minutes of digging
//! when it is not named: a dev server left running on the app's port, so the
//! front door shows yesterday's code; a dotenv file that still names the old
//! gateway port; a stand that is down, or a session that lapsed overnight; a
//! tool that is not on this shell's PATH; a password that was never stored.
//! The doctor looks at all of them at once and says, for each thing that is
//! wrong, what to run.
//!
//! It only looks. `--fix` repairs what is turnout's own to repair and can be
//! proven wrong - its block in a dotenv file, the file's line in
//! `.gitignore`, the record of a gateway that is gone, a binding to a server
//! that is not in the catalog - and ends a process that holds an app's dev
//! port only after a yes. Everything else names the command for a person.
//!
//! `--server NAME` turns to a machine instead: see
//! [`crate::commands::doctor_server`]. Both print the same checklist, and
//! `--json` prints the same checks as data (ADR 0020). The exit code is 1
//! when anything failed, so a script can gate on it; warnings do not count.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;
use crate::control::Verdict;
use crate::envfile::{Drift, InGit};
use crate::health::Health;
use crate::model::{App, Auth, Server};
use crate::ports::Holder;
use crate::{output, paths, pick, ports, process, registry, store, style};

/// How a check came out.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// As it should be.
    Ok,
    /// Worth knowing, nothing to do: no session yet, nothing to check.
    Info,
    /// Needs a look; the work goes on.
    Warn,
    /// Broken: something will fail because of it.
    Fail,
}

/// One line of the checklist.
#[derive(Serialize, Clone, Debug)]
pub struct CheckView {
    /// The part of the checklist it belongs to: `environment`, `apps`,
    /// `gateway`, `stands`, `catalog`; `server`, `paths`, `web` for `--server`.
    pub section: &'static str,
    /// What was checked, as a stable word for scripts: `node`, `dev_port`,
    /// `env_file`, `session`, `disk`, ...
    pub check: &'static str,
    pub state: State,
    /// The app, server or credential the line is about.
    pub subject: Option<String>,
    pub message: String,
    /// What to do about it: a command to run, or what to change.
    pub fix: Option<String>,
    /// `turnout doctor --fix` takes care of it.
    pub fixable: bool,
    /// What `--fix` does about it; the view says only that it can.
    #[serde(skip)]
    repair: Option<Repair>,
}

/// The repairs `--fix` knows.
#[derive(Clone, Debug)]
enum Repair {
    /// Write the app's dotenv block as the port says.
    EnvFile { app: String },
    /// Put the app's dotenv file into `.gitignore`.
    Ignore { app: String },
    /// Clear the record of a gateway whose process is gone.
    GatewayRecord,
    /// Drop a binding to a server that is not in the catalog.
    Unbind { app: String },
    /// End the process holding the app's dev port - after a yes.
    EndProcess { app: String, port: u16, pid: u32, name: String },
}

impl CheckView {
    fn new(section: &'static str, check: &'static str, state: State, subject: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            section,
            check,
            state,
            subject: subject.map(str::to_string),
            message: message.into(),
            fix: None,
            fixable: false,
            repair: None,
        }
    }

    pub(crate) fn ok(section: &'static str, check: &'static str, subject: Option<&str>, message: impl Into<String>) -> Self {
        Self::new(section, check, State::Ok, subject, message)
    }

    pub(crate) fn info(section: &'static str, check: &'static str, subject: Option<&str>, message: impl Into<String>) -> Self {
        Self::new(section, check, State::Info, subject, message)
    }

    pub(crate) fn warn(section: &'static str, check: &'static str, subject: Option<&str>, message: impl Into<String>) -> Self {
        Self::new(section, check, State::Warn, subject, message)
    }

    pub(crate) fn fail(section: &'static str, check: &'static str, subject: Option<&str>, message: impl Into<String>) -> Self {
        Self::new(section, check, State::Fail, subject, message)
    }

    /// What to run, or what to change.
    pub(crate) fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }

    fn repair(mut self, repair: Repair) -> Self {
        self.fix = Some("turnout doctor --fix".to_string());
        self.fixable = true;
        self.repair = Some(repair);
        self
    }
}

/// Everything one run of the doctor found.
#[derive(Serialize)]
pub struct DoctorView {
    pub version: &'static str,
    /// The server examined with `--server`; `null` for the local checklist.
    pub server: Option<String>,
    pub checks: Vec<CheckView>,
    /// Lines that failed.
    pub problems: usize,
    pub warnings: usize,
    /// `--server`: the deploy step the first problem would stop.
    pub deploy_stops_at: Option<&'static str>,
    /// What `--fix` did, before the checks ran again.
    pub fixed: Vec<String>,
}

impl DoctorView {
    pub(crate) fn new(server: Option<String>, checks: Vec<CheckView>, deploy_stops_at: Option<&'static str>, fixed: Vec<String>) -> Self {
        let count = |state: State| checks.iter().filter(|check| check.state == state).count();
        Self {
            version: env!("CARGO_PKG_VERSION"),
            server,
            problems: count(State::Fail),
            warnings: count(State::Warn),
            checks,
            deploy_stops_at,
            fixed,
        }
    }
}

pub fn run(format: Format, fix: bool, assume_yes: bool, server: Option<Option<String>>) -> Result<()> {
    let view = match server {
        Some(name) => {
            let server = crate::commands::doctor_server::resolve(name)?;
            let checks = while_checking(format, &format!("Checking {}", server.name), || crate::commands::doctor_server::checks(&server))?;
            let stops = crate::commands::doctor_server::stops_at(&checks);
            DoctorView::new(Some(server.name), checks, stops, Vec::new())
        }
        None => {
            let mut checks = while_checking(format, "Checking", local)?;
            let mut fixed = Vec::new();
            if fix && checks.iter().any(|check| check.fixable) {
                fixed = repair(&checks, assume_yes)?;
                // What the repairs left is what is reported: the checklist
                // runs again rather than guessing the repairs worked.
                checks = while_checking(format, "Checking again", local)?;
            }
            DoctorView::new(None, checks, None, fixed)
        }
    };
    output::emit(format, &view, text)?;
    if view.problems > 0 {
        // The report is the message; an `error:` line under it would only
        // repeat the count. The terminal is put back first, as `main` would.
        crate::term::restore();
        std::process::exit(1);
    }
    Ok(())
}

/// Run `work` under a spinner on a terminal: the stands and the sessions take
/// a few seconds when one of them does not answer. Off a terminal, and for
/// `--json`, nothing is drawn.
fn while_checking<T>(format: Format, label: &str, work: impl FnOnce() -> Result<T>) -> Result<T> {
    if format.json || !std::io::stdout().is_terminal() {
        return work();
    }
    let spinner = cliclack::spinner();
    spinner.start(format!("{label} ..."));
    let result = work();
    spinner.clear();
    result
}

// --- the local checklist ------------------------------------------------------------

const ENVIRONMENT: &str = "environment";
const APPS: &str = "apps";
const GATEWAY: &str = "gateway";
const STANDS: &str = "stands";
const CATALOG: &str = "catalog";

/// The catalogs, read once.
struct Catalogs {
    apps: Vec<App>,
    servers: Vec<Server>,
    targets: Vec<crate::model::Target>,
    paths: Vec<crate::model::Path>,
    credentials: Vec<crate::commands::credential::CredentialView>,
    state: crate::model::State,
}

impl Catalogs {
    fn load() -> Result<Self> {
        Ok(Self {
            apps: store::load_apps()?,
            servers: store::load_servers()?,
            targets: store::load_targets()?,
            paths: store::load_paths()?,
            credentials: crate::commands::credential::views()?,
            state: store::load_state()?,
        })
    }
}

/// The gateway as the record and its ports say it is.
enum GatewayState {
    Running(crate::model::Gateway),
    NotAnswering(crate::model::Gateway),
    /// Recorded, but its process is no more.
    Gone(u32),
    Stopped,
}

fn gateway_state() -> Result<GatewayState> {
    let entry = registry::load(registry::GATEWAY)?;
    let alive = entry.clone().filter(registry::Entry::is_running).and_then(registry::as_gateway);
    Ok(match (entry, alive) {
        (_, Some(gateway)) if crate::commands::gateway::probe(&gateway) => GatewayState::Running(gateway),
        (_, Some(gateway)) => GatewayState::NotAnswering(gateway),
        (Some(entry), None) if entry.ended.is_none() => GatewayState::Gone(entry.pid),
        _ => GatewayState::Stopped,
    })
}

/// Every local check, in the order the checklist prints them.
fn local() -> Result<Vec<CheckView>> {
    let data_dir = paths::data_dir()?;
    if !store::is_initialized()? {
        return Ok(vec![
            CheckView::fail(ENVIRONMENT, "setup", None, format!("turnout is not set up in {}", data_dir.display())).fix("turnout setup"),
        ]);
    }
    let catalogs = match Catalogs::load() {
        Ok(catalogs) => catalogs,
        Err(err) => {
            return Ok(vec![CheckView::fail(
                ENVIRONMENT,
                "catalogs",
                None,
                format!("the catalogs do not read: {err:#}"),
            )]);
        }
    };
    let gateway = gateway_state()?;

    // The slow parts run side by side: tools are processes to start, stands
    // and sessions are network round trips that may wait out a timeout.
    let (tools, stands, sessions) = std::thread::scope(|scope| {
        let tools = scope.spawn(|| Tools::find(&catalogs.apps));
        let stands = scope.spawn(|| stands(&catalogs));
        let sessions = scope.spawn(|| sessions(&catalogs, &gateway));
        (
            tools.join().unwrap_or_default(),
            stands.join().unwrap_or_default(),
            sessions.join().unwrap_or_default(),
        )
    });

    let mut checks = vec![CheckView::ok(
        ENVIRONMENT,
        "turnout",
        None,
        format!("turnout {} · data {}", env!("CARGO_PKG_VERSION"), data_dir.display()),
    )];
    checks.extend(tools.checks(&catalogs.apps));
    checks.push(keyring());
    for app in &catalogs.apps {
        checks.extend(app_checks(app, &catalogs, &gateway, &tools));
    }
    checks.extend(gateway_checks(&catalogs, &gateway));
    checks.extend(stands);
    checks.extend(sessions);
    checks.extend(catalog_checks(&catalogs));
    Ok(checks)
}

// --- tools ----------------------------------------------------------------------------

/// The programs an app command can start with that the doctor knows how to
/// ask for a version. Anything else - a script, `vite` from `node_modules` -
/// is the project's own business.
const RUNNERS: [&str; 10] = ["node", "npm", "npx", "pnpm", "pnpx", "yarn", "bun", "bunx", "deno", "cargo"];

/// The runners that need Node underneath them.
const ON_NODE: [&str; 6] = ["node", "npm", "npx", "pnpm", "pnpx", "yarn"];

/// How long a `--version` may take. Corepack downloads a pinned manager on
/// its first call, and that is a minute nobody should wait inside a doctor.
const VERSION_PATIENCE: Duration = Duration::from_secs(10);

/// The runners the apps use, and the version each answered with.
#[derive(Default)]
struct Tools {
    /// Runner -> its `--version`, or `None` when it did not answer.
    versions: BTreeMap<String, Option<String>>,
}

impl Tools {
    fn find(apps: &[App]) -> Self {
        let mut wanted: BTreeSet<String> = apps.iter().flat_map(|app| app.commands.values()).filter_map(|line| runner(line)).collect();
        if wanted.iter().any(|runner| ON_NODE.contains(&runner.as_str())) {
            wanted.insert("node".to_string());
        }
        let versions = std::thread::scope(|scope| {
            let asks: Vec<_> = wanted.iter().map(|runner| (runner.clone(), scope.spawn(move || version_of(runner)))).collect();
            asks.into_iter().map(|(runner, ask)| (runner, ask.join().ok().flatten())).collect()
        });
        Self { versions }
    }

    fn version(&self, runner: &str) -> Option<&str> {
        self.versions.get(runner).and_then(|version| version.as_deref())
    }

    /// One line for everything that answered, one per runner that did not.
    fn checks(&self, apps: &[App]) -> Vec<CheckView> {
        let found: Vec<String> = self
            .versions
            .iter()
            .filter_map(|(runner, version)| version.as_ref().map(|version| format!("{runner} {version}")))
            .collect();
        let mut checks = Vec::new();
        if !found.is_empty() {
            checks.push(CheckView::ok(ENVIRONMENT, "tools", None, found.join(" · ")));
        }
        for (runner, _) in self.versions.iter().filter(|(_, version)| version.is_none()) {
            let users: Vec<&str> = apps
                .iter()
                .filter(|app| {
                    app.commands
                        .values()
                        .filter_map(|line| crate::commands::doctor::runner(line))
                        .any(|used| &used == runner || (runner == "node" && ON_NODE.contains(&used.as_str())))
                })
                .map(|app| app.name.as_str())
                .collect();
            checks.push(
                CheckView::fail(
                    ENVIRONMENT,
                    "tools",
                    None,
                    format!(
                        "{runner} does not answer on this PATH - {} {} it",
                        users.join(", "),
                        if users.len() == 1 { "runs" } else { "run" }
                    ),
                )
                .fix(format!("install {runner}, or put it on the PATH of this shell")),
            );
        }
        checks
    }
}

/// The program a command line starts, when it is one of [`RUNNERS`].
///
/// Leading `NAME=value` words are environment for the command, not the
/// command; a Windows launcher's `.cmd` or `.exe` is the same program.
pub(crate) fn runner(command_line: &str) -> Option<String> {
    let word = command_line.split_whitespace().find(|word| !word.contains('='))?;
    let word = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    let word = word.trim_end_matches(".cmd").trim_end_matches(".exe");
    RUNNERS.contains(&word).then(|| word.to_string())
}

/// `{program} --version` through the shell the app commands run in, first
/// line of its answer; `None` when it is not there or says nothing.
fn version_of(program: &str) -> Option<String> {
    let mut command = if cfg!(windows) {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", &format!("{program} --version")]);
        command
    } else {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", &format!("{program} --version")]);
        command
    };
    // Corepack asks before it downloads a manager, and nobody is there to answer.
    command
        .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if started.elapsed() > VERSION_PATIENCE => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    // `cargo --version` says `cargo 1.89.0 (...)`; the others say the number.
    let version = line.strip_prefix(program).map(str::trim).unwrap_or(line);
    Some(version.split_whitespace().next().unwrap_or(version).to_string())
}

// --- the keyring ----------------------------------------------------------------------

fn keyring() -> CheckView {
    if std::env::var(crate::secrets::BACKEND_ENV).is_ok_and(|value| value == "insecure-file") {
        return CheckView::warn(
            ENVIRONMENT,
            "keyring",
            None,
            format!(
                "secrets are kept in a plain file ({}=insecure-file) - for tests only",
                crate::secrets::BACKEND_ENV
            ),
        )
        .fix(format!("unset {}", crate::secrets::BACKEND_ENV));
    }
    match crate::secrets::probe() {
        Ok(backend) => CheckView::ok(ENVIRONMENT, "keyring", None, format!("keyring answers ({backend})")),
        Err(err) => CheckView::fail(ENVIRONMENT, "keyring", None, format!("the OS keyring does not answer: {err:#}"))
            .fix("passwords can be neither stored nor read until it does - see the keyring of this OS"),
    }
}

// --- one app --------------------------------------------------------------------------

fn app_checks(app: &App, catalogs: &Catalogs, gateway: &GatewayState, tools: &Tools) -> Vec<CheckView> {
    let name = Some(app.name.as_str());
    let mut checks = Vec::new();
    let dir = Path::new(&app.path);
    let dir_ok = dir.is_dir();
    if dir_ok {
        checks.push(CheckView::ok(APPS, "project", name, format!("project {}", app.path)));
        checks.extend(project_checks(app, dir, tools));
    } else {
        checks.push(
            CheckView::fail(APPS, "project", name, format!("project directory {} is not there", app.path))
                .fix(format!("turnout app edit {} --path DIR", app.name)),
        );
    }
    if let Some(port) = app.dev_port {
        checks.push(dev_port_check(app, port));
    }
    if let Some(port) = app.gateway_port {
        checks.push(gateway_port_check(app, port, catalogs, gateway));
    }
    if dir_ok {
        checks.extend(env_file_checks(app));
    }
    checks
}

/// What a JavaScript project needs before any command of it runs.
fn project_checks(app: &App, dir: &Path, tools: &Tools) -> Vec<CheckView> {
    let name = Some(app.name.as_str());
    let mut checks = Vec::new();
    if !dir.join("package.json").is_file() {
        return checks;
    }
    let used: BTreeSet<String> = app
        .commands
        .values()
        .filter_map(|line| runner(line))
        .filter(|runner| ["pnpm", "yarn", "npm", "bun"].contains(&runner.as_str()))
        .collect();
    let locked = lockfile_manager(dir);
    let manager = used
        .iter()
        .next()
        .cloned()
        .or_else(|| locked.map(str::to_string))
        .unwrap_or_else(|| "npm".to_string());
    if let Some(locked) = locked
        && let Some(other) = used.iter().find(|runner| runner.as_str() != locked)
    {
        checks.push(
            CheckView::warn(
                APPS,
                "manager",
                name,
                format!("the commands run {other}, but the lockfile is {locked}'s - each installs what the other does not expect"),
            )
            .fix(format!("turnout app edit {} --command dev=\"{locked} dev\" (and the others)", app.name)),
        );
    }
    if !has_node_modules(dir) {
        checks.push(
            CheckView::warn(APPS, "dependencies", name, "dependencies are not installed - there is no node_modules")
                .fix(format!("cd \"{}\" && {manager} install", app.path)),
        );
    }
    if let Some((asked, file)) = node_wanted(dir)
        && let Some(installed) = tools.version("node")
        && major(installed).is_some_and(|major| major != asked)
    {
        checks.push(
            CheckView::warn(
                APPS,
                "node_version",
                name,
                format!("{file} asks for node {asked}, and node {installed} is on PATH"),
            )
            .fix(format!("switch to node {asked} (nvm use, fnm use)")),
        );
    }
    checks
}

/// The directories from `dir` up to the repository root, or to the top when
/// there is no repository: where a monorepo keeps its lockfile and its
/// hoisted `node_modules`.
fn up_to_root(dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut at = Some(dir);
    while let Some(current) = at {
        dirs.push(current.to_path_buf());
        if current.join(".git").exists() {
            break;
        }
        at = current.parent();
    }
    dirs
}

/// Whose lockfile the project has: the nearest one up to the repository root.
fn lockfile_manager(dir: &Path) -> Option<&'static str> {
    const LOCKS: [(&str, &str); 5] = [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("package-lock.json", "npm"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
    ];
    up_to_root(dir)
        .iter()
        .find_map(|at| LOCKS.iter().find(|(file, _)| at.join(file).is_file()).map(|(_, manager)| *manager))
}

/// Whether dependencies are installed here or where a workspace hoists them.
fn has_node_modules(dir: &Path) -> bool {
    up_to_root(dir).iter().any(|at| at.join("node_modules").is_dir())
}

/// The Node major the project pins in `.nvmrc` or `.node-version`, and the
/// file that pins it. An alias (`lts/*`, `node`) pins no number and is left
/// alone.
fn node_wanted(dir: &Path) -> Option<(u32, &'static str)> {
    up_to_root(dir).iter().find_map(|at| {
        [".nvmrc", ".node-version"].into_iter().find_map(|file| {
            let text = std::fs::read_to_string(at.join(file)).ok()?;
            Some((major(text.trim())?, file))
        })
    })
}

/// The major number of a version as tools write it: `v22.11.0`, `20`, `18.19`.
fn major(version: &str) -> Option<u32> {
    version.trim().trim_start_matches('v').split('.').next()?.parse().ok()
}

/// Who holds the app's dev port, and whether that is the app's own dev job.
fn dev_port_check(app: &App, port: u16) -> CheckView {
    let job = registry::load(&registry::command_key(&app.name, "dev"))
        .ok()
        .flatten()
        .filter(registry::Entry::is_running);
    // Where a running job's server came up says whether the port is its own:
    // a server that had to move on did so because something sat there first.
    let landed = job
        .as_ref()
        .and_then(|job| job.ready.as_ref())
        .and_then(|ready| ready.url.as_deref())
        .and_then(port_in);
    judge_dev_port(app, port, job.is_some(), landed, ports::holder(port))
}

/// The pure half of [`dev_port_check`]: whether the app's dev job runs, the
/// port its server came up on, and who holds the dev port.
fn judge_dev_port(app: &App, port: u16, running: bool, landed: Option<u16>, holder: Option<Holder>) -> CheckView {
    let name = Some(app.name.as_str());
    match (running, holder) {
        (false, None) => CheckView::ok(APPS, "dev_port", name, format!("dev port {port} is free")),
        (true, Some(holder)) if landed.is_some_and(|landed| landed != port) => stray(
            app,
            port,
            &holder,
            State::Fail,
            format!(
                "dev port {port} is held by {} - the running dev server moved to {}, and the front door shows the old one",
                holder.describe(),
                landed.unwrap_or_default()
            ),
        ),
        (true, _) => CheckView::ok(APPS, "dev_port", name, format!("dev port {port} - the running `{} dev`", app.name)),
        (false, Some(holder)) => stray(
            app,
            port,
            &holder,
            State::Warn,
            format!(
                "dev port {port} is held by {}, which turnout did not start - `turnout dev {}` will not run over it",
                holder.describe(),
                app.name
            ),
        ),
    }
}

/// The line for a process sitting on the app's dev port that is not the
/// app's own dev server: one a closed terminal left behind, or one started by
/// hand. `--fix` can end it when the OS says which process it is.
fn stray(app: &App, port: u16, holder: &Holder, state: State, message: String) -> CheckView {
    let check = CheckView::new(APPS, "dev_port", state, Some(&app.name), message);
    match holder.pid {
        Some(pid) => check.repair(Repair::EndProcess {
            app: app.name.clone(),
            port,
            pid,
            name: holder.name.clone().unwrap_or_else(|| "a process".to_string()),
        }),
        None => check.fix("end the process that holds it - the OS does not say which one it is"),
    }
}

/// The port an address names: `http://localhost:5101/` -> 5101.
fn port_in(url: &str) -> Option<u16> {
    let authority = url.split("://").nth(1)?.split('/').next()?;
    authority.rsplit_once(':')?.1.parse().ok()
}

fn gateway_port_check(app: &App, port: u16, catalogs: &Catalogs, gateway: &GatewayState) -> CheckView {
    let name = Some(app.name.as_str());
    if let Some(other) = catalogs.apps.iter().find(|other| other.name != app.name && other.gateway_port == Some(port)) {
        return CheckView::fail(
            APPS,
            "gateway_port",
            name,
            format!("gateway port {port} is also '{}''s - the gateway refuses to start", other.name),
        )
        .fix(format!("turnout app edit {} --port PORT", app.name));
    }
    match gateway {
        GatewayState::Running(running) if running.ports.get(&port).is_some_and(|owner| owner == &app.name) => {
            CheckView::ok(APPS, "gateway_port", name, format!("the gateway listens on {port}"))
        }
        GatewayState::Running(_) => CheckView::warn(
            APPS,
            "gateway_port",
            name,
            format!("the gateway was started before the app had port {port}, and does not listen there"),
        )
        .fix("turnout gateway stop, then turnout gateway start (the stands' sessions start over)"),
        _ => match ports::holder(port) {
            None => CheckView::ok(APPS, "gateway_port", name, format!("gateway port {port} is free")),
            Some(holder) => CheckView::fail(
                APPS,
                "gateway_port",
                name,
                format!("gateway port {port} is held by {} - the gateway cannot start", holder.describe()),
            )
            .fix(format!("end that process, or turnout app edit {} --port PORT", app.name)),
        },
    }
}

/// The app's dotenv file against its port, in git, and in this shell.
fn env_file_checks(app: &App) -> Vec<CheckView> {
    let name = Some(app.name.as_str());
    let file = app.env_file_name();
    let mut checks = Vec::new();
    let in_git = crate::envfile::in_git(app);
    let fixable = |check: CheckView| check.repair(Repair::EnvFile { app: app.name.clone() });
    match crate::envfile::drift(app) {
        Ok(Drift::Nothing) => {}
        Ok(Drift::InStep) => {
            let url = app.gateway_url().unwrap_or_default();
            let note = if in_git == InGit::Ignored { " (ignored by git)" } else { "" };
            checks.push(CheckView::ok(
                APPS,
                "env_file",
                name,
                format!("{file} says {}={url}{note}", app.gateway_env_name()),
            ));
        }
        Ok(Drift::Missing) => checks.push(fixable(CheckView::warn(
            APPS,
            "env_file",
            name,
            format!("{file} has no gateway line - the app started outside turnout does not find the gateway"),
        ))),
        Ok(Drift::Stale { says }) => {
            let says = if says.is_empty() { "nothing under turnout's line".to_string() } else { says };
            checks.push(fixable(CheckView::fail(
                APPS,
                "env_file",
                name,
                format!(
                    "{file} says {says}; the gateway is at {} - the app started outside turnout talks to the wrong port",
                    app.gateway_url().unwrap_or_default()
                ),
            )));
        }
        Ok(Drift::Shadowed { line }) => checks.push(
            CheckView::fail(
                APPS,
                "env_file",
                name,
                format!("{file} sets the variable again further down ({line}) - that line wins over turnout's"),
            )
            .fix(format!("remove that line from {}", crate::envfile::path_of(app).display())),
        ),
        Ok(Drift::Leftover) => checks.push(fixable(CheckView::warn(
            APPS,
            "env_file",
            name,
            format!("{file} still names a gateway port the app no longer has"),
        ))),
        Err(err) => checks.push(CheckView::fail(APPS, "env_file", name, format!("{err:#}"))),
    }
    let file_exists = crate::envfile::path_of(app).is_file();
    match in_git {
        InGit::Tracked => checks.push(
            CheckView::fail(
                APPS,
                "env_file_git",
                name,
                format!("{file} is committed - the local address travels to everyone who pulls, and ignore rules no longer apply"),
            )
            .fix(format!("git rm --cached \"{file}\" in {}, then commit", app.path)),
        ),
        InGit::Noticed if file_exists => checks.push(
            CheckView::warn(
                APPS,
                "env_file_git",
                name,
                format!("{file} is not ignored by git - every rewrite shows up as a change"),
            )
            .repair(Repair::Ignore { app: app.name.clone() }),
        ),
        _ => {}
    }
    if let Some(url) = app.gateway_url()
        && let Ok(set) = std::env::var(app.gateway_env_name())
        && set != url
    {
        checks.push(
            CheckView::warn(
                APPS,
                "env_shell",
                name,
                format!(
                    "this shell sets {}={set}, and that wins over {file} for an app started from it",
                    app.gateway_env_name()
                ),
            )
            .fix(format!("unset {} in the shell profile", app.gateway_env_name())),
        );
    }
    checks
}

// --- the gateway ------------------------------------------------------------------------

fn gateway_checks(catalogs: &Catalogs, gateway: &GatewayState) -> Vec<CheckView> {
    let bound = !catalogs.state.bindings.is_empty();
    let mut checks = Vec::new();
    if let Err(err) = crate::gateway::listening_ports(&catalogs.apps)
        && catalogs.apps.iter().any(|app| app.gateway_port.is_some())
    {
        checks.push(CheckView::fail(GATEWAY, "gateway", None, format!("{err:#}")));
    }
    match gateway {
        GatewayState::Running(running) => {
            let door = match running.front_port {
                Some(port) => format!(" · front door {}", crate::front::door(port)),
                None => String::new(),
            };
            checks.push(CheckView::ok(GATEWAY, "gateway", None, format!("gateway running · pid {}{door}", running.pid)));
            if running.front_port.is_none() {
                let held: Vec<String> = [crate::front::PORT, crate::front::FALLBACK_PORT]
                    .iter()
                    .map(|port| match ports::holder(*port) {
                        Some(holder) => format!("{port} by {}", holder.describe()),
                        None => format!("{port} by nobody now"),
                    })
                    .collect();
                checks.push(
                    CheckView::warn(
                        GATEWAY,
                        "front_door",
                        None,
                        format!("the front door is closed - ports held: {}", held.join(", ")),
                    )
                    .fix(format!("free one of them and restart the gateway, or set {}", crate::front::ENV_PORT)),
                );
            }
            match &running.control {
                None => checks.push(
                    CheckView::warn(
                        GATEWAY,
                        "control",
                        None,
                        "the gateway was started by an older turnout - it takes no cookies and answers no checks",
                    )
                    .fix("turnout gateway stop, then turnout gateway start"),
                ),
                Some(control) => {
                    if let Err(err) = crate::control::jars(control) {
                        checks.push(
                            CheckView::fail(GATEWAY, "control", None, format!("the gateway's control channel does not answer: {err:#}"))
                                .fix("turnout gateway stop, then turnout gateway start"),
                        );
                    }
                }
            }
        }
        GatewayState::NotAnswering(running) => checks.push(
            CheckView::fail(
                GATEWAY,
                "gateway",
                None,
                format!("the gateway (pid {}) is alive and does not answer on its ports", running.pid),
            )
            .fix("turnout gateway stop, then turnout gateway start"),
        ),
        GatewayState::Gone(pid) => checks.push(
            CheckView::warn(
                GATEWAY,
                "gateway",
                None,
                format!("the gateway (pid {pid}) is gone and its record stayed behind"),
            )
            .repair(Repair::GatewayRecord),
        ),
        GatewayState::Stopped if bound => {
            checks.push(CheckView::warn(GATEWAY, "gateway", None, "the gateway is not running - the bound apps reach no stand").fix("turnout gateway start"))
        }
        GatewayState::Stopped => checks.push(CheckView::info(
            GATEWAY,
            "gateway",
            None,
            "the gateway is not running - no app is bound to a stand",
        )),
    }
    checks
}

// --- stands and sessions --------------------------------------------------------------------

/// Every stand in the catalog, probed at once. A stand an app is bound to and
/// that does not answer is a problem; one nobody uses right now is a warning.
fn stands(catalogs: &Catalogs) -> Vec<CheckView> {
    let servers: Vec<&Server> = catalogs.servers.iter().collect();
    let health = crate::health::probe_all(&servers);
    servers
        .iter()
        .zip(health)
        .map(|(server, health)| {
            let name = Some(server.name.as_str());
            let url = crate::health::health_url(&server.url);
            let bound = catalogs.state.bindings.values().any(|bound| bound == &server.name);
            let trouble = |message: String| {
                if bound {
                    CheckView::fail(STANDS, "stand", name, message)
                } else {
                    CheckView::warn(STANDS, "stand", name, message)
                }
            };
            match health {
                Health::Up { status, millis } => CheckView::ok(STANDS, "stand", name, format!("answers · {url} {status} in {millis} ms")),
                Health::Down { status } => trouble(format!("answers {status} at {url} - the stand is down")),
                Health::Unreachable => trouble(format!("does not answer at {url} - it is off, or out of reach")),
                Health::Untrusted => {
                    trouble("answers with a certificate this machine does not trust".to_string()).fix(format!("turnout server edit {} --insecure", server.name))
                }
            }
        })
        .collect()
}

/// The session of every bound app, asked of the stand through the gateway.
fn sessions(catalogs: &Catalogs, gateway: &GatewayState) -> Vec<CheckView> {
    let GatewayState::Running(running) = gateway else {
        return Vec::new();
    };
    let Some(control) = &running.control else {
        return Vec::new();
    };
    let jars = crate::control::jars(control).unwrap_or_default();
    let mut checks = Vec::new();
    for (app, server) in &catalogs.state.bindings {
        if !catalogs.apps.iter().any(|a| &a.name == app && a.gateway_port.is_some()) || !catalogs.servers.iter().any(|s| &s.name == server) {
            continue;
        }
        let jar = jars.iter().find(|jar| &jar.app == app && &jar.server == server);
        let check = match crate::control::check(control, app, server) {
            Ok(check) => check,
            Err(err) => {
                checks.push(CheckView::warn(
                    STANDS,
                    "session",
                    Some(app),
                    format!("the session on {server} could not be checked: {err:#}"),
                ));
                continue;
            }
        };
        checks.push(session_line(app, server, &check, jar, running.front_port));
    }
    checks
}

/// The line one session check makes.
fn session_line(app: &str, server: &str, check: &crate::control::SessionCheck, jar: Option<&crate::control::JarView>, door: Option<u16>) -> CheckView {
    let name = Some(app);
    let asked = check.path.as_deref().map(|path| format!("GET {path}")).unwrap_or_default();
    let status = check.status.map(|status| status.to_string()).unwrap_or_default();
    let login = match door {
        Some(door) => format!("log in at {}, or turnout cookie set {app}", crate::front::address(app, door)),
        None => format!("log in through the app again, or turnout cookie set {app}"),
    };
    match check.verdict {
        Verdict::Alive => {
            let expires = jar
                .and_then(|jar| jar.expires.as_deref())
                .and_then(output::parse_iso8601)
                .map(|at| format!(" · expires in {}", crate::progress::human_span(at.saturating_sub(registry::now()))))
                .unwrap_or_default();
            CheckView::ok(
                STANDS,
                "session",
                name,
                format!("session on {server} is alive - {asked} answers {status}{expires}"),
            )
        }
        Verdict::Expired if check.status.is_none() => {
            CheckView::fail(STANDS, "session", name, format!("session on {server} expired - its cookies ran out")).fix(login)
        }
        Verdict::Expired => CheckView::fail(STANDS, "session", name, format!("session on {server} expired - {asked} now answers {status}")).fix(login),
        Verdict::Unproven => CheckView::info(
            STANDS,
            "session",
            name,
            format!("session on {server}: cookies held, and no request has used them yet - nothing to check"),
        ),
        Verdict::Empty => CheckView::info(
            STANDS,
            "session",
            name,
            format!("no session on {server} - nothing has logged in through the gateway"),
        ),
        Verdict::Down | Verdict::Unreachable => CheckView::warn(
            STANDS,
            "session",
            name,
            format!("session on {server} could not be checked - the stand does not answer it"),
        ),
        Verdict::Unclear => CheckView::warn(
            STANDS,
            "session",
            name,
            format!("session on {server} is unclear - {asked} answered 2xx before and {status} now"),
        ),
    }
}

// --- the catalog ------------------------------------------------------------------------------

fn catalog_checks(catalogs: &Catalogs) -> Vec<CheckView> {
    let mut checks = Vec::new();
    let server_named = |name: &str| catalogs.servers.iter().any(|s| s.name == name);
    for (app, server) in &catalogs.state.bindings {
        if !server_named(server) {
            checks.push(
                CheckView::fail(CATALOG, "binding", Some(app), format!("bound to '{server}', which is not in the catalog"))
                    .repair(Repair::Unbind { app: app.clone() }),
            );
        }
    }
    for server in &catalogs.servers {
        if let Some(credential) = &server.credential
            && !catalogs.credentials.iter().any(|c| &c.name == credential)
        {
            checks.push(
                CheckView::fail(
                    CATALOG,
                    "server",
                    Some(&server.name),
                    format!("logs in as '{credential}', which is not in the catalog"),
                )
                .fix(format!("turnout server edit {} --credential NAME", server.name)),
            );
        }
    }
    for target in &catalogs.targets {
        let missing: Vec<String> = [
            ("app", catalogs.apps.iter().any(|a| a.name == target.app), &target.app),
            ("server", server_named(&target.server), &target.server),
            (
                "credential",
                catalogs.credentials.iter().any(|c| c.name == target.credential),
                &target.credential,
            ),
            ("path", catalogs.paths.iter().any(|p| p.name == target.path), &target.path),
        ]
        .into_iter()
        .filter(|(_, present, _)| !present)
        .map(|(kind, _, name)| format!("{kind} '{name}'"))
        .collect();
        if !missing.is_empty() {
            checks.push(
                CheckView::fail(
                    CATALOG,
                    "target",
                    Some(&target.name),
                    format!("names {} - not in the catalog", missing.join(", ")),
                )
                .fix(format!("turnout target edit {0}, or turnout target remove {0}", target.name)),
            );
        }
    }
    let mut agent_users = Vec::new();
    for credential in &catalogs.credentials {
        let name = Some(credential.name.as_str());
        match credential.auth {
            Auth::Password if !credential.secret_stored => checks.push(
                CheckView::warn(CATALOG, "credential", name, "has no stored password - a deploy as it stops at the sign-in")
                    .fix(format!("turnout pass set {}", credential.name)),
            ),
            Auth::Key => match credential.key.as_deref() {
                None => checks.push(
                    CheckView::fail(CATALOG, "credential", name, "signs in by key and names no key file")
                        .fix(format!("turnout credential edit {} --key PATH", credential.name)),
                ),
                Some(key) if !Path::new(key).is_file() => checks.push(
                    CheckView::fail(CATALOG, "credential", name, format!("its key file {key} is not there"))
                        .fix(format!("turnout credential edit {} --key PATH", credential.name)),
                ),
                Some(_) => {}
            },
            Auth::Agent => agent_users.push(credential.name.clone()),
            Auth::Password => {}
        }
    }
    if !agent_users.is_empty()
        && let Err(err) = crate::utils::run_blocking(crate::agent::connect())
    {
        let (message, fix) = match format!("{err:#}").split_once(" - ") {
            Some((what, how)) => (what.to_string(), how.to_string()),
            None => (format!("{err:#}"), "start the SSH agent and add the key".to_string()),
        };
        checks.push(
            CheckView::warn(
                CATALOG,
                "agent",
                None,
                format!(
                    "{message} - {} {} through one",
                    agent_users.join(", "),
                    if agent_users.len() == 1 { "signs in" } else { "sign in" }
                ),
            )
            .fix(fix),
        );
    }
    if checks.is_empty() {
        checks.push(CheckView::ok(
            CATALOG,
            "catalog",
            None,
            format!(
                "catalog: {} {}, {} {}, {} {} - every name resolves",
                catalogs.servers.len(),
                plural(catalogs.servers.len(), "server", "servers"),
                catalogs.credentials.len(),
                plural(catalogs.credentials.len(), "credential", "credentials"),
                catalogs.targets.len(),
                plural(catalogs.targets.len(), "target", "targets"),
            ),
        ));
    }
    checks
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

// --- --fix ----------------------------------------------------------------------------------

/// Carry out the repairs the checks offer; what was done, in words.
fn repair(checks: &[CheckView], assume_yes: bool) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for check in checks {
        let Some(repair) = &check.repair else { continue };
        match repair {
            Repair::EnvFile { app } => {
                let Some(app) = store::load_apps()?.into_iter().find(|a| &a.name == app) else {
                    continue;
                };
                match crate::envfile::write(&app)? {
                    crate::envfile::Outcome::Written(assignment) => done.push(format!("{}: wrote {assignment} to {}", app.name, app.env_file_name())),
                    crate::envfile::Outcome::Removed => done.push(format!("{}: took the old gateway line out of {}", app.name, app.env_file_name())),
                    _ => {}
                }
            }
            Repair::Ignore { app } => {
                let Some(app) = store::load_apps()?.into_iter().find(|a| &a.name == app) else {
                    continue;
                };
                crate::envfile::keep_out_of_git(&app)?;
                done.push(format!("{}: {} is in .gitignore now", app.name, app.env_file_name()));
            }
            Repair::GatewayRecord => {
                registry::remove(registry::GATEWAY)?;
                done.push("cleared the record of the gateway that is gone".to_string());
            }
            Repair::Unbind { app } => {
                let mut state = store::load_state()?;
                if let Some(server) = state.bindings.remove(app) {
                    store::save_state(&state)?;
                    done.push(format!("{app}: unbound from '{server}', which is not in the catalog"));
                }
            }
            Repair::EndProcess { app, port, pid, name } => {
                if let Some(line) = end_stray(app, *port, *pid, name, assume_yes)? {
                    done.push(line);
                }
            }
        }
    }
    Ok(done)
}

/// End the process holding an app's dev port, once somebody said yes.
///
/// Off a terminal and without `--yes` it is left alone: ending a process
/// found by its port is a decision, and turnout cannot tell a dev server left
/// behind from one somebody is using right now.
fn end_stray(app: &str, port: u16, pid: u32, name: &str, assume_yes: bool) -> Result<Option<String>> {
    // The table is read again: the process may have gone, or another taken
    // the port, since the check looked.
    if ports::holder(port).and_then(|holder| holder.pid) != Some(pid) {
        return Ok(None);
    }
    if !assume_yes && !pick::interactive() {
        eprintln!("note: left {name} (pid {pid}) on {app}'s dev port {port} running - ending a process needs a yes; run this in a terminal, or pass --yes");
        return Ok(None);
    }
    if !pick::confirm_destructive(format!("End {name} (pid {pid}), which holds {app}'s dev port {port}?"), assume_yes)? {
        return Ok(None);
    }
    end_holder(pid, port)?;
    Ok(Some(format!("{app}: ended {name} (pid {pid}), which held dev port {port}")))
}

/// End `pid` and wait for `port` to be let go: the next thing to happen is
/// usually a `dev` that wants it.
pub(crate) fn end_holder(pid: u32, port: u16) -> Result<()> {
    process::end(pid, None, process::Ending::Kill)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while ports::holder(port).is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

// --- the checklist on screen ------------------------------------------------------------------

fn text(view: &DoctorView) -> String {
    let bar = style::dim("│");
    let mut out = Vec::new();
    for line in &view.fixed {
        out.push(format!("{}  {line}", style::good("✓")));
    }
    if !view.fixed.is_empty() {
        out.push(String::new());
    }
    let title = match &view.server {
        Some(server) => format!("Server {server}"),
        None => "Local environment".to_string(),
    };
    out.push(format!("{}  {}", style::dim("┌"), style::heading(&title)));
    out.push(bar.clone());
    let mut section = None;
    for check in &view.checks {
        if section.is_some_and(|section| section != check.section) {
            out.push(bar.clone());
        }
        section = Some(check.section);
        out.push(line(check));
    }
    out.push(bar);
    out.push(format!("{}  {}", style::dim("└"), summary(view)));
    out.join("\n")
}

fn line(check: &CheckView) -> String {
    let mark = match check.state {
        State::Ok => style::good("◇"),
        State::Info => style::dim("●"),
        State::Warn => style::warn("▲"),
        State::Fail => style::bad("✗"),
    };
    let subject = check
        .subject
        .as_deref()
        .map(|subject| format!("{} · ", style::name(subject)))
        .unwrap_or_default();
    let fix = check.fix.as_deref().map(|fix| style::dim(&format!(" · fix: {fix}"))).unwrap_or_default();
    format!("{mark}  {subject}{}{fix}", check.message)
}

/// The closing line: the counts, what `--fix` would take care of, and for a
/// server the deploy step the first problem stops.
fn summary(view: &DoctorView) -> String {
    if view.problems == 0 && view.warnings == 0 {
        return style::good("Everything checks out");
    }
    let mut parts = Vec::new();
    if view.problems > 0 {
        parts.push(style::bad(&format!("{} {}", view.problems, plural(view.problems, "problem", "problems"))));
    }
    if view.warnings > 0 {
        parts.push(style::warn(&format!("{} {}", view.warnings, plural(view.warnings, "warning", "warnings"))));
    }
    let fixable = view.checks.iter().filter(|check| check.fixable).count();
    if fixable > 0 {
        parts.push(format!("`turnout doctor --fix` repairs {fixable}"));
    }
    if let Some(step) = view.deploy_stops_at {
        parts.push(format!("a deploy would stop at the {step} step"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runner_is_the_first_word_that_is_not_an_assignment() {
        assert_eq!(runner("pnpm dev").as_deref(), Some("pnpm"));
        assert_eq!(runner("NODE_ENV=development yarn start").as_deref(), Some("yarn"));
        assert_eq!(runner("npm.cmd run dev").as_deref(), Some("npm"));
        assert_eq!(runner("C:\\tools\\node.exe server.js").as_deref(), Some("node"));
        assert_eq!(runner("vite --port {port}"), None, "a project's own binary is not a tool on PATH");
        assert_eq!(runner(""), None);
    }

    #[test]
    fn a_major_is_read_from_how_tools_write_versions() {
        assert_eq!(major("v22.11.0"), Some(22));
        assert_eq!(major("20"), Some(20));
        assert_eq!(major("18.19\n"), Some(18));
        assert_eq!(major("lts/*"), None);
        assert_eq!(major("node"), None);
    }

    /// A monorepo keeps its lockfile, its `.nvmrc` and its hoisted
    /// `node_modules` at the root; the app two levels down has them all.
    #[test]
    fn a_workspace_app_finds_what_its_root_holds() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(repo.path().join(".nvmrc"), "v20.11.0\n").unwrap();
        let app = repo.path().join("apps").join("web");
        std::fs::create_dir_all(&app).unwrap();
        assert_eq!(lockfile_manager(&app), Some("pnpm"));
        assert_eq!(node_wanted(&app), Some((20, ".nvmrc")));
        assert!(!has_node_modules(&app));
        std::fs::create_dir(repo.path().join("node_modules")).unwrap();
        assert!(has_node_modules(&app), "hoisted dependencies count");
        // The app's own file wins over the root's.
        std::fs::write(app.join(".node-version"), "22\n").unwrap();
        assert_eq!(node_wanted(&app), Some((22, ".node-version")));
    }

    /// Nothing above the repository root is the project's: an outer
    /// directory's lockfile does not count.
    #[test]
    fn the_walk_stops_at_the_repository_root() {
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join("yarn.lock"), "").unwrap();
        let repo = outer.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(lockfile_manager(&repo), None);
    }

    fn an_app() -> App {
        App {
            name: "web".into(),
            path: String::new(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: None,
            gateway_env: None,
            env_file: None,
            dev_port: Some(5100),
            servers: Vec::new(),
        }
    }

    fn node(pid: Option<u32>) -> Option<Holder> {
        Some(Holder {
            pid,
            name: Some("node.exe".into()),
        })
    }

    /// Every way the dev port can be held, and what each one means.
    #[test]
    fn the_dev_port_is_judged_by_who_holds_it() {
        let app = an_app();
        let free = judge_dev_port(&app, 5100, false, None, None);
        assert_eq!((free.state, free.fixable), (State::Ok, false));

        let own = judge_dev_port(&app, 5100, true, Some(5100), node(Some(42)));
        assert_eq!(own.state, State::Ok, "the app's own dev server: {}", own.message);

        let stray = judge_dev_port(&app, 5100, false, None, node(Some(42)));
        assert_eq!((stray.state, stray.fixable), (State::Warn, true));
        assert!(stray.message.contains("node.exe (pid 42), which turnout did not start"), "{}", stray.message);

        // The job runs, but its server had to move: the front door shows the old one.
        let moved = judge_dev_port(&app, 5100, true, Some(5101), node(Some(42)));
        assert_eq!((moved.state, moved.fixable), (State::Fail, true));
        assert!(moved.message.contains("moved to 5101"), "{}", moved.message);

        // A holder the OS will not name cannot be ended by --fix; the line says so.
        let unseen = judge_dev_port(&app, 5100, false, None, node(None));
        assert_eq!((unseen.state, unseen.fixable), (State::Warn, false));
        assert!(unseen.fix.as_deref().is_some_and(|fix| fix.contains("does not say")), "{unseen:?}");
    }

    #[test]
    fn the_port_comes_out_of_an_address() {
        assert_eq!(port_in("http://localhost:5101/"), Some(5101));
        assert_eq!(port_in("http://127.0.0.1:5100"), Some(5100));
        assert_eq!(port_in("http://web.localhost/"), None);
    }

    fn check(state: State, section: &'static str) -> CheckView {
        CheckView::new(section, "x", state, None, "message")
    }

    /// The counts are the lines; only failures are problems.
    #[test]
    fn problems_are_failures_and_warnings_are_counted_apart() {
        let view = DoctorView::new(
            None,
            vec![
                check(State::Ok, ENVIRONMENT),
                check(State::Warn, APPS),
                check(State::Fail, APPS),
                check(State::Info, GATEWAY),
                check(State::Fail, STANDS),
            ],
            None,
            Vec::new(),
        );
        assert_eq!((view.problems, view.warnings), (2, 1));
        assert!(summary(&view).contains("2 problems"), "{}", summary(&view));
        let clean = DoctorView::new(None, vec![check(State::Ok, ENVIRONMENT), check(State::Info, GATEWAY)], None, Vec::new());
        assert_eq!(summary(&clean), "Everything checks out");
    }

    /// The checklist is the approved frame: a title, a bar between sections,
    /// a mark per line, the fix after it, and the counts at the bottom.
    #[test]
    fn the_checklist_reads_as_a_clack_frame() {
        let checks = vec![
            CheckView::ok(ENVIRONMENT, "tools", None, "node v22.11.0 · pnpm 10.34.5"),
            CheckView::warn(APPS, "env_file", Some("web"), ".env.development.local has no gateway line").repair(Repair::EnvFile { app: "web".into() }),
            CheckView::fail(STANDS, "stand", Some("prod"), "does not answer within 3s").fix("check the VPN"),
        ];
        let rendered = text(&DoctorView::new(None, checks, None, vec!["web: wrote X=1".into()]));
        let expected = "✓  web: wrote X=1\n\
             \n\
             ┌  Local environment\n\
             │\n\
             ◇  node v22.11.0 · pnpm 10.34.5\n\
             │\n\
             ▲  web · .env.development.local has no gateway line · fix: turnout doctor --fix\n\
             │\n\
             ✗  prod · does not answer within 3s · fix: check the VPN\n\
             │\n\
             └  1 problem · 1 warning · `turnout doctor --fix` repairs 1";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn a_server_checklist_names_the_step_a_deploy_would_stop_at() {
        let view = DoctorView::new(Some("prod".into()), vec![check(State::Fail, "paths")], Some("upload"), Vec::new());
        let rendered = text(&view);
        assert!(rendered.contains("┌  Server prod"), "{rendered}");
        assert!(rendered.ends_with("└  1 problem · a deploy would stop at the upload step"), "{rendered}");
    }
}
