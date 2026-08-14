//! Paths: named remote directories, plus what to run after writing to them.
//!
//! Free-standing since v0.9.0 and deliberately not owned by a server. The same
//! web root usually exists on the staging box and the production one, and the
//! post-write command is a property of the directory's role, not the machine.

use anyhow::{Result, bail};
use dialoguer::Input;

use crate::cli::PathCommand;
use crate::model::{normalize_remote_path, validate_name, validate_remote_path};
use crate::{pick, store};

pub fn run(command: PathCommand) -> Result<()> {
    match command {
        PathCommand::Add { name, dir, restart } => add(name, dir, restart),
        PathCommand::List => list(),
        PathCommand::Show { name } => show(&resolve(name, "Show path")?),
        PathCommand::Edit { name, dir, restart } => {
            let name = resolve(name, "Edit path")?;
            edit(&name, dir, restart)
        }
        PathCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove path")?;
            remove(&name, assume_yes)
        }
    }
}

fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::path(&store::load_paths()?, prompt),
    }
}

fn add(name: Option<String>, dir: Option<String>, restart: Option<String>) -> Result<()> {
    let mut paths = store::load_paths()?;
    let wizard = name.is_none() || dir.is_none();
    if wizard {
        pick::ensure_interactive("path name and --dir are required")?;
    }

    let name = match name {
        Some(name) => name,
        None => Input::new()
            .with_prompt("Path name")
            .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_name(&name)?;
    if paths.iter().any(|p| p.name == name) {
        bail!("path '{name}' already exists");
    }

    let dir = match dir {
        Some(dir) => dir,
        None => Input::new()
            .with_prompt("Remote directory")
            .validate_with(|s: &String| validate_remote_path(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_remote_path(&dir)?;

    let restart = match restart {
        Some(restart) => Some(restart),
        None if wizard => {
            let answer: String = Input::new()
                .with_prompt("Command to run after writing here (empty for none)")
                .allow_empty(true)
                .interact_text()?;
            Some(answer)
        }
        None => None,
    };

    paths.push(crate::model::Path {
        name: name.clone(),
        dir: normalize_remote_path(&dir),
        restart: restart.filter(|r| !r.trim().is_empty()).map(|r| r.trim().to_string()),
    });
    paths.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_paths(&paths)?;
    crate::journal::record("path.add", None, None, Some(&name));
    println!("Path '{name}' added.");
    Ok(())
}

fn list() -> Result<()> {
    let paths = store::load_paths()?;
    if paths.is_empty() {
        println!("No paths yet - run `turnout path add`.");
        return Ok(());
    }
    let width = paths.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for path in paths {
        match &path.restart {
            Some(restart) => println!("{:width$}  {}  (then: {restart})", path.name, path.dir),
            None => println!("{:width$}  {}", path.name, path.dir),
        }
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let paths = store::load_paths()?;
    let path = paths.iter().find(|p| p.name == name).ok_or_else(|| unknown(name))?;
    println!("{}", path.name);
    println!("  Directory: {}", path.dir);
    match &path.restart {
        Some(restart) => println!("  After:     {restart}"),
        None => println!("  After:     nothing"),
    }
    let used_by = used_by(name)?;
    if !used_by.is_empty() {
        println!("  Used by:   {}", used_by.join(", "));
    }
    Ok(())
}

/// Which `server/app` pairs deploy into this path.
fn used_by(name: &str) -> Result<Vec<String>> {
    let servers = store::load_servers()?;
    Ok(servers
        .iter()
        .flat_map(|server| {
            server
                .deploy
                .iter()
                .filter(|(_, path)| path.as_str() == name)
                .map(|(app, _)| format!("{}/{app}", server.name))
        })
        .collect())
}

fn edit(name: &str, dir: Option<String>, restart: Option<String>) -> Result<()> {
    let mut paths = store::load_paths()?;
    let index = paths.iter().position(|p| p.name == name).ok_or_else(|| unknown(name))?;

    if dir.is_none() && restart.is_none() {
        pick::ensure_interactive("nothing to change: pass flags to edit non-interactively")?;
        let path = &mut paths[index];
        let dir: String = Input::new()
            .with_prompt("Remote directory")
            .default(path.dir.clone())
            .validate_with(|s: &String| validate_remote_path(s).map_err(|e| e.to_string()))
            .interact_text()?;
        path.dir = normalize_remote_path(&dir);
        let restart: String = Input::new()
            .with_prompt("Command to run after writing here (empty for none)")
            .default(path.restart.clone().unwrap_or_default())
            .allow_empty(true)
            .interact_text()?;
        path.restart = if restart.trim().is_empty() { None } else { Some(restart.trim().to_string()) };
    } else {
        let path = &mut paths[index];
        if let Some(dir) = dir {
            validate_remote_path(&dir)?;
            path.dir = normalize_remote_path(&dir);
        }
        if let Some(restart) = restart {
            path.restart = if restart.trim().is_empty() { None } else { Some(restart.trim().to_string()) };
        }
    }
    store::save_paths(&paths)?;
    println!("Path '{name}' updated.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut paths = store::load_paths()?;
    if !paths.iter().any(|p| p.name == name) {
        return Err(unknown(name));
    }
    let used_by = used_by(name)?;
    if !used_by.is_empty() {
        println!("Used by: {}. They will be left without a deploy path.", used_by.join(", "));
    }
    let confirmed = pick::confirm_destructive(format!("Remove path '{name}' from the catalog?"), assume_yes)?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    paths.retain(|p| p.name != name);
    store::save_paths(&paths)?;

    let mut servers = store::load_servers()?;
    let mut touched = false;
    for server in servers.iter_mut() {
        let before = server.deploy.len();
        server.deploy.retain(|_, path| path.as_str() != name);
        touched |= server.deploy.len() != before;
    }
    if touched {
        store::save_servers(&servers)?;
    }
    crate::journal::record("path.remove", None, None, Some(name));
    // The directory on the server is the user's, and turnout has no business
    // deleting it just because it stopped tracking the name.
    println!("Path '{name}' removed from the catalog (the directory on the server is untouched).");
    Ok(())
}

fn unknown(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no path named '{name}' - see `turnout path list`")
}
