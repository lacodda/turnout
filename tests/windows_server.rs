//! Against a real Windows server: OpenSSH Server answering with `cmd.exe`, as
//! it does out of the box.
//!
//! The unit tests run every Windows command through a real `cmd.exe`, behind
//! an SSH server written for the tests. What they cannot show is the real
//! sshd around it: its sessions, its SFTP server, its profiles, which account
//! it reads which authorized-keys file for and which ACL it accepts on that
//! file. These tests deploy, back up, restore, roll back and install keys
//! against the real thing, and read the result straight off the disk -
//! the server is this machine.
//!
//! Every test is `#[ignore]`d: it needs the server, and a plain `cargo test`
//! has none. CI stands one up on its Windows runner (`tools/ci/windows-sshd.ps1`)
//! and runs `cargo test --test windows_server -- --ignored` on every push and
//! every tag. Without the server's variables the tests fail rather than skip:
//! a proof that quietly did not run reads exactly like one that passed.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;

/// One local account the server lets in with a password.
struct Account {
    user: String,
    password: String,
}

/// The server the CI script stood up, as it described it in the environment.
struct Stand {
    /// `HOST:PORT` of its sshd.
    ssh: String,
    /// A directory the administrators may write to, for deploys to land in.
    dir: PathBuf,
    admin: Account,
    second_admin: Account,
    user: Account,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set - these tests need the Windows server tools/ci/windows-sshd.ps1 stands up, and say so instead of skipping")
    })
}

impl Stand {
    fn from_env() -> Self {
        let account = |prefix: &str| Account {
            user: required(prefix),
            password: required(&format!("{prefix}_PASSWORD")),
        };
        Self {
            ssh: required("TURNOUT_E2E_SSH"),
            dir: PathBuf::from(required("TURNOUT_E2E_DIR")),
            admin: account("TURNOUT_E2E_ADMIN"),
            second_admin: account("TURNOUT_E2E_SECOND_ADMIN"),
            user: account("TURNOUT_E2E_USER"),
        }
    }
}

fn turnout(data: &Path) -> Command {
    let mut cmd = Command::cargo_bin("turnout").unwrap();
    cmd.env("TURNOUT_DATA_DIR", data)
        .env("TURNOUT_UPDATE_CHECK", "0")
        .env("TURNOUT_TOASTS", data.join("toasts.jsonl"))
        // The passwords go to a file in the data directory: what is under
        // test is the server, not the runner's credential store.
        .env("TURNOUT_KEYRING", "insecure-file");
    cmd
}

/// Everything a command printed, both streams, for messages and matching.
fn printed(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

/// Run turnout and insist it succeeded; returns what it printed.
fn ok(data: &Path, args: &[&str]) -> String {
    let output = turnout(data).args(args).output().unwrap();
    let text = printed(&output);
    assert!(output.status.success(), "turnout {} failed ({}):\n{text}", args.join(" "), output.status);
    text
}

/// Run turnout and insist it failed; returns what it printed.
fn fails(data: &Path, args: &[&str]) -> String {
    let output = turnout(data).args(args).output().unwrap();
    let text = printed(&output);
    assert!(!output.status.success(), "turnout {} was expected to fail:\n{text}", args.join(" "));
    text
}

fn json(data: &Path, args: &[&str]) -> serde_json::Value {
    let output = turnout(data).args(args).output().unwrap();
    assert!(output.status.success(), "turnout {} failed:\n{}", args.join(" "), printed(&output));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("{err}: {}", printed(&output)))
}

/// A data directory that knows the server as `win`, and nothing else yet.
fn catalog(stand: &Stand) -> tempfile::TempDir {
    let data = tempfile::tempdir().unwrap();
    ok(data.path(), &["setup", "--yes"]);
    ok(data.path(), &["server", "add", "win", "--url", "http://127.0.0.1", "--host", &stand.ssh]);
    data
}

