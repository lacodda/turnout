//! `turnout tunnel`: open a tunnel in the background, list the tunnels, close
//! them. The tunnel itself is [`crate::tunnel`]; this is its console.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::cli::{Format, TunnelCommand};
use crate::process;
use crate::progress::human_span;
use crate::registry::{self, Entry, Exit, Status};
use crate::tunnel::{self, Route};

/// How long a tunnel gets to listen, sign in and hear from the far end before
/// opening it counts as failed. Signing in alone may take twenty seconds
/// before the tunnel itself gives up on it.
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

pub fn run(command: Option<TunnelCommand>, route: Option<String>, ports: Option<String>, credential: Option<String>) -> Result<()> {
    match command {
        Some(TunnelCommand::List { format }) => list(format),
        Some(TunnelCommand::Stop { names, all }) => stop(names, all),
        Some(TunnelCommand::Run {
            name,
            server,
            credential,
            local_port,
            host,
            port,
            log,
        }) => tunnel::run(
            Route {
                name,
                server,
                credential,
                local_port,
                host,
                port,
            },
            log,
        ),
        None => {
            // clap requires both without a subcommand; the message is for a
            // caller that reaches here some other way.
            let (Some(route), Some(ports)) = (route, ports) else {
                bail!("name a tunnel to open - `turnout tunnel NAME@SERVER PORT` - or see `turnout tunnel list`");
            };
            open(&route, &ports, credential)
        }
    }
}

/// The route a command line asks for, resolved against the catalogs.
fn resolve(route: &str, ports: &str, credential: Option<String>) -> Result<Route> {
    let ports = tunnel::parse_ports(ports)?;
    let (name, through) = tunnel::parse_route(route)?;
    let host =
        crate::remote::resolve_machine(&through, credential)?.ok_or_else(|| anyhow!("no server or target named '{through}' - see `turnout server list`"))?;
    let name = match name {
        Some(name) => name,
        None => {
            let name = tunnel::default_name(&through, ports.port);
            crate::model::validate_name(&name).map_err(|_| anyhow!("'{name}' cannot name a tunnel - give it a name: `turnout tunnel NAME@{through} ...`"))?;
            name
        }
    };
    Ok(Route {
        name,
        server: host.server.name,
        credential: host.credential.name,
        local_port: ports.local,
        host: ports.host,
        port: ports.port,
    })
}

