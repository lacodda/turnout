use anyhow::Result;
use dialoguer::Confirm;

use crate::{paths, store};

pub fn run(assume_yes: bool) -> Result<()> {
    let dir = paths::data_dir()?;
    if store::is_initialized()? {
        println!("turnout is already set up.");
        println!("Data directory: {}", dir.display());
        println!("Run `turnout status` for an overview.");
        return Ok(());
    }
    println!("Welcome to turnout!");
    println!("Apps, servers and settings will live in:");
    println!("  {}", dir.display());
    let proceed = assume_yes || Confirm::new().with_prompt("Create the data directory?").default(true).interact()?;
    if !proceed {
        println!("Setup cancelled. Nothing was created.");
        return Ok(());
    }
    store::initialize()?;
    println!("Done. turnout is ready to store your settings.");
    println!("Run `turnout status` any time for an overview.");
    Ok(())
}
