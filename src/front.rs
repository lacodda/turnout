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

/// What [`ENV_PORT`] pins the door to, as it was set.
pub fn pinned_port() -> Option<String> {
    std::env::var(ENV_PORT).ok()
}

/// The port the door should open on: `pinned` (see [`pinned_port`]), or the
/// first of 80 and 7000 that binds. `None` means no door - pinned shut, or
/// both ports taken.
///
/// The pin is an argument rather than read here so that it can be tested
/// without setting a variable every test in the process shares.
pub fn pick_port(pinned: Option<&str>) -> Option<u16> {
    if let Some(pinned) = pinned.and_then(|value| value.trim().parse::<u16>().ok()) {
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

/// Ports the per-app gateway listeners are handed out from, one per app,
/// fixed when the app is registered. Same size and the same reasoning as
/// [`DEV_PORTS`]; 7100 is where the wizard used to start counting, so a
/// catalog filled in by hand keeps the numbers it already had.
pub const GATEWAY_PORTS: std::ops::RangeInclusive<u16> = 7100..=7199;

/// A dev port for `app`: its own when it has one, else the first free port
/// of the range no other app holds. `None` when the range is exhausted.
///
/// Assigned late, on the first `dev`: nothing needs the number until a
/// server is actually started.
pub fn dev_port_for(apps: &[App], app: &str) -> Option<u16> {
    free_port(DEV_PORTS, apps, apps.iter().find(|a| a.name == app).and_then(|a| a.dev_port))
}

/// A gateway port for a new app: the first of the range no app holds and
/// nothing on the machine is listening on. `None` when the range is
/// exhausted.
///
/// Assigned early, when the app is registered - unlike a dev port. The
/// number is written into the app's dotenv file and substituted into
/// `{gateway}` command lines straight away, so an app without one is an app
/// whose IDE-run `pnpm dev` talks to nothing.
pub fn free_gateway_port(apps: &[App]) -> Option<u16> {
    free_port(GATEWAY_PORTS, apps, None)
}

/// The first port of `range` that no app in `apps` holds - either kind, so
/// the two ranges could overlap without two apps colliding - and that the
/// machine will actually hand out. `held` short-circuits: an app that
/// already has its port keeps it, taken or not, because moving it would
/// break the address its dotenv file already carries.
fn free_port(range: std::ops::RangeInclusive<u16>, apps: &[App], held: Option<u16>) -> Option<u16> {
    if held.is_some() {
        return held;
    }
    let taken: Vec<u16> = apps.iter().flat_map(|a| a.dev_port.into_iter().chain(a.gateway_port)).collect();
    range
        .filter(|port| !taken.contains(port))
        .find(|port| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok())
}

/// Give the apps that just arrived in `apps` ports this catalog can hold:
/// each its own gateway port, and no dev port another app already uses.
///
/// An import is the one way into the catalog that skips `app add`, so it is
/// where an export from before v0.18 brings an app with no gateway port (the
/// state the schema 4 migration put an end to), and an export from before
/// v0.13.1 - or simply from another machine - brings a port some app here
/// already holds. The apps already here keep their numbers: their dotenv
/// files carry them. An arriving app that clashes takes a free gateway port,
/// and loses a clashing dev port, which its next `dev` assigns again.
///
/// Returns what was changed, one line per app, for the import to report.
pub fn settle_arrivals(apps: &mut [App], arriving: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut settled: Vec<bool> = apps.iter().map(|app| !arriving.contains(&app.name)).collect();
    for index in 0..apps.len() {
        if settled[index] {
            continue;
        }
        let holder = |port: u16, apps: &[App], settled: &[bool]| {
            apps.iter()
                .enumerate()
                .find(|(other, app)| *other != index && settled[*other] && (app.gateway_port == Some(port) || app.dev_port == Some(port)))
                .map(|(_, app)| app.name.clone())
        };

        let clash = apps[index]
            .gateway_port
            .and_then(|port| holder(port, apps, &settled).map(|other| (port, other)));
        if apps[index].gateway_port.is_none() || clash.is_some() {
            let name = apps[index].name.clone();
            match free_gateway_port(apps) {
                Some(port) => {
                    apps[index].gateway_port = Some(port);
                    notes.push(match &clash {
                        Some((old, other)) => format!("'{name}' moved to gateway port {port} - {old} is '{other}''s on this machine"),
                        None => format!("'{name}' was given gateway port {port} - turnout assigns one to every app now"),
                    });
                }
                None => {
                    if clash.is_some() {
                        apps[index].gateway_port = None;
                    }
                    notes.push(format!(
                        "'{name}' has no gateway port - none is free in {GATEWAY_PORTS:?}; set one with `turnout app edit {name} --port PORT`"
                    ));
                }
            }
        }

        if let Some(port) = apps[index].dev_port
            && let Some(other) = holder(port, apps, &settled)
        {
            apps[index].dev_port = None;
            notes.push(format!(
                "'{}' gave up dev port {port} - '{other}' uses it here; its next `dev` picks a free one",
                apps[index].name
            ));
        }
        settled[index] = true;
    }
    notes
}

/// A port pinned by hand belongs to one app, whichever kind it is pinned as.
///
/// Gateway and dev ports share one list: [`free_port`] hands out no number
/// any app holds as either kind (ADR 0016), and a pinned port is held to the
/// same rule. Each kind used to be checked against its own kind only, so
/// `app edit web --dev-port 7100` took another app's gateway port, and the
/// gateway's listener and the dev server fought over it. `this` is the app
/// being added or edited: keeping the port it holds is not a clash. Its own
/// two ports are [`ensure_own_ports_differ`]'s.
pub fn ensure_port_is_free(apps: &[App], port: u16, this: &str) -> Result<()> {
    for other in apps.iter().filter(|app| app.name != this) {
        let (kind, flag) = if other.gateway_port == Some(port) {
            ("gateway port", "--port")
        } else if other.dev_port == Some(port) {
            ("dev port", "--dev-port")
        } else {
            continue;
        };
        bail!(
            "port {port} is already used by app '{0}' as its {kind} - pick another, or move '{0}' first with `turnout app edit {0} {flag} PORT`",
            other.name
        );
    }
    Ok(())
}

/// An app's gateway listener and its dev server cannot share one port.
pub fn ensure_own_ports_differ(app: &str, gateway_port: Option<u16>, dev_port: Option<u16>) -> Result<()> {
    if let Some(port) = gateway_port.filter(|port| Some(*port) == dev_port) {
        bail!("port {port} would be both the gateway port and the dev port of '{app}' - the listener and the dev server cannot share it");
    }
    Ok(())
}

/// Writes the pages turnout would open into a file, one per line, instead of
/// opening them. Undocumented in the CLI, like `TURNOUT_TOASTS`.
pub const OPENED_ENV: &str = "TURNOUT_OPENED";

/// Open a URL in the default browser, the platform's way.
pub fn open_in_browser(url: &str) -> Result<()> {
    // For tests, as `TURNOUT_TOASTS` is for notifications: which page a
    // command opens is the behaviour under test, and a test run must not
    // open real browser tabs - nor can a CI runner.
    if let Some(sink) = std::env::var_os(OPENED_ENV).filter(|path| !path.is_empty()) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sink)
            .with_context(|| format!("cannot write {}", std::path::Path::new(&sink).display()))?;
        return writeln!(file, "{url}").context("cannot record the opened page");
    }
    match opener(url)? {
        Opener::Shell { file } => shell_open(&file).with_context(|| format!("cannot open {url}")),
        Opener::Program { program, args } => {
            let status = std::process::Command::new(program)
                .args(&args)
                .status()
                .with_context(|| format!("cannot open {url}"))?;
            if !status.success() {
                bail!("the browser did not open {url} ({status})");
            }
            Ok(())
        }
    }
}

