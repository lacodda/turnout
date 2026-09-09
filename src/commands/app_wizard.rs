//! The interactive form behind `app add` and `app edit`.
//!
//! The form is a *list of fields*, not a run of prompts. Each field says which
//! part of an [`App`] it fills and how to ask for it, so both commands walk the
//! same list: `add` starts from a blank app, `edit` from the stored one with
//! its current values as the defaults.
//!
//! Written this way for two reasons. Prompts spelled out inline had already
//! drifted - `add` asked for five of the eight fields and `edit` for four, and
//! `dist_dir`, `env_file` and `dev_port` were reachable only through flags.
//! And a run of `dialoguer` calls cannot be tested at all: every prompt needs a
//! terminal, while the CLI tests drive turnout without one. A list can be
//! checked as data, which is what [`FIELDS`] and the gate over it do.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use dialoguer::{Input, Select};

use crate::detect;
use crate::model::{App, Server};

/// One field of an app, as the wizard sees it.
struct Field {
    /// The field of [`App`] this fills, spelled as the struct spells it. Read
    /// only by the gate below, which compares these names against the model.
    #[cfg_attr(not(test), allow(dead_code))]
    name: &'static str,
    /// Asks for the field. The app carries everything answered so far:
    /// `gateway_env` needs the port, and its suggested name needs the path.
    ask: fn(&mut App, &Surroundings) -> Result<()>,
}

/// What the wizard knows besides the app itself.
pub struct Surroundings {
    /// Every other app in the catalog: ports are checked against these.
    pub others: Vec<App>,
    /// Servers that exist, for the "allowed servers" question.
    pub known: Vec<Server>,
    /// True while adding. A new app gets a suggested port and a look at what
    /// its directory can run; an edit leaves both alone.
    pub adding: bool,
}

/// Every field of [`App`] the wizard walks, in the order it asks.
///
/// `name` is not here: `add` takes it before the form opens and `edit`
/// addresses the app by it. The gate below knows that and expects it to be
/// absent.
const FIELDS: &[Field] = &[
    Field { name: "path", ask: ask_path },
    Field {
        name: "commands",
        ask: ask_commands,
    },
    Field {
        name: "gateway_port",
        ask: ask_gateway_port,
    },
    Field {
        name: "gateway_env",
        ask: ask_gateway_env,
    },
    Field {
        name: "env_file",
        ask: ask_env_file,
    },
    Field {
        name: "dev_port",
        ask: ask_dev_port,
    },
    Field {
        name: "dist_dir",
        ask: ask_dist_dir,
    },
    Field {
        name: "servers",
        ask: ask_servers,
    },
];

/// Walk the whole form over `app`, leaving it filled in.
pub fn walk(app: &mut App, around: &Surroundings) -> Result<()> {
    for field in FIELDS {
        (field.ask)(app, around)?;
    }
    Ok(())
}

/// The field names the form covers; the gate reads this.
#[cfg(test)]
fn covered_fields() -> Vec<&'static str> {
    FIELDS.iter().map(|f| f.name).collect()
}

fn ask_path(app: &mut App, around: &Surroundings) -> Result<()> {
    let default = if app.path.is_empty() {
        std::env::current_dir()?.display().to_string()
    } else {
        app.path.clone()
    };
    let answer: String = Input::new().with_prompt("Project directory").default(default).interact_text()?;
    let path = crate::utils::project_dir(Path::new(&answer))?;
    let moved = app.path != path.display().to_string();
    app.path = path.display().to_string();
    // A new app has no commands yet, and an app that just moved may be a
    // different project; both deserve a fresh look at what is in there.
    if around.adding || moved {
        app.commands = merge_detected(&app.commands, &path);
    }
    Ok(())
}

/// Detected commands, without losing what the user has already set by hand.
fn merge_detected(current: &BTreeMap<String, String>, path: &Path) -> BTreeMap<String, String> {
    let mut merged = detect::commands_for(path, detect::detect(path));
    for (name, line) in current {
        merged.insert(name.clone(), line.clone());
    }
    merged
}

