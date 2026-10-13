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
    /// Environment variable that carries the gateway URL to the app's
    /// commands; `None` means `DEFAULT_GATEWAY_ENV`. Per app because the
    /// framework decides what it can see: Vite only `VITE_*`, CRA `REACT_APP_*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway_env: Option<String>,
    /// Dotenv file, relative to `path`, that turnout keeps in step with the
    /// gateway port; `None` means `DEFAULT_ENV_FILE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_file: Option<String>,
    /// Port the app's dev server listens on; the front door routes
    /// `{name}.localhost` here. Fixed on the first `dev`, or pinned by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev_port: Option<u16>,
    /// Names of servers this app is allowed to use.
    #[serde(default)]
    pub servers: Vec<String>,
}

/// The variable an app gets when it names none.
pub const DEFAULT_GATEWAY_ENV: &str = "TURNOUT_GATEWAY_URL";
/// The dotenv file an app gets when it names none: read by Vite and CRA in
/// development only, on top of the project's own `.env`.
pub const DEFAULT_ENV_FILE: &str = ".env.development.local";

impl App {
    /// The address the app reaches the gateway on, when it has a port.
    pub fn gateway_url(&self) -> Option<String> {
        self.gateway_port.map(|port| format!("http://localhost:{port}"))
    }

    pub fn gateway_env_name(&self) -> &str {
        self.gateway_env.as_deref().unwrap_or(DEFAULT_GATEWAY_ENV)
    }

    pub fn env_file_name(&self) -> &str {
        self.env_file.as_deref().unwrap_or(DEFAULT_ENV_FILE)
    }
}

/// A machine: where it lives and how to reach it.
///
/// Split from the access used to log in (`Credential`) and the directories
/// worked in (`Path`) in v0.9.0. What stayed is everything that is a property of
/// the machine itself - including the base URL, because "which stand is this"
/// and "which host is this" are the same answer here, and the gateway routes by
/// that URL.
#[derive(Serialize, Deserialize, Clone)]
pub struct Server {
    pub name: String,
    /// Human-friendly label shown next to the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Base URL the gateway routes to, e.g. `https://staging.example.com`.
    pub url: String,
    /// SSH host, when it differs from the URL's - a stand often answers HTTP on
    /// one name and SSH on another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Accept self-signed or otherwise invalid TLS certificates for this server only.
    #[serde(default)]
    pub accept_invalid_certs: bool,
    /// Which shell answers SSH here, learned by probing on first use.
    ///
    /// Cached because it cannot change without someone reconfiguring sshd, and
    /// paying a round trip for it on every command would be a tax on the common
    /// case. `None` means "not asked yet".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<crate::shell::Dialect>,
    /// Credential used to log in here unless a command names another one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl Server {
    /// The host SSH connects to: the explicit one, or the URL's own host.
    pub fn ssh_host(&self) -> String {
        self.host.clone().unwrap_or_else(|| url_host(&self.url).to_string())
    }
}

/// A way to log in: who, and with what. The secret itself lives in the OS
/// keyring under the credential's name.
///
/// Free-standing since v0.9.0: one credential serves every server that accepts
/// it, instead of being re-entered per server.
#[derive(Serialize, Deserialize, Clone)]
pub struct Credential {
    pub name: String,
    /// The remote user this logs in as.
    pub user: String,
    #[serde(default)]
    pub auth: Auth,
    /// Private key file on this machine, when `auth` is `key`.
    ///
    /// A path, not a secret: it names a file the OS already protects, and
    /// keeping it in the catalog means `credential show` can display it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// How a credential proves itself.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    /// A password stored in the OS keyring.
    #[default]
    Password,
    /// A private key file on this machine.
    Key,
    /// A key held by a running SSH agent, which signs on its behalf.
    ///
    /// No key file and no stored secret: the passphrase was given to the agent
    /// once, and turnout never sees the key at all.
    Agent,
}

impl Auth {
    pub fn as_str(self) -> &'static str {
        match self {
            Auth::Password => "password",
            Auth::Key => "key",
            Auth::Agent => "agent",
        }
    }
}

impl std::fmt::Display for Auth {
    /// `pad`, not `write_str`: a column of auth kinds is laid out with `{:8}`,
    /// and `write_str` ignores the width it is given.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl std::str::FromStr for Auth {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self> {
        match text {
            "password" => Ok(Auth::Password),
            "key" => Ok(Auth::Key),
            "agent" => Ok(Auth::Agent),
            other => bail!("unknown auth kind '{other}': expected 'password', 'key' or 'agent'"),
        }
    }
}

