use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use axum::body::Body;
use axum::extract::ws::{Message as ClientMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{CONNECTION, CONTENT_LENGTH, COOKIE, HOST, LOCATION, SEC_WEBSOCKET_PROTOCOL, SET_COOKIE, TRANSFER_ENCODING, UPGRADE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as StandMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::control::{SessionCheck, Verdict};
use crate::jar::{Answer, Jar, Proof};
use crate::model::{App, Server};
use crate::signal::{Board, Kind, Seen, Signal};
use crate::store;

/// Upper bound for a buffered request body (dev API calls and uploads).
pub(crate) const MAX_REQUEST_BODY: usize = 256 * 1024 * 1024;

/// Cookies issued by stands, keyed by (app, server) - the browser never sees them.
/// In-memory by design: restarting the gateway means logging in again (ADR 0002).
pub(crate) type Jars = Arc<Mutex<HashMap<(String, String), Jar>>>;
pub(crate) type Clients = Arc<Mutex<HashMap<String, reqwest::Client>>>;

/// How long the session check waits for the stand. A proving request
/// answered within [`crate::jar::QUICK`] once; this is the margin for a stand
/// that is slower this morning, not for one that is gone.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct Ctx {
    app: String,
    port: u16,
    jars: Jars,
    clients: Clients,
    watch: Watch,
}

/// The gateway's eye on the stands: the board of signals, and where a change
/// on it goes - the gateway's own job record, which `status` reads, and the
/// desktop.
///
/// Both outlets are tasks of their own, fed through channels: a request only
/// updates the board and moves on, and the record is written in the order the
/// changes happened even when two requests change it at once.
#[derive(Clone)]
pub(crate) struct Watch {
    board: Arc<Mutex<Board>>,
    record: tokio::sync::watch::Sender<Vec<Signal>>,
    toasts: tokio::sync::mpsc::UnboundedSender<Signal>,
}

impl Watch {
    /// Start the outlets. `door` and `ports` are where the apps answer, for
    /// the toast that sends a lapsed session back to its login page.
    fn start(door: Option<u16>, ports: BTreeMap<u16, String>) -> Self {
        let (record, mut changed) = tokio::sync::watch::channel(Vec::<Signal>::new());
        tokio::spawn(async move {
            while changed.changed().await.is_ok() {
                let signals = changed.borrow_and_update().clone();
                let written = tokio::task::spawn_blocking(move || crate::registry::update_own(crate::registry::GATEWAY, |entry| entry.signals = signals)).await;
                if let Ok(Err(err)) = written {
                    eprintln!("turnout: cannot record the gateway's signals: {err:#}");
                }
            }
        });
        let (toasts, mut raised) = tokio::sync::mpsc::unbounded_channel::<Signal>();
        tokio::spawn(async move {
            while let Some(signal) = raised.recv().await {
                let ports = ports.clone();
                let shown = tokio::task::spawn_blocking(move || crate::notify::show(&toast_for(&signal, door, &ports))).await;
                if let Ok(Err(err)) = shown {
                    eprintln!("turnout: cannot show the notification: {err:#}");
                }
            }
        });
        Self {
            board: Arc::default(),
            record,
            toasts,
        }
    }

    /// Take in what one exchange between `app` and `server` showed.
    fn see(&self, app: &str, server: &str, seen: Seen) {
        let mut board = self.board.lock().expect("signal board poisoned");
        let change = board.observe(app, server, seen, crate::registry::now());
        self.publish(change, board.active());
    }

    /// The session of `app` on `server` got fresh cookies by hand.
    pub(crate) fn renewed(&self, app: &str, server: &str) {
        let mut board = self.board.lock().expect("signal board poisoned");
        let change = board.renewed(app, server);
        self.publish(change, board.active());
    }

    fn publish(&self, change: crate::signal::Change, active: &[Signal]) {
        if change.is_empty() {
            return;
        }
        // The gateway's output is its log: `turnout logs gateway` tells the
        // story of an afternoon's outages without anyone asking at the time.
        for signal in &change.raised {
            println!("{}", crate::signal::describe(signal));
        }
        for signal in &change.cleared {
            match signal.kind {
                Kind::StandDown => println!("'{}' answers again", signal.server),
                Kind::SessionExpired => println!("the '{}' session of '{}' is good again", signal.server, signal.app),
            }
        }
        self.record.send_replace(active.to_vec());
        for signal in change.toast {
            let _ = self.toasts.send(signal);
        }
    }
}

/// The notification a signal makes.
fn toast_for(signal: &Signal, door: Option<u16>, ports: &BTreeMap<u16, String>) -> crate::notify::Toast {
    match signal.kind {
        Kind::SessionExpired => {
            // The app's address behind the door, or its own port when the
            // door is shut: wherever the login page is reached through the
            // gateway, so the new session lands in the jar.
            let login = match door {
                Some(door) => crate::front::address(&signal.app, door),
                None => ports
                    .iter()
                    .find(|(_, app)| **app == signal.app)
                    .map(|(port, _)| format!("http://localhost:{port}"))
                    .unwrap_or_default(),
            };
            crate::notify::session_expired(&signal.app, &signal.server, &login)
        }
        Kind::StandDown => {
            let url = store::load_servers()
                .ok()
                .and_then(|servers| servers.into_iter().find(|server| server.name == signal.server))
                .map(|server| server.url)
                .unwrap_or_default();
            crate::notify::stand_down(&signal.server, signal.status, &signal.app, &url)
        }
    }
}

/// The port each app is served on, or why the gateway cannot start.
///
/// Shared by `gateway run` and `gateway start` so both refuse the same
/// catalogs with the same words. A port held by two apps used to collapse
/// silently into one map entry here; the second `bind` then failed inside
/// the detached child, after `start` had already reported success, and the
/// first sign was `stop` failing on a pid that was long gone.
pub fn listening_ports(apps: &[App]) -> Result<BTreeMap<u16, String>> {
    let mut ports = BTreeMap::new();
    for app in apps {
        let Some(port) = app.gateway_port else { continue };
        if let Some(other) = ports.insert(port, app.name.clone()) {
            bail!(
                "apps '{other}' and '{}' share gateway port {port} - give one of them another port with `turnout app edit NAME --port PORT`",
                app.name
            );
        }
    }
    if ports.is_empty() {
        bail!("no apps with a gateway port - set one with `turnout app edit NAME --port PORT`");
    }
    Ok(ports)
}

/// Run listeners for every app with a gateway port, in the foreground, plus
/// the front door on `front_port` (picked when `None`).
///
/// Once everything that is going to listen does, the gateway records itself
/// in the job registry - the same whether a terminal runs it or `gateway
/// start` does, with `log` naming the file `start` sends its output to - and
/// takes the record away again when it stops.
pub fn run(front_port: Option<u16>, log: Option<std::path::PathBuf>) -> Result<()> {
    let ports = listening_ports(&store::load_apps()?)?;
    let apps: Vec<(String, u16)> = ports.iter().map(|(port, app)| (app.clone(), *port)).collect();
    let front_port = front_port.or_else(crate::front::pick_port);
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let jars: Jars = Arc::default();
        let clients: Clients = Arc::default();
        let watch = Watch::start(front_port, ports.clone());
        for (app, port) in apps {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .with_context(|| format!("cannot listen on 127.0.0.1:{port} - the port is taken (a running gateway? stop it with `turnout gateway stop`)"))?;
            println!("{app}: listening on http://localhost:{port}");
            let ctx = Ctx {
                app,
                port,
                jars: jars.clone(),
                clients: clients.clone(),
                watch: watch.clone(),
            };
            let router = axum::Router::new().fallback(proxy).with_state(ctx);
            tokio::spawn(async move { axum::serve(listener, router).await });
        }
        // The door is the second thing to open and the one thing allowed to
        // fail: the stand proxies above are the daily flow, the door is on
        // top of it.
        let door = match front_port {
            Some(port) => match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => {
                    println!(
                        "front door: {} - apps answer at {}",
                        crate::front::door(port),
                        crate::front::address("NAME", port)
                    );
                    tokio::spawn(crate::front::serve(listener, port));
                    Some(port)
                }
                Err(err) => {
                    eprintln!("front door: cannot listen on 127.0.0.1:{port} ({err}) - apps are reachable by port only");
                    None
                }
            },
            None => {
                eprintln!(
                    "front door: ports {} and {} are both taken - apps are reachable by port only; set {} to open the door elsewhere",
                    crate::front::PORT,
                    crate::front::FALLBACK_PORT,
                    crate::front::ENV_PORT
                );
                None
            }
        };
        // Last to open, and never in the way: without it `cookie` has nobody
        // to talk to, and that is all.
        let control = match open_control().await {
            Ok((port, listener)) => {
                let token = crate::control::token();
                tokio::spawn(crate::control::serve(listener, token.clone(), jars.clone(), clients.clone(), watch.clone()));
                Some(crate::registry::Control { port, token })
            }
            Err(err) => {
                eprintln!("control channel: cannot listen on a loopback port ({err}) - `turnout cookie` will not reach this gateway");
                None
            }
        };
        // The door that actually opened, not the one that was asked for: the
        // record is what `open`, `status` and `dev` print addresses from.
        crate::commands::gateway::record(ports, door, control, log)?;
        tokio::signal::ctrl_c().await?;
        let _ = crate::registry::remove_own(crate::registry::GATEWAY);
        println!("Gateway stopped.");
        Ok(())
    })
}

