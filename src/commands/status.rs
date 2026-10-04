//! `turnout status`: one window onto the whole switchyard.
//!
//! Where every app points, whether the gateway is up and what it has noticed
//! about the stands, which jobs are running, what is in the catalogs and what
//! happened last. Three renderings of one [`StatusView`]: sections for a
//! person, `--short` for a prompt, `--json` for a program.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use crate::cli::Format;
use crate::commands::app::AppView;
use crate::commands::group::GroupView;
use crate::commands::jobs::JobView;
use crate::output::{self, iso8601};
use crate::progress::human_span;
use crate::signal::{self, Kind, Signal};
use crate::{paths, registry, store, style};

#[derive(Serialize)]
pub struct StatusView {
    pub version: &'static str,
    pub data_dir: PathBuf,
    /// Whether `turnout setup` has run here; everything below is empty when not.
    pub set_up: bool,
    pub apps: Vec<AppView>,
    pub groups: Vec<GroupView>,
    pub gateway: GatewayView,
    /// The apps' jobs that are running; the gateway has its own block.
    pub jobs: Vec<JobView>,
    pub catalog: CatalogView,
    /// The latest entries of the journal, newest last.
    pub recent: Vec<RecentView>,
}

#[derive(Serialize)]
pub struct GatewayView {
    /// `running`; `not_answering` - its process is alive, its ports are not;
    /// `gone` - recorded, but the process is no more; or `stopped`.
    pub state: &'static str,
    pub pid: Option<u32>,
    pub started: Option<String>,
    /// `http://localhost` (or with its port) while the door is open.
    pub front_door: Option<String>,
    /// What it has noticed about the stands and is still seeing.
    pub signals: Vec<SignalView>,
}

#[derive(Serialize)]
pub struct SignalView {
    /// `stand_down` or `session_expired`.
    pub kind: Kind,
    pub app: String,
    pub server: String,
    /// The status the stand answered with; `null` when it did not answer.
    pub status: Option<u16>,
    pub since: String,
    /// The same words the gateway's log uses.
    pub message: String,
}

impl SignalView {
    fn new(signal: &Signal) -> Self {
        Self {
            kind: signal.kind,
            app: signal.app.clone(),
            server: signal.server.clone(),
            status: signal.status,
            since: iso8601(signal.since),
            message: signal::describe(signal),
        }
    }
}

/// The catalogs by name - the entities that are there rather than in use.
#[derive(Serialize, Default)]
pub struct CatalogView {
    pub servers: Vec<String>,
    pub credentials: Vec<String>,
    pub paths: Vec<String>,
    pub targets: Vec<String>,
}

/// One journal entry; every key present, `null` where the entry has nothing.
#[derive(Serialize)]
pub struct RecentView {
    pub at: String,
    pub action: String,
    pub app: Option<String>,
    pub server: Option<String>,
    pub detail: Option<String>,
}

/// How many journal entries `status` shows.
const RECENT: usize = 5;

pub fn run(format: Format, short: bool) -> Result<()> {
    let view = view()?;
    if short {
        let here = here_app()?;
        let line = short_line(&view, here.as_deref());
        // Nothing to say is said with nothing: a prompt segment that prints
        // an empty line still takes room.
        if !line.is_empty() {
            println!("{line}");
        }
        return Ok(());
    }
    output::emit(format, &view, text)
}

/// Everything `status` knows, read once.
pub fn view() -> Result<StatusView> {
    let mut view = StatusView {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: paths::data_dir()?,
        set_up: store::is_initialized()?,
        apps: Vec::new(),
        groups: Vec::new(),
        gateway: GatewayView {
            state: "stopped",
            pid: None,
            started: None,
            front_door: None,
            signals: Vec::new(),
        },
        jobs: Vec::new(),
        catalog: CatalogView::default(),
        recent: Vec::new(),
    };
    if !view.set_up {
        return Ok(view);
    }
    view.apps = crate::commands::app::views()?;
    view.groups = crate::commands::group::views()?;
    view.gateway = gateway()?;
    view.jobs = crate::commands::jobs::views()?
        .into_iter()
        .filter(|job| job.app.is_some() && job.is_running())
        .collect();
    view.catalog = CatalogView {
        servers: store::load_servers()?.into_iter().map(|s| s.name).collect(),
        credentials: store::load_credentials()?.into_iter().map(|c| c.name).collect(),
        paths: store::load_paths()?.into_iter().map(|p| p.name).collect(),
        targets: store::load_targets()?.into_iter().map(|t| t.name).collect(),
    };
    view.recent = crate::journal::tail(RECENT)
        .into_iter()
        .map(|entry| RecentView {
            at: entry.at,
            action: entry.action,
            app: entry.app,
            server: entry.server,
            detail: entry.detail,
        })
        .collect();
    Ok(view)
}