/// What opening a page runs, built apart from running it so that it can be
/// checked.
#[derive(Debug, PartialEq, Eq)]
enum Opener {
    /// `ShellExecuteW` with the URL as the file to open (Windows).
    ///
    /// Not `cmd /C start "" URL`, which is what opened pages until v0.26:
    /// cmd still acts on `&`, `|`, `^` and `%VAR%` in its command line, so a
    /// target at `https://stand.example.com/app/?tenant=a&lang=en` opened a
    /// page cut off at the `&`, and cmd ran `lang=en` as a command of its own.
    /// Nor `rundll32 url.dll,FileProtocolHandler URL`, which takes the raw
    /// rest of its command line, quotes and all. The shell is handed the URL
    /// as one string, NUL-terminated UTF-16, and passes it to the handler
    /// registered for its scheme; no command line is built on the way.
    Shell { file: Vec<u16> },
    /// A program with the URL as its one argument: argv reaches it as it is,
    /// with no shell in between.
    Program { program: &'static str, args: Vec<String> },
}

fn opener(url: &str) -> Result<Opener> {
    if cfg!(windows) {
        if url.contains('\0') {
            bail!("cannot open an address with a NUL character in it");
        }
        return Ok(Opener::Shell {
            file: url.encode_utf16().chain(std::iter::once(0)).collect(),
        });
    }
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Ok(Opener::Program {
        program,
        args: vec![url.to_string()],
    })
}

/// Open `file` - a NUL-terminated wide string - the way a double click
/// would: with whatever handles its scheme.
#[cfg(windows)]
fn shell_open(file: &[u16]) -> Result<()> {
    use windows_sys::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    // SAFETY: both strings are NUL-terminated and outlive the call; every
    // other pointer is null, which ShellExecuteW takes as "none". COM is
    // initialised for the call, as ShellExecuteW asks - a handler may be a
    // shell extension - and uninitialised only when this call initialised it.
    let result = unsafe {
        let initialised = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
        let result = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
        if initialised >= 0 {
            CoUninitialize();
        }
        result as isize
    };
    // Anything above 32 is success; below is an error code from the shell.
    if result <= 32 {
        bail!("the shell did not open it (error {result})");
    }
    Ok(())
}

#[cfg(not(windows))]
fn shell_open(_file: &[u16]) -> Result<()> {
    unreachable!("only Windows opens a page through the shell")
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
        assert!(ensure_port_is_free(&apps, 5100, "web").is_ok());
        let error = ensure_port_is_free(&apps, 5100, "new").unwrap_err().to_string();
        assert!(error.contains("already used by app 'web' as its dev port"), "{error}");
    }