/// A loopback port of the system's choosing for the control channel.
async fn open_control() -> std::io::Result<(u16, tokio::net::TcpListener)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    Ok((listener.local_addr()?.port(), listener))
}

async fn proxy(State(ctx): State<Ctx>, req: Request) -> Response {
    let result = if is_websocket_upgrade(req.headers()) {
        websocket_proxy(ctx, req).await
    } else {
        forward(ctx, req).await
    };
    result.unwrap_or_else(|err| {
        Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from(format!("turnout gateway: {err:#}\n")))
            .expect("static response")
    })
}

/// The binding is re-read per request: `turnout use` changes it with no IPC (ADR 0008).
fn resolve_binding(ctx: &Ctx) -> Result<Server> {
    let state = store::load_state()?;
    let server_name = state
        .bindings
        .get(&ctx.app)
        .cloned()
        .ok_or_else(|| anyhow!("app '{0}' is not bound to a server - run `turnout use {0} SERVER`", ctx.app))?;
    store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow!("server '{server_name}' is gone from the catalog"))
}

async fn forward(ctx: Ctx, req: Request) -> Result<Response> {
    let server = resolve_binding(&ctx)?;
    let server_name = server.name.clone();
    let client = client_for(&ctx.clients, &server)?;

    // The request is consumed either way, so its parts are moved, not cloned -
    // this is the hot path of the whole gateway.
    let (parts, body) = req.into_parts();
    let path_query = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/").to_string();
    let target = format!("{}{}", server.url.trim_end_matches('/'), path_query);
    let url = reqwest::Url::parse(&target).with_context(|| format!("'{target}' is not a URL"))?;
    let mut headers = parts.headers;
    strip_hop_headers(&mut headers);
    headers.remove(HOST);
    // Only jar cookies travel to the stand; the browser's localhost cookies stay home.
    headers.remove(COOKIE);
    let jar_key = (ctx.app.clone(), server_name.clone());
    let cookie = stored_cookie_header(&ctx.jars, &jar_key, &url)?;
    let sent_cookies = cookie.is_some();
    // A GET with cookies may turn out to prove the session; its headers are
    // what the check sends again, so they are kept as the app sent them.
    let get = parts.method == Method::GET;
    let asked = (get && sent_cookies).then(|| headers.clone());
    if let Some(cookie) = cookie {
        headers.insert(COOKIE, cookie);
    }

    let body = axum::body::to_bytes(body, MAX_REQUEST_BODY).await.context("request body too large")?;
    let started = Instant::now();
    let upstream = match client.request(parts.method, url.clone()).headers(headers).body(body).send().await {
        Ok(upstream) => upstream,
        Err(err) => {
            ctx.watch.see(&ctx.app, &server_name, Seen::Unreachable);
            bail!("stand '{server_name}' is unreachable: {err}");
        }
    };
    let took = started.elapsed();

    let status = upstream.status();
    let mut resp_headers = upstream.headers().clone();
    ctx.watch.see(
        &ctx.app,
        &server_name,
        Seen::Answered {
            status: status.as_u16(),
            sent_cookies,
            got_cookies: resp_headers.contains_key(SET_COOKIE),
        },
    );
    {
        let mut jars = ctx.jars.lock().expect("cookie jar poisoned");
        let jar = jars.entry(jar_key).or_default();
        let taken = take_cookies(jar, &url, &resp_headers);
        if sent_cookies {
            let at = crate::registry::now();
            jar.answered(Answer { status: status.as_u16(), at });
            if let Some(headers) = asked
                && crate::jar::proves(get, sent_cookies, status.as_u16(), taken.deleted, took)
            {
                jar.proved(Proof { path: path_query, headers, at });
            }
        }
    }
    resp_headers.remove(SET_COOKIE);
    strip_hop_headers(&mut resp_headers);
    rewrite_location(&mut resp_headers, &server.url, ctx.port)?;

    let mut response = Response::builder().status(status);
    if let Some(headers_mut) = response.headers_mut() {
        *headers_mut = resp_headers;
    }
    Ok(response.body(Body::from_stream(upstream.bytes_stream()))?)
}

