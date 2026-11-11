//! Tunnels: a port on this machine that reaches a port the server reaches.
//!
//! `turnout tunnel db@staging 5432` makes staging's database answer at
//! `localhost:5432` here, for whatever client asks - psql, a GUI, an app's own
//! `DATABASE_URL` - without opening the database to the network and without a
//! terminal kept open on `ssh -L`.
//!
//! A tunnel is a job like the gateway (see [`crate::registry`]): a turnout
//! process of its own, `tunnel run`, in the background. It listens here,
//! signs in to the server the way the credential says - a key, the keyring's
//! password, the agent, the host key checked - and carries each connection it
//! accepts over an SSH channel (`direct-tcpip`) to the address on the far
//! side. Not the system `ssh -L`: a process in the background has nobody to
//! type a password, and turnout's own SSH already knows every way a
//! credential signs in (ADR 0034).
//!
//! One SSH connection carries all the tunnel's connections, and it is
//! expected to drop: a laptop sleeps, a server reboots, a router forgets.
//! Keepalives notice ([`crate::ssh::Forwarder`]); the tunnel says so in its
//! record, raises one notification and connects again, soon at first and then
//! less often. A client that arrives meanwhile gets a fresh connection or a
//! closed socket - never one that hangs on a connection that is gone.

use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};

use crate::registry::{self, Entry, Work};
use crate::ssh::{Forwarder, OpenFailure};

/// Where a tunnel goes: what its record says, and what `tunnel run` is told.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// What `tunnel list` and `tunnel stop` know it by.
    pub name: String,
    /// The server in the catalog it goes through.
    pub server: String,
    /// The credential it signs in with.
    pub credential: String,
    /// The port it listens on here, on the loopback.
    pub local_port: u16,
    /// What the server connects to; `localhost` is the server itself.
    pub host: String,
    pub port: u16,
}

impl Route {
    /// Where clients connect: `localhost:5432`.
    pub fn local(&self) -> String {
        format!("localhost:{}", self.local_port)
    }

    /// What the tunnel reaches: `staging:5432` on the server itself,
    /// `db.internal:5432 via staging` beyond it.
    pub fn remote(&self) -> String {
        if is_loopback(&self.host) {
            format!("{}:{}", self.server, self.port)
        } else {
            format!("{} via {}", address(&self.host, self.port), self.server)
        }
    }

    /// `localhost:5432 -> staging:5432`.
    pub fn describe(&self) -> String {
        format!("{} -> {}", self.local(), self.remote())
    }

    /// The address the server is asked to connect to: `localhost:5432`.
    fn far_end(&self) -> String {
        address(&self.host, self.port)
    }
}

/// Whether `host` names the server's own loopback.
fn is_loopback(host: &str) -> bool {
    matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
}

/// `host:port`, with an IPv6 address in brackets.
fn address(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// The ports a tunnel joins: the one it listens on here, and the address the
/// server connects to.
#[derive(Debug, PartialEq, Eq)]
pub struct Ports {
    pub local: u16,
    pub host: String,
    pub port: u16,
}

/// The host the server connects to when none is named: the server itself.
pub const DEFAULT_HOST: &str = "localhost";

/// Read `PORT`, `LOCAL:PORT` or `LOCAL:HOST:PORT` - the shape `ssh -L` takes,
/// without its bind address: a tunnel listens on the loopback only.
pub fn parse_ports(spec: &str) -> Result<Ports> {
    let spec = spec.trim();
    let bad = || anyhow!("'{spec}' is not a port to forward - give PORT, LOCAL:PORT or LOCAL:HOST:PORT, e.g. 5432, 15432:5432 or 15432:db.internal:5432");
    let (local, rest) = match spec.split_once(':') {
        Some((local, rest)) => (Some(local), rest),
        None => (None, spec),
    };
    let (host, port) = match (local, rest.rsplit_once(':')) {
        (Some(_), Some((host, port))) => (Some(host), port),
        _ => (None, rest),
    };
    let port = parse_port(port).ok_or_else(bad)?;
    let local = match local {
        Some(local) => parse_port(local).ok_or_else(bad)?,
        None => port,
    };
    let host = match host {
        Some(host) => {
            let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
            if host.is_empty() || host.contains(|c: char| c.is_whitespace() || matches!(c, '[' | ']' | '/' | '@')) {
                return Err(bad());
            }
            host.to_string()
        }
        None => DEFAULT_HOST.to_string(),
    };
    Ok(Ports { local, host, port })
}

/// A port number, 1 to 65535.
fn parse_port(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|port| *port != 0)
}