fn gateway() -> Result<GatewayView> {
    let entry = registry::load(registry::GATEWAY)?;
    let alive = entry.clone().filter(registry::Entry::is_running).and_then(registry::as_gateway);
    Ok(match (entry, alive) {
        (_, Some(gateway)) => {
            let answering = crate::commands::gateway::probe(&gateway);
            GatewayView {
                state: if answering { "running" } else { "not_answering" },
                pid: Some(gateway.pid),
                started: Some(iso8601(gateway.started)),
                front_door: gateway.front_port.filter(|_| answering).map(crate::front::door),
                signals: if answering {
                    gateway.signals.iter().map(SignalView::new).collect()
                } else {
                    Vec::new()
                },
            }
        }
        (Some(entry), None) if entry.ended.is_none() => GatewayView {
            state: "gone",
            pid: Some(entry.pid),
            started: Some(iso8601(entry.started)),
            front_door: None,
            signals: Vec::new(),
        },
        _ => GatewayView {
            state: "stopped",
            pid: None,
            started: None,
            front_door: None,
            signals: Vec::new(),
        },
    })
}

/// The app whose directory this is, if any.
fn here_app() -> Result<Option<String>> {
    if !store::is_initialized()? {
        return Ok(None);
    }
    let apps = store::load_apps()?;
    Ok(crate::commands::exec::app_here(&apps)?.map(|app| app.name.clone()))
}

// --- the sections ----------------------------------------------------------------

fn text(view: &StatusView) -> String {
    let now = registry::now();
    let data_dir = view.data_dir.display().to_string();
    let mut out = vec![format!(
        "{} {}  {}",
        style::heading("turnout"),
        view.version,
        style::dim(&style::link(&data_dir, &crate::notify::file_url(&view.data_dir)))
    )];
    if !view.set_up {
        out.push("Not set up yet - run `turnout setup` first.".to_string());
        return out.join("\n");
    }
    out.push(String::new());
    out.extend(apps_section(view));
    if !view.groups.is_empty() {
        out.push(String::new());
        out.push(style::heading("Groups"));
        let width = view.groups.iter().map(|g| g.name.len()).max().unwrap_or(0);
        for group in &view.groups {
            let members: Vec<String> = group
                .apps
                .iter()
                .map(|member| match &member.server {
                    Some(server) => format!("{} -> {server}", member.app),
                    None => format!("{} (unbound)", member.app),
                })
                .collect();
            out.push(format!(
                "  {}  {}",
                style::pad(&style::name(&group.name), &group.name, width),
                members.join(", ")
            ));
        }
    }
    out.push(String::new());
    out.extend(gateway_section(&view.gateway, now));
    if !view.jobs.is_empty() {
        out.push(String::new());
        out.extend(jobs_section(&view.jobs, now));
    }
    out.push(String::new());
    out.extend(catalog_section(&view.catalog));
    if !view.recent.is_empty() {
        out.push(String::new());
        out.extend(recent_section(&view.recent, now));
    }
    out.join("\n")
}