pub(crate) fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Proxy a WebSocket: accept the browser's upgrade, open a matching connection
/// to the stand (jar cookies attached, TLS per server policy) and pump frames
/// both ways until either side closes.
async fn websocket_proxy(ctx: Ctx, req: Request) -> Result<Response> {
    let (mut parts, _body) = req.into_parts();
    let mut upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|err| anyhow!("invalid websocket upgrade: {err}"))?;
    let server = resolve_binding(&ctx)?;
    let server_name = server.name.clone();

    let path_query = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let ws_base = if let Some(rest) = server.url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = server.url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        bail!("server URL '{}' is neither http nor https", server.url);
    };
    let target = format!("{}{path_query}", ws_base.trim_end_matches('/'));

    let mut request = target.into_client_request().context("cannot build the upstream websocket request")?;
    let jar_key = (ctx.app.clone(), server_name.clone());
    // The upgrade is an HTTP request, and its cookies are matched as one: a
    // `ws://` address would leave out the HttpOnly and Secure ones.
    let handshake = format!("{}{path_query}", server.url.trim_end_matches('/'));
    let handshake = reqwest::Url::parse(&handshake).with_context(|| format!("'{handshake}' is not a URL"))?;
    let cookie = stored_cookie_header(&ctx.jars, &jar_key, &handshake)?;
    let sent_cookies = cookie.is_some();
    if let Some(cookie) = cookie {
        request.headers_mut().insert(COOKIE, cookie);
    }
    if let Some(protocol) = parts.headers.get(SEC_WEBSOCKET_PROTOCOL) {
        request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, protocol.clone());
        if let Ok(protocols) = protocol.to_str() {
            upgrade = upgrade.protocols(protocols.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>());
        }
    }
    // rustls throughout (the SSH transport moved to russh + rustls too), so the
    // gateway no longer pulls in a second, C-backed TLS stack. `None` uses
    // tungstenite's own rustls config with the platform roots; the accept-any
    // path is opt-in per server, exactly as before.
    let connector = if server.accept_invalid_certs {
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(accept_any_tls_config())))
    } else {
        None
    };

    let (app, watch) = (ctx.app.clone(), ctx.watch.clone());
    Ok(upgrade.on_upgrade(move |client| async move {
        match tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector).await {
            Ok((upstream, response)) => {
                let seen = Seen::Answered {
                    status: response.status().as_u16(),
                    sent_cookies,
                    got_cookies: false,
                };
                watch.see(&app, &server_name, seen);
                pump(client, upstream).await;
            }
            Err(err) => {
                // A stand that answered the upgrade with a status said
                // something; one that did not answer said nothing at all.
                let seen = match &err {
                    tokio_tungstenite::tungstenite::Error::Http(response) => Seen::Answered {
                        status: response.status().as_u16(),
                        sent_cookies,
                        got_cookies: response.headers().contains_key(SET_COOKIE),
                    },
                    _ => Seen::Unreachable,
                };
                watch.see(&app, &server_name, seen);
                eprintln!("turnout gateway: websocket to '{server_name}' failed: {err}");
            }
        }
    }))
}

