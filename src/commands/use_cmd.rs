use anyhow::{Result, bail};

use crate::model::App;
use crate::{pick, store, utils};

/// `use NAME SERVER` where NAME is an app or a group - a group binds every member.
/// Either argument may be omitted on a terminal and is then picked from a list.
pub fn run(name: Option<String>, server_name: Option<String>, no_check: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let groups = store::load_groups()?;
    let servers = store::load_servers()?;
    let state = store::load_state()?;

    let name = match name {
        Some(name) => name,
        None => pick::app_or_group(&apps, &groups, &state, "Switch")?,
    };
    let name = name.as_str();

    let server_name = match server_name {
        Some(server) => server,
        // A single app narrows the list to what it is allowed to use; a group
        // has no allow-list of its own, so it offers every server and the
        // per-member check below still applies.
        None => match apps.iter().find(|a| a.name == name) {
            Some(app) => pick::server_for_app(&servers, app, &format!("Point '{name}' at"))?,
            None => pick::server(&servers, &format!("Point '{name}' at"))?,
        },
    };
    let server_name = server_name.as_str();

    let server = servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;

    let members: Vec<&App> = if let Some(app) = apps.iter().find(|a| a.name == name) {
        vec![app]
    } else if let Some(group) = groups.iter().find(|g| g.name == name) {
        group
            .apps
            .iter()
            .map(|member| {
                apps.iter()
                    .find(|a| &a.name == member)
                    .ok_or_else(|| anyhow::anyhow!("group member '{member}' is gone from the catalog"))
            })
            .collect::<Result<_>>()?
    } else {
        bail!("no app or group named '{name}' - see `turnout app list` and `turnout group list`");
    };

    // An empty allow-list means the app is not restricted to particular servers.
    for app in &members {
        if !app.servers.is_empty() && !app.servers.contains(&server.name) {
            bail!("server '{server_name}' is not allowed for '{}' (allowed: {})", app.name, app.servers.join(", "));
        }
    }

    let mut state = state;
    for app in &members {
        state.bindings.insert(app.name.clone(), server.name.clone());
    }
    store::save_state(&state)?;
    match members.as_slice() {
        [app] => println!("'{}' now uses '{server_name}'.", app.name),
        _ => {
            println!("Group '{name}' now uses '{server_name}':");
            for app in &members {
                println!("  {} -> {server_name}", app.name);
            }
        }
    }

    match &state.gateway {
        Some(gateway) if crate::commands::gateway::probe(gateway) => {
            println!("The running gateway picks this up automatically.");
        }
        _ => {
            if members.iter().any(|app| app.gateway_port.is_some()) {
                println!("Start the gateway with `turnout gateway start` to route the traffic.");
            } else {
                println!("Set a gateway port first: `turnout app edit NAME --port PORT`.");
            }
        }
    }

    if !no_check {
        match utils::check_reachable(server) {
            Ok(status) => println!("Stand check: {} responded with {status}.", server.url),
            Err(err) => println!("Stand check: warning - {err:#}."),
        }
    }
    Ok(())
}