fn apps_section(view: &StatusView) -> Vec<String> {
    let mut lines = vec![style::heading("Apps")];
    if view.apps.is_empty() {
        lines.push(format!("  {}", style::dim("none yet - add one with `turnout app add`")));
        return lines;
    }
    let name_width = view.apps.iter().map(|a| a.name.len()).max().unwrap_or(0);
    let server_width = view.apps.iter().map(|a| a.server.as_deref().unwrap_or("unbound").len()).max().unwrap_or(0) + 3;
    // The door's address is the one to use while the gateway runs; without
    // it, the app's own port is the only way in, so that is what is shown.
    let door_open = view.gateway.front_door.is_some();
    let address_width = view
        .apps
        .iter()
        .map(|a| {
            if door_open {
                a.address.len()
            } else {
                a.gateway_url.as_deref().map_or(0, str::len)
            }
        })
        .max()
        .unwrap_or(0);
    for app in &view.apps {
        let (server, server_plain) = match &app.server {
            Some(server) => (format!("-> {}", style::good(server)), format!("-> {server}")),
            None => (style::dim("   unbound"), "   unbound".to_string()),
        };
        let (address, address_plain) = if door_open {
            (style::url(&app.address), app.address.clone())
        } else {
            match &app.gateway_url {
                Some(url) => (style::url(url), url.clone()),
                None => (String::new(), String::new()),
            }
        };
        let mut line = format!(
            "  {}  {}  {}",
            style::pad(&style::name(&app.name), &app.name, name_width),
            style::pad(&server, &server_plain, server_width),
            style::pad(&address, &address_plain, address_width)
        );
        if door_open && let Some(port) = app.gateway_port {
            line.push_str(&format!("  {}", style::dim(&format!("spare :{port} ({})", app.env_var))));
        }
        lines.push(line.trim_end().to_string());
    }
    lines
}

fn gateway_section(gateway: &GatewayView, now: u64) -> Vec<String> {
    let since = |at: &Option<String>| at.as_deref().and_then(output::parse_iso8601).map(|at| human_span(now.saturating_sub(at)));
    let pid = gateway.pid.map(|pid| format!("pid {pid}")).unwrap_or_default();
    let head = match gateway.state {
        "running" => format!(
            "{}  {} {}",
            style::heading("Gateway"),
            style::good("running"),
            style::dim(&format!(
                "· {pid}{}",
                since(&gateway.started).map(|span| format!(" · up {span}")).unwrap_or_default()
            ))
        ),
        "not_answering" => format!(
            "{}  {} {} - try `turnout gateway stop`",
            style::heading("Gateway"),
            style::bad("not answering"),
            style::dim(&format!("· {pid}"))
        ),
        "gone" => format!(
            "{}  {} {} - `turnout gateway stop` clears the record",
            style::heading("Gateway"),
            style::warn("no longer running"),
            style::dim(&format!("· was {pid}"))
        ),
        _ => format!(
            "{}  {} - start it with `turnout gateway start`",
            style::heading("Gateway"),
            style::dim("not running")
        ),
    };
    let mut lines = vec![head];
    if gateway.state == "running" {
        lines.push(match &gateway.front_door {
            Some(door) => format!("  front door  {}", style::url(door)),
            None => format!(
                "  front door  {}",
                style::warn(&format!(
                    "closed - ports {} and {} were taken; apps answer on their own ports",
                    crate::front::PORT,
                    crate::front::FALLBACK_PORT
                ))
            ),
        });
    }
    for signal in &gateway.signals {
        let mark = match signal.kind {
            Kind::StandDown => style::bad("!"),
            Kind::SessionExpired => style::warn("!"),
        };
        let age = since(&Some(signal.since.clone()))
            .map(|span| style::dim(&format!(" ({span})")))
            .unwrap_or_default();
        lines.push(format!("  {mark} {}{age}", signal.message));
    }
    lines
}

fn jobs_section(jobs: &[JobView], now: u64) -> Vec<String> {
    let mut lines = vec![format!(
        "{}  {} {}",
        style::heading("Jobs"),
        jobs.len(),
        style::dim("running - `turnout ps` shows the rest")
    )];
    let width = jobs.iter().map(|job| job.job.len()).max().unwrap_or(0);
    let state_width = jobs.iter().map(|job| job.state.len()).max().unwrap_or(0);
    for job in jobs {
        let span = output::parse_iso8601(&job.started)
            .map(|at| human_span(now.saturating_sub(at)))
            .unwrap_or_default();
        let address = job.address.as_deref().map(style::url).unwrap_or_default();
        let line = format!(
            "  {}  {}  {}  {address}",
            style::pad(&style::name(&job.job), &job.job, width),
            style::pad(&style::good(job.state), job.state, state_width),
            style::dim(&format!("{span:>7}"))
        );
        lines.push(line.trim_end().to_string());
    }
    lines
}

