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