/// A directory to work in on a server, plus what to run after writing to it.
///
/// Free-standing since v0.9.0 and deliberately not tied to a server: the same
/// web root exists on the staging box and the production one, and duplicating
/// it per server was the thing the split set out to stop.
#[derive(Serialize, Deserialize, Clone)]
pub struct Path {
    pub name: String,
    /// Absolute directory on the server, POSIX or Windows.
    pub dir: String,
    /// Command run on the server after upload, e.g. a service restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart: Option<String>,
}

/// A named deploy target: which app goes to which server, as whom, and where.
///
/// The four coordinates were free-standing entities since v0.9.0, but the
/// relationship between them lived inside the server as a map from app to path.
/// A relationship hidden inside one of its participants cannot be named, listed
/// or reused - which is exactly what ADR 0010 pulled `user@host` out of the
/// server for. Since v0.11.0 it is an entity of its own (ADR 0013), and the one
/// way a deploy is addressed.
#[derive(Serialize, Deserialize, Clone)]
pub struct Target {
    pub name: String,
    /// The app whose artifacts travel.
    pub app: String,
    /// The server they land on.
    pub server: String,
    /// The credential that logs in.
    pub credential: String,
    /// The named path they are written to.
    pub path: String,
    /// Where the deployed app answers, when that is not the server's own URL.
    ///
    /// A server's URL is the stand the gateway routes an app's requests to -
    /// usually an API. What a deploy puts on it is often seen elsewhere: an
    /// admin front end on its own host name, a site under a path. `open` and a
    /// relative health check need that address, not the API's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// How a deploy proves it worked. A deploy that cannot prove it is rolled
    /// back to what the stand ran before (schema 6, v0.24.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthCheck>,
}

impl Target {
    /// Where the deployed app answers: the target's own address, or the
    /// server's.
    pub fn address(&self, server: &Server) -> String {
        self.url.clone().unwrap_or_else(|| server.url.clone())
    }
}

/// What a deploy asks after the restart to know the stand works.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HealthCheck {
    #[serde(flatten)]
    pub probe: Probe,
    /// Seconds to keep asking before the deploy counts as failed; `None` is
    /// [`DEFAULT_HEALTH_WAIT`]. Per target, because a service decides how long
    /// it takes to start: a static site answers at once, a JVM after a minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<u64>,
}

/// How long a health check keeps asking when the target names no wait.
pub const DEFAULT_HEALTH_WAIT: u64 = 60;

impl HealthCheck {
    pub fn wait(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.wait.unwrap_or(DEFAULT_HEALTH_WAIT))
    }

    /// The check in a few words: `GET https://.../health`, `run curl -f ...`.
    pub fn describe(&self, address: &str) -> String {
        match &self.probe {
            Probe::Url(url) => format!("GET {}", health_url(url, address)),
            Probe::Command(command) => format!("`{command}`"),
        }
    }
}

/// One way to ask a stand whether it works. Exactly one per check: a check
/// that both fetched and ran something would leave "which one failed" open.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Probe {
    /// An address that answers below 400 when the stand works: absolute, or a
    /// path under the target's address (`/health`).
    Url(String),
    /// A command run on the server, in the deploy directory; exit code 0 means
    /// the stand works.
    Command(String),
}

/// The address a URL probe asks: an absolute one as it is, a path joined to
/// the target's address.
pub fn health_url(url: &str, address: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("{}/{}", address.trim_end_matches('/'), url.trim_start_matches('/'))
    }
}

/// A health URL is an address or a path under one; anything else would be
/// joined into nonsense.
pub fn validate_health_url(url: &str) -> Result<()> {
    if url.starts_with('/') || url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        bail!("invalid health URL '{url}': give a path under the target's address (/health) or a full http(s):// URL")
    }
}

/// The name a target gets when nobody typed one: `{app}-{server}`.
///
/// Both halves were named by the user, so this joins rather than invents - the
/// distinction ADR 0013 leans on to let the schema-3 migration name things at
/// all. Used by the migration and by the "save this as a target?" prompt, so the
/// two agree by construction.
pub fn target_name(app: &str, server: &str) -> String {
    format!("{app}-{server}")
}

