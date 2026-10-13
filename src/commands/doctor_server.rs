//! `turnout doctor --server NAME`: what would stop a deploy, found before it.
//!
//! A deploy that fails tells you where it failed - after the build, after
//! the connection, sometimes after half the files went up. Most of the
//! reasons were there before it started: sshd is not answering, the password
//! was never stored, the deploy directory belongs to root, the disk is full,
//! the restart needs a sudo password nobody is there to type, the web server
//! itself is down. Each of those is a question the server answers in a
//! second, so the doctor asks them all, as the account each target signs in
//! with, and names the step the first problem would stop a deploy at.
//!
//! Nothing on the server changes. The one write is a probe file created and
//! deleted in the deploy directory - the only honest way to know a directory
//! is writable is to write to it, since permissions, ACLs and read-only mounts
//! each have a say. A directory that does not exist yet is not created; its
//! nearest existing parent is asked instead, since that is where a deploy
//! would create it.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use crate::commands::doctor::CheckView;
use crate::health::Health;
use crate::model::{App, Auth, Credential, Server};
use crate::shell::Dialect;
use crate::ssh::Session;
use crate::{pick, progress, remote, store};

const SERVER: &str = "server";
const PATHS: &str = "paths";
const WEB: &str = "web";

/// How long the TCP probe of sshd waits.
const REACH_TIMEOUT: Duration = Duration::from_secs(5);

/// Below this much free space a deploy is a gamble even when the last build
/// fits: logs and backups grow beside it.
const LOW_SPACE: u64 = 1024 * 1024 * 1024;

/// The deploy steps, in the order a deploy takes them; what a check guards.
const STEPS: [(&str, &[&str]); 4] = [
    ("start", &["artifacts", "credential"]),
    ("connect", &["sshd", "sign_in"]),
    ("upload", &["writable", "disk"]),
    ("restart", &["restart"]),
];

/// The server `--server` names, or the one picked when it names none.
pub fn resolve(name: Option<String>) -> Result<Server> {
    let servers = store::load_servers()?;
    let name = match name {
        Some(name) => name,
        None => {
            pick::ensure_interactive("name the server: turnout doctor --server NAME")?;
            pick::server(&servers, "Server to check")?
        }
    };
    servers
        .into_iter()
        .find(|server| server.name == name)
        .ok_or_else(|| anyhow!("no server named '{name}' - see `turnout server list`"))
}

/// The step a deploy would stop at: the earliest one a failed check guards.
pub fn stops_at(checks: &[CheckView]) -> Option<&'static str> {
    let failed: Vec<&str> = checks
        .iter()
        .filter(|check| check.state == crate::commands::doctor::State::Fail)
        .map(|check| check.check)
        .collect();
    STEPS
        .iter()
        .find(|(_, guarded)| guarded.iter().any(|check| failed.contains(check)))
        .map(|(step, _)| *step)
}

/// One way into the server, and the deploy directories reached through it.
struct Login {
    credential: Credential,
    /// Path name -> the path, for the targets that sign in with this credential.
    paths: BTreeMap<String, (crate::model::Path, Option<App>)>,
}

/// Every check of one server.
pub fn checks(server: &Server) -> Result<Vec<CheckView>> {
    let name = Some(server.name.as_str());
    let apps = store::load_apps()?;
    let credentials = store::load_credentials()?;
    let paths = store::load_paths()?;
    let mut checks = Vec::new();

    let logins = logins(server, &apps, &credentials, &paths, &mut checks);
    // The web server is asked while SSH is busy: they do not depend on each other.
    let web = std::thread::scope(|scope| {
        let web = scope.spawn(|| crate::health::probe_one(server));
        let host = server.ssh_host();
        match reach(&host, server.port) {
            Ok(took) => {
                checks.push(CheckView::ok(
                    SERVER,
                    "sshd",
                    name,
                    format!("sshd answers on {host}:{} in {} ms", server.port, took.as_millis()),
                ));
                for login in &logins {
                    checks.extend(through(server, login));
                }
            }
            Err(why) => checks.push(
                CheckView::fail(SERVER, "sshd", name, why).fix("start sshd there, or check the host and port: turnout server edit NAME --host HOST:PORT"),
            ),
        }
        web.join().unwrap_or(Health::Unreachable)
    });
    checks.push(web_check(server, web));
    Ok(checks)
}

