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

/// A named set of apps: `turnout use GROUP SERVER` switches the whole contour.
#[derive(Serialize, Deserialize, Clone)]
pub struct Group {
    pub name: String,
    pub apps: Vec<String>,
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

/// Normalize a remote directory for storage: `//var/www/app` becomes
/// `/var/www/app`.
///
/// The doubled leading slash is what a Git Bash user types to stop the shell
/// from rewriting the argument, so it arrives here as part of a working path
/// rather than a mistake. POSIX treats `//foo` as implementation-defined, and
/// it would show up in every line that echoes the path back.
pub fn normalize_remote_path(path: &str) -> String {
    let path = path.trim();
    match path.strip_prefix("//") {
        Some(rest) if !rest.starts_with('/') => format!("/{rest}"),
        _ => path.to_string(),
    }
}

/// A directory on the server, which is a POSIX path - never a local one.
///
/// The check exists because Git Bash rewrites POSIX-looking arguments into
/// Windows paths before the process sees them: `/var/www/app` arrives as
/// `C:/Program Files/Git/var/www/app`. Storing that silently deploys to a
/// directory nobody meant, so a local-looking path is refused with the way out.
pub fn validate_remote_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        bail!("remote path is empty: pass an absolute path on the server, e.g. /var/www/myapp");
    }
    if looks_like_a_windows_path(path) {
        bail!(
            "'{path}' is a local Windows path, not a directory on the server.\n\
             Git Bash rewrites arguments that look like POSIX paths - prefix the command with \
             MSYS_NO_PATHCONV=1, double the leading slash (//var/www/myapp), or use PowerShell."
        );
    }
    if !path.starts_with('/') {
        bail!("remote path '{path}' must be absolute, e.g. /var/www/myapp");
    }
    Ok(())
}

/// `C:\dir`, `C:/dir` or a UNC share - all of them local, none of them a
/// destination on a server.
fn looks_like_a_windows_path(path: &str) -> bool {
    let drive_letter = {
        let mut chars = path.chars();
        matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some(':'), Some('/' | '\\')) if c.is_ascii_alphabetic())
    };
    drive_letter || path.starts_with("\\\\")
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

    #[test]
    fn remote_paths_are_posix_and_absolute() {
        assert!(validate_remote_path("/var/www/myapp").is_ok());
        assert!(validate_remote_path("/srv/app with spaces").is_ok());
        assert!(validate_remote_path("").is_err());
        assert!(validate_remote_path("   ").is_err());
        assert!(validate_remote_path("var/www/myapp").is_err(), "relative paths are not a server directory");
    }

    /// The doubled slash is the escape hatch the error message suggests, so a
    /// path typed that way must end up stored the same as any other.
    #[test]
    fn remote_paths_drop_the_escaping_slash() {
        assert_eq!(normalize_remote_path("//var/www/myapp"), "/var/www/myapp");
        assert_eq!(normalize_remote_path("/var/www/myapp"), "/var/www/myapp");
        assert_eq!(normalize_remote_path("  /srv/app  "), "/srv/app");
        // Three or more leading slashes are not the Git Bash idiom; left alone.
        assert_eq!(normalize_remote_path("///odd"), "///odd");
    }

    /// The exact shape Git Bash produces from `/var/www/myapp`, plus the other
    /// local forms that mean the value never reached the server as written.
    #[test]
    fn remote_paths_reject_windows_paths() {
        let mangled = validate_remote_path("C:/Program Files/Git/var/www/myapp").unwrap_err().to_string();
        assert!(mangled.contains("MSYS_NO_PATHCONV"), "the error must say how to get past it: {mangled}");

        assert!(validate_remote_path("C:\\www\\myapp").is_err());
        assert!(validate_remote_path("d:/sites/myapp").is_err(), "any drive letter, not just C");
        assert!(validate_remote_path("\\\\server\\share").is_err(), "UNC is local too");
    }
}