fn ask_gateway_port(app: &mut App, around: &Surroundings) -> Result<()> {
    let default = match app.gateway_port {
        Some(port) => port.to_string(),
        // A fresh app is offered the first port nothing else holds.
        None if around.adding => free_gateway_port(&around.others).to_string(),
        None => String::new(),
    };
    let answer: String = Input::new()
        .with_prompt("Local gateway port (empty for none)")
        .default(default)
        .allow_empty(true)
        .interact_text()?;
    let answer = answer.trim();
    if answer.is_empty() {
        app.gateway_port = None;
        return Ok(());
    }
    let port: u16 = answer.parse().context("port must be a number")?;
    if let Some(other) = around.others.iter().find(|a| a.gateway_port == Some(port)) {
        bail!(
            "port {port} is already used by app '{0}' - pick another, or move '{0}' first with `turnout app edit {0} --port PORT`",
            other.name
        );
    }
    app.gateway_port = Some(port);
    Ok(())
}

/// The lowest port from 7100 up that no app holds.
fn free_gateway_port(others: &[App]) -> u16 {
    (7100..u16::MAX)
        .find(|port| !others.iter().any(|a| a.gateway_port == Some(*port)))
        .unwrap_or(7100)
}

fn ask_gateway_env(app: &mut App, _around: &Surroundings) -> Result<()> {
    // Without a gateway port there is no address to carry, so the variable
    // would name nothing.
    if app.gateway_port.is_none() {
        app.gateway_env = None;
        return Ok(());
    }
    let default = match &app.gateway_env {
        Some(name) => name.clone(),
        None => suggested_env_name(Path::new(&app.path)).to_string(),
    };
    let answer: String = Input::new()
        .with_prompt("Variable that carries the gateway URL to the app")
        .default(default)
        .interact_text()?;
    app.gateway_env = Some(super::app::validate_env_name(answer)?);
    Ok(())
}

fn ask_env_file(app: &mut App, _around: &Surroundings) -> Result<()> {
    // The file only ever holds the gateway line, so it is pointless without a port.
    if app.gateway_port.is_none() {
        return Ok(());
    }
    let answer: String = Input::new()
        .with_prompt("Dotenv file turnout keeps in step with the port")
        .default(app.env_file_name().to_string())
        .interact_text()?;
    let answer = super::app::validate_env_file(answer)?;
    // Storing the default explicitly would freeze today's default into the
    // catalog; `None` keeps following it.
    app.env_file = (answer != crate::model::DEFAULT_ENV_FILE).then_some(answer);
    Ok(())
}

fn ask_dev_port(app: &mut App, around: &Surroundings) -> Result<()> {
    let answer: String = Input::new()
        .with_prompt("Dev server port (empty to let the next `dev` assign one)")
        .default(app.dev_port.map(|p| p.to_string()).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;
    let answer = answer.trim();
    if answer.is_empty() {
        app.dev_port = None;
        return Ok(());
    }
    let port: u16 = answer.parse().context("port must be a number")?;
    crate::front::ensure_dev_port_is_free(&around.others, port, &app.name)?;
    app.dev_port = Some(port);
    Ok(())
}

fn ask_dist_dir(app: &mut App, _around: &Surroundings) -> Result<()> {
    let default = app.dist_dir.clone().unwrap_or_else(|| suggested_dist(Path::new(&app.path)));
    let answer: String = Input::new()
        .with_prompt("Build artifact directory, relative to the project (empty for none)")
        .default(default)
        .allow_empty(true)
        .interact_text()?;
    let answer = answer.trim().to_string();
    app.dist_dir = (!answer.is_empty()).then_some(answer);
    Ok(())
}

/// The build directory this project most likely produces.
fn suggested_dist(path: &Path) -> String {
    ["dist", "build", "out", "public"]
        .iter()
        .find(|candidate| path.join(candidate).is_dir())
        .unwrap_or(&"dist")
        .to_string()
}

fn ask_servers(app: &mut App, around: &Surroundings) -> Result<()> {
    if around.known.is_empty() {
        return Ok(());
    }
    app.servers = super::app::pick_servers(&around.known, &app.servers)?;
    Ok(())
}

/// The variable a framework can actually see: Vite exposes only `VITE_*` to
/// the client, so a Vite project gets that prefix by default.
fn suggested_env_name(path: &Path) -> &'static str {
    let vite = std::fs::read_to_string(path.join("package.json")).is_ok_and(|text| text.contains("\"vite\""));
    if vite { "VITE_API_URL" } else { crate::model::DEFAULT_GATEWAY_ENV }
}