/// The credentials to sign in with: each target's on this server, or the
/// server's own when no target goes there. Problems with them are checks.
fn logins(server: &Server, apps: &[App], credentials: &[Credential], paths: &[crate::model::Path], checks: &mut Vec<CheckView>) -> Vec<Login> {
    let mut logins: BTreeMap<String, Login> = BTreeMap::new();
    let find = |name: &str| credentials.iter().find(|c| c.name == name).cloned();
    for target in store::load_targets().unwrap_or_default().into_iter().filter(|t| t.server == server.name) {
        let Some(credential) = find(&target.credential) else {
            checks.push(
                CheckView::fail(
                    SERVER,
                    "credential",
                    Some(&target.name),
                    format!("signs in as '{}', which is not in the catalog", target.credential),
                )
                .fix(format!("turnout target edit {} --credential NAME", target.name)),
            );
            continue;
        };
        let Some(path) = paths.iter().find(|p| p.name == target.path).cloned() else {
            checks.push(
                CheckView::fail(
                    SERVER,
                    "credential",
                    Some(&target.name),
                    format!("writes to path '{}', which is not in the catalog", target.path),
                )
                .fix(format!("turnout target edit {} --path NAME", target.name)),
            );
            continue;
        };
        let app = apps.iter().find(|a| a.name == target.app).cloned();
        if let Some(app) = &app
            && app.dist_dir.is_none()
        {
            checks.push(
                CheckView::fail(
                    SERVER,
                    "artifacts",
                    Some(&target.name),
                    format!("'{}' names no build directory - a deploy has nothing to send", app.name),
                )
                .fix(format!("turnout app edit {} --dist DIR", app.name)),
            );
        }
        logins
            .entry(credential.name.clone())
            .or_insert_with(|| Login {
                credential,
                paths: BTreeMap::new(),
            })
            .paths
            .insert(path.name.clone(), (path, app));
    }
    if logins.is_empty() {
        match server.credential.as_deref().map(|name| (name, find(name))) {
            Some((_, Some(credential))) => {
                logins.insert(
                    credential.name.clone(),
                    Login {
                        credential,
                        paths: BTreeMap::new(),
                    },
                );
            }
            Some((missing, None)) => checks.push(
                CheckView::fail(
                    SERVER,
                    "credential",
                    Some(&server.name),
                    format!("logs in as '{missing}', which is not in the catalog"),
                )
                .fix(format!("turnout server edit {} --credential NAME", server.name)),
            ),
            None => checks.push(
                CheckView::fail(
                    SERVER,
                    "credential",
                    Some(&server.name),
                    "has no credential and no target - there is nobody to sign in as",
                )
                .fix(format!("turnout server edit {} --credential NAME, or turnout deploy-setup", server.name)),
            ),
        }
    }
    logins.into_values().collect()
}

/// Whether anything answers on the SSH port, and how fast.
fn reach(host: &str, port: u16) -> std::result::Result<Duration, String> {
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| format!("{host} does not resolve - the name is wrong, or the VPN is off"))?
        .collect();
    let started = Instant::now();
    for address in &addresses {
        if std::net::TcpStream::connect_timeout(address, REACH_TIMEOUT).is_ok() {
            return Ok(started.elapsed());
        }
    }
    Err(format!(
        "nothing answers on {host}:{port} within {}s - sshd is down, or a firewall is in the way",
        REACH_TIMEOUT.as_secs()
    ))
}

