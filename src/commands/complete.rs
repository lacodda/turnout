//! Feeds live entity names to the shell completion scripts.
//!
//! Hidden from `--help`: users never type this, their shell does. It stays
//! silent on any error, because a completion helper that prints diagnostics
//! would splatter them over the command line the user is typing.

use anyhow::Result;

use crate::cli::CompleteKind;
use crate::store;

pub fn run(what: CompleteKind) -> Result<()> {
    let names = match what {
        CompleteKind::Apps => apps(),
        CompleteKind::Servers => servers(),
        CompleteKind::Credentials => credentials(),
        CompleteKind::Paths => remote_paths(),
        CompleteKind::Groups => groups(),
        CompleteKind::Targets => {
            let mut names = apps();
            names.extend(groups());
            names.sort();
            names
        }
        CompleteKind::Commands => commands(),
    };
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn apps() -> Vec<String> {
    store::load_apps().map(|apps| apps.into_iter().map(|a| a.name).collect()).unwrap_or_default()
}

fn servers() -> Vec<String> {
    store::load_servers()
        .map(|servers| servers.into_iter().map(|s| s.name).collect())
        .unwrap_or_default()
}

fn credentials() -> Vec<String> {
    store::load_credentials()
        .map(|credentials| credentials.into_iter().map(|c| c.name).collect())
        .unwrap_or_default()
}

/// Named remote directories - not to be confused with local filesystem paths,
/// which the shell completes on its own.
fn remote_paths() -> Vec<String> {
    store::load_paths().map(|paths| paths.into_iter().map(|p| p.name).collect()).unwrap_or_default()
}

fn groups() -> Vec<String> {
    store::load_groups()
        .map(|groups| groups.into_iter().map(|g| g.name).collect())
        .unwrap_or_default()
}

/// Every command name defined by any app, deduplicated.
fn commands() -> Vec<String> {
    let mut names: Vec<String> = store::load_apps()
        .map(|apps| apps.into_iter().flat_map(|a| a.commands.into_keys()).collect())
        .unwrap_or_default();
    names.sort();
    names.dedup();
    names
}