/// A rustls client config that accepts any server certificate.
///
/// Used only when a server is marked `accept_invalid_certs` - the dev stands
/// this tool talks to serve self-signed certificates, and the alternative is
/// not connecting to them at all. It is deliberately confined to that opt-in
/// path; the default WebSocket connector verifies certificates normally.
fn accept_any_tls_config() -> rustls::ClientConfig {
    rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCert))
        .with_no_client_auth()
}

/// The verifier behind [`accept_any_tls_config`]: it approves every certificate
/// and signature. Scoped to opt-in `accept_invalid_certs` servers only.
#[derive(Debug)]
struct AcceptAnyServerCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        // The default aws-lc-rs provider's full set; anything it can verify, we
        // accept without checking.
        rustls::crypto::aws_lc_rs::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(crate) async fn pump<S>(client: WebSocket, upstream: tokio_tungstenite::WebSocketStream<S>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut client_tx, mut client_rx) = client.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let to_stand = async {
        while let Some(Ok(message)) = client_rx.next().await {
            if upstream_tx.send(client_to_stand(message)).await.is_err() {
                break;
            }
        }
        let _ = upstream_tx.close().await;
    };
    let to_client = async {
        while let Some(Ok(message)) = upstream_rx.next().await {
            let Some(message) = stand_to_client(message) else { continue };
            if client_tx.send(message).await.is_err() {
                break;
            }
        }
        let _ = client_tx.close().await;
    };
    tokio::join!(to_stand, to_client);
}