/// Everything asked as one credential: the sign-in, the shell, tar, and each
/// deploy directory it writes to.
fn through(server: &Server, login: &Login) -> Vec<CheckView> {
    let credential = &login.credential;
    let who = Some(credential.name.as_str());
    let mut checks = Vec::new();
    let session = match Session::connect(server, credential) {
        Ok(session) => session,
        Err(err) => {
            let fix = match credential.auth {
                Auth::Password => format!("turnout pass set {} - or set up a key: turnout key setup {}", credential.name, server.name),
                Auth::Key => format!("turnout key setup {} --credential {}", server.name, credential.name),
                Auth::Agent => "add the key to the agent: ssh-add".to_string(),
            };
            checks.push(CheckView::fail(SERVER, "sign_in", who, format!("{}@ cannot sign in: {err:#}", credential.user)).fix(fix));
            return checks;
        }
    };
    let dialect = remote::dialect(&session, server);
    let how = match credential.auth {
        Auth::Password => "password".to_string(),
        Auth::Key => format!("key {}", credential.key.as_deref().unwrap_or_default()),
        Auth::Agent => "the SSH agent".to_string(),
    };
    let shell = match dialect {
        Dialect::Posix => "sh",
        Dialect::Windows => "cmd.exe",
    };
    checks.push(CheckView::ok(
        SERVER,
        "sign_in",
        who,
        format!("{}@ signs in with {how} · shell: {shell}", credential.user),
    ));
    match remote::exec(&session, "tar --version") {
        Ok(version) => checks.push(CheckView::ok(
            SERVER,
            "tar",
            who,
            format!("tar · {}", version.lines().next().unwrap_or_default().trim()),
        )),
        Err(_) => checks.push(CheckView::warn(
            SERVER,
            "tar",
            who,
            "no tar answers - a deploy sends the files one by one, which is slower",
        )),
    }
    for (path, app) in login.paths.values() {
        checks.extend(path_checks(&session, dialect, &credential.user, path, app.as_ref()));
    }
    checks
}

/// What the server says about one deploy directory.
#[derive(Debug, Default, PartialEq, Eq)]
struct Probed {
    /// The directory itself, or its nearest existing parent.
    dir: String,
    exists: bool,
    writable: bool,
    owner: Option<String>,
    /// Kibibytes free on its file system.
    free_kib: Option<u64>,
    user: Option<String>,
}

/// Read the `key=value` lines the probe script prints.
fn read_probe(output: &str) -> Probed {
    let mut probed = Probed::default();
    for line in output.lines() {
        let Some((key, value)) = line.trim().split_once('=') else { continue };
        let value = value.trim();
        match key {
            "dir" => probed.dir = value.to_string(),
            "exists" => probed.exists = value == "yes",
            "writable" => probed.writable = value == "yes",
            "owner" if !value.is_empty() => probed.owner = Some(value.to_string()),
            "free" => probed.free_kib = value.parse().ok(),
            "user" if !value.is_empty() => probed.user = Some(value.to_string()),
            _ => {}
        }
    }
    probed
}