// ---------------------------------------------------------------- commands --

/// turnout's own roles, offered first when naming a command.
const ROLES: &[&str] = &["dev", "build", "test", "lint"];

/// The commands question: show what the app has, then work on the list.
///
/// Detection proposes; it does not rule. Answering "no" to the proposal used to
/// leave the app with no commands at all and a note about flags - now the list
/// is simply there to be edited.
fn ask_commands(app: &mut App, around: &Surroundings) -> Result<()> {
    if around.adding {
        let path = Path::new(&app.path);
        let kind = detect::detect(path);
        let source = if path.join("package.json").exists() { ", from package.json" } else { "" };
        println!("Detected a {} project{source}.", kind.label());
    }
    loop {
        println!("Commands of '{}':", app.name);
        if app.commands.is_empty() {
            println!("  (none yet)");
        } else {
            super::app::print_commands(&app.commands);
        }
        let mut labels = vec!["Done".to_string(), "Add a command".to_string()];
        if !app.commands.is_empty() {
            labels.push("Change a command line".to_string());
            labels.push("Rename a command".to_string());
            labels.push("Remove a command".to_string());
        }
        match Select::new().with_prompt("Commands").items(&labels).default(0).interact()? {
            0 => return Ok(()),
            1 => add_command(&mut app.commands)?,
            2 => change_command(&mut app.commands)?,
            3 => rename_command(&mut app.commands)?,
            _ => remove_command(&mut app.commands)?,
        }
    }
}

fn add_command(commands: &mut BTreeMap<String, String>) -> Result<()> {
    // The roles turnout runs by name come first; anything else is typed in and
    // stays reachable through `turnout run NAME`.
    let free: Vec<&str> = ROLES.iter().copied().filter(|role| !commands.contains_key(*role)).collect();
    let name = if free.is_empty() {
        Input::<String>::new().with_prompt("Command name").interact_text()?
    } else {
        let mut labels: Vec<String> = free.iter().map(|role| (*role).to_string()).collect();
        labels.push("Another name...".to_string());
        let choice = Select::new().with_prompt("Command name").items(&labels).default(0).interact()?;
        match free.get(choice) {
            Some(role) => (*role).to_string(),
            None => Input::<String>::new().with_prompt("Command name").interact_text()?,
        }
    };
    let name = validate_command_name(name)?;
    if commands.contains_key(&name) {
        bail!("'{name}' is already a command - change its command line instead");
    }
    let line: String = Input::new().with_prompt(format!("Command line for '{name}'")).interact_text()?;
    commands.insert(name, line);
    Ok(())
}

fn change_command(commands: &mut BTreeMap<String, String>) -> Result<()> {
    let Some(name) = pick_command(commands, "Change which command")? else {
        return Ok(());
    };
    let current = commands[&name].clone();
    let line: String = Input::new()
        .with_prompt(format!("Command line for '{name}'"))
        .default(current)
        .interact_text()?;
    commands.insert(name, line);
    Ok(())
}

fn rename_command(commands: &mut BTreeMap<String, String>) -> Result<()> {
    let Some(name) = pick_command(commands, "Rename which command")? else {
        return Ok(());
    };
    let new_name: String = Input::new().with_prompt("New name").default(name.clone()).interact_text()?;
    let new_name = validate_command_name(new_name)?;
    if new_name == name {
        return Ok(());
    }
    if commands.contains_key(&new_name) {
        bail!("'{new_name}' is already a command");
    }
    let line = commands.remove(&name).expect("picked from this map");
    commands.insert(new_name, line);
    Ok(())
}

