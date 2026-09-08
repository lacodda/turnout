//! The front door: one address per app.
//!
//! Dev servers take their port in the order they start - Vite hands out 5173,
//! then 5174 - so "which app is on which port" is a fresh question every
//! morning. The front door answers it by name instead: the gateway listens on
//! one well-known port and routes by host, `myapp.localhost` to whatever dev
//! server turnout started for `myapp`. Every name under `.localhost` resolves
//! to loopback (RFC 6761), so the address works without a hosts file, and the
//! port behind it is turnout's business.
//!
//! The door is port 80 when the machine hands it out (Windows and macOS do,
//! Linux asks for privileges) and 7000 otherwise; `TURNOUT_FRONT_PORT` pins
//! one. A door that cannot open is a warning, not the end of the gateway: the
//! per-app stand proxies still work, and that is the daily flow.

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{COOKIE, HOST, LOCATION, SEC_WEBSOCKET_PROTOCOL};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use crate::model::App;
use crate::store;

/// The port the door opens on when it can.
pub const PORT: u16 = 80;
/// Where it opens when 80 is taken or privileged.
pub const FALLBACK_PORT: u16 = 7000;
/// Pins the door to one port; `0` keeps it shut.
pub const ENV_PORT: &str = "TURNOUT_FRONT_PORT";

/// The port the door should open on: pinned, or the first of 80 and 7000
/// that binds. `None` means no door - pinned shut, or both ports taken.
pub fn pick_port() -> Option<u16> {
    if let Some(pinned) = std::env::var(ENV_PORT).ok().and_then(|value| value.trim().parse::<u16>().ok()) {
        return (pinned != 0).then_some(pinned);
    }
    [PORT, FALLBACK_PORT]
        .into_iter()
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

/// The door itself: `http://localhost`, with the port when it is not 80.
pub fn door(front_port: u16) -> String {
    if front_port == PORT {
        "http://localhost".to_string()
    } else {
        format!("http://localhost:{front_port}")
    }
}

/// The app's address behind a door on `front_port`.
pub fn address(app: &str, front_port: u16) -> String {
    if front_port == PORT {
        format!("http://{app}.localhost")
    } else {
        format!("http://{app}.localhost:{front_port}")
    }
}

/// The app a `Host` header names: `myapp.localhost[:port]` gives `myapp`.
pub fn app_of_host(host: &str) -> Option<&str> {
    let host = host
        .rsplit_once(':')
        .map_or(host, |(name, port)| if port.chars().all(|c| c.is_ascii_digit()) { name } else { host });
    let name = host.strip_suffix(".localhost")?;
    (!name.is_empty() && !name.contains('.')).then_some(name)
}

/// Serve the door on `listener` until the gateway stops.
pub async fn serve(listener: tokio::net::TcpListener, front_port: u16) {
    let router = axum::Router::new().fallback(route).with_state(front_port);
    let _ = axum::serve(listener, router).await;
}

async fn route(State(front_port): State<u16>, req: Request) -> Response {
    match dispatch(front_port, req).await {
        Ok(response) => response,
        Err(err) => page(StatusCode::BAD_GATEWAY, &format!("turnout front door: {err:#}")),
    }
}

async fn dispatch(front_port: u16, req: Request) -> Result<Response> {
    let host = req.headers().get(HOST).and_then(|value| value.to_str().ok()).unwrap_or_default();
    let Some(name) = app_of_host(host) else {
        return Ok(page(
            StatusCode::NOT_FOUND,
            &format!("turnout front door: '{host}' names no app - apps answer at http://NAME.localhost, see `turnout app list`"),
        ));
    };
    let apps = store::load_apps()?;
    let Some(app) = apps.iter().find(|app| app.name == name) else {
        return Ok(page(StatusCode::NOT_FOUND, &format!("turnout: no app named '{name}' - see `turnout app list`")));
    };
    let Some(dev_port) = app.dev_port else {
        return Ok(not_running(app));
    };
    if crate::gateway::is_websocket_upgrade(req.headers()) {
        return websocket(app, dev_port, req).await;
    }
    match forward(app, dev_port, front_port, req).await {
        Ok(response) => Ok(response),
        Err(err) if err.downcast_ref::<reqwest::Error>().is_some_and(reqwest::Error::is_connect) => Ok(not_running(app)),
        Err(err) => Err(err),
    }
}

/// The page for a name whose dev server is not answering: what to run, not
/// a reset connection.
fn not_running(app: &App) -> Response {
    page(
        StatusCode::SERVICE_UNAVAILABLE,
        &format!("'{0}' is not running - start it with `turnout dev {0}`", app.name),
    )
}

fn page(status: StatusCode, text: &str) -> Response {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>turnout</title>\
         <style>body{{font:16px/1.5 system-ui,sans-serif;margin:3rem auto;max-width:40rem;color:#1b2126}}code{{background:#eee;padding:.1em .3em}}</style>\
         <p>{}</p>",
        html_escape(text)
    );
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("static response")
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for c in text.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '`' => {
                out.push_str(if in_code { "</code>" } else { "<code>" });
                in_code = !in_code;
            }
            c => out.push(c),
        }
    }
    out
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a plain HTTP client builds")
    })
}