/// The probe of one directory, as one command in the server's dialect.
///
/// POSIX runs it under `sh` explicitly: the login shell may be fish or
/// something else that does not read `while ... do ... done`. Windows runs it
/// in PowerShell, handed over encoded - `cmd.exe` has no quoting that would
/// carry a script through intact, and base64 has nothing to quote.
fn probe_command(dialect: Dialect, dir: &str) -> String {
    match dialect {
        Dialect::Posix => {
            let target = dialect.quote(dir);
            let script = format!(
                "d={target}; while [ ! -d \"$d\" ]; do p=$(dirname \"$d\"); [ \"$p\" = \"$d\" ] && break; d=$p; done; \
                 echo \"dir=$d\"; if [ -d {target} ]; then echo exists=yes; else echo exists=no; fi; \
                 f=\"$d/.turnout-doctor-$$\"; if ( : > \"$f\" ) 2>/dev/null; then rm -f \"$f\"; echo writable=yes; else echo writable=no; fi; \
                 echo \"owner=$(ls -ld \"$d\" | awk '{{print $3}}')\"; \
                 echo \"free=$(df -Pk \"$d\" | awk 'NR==2 {{print $4}}')\"; \
                 echo \"user=$(id -un)\""
            );
            format!("sh -c {}", dialect.quote(&script))
        }
        Dialect::Windows => {
            let target = dir.replace('\'', "''");
            let script = format!(
                "$ErrorActionPreference = 'SilentlyContinue'\n\
                 $target = '{target}'\n\
                 $d = $target\n\
                 while ($d -and -not (Test-Path -LiteralPath $d -PathType Container)) {{ $d = Split-Path -Parent $d }}\n\
                 'dir=' + $d\n\
                 if (Test-Path -LiteralPath $target -PathType Container) {{ 'exists=yes' }} else {{ 'exists=no' }}\n\
                 $f = Join-Path $d ('.turnout-doctor-' + $PID)\n\
                 try {{ [IO.File]::WriteAllText($f, ''); Remove-Item -LiteralPath $f -Force; 'writable=yes' }} catch {{ 'writable=no' }}\n\
                 try {{ 'owner=' + (Get-Acl -LiteralPath $d).Owner }} catch {{ }}\n\
                 try {{ 'free=' + [math]::Floor((New-Object IO.DriveInfo ([IO.Path]::GetPathRoot($d))).AvailableFreeSpace / 1024) }} catch {{ }}\n\
                 'user=' + [Environment]::UserName\n"
            );
            crate::shell::powershell(&script)
        }
    }
}

fn path_checks(session: &Session, dialect: Dialect, user: &str, path: &crate::model::Path, app: Option<&App>) -> Vec<CheckView> {
    let subject = Some(path.name.as_str());
    let mut checks = Vec::new();
    if let Err(reason) = dialect.reject_unquotable(&path.dir) {
        checks.push(CheckView::fail(PATHS, "writable", subject, reason).fix(format!("turnout path edit {} --dir DIR", path.name)));
        return checks;
    }
    let probed = match remote::exec(session, &probe_command(dialect, &path.dir)) {
        Ok(output) => read_probe(&output),
        Err(err) => {
            checks.push(CheckView::warn(
                PATHS,
                "writable",
                subject,
                format!("{} could not be asked about: {err:#}", path.dir),
            ));
            return checks;
        }
    };
    let user = probed.user.clone().unwrap_or_else(|| user.to_string());
    let owner = probed.owner.as_deref().map(|owner| format!(", owned by {owner}")).unwrap_or_default();
    checks.push(match (probed.exists, probed.writable) {
        (true, true) => CheckView::ok(PATHS, "writable", subject, format!("{} is writable for {user}", path.dir)),
        (false, true) => CheckView::ok(
            PATHS,
            "writable",
            subject,
            format!("{} is not there yet - a deploy creates it under {}, writable for {user}", path.dir, probed.dir),
        ),
        (true, false) => {
            CheckView::fail(PATHS, "writable", subject, format!("{} is not writable for {user}{owner}", path.dir)).fix(grant(dialect, &path.dir, &user))
        }
        (false, false) => CheckView::fail(
            PATHS,
            "writable",
            subject,
            format!(
                "{} is not there, and {} is not writable for {user}{owner} - a deploy cannot create it",
                path.dir, probed.dir
            ),
        )
        .fix(create(dialect, &path.dir, &user)),
    });
    checks.push(disk_check(path, &probed, app));
    if let Some(restart) = &path.restart {
        checks.push(restart_check(session, dialect, path, restart, &user, probed.exists));
    }
    checks
}

/// The command that gives `user` the right to write into `dir`.
fn grant(dialect: Dialect, dir: &str, user: &str) -> String {
    match dialect {
        Dialect::Posix => format!("sudo chown -R {user} {}", dialect.quote(dir)),
        Dialect::Windows => format!("icacls \"{dir}\" /grant {user}:(OI)(CI)M"),
    }
}

/// The command that creates `dir` for `user`.
fn create(dialect: Dialect, dir: &str, user: &str) -> String {
    match dialect {
        Dialect::Posix => format!("sudo mkdir -p {0} && sudo chown {user} {0}", dialect.quote(dir)),
        Dialect::Windows => format!("mkdir \"{dir}\" && icacls \"{dir}\" /grant {user}:(OI)(CI)M"),
    }
}

