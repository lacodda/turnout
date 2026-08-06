use assert_cmd::Command;
use predicates::prelude::*;

fn turnout(data_dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("turnout").unwrap();
    cmd.env("TURNOUT_DATA_DIR", data_dir);
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