fn catalog_section(catalog: &CatalogView) -> Vec<String> {
    let mut lines = vec![style::heading("Catalog")];
    let rows = [
        ("servers", &catalog.servers),
        ("credentials", &catalog.credentials),
        ("paths", &catalog.paths),
        ("targets", &catalog.targets),
    ];
    for (label, names) in rows {
        let names = if names.is_empty() {
            style::dim("none yet")
        } else {
            names.iter().map(|n| style::name(n)).collect::<Vec<_>>().join(", ")
        };
        lines.push(format!("  {label:<11}  {names}"));
    }
    lines
}

fn recent_section(recent: &[RecentView], now: u64) -> Vec<String> {
    let mut lines = vec![style::heading("Recent")];
    let ages: Vec<String> = recent
        .iter()
        .map(|entry| match output::parse_iso8601(&entry.at) {
            Some(at) => format!("{} ago", human_span(now.saturating_sub(at))),
            None => entry.at.clone(),
        })
        .collect();
    let age_width = ages.iter().map(String::len).max().unwrap_or(0);
    let action_width = recent.iter().map(|entry| entry.action.len()).max().unwrap_or(0);
    // Newest first: the last thing done is the one being remembered.
    for (entry, age) in recent.iter().zip(&ages).rev() {
        let what = match (&entry.app, &entry.server) {
            (Some(app), Some(server)) => format!("{app} -> {server}"),
            (Some(app), None) => app.clone(),
            (None, Some(server)) => server.clone(),
            (None, None) => String::new(),
        };
        let space = if what.is_empty() { "" } else { " " };
        let detail = entry.detail.as_deref().map(|d| style::dim(&format!("{space}({d})"))).unwrap_or_default();
        let line = format!(
            "  {}  {:action_width$}  {what}{detail}",
            style::dim(&format!("{age:>age_width$}")),
            entry.action
        );
        lines.push(line.trim_end().to_string());
    }
    lines
}

// --- the one line ----------------------------------------------------------------