/// Free space where the deploy lands, against what it brings: the archive
/// and the files unpacked from it sit there side by side for a moment.
fn disk_check(path: &crate::model::Path, probed: &Probed, app: Option<&App>) -> CheckView {
    let subject = Some(path.name.as_str());
    let Some(free_kib) = probed.free_kib else {
        return CheckView::warn(PATHS, "disk", subject, format!("the free space at {} could not be read", probed.dir));
    };
    let free = free_kib.saturating_mul(1024);
    let built = app.and_then(|app| Some((app, app.dist_dir.as_deref()?))).and_then(|(app, dist)| {
        let dir = Path::new(&app.path).join(dist);
        dir.is_dir().then(|| size_of_dir(&dir))
    });
    let need = built.map(|size| size.saturating_mul(2));
    let last = built
        .map(|size| format!(" (the last build is {})", progress::human_bytes(size)))
        .unwrap_or_default();
    match need {
        Some(need) if free < need => CheckView::fail(
            PATHS,
            "disk",
            subject,
            format!(
                "{} free at {} - a deploy needs about {}{last}",
                progress::human_bytes(free),
                probed.dir,
                progress::human_bytes(need)
            ),
        )
        .fix("free space on the server: old backups sit beside the deploy directory, in .backups"),
        _ if free < LOW_SPACE => CheckView::warn(
            PATHS,
            "disk",
            subject,
            format!("only {} free at {}{last}", progress::human_bytes(free), probed.dir),
        )
        .fix("free space on the server: old backups sit beside the deploy directory, in .backups"),
        _ => CheckView::ok(PATHS, "disk", subject, format!("{} free{last}", progress::human_bytes(free))),
    }
}

/// The bytes of every file under `dir`.
fn size_of_dir(dir: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push(entry.path()),
                Ok(_) => total += entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    total
}

/// `cmd.exe` commands that are no program on disk, and so are never found by `where`.
const CMD_BUILTINS: [&str; 22] = [
    "call", "cd", "chdir", "cls", "copy", "del", "dir", "echo", "erase", "exit", "for", "if", "md", "mkdir", "move", "popd", "pushd", "rd", "ren", "rmdir",
    "set", "start",
];

/// Whether the restart command can run as the deploy runs it: unattended,
/// as this account, in the deploy directory.
fn restart_check(session: &Session, dialect: Dialect, path: &crate::model::Path, restart: &str, user: &str, dir_exists: bool) -> CheckView {
    let subject = Some(path.name.as_str());
    let (sudo, programs) = restart_programs(dialect, restart);
    if sudo && dialect == Dialect::Posix && remote::exec(session, "sudo -n true").is_err() {
        return CheckView::fail(
            PATHS,
            "restart",
            subject,
            format!("restart `{restart}` - sudo asks {user} for a password, and a deploy has nobody to type it"),
        )
        .fix(format!("allow the command without a password for {user}: a NOPASSWD rule in sudoers"));
    }
    if programs.is_empty() {
        return CheckView::ok(PATHS, "restart", subject, format!("restart `{restart}` - shell built-ins only"));
    }
    if !dir_exists {
        return CheckView::info(
            PATHS,
            "restart",
            subject,
            format!(
                "restart `{restart}` runs in {}, which a deploy creates first - asked after the first deploy",
                path.dir
            ),
        );
    }
    let missing: Vec<&str> = programs
        .iter()
        .filter(|program| {
            let lookup = match dialect {
                Dialect::Posix => format!("sh -c {}", dialect.quote(&format!("command -v {}", dialect.quote(program)))),
                Dialect::Windows => format!("where {}", dialect.quote(program)),
            };
            remote::exec(session, &dialect.run_in(&path.dir, &lookup)).is_err()
        })
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        let there = if programs.len() == 1 { "is there" } else { "are there" };
        return CheckView::ok(PATHS, "restart", subject, format!("restart `{restart}` - {} {there}", programs.join(", ")));
    }
    CheckView::fail(
        PATHS,
        "restart",
        subject,
        format!(
            "restart `{restart}` - {} {} not found for {user}",
            missing.join(", "),
            if missing.len() == 1 { "is" } else { "are" }
        ),
    )
    .fix(format!("install it there, or turnout path edit {} --restart CMD", path.name))
}