/// A generated name that no existing target claims, suffixed if it has to be.
///
/// A collision means two different four-tuples produced the same pair, which
/// happens when one is a leftover; overwriting either silently would lose a
/// deploy target, and an ugly name is the cheaper outcome.
pub fn unique_target_name(app: &str, server: &str, taken: &[Target]) -> String {
    let base = target_name(app, server);
    if !taken.iter().any(|b| b.name == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken.iter().any(|b| &b.name == name))
        .expect("an unused suffix exists")
}

fn default_ssh_port() -> u16 {
    22
}

/// The host part of a base URL, without scheme, port, path or credentials.
///
/// Hand-written rather than pulled from a URL crate: the input is already
/// validated by [`validate_url`], and this is the one field ever needed from it.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Strip anything that follows the authority, then userinfo, then the port.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    // An IPv6 literal keeps its brackets: `[::1]:8080` splits at the last colon.
    match host.rsplit_once(':') {
        Some((before, after)) if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) => before,
        _ => host,
    }
}

/// A named set of apps: `turnout use GROUP SERVER` switches the whole contour.
#[derive(Serialize, Deserialize, Clone)]
pub struct Group {
    pub name: String,
    pub apps: Vec<String>,
}

/// Current working mode, kept apart from the catalogs (`state.json`).
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
pub struct State {
    /// Which server each app currently uses for development.
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
    /// The server each app used before its binding last changed - where
    /// `use -` takes it back to.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub previous: BTreeMap<String, String>,
    /// The app or group the last `use` switched: what a bare `use -` undoes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

impl State {
    /// Point `app` at `server`, remembering where it pointed before.
    ///
    /// Re-binding to the server it already uses changes nothing, so a repeated
    /// `use` does not make `use -` a no-op.
    pub fn bind(&mut self, app: &str, server: &str) {
        match self.bindings.insert(app.to_string(), server.to_string()) {
            Some(before) if before != server => {
                self.previous.insert(app.to_string(), before);
            }
            _ => {}
        }
    }

    /// An app left the catalog: it points nowhere and is nobody's last switch.
    pub fn forget_app(&mut self, app: &str) {
        self.bindings.remove(app);
        self.previous.remove(app);
        self.forget_name(app);
    }

    /// A server left the catalog: whatever pointed at it - now or before -
    /// points nowhere. Returns the apps that lost their binding.
    pub fn forget_server(&mut self, server: &str) -> Vec<String> {
        let unbound: Vec<String> = self.bindings.iter().filter(|(_, s)| *s == server).map(|(app, _)| app.clone()).collect();
        self.bindings.retain(|_, s| s != server);
        self.previous.retain(|_, s| s != server);
        unbound
    }

    /// A name that may have been the last switch is gone.
    pub fn forget_name(&mut self, name: &str) {
        if self.last.as_deref() == Some(name) {
            self.last = None;
        }
    }
}

/// The running gateway, read from its record in the job registry
/// ([`crate::registry::gateway`]). Until v0.20 this lived in `state.json`.
#[derive(Clone)]
pub struct Gateway {
    pub pid: u32,
    /// Seconds since the epoch.
    pub started: u64,
    /// Listening port per app at the moment the gateway started.
    pub ports: BTreeMap<u16, String>,
    /// Where the front door opened, when it did.
    pub front_port: Option<u16>,
    /// Where it takes requests about itself; `None` for a gateway started by
    /// a turnout before v0.22.
    pub control: Option<crate::registry::Control>,
    /// What it has noticed about the stands and is still seeing.
    pub signals: Vec<crate::signal::Signal>,
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

/// Parse `host[:port]` - what a server is addressed by since the v0.9.0 split.
///
/// The user half that `user@host` used to carry now belongs to a credential, so
/// a spec that still has one is a leftover from the old shape and says so
/// rather than silently dropping the name.
pub fn parse_host(spec: &str) -> Result<(String, u16)> {
    let spec = spec.trim();
    if let Some((user, rest)) = spec.split_once('@') {
        bail!(
            "'{spec}' names a user: since v0.9.0 the login lives in a credential, not the server.\n\
             Use the host alone (`{rest}`) and `turnout credential add --user {user}` for who logs in."
        );
    }
    let (host, port) = match spec.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().map_err(|_| anyhow::anyhow!("invalid SSH port in '{spec}'"))?),
        None => (spec, default_ssh_port()),
    };
    if host.is_empty() {
        bail!("invalid host '{spec}': expected host[:port]");
    }
    Ok((host.to_string(), port))
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
    fn host_specs() {
        assert_eq!(parse_host("staging.example.com").unwrap(), ("staging.example.com".to_string(), 22));
        assert_eq!(parse_host("10.0.0.1:2222").unwrap(), ("10.0.0.1".to_string(), 2222));
        assert!(parse_host("").is_err());
        assert!(parse_host("host:notaport").is_err());
    }