/// Read `NAME@SERVER` or `SERVER`: the name a tunnel goes by, when given, and
/// the server or target it goes through.
pub fn parse_route(spec: &str) -> Result<(Option<String>, String)> {
    let spec = spec.trim();
    let (name, server) = match spec.split_once('@') {
        Some((name, server)) => (Some(name), server),
        None => (None, spec),
    };
    if server.is_empty() || server.contains('@') {
        bail!("'{spec}' does not name a server - give NAME@SERVER, or SERVER alone");
    }
    if let Some(name) = name {
        crate::model::validate_name(name).map_err(|_| anyhow!("invalid tunnel name '{name}': use lowercase letters, digits and dashes"))?;
    }
    Ok((name.map(str::to_string), server.to_string()))
}

/// The name a tunnel gets when nobody gave one: `SERVER-PORT`, from the name
/// typed and the far end's port - so two tunnels to one server differ.
pub fn default_name(server: &str, port: u16) -> String {
    format!("{server}-{port}")
}

/// How long signing in may take before it counts as a failure. A server that
/// does not answer otherwise holds a connection attempt for as long as the
/// system's TCP gives it - minutes on Linux.
const CONNECT_PATIENCE: Duration = Duration::from_secs(20);

/// How long the server gets to say whether it reached the far end. Past it
/// the SSH connection is taken for one that died without a word.
const OPEN_PATIENCE: Duration = Duration::from_secs(10);

/// The longest pause between two attempts to connect again.
const LONGEST_PAUSE: Duration = Duration::from_secs(30);

/// `tunnel run`: the tunnel itself, until Ctrl+C or `stop` ends it.
///
/// Listens first, signs in second, asks the server once whether it reaches the
/// far end, and only then writes its record - so a record is a tunnel that
/// works, the way the gateway's is a gateway that listens. `log` names the
/// file opening a tunnel sends this process's output to, and marks it as one
/// in the background.
pub fn run(route: Route, log: Option<PathBuf>) -> Result<()> {
    crate::model::validate_name(&route.name).map_err(|_| anyhow!("invalid tunnel name '{}'", route.name))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("cannot start the tunnel's runtime")?;
    runtime.block_on(serve(route, log))
}

async fn serve(route: Route, log: Option<PathBuf>) -> Result<()> {
    let listeners = listen(&route).await?;
    let link = Arc::new(Link::new(route.clone()));
    let forwarder = link.get().await?;
    let origin = listeners[0].local_addr().context("cannot read the tunnel's own address")?;
    check_far_end(&forwarder, &route, origin).await?;

    let mut entry = Entry::own(Work::Tunnel(route.clone()), log.is_some(), log.clone());
    entry.ready = Some(registry::Ready {
        at: registry::now(),
        url: None,
    });
    registry::claim(&entry)?;
    let bound: Vec<String> = listeners
        .iter()
        .filter_map(|listener| listener.local_addr().ok())
        .map(|address| address.to_string())
        .collect();
    say(&format!(
        "tunnel {}: {} - listening on {}, signed in to {} as {}",
        route.name,
        route.describe(),
        bound.join(" and "),
        route.server,
        route.credential
    ));

    for listener in listeners {
        tokio::spawn(accept(listener, link.clone()));
    }
    tokio::spawn(watch(link.clone(), log));
    // Ctrl+C is the tunnel's to handle: the record goes with it.
    crate::term::hand_over_interrupts();
    tokio::signal::ctrl_c().await.context("cannot wait for Ctrl+C")?;
    let _ = registry::remove_own(&registry::tunnel_key(&route.name));
    say(&format!("tunnel {} closed", route.name));
    Ok(())
}