/// Shell built-ins: no program on disk, nothing to look for.
const POSIX_BUILTINS: [&str; 14] = [
    "cd", "echo", "export", "set", "unset", "true", "false", "exit", "test", "[", ":", ".", "source", "eval",
];

/// The programs a restart command runs, and whether any goes through sudo.
///
/// A restart is often a chain - `cd /srv/app && docker compose up -d` - and
/// its first word is a built-in that is always there. Each command of the
/// chain is looked at, split where the shell would split it (`&&`, `||`, `;`,
/// `|`, outside quotes), and the built-ins are left out.
fn restart_programs(dialect: Dialect, command: &str) -> (bool, Vec<String>) {
    let mut sudo = false;
    let mut programs: Vec<String> = Vec::new();
    for part in commands_of(command) {
        let Some((uses_sudo, program)) = restart_program(&part) else { continue };
        sudo |= uses_sudo;
        let builtin = match dialect {
            Dialect::Posix => POSIX_BUILTINS.contains(&program.as_str()),
            Dialect::Windows => CMD_BUILTINS.contains(&program.to_ascii_lowercase().as_str()),
        };
        if !builtin && !programs.contains(&program) {
            programs.push(program);
        }
    }
    (sudo, programs)
}

/// The commands of a chain, split on `&&`, `||`, `;`, `|` and `&` outside
/// quotes.
fn commands_of(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(open), c) if c == open => {
                quote = None;
                current.push(c);
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                current.push(c);
            }
            (None, ';' | '|' | '&') => {
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                parts.push(std::mem::take(&mut current));
            }
            (None, c) => current.push(c),
        }
    }
    parts.push(current);
    parts.into_iter().map(|part| part.trim().to_string()).filter(|part| !part.is_empty()).collect()
}

/// The program a restart command starts, and whether it goes through sudo:
/// `sudo -u web pm2 restart api` -> (true, `pm2`). Environment assignments in
/// front of it are the command's, and sudo's own options - with the
/// argument of those that take one - are sudo's.
fn restart_program(command: &str) -> Option<(bool, String)> {
    /// sudo options that take the next word as their argument.
    const TAKES: [&str; 8] = ["-u", "-g", "-C", "-h", "-p", "-r", "-t", "-U"];
    let mut words = command.split_whitespace();
    let mut sudo = false;
    let mut word = words.next()?;
    loop {
        if word == "sudo" && !sudo {
            sudo = true;
            word = words.next()?;
            while word.starts_with('-') {
                let takes = TAKES.contains(&word);
                word = words.next()?;
                if takes {
                    word = words.next()?;
                }
            }
            continue;
        }
        if word.contains('=') && !word.starts_with('-') {
            word = words.next()?;
            continue;
        }
        break;
    }
    Some((sudo, word.trim_matches(['"', '\'']).to_string()))
}

