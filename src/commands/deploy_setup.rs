//! One wizard for everything a deploy needs, instead of five `edit` calls.
//!
//! Since the v0.9.0 split a deploy touches four entities - the app's artifact
//! directory, the server, the credential that logs in and the path files land
//! in - plus a secret in the keyring. The wizard exists precisely so nobody has
//! to assemble that by hand: it walks them in the order they are needed, offers
//! existing entries where there are any, and only writes once every answer is
//! in.

use std::path::Path;

use anyhow::{Result, bail};
use dialoguer::{Confirm, Input, Password, Select};

use crate::model::{App, Auth, Credential, Server, parse_host, validate_name, validate_remote_path};
use crate::{pick, secrets, store};

pub fn run(app_name: Option<String>, server_name: Option<String>) -> Result<()> {
    pick::ensure_interactive("`deploy-setup` is a wizard; in scripts configure with `turnout app edit` and `turnout server edit`")?;
    let mut apps = store::load_apps()?;
    let mut servers = store::load_servers()?;
    let mut credentials = store::load_credentials()?;
    let mut paths = store::load_paths()?;
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

    // 2. How to reach the machine. An empty answer keeps the URL's host, which
    //    is right for most stands.
    let current_host = servers[server_index]
        .host
        .as_ref()
        .map(|h| format!("{h}:{}", servers[server_index].port))
        .unwrap_or_default();
    let host_spec: String = Input::new()
        .with_prompt("SSH host[:port] (empty to use the URL's host)")
        .default(current_host)
        .allow_empty(true)
        .interact_text()?;
    let (host, port) = if host_spec.trim().is_empty() {
        (None, 22)
    } else {
        let (host, port) = parse_host(host_spec.trim())?;
        (Some(host), port)
    };

    // 3. Who logs in. An existing credential is reused rather than duplicated -
    //    that reuse is the whole reason credentials became their own entity.
    let credential_name = choose_credential(&mut credentials, servers[server_index].credential.as_deref())?;
    let credential_index = credentials.iter().position(|c| c.name == credential_name).expect("just chosen or created");

    // 4. Where the files land, as a named path.
    let path_name = choose_path(&mut paths, servers[server_index].deploy.get(&app_name).map(String::as_str))?;

    // 5. The secret, only when this credential logs in by password.
    let mut secret_to_save = None;
    if credentials[credential_index].auth == Auth::Password {
        let has_secret = secrets::get(&credential_name).is_ok();
        let prompt = if has_secret {
            format!("Replace the stored secret of '{credential_name}'?")
        } else {
            format!("Store a secret for '{credential_name}' in the OS keyring?")
        };
        if Confirm::new().with_prompt(prompt).default(!has_secret).interact()? {
            let secret = Password::new()
                .with_prompt("Secret")
                .with_confirmation("Repeat to confirm", "Values do not match")
                .interact()?;
            if secret.is_empty() {
                bail!("empty secret - nothing saved");
            }
            secret_to_save = Some(secret);
        }
    }

    // Everything answered - now write.
    let allowed = apply(
        &mut apps[app_index],
        &mut servers[server_index],
        Answers {
            dist: dist.trim(),
            host,
            port,
            credential: &credential_name,
            path: &path_name,
        },
    );
    if allowed {
        println!("  Allowed '{server_name}' for '{app_name}'.");
    }
    store::save_apps(&apps)?;
    store::save_servers(&servers)?;
    store::save_credentials(&credentials)?;
    store::save_paths(&paths)?;
    if let Some(secret) = secret_to_save {
        secrets::set(&credential_name, &secret)?;
    }

    println!("Done. Deploy with:");
    println!("  turnout deploy {app_name} --server {server_name}");
    Ok(())
}

