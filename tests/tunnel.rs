//! A tunnel end to end: the binary opens it in the background, a client here
//! reaches a service the server reaches, the SSH server goes away and comes
//! back, and the tunnel goes down and comes back with it.
//!
//! The server is russh's, in this process, on a loopback port: password
//! sign-in and `direct-tcpip`, the two things a tunnel asks of sshd. Nothing
//! below the protocol is mocked - the tunnel crosses a real SSH connection to
//! a real socket, so it fails for the reasons a stand would make it fail.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, Session};
use russh::{Channel, ChannelOpenFailure};

mod common;
use common::{toast_file, turnout};

const USER: &str = "deploy";
const PASSWORD: &str = "sesame";

/// An SSH server on a loopback port that can be taken down and brought up
/// again on the same port, with the same host key.
struct Stand {
    port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Stand {
    fn start(port: u16) -> Self {
        let (ready, bound) = std::sync::mpsc::channel();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            // The runtime goes with the thread: every connection it holds is
            // dropped, the way a server that goes down drops them.
            let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
            runtime.block_on(async move {
                let pair = Ed25519Keypair::from_seed(&[21; 32]);
                let config = Arc::new(server::Config {
                    auth_rejection_time: Duration::ZERO,
                    auth_rejection_time_initial: Some(Duration::ZERO),
                    keys: vec![russh::keys::PrivateKey::new(KeypairData::Ed25519(pair), "tunnel test host").unwrap()],
                    ..Default::default()
                });
                let socket = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.expect("bind the stand's port");
                ready.send(socket.local_addr().unwrap().port()).unwrap();
                let mut factory = Factory;
                tokio::select! {
                    _ = server::Server::run_on_socket(&mut factory, config, &socket) => {}
                    _ = stopped => {}
                }
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
        });
        let port = bound.recv().expect("the stand reports its port");
        Self {
            port,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    /// Take the server down, connections and all.
    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Stand {
    fn drop(&mut self) {
        self.halt();
    }
}

struct Factory;

impl server::Server for Factory {
    type Handler = Handler;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Handler {
        Handler
    }
}

struct Handler;

impl server::Handler for Handler {
    type Error = anyhow::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == USER && password == PASSWORD {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        })
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let port = u16::try_from(port_to_connect).unwrap_or(0);
        match tokio::net::TcpStream::connect((host_to_connect, port)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                });
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}

/// A service on the server's side: it answers every line with the line in
/// capitals, so an answer is told apart from an echo of the question.
fn service() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut socket = socket;
                let mut buffer = [0u8; 1024];
                while let Ok(read) = socket.read(&mut buffer) {
                    if read == 0 || socket.write_all(&buffer[..read].to_ascii_uppercase()).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// A port nobody listens on right now.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Ask through the tunnel, and read the answer.
fn ask(port: u16, question: &str) -> std::io::Result<String> {
    let mut socket = TcpStream::connect(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(Duration::from_secs(15)))?;
    socket.write_all(question.as_bytes())?;
    let mut answer = vec![0u8; question.len()];
    socket.read_exact(&mut answer)?;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// Closes every tunnel of the data directory however the test ends: a
/// tunnel is a process in the background, and one left behind would outlive
/// the suite.
struct Closing(PathBuf);

impl Drop for Closing {
    fn drop(&mut self) {
        let _ = turnout(&self.0).args(["tunnel", "stop", "--all"]).output();
    }
}

fn tunnels(dir: &Path) -> serde_json::Value {
    let output = turnout(dir).args(["tunnel", "list", "--json"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Wait until the tunnel's state is `state`, and return its row.
fn until_state(dir: &Path, name: &str, state: &str, patience: Duration) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let list = tunnels(dir);
        if let Some(row) = list.as_array().unwrap().iter().find(|row| row["name"] == name)
            && row["state"] == state
        {
            return row.clone();
        }
        assert!(
            started.elapsed() < patience,
            "tunnel {name} did not become {state} within {patience:?}: {list:#}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// A data directory with the stand in its catalog, signing in by password.
fn catalog(dir: &Path, stand: u16) {
    turnout(dir).args(["setup", "--yes"]).assert().success();
    turnout(dir)
        .args(["credential", "add", "deploy", "--user", USER, "--auth", "password"])
        .assert()
        .success();
    turnout(dir).args(["pass", "set", "deploy"]).write_stdin(PASSWORD).assert().success();
    turnout(dir)
        .args(["server", "add", "stand", "--url", "http://127.0.0.1", "--host"])
        .arg(format!("127.0.0.1:{stand}"))
        .args(["--credential", "deploy"])
        .assert()
        .success();
}

#[test]
fn a_tunnel_carries_goes_down_with_its_server_and_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let _closing = Closing(dir.path().to_path_buf());
    let mut stand = Stand::start(0);
    catalog(dir.path(), stand.port);
    let far = service();
    let here = free_port();

    let opened = turnout(dir.path()).args(["tunnel", "db@stand"]).arg(format!("{here}:{far}")).output().unwrap();
    let said = String::from_utf8_lossy(&opened.stdout);
    assert!(opened.status.success(), "{said}{}", String::from_utf8_lossy(&opened.stderr));
    assert!(said.contains(&format!("Tunnel db open: localhost:{here} -> stand:{far}")), "{said}");
    assert_eq!(ask(here, "select 1\n").unwrap(), "SELECT 1\n");

    // Asked again, the same tunnel is there already; under another route the
    // name is refused.
    turnout(dir.path())
        .args(["tunnel", "db@stand"])
        .arg(format!("{here}:{far}"))
        .assert()
        .success()
        .stdout(predicates::str::contains("already open"));
    turnout(dir.path())
        .args(["tunnel", "db@stand"])
        .arg(format!("{}:{far}", free_port()))
        .assert()
        .failure()
        .stderr(predicates::str::contains("already open to something else"));

    // The registry has it: `tunnel list`, `ps`, `status` all say so.
    let row = until_state(dir.path(), "db", "open", Duration::from_secs(5));
    assert_eq!(row["local_port"], here);
    assert_eq!(row["server"], "stand");
    let ps = turnout(dir.path()).args(["ps", "--json"]).output().unwrap();
    let ps: serde_json::Value = serde_json::from_slice(&ps.stdout).unwrap();
    let job = ps
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["job"] == "tunnel db")
        .expect("ps lists the tunnel");
    assert_eq!(job["state"], "ready");
    assert_eq!(job["port"], here);
    turnout(dir.path())
        .args(["status", "--short"])
        .assert()
        .success()
        .stdout(predicates::str::contains("1 tunnel"));

    // The server goes away: the tunnel says it is down, once, and stays.
    stand.halt();
    let row = until_state(dir.path(), "db", "down", Duration::from_secs(30));
    assert!(row["trouble"].as_str().is_some_and(|trouble| !trouble.is_empty()), "{row:#}");
    // The record says it first; the notification follows it.
    let started = Instant::now();
    let toasts = loop {
        let toasts = std::fs::read_to_string(toast_file(dir.path())).unwrap_or_default();
        if toasts.contains("Tunnel db is down") || started.elapsed() > Duration::from_secs(10) {
            break toasts;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(toasts.matches("Tunnel db is down").count(), 1, "{toasts}");
    assert!(ask(here, "select 2\n").is_err(), "a tunnel that is down carries nothing");

    // It comes back on the same port: the tunnel signs in again by itself.
    let _stand = Stand::start(stand.port);
    let row = until_state(dir.path(), "db", "open", Duration::from_secs(60));
    assert!(row["trouble"].is_null(), "{row:#}");
    assert_eq!(ask(here, "select 3\n").unwrap(), "SELECT 3\n");

    // Closed, it is gone from the list and its port is free.
    turnout(dir.path())
        .args(["tunnel", "stop", "db"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Closed tunnel db"));
    assert_eq!(tunnels(dir.path()), serde_json::json!([]));
    assert!(TcpStream::connect(("127.0.0.1", here)).is_err(), "the port is still held");
}

/// What cannot work is refused before anything is left running: a port with
/// nothing behind it on the far side, and a local port somebody holds.
#[test]
fn a_tunnel_that_cannot_work_does_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let _closing = Closing(dir.path().to_path_buf());
    let stand = Stand::start(0);
    catalog(dir.path(), stand.port);

    let nothing = free_port();
    turnout(dir.path())
        .args(["tunnel", "db@stand"])
        .arg(format!("{}:{nothing}", free_port()))
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!("nothing listens on port {nothing} there")));

    let held = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = held.local_addr().unwrap().port();
    turnout(dir.path())
        .args(["tunnel", "db@stand"])
        .arg(format!("{port}:{}", service()))
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!("port {port} is already in use here")));
    assert_eq!(tunnels(dir.path()), serde_json::json!([]), "a tunnel that failed left a record");

    // A server that is not in the catalog is named as such.
    turnout(dir.path())
        .args(["tunnel", "db@nowhere", "5432"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no server or target named 'nowhere'"));
}
