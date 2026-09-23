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
    let targets = store::load_targets()?;
    match targets.len() {
        0 => println!("Targets: none yet"),
        n => println!("Targets: {} ({})", n, targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")),
    }
    let state = store::load_state()?;
    if !state.bindings.is_empty() {
        println!("Bindings:");
        for (app, server) in &state.bindings {
            println!("  {app} -> {server}");
        }
    }
    let gateway = crate::registry::load(crate::registry::GATEWAY)?;
    let running = gateway.clone().filter(crate::registry::Entry::is_running).and_then(crate::registry::as_gateway);
    match (&gateway, &running) {
        (_, Some(gateway)) if crate::commands::gateway::probe(gateway) => {
            println!("Gateway: running (pid {})", gateway.pid);
            match gateway.front_port {
                Some(front) => println!("Front:   {} - {}", crate::front::door(front), crate::front::address("NAME", front)),
                None => println!("Front:   closed (ports {} and {} were taken)", crate::front::PORT, crate::front::FALLBACK_PORT),
            }
            // The spare address per app: the way in when the front door is
            // shut, and the URL the app's own variable carries.
            let spares: Vec<String> = gateway
                .ports
                .iter()
                .map(|(port, name)| match apps.iter().find(|app| app.name == *name) {
                    Some(app) => format!("{name}:{port} -> {}", app.gateway_env_name()),
                    None => format!("{name}:{port}"),
                })
                .collect();
            if !spares.is_empty() {
                println!("Spare:   {}", spares.join(", "));
            }
        }
        (_, Some(gateway)) => println!("Gateway: running (pid {}) but not answering - try `turnout gateway stop`", gateway.pid),
        (Some(entry), None) if entry.ended.is_none() => println!(
            "Gateway: recorded (pid {}) but no longer running - `turnout gateway stop` clears the record",
            entry.pid
        ),
        _ => println!("Gateway: not running"),
    }
    // The gateway has its line above; everything else turnout runs is a job.
    let jobs: Vec<String> = crate::registry::list()?
        .into_iter()
        .filter(|entry| entry.app().is_some() && entry.is_running())
        .map(|entry| entry.title())
        .collect();
    if !jobs.is_empty() {
        println!("Jobs:    {} running ({}) - see `turnout ps`", jobs.len(), jobs.join(", "));
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