/// A credential `credential` that signs in as `account` with its password.
fn add_login(data: &Path, credential: &str, account: &Account) {
    ok(data, &["credential", "add", credential, "--user", &account.user, "--auth", "password"]);
    let output = turnout(data)
        .args(["pass", "set", credential])
        .write_stdin(format!("{}\n", account.password))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", printed(&output));
}

/// Backup archives are named to the second; two taken within one second
/// would share a name. Each step that takes one waits for a fresh second, so
/// the counts below are about turnout and not about the clock.
fn next_second() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    std::thread::sleep(std::time::Duration::from_nanos(1_000_000_000 - u64::from(now.subsec_nanos())) + std::time::Duration::from_millis(50));
}

/// Write a build: every file with its exact bytes, nothing else in `dist`.
fn build(dist: &Path, files: &[(&str, &str)]) {
    if dist.exists() {
        std::fs::remove_dir_all(dist).unwrap();
    }
    for (name, body) in files {
        let path = dist.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
}

/// The files of `files` are on the server byte for byte.
fn assert_landed(site: &Path, files: &[(&str, &str)]) {
    for (name, body) in files {
        let path = site.join(name);
        let landed = std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{} did not land: {err}", path.display()));
        assert_eq!(&landed, body, "{} arrived different", path.display());
    }
}

fn assert_absent(site: &Path, name: &str) {
    assert!(!site.join(name).exists(), "{name} should not be in {}", site.display());
}

/// The backup archives beside the deploy directory, oldest first.
fn backups(site: &Path) -> Vec<String> {
    let dir = PathBuf::from(format!("{}.backups", site.display()));
    let mut names: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tar.gz"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// A web server that answers 200 to anything, for the doctor's last check.
fn answering() -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    address
}

const FIRST_BUILD: &[(&str, &str)] = &[
    ("index.html", "<h1>first</h1>\n"),
    ("health.txt", "ok\n"),
    ("404.html", "<h1>not here</h1>\n"),
    ("robots.txt", "User-agent: *\n"),
    ("my notes.txt", "a name with a space\n"),
    ("assets/app.js", "console.log('first');\n"),
    ("assets/app.css", "body { margin: 0 }\n"),
    ("assets/img/logo.svg", "<svg/>\n"),
    ("deep/a/b/c.txt", "nested\n"),
];

const SECOND_BUILD: &[(&str, &str)] = &[
    ("index.html", "<h1>second</h1>\n"),
    ("health.txt", "ok\n"),
    ("404.html", "<h1>not here</h1>\n"),
    ("robots.txt", "User-agent: *\n"),
    ("my notes.txt", "a name with a space\n"),
    ("assets/app.js", "console.log('first');\n"),
    ("assets/app.css", "body { margin: 0 }\n"),
    ("assets/img/logo.svg", "<svg/>\n"),
    ("deep/a/b/c.txt", "nested\n"),
    ("assets/new.js", "console.log('new');\n"),
];

/// A build that fails its check: its `health.txt` does not say `ok`. A file
/// it lacked would not do - unpacked over the stand, the old one would stay.
const BROKEN_BUILD: &[(&str, &str)] = &[
    ("index.html", "<h1>broken</h1>\n"),
    ("health.txt", "down\n"),
    ("404.html", "<h1>not here</h1>\n"),
    ("robots.txt", "User-agent: *\n"),
    ("my notes.txt", "a name with a space\n"),
    ("assets/app.js", "console.log('broken');\n"),
    ("assets/app.css", "body { margin: 0 }\n"),
    ("assets/img/logo.svg", "<svg/>\n"),
    ("deep/a/b/c.txt", "nested\n"),
    ("assets/broken.js", "throw 1;\n"),
];

/// The whole life of a stand on a Windows server, in the order a user lives
/// it: check the server, deploy as one archive, compare, deploy file by file
/// with a backup, back up, break and restore, prune, deploy a build that
/// fails its check and is rolled back, go back to the first deploy, and
/// deploy into a cleared directory.
#[test]
#[ignore = "needs the Windows server tools/ci/windows-sshd.ps1 stands up"]
fn a_windows_server_lives_through_every_kind_of_deploy() {
    let stand = Stand::from_env();
    let data = catalog(&stand);
    let data = data.path();
    // A space in the path, as `C:\Program Files` and `C:\inetpub\my app` have.
    let site = stand.dir.join("every deploy").join("my site");
    let _ = std::fs::remove_dir_all(&site);
    let _ = std::fs::remove_dir_all(format!("{}.backups", site.display()));
    let project = data.join("project");
    let dist = project.join("dist");
    build(&dist, FIRST_BUILD);

    add_login(data, "admin", &stand.admin);
    ok(data, &["server", "edit", "win", "--credential", "admin", "--url", &answering()]);
    // The restart writes a file where it ran: in the deploy directory, which
    // `cd /d` reaches from the profile on C: whatever drive it is on.
    ok(
        data,
        &[
            "path",
            "add",
            "site",
            "--dir",
            &site.display().to_string(),
            "--restart",
            "echo restarted>restarted.txt",
        ],
    );
    ok(data, &["app", "add", "web", "--path", &project.display().to_string(), "--dist", "dist"]);
    ok(
        data,
        &[
            "target",
            "add",
            "web-win",
            "--app",
            "web",
            "--server",
            "win",
            "--credential",
            "admin",
            "--path",
            "site",
        ],
    );

    // cmd.exe answers, and passes its exit code through.
    let said = ok(data, &["exec", "win", "--", "echo", "%COMSPEC%"]);
    assert!(said.to_ascii_lowercase().contains("cmd.exe"), "{said}");
    let output = turnout(data).args(["exec", "win", "--", "exit", "3"]).output().unwrap();
    assert_eq!(output.status.code(), Some(3), "{}", printed(&output));

    // Before the first deploy, the doctor finds nothing in the way.
    let output = turnout(data).args(["doctor", "--server", "win", "--json"]).output().unwrap();
    let view: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|err| panic!("{err}: {}", printed(&output)));
    for check in view["checks"].as_array().unwrap() {
        assert_eq!(check["state"], "ok", "{view:#}");
    }
    assert_eq!(output.status.code(), Some(0), "{view:#}");
    assert!(view["deploy_stops_at"].is_null(), "{view:#}");

    // 1. The first deploy travels as one archive.
    let said = ok(data, &["deploy", "web-win", "--no-build"]);
    assert!(said.contains(&format!("Unpacked {} files into", FIRST_BUILD.len())), "{said}");
    assert_landed(&site, FIRST_BUILD);
    assert!(site.join("restarted.txt").exists(), "the restart did not run in the deploy directory:\n{said}");
    let server = json(data, &["server", "show", "win", "--json"]);
    assert_eq!(server["shell"], "windows", "the probe should have found cmd.exe: {server:#}");
    let said = ok(data, &["exec", "web-win", "--", "cd"]);
    assert!(said.contains(&site.display().to_string()), "exec on a target runs in its directory:\n{said}");

    // The comparison asks the server's files, hashed by .NET there.
    build(&dist, SECOND_BUILD);
    let compared = json(data, &["deploy", "web-win", "--no-build", "--diff", "--json"]);
    let compared = &compared[0];
    assert_eq!(compared["compared"], "content", "{compared:#}");
    assert_eq!(compared["added"], serde_json::json!(["assets/new.js"]), "{compared:#}");
    assert_eq!(compared["updated"], serde_json::json!(["index.html"]), "{compared:#}");
    assert_eq!(compared["unchanged"], FIRST_BUILD.len() - 1, "{compared:#}");
    assert_landed(&site, FIRST_BUILD);
    assert_absent(&site, "assets/new.js");

    // 2. File by file, with a backup of what was there.
    std::fs::remove_file(site.join("restarted.txt")).unwrap();
    let said = ok(data, &["deploy", "web-win", "--no-build", "--no-archive", "--backup"]);
    assert!(!said.contains("Unpacked"), "--no-archive must not pack:\n{said}");
    assert_landed(&site, SECOND_BUILD);
    assert!(site.join("restarted.txt").exists(), "{said}");
    assert_eq!(backups(&site).len(), 1, "{said}");

    // A backup on request, listed newest first.
    next_second();
    ok(data, &["backup", "web-win"]);
    let taken = backups(&site);
    assert_eq!(taken.len(), 2, "{taken:?}");
    let listed = json(data, &["restore", "web-win", "--list", "--json"]);
    let newest_first: Vec<String> = taken.iter().rev().cloned().collect();
    assert_eq!(listed["backups"], serde_json::json!(newest_first), "{listed:#}");

    // Break the stand by hand; the newest backup puts it back exactly,
    // including taking away what was not in it.
    std::fs::remove_file(site.join("index.html")).unwrap();
    std::fs::remove_dir_all(site.join("deep")).unwrap();
    std::fs::write(site.join("stray.txt"), "left by hand\n").unwrap();
    ok(data, &["restore", "web-win"]);
    assert_landed(&site, SECOND_BUILD);
    assert_absent(&site, "stray.txt");

    // Pruning is checked by listing again: `del` on Windows says yes to anything.
    let pruned = json(data, &["backup", "prune", "web-win", "--keep", "1", "--json"]);
    assert_eq!(backups(&site), vec![taken[1].clone()], "{pruned:#}");

    // 3. A build that fails its check is rolled back to what ran before.
    ok(
        data,
        &[
            "target",
            "edit",
            "web-win",
            "--health-command",
            // Not `/x`: findstr matches a whole line only up to a CRLF, and
            // these files end their lines in LF.
            "findstr ok health.txt",
            "--health-wait",
            "4",
        ],
    );
    build(&dist, BROKEN_BUILD);
    next_second();
    let said = fails(data, &["deploy", "web-win", "--no-build"]);
    assert!(said.contains("rolled back to"), "{said}");
    assert!(said.contains("the stand works again"), "{said}");
    assert_landed(&site, SECOND_BUILD);
    assert_absent(&site, "assets/broken.js");

    let history = json(data, &["history", "web-win", "--json"]);
    let outcomes: Vec<(u64, &str)> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|deploy| (deploy["id"].as_u64().unwrap(), deploy["outcome"].as_str().unwrap()))
        .collect();
    assert_eq!(outcomes, vec![(1, "deployed"), (2, "deployed"), (3, "rolled_back")], "{history:#}");

    // 4. Back to the first deploy's build: its files return, and the file the
    //    second deploy brought is taken away - with `del`, on a path written
    //    with forward slashes.
    let said = ok(data, &["rollback", "web-win", "--to", "1"]);
    assert_landed(&site, FIRST_BUILD);
    assert_absent(&site, "assets/new.js");
    assert!(said.contains("Removed 1 file the newer builds added"), "{said}");

    // 5. A cleared directory keeps nothing the build does not have.
    std::fs::write(site.join("stray.txt"), "left by hand\n").unwrap();
    build(&dist, SECOND_BUILD);
    next_second();
    ok(data, &["deploy", "web-win", "--no-build", "--clear"]);
    assert_landed(&site, SECOND_BUILD);
    assert_absent(&site, "stray.txt");
}