fn web_check(server: &Server, health: Health) -> CheckView {
    let name = Some(server.name.as_str());
    let url = crate::health::health_url(&server.url);
    match health {
        Health::Up { status, millis } => CheckView::ok(WEB, "web", name, format!("the web server answers · {url} {status} in {millis} ms")),
        Health::Down { status } => CheckView::fail(
            WEB,
            "web",
            name,
            format!("the web server answers {status} at {url} - the application behind it is down"),
        ),
        Health::Unreachable => CheckView::fail(
            WEB,
            "web",
            name,
            format!("the web server does not answer at {url} - it is off, or out of reach"),
        ),
        Health::Untrusted => CheckView::warn(
            WEB,
            "web",
            name,
            format!("the web server at {url} answers with a certificate this machine does not trust"),
        )
        .fix(format!("turnout server edit {} --insecure", server.name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::doctor::State;

    #[test]
    fn the_probe_reads_back_what_the_script_prints() {
        let probed = read_probe("dir=/var/www\nexists=no\nwritable=no\nowner=root\nfree=1048576\nuser=deploy\n");
        assert_eq!(
            probed,
            Probed {
                dir: "/var/www".into(),
                exists: false,
                writable: false,
                owner: Some("root".into()),
                free_kib: Some(1_048_576),
                user: Some("deploy".into()),
            }
        );
        // PowerShell's CRLF and a missing owner line read the same way.
        let windows = read_probe("dir=C:\\inetpub\\api\r\nexists=yes\r\nwritable=yes\r\nfree=35651584\r\nuser=deploy\r\n");
        assert!(windows.exists && windows.writable);
        assert_eq!(windows.owner, None);
        assert_eq!(windows.free_kib, Some(35_651_584));
    }

    /// The POSIX probe goes through `sh`, whatever the login shell, and the
    /// directory reaches it as one quoted word.
    #[test]
    fn the_posix_probe_runs_under_sh_with_the_directory_quoted() {
        let command = probe_command(Dialect::Posix, "/srv/my app");
        assert!(command.starts_with("sh -c '"), "{command}");
        assert!(command.contains("/srv/my app"), "{command}");
        let windows = probe_command(Dialect::Windows, "C:\\it's");
        assert!(windows.starts_with("powershell -NoProfile -NonInteractive -EncodedCommand "), "{windows}");
        assert!(!windows.contains('\''), "the script travels encoded: {windows}");
    }

    #[test]
    fn every_program_of_a_chained_restart_is_looked_for() {
        assert_eq!(
            restart_programs(Dialect::Posix, "cd /srv/app && docker compose pull && docker compose up -d"),
            (false, vec!["docker".to_string()])
        );
        assert_eq!(
            restart_programs(Dialect::Posix, "pm2 reload api || sudo systemctl restart api; echo done | tee log"),
            (true, vec!["pm2".to_string(), "systemctl".to_string(), "tee".to_string()])
        );
        // A separator inside quotes is part of an argument.
        assert_eq!(
            restart_programs(Dialect::Posix, "sh -c 'a && b' && node x.js"),
            (false, vec!["sh".to_string(), "node".to_string()])
        );
        assert_eq!(
            restart_programs(Dialect::Windows, "cd /d C:\\site & iisreset"),
            (false, vec!["iisreset".to_string()])
        );
        assert_eq!(restart_programs(Dialect::Posix, "cd /srv && echo ok"), (false, Vec::new()));
    }

    #[test]
    fn the_restart_program_is_found_past_sudo_and_assignments() {
        assert_eq!(restart_program("pm2 restart api"), Some((false, "pm2".into())));
        assert_eq!(restart_program("sudo systemctl restart api"), Some((true, "systemctl".into())));
        assert_eq!(restart_program("sudo -u web -n pm2 reload all"), Some((true, "pm2".into())));
        assert_eq!(restart_program("NODE_ENV=production ./restart.sh"), Some((false, "./restart.sh".into())));
        assert_eq!(restart_program("iisreset"), Some((false, "iisreset".into())));
        assert_eq!(restart_program("sudo"), None);
        assert_eq!(restart_program(""), None);
    }

    fn failed(check: &'static str) -> CheckView {
        CheckView::fail(PATHS, check, None, "x")
    }

    /// The step named is the earliest the failures guard, whatever order they
    /// were found in.
    #[test]
    fn a_deploy_stops_at_the_earliest_step_a_failure_guards() {
        assert_eq!(stops_at(&[failed("restart"), failed("writable")]), Some("upload"));
        assert_eq!(stops_at(&[failed("disk"), failed("sign_in")]), Some("connect"));
        assert_eq!(stops_at(&[failed("restart")]), Some("restart"));
        assert_eq!(stops_at(&[failed("artifacts"), failed("sshd")]), Some("start"));
        assert_eq!(stops_at(&[failed("web")]), None, "a down web server stops no deploy step");
        let warned = CheckView::warn(PATHS, "disk", None, "x");
        assert_eq!(warned.state, State::Warn);
        assert_eq!(stops_at(&[warned]), None);
    }

    /// The probe scripts run for real: through an SSH session into this
    /// machine's own shell - sh here on Linux and macOS, cmd and PowerShell
    /// on Windows - the way sshd runs them on a server.
    #[test]
    fn the_probe_scripts_answer_through_a_real_shell() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        assert_eq!(dialect, if cfg!(windows) { Dialect::Windows } else { Dialect::Posix });
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("site");
        std::fs::create_dir(&existing).unwrap();
        let dir = existing.display().to_string();

        let probed = read_probe(&remote::exec(&session, &probe_command(dialect, &dir)).unwrap());
        assert!(probed.exists && probed.writable, "{probed:?}");
        assert!(probed.free_kib.is_some_and(|free| free > 0), "{probed:?}");
        assert!(probed.user.is_some(), "{probed:?}");
        assert_eq!(std::fs::read_dir(&existing).unwrap().count(), 0, "the probe file stayed behind");

        // A directory a deploy would create: its nearest parent answers.
        let missing = existing.join("not").join("yet").display().to_string();
        let probed = read_probe(&remote::exec(&session, &probe_command(dialect, &missing)).unwrap());
        assert!(!probed.exists && probed.writable, "{probed:?}");
        assert_eq!(std::path::Path::new(&probed.dir), existing.as_path(), "{probed:?}");

        let path = crate::model::Path {
            name: "site".into(),
            dir: dir.clone(),
            restart: Some(if cfg!(windows) { "cmd /C exit 0".into() } else { "sh -c true".into() }),
        };
        let checks = path_checks(&session, dialect, "deploy", &path, None);
        let state = |check: &str| checks.iter().find(|c| c.check == check).map(|c| c.state);
        assert_eq!(state("writable"), Some(State::Ok), "{checks:?}");
        assert!(matches!(state("disk"), Some(State::Ok | State::Warn)), "{checks:?}");
        assert_eq!(state("restart"), Some(State::Ok), "{checks:?}");

        let gone = crate::model::Path {
            restart: Some("no-such-program-anywhere --restart".into()),
            ..path
        };
        let checks = path_checks(&session, dialect, "deploy", &gone, None);
        let restart = checks.iter().find(|c| c.check == "restart").unwrap();
        assert_eq!(restart.state, State::Fail, "{checks:?}");
        assert!(restart.message.contains("no-such-program-anywhere is not found"), "{}", restart.message);
    }

    /// A directory the account cannot write to fails, with the command that
    /// grants it. Unix only: there a mode takes the right away, and a root
    /// user - who writes anywhere - skips the test rather than passing it.
    #[cfg(unix)]
    #[test]
    fn a_directory_the_account_cannot_write_to_fails_with_the_grant() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let path = crate::model::Path {
            name: "site".into(),
            dir: locked.display().to_string(),
            restart: None,
        };
        let checks = path_checks(&session, Dialect::Posix, "deploy", &path, None);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let writable = checks.iter().find(|c| c.check == "writable").unwrap();
        assert_eq!(writable.state, State::Fail, "{checks:?}");
        assert!(writable.fix.as_deref().is_some_and(|fix| fix.starts_with("sudo chown -R")), "{writable:?}");
        assert_eq!(stops_at(&checks), Some("upload"));
    }

    #[test]
    fn the_fixes_speak_the_server_s_dialect() {
        assert_eq!(grant(Dialect::Posix, "/var/www/app", "deploy"), "sudo chown -R deploy '/var/www/app'");
        assert_eq!(
            grant(Dialect::Windows, "C:\\inetpub\\api", "deploy"),
            "icacls \"C:\\inetpub\\api\" /grant deploy:(OI)(CI)M"
        );
        assert!(create(Dialect::Posix, "/srv/x", "deploy").starts_with("sudo mkdir -p '/srv/x'"));
    }
}
