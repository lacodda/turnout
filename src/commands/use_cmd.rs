use anyhow::{Result, bail};

use crate::model::App;
use crate::{store, utils};

/// `use NAME SERVER` where NAME is an app or a group - a group binds every member.
pub fn run(name: &str, server_name: &str, no_check: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let groups = store::load_groups()?;
    let servers = store::load_servers()?;
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

    let mut state = store::load_state()?;
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
