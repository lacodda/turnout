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
    println!("Gateway: not running");
    Ok(())
}
