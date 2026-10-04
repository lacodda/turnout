use anyhow::{Result, anyhow, bail};

use crate::health::{self, Health};
use crate::model::{App, Group, Server, State};
use crate::{pick, store};

/// What `-` means in place of a name: back where it was.
const BACK: &str = "-";

/// `use NAME SERVER` where NAME is an app or a group - a group binds every member.
/// Either argument may be omitted on a terminal and is then picked from a list.
///
/// `use NAME -` takes NAME back to the server it used before; a bare `use -`
/// does that for whatever the last `use` switched - the way `cd -` does.
pub fn run(name: Option<String>, server_name: Option<String>, no_check: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let groups = store::load_groups()?;
    let servers = store::load_servers()?;
    let mut state = store::load_state()?;

    if name.as_deref() == Some(BACK) {
        if let Some(server) = server_name {
            bail!("`turnout use -` goes back to where the last switch came from - it takes no server, and got '{server}'");
        }
        let last = state
            .last
            .clone()
            .ok_or_else(|| anyhow!("nothing to go back to - `turnout use` has not switched anything yet"))?;
        return back(&last, &apps, &groups, &servers, &mut state, no_check);
    }

    let name = match name {
        Some(name) => name,
        None => pick::app_or_group(&apps, &groups, &state, "Switch")?,
    };
    if server_name.as_deref() == Some(BACK) {
        return back(&name, &apps, &groups, &servers, &mut state, no_check);
    }
    let members = members(&name, &apps, &groups)?;

    // Picked from a list, every stand on offer is probed first and the answer
    // shown beside it; the pick then needs no second probe.
    let (server_name, seen) = match server_name {
        Some(server) => (server, None),
        None => pick_server(&name, &members, &servers, no_check)?,
    };
    let server = find_server(&servers, &server_name)?;
    check_allowed(&members, server)?;

    for app in &members {
        state.bind(&app.name, &server.name);
    }
    state.last = Some(name.clone());
    store::save_state(&state)?;
    for app in &members {
        crate::journal::record("use", Some(&app.name), Some(&server.name), None);
    }
    match members.as_slice() {
        [app] => println!("'{}' now uses '{}'.", app.name, server.name),
        _ => {
            println!("Group '{name}' now uses '{}':", server.name);
            for app in &members {
                println!("  {} -> {}", app.name, server.name);
            }
        }
    }
    gateway_note(&members)?;
    if !no_check {
        let health = seen.unwrap_or_else(|| health::probe_one(server));
        println!("{}", health.sentence(server));
    }
    Ok(())
}

/// Take NAME's apps back to the servers they used before their last switch.
fn back(name: &str, apps: &[App], groups: &[Group], servers: &[Server], state: &mut State, no_check: bool) -> Result<()> {
    let members = members(name, apps, groups)?;
    // Every move is checked before any is made: a group goes back whole or
    // not at all.
    let mut moves: Vec<(&App, &Server)> = Vec::new();
    let mut stay: Vec<&str> = Vec::new();
    for app in &members {
        match state.previous.get(&app.name) {
            Some(before) => {
                let server = find_server(servers, before)?;
                check_allowed(&[*app], server)?;
                moves.push((app, server));
            }
            None => stay.push(&app.name),
        }
    }
    if moves.is_empty() {
        bail!("'{name}' has not used another server yet - there is nowhere to go back to");
    }
    let mut lines = Vec::new();
    for (app, server) in &moves {
        let now = state.bindings.get(&app.name).cloned();
        state.bind(&app.name, &server.name);
        lines.push(match now {
            Some(now) => format!("'{}' is back on '{}' (was '{now}').", app.name, server.name),
            None => format!("'{}' is back on '{}'.", app.name, server.name),
        });
    }
    state.last = Some(name.to_string());
    store::save_state(state)?;
    for (app, server) in &moves {
        crate::journal::record("use", Some(&app.name), Some(&server.name), Some("back"));
    }
    for line in lines {
        println!("{line}");
    }
    if !stay.is_empty() {
        println!("Staying where they are, with no server before: {}.", stay.join(", "));
    }
    let moved: Vec<&App> = moves.iter().map(|(app, _)| *app).collect();
    gateway_note(&moved)?;
    if !no_check {
        let mut targets: Vec<&Server> = moves.iter().map(|(_, server)| *server).collect();
        targets.sort_by(|a, b| a.name.cmp(&b.name));
        targets.dedup_by(|a, b| a.name == b.name);
        for (server, health) in targets.iter().zip(health::probe_all(&targets)) {
            println!("{}", health.sentence(server));
        }
    }
    Ok(())
}

