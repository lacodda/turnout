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
    /// Which shell answers SSH here, learned by probing on first use.
    ///
    /// Cached because it cannot change without someone reconfiguring sshd, and
    /// paying a round trip for it on every command would be a tax on the common
    /// case. `None` means "not asked yet", which is also what every server
    /// written by an older turnout looks like.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<crate::shell::Dialect>,
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

/// A directory on the server: an absolute path, POSIX or Windows.
///
/// Two different things look identical here, and telling them apart is the
/// whole job:
///
/// - A Windows *server* has directories like `C:\inetpub\site`, and refusing
///   them would mean turnout cannot deploy to Windows at all.
/// - Git Bash rewrites POSIX-looking arguments before the process sees them, so
///   a user typing `/var/www/app` can arrive here as
///   `C:/Program Files/Git/var/www/app` - a path nobody meant.
///
/// The tell is the Git Bash installation prefix, not the drive letter: the
/// rewrite always splices the MSYS root in front of the path the user typed.
/// A plain `C:\inetpub\site` carries no such prefix and is taken at face value.
pub fn validate_remote_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        bail!("remote path is empty: pass an absolute path on the server, e.g. /var/www/myapp");
    }
    if let Some(original) = looks_rewritten_by_git_bash(path) {
        bail!(
            "'{path}' looks like Git Bash rewrote '{original}' into a local path before turnout saw it.\n\
             Prefix the command with MSYS_NO_PATHCONV=1, double the leading slash (/{original}), or use PowerShell.\n\
             If you really meant this directory on a Windows server, pass it with backslashes."
        );
    }
    if !is_absolute_remote(path) {
        bail!("remote path '{path}' must be absolute, e.g. /var/www/myapp or C:\\inetpub\\myapp");
    }
    Ok(())
}

/// Absolute for either kind of server: a leading `/`, a drive (`C:\` or `C:/`)
/// or a UNC share.
fn is_absolute_remote(path: &str) -> bool {
    path.starts_with('/') || has_drive_letter(path) || path.starts_with("\\\\")
}

/// The POSIX path a Git Bash rewrite started from, if this looks like one.
///
/// The rewrite splices the MSYS installation root in front, so the giveaway is
/// a drive-letter path containing a known Git-for-Windows prefix followed by
/// what the user actually typed.
fn looks_rewritten_by_git_bash(path: &str) -> Option<&str> {
    if !has_drive_letter(path) {
        return None;
    }
    let normalized = path.replace('\\', "/");
    // Only the prefixes MSYS actually uses; a server directory that merely
    // contains the word "git" is not a rewrite.
    const MSYS_ROOTS: [&str; 4] = ["/Program Files/Git/", "/Program Files (x86)/Git/", "/git/", "/msys64/"];
    let rest_index = MSYS_ROOTS.iter().find_map(|root| normalized.find(root).map(|at| at + root.len()))?;
    // Borrow from the original so the caller can quote it back verbatim.
    Some(&path[rest_index..])
}

/// `C:\dir` or `C:/dir` - a path anchored at a drive.
fn has_drive_letter(path: &str) -> bool {
    let mut chars = path.chars();
    matches!((chars.next(), chars.next(), chars.next()), (Some(c), Some(':'), Some('/' | '\\')) if c.is_ascii_alphabetic())
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
    fn remote_paths_must_be_absolute() {
        assert!(validate_remote_path("/var/www/myapp").is_ok());
        assert!(validate_remote_path("/srv/app with spaces").is_ok());
        assert!(validate_remote_path("").is_err());
        assert!(validate_remote_path("   ").is_err());
        assert!(validate_remote_path("var/www/myapp").is_err(), "relative paths are not a server directory");
        assert!(validate_remote_path("inetpub\\myapp").is_err(), "relative is relative on Windows too");
    }

    /// A Windows server's directories are drive-letter paths. Refusing them
    /// outright - as every release before v0.7.0 did - meant turnout could not
    /// be told where to deploy on Windows at all.
    #[test]
    fn windows_server_directories_are_accepted() {
        assert!(validate_remote_path("C:\\inetpub\\wwwroot\\myapp").is_ok());
        assert!(validate_remote_path("D:/sites/myapp").is_ok());
        assert!(validate_remote_path("C:\\sites\\my app").is_ok(), "spaces are ordinary in Windows paths");
        assert!(
            validate_remote_path("\\\\fileserver\\share\\myapp").is_ok(),
            "a UNC share is a real destination"
        );
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
        // A UNC share opens with backslashes, and both of them are load-bearing.
        assert_eq!(normalize_remote_path("\\\\fileserver\\share"), "\\\\fileserver\\share");
    }

    /// The exact shape Git Bash produces from `/var/www/myapp`. The tell is the
    /// MSYS root spliced in front, which is what separates this from a genuine
    /// Windows server path.
    #[test]
    fn a_git_bash_rewrite_is_caught_and_explained() {
        let mangled = validate_remote_path("C:/Program Files/Git/var/www/myapp").unwrap_err().to_string();
        assert!(mangled.contains("MSYS_NO_PATHCONV"), "the error must say how to get past it: {mangled}");
        assert!(mangled.contains("var/www/myapp"), "it must name the path the user meant: {mangled}");

        assert!(validate_remote_path("C:\\Program Files\\Git\\var\\www\\myapp").is_err(), "backslashes too");
        assert!(validate_remote_path("C:/msys64/var/www/myapp").is_err());
    }

    /// The rewrite check keys on the MSYS root, so a Windows directory that
    /// merely mentions git is a normal destination.
    #[test]
    fn a_windows_path_that_mentions_git_is_not_a_rewrite() {
        assert!(validate_remote_path("C:\\sites\\gitlab-runner").is_ok());
        assert!(validate_remote_path("D:\\git-repos\\myapp").is_ok());
    }
}