/// The `--short` line: where things point, whether the gateway carries them,
/// what runs. `prod · gw ok · 2 dev`.
///
/// In an app's directory the stand is that app's; elsewhere it is every stand
/// an app is bound to. A stand the gateway has a signal about says so in its
/// own segment - `prod 502`, `prod down`, `prod login` - because that is what
/// the person at the prompt needs to know about it.
pub fn short_line(view: &StatusView, here: Option<&str>) -> String {
    if !view.set_up {
        return String::new();
    }
    let here = here.and_then(|name| view.apps.iter().find(|app| app.name == name));
    let mut parts = Vec::new();

    let stands: BTreeSet<&str> = match here {
        Some(app) => app.server.as_deref().into_iter().collect(),
        None => view.apps.iter().filter_map(|app| app.server.as_deref()).collect(),
    };
    let marks: BTreeMap<&str, &str> = stands
        .iter()
        .filter_map(|stand| {
            let signals = view.gateway.signals.iter().filter(|s| s.server == *stand);
            let mut mark = None;
            for signal in signals {
                match signal.kind {
                    Kind::StandDown => return Some((*stand, signal.status.map_or("down", status_word))),
                    Kind::SessionExpired if here.is_none_or(|app| app.name == signal.app) => mark = Some("login"),
                    Kind::SessionExpired => {}
                }
            }
            mark.map(|mark| (*stand, mark))
        })
        .collect();
    match here {
        Some(app) if app.server.is_none() => parts.push(format!("{} unbound", app.name)),
        _ if !stands.is_empty() => parts.push(
            stands
                .iter()
                .map(|stand| match marks.get(stand) {
                    Some(mark) => format!("{stand} {mark}"),
                    None => stand.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => {}
    }

    match view.gateway.state {
        "running" => parts.push("gw ok".to_string()),
        "not_answering" => parts.push("gw down".to_string()),
        // A gateway nobody needs is not news; one that bound apps wait for is.
        _ if !stands.is_empty() => parts.push("gw off".to_string()),
        _ => {}
    }

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for job in &view.jobs {
        if let Some(command) = job.command.as_deref() {
            *counts.entry(command).or_default() += 1;
        }
    }
    parts.extend(counts.iter().map(|(command, count)| format!("{count} {command}")));
    parts.join(" · ")
}

/// A status code as the short line says it.
fn status_word(status: u16) -> &'static str {
    match status {
        502 => "502",
        503 => "503",
        504 => "504",
        _ => "down",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, server: Option<&str>) -> AppView {
        AppView {
            name: name.into(),
            path: String::new(),
            path_exists: true,
            server: server.map(str::to_string),
            address: format!("http://{name}.localhost"),
            dev_port: None,
            gateway_port: None,
            gateway_url: None,
            env_var: "TURNOUT_GATEWAY_URL".into(),
            env_file: ".env.development.local".into(),
            dist: None,
            commands: BTreeMap::new(),
            servers: Vec::new(),
        }
    }

    fn job(app: &str, command: &str) -> JobView {
        JobView {
            job: format!("{app} {command}"),
            app: Some(app.into()),
            command: Some(command.into()),
            state: "running",
            exit_code: None,
            pid: Some(1),
            port: None,
            address: None,
            background: true,
            started: iso8601(0),
            ended: None,
            log: None,
        }
    }

    fn signal(kind: Kind, app: &str, server: &str, status: Option<u16>) -> SignalView {
        SignalView::new(&Signal {
            kind,
            app: app.into(),
            server: server.into(),
            status,
            since: 0,
        })
    }

    fn status(apps: Vec<AppView>, gateway: &'static str, signals: Vec<SignalView>, jobs: Vec<JobView>) -> StatusView {
        StatusView {
            version: "0.0.0",
            data_dir: PathBuf::new(),
            set_up: true,
            apps,
            groups: Vec::new(),
            gateway: GatewayView {
                state: gateway,
                pid: None,
                started: None,
                front_door: None,
                signals,
            },
            jobs,
            catalog: CatalogView::default(),
            recent: Vec::new(),
        }
    }

    /// The line the plan promised, word for word.
    #[test]
    fn the_short_line_says_where_things_point_and_what_runs() {
        let view = status(
            vec![app("web", Some("prod")), app("admin", Some("prod"))],
            "running",
            Vec::new(),
            vec![job("web", "dev"), job("admin", "dev")],
        );
        assert_eq!(short_line(&view, None), "prod · gw ok · 2 dev");
        assert_eq!(short_line(&view, Some("web")), "prod · gw ok · 2 dev");
    }

    /// In an app's directory the stand is that app's; elsewhere, all of them.
    #[test]
    fn the_short_line_narrows_to_the_app_here() {
        let view = status(
            vec![app("web", Some("prod")), app("api", Some("dev")), app("tool", None)],
            "stopped",
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(short_line(&view, None), "dev, prod · gw off");
        assert_eq!(short_line(&view, Some("api")), "dev · gw off");
        assert_eq!(short_line(&view, Some("tool")), "tool unbound");
    }

    /// A stand the gateway saw fail says how; a lapsed session says "login",
    /// but only for the app it belongs to when there is one here.
    #[test]
    fn the_short_line_carries_the_signals() {
        let down = status(
            vec![app("web", Some("prod")), app("api", Some("dev"))],
            "running",
            vec![signal(Kind::StandDown, "web", "prod", Some(502))],
            Vec::new(),
        );
        assert_eq!(short_line(&down, None), "dev, prod 502 · gw ok");
        assert_eq!(short_line(&down, Some("api")), "dev · gw ok", "another stand's trouble is not this app's");
        let silent = status(
            vec![app("web", Some("prod"))],
            "running",
            vec![signal(Kind::StandDown, "web", "prod", None)],
            Vec::new(),
        );
        assert_eq!(short_line(&silent, None), "prod down · gw ok");
        let lapsed = status(
            vec![app("web", Some("prod")), app("admin", Some("prod"))],
            "running",
            vec![signal(Kind::SessionExpired, "web", "prod", Some(401))],
            Vec::new(),
        );
        assert_eq!(short_line(&lapsed, Some("web")), "prod login · gw ok");
        assert_eq!(short_line(&lapsed, Some("admin")), "prod · gw ok");
    }

    /// Nothing set up, nothing bound, nothing running: nothing to print.
    #[test]
    fn an_idle_machine_has_an_empty_short_line() {
        let mut view = status(Vec::new(), "stopped", Vec::new(), Vec::new());
        assert_eq!(short_line(&view, None), "");
        view.set_up = false;
        assert_eq!(short_line(&view, None), "");
    }
}