/// The private key turnout generated for `credential`, which `key setup`
/// leaves in the home of whoever ran it; removed when the test is done.
struct GeneratedKey(PathBuf);

impl GeneratedKey {
    fn of(credential: &str) -> Self {
        let home = directories::UserDirs::new().unwrap().home_dir().to_path_buf();
        Self(home.join(".ssh").join(format!("id_ed25519_{credential}")))
    }

    fn public(&self) -> String {
        std::fs::read_to_string(format!("{}.pub", self.0.display())).unwrap().trim().to_string()
    }
}

impl Drop for GeneratedKey {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}.pub", self.0.display()));
    }
}

/// A credential name no earlier run left a key file behind for.
fn fresh(prefix: &str) -> String {
    format!("{prefix}-{}", std::process::id())
}

fn credential_auth(data: &Path, credential: &str) -> String {
    json(data, &["credential", "show", credential, "--json"])["auth"].as_str().unwrap().to_string()
}

/// OpenSSH on Windows reads one shared file for every administrator. Two of
/// them set up keys in turn: the second must be able to add to the file the
/// first one created, and sshd must still accept it for both.
#[test]
#[ignore = "needs the Windows server tools/ci/windows-sshd.ps1 stands up"]
fn administrators_share_one_key_file_and_each_signs_in_with_their_own_key() {
    let stand = Stand::from_env();
    let data = catalog(&stand);
    let data = data.path();
    let first = fresh("first");
    add_login(data, &first, &stand.admin);
    let second = "second";
    add_login(data, second, &stand.second_admin);

    // The first administrator's key is generated by turnout.
    let generated = GeneratedKey::of(&first);
    let said = ok(data, &["key", "setup", "win", "--credential", &first]);
    assert!(said.contains("administrators_authorized_keys"), "{said}");
    assert!(said.contains("The key signs in"), "{said}");
    assert_eq!(credential_auth(data, &first), "key");

    // The second brings a key ssh-keygen made, and appends to the same file.
    let keys = tempfile::tempdir().unwrap();
    let own_key = keys.path().join("id_ed25519");
    let status = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "second-admin", "-f"])
        .arg(&own_key)
        .status()
        .unwrap();
    assert!(status.success());
    let said = ok(data, &["key", "setup", "win", "--credential", second, "--key", &own_key.display().to_string()]);
    assert!(said.contains("The key signs in"), "{said}");
    assert_eq!(credential_auth(data, second), "key");

    // Both still sign in - the second install must not have locked out the first.
    ok(data, &["key", "check", "win", "--credential", &first]);
    ok(data, &["key", "check", "win", "--credential", second]);
    let said = ok(data, &["exec", "win", "--credential", &first, "--", "whoami"]);
    assert!(said.to_lowercase().contains(&stand.admin.user.to_lowercase()), "{said}");
    let said = ok(data, &["exec", "win", "--credential", second, "--", "whoami"]);
    assert!(said.to_lowercase().contains(&stand.second_admin.user.to_lowercase()), "{said}");

    // Setting up again changes nothing: the key is recognized, not appended twice.
    let said = ok(data, &["key", "setup", "win", "--credential", &first]);
    assert!(said.contains("already authorized"), "{said}");

    // The file is the administrators' own: any administrator reads it, as
    // this test does, and each key is in it once.
    let shared = std::fs::read_to_string(r"C:\ProgramData\ssh\administrators_authorized_keys").unwrap();
    let own_public = std::fs::read_to_string(format!("{}.pub", own_key.display())).unwrap();
    for public in [generated.public(), own_public.trim().to_string()] {
        let body = public.split_whitespace().nth(1).unwrap();
        assert_eq!(shared.matches(body).count(), 1, "{public} in:\n{shared}");
    }
}

/// An account outside the administrators keeps its key in its own profile.
#[test]
#[ignore = "needs the Windows server tools/ci/windows-sshd.ps1 stands up"]
fn an_ordinary_account_keeps_its_key_in_its_own_profile() {
    let stand = Stand::from_env();
    let data = catalog(&stand);
    let data = data.path();
    let credential = fresh("ordinary");
    add_login(data, &credential, &stand.user);

    let generated = GeneratedKey::of(&credential);
    let said = ok(data, &["key", "setup", "win", "--credential", &credential]);
    assert!(!said.contains("administrators_authorized_keys"), "{said}");
    assert!(said.contains(r"\.ssh\authorized_keys"), "{said}");
    assert!(said.contains("The key signs in"), "{said}");
    assert_eq!(credential_auth(data, &credential), "key");

    // Signed in by key, the account reads its own file back.
    let said = ok(
        data,
        &["exec", "win", "--credential", &credential, "--", r"type %USERPROFILE%\.ssh\authorized_keys"],
    );
    let body = generated.public().split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(said.matches(&body).count(), 1, "{said}");
}
