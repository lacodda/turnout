//! The gateway against connections torn the way a network tears them - not
//! against a stand that pretends to fail.
//!
//! A proxy (lacodda-tear, the line's test bench) stands between the gateway
//! and a real stand on a real socket, and tears the connection on cue: the
//! network gone and back, a pooled connection reset while it idles, an
//! answer lost after the stand did the work. What the stand did is asked of
//! the stand itself, around the proxy.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use common::{turnout, turnout_process};
use lacodda_tear::{Cut, Tear};

/// A stand behind a proxy, with the gateway in front of both.
///
/// Fields drop in order: the gateway goes before the data directory it
/// writes to, the runtime that drives the proxy and the stand goes last.
struct Torn {
    _gateway: Gateway,
    tear: Tear,
    data: tempfile::TempDir,
    gateway_port: u16,
    /// How many orders reached the stand.
    orders: Arc<AtomicUsize>,
    runtime: tokio::runtime::Runtime,
}

/// Kills the gateway even when an assertion panics.
struct Gateway(std::process::Child);

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

impl Torn {
    fn start() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let orders = Arc::new(AtomicUsize::new(0));
        let counted = orders.clone();
        let stand = runtime.block_on(async move {
            use axum::routing::{get, post};
            let app = axum::Router::new().route("/hello", get(|| async { "hello from the stand" })).route(
                "/order",
                post(move || {
                    let counted = counted.clone();
                    async move { format!("order {}", counted.fetch_add(1, Ordering::SeqCst) + 1) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            address
        });
        let tear = runtime.block_on(Tear::start(stand)).unwrap();

        let data = tempfile::tempdir().unwrap();
        turnout(data.path()).args(["setup", "--yes"]).assert().success();
        let project = data.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gateway_port = reservation.local_addr().unwrap().port();
        turnout(data.path())
            .args(["server", "add", "stand", "--url", &format!("http://{}", tear.addr())])
            .assert()
            .success();
        turnout(data.path())
            .args(["app", "add", "myapp", "--path"])
            .arg(&project)
            .args(["--port", &gateway_port.to_string()])
            .assert()
            .success();
        turnout(data.path()).args(["use", "myapp", "stand", "--no-check"]).assert().success();
        drop(reservation);
        let child = turnout_process(data.path())
            .env("TURNOUT_FRONT_PORT", "0")
            .args(["gateway", "run"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let gateway = Gateway(child);
        let torn = Self {
            _gateway: gateway,
            tear,
            data,
            gateway_port,
            orders,
            runtime,
        };
        torn.wait_until("the gateway's record", |torn| torn.status()["gateway"]["state"] == "running");
        torn
    }

    /// Ask the gateway: the status and the body it answered with.
    fn ask(&self, method: &str, path: &str) -> (u16, String) {
        let url = format!("http://127.0.0.1:{}{path}", self.gateway_port);
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        self.runtime.block_on(async move {
            // A client of its own each time: what is under test is the
            // gateway's pool towards the stand, not this side's.
            let client = reqwest::Client::new();
            let answer = client.request(method, url).send().await.unwrap();
            let status = answer.status().as_u16();
            (status, answer.text().await.unwrap_or_default())
        })
    }

    fn status(&self) -> serde_json::Value {
        let output = turnout(self.data.path()).args(["status", "--json"]).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn signals(&self) -> Vec<serde_json::Value> {
        self.status()["gateway"]["signals"].as_array().cloned().unwrap_or_default()
    }

    fn wait_until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        for _ in 0..200 {
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("{what} never happened");
    }
}

/// The network between the gateway and the stand goes and comes back: while
/// it is gone the gateway answers 502 and says the stand does not answer,
/// and once it is back the next request goes through and the word goes.
#[test]
fn a_stand_the_network_dropped_is_unreachable_until_it_is_back() {
    let torn = Torn::start();
    assert_eq!(torn.ask("GET", "/hello"), (200, "hello from the stand".to_string()));

    torn.tear.offline();
    for _ in 0..3 {
        let (status, body) = torn.ask("GET", "/hello");
        assert_eq!(status, 502, "{body}");
        assert!(body.contains("stand 'stand' is unreachable"), "{body}");
    }
    torn.wait_until("the stand signal", |torn| torn.signals().len() == 1);
    let down = &torn.signals()[0];
    assert_eq!(down["kind"], "stand_down", "{down}");
    assert_eq!(down["status"], serde_json::Value::Null, "no answer at all: {down}");

    torn.tear.online();
    assert_eq!(torn.ask("GET", "/hello").0, 200);
    torn.wait_until("the stand to be back", |torn| torn.signals().is_empty());
}

/// A connection the gateway keeps open to the stand between requests is
/// reset while it idles - a router that forgot it, a stand restarted behind
/// a proxy. The next request is not the one that pays for it.
#[test]
fn a_pooled_connection_torn_while_it_idles_fails_no_request() {
    let torn = Torn::start();
    assert_eq!(torn.ask("GET", "/hello").0, 200);
    assert_eq!(torn.tear.connections(), 1);

    torn.tear.cut(Cut::Now);
    torn.wait_until("the idle connection torn", |torn| torn.tear.torn() == 1);
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..3 {
        assert_eq!(torn.ask("GET", "/hello").0, 200);
    }
    assert_eq!(torn.tear.connections(), 2, "one new connection carries every request after the tear");
}

/// The stand takes an order and its answer is lost on the way back. The
/// gateway says so - and does not send the order again on its own: a POST
/// that already did its work must not do it twice.
#[test]
fn an_answer_lost_after_the_stand_did_the_work_is_not_sent_again() {
    let torn = Torn::start();
    assert_eq!(torn.ask("GET", "/hello").0, 200);

    torn.tear.cut(Cut::Down(0));
    let (status, body) = torn.ask("POST", "/order");
    assert_eq!(status, 502, "{body}");
    assert_eq!(torn.tear.torn(), 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(torn.orders.load(Ordering::SeqCst), 1, "the stand did the work once, and only once");

    assert_eq!(torn.ask("POST", "/order"), (200, "order 2".to_string()));
}
