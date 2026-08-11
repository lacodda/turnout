//! One wizard for everything a deploy needs, instead of five `edit` calls.
//!
//! Deployment settings live in two entities - the artifact directory on the
//! app, the SSH access and per-app target on the server - plus a secret in the
//! keyring. This walks all of them in the order they are needed and only
//! writes once every answer is in.

use std::path::Path;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Password};

use crate::model::{App, DeployTarget, Server, Ssh, parse_ssh};
use crate::{pick, secrets, store};

pub fn run(app_name: Option<String>, server_name: Option<String>) -> Result<()> {
    pick::ensure_interactive("`deploy-setup` is a wizard; in scripts configure with `turnout app edit` and `turnout server edit`")?;
    let mut apps = store::load_apps()?;
    let mut servers = store::load_servers()?;
    let state = store::load_state()?;

    let app_name = match app_name {
        Some(name) => name,
        None => pick::app(&apps, &state, "Deploy which app")?,
    };
    let app_index = apps
        .iter()
        .position(|a| a.name == app_name)
        .ok_or_else(|| anyhow::anyhow!("no app named '{app_name}' - see `turnout app list`"))?;

    let server_name = match server_name {
        Some(name) => name,
        None => pick::server_for_app(&servers, &apps[app_index], "Deploy to")?,
    };
    let server_index = servers
        .iter()
        .position(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;

    println!("Setting up deploy of '{app_name}' to '{server_name}'.");

    // 1. What to upload. Suggest the usual suspects if one is already on disk.
    let project = Path::new(&apps[app_index].path).to_path_buf();
    let suggested = apps[app_index]
        .dist_dir
        .clone()
        .or_else(|| {
            ["dist", "build", "out", "public"]
                .iter()
                .find(|d| project.join(d).is_dir())
                .map(|d| d.to_string())
        })
        .unwrap_or_else(|| "dist".to_string());
    let dist: String = Input::new()
        .with_prompt("Artifact directory (relative to the project)")
        .default(suggested)
        .interact_text()?;
    if !project.join(dist.trim()).is_dir() {
        println!(
            "  Note: {} does not exist yet - the build should produce it.",
            project.join(dist.trim()).display()
        );
    }

    // 2. How to reach the server.
    let current_ssh = servers[server_index]
        .ssh
        .as_ref()
        .map(|s| format!("{}@{}:{}", s.user, s.host, s.port))
        .unwrap_or_default();
    let mut ssh_input = Input::<String>::new().with_prompt("SSH as user@host[:port]");
    if !current_ssh.is_empty() {
        ssh_input = ssh_input.default(current_ssh);
    }
    let ssh_spec: String = ssh_input.interact_text()?;
    let mut ssh = parse_ssh(ssh_spec.trim())?;
    // A key already configured for this server stays unless replaced below.
    let existing_key = servers[server_index].ssh.as_ref().and_then(|s| s.key.clone());
    let key: String = Input::new()
        .with_prompt("Private key file (empty to use the agent or a password)")
        .default(existing_key.unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;
    ssh.key = if key.trim().is_empty() { None } else { Some(key.trim().to_string()) };

    // 3. Where the files land and what runs afterwards.
    let existing_target = servers[server_index].deploy.get(&app_name);
    let mut path_input = Input::<String>::new()
        .with_prompt("Remote directory")
        .validate_with(|s: &String| crate::model::validate_remote_path(s).map_err(|e| e.to_string()));
    if let Some(target) = existing_target {
        path_input = path_input.default(target.path.clone());
    }
    let remote_path: String = path_input.interact_text()?;
    let restart: String = Input::new()
        .with_prompt("Command to run after upload (empty for none)")
        .default(existing_target.and_then(|t| t.restart.clone()).unwrap_or_default())
        .allow_empty(true)
        .interact_text()?;

    // 4. The secret, only when there is no key to authenticate with.
    let mut save_secret = None;
    if ssh.key.is_none() {
        let has_secret = secrets::get(&server_name, "ssh").is_ok();
        let prompt = if has_secret {
            "Replace the stored SSH password?"
        } else {
            "Store an SSH password in the OS keyring?"
        };
        if Confirm::new().with_prompt(prompt).default(!has_secret).interact()? {
            let secret = Password::new()
                .with_prompt("SSH password")
                .with_confirmation("Repeat to confirm", "Values do not match")
                .interact()?;
            if secret.is_empty() {
                bail!("empty secret - nothing saved");
            }
            save_secret = Some((ssh.user.clone(), secret));
        }
    }

    // Everything answered - now write.
    let allowed = apply(
        &mut apps[app_index],
        &mut servers[server_index],
        Answers {
            dist: dist.trim(),
            ssh,
            remote_path: remote_path.trim(),
            restart: restart.trim(),
        },
    );
    if allowed {
        println!("  Allowed '{server_name}' for '{app_name}'.");
    }
    store::save_apps(&apps)?;
    store::save_servers(&servers)?;

    if let Some((login, secret)) = save_secret {
        secrets::set(&server_name, "ssh", &secret)?;
        let mut credentials = store::load_credentials()?;
        match credentials.iter_mut().find(|c| c.server == server_name && c.kind == "ssh") {
            Some(credential) => credential.login = login,
            None => credentials.push(crate::model::Credential {
                server: server_name.clone(),
                kind: "ssh".to_string(),
                login,
            }),
        }
        credentials.sort_by(|a, b| (a.server.as_str(), a.kind.as_str()).cmp(&(b.server.as_str(), b.kind.as_str())));
        store::save_credentials(&credentials)?;
    }

    println!("Done. Deploy with:");
    println!("  turnout deploy {app_name} --server {server_name}");
    Ok(())
}

/// What the wizard collected, ready to be written onto the entities.
struct Answers<'a> {
    dist: &'a str,
    ssh: Ssh,
    remote_path: &'a str,
    restart: &'a str,
}

/// Write the answers onto the app and the server. Returns whether the server
/// had to be added to the app's allow-list.
fn apply(app: &mut App, server: &mut Server, answers: Answers) -> bool {
    app.dist_dir = Some(answers.dist.to_string());
    // An empty allow-list means "any server", so it must stay empty.
    let allowed = !app.servers.is_empty() && !app.servers.contains(&server.name);
    if allowed {
        app.servers.push(server.name.clone());
    }
    server.ssh = Some(answers.ssh);
    server.deploy.insert(
        app.name.clone(),
        DeployTarget {
            path: crate::model::normalize_remote_path(answers.remote_path),
            restart: if answers.restart.is_empty() {
                None
            } else {
                Some(answers.restart.to_string())
            },
        },
    );
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn an_app(servers: &[&str]) -> App {
        App {
            name: "web".into(),
            path: "/tmp/web".into(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: None,
            servers: servers.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn a_server() -> Server {
        Server {
            name: "prod".into(),
            label: None,
            url: "https://prod.example.com".into(),
            ssh: None,
            accept_invalid_certs: false,
            deploy: BTreeMap::new(),
        }
    }

    fn answers() -> Answers<'static> {
        Answers {
            dist: "dist",
            ssh: parse_ssh("deploy@prod.example.com").unwrap(),
            remote_path: "/var/www/web",
            restart: "systemctl restart web",
        }
    }

    #[test]
    fn writes_every_field_the_deploy_needs() {
        let (mut app, mut server) = (an_app(&[]), a_server());
        apply(&mut app, &mut server, answers());
        assert_eq!(app.dist_dir.as_deref(), Some("dist"));
        let target = server.deploy.get("web").unwrap();
        assert_eq!(target.path, "/var/www/web");
        assert_eq!(target.restart.as_deref(), Some("systemctl restart web"));
        assert_eq!(server.ssh.as_ref().unwrap().user, "deploy");
    }

    #[test]
    fn an_empty_restart_stays_unset() {
        let (mut app, mut server) = (an_app(&[]), a_server());
        apply(&mut app, &mut server, Answers { restart: "", ..answers() });
        assert!(server.deploy.get("web").unwrap().restart.is_none());
    }

    #[test]
    fn a_restricted_app_gains_the_target_server() {
        let (mut app, mut server) = (an_app(&["staging"]), a_server());
        assert!(apply(&mut app, &mut server, answers()));
        assert!(app.servers.contains(&"prod".to_string()));
        // An unrestricted app must not gain a list, which would restrict it.
        let (mut app, mut server) = (an_app(&[]), a_server());
        assert!(!apply(&mut app, &mut server, answers()));
        assert!(app.servers.is_empty());
    }
}