    /// The one shape a user upgrading from v0.8 will type out of habit. Dropping
    /// the user half silently would connect as the wrong account.
    #[test]
    fn a_user_at_host_spec_explains_where_the_user_went() {
        let err = parse_host("deploy@staging.example.com").unwrap_err().to_string();
        assert!(err.contains("credential"), "{err}");
        assert!(err.contains("--user deploy"), "the error must carry the name forward: {err}");
        assert!(err.contains("staging.example.com"), "and the host to use instead: {err}");
    }

    /// The SSH host falls back to the URL's, which is right for the common case
    /// where a stand answers HTTP and SSH on the same name.
    #[test]
    fn the_ssh_host_falls_back_to_the_url() {
        assert_eq!(url_host("https://staging.example.com"), "staging.example.com");
        assert_eq!(url_host("http://staging.example.com:8081/app"), "staging.example.com");
        assert_eq!(url_host("https://user@host.example.com/x"), "host.example.com");
        assert_eq!(url_host("http://[::1]:8080"), "[::1]");
        // A bare host with no scheme is still just a host.
        assert_eq!(url_host("example.com"), "example.com");
    }

    fn a_target(name: &str) -> Target {
        Target {
            name: name.into(),
            app: "web".into(),
            server: "prod".into(),
            credential: "deploy".into(),
            path: "wwwroot".into(),
            url: None,
            health: None,
        }
    }

