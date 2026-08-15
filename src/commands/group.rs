use anyhow::{Result, bail};
use dialoguer::{Input, MultiSelect};

use crate::cli::GroupCommand;
use crate::model::{Group, validate_name};
use crate::{pick, store};

pub fn run(command: GroupCommand) -> Result<()> {
    match command {
        GroupCommand::Add { name, apps } => add(name, apps),
        GroupCommand::List => list(),
        GroupCommand::Show { name } => show(&resolve(name, "Show group")?),
        GroupCommand::Edit { name, add_apps, rm_apps } => {
            let name = resolve(name, "Edit group")?;
            edit(&name, add_apps, rm_apps)
        }
        GroupCommand::Remove { name, assume_yes } => {
            let name = resolve(name, "Remove group")?;
            remove(&name, assume_yes)
        }
    }
}

/// Take the group name as given, or let the user pick one.
fn resolve(name: Option<String>, prompt: &str) -> Result<String> {
    match name {
        Some(name) => Ok(name),
        None => pick::group(&store::load_groups()?, prompt),
    }
}

fn add(name: Option<String>, apps: Vec<String>) -> Result<()> {
    let mut groups = store::load_groups()?;
    let known = store::load_apps()?;
    if known.is_empty() {
        bail!("no apps in the catalog yet - run `turnout app add` first");
    }
    let wizard = name.is_none();
    if wizard {
        pick::ensure_interactive("group name is required")?;
    }

    let name = match name {
        Some(name) => name,
        None => Input::new()
            .with_prompt("Group name")
            .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
            .interact_text()?,
    };
    validate_name(&name)?;
    if groups.iter().any(|g| g.name == name) {
        bail!("group '{name}' already exists");
    }
    if known.iter().any(|a| a.name == name) {
        bail!("an app named '{name}' already exists - group names must not clash with app names");
    }

    let apps = if apps.is_empty() && wizard {
        let items: Vec<&str> = known.iter().map(|a| a.name.as_str()).collect();
        let picked = MultiSelect::new()
            .with_prompt("Apps in the group (space to toggle, enter to accept)")
            .items(&items)
            .interact()?;
        picked.into_iter().map(|i| known[i].name.clone()).collect()
    } else {
        for app in &apps {
            if !known.iter().any(|a| &a.name == app) {
                bail!("no app named '{app}' - see `turnout app list`");
            }
        }
        apps
    };
    if apps.is_empty() {
        bail!("a group needs at least one app - pass --app NAME (repeatable)");
    }

    groups.push(Group { name: name.clone(), apps });
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    store::save_groups(&groups)?;
    crate::journal::record("group.add", None, None, Some(&name));
    println!("Group '{name}' added.");
    Ok(())
}

fn list() -> Result<()> {
    let groups = store::load_groups()?;
    if groups.is_empty() {
        println!("No groups yet - run `turnout group add`.");
        return Ok(());
    }
    let width = groups.iter().map(|g| g.name.len()).max().unwrap_or(0);
    for group in groups {
        println!("{:width$}  {}", group.name, group.apps.join(", "));
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let groups = store::load_groups()?;
    let group = groups.iter().find(|g| g.name == name).ok_or_else(|| unknown_group(name))?;
    let state = store::load_state()?;
    println!("{}", group.name);
    for app in &group.apps {
        match state.bindings.get(app) {
            Some(server) => println!("  {app} -> {server}"),
            None => println!("  {app} (not bound)"),
        }
    }
    Ok(())
}

fn edit(name: &str, add_apps: Vec<String>, rm_apps: Vec<String>) -> Result<()> {
    if add_apps.is_empty() && rm_apps.is_empty() {
        bail!("nothing to change: pass --add-app or --rm-app");
    }
    let mut groups = store::load_groups()?;
    let known = store::load_apps()?;
    let group = groups.iter_mut().find(|g| g.name == name).ok_or_else(|| unknown_group(name))?;
    for app in &add_apps {
        if !known.iter().any(|a| &a.name == app) {
            bail!("no app named '{app}' - see `turnout app list`");
        }
        if !group.apps.contains(app) {
            group.apps.push(app.clone());
        }
    }
    group.apps.retain(|a| !rm_apps.contains(a));
    if group.apps.is_empty() {
        bail!("removing these apps would leave '{name}' empty - remove the group instead");
    }
    store::save_groups(&groups)?;
    crate::journal::record("group.edit", None, None, Some(name));
    println!("Group '{name}' updated.");
    Ok(())
}

fn remove(name: &str, assume_yes: bool) -> Result<()> {
    let mut groups = store::load_groups()?;
    if !groups.iter().any(|g| g.name == name) {
        return Err(unknown_group(name));
    }
    let confirmed = pick::confirm_destructive(format!("Remove group '{name}'?"), assume_yes)?;
    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }
    groups.retain(|g| g.name != name);
    store::save_groups(&groups)?;
    crate::journal::record("group.remove", None, None, Some(name));
    println!("Group '{name}' removed. Its apps are untouched.");
    Ok(())
}

fn unknown_group(name: &str) -> anyhow::Error {
    anyhow::anyhow!("no group named '{name}' - see `turnout group list`")
}