    /// A pinned port is checked against both kinds, as a handed-out one is:
    /// a dev port on another app's gateway port, or the other way round, put
    /// the gateway's listener and a dev server on one port.
    #[test]
    fn a_pinned_port_clashes_with_either_kind() {
        let apps = [app("web", Some(5100), Some(7100))];
        let error = ensure_port_is_free(&apps, 7100, "api").unwrap_err().to_string();
        assert!(error.contains("already used by app 'web' as its gateway port"), "{error}");
        assert!(error.contains("turnout app edit web --port PORT"), "{error}");
        let error = ensure_port_is_free(&apps, 5100, "api").unwrap_err().to_string();
        assert!(error.contains("as its dev port"), "{error}");
        assert!(error.contains("--dev-port PORT"), "{error}");
        assert!(ensure_port_is_free(&apps, 7100, "web").is_ok(), "an app keeps what it holds");

        assert!(ensure_own_ports_differ("web", Some(7100), Some(7100)).is_err());
        assert!(ensure_own_ports_differ("web", Some(7100), Some(5100)).is_ok());
        assert!(ensure_own_ports_differ("web", None, None).is_ok());
    }

    #[test]
    fn a_gateway_port_comes_from_its_own_range_and_skips_what_apps_hold() {
        // Not "7100 exactly": the probe bind makes the answer depend on the
        // machine, and on every other test probing the range at that instant.
        let first = free_gateway_port(&[]).unwrap();
        assert!(GATEWAY_PORTS.contains(&first), "{first}");
        let apps = [app("web", None, Some(7100)), app("api", Some(7101), None)];
        // 7100 is web's gateway port and 7101 is api's dev port, pinned there
        // by hand: both are out, whichever kind of port they are.
        let picked = free_gateway_port(&apps).unwrap();
        assert!(picked >= 7102, "{picked}");
        assert!(GATEWAY_PORTS.contains(&picked));
    }

    #[test]
    fn a_port_already_held_is_kept_even_when_the_machine_holds_it_too() {
        // Something else is listening on the port the app was given. Moving
        // it would break the address its dotenv file carries, so it stays.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = listener.local_addr().unwrap().port();
        assert_eq!(free_port(DEV_PORTS, &[], Some(busy)), Some(busy));
    }

    fn names(of: &[&str]) -> Vec<String> {
        of.iter().map(|name| name.to_string()).collect()
    }