    /// A check is written as the one probe it is, so a file can never hold a
    /// check that both fetches and runs; and a target without one reads and
    /// writes exactly as before schema 6.
    #[test]
    fn a_health_check_is_stored_as_its_one_probe() {
        let mut target = a_target("web-prod");
        assert!(!serde_json::to_string(&target).unwrap().contains("health"), "nothing to write");
        target.health = Some(HealthCheck {
            probe: Probe::Url("/health".into()),
            wait: None,
        });
        let json = serde_json::to_value(&target).unwrap();
        assert_eq!(json["health"], serde_json::json!({"url": "/health"}));
        let back: Target = serde_json::from_value(json).unwrap();
        assert_eq!(back.health, target.health);

        let command: HealthCheck = serde_json::from_str(r#"{"command":"curl -f localhost:3000","wait":120}"#).unwrap();
        assert_eq!(command.probe, Probe::Command("curl -f localhost:3000".into()));
        assert_eq!(command.wait(), std::time::Duration::from_secs(120));
        assert!(
            serde_json::from_str::<HealthCheck>(r#"{"wait":5}"#).is_err(),
            "a check with no probe is not a check"
        );
    }

    /// A path is asked under the target's address, a full URL as it is.
    #[test]
    fn a_health_path_joins_the_address() {
        assert_eq!(health_url("/health", "https://admin.example.com/"), "https://admin.example.com/health");
        assert_eq!(health_url("/api/health", "https://example.com/app"), "https://example.com/app/api/health");
        assert_eq!(health_url("http://10.0.0.5:3000/up", "https://example.com"), "http://10.0.0.5:3000/up");
        assert!(validate_health_url("/health").is_ok());
        assert!(validate_health_url("https://x.example.com/up").is_ok());
        assert!(validate_health_url("health").is_err(), "a bare word joins into nonsense");

        let mut target = a_target("web-prod");
        let server = Server {
            name: "prod".into(),
            label: None,
            url: "https://api.example.com".into(),
            host: None,
            port: 22,
            accept_invalid_certs: false,
            shell: None,
            credential: None,
        };
        assert_eq!(target.address(&server), "https://api.example.com", "the server's, when it names none");
        target.url = Some("https://admin.example.com".into());
        assert_eq!(target.address(&server), "https://admin.example.com");
    }

    /// The generated name joins two names the user already chose - that is what
    /// separates it from inventing one, and what lets the schema-3 migration
    /// name targets at all (ADR 0013).
    #[test]
    fn a_generated_target_name_joins_the_parts() {
        assert_eq!(target_name("web", "prod"), "web-prod");
        assert_eq!(unique_target_name("web", "prod", &[]), "web-prod");
    }

    /// Two four-tuples can collide on the pair. Overwriting either would drop a
    /// deploy target on the floor, so the second one gets an ugly name instead.
    #[test]
    fn a_colliding_target_name_gains_a_suffix() {
        let taken = vec![a_target("web-prod")];
        assert_eq!(unique_target_name("web", "prod", &taken), "web-prod-2");
        let taken = vec![a_target("web-prod"), a_target("web-prod-2")];
        assert_eq!(unique_target_name("web", "prod", &taken), "web-prod-3");
        // A different pair is untouched by either of them.
        assert_eq!(unique_target_name("api", "prod", &taken), "api-prod");
    }

    /// Generated names have to survive `validate_name`, or the migration would
    /// write a catalog that `build add` refuses to touch afterwards.
    #[test]
    fn generated_names_are_valid_names() {
        assert!(validate_name(&target_name("my-app", "kib-2")).is_ok());
        assert!(validate_name(&unique_target_name("web", "prod", &[a_target("web-prod")])).is_ok());
    }

    /// `use -` goes back one switch, and a repeated `use` of the same server
    /// is not a switch.
    #[test]
    fn binding_remembers_the_server_before() {
        let mut state = State::default();
        state.bind("web", "dev");
        assert!(state.previous.is_empty(), "the first binding has nothing before it");
        state.bind("web", "prod");
        assert_eq!(state.previous.get("web").map(String::as_str), Some("dev"));
        state.bind("web", "prod");
        assert_eq!(
            state.previous.get("web").map(String::as_str),
            Some("dev"),
            "the same server again is not a switch"
        );
    }

    /// Removing a server unbinds what pointed at it, now or before; removing
    /// an app takes it out of every part of the state.
    #[test]
    fn removed_entities_leave_the_state() {
        let mut state = State::default();
        state.bind("web", "dev");
        state.bind("web", "prod");
        state.bind("api", "prod");
        state.bind("admin", "dev");
        state.last = Some("web".into());
        assert_eq!(state.forget_server("prod"), vec!["api".to_string(), "web".to_string()]);
        assert_eq!(state.bindings.get("admin").map(String::as_str), Some("dev"));
        assert!(!state.bindings.contains_key("web"));
        assert_eq!(
            state.previous.get("web").map(String::as_str),
            Some("dev"),
            "a previous server that exists stays"
        );
        state.forget_server("dev");
        assert!(state.previous.is_empty());
        state.bind("web", "stage");
        state.forget_app("web");
        assert_eq!(state, State::default());
    }

    /// The fields `use -` added are optional on disk, so a state file written
    /// before them reads as it was.
    #[test]
    fn an_older_state_file_still_reads() {
        let state: State = serde_json::from_str(r#"{"bindings":{"web":"prod"}}"#).unwrap();
        assert_eq!(state.bindings.get("web").map(String::as_str), Some("prod"));
        assert!(state.previous.is_empty() && state.last.is_none());
        assert_eq!(serde_json::to_string(&state).unwrap(), r#"{"bindings":{"web":"prod"}}"#);
    }

    /// A column of auth kinds lines up: Display pads, `write_str` would not.
    #[test]
    fn auth_kinds_fill_their_column() {
        assert_eq!(format!("{:8}|", Auth::Key), "key     |");
    }

    #[test]
    fn auth_kinds_round_trip() {
        assert_eq!("password".parse::<Auth>().unwrap(), Auth::Password);
        assert_eq!("key".parse::<Auth>().unwrap(), Auth::Key);
        assert_eq!("agent".parse::<Auth>().unwrap(), Auth::Agent);
        assert_eq!(Auth::Key.to_string(), "key");
        assert_eq!(Auth::Agent.to_string(), "agent");
        // The round trip is what the catalog on disk relies on: `as_str` is
        // what gets written, `from_str` is what reads it back.
        for auth in [Auth::Password, Auth::Key, Auth::Agent] {
            assert_eq!(auth.as_str().parse::<Auth>().unwrap(), auth);
        }
        assert!("smartcard".parse::<Auth>().is_err());
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