/// Pick an existing credential or define a new one, appending it to `credentials`.
fn choose_credential(credentials: &mut Vec<Credential>, current: Option<&str>) -> Result<String> {
    if !credentials.is_empty() {
        let mut labels: Vec<String> = credentials.iter().map(|c| format!("{}  ({}@, {})", c.name, c.user, c.auth)).collect();
        labels.push("+ a new credential".to_string());
        // Default to the one this server already uses, so re-running the wizard
        // is a confirmation rather than a re-entry.
        let default = current.and_then(|name| credentials.iter().position(|c| c.name == name)).unwrap_or(0);
        let choice = Select::new().with_prompt("Logs in with").items(&labels).default(default).interact()?;
        if choice < credentials.len() {
            return Ok(credentials[choice].name.clone());
        }
    }

    let name: String = Input::new()
        .with_prompt("New credential name")
        .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
        .interact_text()?;
    if credentials.iter().any(|c| c.name == name) {
        bail!("credential '{name}' already exists");
    }
    let user: String = Input::new().with_prompt("Logs in as (remote user)").interact_text()?;
    if user.trim().is_empty() {
        bail!("the remote user cannot be empty");
    }
    let auth = if Select::new()
        .with_prompt("Authenticates with")
        .items(["a password", "a private key file"])
        .default(0)
        .interact()?
        == 0
    {
        Auth::Password
    } else {
        Auth::Key
    };
    let key = if auth == Auth::Key {
        let answer: String = Input::new().with_prompt("Private key file").interact_text()?;
        Some(answer.trim().to_string())
    } else {
        None
    };
    credentials.push(Credential {
        name: name.clone(),
        user: user.trim().to_string(),
        auth,
        key,
    });
    credentials.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(name)
}

/// Pick an existing path or define a new one, appending it to `paths`.
fn choose_path(paths: &mut Vec<crate::model::Path>, current: Option<&str>) -> Result<String> {
    if !paths.is_empty() {
        let mut labels: Vec<String> = paths.iter().map(|p| format!("{}  ({})", p.name, p.dir)).collect();
        labels.push("+ a new path".to_string());
        let default = current.and_then(|name| paths.iter().position(|p| p.name == name)).unwrap_or(0);
        let choice = Select::new().with_prompt("Files land in").items(&labels).default(default).interact()?;
        if choice < paths.len() {
            return Ok(paths[choice].name.clone());
        }
    }

    let name: String = Input::new()
        .with_prompt("New path name")
        .validate_with(|s: &String| validate_name(s).map_err(|e| e.to_string()))
        .interact_text()?;
    if paths.iter().any(|p| p.name == name) {
        bail!("path '{name}' already exists");
    }
    let dir: String = Input::new()
        .with_prompt("Remote directory")
        .validate_with(|s: &String| validate_remote_path(s).map_err(|e| e.to_string()))
        .interact_text()?;
    let restart: String = Input::new()
        .with_prompt("Command to run after upload (empty for none)")
        .allow_empty(true)
        .interact_text()?;
    paths.push(crate::model::Path {
        name: name.clone(),
        dir: crate::model::normalize_remote_path(dir.trim()),
        restart: if restart.trim().is_empty() { None } else { Some(restart.trim().to_string()) },
    });
    paths.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(name)
}

/// What the wizard collected, ready to be written onto the entities.
struct Answers<'a> {
    dist: &'a str,
    host: Option<String>,
    port: u16,
    credential: &'a str,
    path: &'a str,
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
    server.host = answers.host;
    server.port = answers.port;
    server.credential = Some(answers.credential.to_string());
    server.deploy.insert(app.name.clone(), answers.path.to_string());
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
            host: None,
            port: 22,
            accept_invalid_certs: false,
            credential: None,
            deploy: BTreeMap::new(),
            shell: None,
        }
    }

    fn answers() -> Answers<'static> {
        Answers {
            dist: "dist",
            host: Some("prod.example.com".into()),
            port: 2222,
            credential: "deploy",
            path: "wwwroot",
        }
    }

    #[test]
    fn writes_every_field_the_deploy_needs() {
        let (mut app, mut server) = (an_app(&[]), a_server());
        apply(&mut app, &mut server, answers());
        assert_eq!(app.dist_dir.as_deref(), Some("dist"));
        assert_eq!(server.deploy.get("web").map(String::as_str), Some("wwwroot"));
        assert_eq!(server.credential.as_deref(), Some("deploy"));
        assert_eq!(server.host.as_deref(), Some("prod.example.com"));
        assert_eq!(server.port, 2222);
    }

    /// Leaving the host empty is how a user says "same as the URL", and it has
    /// to survive as `None` rather than being frozen into a copy of the URL's
    /// host that stops following it.
    #[test]
    fn an_empty_host_keeps_following_the_url() {
        let (mut app, mut server) = (an_app(&[]), a_server());
        apply(&mut app, &mut server, Answers { host: None, ..answers() });
        assert!(server.host.is_none());
        assert_eq!(server.ssh_host(), "prod.example.com", "it resolves through the URL");
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