/// Plain pass-through to the dev server. No cookie jar: the browser talks to
/// the app by name and keeps the app's own cookies itself; the stand's
/// cookies live in the per-app gateway jar as before.
async fn forward(app: &App, dev_port: u16, front_port: u16, req: Request) -> Result<Response> {
    let (parts, body) = req.into_parts();
    let path_query = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    // `localhost`, not `127.0.0.1`: Vite binds the name, and Node resolves it
    // to `::1` first on many machines, so the server answers on IPv6 only.
    // The connector tries every address the name resolves to; a fixed
    // address would try one and call a running server "not running".
    let target = format!("http://localhost:{dev_port}{path_query}");
    let mut headers = parts.headers;
    crate::gateway::strip_hop_headers(&mut headers);
    // The dev server sees itself as localhost:PORT; Vite checks the Host it
    // is asked for against its own name and answers that one without a
    // configuration line.
    headers.remove(HOST);
    let body = axum::body::to_bytes(body, crate::gateway::MAX_REQUEST_BODY)
        .await
        .context("request body too large")?;
    let upstream = client().request(parts.method, &target).headers(headers).body(body).send().await?;

    let status = upstream.status();
    let mut resp_headers = upstream.headers().clone();
    crate::gateway::strip_hop_headers(&mut resp_headers);
    rewrite_location(&mut resp_headers, dev_port, &address(&app.name, front_port))?;
    let mut response = Response::builder().status(status);
    if let Some(headers_mut) = response.headers_mut() {
        *headers_mut = resp_headers;
    }
    Ok(response.body(Body::from_stream(upstream.bytes_stream()))?)
}

/// A dev server that redirects to its own port is redirected back to the name.
fn rewrite_location(headers: &mut HeaderMap, dev_port: u16, public: &str) -> Result<()> {
    let Some(location) = headers.get(LOCATION).and_then(|v| v.to_str().ok()).map(str::to_string) else {
        return Ok(());
    };
    for own in [format!("http://localhost:{dev_port}"), format!("http://127.0.0.1:{dev_port}")] {
        if let Some(rest) = location.strip_prefix(&own) {
            let rewritten = format!("{public}{rest}");
            headers.insert(
                LOCATION,
                HeaderValue::from_str(&rewritten).context("rewritten location is not a valid header value")?,
            );
            break;
        }
    }
    Ok(())
}

/// HMR and the like: accept the browser's upgrade, open the same socket to
/// the dev server and pump frames both ways.
async fn websocket(app: &App, dev_port: u16, req: Request) -> Result<Response> {
    let (mut parts, _body) = req.into_parts();
    let mut upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|err| anyhow!("invalid websocket upgrade: {err}"))?;
    let path_query = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    // The name, for the same reason as in `forward`: the socket tries each
    // address it resolves to.
    let target = format!("ws://localhost:{dev_port}{path_query}");
    let mut request = target.into_client_request().context("cannot build the upstream websocket request")?;
    if let Some(cookie) = parts.headers.get(COOKIE) {
        request.headers_mut().insert(COOKIE, cookie.clone());
    }
    if let Some(protocol) = parts.headers.get(SEC_WEBSOCKET_PROTOCOL) {
        request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, protocol.clone());
        if let Ok(protocols) = protocol.to_str() {
            upgrade = upgrade.protocols(protocols.split(',').map(|p| p.trim().to_string()).collect::<Vec<_>>());
        }
    }
    let name = app.name.clone();
    Ok(upgrade.on_upgrade(move |client| async move {
        match tokio_tungstenite::connect_async(request).await {
            Ok((upstream, _response)) => crate::gateway::pump(client, upstream).await,
            Err(err) => eprintln!("turnout front door: websocket to '{name}' failed: {err}"),
        }
    }))
}

/// Ports the dev servers are handed out from, one per app, fixed on first
/// `dev`. A hundred is more apps than a workstation runs; the range sits
/// clear of the 5173 Vite defaults to, so a server started by hand does
/// not collide with one turnout placed.
pub const DEV_PORTS: std::ops::RangeInclusive<u16> = 5100..=5199;