/// The loopback listeners: `127.0.0.1`, and `::1` where the machine has it.
///
/// Both, because `localhost` is either: Node and the JVM may try `::1` first,
/// psql tries both, and a tunnel on one family alone would be "connection
/// refused" for some clients. The port must be nobody's on either - a
/// listener on `0.0.0.0` holds it too, and on Windows a bind to `127.0.0.1`
/// would quietly succeed beside it and take its connections.
async fn listen(route: &Route) -> Result<Vec<TcpListener>> {
    let port = route.local_port;
    if let Some(holder) = crate::ports::holder(port) {
        bail!("{}", taken(route, &holder.describe()));
    }
    let v4 = TcpListener::bind(("127.0.0.1", port)).await.map_err(|err| bind_error(route, err))?;
    let mut listeners = vec![v4];
    match TcpListener::bind(("::1", port)).await {
        Ok(v6) => listeners.push(v6),
        // Somebody's on the other half of `localhost`.
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => bail!("{}", taken(route, "another process, on ::1")),
        // No IPv6 loopback on this machine: `localhost` is `127.0.0.1` alone.
        Err(_) => {}
    }
    Ok(listeners)
}

/// The refusal of a local port somebody else holds, with the way out.
fn taken(route: &Route, holder: &str) -> String {
    format!(
        "port {port} is already in use here by {holder} - listen on another one: `turnout tunnel {name}@{server} {other}:{far}`",
        port = route.local_port,
        name = route.name,
        server = route.server,
        other = suggested_port(route.local_port),
        far = if is_loopback(&route.host) {
            route.port.to_string()
        } else {
            format!("{}:{}", route.host, route.port)
        }
    )
}

/// A port to suggest instead of a taken one: the same digits behind a 1, the
/// way people pick a second Postgres port, when that is still a port.
fn suggested_port(port: u16) -> u16 {
    let shifted = 10_000 + u32::from(port);
    u16::try_from(shifted).ok().filter(|_| port < 10_000).unwrap_or(port.saturating_add(1))
}

fn bind_error(route: &Route, err: std::io::Error) -> anyhow::Error {
    let port = route.local_port;
    if err.kind() == std::io::ErrorKind::PermissionDenied && port < 1024 {
        return anyhow!(
            "cannot listen on 127.0.0.1:{port}: ports below 1024 need administrator rights here - listen on another one: `turnout tunnel {}@{} {}:{}`",
            route.name,
            route.server,
            suggested_port(port).max(1024),
            route.port
        );
    }
    anyhow!("cannot listen on 127.0.0.1:{port}: {err}")
}

/// Ask the server once whether it reaches the far end.
///
/// A typo in the port, a service that is not running, an sshd that forwards
/// nothing: each would otherwise show as the first client's dropped
/// connection, long after the tunnel said it was open.
async fn check_far_end(forwarder: &Forwarder, route: &Route, origin: SocketAddr) -> Result<()> {
    match tokio::time::timeout(OPEN_PATIENCE, forwarder.open(&route.host, route.port, origin)).await {
        Ok(Ok(mut stream)) => {
            use tokio::io::AsyncWriteExt;
            let _ = stream.shutdown().await;
            Ok(())
        }
        Ok(Err(failure)) => Err(anyhow!("{}", refusal(route, &failure))),
        Err(_) => bail!(
            "'{}' did not say within {}s whether it reaches {}",
            route.server,
            OPEN_PATIENCE.as_secs(),
            route.far_end()
        ),
    }
}

/// Why the server did not carry a connection, with what to do about it.
fn refusal(route: &Route, failure: &OpenFailure) -> String {
    let server = &route.server;
    let far = route.far_end();
    match failure {
        OpenFailure::Unreachable if is_loopback(&route.host) => {
            format!(
                "'{server}' cannot connect to {far}: nothing listens on port {} there - is the service running?",
                route.port
            )
        }
        OpenFailure::Unreachable => format!(
            "'{server}' cannot connect to {far}: nothing listens there, or '{server}' cannot resolve '{}'",
            route.host
        ),
        OpenFailure::Prohibited => {
            format!("sshd on '{server}' does not forward ports for this account - AllowTcpForwarding in its sshd_config (or a Match block) has to allow it")
        }
        OpenFailure::Refused(reason) => format!("'{server}' refused to connect to {far}: {reason}"),
        OpenFailure::Lost(reason) => format!("the connection to '{server}' dropped: {reason}"),
    }
}