fn remove_command(commands: &mut BTreeMap<String, String>) -> Result<()> {
    let Some(name) = pick_command(commands, "Remove which command")? else {
        return Ok(());
    };
    commands.remove(&name);
    Ok(())
}

/// Pick one command by name; the last entry backs out without choosing.
fn pick_command(commands: &BTreeMap<String, String>, prompt: &str) -> Result<Option<String>> {
    let names: Vec<String> = commands.keys().cloned().collect();
    let mut labels: Vec<String> = commands.iter().map(|(name, line)| format!("{name}  ({line})")).collect();
    labels.push("Back".to_string());
    let choice = Select::new().with_prompt(prompt).items(&labels).default(0).interact()?;
    Ok(names.get(choice).cloned())
}

/// A command name turnout can take on the command line as one word.
fn validate_command_name(name: String) -> Result<String> {
    let name = name.trim().to_string();
    let valid = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':');
    if !valid {
        bail!("'{name}' is not a command name: letters, digits, dashes, underscores and colons");
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate against the drift that made this module necessary: every field
    /// of `App` is either walked by the form or named here as deliberately
    /// outside it. A new field in the model fails this test until it is placed.
    ///
    /// The model is read from its own source, so the test cannot go stale
    /// against a struct it only remembers.
    #[test]
    fn the_form_asks_for_every_field_of_the_model() {
        let source = include_str!("../model.rs");
        let body = source
            .split_once("pub struct App {")
            .expect("the App struct is declared in model.rs")
            .1
            .split_once("\n}")
            .expect("the App struct is closed")
            .0;
        let model_fields: Vec<&str> = body
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub "))
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.trim())
            .collect();
        assert!(model_fields.len() >= 8, "failed to read the fields of App: {model_fields:?}");

        // `name` identifies the app: `add` takes it before the form opens and
        // `edit` addresses the app by it, so the form never asks for it.
        let outside_the_form = ["name"];
        let covered = covered_fields();
        let missing: Vec<&str> = model_fields
            .iter()
            .copied()
            .filter(|field| !covered.contains(field) && !outside_the_form.contains(field))
            .collect();
        assert!(
            missing.is_empty(),
            "these fields of App are not in the wizard: {missing:?} - add them to FIELDS, or name them in `outside_the_form` with the reason"
        );
    }

    /// Detection fills the gaps and never overwrites what is already set.
    #[test]
    fn detected_commands_do_not_clobber_the_ones_already_set() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), r#"{"scripts":{"dev":"vite","build":"vite build"}}"#).unwrap();
        std::fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        let mut current = BTreeMap::new();
        current.insert("dev".to_string(), "my own dev".to_string());
        let merged = merge_detected(&current, dir.path());
        assert_eq!(merged["dev"], "my own dev", "a command set by hand survives detection");
        assert_eq!(merged["build"], "npm run build", "a role the user has not set is filled in");
    }

    #[test]
    fn a_command_name_is_one_word() {
        assert_eq!(validate_command_name("build:prod".to_string()).unwrap(), "build:prod");
        assert_eq!(validate_command_name("  dev  ".to_string()).unwrap(), "dev");
        assert!(validate_command_name("two words".to_string()).is_err());
        assert!(validate_command_name(String::new()).is_err());
    }

    /// The port offered to a new app is one nothing else holds.
    #[test]
    fn the_offered_port_steps_over_the_ones_taken() {
        let app = |port: u16| App {
            name: format!("app{port}"),
            path: String::new(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: Some(port),
            gateway_env: None,
            env_file: None,
            dev_port: None,
            servers: Vec::new(),
        };
        assert_eq!(free_gateway_port(&[]), 7100);
        assert_eq!(free_gateway_port(&[app(7100), app(7101)]), 7102);
        assert_eq!(free_gateway_port(&[app(7101)]), 7100);
    }
}