/// A dev port for `app`: its own when it has one, else the first free port
/// of the range no other app holds. `None` when the range is exhausted.
pub fn dev_port_for(apps: &[App], app: &str) -> Option<u16> {
    if let Some(port) = apps.iter().find(|a| a.name == app).and_then(|a| a.dev_port) {
        return Some(port);
    }
    let taken: Vec<u16> = apps.iter().flat_map(|a| a.dev_port.into_iter().chain(a.gateway_port)).collect();
    DEV_PORTS
        .filter(|port| !taken.contains(port))
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

/// A pinned dev port belongs to one app, like a gateway port.
pub fn ensure_dev_port_is_free(apps: &[App], port: u16, this: &str) -> Result<()> {
    if let Some(other) = apps.iter().find(|a| a.name != this && a.dev_port == Some(port)) {
        bail!("dev port {port} is already used by app '{}' - pick another", other.name);
    }
    Ok(())
}

/// Open a URL in the default browser, the platform's way.
pub fn open_in_browser(url: &str) -> Result<()> {
    #[cfg(windows)]
    let status = std::process::Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg(url).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(url).status();
    let status = status.with_context(|| format!("cannot open {url}"))?;
    if !status.success() {
        bail!("the browser did not open {url} ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn app(name: &str, dev_port: Option<u16>, gateway_port: Option<u16>) -> App {
        App {
            name: name.to_string(),
            path: String::new(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port,
            gateway_env: None,
            env_file: None,
            dev_port,
            servers: Vec::new(),
        }
    }

    #[test]
    fn the_host_names_the_app_with_or_without_a_port() {
        assert_eq!(app_of_host("myapp.localhost"), Some("myapp"));
        assert_eq!(app_of_host("myapp.localhost:7000"), Some("myapp"));
        assert_eq!(app_of_host("my-app.localhost:80"), Some("my-app"));
        for host in [
            "localhost",
            "localhost:7000",
            ".localhost",
            "a.b.localhost",
            "myapp.example.com",
            "127.0.0.1:7000",
            "",
        ] {
            assert_eq!(app_of_host(host), None, "{host}");
        }
    }

    #[test]
    fn the_address_hides_port_80_and_shows_any_other() {
        assert_eq!(address("myapp", 80), "http://myapp.localhost");
        assert_eq!(address("myapp", 7000), "http://myapp.localhost:7000");
        assert_eq!(door(80), "http://localhost");
        assert_eq!(door(7000), "http://localhost:7000");
    }

    #[test]
    fn a_dev_port_is_kept_once_given_and_skips_ports_other_apps_hold() {
        let apps = [app("web", Some(5100), None), app("api", None, Some(5101)), app("new", None, None)];
        assert_eq!(dev_port_for(&apps, "web"), Some(5100));
        // 5100 is web's, 5101 is api's gateway port: the next free one is 5102.
        let picked = dev_port_for(&apps, "new").unwrap();
        assert!(picked >= 5102, "{picked}");
        assert!(DEV_PORTS.contains(&picked));
        assert!(ensure_dev_port_is_free(&apps, 5100, "web").is_ok());
        let error = ensure_dev_port_is_free(&apps, 5100, "new").unwrap_err().to_string();
        assert!(error.contains("already used by app 'web'"), "{error}");
    }

    #[test]
    fn a_redirect_to_the_dev_servers_own_port_comes_back_by_name() {
        let mut headers = HeaderMap::new();
        headers.insert(LOCATION, HeaderValue::from_static("http://localhost:5100/after?x=1"));
        rewrite_location(&mut headers, 5100, "http://myapp.localhost").unwrap();
        assert_eq!(headers.get(LOCATION).unwrap(), "http://myapp.localhost/after?x=1");
        headers.insert(LOCATION, HeaderValue::from_static("https://elsewhere.example.com/"));
        rewrite_location(&mut headers, 5100, "http://myapp.localhost").unwrap();
        assert_eq!(headers.get(LOCATION).unwrap(), "https://elsewhere.example.com/");
    }

    #[test]
    fn the_page_marks_code_and_escapes_markup() {
        assert_eq!(html_escape("run `turnout dev x` <now>"), "run <code>turnout dev x</code> &lt;now&gt;");
    }

    #[test]
    fn a_pinned_port_wins_and_zero_keeps_the_door_shut() {
        // Set and unset around the call: the variable is process-wide.
        unsafe { std::env::set_var(ENV_PORT, "7123") };
        assert_eq!(pick_port(), Some(7123));
        unsafe { std::env::set_var(ENV_PORT, "0") };
        assert_eq!(pick_port(), None);
        unsafe { std::env::remove_var(ENV_PORT) };
    }
}