/// `turnout tunnel NAME@SERVER PORTS`: start the tunnel in the background and
/// return once it carries - or with its reason when it cannot.
fn open(spec: &str, ports: &str, credential: Option<String>) -> Result<()> {
    let route = resolve(spec, ports, credential)?;
    let key = registry::tunnel_key(&route.name);

    // The same tunnel asked for again is there already: a script that opens
    // what it needs before it starts gets on with it.
    if let Some(running) = registry::load(&key)?.filter(Entry::is_running) {
        let open = running.tunnel().cloned().unwrap_or_else(|| route.clone());
        if open == route {
            println!("Tunnel {} is already open: {} (pid {}).", route.name, route.describe(), running.pid);
            return Ok(());
        }
        bail!(
            "tunnel {} is already open to something else: {} - close it first with `turnout tunnel stop {0}`, or give this one another name",
            route.name,
            open.describe()
        );
    }
    // A port one of our own tunnels holds is said as that tunnel, not as
    // "turnout.exe".
    if let Some(other) = registry::list()?
        .into_iter()
        .filter(Entry::is_running)
        .filter_map(|entry| entry.tunnel().cloned())
        .find(|other| other.local_port == route.local_port)
    {
        bail!(
            "port {} here already carries tunnel {} ({}) - close it with `turnout tunnel stop {}`, or listen on another port: `turnout tunnel {}@{} LOCAL:{}`",
            route.local_port,
            other.name,
            other.describe(),
            other.name,
            route.name,
            route.server,
            route.port
        );
    }

    let log = registry::log_path(&key)?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let output = std::fs::File::create(&log).with_context(|| format!("cannot write {}", log.display()))?;
    let exe = std::env::current_exe().context("cannot locate the turnout binary")?;
    let mut command = Command::new(exe);
    command
        .args([
            "tunnel",
            "run",
            "--name",
            &route.name,
            "--server",
            &route.server,
            "--credential",
            &route.credential,
        ])
        .args([
            "--local-port",
            &route.local_port.to_string(),
            "--host",
            &route.host,
            "--port",
            &route.port.to_string(),
        ])
        .arg("--log")
        .arg(&log);
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone().context("cannot share the tunnel's log")?)
        .stderr(output);
    process::detach(&mut command);
    crate::utils::stop_inheriting_stdio();
    let step = crate::progress::Step::start(format!("Opening tunnel {} - {} ...", route.name, route.describe()));
    let mut child = command.spawn().context("cannot start the tunnel")?;

    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("cannot check on the tunnel")? {
            step.clear();
            return Err(why_not(&log, &status.to_string()));
        }
        if let Some(entry) = registry::load(&key)?.filter(|entry| entry.pid == child.id()) {
            step.done(format!("Tunnel {} open: {}", route.name, route.describe()));
            println!("  pid {} · log: turnout logs tunnel {}", entry.pid, route.name);
            println!("  close: turnout tunnel stop {}", route.name);
            crate::journal::record("tunnel.open", None, Some(&route.server), Some(&format!("{} {}", route.name, route.describe())));
            return Ok(());
        }
        if started.elapsed() > OPEN_TIMEOUT {
            let _ = process::end(child.id(), None, process::Ending::Kill);
            step.clear();
            return Err(why_not(
                &log,
                &format!("it did not open within {}s - its log may say why", OPEN_TIMEOUT.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The error of a tunnel that did not open: its own error line when its log
/// has one, with whatever else it said put on screen first.
fn why_not(log: &std::path::Path, how: &str) -> anyhow::Error {
    let lines = crate::commands::jobs::tail(log, 20).unwrap_or_default();
    let error = lines.iter().rev().find_map(|line| line.strip_prefix("error: ")).map(str::to_string);
    for line in lines.iter().filter(|line| !line.starts_with("error: ")) {
        eprintln!("{line}");
    }
    match error {
        Some(error) => anyhow!("{error}"),
        None => anyhow!("the tunnel did not open ({how}) - see `{}`", log.display()),
    }
}

/// A tunnel as `tunnel list` and `status` report it.
#[derive(Serialize)]
pub struct TunnelView {
    pub name: String,
    pub server: String,
    pub credential: String,
    /// Where clients connect: `localhost:5432`.
    pub local: String,
    pub local_port: u16,
    /// What the server connects to; `localhost` is the server itself.
    pub host: String,
    pub port: u16,
    /// `open`, `down` (running, but its SSH connection is lost - see
    /// `trouble`), `failed` (it ended by itself) or `gone` (its process
    /// vanished without a word).
    pub state: &'static str,
    /// Why it is down; `null` while it carries.
    pub trouble: Option<String>,
    /// Since when it is down.
    pub down_since: Option<String>,
    pub pid: Option<u32>,
    pub started: String,
    pub log: Option<PathBuf>,
    /// `localhost:5432 -> staging:5432`, as the text says it.
    #[serde(skip)]
    route: Option<Route>,
}

impl TunnelView {
    pub(crate) fn new(entry: &Entry, route: &Route) -> Self {
        let status = entry.status();
        let running = status == Status::Running;
        let trouble = entry.trouble.as_ref().filter(|_| running);
        Self {
            name: route.name.clone(),
            server: route.server.clone(),
            credential: route.credential.clone(),
            local: route.local(),
            local_port: route.local_port,
            host: route.host.clone(),
            port: route.port,
            state: match status {
                Status::Running if trouble.is_some() => "down",
                Status::Running => "open",
                Status::Exited(_) => "failed",
                Status::Gone => "gone",
            },
            trouble: trouble.map(|trouble| trouble.reason.clone()),
            down_since: trouble.map(|trouble| crate::output::iso8601(trouble.since)),
            pid: running.then_some(entry.pid),
            started: crate::output::iso8601(entry.started),
            log: entry.log.clone(),
            route: Some(route.clone()),
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, "open" | "down")
    }

    /// `localhost:5432 -> staging:5432`.
    pub fn describe(&self) -> String {
        self.route.as_ref().map(Route::describe).unwrap_or_default()
    }

    /// `staging:5432`, or `db.internal:5432 via staging`.
    fn remote(&self) -> String {
        self.route.as_ref().map(Route::remote).unwrap_or_default()
    }
}

/// Every tunnel with a record, open ones first, then by name.
pub fn views() -> Result<Vec<TunnelView>> {
    let mut views: Vec<TunnelView> = crate::commands::jobs::tunnels()?
        .iter()
        .filter_map(|entry| entry.tunnel().map(|route| TunnelView::new(entry, route)))
        .collect();
    views.sort_by(|a, b| (!a.is_open(), &a.name).cmp(&(!b.is_open(), &b.name)));
    Ok(views)
}

/// `turnout tunnel list`.
fn list(format: Format) -> Result<()> {
    crate::output::emit(format, &views()?, |tunnels| table(tunnels))
}

fn table(tunnels: &[TunnelView]) -> String {
    if tunnels.is_empty() {
        return "No tunnels. Open one with `turnout tunnel NAME@SERVER PORT`.".to_string();
    }
    let now = registry::now();
    let mut rows = vec![["NAME", "STATE", "LOCAL", "REMOTE", "TIME"].map(str::to_string)];
    let mut notes: Vec<Option<String>> = vec![None];
    for tunnel in tunnels {
        let time = crate::output::parse_iso8601(&tunnel.started)
            .filter(|_| tunnel.is_open())
            .map_or_else(|| "-".to_string(), |at| human_span(now.saturating_sub(at)));
        rows.push([tunnel.name.clone(), tunnel.state.to_string(), tunnel.local.clone(), tunnel.remote(), time]);
        notes.push(match (&tunnel.trouble, tunnel.state) {
            (Some(trouble), _) => {
                let since = tunnel
                    .down_since
                    .as_deref()
                    .and_then(crate::output::parse_iso8601)
                    .map(|at| format!(" (for {})", human_span(now.saturating_sub(at))))
                    .unwrap_or_default();
                Some(format!("  ! {trouble}{since} - turnout keeps trying"))
            }
            (None, "failed" | "gone") => Some(format!(
                "  ! no longer runs - `turnout logs tunnel {}` says why, `turnout tunnel stop {0}` clears it",
                tunnel.name
            )),
            _ => None,
        });
    }
    let widths: Vec<usize> = (0..5)
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();
    let mut lines = Vec::new();
    for (row, note) in rows.iter().zip(notes) {
        let mut line = String::new();
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(&format!("{cell:<width$}  ", width = widths[column]));
            }
        }
        lines.push(line.trim_end().to_string());
        lines.extend(note);
    }
    lines.join("\n")
}

/// `turnout tunnel stop`: the tunnels named, every one with `--all`, or the
/// one picked on a terminal.
fn stop(names: Vec<String>, all: bool) -> Result<()> {
    let entries = crate::commands::jobs::tunnels()?;
    if all {
        return crate::commands::jobs::stop_entries(crate::commands::jobs::TUNNELS, entries);
    }
    let names = if names.is_empty() { vec![pick(&entries)?] } else { names };
    let mut chosen = Vec::new();
    for name in &names {
        let entry = entries
            .iter()
            .find(|entry| entry.tunnel().is_some_and(|route| &route.name == name))
            .ok_or_else(|| anyhow!("no tunnel named '{name}' - see `turnout tunnel list`"))?;
        chosen.push(entry.clone());
    }
    for entry in chosen {
        let title = entry.title();
        let stoppable = matches!(entry.status(), Status::Running | Status::Gone);
        if stoppable {
            crate::commands::jobs::stop_entries(&title, vec![entry])?;
        } else {
            // Ended by itself: its record is all that is left, and closing it
            // means taking that away too.
            registry::remove_if(&entry.key(), entry.pid, entry.birth)?;
            let how = entry.ended.as_ref().map_or(Exit::Killed, registry::Ended::exit);
            println!("{title} had ended by itself ({}) - cleared its record.", how.describe());
        }
    }
    Ok(())
}

/// The tunnel to close when none was named.
///
/// A terminal first, even for the only tunnel there is: in a script an empty
/// `$NAME` is a missing name, not a choice - the lesson `credential remove`
/// taught (see [`crate::pick::credential`]).
fn pick(entries: &[Entry]) -> Result<String> {
    let routes: Vec<&Route> = entries.iter().filter_map(Entry::tunnel).collect();
    if routes.is_empty() {
        bail!("no tunnel is open - open one with `turnout tunnel NAME@SERVER PORT`");
    }
    crate::pick::ensure_interactive("name the tunnel to close, or pass --all")?;
    match routes.as_slice() {
        [only] => Ok(only.name.clone()),
        _ => {
            let width = routes.iter().map(|route| route.name.len()).max().unwrap_or(0);
            let labels: Vec<String> = routes.iter().map(|route| format!("{:width$}  {}", route.name, route.describe())).collect();
            let index = dialoguer::Select::new()
                .with_prompt("Close which tunnel")
                .items(&labels)
                .default(0)
                .interact()?;
            Ok(routes[index].name.clone())
        }
    }
}
