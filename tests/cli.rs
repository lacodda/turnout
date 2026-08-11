use assert_cmd::Command;
use predicates::prelude::*;

fn turnout(data_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("turnout").unwrap();
    cmd.env("TURNOUT_DATA_DIR", data_dir);
    // No test should reach github.com as a side effect of running a command.
    // The update check is already off under `CI`, but a local run would spawn
    // a background lookup from every single `setup`. The few tests that do
    // exercise the check turn it back on via `with_update_check`.
    cmd.env("TURNOUT_UPDATE_CHECK", "0");
    cmd
}

#[test]
fn status_before_setup_reports_uninitialized() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Not set up yet"));
}

#[test]
fn setup_creates_data_directory_and_meta() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    turnout(&data)
        .args(["setup", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ready to store"));
    assert!(data.join("meta.json").exists());
}

#[test]
fn setup_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    turnout(dir.path())
        .args(["setup", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("already set up"));
}

#[test]
fn status_after_setup_shows_overview() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Gateway: not running"));
}

/// Initialized data dir + a project directory to register as an app.
fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    let project = dir.path().join("project");
    std::fs::create_dir(&project).unwrap();
    (dir, project)
}

#[test]
fn app_crud_roundtrip() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--port", "7100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("'myapp' added"));
    turnout(dir.path())
        .args(["app", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myapp").and(predicate::str::contains(":7100")));
    turnout(dir.path())
        .args(["app", "show", "myapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("localhost:7100"));
    turnout(dir.path()).args(["app", "remove", "myapp", "--yes"]).assert().success();
    turnout(dir.path())
        .args(["app", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No apps yet"));
}

#[test]
fn app_add_rejects_duplicates_and_bad_names() {
    let (dir, project) = workspace();
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    turnout(dir.path())
        .args(["app", "add", "My App", "--path"])
        .arg(&project)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid name"));
}

#[test]
fn app_add_detects_npm_commands() {
    let (dir, project) = workspace();
    std::fs::write(project.join("package.json"), "{}").unwrap();
    turnout(dir.path()).args(["app", "add", "webapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["app", "show", "webapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("npm run dev"));
}

/// Real scripts beat conventions: a Vue-style project calls its dev script
/// `serve`, and scripts that fill no role stay reachable through `run`.
#[test]
fn app_add_maps_package_json_scripts_to_roles() {
    let (dir, project) = workspace();
    std::fs::write(
        project.join("package.json"),
        r#"{"scripts":{"serve":"vue-cli-service serve","build":"vue-cli-service build","storybook":"start-storybook"}}"#,
    )
    .unwrap();
    std::fs::write(project.join("pnpm-lock.yaml"), "").unwrap();
    turnout(dir.path()).args(["app", "add", "webapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["app", "show", "webapp"]).assert().success().stdout(
        predicate::str::contains("dev")
            .and(predicate::str::contains("pnpm serve"))
            .and(predicate::str::contains("pnpm storybook")),
    );
}

#[test]
fn app_edit_updates_commands_and_port() {
    let (dir, project) = workspace();
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["app", "edit", "myapp", "--port", "7200", "--command", "deploy=echo deploy"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["app", "show", "myapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("localhost:7200").and(predicate::str::contains("echo deploy")));
}

#[cfg(windows)]
#[test]
fn app_add_canonicalizes_the_drive_letter() {
    let (dir, project) = workspace();
    let display = project.display().to_string();
    // A lowercase drive letter would reach dev servers as a lowercase cwd
    // and break them (Vite resolves imports against it).
    let lowercase = format!("{}{}", display[..1].to_lowercase(), &display[1..]);
    turnout(dir.path()).args(["app", "add", "myapp", "--path", &lowercase]).assert().success();
    let canonical = std::fs::canonicalize(&project).unwrap().display().to_string();
    let canonical = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical).to_string();
    turnout(dir.path())
        .args(["app", "show", "myapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&canonical));
}

#[test]
fn gateway_run_on_a_busy_port_suggests_stopping() {
    let (dir, project) = workspace();
    // Hold the listener that chose the port rather than rebinding its number:
    // releasing it first leaves a window where another process takes the port
    // and the rebind fails (it did, on a macOS runner).
    let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--port", &port.to_string()])
        .assert()
        .success();
    turnout(dir.path())
        .args(["gateway", "run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("turnout gateway stop"));
}

#[test]
fn server_crud_roundtrip() {
    let (dir, _) = workspace();
    turnout(dir.path())
        .args([
            "server",
            "add",
            "staging",
            "--url",
            "https://staging.example.com",
            "--label",
            "Staging",
            "--ssh",
            "deploy@staging.example.com:2200",
            "--insecure",
        ])
        .assert()
        .success();
    turnout(dir.path()).args(["server", "show", "staging"]).assert().success().stdout(
        predicate::str::contains("https://staging.example.com")
            .and(predicate::str::contains("deploy@staging.example.com:2200"))
            .and(predicate::str::contains("accept invalid certificates")),
    );
    turnout(dir.path()).args(["server", "edit", "staging", "--secure"]).assert().success();
    turnout(dir.path())
        .args(["server", "show", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("verify certificates"));
}

#[test]
fn server_add_requires_valid_url() {
    let (dir, _) = workspace();
    turnout(dir.path())
        .args(["server", "add", "bad", "--url", "staging.example.com"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must start with http"));
}

#[test]
fn app_cannot_allow_unknown_server() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--server", "nosuch"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no server named 'nosuch'"));
}

#[test]
fn removing_server_updates_apps() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--server", "staging"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["server", "remove", "staging", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed 'staging' from apps: myapp"));
    turnout(dir.path())
        .args(["app", "show", "myapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("none allowed yet"));
}

/// Pass commands run with the insecure file backend: the OS keyring is
/// per-process-safe but not shareable across the separate CLI invocations
/// a test makes, and CI runners have no unlocked keyring at all.
fn turnout_secrets(data_dir: &std::path::Path) -> Command {
    let mut cmd = turnout(data_dir);
    cmd.env("TURNOUT_KEYRING", "insecure-file");
    cmd
}

fn add_staging(dir: &std::path::Path) {
    turnout(dir)
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
}

fn save_access(dir: &std::path::Path) {
    turnout_secrets(dir)
        .args(["pass", "set", "staging", "--login", "deploy"])
        .write_stdin("s3cret\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("saved"));
}

#[test]
fn pass_set_copy_roundtrip() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    save_access(dir.path());
    turnout_secrets(dir.path())
        .args(["pass", "copy", "staging", "--show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("s3cret"));
    turnout_secrets(dir.path())
        .args(["pass", "copy", "staging", "--login", "--show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy"));
    turnout_secrets(dir.path())
        .args(["pass", "show", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deploy").and(predicate::str::contains("s3cret").not()));
    turnout_secrets(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Access:  saved for staging"));
}

#[test]
fn pass_set_requires_known_server() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    turnout_secrets(dir.path())
        .args(["pass", "set", "nosuch", "--login", "x"])
        .write_stdin("value\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no server named 'nosuch'"));
}

#[test]
fn pass_remove_deletes_secret() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    save_access(dir.path());
    turnout_secrets(dir.path()).args(["pass", "remove", "staging", "--yes"]).assert().success();
    turnout_secrets(dir.path())
        .args(["pass", "copy", "staging", "--show"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no access saved"));
}

#[test]
fn server_remove_purges_access() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    save_access(dir.path());
    turnout_secrets(dir.path())
        .args(["server", "remove", "staging", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed stored access: password"));
    turnout_secrets(dir.path())
        .args(["pass", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No access saved yet"));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn pass_copy_never_prints_the_secret() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    save_access(dir.path());
    turnout_secrets(dir.path())
        .args(["pass", "copy", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("copied to the clipboard").and(predicate::str::contains("s3cret").not()));
}

#[test]
fn use_binds_app_and_shows_in_status() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["use", "myapp", "staging", "--no-check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("'myapp' now uses 'staging'"));
    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("myapp -> staging"));
}

#[test]
fn use_respects_the_allow_list() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    turnout(dir.path())
        .args(["server", "add", "other", "--url", "https://other.example.com"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--server", "staging"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["use", "myapp", "other", "--no-check"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not allowed"));
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn wait_for_port(port: u16) {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    for _ in 0..100 {
        if std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(100)).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("port {port} never came up");
}

/// Kills the gateway child even when an assertion panics.
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
    }
}

/// A tiny "stand": sets a session cookie, echoes cookies back, redirects to itself.
fn spawn_stand(port: u16) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async move {
            use axum::http::HeaderMap;
            use axum::routing::get;
            let base = format!("http://127.0.0.1:{port}");
            let app = axum::Router::new()
                .route(
                    "/hello",
                    get(|| async { ([("set-cookie", "sid=abc123; Path=/; HttpOnly")], "hello from the stand") }),
                )
                .route(
                    "/echo-cookie",
                    get(|headers: HeaderMap| async move { headers.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("none").to_string() }),
                )
                .route(
                    "/login",
                    get(move || async move { (axum::http::StatusCode::FOUND, [("location", format!("{base}/after"))]) }),
                )
                .route(
                    "/ws",
                    get(|ws: axum::extract::ws::WebSocketUpgrade| async move {
                        ws.on_upgrade(|mut socket| async move {
                            while let Some(Ok(message)) = socket.recv().await {
                                if let axum::extract::ws::Message::Text(text) = message {
                                    let reply = axum::extract::ws::Message::Text(format!("echo: {text}").into());
                                    if socket.send(reply).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        })
                    }),
                );
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    wait_for_port(port);
}

#[test]
fn gateway_proxies_with_cookie_jar_and_location_rewrite() {
    let (dir, project) = workspace();
    let stand_port = free_port();
    let gateway_port = free_port();
    spawn_stand(stand_port);

    turnout(dir.path())
        .args(["server", "add", "stand", "--url", &format!("http://127.0.0.1:{stand_port}")])
        .assert()
        .success();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--port", &gateway_port.to_string()])
        .assert()
        .success();
    turnout(dir.path()).args(["use", "myapp", "stand", "--no-check"]).assert().success();

    let child = std::process::Command::new(assert_cmd::cargo::cargo_bin("turnout"))
        .env("TURNOUT_DATA_DIR", dir.path())
        .args(["gateway", "run"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _guard = ChildGuard(child);
    wait_for_port(gateway_port);

    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async move {
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let base = format!("http://127.0.0.1:{gateway_port}");

        // Pass-through works and the stand's cookie never reaches the browser.
        let response = client.get(format!("{base}/hello")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert!(response.headers().get("set-cookie").is_none(), "set-cookie must be stripped");
        assert_eq!(response.text().await.unwrap(), "hello from the stand");

        // The jar sends the captured cookie back to the stand.
        let response = client.get(format!("{base}/echo-cookie")).send().await.unwrap();
        assert_eq!(response.text().await.unwrap(), "sid=abc123");

        // Absolute redirects to the stand come back rewritten to localhost.
        let response = client.get(format!("{base}/login")).send().await.unwrap();
        assert_eq!(response.status(), 302);
        let location = response.headers().get("location").unwrap().to_str().unwrap();
        assert_eq!(location, &format!("http://localhost:{gateway_port}/after"));

        // WebSocket frames travel through the gateway both ways.
        use futures_util::{SinkExt, StreamExt};
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{gateway_port}/ws")).await.unwrap();
        socket.send(tokio_tungstenite::tungstenite::Message::text("ping")).await.unwrap();
        let reply = socket.next().await.unwrap().unwrap();
        assert_eq!(reply.to_text().unwrap(), "echo: ping");
    });
}

#[test]
fn run_executes_app_commands_with_exit_codes() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--command", "hello=echo hi from turnout", "--command", "fail=exit 3"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["run", "hello", "myapp"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hi from turnout"));
    turnout(dir.path()).args(["run", "fail", "myapp"]).assert().code(3);
    turnout(dir.path())
        .args(["run", "nosuch", "myapp"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no 'nosuch' command"));
}

#[test]
fn run_resolves_app_from_current_directory() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["app", "add", "myapp", "--path"])
        .arg(&project)
        .args(["--command", "hello=echo resolved by cwd"])
        .assert()
        .success();
    let nested = project.join("src");
    std::fs::create_dir(&nested).unwrap();
    turnout(dir.path())
        .current_dir(&nested)
        .args(["run", "hello"])
        .assert()
        .success()
        .stdout(predicate::str::contains("resolved by cwd"));
    turnout(dir.path())
        .current_dir(dir.path())
        .args(["run", "hello"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a known app directory"));
}

#[test]
fn server_edit_sets_deploy_targets() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args([
            "server",
            "edit",
            "staging",
            "--deploy-path",
            "myapp=/var/www/myapp",
            "--restart-cmd",
            "myapp=systemctl restart myapp",
        ])
        .assert()
        .success();
    turnout(dir.path())
        .args(["server", "show", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("/var/www/myapp").and(predicate::str::contains("systemctl restart myapp")));
    turnout(dir.path())
        .args(["server", "edit", "staging", "--restart-cmd", "ghost=oops"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("has no deploy path"));
}

#[test]
fn server_edit_manages_the_ssh_key() {
    let (dir, _) = workspace();
    add_staging(dir.path());
    turnout(dir.path())
        .args(["server", "edit", "staging", "--ssh-key", "/home/me/.ssh/id_ed25519"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("set SSH access first"));
    turnout(dir.path())
        .args([
            "server",
            "edit",
            "staging",
            "--ssh",
            "deploy@staging.example.com",
            "--ssh-key",
            "/home/me/.ssh/id_ed25519",
        ])
        .assert()
        .success();
    turnout(dir.path())
        .args(["server", "show", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("key: /home/me/.ssh/id_ed25519"));
    turnout(dir.path()).args(["server", "edit", "staging", "--ssh-key", ""]).assert().success();
    turnout(dir.path())
        .args(["server", "show", "staging"])
        .assert()
        .success()
        .stdout(predicate::str::contains("key:").not());
}

#[test]
fn backup_and_restore_validate_preconditions() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["backup", "myapp", "--server", "staging"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no SSH access"));
    turnout(dir.path())
        .args(["server", "edit", "staging", "--ssh", "deploy@staging.example.com"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["restore", "myapp", "--server", "staging", "--list"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no deploy path"));
}

#[test]
fn deploy_validates_its_preconditions() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["deploy", "myapp", "--no-build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("pass --server or bind"));
    turnout(dir.path())
        .args(["deploy", "myapp", "--server", "staging", "--no-build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no SSH access"));
    turnout(dir.path())
        .args(["server", "edit", "staging", "--ssh", "deploy@staging.example.com"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["deploy", "myapp", "--server", "staging", "--no-build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no deploy path"));
    turnout(dir.path())
        .args(["server", "edit", "staging", "--deploy-path", "myapp=/var/www/myapp"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["deploy", "myapp", "--server", "staging", "--no-build"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no artifact directory"));
}

#[test]
fn group_use_binds_all_members() {
    let (dir, project) = workspace();
    add_staging(dir.path());
    let second = dir.path().join("second");
    std::fs::create_dir(&second).unwrap();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["app", "add", "api", "--path"]).arg(&second).assert().success();
    turnout(dir.path())
        .args(["group", "add", "contour", "--app", "web", "--app", "api"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["use", "contour", "staging", "--no-check"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Group 'contour' now uses 'staging'"));
    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("web -> staging").and(predicate::str::contains("api -> staging")));
    turnout(dir.path())
        .args(["group", "show", "contour"])
        .assert()
        .success()
        .stdout(predicate::str::contains("web -> staging"));
}

#[test]
fn group_names_cannot_clash_with_apps() {
    let (dir, project) = workspace();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["group", "add", "web", "--app", "web"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not clash"));
}

#[test]
fn removing_an_app_updates_groups() {
    let (dir, project) = workspace();
    let second = dir.path().join("second");
    std::fs::create_dir(&second).unwrap();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["app", "add", "api", "--path"]).arg(&second).assert().success();
    turnout(dir.path())
        .args(["group", "add", "contour", "--app", "web", "--app", "api"])
        .assert()
        .success();
    turnout(dir.path())
        .args(["app", "remove", "web", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Removed 'web' from groups: contour"));
    turnout(dir.path())
        .args(["group", "show", "contour"])
        .assert()
        .success()
        .stdout(predicate::str::contains("web").not());
}

/// The completion helper feeds live names to the shell; `use` accepts apps and
/// groups together, so `targets` must carry both.
#[test]
fn complete_lists_live_entity_names() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["group", "add", "contour", "--app", "web"]).assert().success();

    turnout(dir.path()).args(["complete", "apps"]).assert().success().stdout("web\n");
    turnout(dir.path()).args(["complete", "servers"]).assert().success().stdout("staging\n");
    turnout(dir.path())
        .args(["complete", "targets"])
        .assert()
        .success()
        .stdout(predicate::str::contains("web").and(predicate::str::contains("contour")));
}

/// bash gets the dynamic wrapper appended; other shells stay untouched.
#[test]
fn bash_completions_carry_the_dynamic_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["completions", "bash"]).assert().success().stdout(
        predicate::str::contains("_turnout_dynamic").and(predicate::str::contains("-F _turnout_dynamic -o nosort -o bashdefault -o default turnout tn")),
    );
    turnout(dir.path())
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(predicate::str::contains("_turnout_dynamic").not());
}

#[test]
fn completions_cover_the_command_surface() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path())
        .args(["completions", "powershell"])
        .assert()
        .success()
        .stdout(predicate::str::contains("turnout").and(predicate::str::contains("deploy")));
}

/// Pickers only stand in on a terminal; piped runs must keep failing loudly
/// rather than blocking on a prompt nobody can answer.
#[test]
fn missing_names_still_fail_without_a_terminal() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();

    for args in [
        vec!["app", "show"],
        vec!["server", "show"],
        vec!["use"],
        vec!["use", "myapp"],
        vec!["pass", "copy"],
    ] {
        turnout(dir.path())
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("no terminal to prompt on").or(predicate::str::contains("no access saved")));
    }
}

/// Confirmations are prompts too: unattended they must name `--yes` rather than
/// failing with dialoguer's bare "not a terminal", and must remove nothing.
#[test]
fn destructive_commands_explain_how_to_skip_the_prompt() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["group", "add", "contour", "--app", "myapp"]).assert().success();

    for args in [
        vec!["app", "remove", "myapp"],
        vec!["server", "remove", "staging"],
        vec!["group", "remove", "contour"],
    ] {
        turnout(dir.path())
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("--yes").and(predicate::str::contains("no terminal to prompt on")));
    }

    // Nothing was confirmed, so nothing may have been removed.
    turnout(dir.path())
        .args(["app", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("myapp"));
    turnout(dir.path())
        .args(["server", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("staging"));
    turnout(dir.path())
        .args(["group", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("contour"));
}

/// A scripted `pass set` reads the secret from stdin; an empty pipe is a
/// forgotten value, not an intentionally empty secret.
#[test]
fn pass_set_says_when_no_secret_arrived_on_stdin() {
    let (dir, _project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();

    turnout(dir.path())
        .args(["pass", "set", "staging", "--kind", "ssh", "--login", "deploy"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("no secret on stdin"));
}

/// The deploy wizard is interactive by nature; scripts must be told what to
/// use instead rather than hanging on a prompt.
#[test]
fn deploy_setup_refuses_to_run_unattended() {
    let (dir, project) = workspace();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .args(["deploy-setup", "web"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is a wizard").and(predicate::str::contains("turnout server edit")));
}

/// Every state change lands in the journal, and `status` shows the tail.
/// Secrets never do - `pass` is not journaled at all.
#[test]
fn actions_are_journaled_without_secrets() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path()).args(["app", "add", "web", "--path"]).arg(&project).assert().success();
    turnout(dir.path()).args(["use", "web", "staging", "--no-check"]).assert().success();
    turnout(dir.path())
        .args(["pass", "set", "staging", "--login", "deploy"])
        .env("TURNOUT_KEYRING", "insecure-file")
        .write_stdin("hunter2-topsecret")
        .assert()
        .success();

    let journal = std::fs::read_to_string(dir.path().join("journal.jsonl")).unwrap();
    assert!(journal.contains(r#""action":"use""#), "{journal}");
    assert!(journal.contains(r#""action":"app.add""#), "{journal}");
    assert!(!journal.contains("hunter2"), "the secret leaked: {journal}");
    assert!(!journal.contains("deploy"), "the login leaked: {journal}");

    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Recent:").and(predicate::str::contains("web -> staging")));
}

#[test]
fn status_counts_catalogs() {
    let (dir, project) = workspace();
    turnout(dir.path())
        .args(["server", "add", "staging", "--url", "https://staging.example.com"])
        .assert()
        .success();
    turnout(dir.path()).args(["app", "add", "myapp", "--path"]).arg(&project).assert().success();
    turnout(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Apps:    1 (myapp)").and(predicate::str::contains("Servers: 1 (staging)")));
}

/// The update check is off under `CI`, which is exactly where these tests run,
/// so they switch it back on explicitly - otherwise they would pass by
/// checking nothing.
fn with_update_check(data_dir: &std::path::Path) -> Command {
    let mut cmd = turnout(data_dir);
    cmd.env("TURNOUT_UPDATE_CHECK", "1").env_remove("CI");
    cmd
}

/// The hidden helper the background process runs. Pointing it at an
/// unreachable host proves the failure path: the attempt is recorded so the
/// next command does not retry immediately, and no version is invented.
///
/// `setup` runs with the check disabled on purpose. Enabled, it would spawn a
/// background lookup against the real GitHub, and that answer - not the failure
/// under test - is what would land in the cache.
#[test]
fn a_failed_update_lookup_is_recorded_without_a_version() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    with_update_check(dir.path())
        .env("TURNOUT_UPDATE_URL", "http://update-check.invalid/releases/latest")
        .arg("check-update")
        .assert()
        .success();

    let cache = std::fs::read_to_string(dir.path().join("update-check.json")).unwrap();
    assert!(cache.contains("checked_at"), "{cache}");
    assert!(!cache.contains("latest"), "an unreachable host must not yield a version: {cache}");
}

/// A newer version in the cache must never reach piped output: scripts read
/// stdout, and a version notice in the middle of it is noise.
#[test]
fn the_update_notice_stays_out_of_piped_output() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    let cache = format!(r#"{{"checked_at":{},"latest":"99.0.0"}}"#, u64::MAX / 2);
    std::fs::write(dir.path().join("update-check.json"), cache).unwrap();

    with_update_check(dir.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("99.0.0").not())
        .stderr(predicate::str::contains("99.0.0").not());
}

/// Turning the check off means nothing is looked up at all - no cache file
/// appears, so no background process went out to the network.
#[test]
fn the_update_check_can_be_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    turnout(dir.path())
        .env("TURNOUT_UPDATE_CHECK", "off")
        .env_remove("CI")
        .arg("status")
        .assert()
        .success();
    // The spawn is fire-and-forget; give a stray one time to land.
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !dir.path().join("update-check.json").exists(),
        "a disabled check must not reach the network or write a cache"
    );
}

/// A command must not fail because the update check did.
#[test]
fn a_broken_update_check_never_fails_the_command() {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    with_update_check(dir.path())
        .env("TURNOUT_UPDATE_URL", "http://update-check.invalid/releases/latest")
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("Gateway: not running"));
}

/// A binary left over from a previous `self-update` is swept by any later
/// command - not only by `self-update` itself, which nobody runs twice in a
/// row. Until it is swept it costs a full binary worth of disk.
#[test]
fn a_leftover_binary_from_self_update_is_swept() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(if cfg!(windows) { "turnout.exe" } else { "turnout" });
    std::fs::copy(assert_cmd::cargo::cargo_bin("turnout"), &exe).unwrap();

    let mut leftover = exe.clone().into_os_string();
    leftover.push(".old");
    let leftover = std::path::PathBuf::from(leftover);
    std::fs::write(&leftover, b"the previous binary").unwrap();

    // A plain command, unrelated to updating. Not `--version`: clap answers
    // that one itself and exits before any of our code runs.
    let mut cmd = Command::new(&exe);
    cmd.env("TURNOUT_DATA_DIR", dir.path()).env("TURNOUT_UPDATE_CHECK", "0");
    cmd.arg("status").assert().success();

    assert!(!leftover.exists(), "the leftover binary should have been removed");
    assert!(exe.exists(), "the running binary must survive the sweep");
}