fn client_to_stand(message: ClientMessage) -> StandMessage {
    match message {
        ClientMessage::Text(text) => StandMessage::text(text.to_string()),
        ClientMessage::Binary(data) => StandMessage::binary(data.to_vec()),
        ClientMessage::Ping(data) => StandMessage::Ping(data.to_vec().into()),
        ClientMessage::Pong(data) => StandMessage::Pong(data.to_vec().into()),
        // The close code and reason travel through: 1008 from a stand is a
        // different diagnosis than 1000, and dropping them here would erase it.
        ClientMessage::Close(frame) => StandMessage::Close(frame.map(|f| tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: f.code.into(),
            reason: f.reason.as_str().into(),
        })),
    }
}

fn stand_to_client(message: StandMessage) -> Option<ClientMessage> {
    match message {
        StandMessage::Text(text) => Some(ClientMessage::Text(text.to_string().into())),
        StandMessage::Binary(data) => Some(ClientMessage::Binary(data.to_vec().into())),
        StandMessage::Ping(data) => Some(ClientMessage::Ping(data.to_vec().into())),
        StandMessage::Pong(data) => Some(ClientMessage::Pong(data.to_vec().into())),
        StandMessage::Close(frame) => Some(ClientMessage::Close(frame.map(|f| axum::extract::ws::CloseFrame {
            code: f.code.into(),
            reason: f.reason.as_str().into(),
        }))),
        StandMessage::Frame(_) => None,
    }
}

/// One HTTP client per server: no redirect following (the browser must see them),
/// TLS verification per the server's policy.
fn client_for(clients: &Clients, server: &Server) -> Result<reqwest::Client> {
    let mut cache = clients.lock().expect("client cache poisoned");
    if let Some(client) = cache.get(&server.name) {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(server.accept_invalid_certs)
        .build()
        .context("cannot build the HTTP client")?;
    cache.insert(server.name.clone(), client.clone());
    Ok(client)
}

/// The jar's cookies for a request to `url` as a ready `Cookie` header value,
/// when any of them goes there.
///
/// Shared by the HTTP and WebSocket proxies and the session check, so the
/// "stored cookie is not a valid header value" complaint has exactly one home.
fn stored_cookie_header(jars: &Jars, key: &(String, String), url: &reqwest::Url) -> Result<Option<HeaderValue>> {
    let cookie = jars.lock().expect("cookie jar poisoned").get(key).and_then(|jar| jar.header(url));
    cookie
        .map(|cookie| HeaderValue::from_str(&cookie).context("stored cookie is not a valid header value"))
        .transpose()
}

/// Put an answer's `Set-Cookie` headers into the jar, the way a browser would.
fn take_cookies(jar: &mut Jar, url: &reqwest::Url, headers: &HeaderMap) -> crate::jar::Taken {
    jar.take(url, headers.get_all(SET_COOKIE).iter().filter_map(|value| value.to_str().ok()))
}

/// Ask the stand whether the session of `app` is still good: send the request
/// that last proved it again, with the jar's cookies as they are now.
///
/// What the stand answers also goes where every answer goes - the signal
/// board and the jar - so a session the check finds expired says so in
/// `status` and on the desktop, exactly as if the app had run into it.
pub(crate) async fn check_session(jars: &Jars, clients: &Clients, watch: &Watch, app: &str, server_name: &str) -> Result<SessionCheck> {
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow!("server '{server_name}' is gone from the catalog"))?;
    let key = (app.to_string(), server_name.to_string());
    let (proof, empty) = {
        let jars = jars.lock().expect("cookie jar poisoned");
        let jar = jars.get(&key);
        (jar.and_then(Jar::proof).cloned(), jar.is_none_or(Jar::is_empty))
    };
    let mut check = SessionCheck {
        verdict: Verdict::Empty,
        status: None,
        path: None,
        proven: None,
    };
    if empty {
        return Ok(check);
    }
    let Some(proof) = proof else {
        check.verdict = Verdict::Unproven;
        return Ok(check);
    };
    check.path = Some(proof.path.clone());
    check.proven = Some(crate::output::iso8601(proof.at));
    let target = format!("{}{}", server.url.trim_end_matches('/'), proof.path);
    let url = reqwest::Url::parse(&target).with_context(|| format!("'{target}' is not a URL"))?;
    let Some(cookie) = stored_cookie_header(jars, &key, &url)? else {
        // Every cookie that request carried has run out since.
        check.verdict = Verdict::Expired;
        return Ok(check);
    };
    let mut headers = proof.headers;
    headers.insert(COOKIE, cookie);
    let client = client_for(clients, &server)?;
    let answer = match client.get(url.clone()).headers(headers).timeout(CHECK_TIMEOUT).send().await {
        Ok(answer) => answer,
        Err(_) => {
            watch.see(app, server_name, Seen::Unreachable);
            check.verdict = Verdict::Unreachable;
            return Ok(check);
        }
    };
    // Only the status is wanted; the body is dropped unread with the answer.
    let status = answer.status().as_u16();
    watch.see(
        app,
        server_name,
        Seen::Answered {
            status,
            sent_cookies: true,
            got_cookies: answer.headers().contains_key(SET_COOKIE),
        },
    );
    {
        let mut jars = jars.lock().expect("cookie jar poisoned");
        let jar = jars.entry(key).or_default();
        take_cookies(jar, &url, answer.headers());
        jar.answered(Answer {
            status,
            at: crate::registry::now(),
        });
    }
    check.status = Some(status);
    check.verdict = crate::control::verdict(status);
    Ok(check)
}

