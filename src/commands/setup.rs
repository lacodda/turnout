use anyhow::Result;
use dialoguer::Confirm;

use crate::{paths, store};

pub fn run(assume_yes: bool) -> Result<()> {
    let dir = paths::data_dir()?;
    // The one command that must work on a data directory this build refuses to
    // read: every refusal points at `setup` as the way forward, so it cannot be
    // the command that also stops at the refusal.
    if store::needs_reset()? {
        return reset(assume_yes);
    }
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

/// Start a fresh catalog next to settings this build cannot read.
///
/// The old catalogs are moved aside rather than deleted: they are the only
/// record of what to re-enter, and a user who changes their mind can put them
/// back and reinstall the previous release.
fn reset(assume_yes: bool) -> Result<()> {
    let dir = paths::data_dir()?;
    println!("The settings in {} were written by an older turnout", dir.display());
    println!("and cannot be read by this one. Starting over means:");
    println!();
    println!("  - the old catalogs are moved to a folder beside them, to read the values from;");
    println!("  - apps, servers, credentials and paths are re-entered by hand;");
    println!("  - secrets already in the OS keyring are untouched, but each one has to be");
    println!("    attached to the credential that now owns it (`turnout pass set NAME`).");
    println!();
    println!("The walkthrough is at https://lacodda.github.io/turnout/guides/upgrading-to-0-9/");
    println!();
    let proceed = assume_yes || Confirm::new().with_prompt("Start a fresh catalog?").default(false).interact()?;
    if !proceed {
        println!("Nothing was changed. Your old settings are exactly where they were.");
        return Ok(());
    }
    let moved = store::reset_to_current_schema()?;
    println!();
    println!("Done. The old catalogs are in:");
    println!("  {}", moved.display());
    println!("Re-enter them with `turnout server add`, `turnout credential add` and `turnout path add`.");
    Ok(())
}
