//! The gateway's control channel: requests about the gateway itself.
//!
//! Everything else reaches the gateway through files it reads per request
//! (ADR 0008). Cookies cannot: they are session secrets, and the jar keeps
//! them in memory precisely so they never touch the disk (ADR 0002). So the
//! gateway opens one more listener, on a loopback port the system picks, and
//! writes its port and a random token into its own job record. A request must
//! carry the token; any web page can send a request to a localhost port, but
//! none can read the record. ADR 0021 has the rest.
//!
//! The channel only ever answers with cookie *names*. Values go in and stay in.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde::{Deserialize, Serialize};

use crate::gateway::{Jars, Watch};
use crate::registry::Control;

/// What `cookie set` sends: the pairs, in the order the browser sent them.
#[derive(Serialize, Deserialize)]
pub struct SetJar {
    pub cookies: Vec<(String, String)>,
}

/// One jar as the channel reports it: whose it is, and the names in it.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct JarView {
    pub app: String,
    pub server: String,
    pub cookies: Vec<String>,
}

#[derive(Clone)]
struct Ctx {
    token: String,
    jars: Jars,
    watch: Watch,
}

/// A fresh token: 32 random bytes, as hex.
pub fn token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Serve the channel on `listener` until the gateway stops.
pub async fn serve(listener: tokio::net::TcpListener, token: String, jars: Jars, watch: Watch) {
    let router = axum::Router::new()
        .route("/jars", get(list))
        .route("/jar/{app}/{server}", put(set).delete(clear))
        .with_state(Ctx { token, jars, watch });
    let _ = axum::serve(listener, router).await;
}

/// Whether the request carries the token. Compared without an early exit, so
/// the time it takes says nothing about how much of a guess was right.
fn authorized(ctx: &Ctx, headers: &HeaderMap) -> bool {
    let Some(given) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    let (given, token) = (given.as_bytes(), ctx.token.as_bytes());
    given.len() == token.len() && given.iter().zip(token).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

fn refused() -> Response {
    (StatusCode::FORBIDDEN, "turnout gateway: this channel takes requests from turnout only\n").into_response()
}

async fn list(State(ctx): State<Ctx>, headers: HeaderMap) -> Response {
    if !authorized(&ctx, &headers) {
        return refused();
    }
    let jars = ctx.jars.lock().expect("cookie jar poisoned");
    let mut views: Vec<JarView> = jars
        .iter()
        .filter(|(_, jar)| !jar.is_empty())
        .map(|((app, server), jar)| {
            let mut cookies: Vec<String> = jar.keys().cloned().collect();
            cookies.sort();
            JarView {
                app: app.clone(),
                server: server.clone(),
                cookies,
            }
        })
        .collect();
    views.sort_by(|a, b| (&a.app, &a.server).cmp(&(&b.app, &b.server)));
    Json(views).into_response()
}

async fn set(State(ctx): State<Ctx>, Path((app, server)): Path<(String, String)>, headers: HeaderMap, Json(body): Json<SetJar>) -> Response {
    if !authorized(&ctx, &headers) {
        return refused();
    }
    let jar: HashMap<String, String> = body.cookies.into_iter().collect();
    let count = jar.len();
    ctx.jars.lock().expect("cookie jar poisoned").insert((app.clone(), server.clone()), jar);
    println!("{app} -> {server}: {count} cookies handed over with `turnout cookie set`");
    ctx.watch.renewed(&app, &server);
    StatusCode::NO_CONTENT.into_response()
}

async fn clear(State(ctx): State<Ctx>, Path((app, server)): Path<(String, String)>, headers: HeaderMap) -> Response {
    if !authorized(&ctx, &headers) {
        return refused();
    }
    ctx.jars.lock().expect("cookie jar poisoned").remove(&(app.clone(), server.clone()));
    println!("{app} -> {server}: cookies forgotten with `turnout cookie clear`");
    StatusCode::NO_CONTENT.into_response()
}

// --- the client side ---------------------------------------------------------------

/// Replace the jar of `app` on `server` with `cookies`.
pub fn set_jar(control: &Control, app: &str, server: &str, cookies: Vec<(String, String)>) -> Result<()> {
    let body = serde_json::to_vec(&SetJar { cookies })?;
    request(control, reqwest::Method::PUT, &format!("/jar/{app}/{server}"), Some(body)).map(drop)
}

/// Empty the jar of `app` on `server`.
pub fn clear_jar(control: &Control, app: &str, server: &str) -> Result<()> {
    request(control, reqwest::Method::DELETE, &format!("/jar/{app}/{server}"), None).map(drop)
}

/// Every jar that holds cookies, by name.
pub fn jars(control: &Control) -> Result<Vec<JarView>> {
    let text = request(control, reqwest::Method::GET, "/jars", None)?;
    serde_json::from_str(&text).context("the gateway answered with something other than its jars")
}

fn request(control: &Control, method: reqwest::Method, path: &str, body: Option<Vec<u8>>) -> Result<String> {
    let url = format!("http://127.0.0.1:{}{path}", control.port);
    let token = control.token.clone();
    crate::utils::run_blocking(async move {
        // Never through a proxy: the address is this machine, and a proxy
        // from the environment would see the token.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .context("cannot build the HTTP client")?;
        let mut request = client.request(method, &url).bearer_auth(token);
        if let Some(body) = body {
            request = request.header(reqwest::header::CONTENT_TYPE, "application/json").body(body);
        }
        let response = request.send().await.context("the gateway does not answer on its control port")?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("the gateway refused: {status} {}", text.trim());
        }
        Ok(text)
    })
}