/// The apps NAME stands for: the app itself, or every member of the group.
fn members<'a>(name: &str, apps: &'a [App], groups: &[Group]) -> Result<Vec<&'a App>> {
    if let Some(app) = apps.iter().find(|a| a.name == name) {
        return Ok(vec![app]);
    }
    let Some(group) = groups.iter().find(|g| g.name == name) else {
        bail!("no app or group named '{name}' - see `turnout app list` and `turnout group list`");
    };
    group
        .apps
        .iter()
        .map(|member| {
            apps.iter()
                .find(|a| &a.name == member)
                .ok_or_else(|| anyhow!("group member '{member}' is gone from the catalog"))
        })
        .collect()
}

fn find_server<'a>(servers: &'a [Server], name: &str) -> Result<&'a Server> {
    servers
        .iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow!("no server named '{name}' - see `turnout server list`"))
}

/// An empty allow-list means the app is not restricted to particular servers.
fn check_allowed(members: &[&App], server: &Server) -> Result<()> {
    for app in members {
        if !app.servers.is_empty() && !app.servers.contains(&server.name) {
            bail!(
                "server '{}' is not allowed for '{}' (allowed: {})",
                server.name,
                app.name,
                app.servers.join(", ")
            );
        }
    }
    Ok(())
}

/// Pick a server for `name`, with each stand's health beside it.
///
/// A single app narrows the list to what it is allowed to use; a group has no
/// allow-list of its own, so it offers every server and the per-member check
/// still applies after the pick.
fn pick_server(name: &str, members: &[&App], servers: &[Server], no_check: bool) -> Result<(String, Option<Health>)> {
    let offered: Vec<&Server> = match members {
        [app] if !app.servers.is_empty() => servers.iter().filter(|s| app.servers.contains(&s.name)).collect(),
        _ => servers.iter().collect(),
    };
    if offered.is_empty() {
        bail!(match members {
            [app] if !servers.is_empty() => format!("no servers allowed for '{0}' - add one with `turnout app edit {0} --add-server NAME`", app.name),
            _ => "no servers in the catalog yet - run `turnout server add` first".to_string(),
        });
    }
    pick::ensure_interactive("server name is required")?;
    let healths: Option<Vec<Health>> = (!no_check).then(|| {
        let step = crate::progress::Step::start(match offered.len() {
            1 => "Checking the stand ...".to_string(),
            n => format!("Checking {n} stands ..."),
        });
        let healths = health::probe_all(&offered);
        step.clear();
        healths
    });
    let labels: Vec<Option<String>> = match &healths {
        Some(healths) => healths.iter().map(|health| Some(health.label())).collect(),
        None => vec![None; offered.len()],
    };
    let index = pick::server_index(&offered, &labels, &format!("Point '{name}' at"))?;
    Ok((offered[index].name.clone(), healths.map(|mut healths| healths.swap_remove(index))))
}

/// What happens next on the gateway's side.
fn gateway_note(members: &[&App]) -> Result<()> {
    match crate::registry::gateway()? {
        Some(gateway) if crate::commands::gateway::probe(&gateway) => {
            println!("The running gateway picks this up automatically.");
        }
        _ => {
            if members.iter().any(|app| app.gateway_port.is_some()) {
                println!("Start the gateway with `turnout gateway start` to route the traffic.");
            } else {
                // Every app registered since v0.18.0 has a port; reaching here
                // means they were all cleared by hand with `--port 0`.
                println!("These apps have no gateway port to route through: `turnout app edit NAME --port PORT`.");
            }
        }
    }
    Ok(())
}
