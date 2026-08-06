use anyhow::{Result, bail};

use crate::{store, utils};

pub fn run(app_name: &str, server_name: &str, no_check: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let app = apps
        .iter()
        .find(|a| a.name == app_name)
        .ok_or_else(|| anyhow::anyhow!("no app named '{app_name}' - see `turnout app list`"))?;
    let servers = store::load_servers()?;
    let server = servers
        .iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    // An empty allow-list means the app is not restricted to particular servers.
    if !app.servers.is_empty() && !app.servers.contains(&server.name) {
        bail!("server '{server_name}' is not allowed for '{app_name}' (allowed: {})", app.servers.join(", "));
    }

    let mut state = store::load_state()?;
    state.bindings.insert(app.name.clone(), server.name.clone());
    store::save_state(&state)?;
    println!("'{app_name}' now uses '{server_name}'.");

    match &state.gateway {
        Some(gateway) if crate::commands::gateway::probe(gateway) => {
            println!("The running gateway picks this up automatically.");
        }
        _ => match app.gateway_port {
            Some(port) => println!("Start the gateway with `turnout gateway start` - the app will talk to http://localhost:{port}."),
            None => println!("Set a gateway port first: `turnout app edit {app_name} --port PORT`."),
        },
    }

    if !no_check {
        match utils::check_reachable(server) {
            Ok(status) => println!("Stand check: {} responded with {status}.", server.url),
            Err(err) => println!("Stand check: warning - {err:#}."),
        }
    }
    Ok(())
}
