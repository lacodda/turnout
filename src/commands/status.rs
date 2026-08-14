use anyhow::Result;

use crate::{paths, store};

pub fn run() -> Result<()> {
    println!("turnout {}", env!("CARGO_PKG_VERSION"));
    println!("Data directory: {}", paths::data_dir()?.display());
    if !store::is_initialized()? {
        println!("Not set up yet - run `turnout setup` first.");
        return Ok(());
    }
    let apps = store::load_apps()?;
    let servers = store::load_servers()?;
    match apps.len() {
        0 => println!("Apps:    none yet"),
        n => println!("Apps:    {} ({})", n, apps.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")),
    }
    match servers.len() {
        0 => println!("Servers: none yet"),
        n => println!("Servers: {} ({})", n, servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")),
    }
    let groups = store::load_groups()?;
    if !groups.is_empty() {
        for group in &groups {
            println!("Group:   {} ({})", group.name, group.apps.join(", "));
        }
    }
    let credentials = store::load_credentials()?;
    match credentials.len() {
        0 => println!("Creds:   none yet"),
        n => println!(
            "Creds:   {} ({})",
            n,
            credentials.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
    let remote_paths = store::load_paths()?;
    match remote_paths.len() {
        0 => println!("Paths:   none yet"),
        n => println!(
            "Paths:   {} ({})",
            n,
            remote_paths.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
    let state = store::load_state()?;
    if !state.bindings.is_empty() {
        println!("Bindings:");
        for (app, server) in &state.bindings {
            println!("  {app} -> {server}");
        }
    }
    match &state.gateway {
        Some(gateway) if crate::commands::gateway::probe(gateway) => {
            let ports: Vec<String> = gateway.ports.iter().map(|(port, app)| format!("{app}:{port}")).collect();
            println!("Gateway: running (pid {}; {})", gateway.pid, ports.join(", "));
        }
        Some(gateway) => println!("Gateway: recorded (pid {}) but not responding - try `turnout gateway stop`", gateway.pid),
        None => println!("Gateway: not running"),
    }
    let recent = crate::journal::tail(5);
    if !recent.is_empty() {
        println!("Recent:");
        for entry in recent.iter().rev() {
            let what = match (&entry.app, &entry.server) {
                (Some(app), Some(server)) => format!("{app} -> {server}"),
                (Some(app), None) => app.clone(),
                (None, Some(server)) => server.clone(),
                (None, None) => String::new(),
            };
            let detail = entry.detail.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
            println!("  {}  {:14} {what}{detail}", entry.at, entry.action);
        }
    }
    Ok(())
}