    /// The app already here keeps its number - its dotenv file carries it -
    /// and the one that arrived with the same number takes another.
    #[test]
    fn an_arriving_app_moves_off_a_gateway_port_held_here() {
        let mut apps = [app("here", None, Some(7150)), app("arrived", None, Some(7150))];
        let notes = settle_arrivals(&mut apps, &names(&["arrived"]));
        assert_eq!(apps[0].gateway_port, Some(7150), "the app already here keeps its port");
        let moved = apps[1].gateway_port.unwrap();
        assert!(moved != 7150 && GATEWAY_PORTS.contains(&moved), "{moved}");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("7150 is 'here''s"), "{notes:?}");
    }

    /// Two apps of one export on one port - accepted before v0.13.1: the first
    /// keeps it, the second moves.
    #[test]
    fn two_arriving_apps_on_one_port_end_up_on_two() {
        let mut apps = [app("first", None, Some(7150)), app("second", None, Some(7150))];
        settle_arrivals(&mut apps, &names(&["first", "second"]));
        assert_eq!(apps[0].gateway_port, Some(7150));
        assert_ne!(apps[1].gateway_port, Some(7150));
        assert!(apps[1].gateway_port.is_some());
    }

    /// An export from before v0.18 has apps with no gateway port at all.
    #[test]
    fn an_arriving_app_without_a_port_is_given_one() {
        let mut apps = [app("here", None, Some(7100)), app("arrived", None, None)];
        let notes = settle_arrivals(&mut apps, &names(&["arrived"]));
        let given = apps[1].gateway_port.expect("a port is given");
        assert_ne!(given, 7100);
        assert!(notes[0].contains("was given gateway port"), "{notes:?}");
    }

    /// A dev port is assigned again on the next `dev`, so a clashing one is
    /// dropped rather than moved - and either kind of port held here counts.
    #[test]
    fn an_arriving_dev_port_held_here_is_given_up() {
        let mut apps = [app("here", Some(5150), Some(7100)), app("arrived", Some(5150), Some(7101))];
        let notes = settle_arrivals(&mut apps, &names(&["arrived"]));
        assert_eq!(apps[0].dev_port, Some(5150));
        assert_eq!(apps[1].dev_port, None);
        assert_eq!(apps[1].gateway_port, Some(7101), "a gateway port nobody holds stays");
        assert!(notes[0].contains("gave up dev port 5150"), "{notes:?}");

        let mut apps = [app("here", None, Some(7101)), app("arrived", Some(7101), Some(7102))];
        settle_arrivals(&mut apps, &names(&["arrived"]));
        assert_eq!(apps[1].dev_port, None, "a dev port that is a gateway port here clashes too");
    }

    /// Apps that are not arriving are never touched, whatever they hold.
    #[test]
    fn apps_already_here_are_left_as_they_are() {
        let mut apps = [app("one", None, Some(7150)), app("two", None, Some(7150)), app("none", None, None)];
        let notes = settle_arrivals(&mut apps, &[]);
        assert!(notes.is_empty());
        assert_eq!(apps[1].gateway_port, Some(7150));
        assert_eq!(apps[2].gateway_port, None);
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

    /// A page is opened with its address whole: the characters a shell acts
    /// on - `&`, `|`, `^`, `%VAR%`, quotes - reach the browser as they are,
    /// because no shell is in the way.
    #[test]
    fn a_page_is_opened_with_its_address_intact() {
        let url = r#"https://stand.example.com/app/?tenant=a&lang=en|x^y%PATH%"z" w"#;
        let opened = opener(url).unwrap();
        if cfg!(windows) {
            let expected: Vec<u16> = url.encode_utf16().chain([0]).collect();
            assert_eq!(opened, Opener::Shell { file: expected });
            assert!(opener("http://a.localhost/\0x").is_err(), "a NUL would cut the address short");
        } else {
            let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
            assert_eq!(
                opened,
                Opener::Program {
                    program,
                    args: vec![url.to_string()]
                }
            );
        }
    }

    #[test]
    fn a_pinned_port_wins_and_zero_keeps_the_door_shut() {
        assert_eq!(pick_port(Some("7123")), Some(7123));
        assert_eq!(pick_port(Some(" 7123 ")), Some(7123));
        assert_eq!(pick_port(Some("0")), None);
        // Unpinned, or pinned to nonsense: whichever of the two ports this
        // machine hands out, if either.
        for unpinned in [None, Some("door")] {
            let picked = pick_port(unpinned);
            assert!(matches!(picked, None | Some(PORT) | Some(FALLBACK_PORT)), "{picked:?}");
        }
    }
}
