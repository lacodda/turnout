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
//! The one request that goes further is the session check `doctor` makes: the
//! gateway asks the stand, and the channel answers with what the stand said.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::{Deserialize, Serialize};

use crate::gateway::{Clients, Jars, Watch};
use crate::output::iso8601;
use crate::registry::Control;

/// What `cookie set` sends: the pairs, in the order the browser sent them.
#[derive(Serialize, Deserialize)]
pub struct SetJar {
    pub cookies: Vec<(String, String)>,
}

/// One jar as the channel reports it: whose it is, the names in it, and what
/// the gateway knows about the session they make.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct JarView {
    pub app: String,
    pub server: String,
    pub cookies: Vec<String>,
    /// When the first of the cookies expires; `null` when they all last as
    /// long as the gateway does.
    pub expires: Option<String>,
    /// The stand's last answer to a request that carried them.
    pub last_answer: Option<AnswerView>,
    /// When a request last proved the session - the one the check sends again.
    pub proven: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct AnswerView {
    pub status: u16,
    pub at: String,
}

/// What the session check found.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The stand answered the proving request as it did before.
    Alive,
    /// It refused it, or sent it elsewhere - a login page, most likely; or
    /// the cookies it carried have run out.
    Expired,
    /// The jar holds cookies, but no request has proved them yet - handed
    /// over by `cookie set` and not used since, say.
    Unproven,
    /// The jar holds no cookies: nobody has logged in through the gateway.
    Empty,
    /// The stand answered 5xx: it is down, whatever the session.
    Down,
    /// The stand did not answer.
    Unreachable,
    /// Some other answer, which says nothing either way.
    Unclear,
}

/// The session check, as the channel answers it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SessionCheck {
    pub verdict: Verdict,
    /// The status the stand answered the check with.
    pub status: Option<u16>,
    /// The request sent again: a path and query under the stand's URL.
    pub path: Option<String>,
    /// When that request proved the session.
    pub proven: Option<String>,
}

/// What a status answered to the proving request says about the session.
///
/// That request was answered 2xx once. A redirect now is the classic way a
/// server-rendered stand sends a lapsed session to its login page, and 403 is
/// how many refuse one - so both read as expired, with 401 and IIS's 440.
pub fn verdict(status: u16) -> Verdict {
    match status {
        200..=299 => Verdict::Alive,
        300..=399 | 401 | 403 | 440 => Verdict::Expired,
        500..=599 => Verdict::Down,
        _ => Verdict::Unclear,
    }
}

#[derive(Clone)]
struct Ctx {
    token: String,
    jars: Jars,
    clients: Clients,
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
pub async fn serve(listener: tokio::net::TcpListener, token: String, jars: Jars, clients: Clients, watch: Watch) {
    let router = axum::Router::new()
        .route("/jars", get(list))
        .route("/jar/{app}/{server}", put(set).delete(clear))
        .route("/jar/{app}/{server}/check", post(check_jar))
        .with_state(Ctx { token, jars, clients, watch });
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
        .map(|((app, server), jar)| JarView {
            app: app.clone(),
            server: server.clone(),
            cookies: jar.names(),
            expires: jar.expires().map(iso8601),
            last_answer: jar.last().map(|answer| AnswerView {
                status: answer.status,
                at: iso8601(answer.at),
            }),
            proven: jar.proof().map(|proof| iso8601(proof.at)),
        })
        .collect();
    views.sort_by(|a, b| (&a.app, &a.server).cmp(&(&b.app, &b.server)));
    Json(views).into_response()
}

async fn set(State(ctx): State<Ctx>, Path((app, server)): Path<(String, String)>, headers: HeaderMap, Json(body): Json<SetJar>) -> Response {
    if !authorized(&ctx, &headers) {
        return refused();
    }
    // The cookies belong to the stand's host; its address is what they are
    // matched against from now on.
    let url = match crate::store::load_servers()
        .ok()
        .and_then(|servers| servers.into_iter().find(|s| s.name == server))
        .and_then(|s| reqwest::Url::parse(&s.url).ok())
    {
        Some(url) => url,
        None => return (StatusCode::NOT_FOUND, format!("turnout gateway: no server named '{server}'\n")).into_response(),
    };
    // A name given twice keeps the last value, as a header read left to right would.
    let pairs: Vec<(String, String)> = body.cookies.into_iter().collect::<HashMap<_, _>>().into_iter().collect();
    let count = pairs.len();
    ctx.jars
        .lock()
        .expect("cookie jar poisoned")
        .entry((app.clone(), server.clone()))
        .or_default()
        .replace(&url, pairs);
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

async fn check_jar(State(ctx): State<Ctx>, Path((app, server)): Path<(String, String)>, headers: HeaderMap) -> Response {
    if !authorized(&ctx, &headers) {
        return refused();
    }
    match crate::gateway::check_session(&ctx.jars, &ctx.clients, &ctx.watch, &app, &server).await {
        Ok(check) => Json(check).into_response(),
        Err(err) => (StatusCode::NOT_FOUND, format!("turnout gateway: {err:#}\n")).into_response(),
    }
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

/// Have the gateway ask the stand whether the session of `app` is alive.
pub fn check(control: &Control, app: &str, server: &str) -> Result<SessionCheck> {
    let text = request(control, reqwest::Method::POST, &format!("/jar/{app}/{server}/check"), None)?;
    serde_json::from_str(&text).context("the gateway answered with something other than a session check")
}

fn request(control: &Control, method: reqwest::Method, path: &str, body: Option<Vec<u8>>) -> Result<String> {
    let url = format!("http://127.0.0.1:{}{path}", control.port);
    let token = control.token.clone();
    crate::utils::run_blocking(async move {
        // Never through a proxy: the address is this machine, and a proxy
        // from the environment would see the token.
        // Longer than the gateway's own patience with a stand, so a check
        // that waits on one is answered rather than abandoned here.
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
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