/// The tunnel's SSH connection: the one there is, or a new one when it has
/// gone. One for every connection the tunnel carries.
struct Link {
    route: Route,
    current: tokio::sync::Mutex<Option<Arc<Forwarder>>>,
}

impl Link {
    fn new(route: Route) -> Self {
        Self {
            route,
            current: tokio::sync::Mutex::new(None),
        }
    }

    /// The live connection, connecting when there is none.
    ///
    /// Under the lock, connecting included: ten clients that arrive while the
    /// connection is down wait for one new connection, not ten.
    async fn get(&self) -> Result<Arc<Forwarder>> {
        let mut current = self.current.lock().await;
        if let Some(live) = current.as_ref().filter(|forwarder| !forwarder.is_closed()) {
            return Ok(live.clone());
        }
        let fresh = Arc::new(self.connect().await?);
        *current = Some(fresh.clone());
        Ok(fresh)
    }

    /// Whether there is a connection, and it is up.
    async fn is_up(&self) -> bool {
        self.current.lock().await.as_ref().is_some_and(|forwarder| !forwarder.is_closed())
    }

    /// Sign in again, with the catalogs as they are now: a server moved to
    /// another address, a password changed with `pass set`, are what the next
    /// connection uses.
    async fn connect(&self) -> Result<Forwarder> {
        let route = self.route.clone();
        let (server, credential) = tokio::task::spawn_blocking(move || catalog(&route))
            .await
            .context("cannot read the catalogs")??;
        let label = format!("{}:{}", server.ssh_host(), server.port);
        match tokio::time::timeout(CONNECT_PATIENCE, Forwarder::connect(&server, &credential)).await {
            Ok(connected) => connected,
            Err(_) => bail!("{label} did not answer within {}s", CONNECT_PATIENCE.as_secs()),
        }
    }