/// Absolute redirects to the stand come back as localhost so the browser never leaves.
fn rewrite_location(headers: &mut HeaderMap, server_url: &str, port: u16) -> Result<()> {
    let Some(location) = headers.get(LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string) else {
        return Ok(());
    };
    let base = server_url.trim_end_matches('/');
    if let Some(rest) = location.strip_prefix(base) {
        let rewritten = format!("http://localhost:{port}{rest}");
        headers.insert(
            LOCATION,
            HeaderValue::from_str(&rewritten).context("rewritten location is not a valid header value")?,
        );
    }
    Ok(())
}

/// Drop headers that describe the hop, not the message.
///
/// `CONTENT_LENGTH` is in the list because both directions re-buffer the body
/// (the request into bytes, the response into a stream), so the original
/// length may no longer hold - reqwest and axum recompute it for the body they
/// actually send.
pub(crate) fn strip_hop_headers(headers: &mut HeaderMap) {
    for name in [CONNECTION, TRANSFER_ENCODING, UPGRADE, CONTENT_LENGTH] {
        headers.remove(&name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-authenticate");
    headers.remove("proxy-authorization");
    headers.remove("te");
    headers.remove("trailer");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, port: Option<u16>) -> App {
        App {
            name: name.to_string(),
            path: String::new(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: port,
            gateway_env: None,
            env_file: None,
            dev_port: None,
            servers: Vec::new(),
        }
    }

    #[test]
    fn listening_ports_maps_each_port_to_its_app() {
        let ports = listening_ports(&[app("web", Some(7001)), app("api", None), app("admin", Some(7002))]).unwrap();
        assert_eq!(ports.get(&7001).map(String::as_str), Some("web"));
        assert_eq!(ports.get(&7002).map(String::as_str), Some("admin"));
        assert_eq!(ports.len(), 2, "an app without a port got a listener");
    }

    #[test]
    fn listening_ports_refuses_a_port_two_apps_share() {
        let error = listening_ports(&[app("web", Some(7001)), app("admin", Some(7001))]).unwrap_err().to_string();
        assert!(error.contains("'web' and 'admin' share gateway port 7001"), "{error}");
        assert!(error.contains("turnout app edit NAME --port PORT"), "{error}");
    }

    #[test]
    fn listening_ports_needs_at_least_one_port() {
        let error = listening_ports(&[app("web", None)]).unwrap_err().to_string();
        assert!(error.contains("no apps with a gateway port"), "{error}");
    }
}
