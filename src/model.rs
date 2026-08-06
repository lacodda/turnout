use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// A local product the user works on.
#[derive(Serialize, Deserialize, Clone)]
pub struct App {
    pub name: String,
    /// Absolute path to the project directory.
    pub path: String,
    /// Named commands runnable via turnout, e.g. dev/build/test/lint.
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    /// Directory that counts as the build artifact, relative to `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dist_dir: Option<String>,
    /// Local port the app talks to; the gateway listens here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_port: Option<u16>,
    /// Names of servers this app is allowed to use.
    #[serde(default)]
    pub servers: Vec<String>,
}

/// A stand, machine or environment.
#[derive(Serialize, Deserialize, Clone)]
pub struct Server {
    pub name: String,
    /// Human-friendly label shown next to the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Base URL the gateway routes to, e.g. `https://staging.example.com`.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<Ssh>,
    /// Accept self-signed or otherwise invalid TLS certificates for this server only.
    #[serde(default)]
    pub accept_invalid_certs: bool,
    /// Deploy targets on this server, per app.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deploy: BTreeMap<String, DeployTarget>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DeployTarget {
    /// Remote directory the app's artifacts land in.
    pub path: String,
    /// Command run on the server after upload, e.g. a service restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
}

/// Access metadata for a server; the secret itself lives in the OS keyring.
#[derive(Serialize, Deserialize, Clone)]
pub struct Credential {
    pub server: String,
    /// What this access is: password, token, ssh, ...
    pub kind: String,
    pub login: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Ssh {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    /// Path to a private key file; tried before the agent and the stored password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

fn default_ssh_port() -> u16 {
    22
}

/// Current working mode, kept apart from the catalogs (`state.json`).
#[derive(Serialize, Deserialize, Default)]
pub struct State {
    /// Which server each app currently uses for development.
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<Gateway>,
}

/// Recorded facts about the background gateway process.
#[derive(Serialize, Deserialize, Clone)]
pub struct Gateway {
    pub pid: u32,
    /// Listening port per app at the moment the gateway started.
    pub ports: BTreeMap<u16, String>,
}

/// Entity names are stable identifiers: lowercase letters, digits and inner dashes.
pub fn validate_name(name: &str) -> Result<()> {
    let ok =
        !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !name.starts_with('-') && !name.ends_with('-');
    if !ok {
        bail!("invalid name '{name}': use lowercase letters, digits and dashes");
    }
    Ok(())
}

/// Parse `user@host[:port]` into an SSH config.
pub fn parse_ssh(spec: &str) -> Result<Ssh> {
    let (user, rest) = spec
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("invalid SSH spec '{spec}': expected user@host[:port]"))?;
    let (host, port) = match rest.split_once(':') {
        Some((host, port)) => (host, port.parse().map_err(|_| anyhow::anyhow!("invalid SSH port in '{spec}'"))?),
        None => (rest, default_ssh_port()),
    };
    if user.is_empty() || host.is_empty() {
        bail!("invalid SSH spec '{spec}': expected user@host[:port]");
    }
    Ok(Ssh {
        host: host.to_string(),
        port,
        user: user.to_string(),
        key: None,
    })
}

/// Minimal sanity check for a server base URL.
pub fn validate_url(url: &str) -> Result<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("invalid URL '{url}': must start with http:// or https://");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(validate_name("myapp").is_ok());
        assert!(validate_name("my-app-2").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("My App").is_err());
        assert!(validate_name("-x").is_err());
    }

    #[test]
    fn ssh_specs() {
        let ssh = parse_ssh("deploy@staging.example.com").unwrap();
        assert_eq!((ssh.user.as_str(), ssh.host.as_str(), ssh.port), ("deploy", "staging.example.com", 22));
        let ssh = parse_ssh("root@10.0.0.1:2222").unwrap();
        assert_eq!(ssh.port, 2222);
        assert!(parse_ssh("nohost").is_err());
        assert!(parse_ssh("user@host:notaport").is_err());
    }
}
