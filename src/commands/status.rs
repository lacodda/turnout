use anyhow::Result;

use crate::{paths, store};

pub fn run() -> Result<()> {
    println!("turnout {}", env!("CARGO_PKG_VERSION"));
    println!("Data directory: {}", paths::data_dir()?.display());
    if !store::is_initialized()? {
        println!("Not set up yet - run `turnout setup` first.");
        return Ok(());
    }
    println!("Apps:    none yet");
    println!("Servers: none yet");
    println!("Gateway: not running");
    Ok(())
}