    /// Forget `dead` - a connection a client found not answering - unless a
    /// newer one has taken its place already.
    async fn forget(&self, dead: &Arc<Forwarder>) {
        {
            let mut current = self.current.lock().await;
            if current.as_ref().is_some_and(|forwarder| Arc::ptr_eq(forwarder, dead)) {
                *current = None;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(1), dead.close()).await;
    }
}

/// The server and the credential the route names, from the catalogs.
fn catalog(route: &Route) -> Result<(crate::model::Server, crate::model::Credential)> {
    let server = crate::store::load_servers()?
        .into_iter()
        .find(|server| server.name == route.server)
        .ok_or_else(|| anyhow!("no server named '{}' any more - see `turnout server list`", route.server))?;
    let credential = crate::store::load_credentials()?
        .into_iter()
        .find(|credential| credential.name == route.credential)
        .ok_or_else(|| anyhow!("no credential named '{}' any more - see `turnout credential list`", route.credential))?;
    Ok((server, credential))
}

/// Take the connections one listener gets, each on a task of its own.
async fn accept(listener: TcpListener, link: Arc<Link>) {
    loop {
        match listener.accept().await {
            Ok((socket, peer)) => {
                let link = link.clone();
                tokio::spawn(async move { carry(&link, socket, peer).await });
            }
            // Out of descriptors, mostly: a pause, not the end of the tunnel.
            Err(err) => {
                say(&format!("cannot accept a connection: {err}"));
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

/// Carry one local connection to the far end and back.
///
/// Twice at most: a connection that turns out dead when asked - the laptop
/// slept, and the keepalives have not told yet - is forgotten, and the client
/// is carried over a new one instead of being dropped.
async fn carry(link: &Link, mut socket: TcpStream, peer: SocketAddr) {
    let _ = socket.set_nodelay(true);
    for attempt in 0..2 {
        let forwarder = match link.get().await {
            Ok(forwarder) => forwarder,
            Err(err) => {
                say(&format!("{peer}: not carried - cannot sign in to '{}': {err:#}", link.route.server));
                return;
            }
        };
        match tokio::time::timeout(OPEN_PATIENCE, forwarder.open(&link.route.host, link.route.port, peer)).await {
            Ok(Ok(mut stream)) => {
                let _ = tokio::io::copy_bidirectional(&mut socket, &mut stream).await;
                return;
            }
            Ok(Err(OpenFailure::Lost(_))) | Err(_) if attempt == 0 => link.forget(&forwarder).await,
            Ok(Err(failure)) => {
                say(&format!("{peer}: not carried - {}", refusal(&link.route, &failure)));
                return;
            }
            Err(_) => {
                say(&format!("{peer}: not carried - '{}' does not answer", link.route.server));
                link.forget(&forwarder).await;
                return;
            }
        }
    }
}

/// Keep the connection up while nobody uses it, and keep the record honest
/// about it.
///
/// Once a second it looks; a connection that has gone is replaced at once,
/// then after a pause that doubles up to [`LONGEST_PAUSE`]. The first failure
/// writes the trouble into the record and raises a notification - one per
/// [`crate::signal::RETOAST`], so a connection that flaps does not bury the
/// desktop - and the next success takes the trouble away.
async fn watch(link: Arc<Link>, log: Option<PathBuf>) {
    let key = registry::tunnel_key(&link.route.name);
    let mut down: Option<String> = None;
    let mut pause = Duration::from_secs(1);
    let mut toasted: Option<u64> = None;
    loop {
        tokio::time::sleep(if down.is_some() { pause } else { Duration::from_secs(1) }).await;
        if link.is_up().await {
            continue;
        }
        match link.get().await {
            Ok(_) => {
                if down.take().is_some() {
                    say(&format!("tunnel {}: signed in to '{}' again", link.route.name, link.route.server));
                    set_trouble(&key, None).await;
                } else {
                    say(&format!(
                        "tunnel {}: the connection to '{}' dropped - signed in again",
                        link.route.name, link.route.server
                    ));
                }
                pause = Duration::from_secs(1);
            }
            Err(err) => {
                let reason = format!("{err:#}");
                if down.as_deref() != Some(reason.as_str()) {
                    say(&format!(
                        "tunnel {}: the connection to '{}' is down - {reason}",
                        link.route.name, link.route.server
                    ));
                    set_trouble(&key, Some(reason.clone())).await;
                }
                let now = registry::now();
                if down.is_none() && toasted.is_none_or(|at| now.saturating_sub(at) >= crate::signal::RETOAST) {
                    toasted = Some(now);
                    let toast = crate::notify::tunnel_down(&link.route, &reason, log.as_deref());
                    let shown = tokio::task::spawn_blocking(move || crate::notify::show(&toast)).await;
                    if let Ok(Err(err)) = shown {
                        say(&format!("cannot show the notification: {err:#}"));
                    }
                }
                down = Some(reason);
                pause = (pause * 2).min(LONGEST_PAUSE);
            }
        }
    }
}

/// Write the tunnel's trouble into its record, or take it away.
async fn set_trouble(key: &str, reason: Option<String>) {
    let key = key.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        registry::update_own(&key, |entry| {
            entry.trouble = reason.map(|reason| registry::Trouble {
                since: entry.trouble.as_ref().map_or_else(registry::now, |trouble| trouble.since),
                reason,
            });
        })
    })
    .await;
}

/// A line in the tunnel's log - its stdout - stamped with the moment, since a
/// tunnel's log spans days. A log that cannot be written is not worth the
/// tunnel: `println!` would panic on it.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{} {line}", crate::output::iso8601(registry::now()));
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports(spec: &str) -> Ports {
        parse_ports(spec).unwrap_or_else(|err| panic!("{spec}: {err}"))
    }

    /// The three shapes `ssh -L` users type, an IPv6 host in brackets, and
    /// what is not a port.
    #[test]
    fn ports_read_the_way_ssh_reads_them() {
        assert_eq!(
            ports("5432"),
            Ports {
                local: 5432,
                host: "localhost".into(),
                port: 5432
            }
        );
        assert_eq!(
            ports("15432:5432"),
            Ports {
                local: 15432,
                host: "localhost".into(),
                port: 5432
            }
        );
        assert_eq!(
            ports("15432:db.internal:5432"),
            Ports {
                local: 15432,
                host: "db.internal".into(),
                port: 5432
            }
        );
        assert_eq!(
            ports("6380:[fd00::7]:6379"),
            Ports {
                local: 6380,
                host: "fd00::7".into(),
                port: 6379
            }
        );
        for bad in [
            "",
            "0",
            "70000",
            "db",
            "5432:",
            ":5432",
            "1:2:3:x",
            "1::5432",
            "1:a b:5432",
            "1:user@db:5432",
            "0:5432",
        ] {
            let err = parse_ports(bad).expect_err(bad).to_string();
            assert!(err.contains("LOCAL:HOST:PORT"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_route_is_a_name_at_a_server_or_a_server_alone() {
        assert_eq!(parse_route("db@staging").unwrap(), (Some("db".into()), "staging".into()));
        assert_eq!(parse_route("staging").unwrap(), (None, "staging".into()));
        assert!(parse_route("db@").is_err());
        assert!(parse_route("a@b@c").is_err());
        let err = parse_route("Db@staging").unwrap_err().to_string();
        assert!(err.contains("invalid tunnel name 'Db'"), "{err}");
        assert_eq!(default_name("staging", 5432), "staging-5432");
    }

    fn route(host: &str) -> Route {
        Route {
            name: "db".into(),
            server: "staging".into(),
            credential: "deploy".into(),
            local_port: 15432,
            host: host.into(),
            port: 5432,
        }
    }

    /// The far end reads as the server's own port when it is the server, and
    /// as an address beyond it otherwise.
    #[test]
    fn a_route_says_where_it_goes() {
        assert_eq!(route("localhost").describe(), "localhost:15432 -> staging:5432");
        assert_eq!(route("127.0.0.1").remote(), "staging:5432");
        assert_eq!(route("db.internal").describe(), "localhost:15432 -> db.internal:5432 via staging");
        assert_eq!(route("fd00::7").remote(), "[fd00::7]:5432 via staging");
    }

    /// A taken port is refused with a command that would work, the far end
    /// kept as it was typed.
    #[test]
    fn a_taken_port_suggests_another() {
        assert_eq!(suggested_port(5432), 15432);
        assert_eq!(suggested_port(15432), 15433);
        assert_eq!(suggested_port(65535), 65535);
        let near = taken(&route("localhost"), "postgres.exe (pid 812)");
        assert!(near.contains("postgres.exe (pid 812)"), "{near}");
        assert!(near.contains("`turnout tunnel db@staging 15433:5432`"), "{near}");
        let far = taken(&route("db.internal"), "x");
        assert!(far.contains("15433:db.internal:5432`"), "{far}");
    }

    /// Each refusal names its cause and its cure.
    #[test]
    fn a_refusal_says_what_to_do() {
        let nothing = refusal(&route("localhost"), &OpenFailure::Unreachable);
        assert!(
            nothing.contains("nothing listens on port 5432") && nothing.contains("service running"),
            "{nothing}"
        );
        let unresolved = refusal(&route("db.internal"), &OpenFailure::Unreachable);
        assert!(unresolved.contains("cannot resolve 'db.internal'"), "{unresolved}");
        let prohibited = refusal(&route("localhost"), &OpenFailure::Prohibited);
        assert!(prohibited.contains("AllowTcpForwarding"), "{prohibited}");
    }

    /// A record of a tunnel reads back as written, and its key is its own.
    #[test]
    fn a_tunnel_record_reads_back() {
        let entry = Entry::own(Work::Tunnel(route("localhost")), true, None);
        assert_eq!(entry.key(), "tunnel@db");
        assert_eq!(entry.title(), "tunnel db");
        let text = serde_json::to_string(&entry).unwrap();
        let back: Entry = serde_json::from_str(&text).unwrap();
        assert_eq!(back.tunnel(), Some(&route("localhost")));
    }
}
