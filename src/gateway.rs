use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::{CONNECTION, CONTENT_LENGTH, COOKIE, HOST, LOCATION, SET_COOKIE, TRANSFER_ENCODING, UPGRADE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;

use crate::model::Server;
use crate::store;

/// Upper bound for a buffered request body (dev API calls and uploads).
const MAX_REQUEST_BODY: usize = 256 * 1024 * 1024;

/// Cookies issued by stands, keyed by (app, server) - the browser never sees them.
/// In-memory by design: restarting the gateway means logging in again (ADR 0002).
type Jars = Arc<Mutex<HashMap<(String, String), HashMap<String, String>>>>;
type Clients = Arc<Mutex<HashMap<String, reqwest::Client>>>;

#[derive(Clone)]
struct Ctx {
    app: String,
    port: u16,
    jars: Jars,
    clients: Clients,
}

/// Run listeners for every app with a gateway port, in the foreground.
pub fn run() -> Result<()> {
    let apps: Vec<(String, u16)> = store::load_apps()?
        .into_iter()
        .filter_map(|app| app.gateway_port.map(|port| (app.name, port)))
        .collect();
    if apps.is_empty() {
        bail!("no apps with a gateway port - set one with `turnout app edit NAME --port PORT`");
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async move {
        let jars: Jars = Arc::default();
        let clients: Clients = Arc::default();
        for (app, port) in apps {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
                .await
                .with_context(|| format!("cannot listen on 127.0.0.1:{port} (is another gateway running?)"))?;
            println!("{app}: listening on http://localhost:{port}");
            let ctx = Ctx {
                app,
                port,
                jars: jars.clone(),
                clients: clients.clone(),
            };
            let router = axum::Router::new().fallback(proxy).with_state(ctx);
            tokio::spawn(async move { axum::serve(listener, router).await });
        }
        tokio::signal::ctrl_c().await?;
        println!("Gateway stopped.");
        Ok(())
    })
}

async fn proxy(State(ctx): State<Ctx>, req: Request) -> Response {
    forward(ctx, req).await.unwrap_or_else(|err| {
        Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from(format!("turnout gateway: {err:#}\n")))
            .expect("static response")
    })
}

async fn forward(ctx: Ctx, req: Request) -> Result<Response> {
    // The binding is re-read per request: `turnout use` changes it with no IPC (ADR 0008).
    let state = store::load_state()?;
    let server_name = state
        .bindings
        .get(&ctx.app)
        .cloned()
        .ok_or_else(|| anyhow!("app '{0}' is not bound to a server - run `turnout use {0} SERVER`", ctx.app))?;
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow!("server '{server_name}' is gone from the catalog"))?;
    let client = client_for(&ctx.clients, &server)?;

    let path_query = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/").to_string();
    let target = format!("{}{}", server.url.trim_end_matches('/'), path_query);
    let method = req.method().clone();
    let mut headers = req.headers().clone();
    strip_hop_headers(&mut headers);
    headers.remove(HOST);
    // Only jar cookies travel to the stand; the browser's localhost cookies stay home.
    headers.remove(COOKIE);
    let jar_key = (ctx.app.clone(), server_name.clone());
    if let Some(cookie) = jar_cookie_header(&ctx.jars, &jar_key) {
        headers.insert(COOKIE, HeaderValue::from_str(&cookie).context("stored cookie is not a valid header value")?);
    }

    let body = axum::body::to_bytes(req.into_body(), MAX_REQUEST_BODY)
        .await
        .context("request body too large")?;
    let upstream = client
        .request(method, &target)
        .headers(headers)
        .body(body)
        .send()
        .await
        .map_err(|err| anyhow!("stand '{server_name}' is unreachable: {err}"))?;

    let status = upstream.status();
    let mut resp_headers = upstream.headers().clone();
    capture_cookies(&ctx.jars, &jar_key, &resp_headers);
    resp_headers.remove(SET_COOKIE);
    strip_hop_headers(&mut resp_headers);
    rewrite_location(&mut resp_headers, &server.url, ctx.port)?;

    let mut response = Response::builder().status(status);
    if let Some(headers_mut) = response.headers_mut() {
        *headers_mut = resp_headers;
    }
    Ok(response.body(Body::from_stream(upstream.bytes_stream()))?)
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

fn jar_cookie_header(jars: &Jars, key: &(String, String)) -> Option<String> {
    let jars = jars.lock().expect("cookie jar poisoned");
    let jar = jars.get(key)?;
    if jar.is_empty() {
        return None;
    }
    Some(jar.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("; "))
}

fn capture_cookies(jars: &Jars, key: &(String, String), headers: &HeaderMap) {
    let mut jars = jars.lock().expect("cookie jar poisoned");
    for value in headers.get_all(SET_COOKIE) {
        let Ok(text) = value.to_str() else { continue };
        let pair = text.split(';').next().unwrap_or_default();
        if let Some((name, value)) = pair.split_once('=') {
            jars.entry(key.clone()).or_default().insert(name.trim().to_string(), value.trim().to_string());
        }
    }
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

fn strip_hop_headers(headers: &mut HeaderMap) {
    for name in [CONNECTION, TRANSFER_ENCODING, UPGRADE, CONTENT_LENGTH] {
        headers.remove(&name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-authenticate");
    headers.remove("proxy-authorization");
    headers.remove("te");
    headers.remove("trailer");
}
