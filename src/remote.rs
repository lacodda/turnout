use std::io::Read;
use std::net::TcpStream;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use ssh2::Session;

use crate::model::{App, DeployTarget, Server, Ssh};
use crate::shell::{self, Dialect};
use crate::{secrets, store};

pub struct Target {
    pub app: App,
    pub server: Server,
}

/// Resolve the app (argument or cwd) and the target server (flag or binding),
/// honoring the app's server allow-list.
pub fn resolve(app_name: Option<String>, server_name: Option<String>) -> Result<Target> {
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, app_name)?.clone();
    let server_name = match server_name {
        Some(name) => name,
        None => store::load_state()?
            .bindings
            .get(&app.name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no target: pass --server or bind one with `turnout use {} SERVER`", app.name))?,
    };
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    if !app.servers.is_empty() && !app.servers.contains(&server.name) {
        bail!("server '{server_name}' is not allowed for '{}' (allowed: {})", app.name, app.servers.join(", "));
    }
    Ok(Target { app, server })
}

pub fn require_ssh(server: &Server) -> Result<&Ssh> {
    server.ssh.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "server '{0}' has no SSH access - set it with `turnout server edit {0} --ssh USER@HOST[:PORT]`",
            server.name
        )
    })
}

pub fn require_deploy_target<'a>(server: &'a Server, app_name: &str) -> Result<&'a DeployTarget> {
    server.deploy.get(app_name).ok_or_else(|| {
        anyhow::anyhow!(
            "no deploy path for '{app_name}' on '{0}' - set it with `turnout server edit {0} --deploy-path {app_name}=DIR`",
            server.name
        )
    })
}

/// Configured key file first, then agent keys (ssh-agent / Pageant),
/// then the keyring password (kind `ssh`, falling back to `password`).
pub fn connect(ssh: &Ssh, server_name: &str) -> Result<Session> {
    let stream = TcpStream::connect((ssh.host.as_str(), ssh.port)).with_context(|| format!("cannot reach {}:{}", ssh.host, ssh.port))?;
    let mut session = Session::new()?;
    session.set_tcp_stream(stream);
    session.handshake().context("SSH handshake failed")?;

    if let Some(key) = &ssh.key {
        session
            .userauth_pubkey_file(&ssh.user, None, Path::new(key), None)
            .with_context(|| format!("key auth with {key} failed"))?;
        return Ok(session);
    }
    let _ = session.userauth_agent(&ssh.user);
    if !session.authenticated() {
        let password = secrets::get(server_name, "ssh")
            .or_else(|_| secrets::get(server_name, "password"))
            .map_err(|_| anyhow::anyhow!("agent auth failed and no password stored - save one with `turnout pass set {server_name} --kind ssh`"))?;
        session.userauth_password(&ssh.user, &password).context("SSH password authentication failed")?;
    }
    Ok(session)
}

/// Run a remote command and return its stdout; a non-zero exit becomes an error
/// carrying the remote stderr.
pub fn exec(session: &Session, command: &str) -> Result<String> {
    let mut channel = session.channel_session()?;
    channel.exec(command).with_context(|| format!("cannot run remote command '{command}'"))?;
    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    let mut stderr = String::new();
    channel.stderr().read_to_string(&mut stderr)?;
    channel.wait_close()?;
    let code = channel.exit_status()?;
    if code != 0 {
        bail!("remote command '{command}' exited with {code}: {}", stderr.trim());
    }
    Ok(output)
}

/// Which shell answers on this server, asking it only when we do not know yet.
///
/// The answer is cached in the server entry: sshd's shell does not change
/// between two deploys, and a round trip per command would be a tax paid on
/// every Linux server to serve the rarer Windows one.
///
/// A failed probe is not an error. It means we could not ask, and the honest
/// fallback is the assumption every release before this one made unconditionally
/// - POSIX. Deploys on Unix keep working even if the probe itself breaks.
pub fn dialect(session: &Session, server: &Server) -> Dialect {
    if let Some(known) = server.shell {
        return known;
    }
    let Ok(reply) = exec(session, shell::PROBE) else {
        return Dialect::default();
    };
    let learned = shell::read_probe(&reply);
    remember_dialect(&server.name, learned);
    learned
}

/// Persist a probed dialect, best-effort.
///
/// Failing to write the catalog must not fail the deploy that is already in
/// flight: the cost is one extra probe next time, which is a round trip, not a
/// broken command.
fn remember_dialect(server_name: &str, dialect: Dialect) {
    let Ok(mut servers) = store::load_servers() else {
        return;
    };
    let Some(entry) = servers.iter_mut().find(|s| s.name == server_name) else {
        return;
    };
    entry.shell = Some(dialect);
    let _ = store::save_servers(&servers);
}

/// Check every value that is about to be spliced into a remote command.
///
/// `cmd.exe` has no escape for a double quote and expands anything between
/// percent signs, so a path carrying either cannot be sent as a literal. Saying
/// so plainly beats sending a command that silently targets a different
/// directory.
pub fn check_quotable(dialect: Dialect, values: &[&str]) -> Result<()> {
    for value in values {
        if let Err(reason) = dialect.reject_unquotable(value) {
            bail!("cannot run this on a Windows server: {reason}");
        }
    }
    Ok(())
}

/// Backups live next to the deploy directory: `{path}.backups/`.
///
/// The separator follows the path: a Windows deploy path is written with
/// backslashes and its backups directory has to match, or the server gets a
/// mixed path that only some tools accept.
pub fn backups_dir(deploy_path: &str) -> String {
    format!("{}.backups", deploy_path.trim_end_matches(['/', '\\']))
}

/// Join a directory and a name with the separator that path already uses.
pub fn join_remote(dir: &str, name: &str) -> String {
    let separator = if dir.contains('\\') && !dir.contains('/') { '\\' } else { '/' };
    format!("{}{separator}{name}", dir.trim_end_matches(['/', '\\']))
}

/// The archive name for a backup taken now: `20260812-181500.tar.gz`.
///
/// Built locally rather than on the server. The old command asked the server
/// for the time with `ts=$(date +%Y%m%d-%H%M%S)`, which is POSIX-only syntax:
/// `cmd.exe` has no command substitution and would have taken it literally.
/// Choosing the name here also means the caller knows it without parsing it
/// back out of the command's output.
///
/// UTC, so that backups from machines in different zones still sort correctly
/// next to each other - the name is a sort key first and a wall clock second.
pub fn backup_name() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}.tar.gz", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// Days since 1970-01-01 to a calendar date, by Howard Hinnant's `civil_from_days`.
///
/// Written out rather than pulled in: a date crate would be a dependency for
/// one filename, and this is the same arithmetic every one of them performs.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    // March is month 0 in this scheme; roll it back to the calendar.
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Create a timestamped tar.gz of the deploy directory.
///
/// Returns the command and the archive name; the name is known up front because
/// this side chose it, so nothing has to be parsed back out of the output.
pub fn backup_command(dialect: Dialect, deploy_path: &str, archive_name: &str) -> String {
    let dir = deploy_path.trim_end_matches(['/', '\\']);
    let backups = backups_dir(deploy_path);
    let archive = join_remote(&backups, archive_name);
    dialect.and_then(&dialect.mkdir_p(&backups), &dialect.tar_czf(&archive, dir))
}

/// Run the backup, explaining the one failure that surprises everyone.
///
/// Archives live *beside* the deploy directory, so creating them needs write
/// access to its parent - typically `/var/www`, owned by root. Uploading works
/// regardless, which makes a bare "permission denied" from `mkdir` look
/// arbitrary; the hint below is what actually unblocks it.
pub fn run_backup(session: &Session, dialect: Dialect, deploy_path: &str) -> Result<String> {
    let name = backup_name();
    check_quotable(dialect, &[deploy_path, &backups_dir(deploy_path)])?;
    exec(session, &backup_command(dialect, deploy_path, &name)).map(|_| name).map_err(|err| {
        if mentions_permission_denial(&err) {
            anyhow::anyhow!(permission_hint(dialect, deploy_path))
        } else {
            err
        }
    })
}

/// Why the backup was refused and the one command that unblocks it.
///
/// The fix is spelled in the server's own idiom: `sudo chown` means nothing on
/// a Windows box, and a hint the user cannot paste is barely a hint.
fn permission_hint(dialect: Dialect, deploy_path: &str) -> String {
    let backups = backups_dir(deploy_path);
    let parent = parent_dir(&backups);
    let fix = match dialect {
        Dialect::Posix => format!("sudo mkdir -p {backups} && sudo chown $USER {backups}"),
        Dialect::Windows => format!("mkdir \"{backups}\"   (in an elevated prompt, if {parent} needs it)"),
    };
    format!(
        "cannot write backups to {backups}: permission denied.\n\
         Backups are kept next to the deploy directory, so this needs write access to {parent} - \
         being able to upload into the deploy directory itself is not enough.\n\
         Create it once on the server with the right owner, e.g.\n  {fix}"
    )
}

/// Whether a failed remote command was refused for lack of permission.
fn mentions_permission_denial(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_lowercase();
    text.contains("permission denied") || text.contains("cannot create directory")
}

/// The directory a path sits in; the root when there is no parent to name.
///
/// Handles both separators: this feeds an error message, and a Windows path cut
/// at the wrong character would name a directory that does not exist.
fn parent_dir(path: &str) -> &str {
    match path.trim_end_matches(['/', '\\']).rsplit_once(['/', '\\']) {
        Some(("", _)) | None => "/",
        Some((parent, _)) => parent,
    }
}

#[cfg(test)]
mod tests {
    use super::{backup_command, backup_name, backups_dir, civil_from_days, join_remote, mentions_permission_denial, parent_dir};
    use crate::shell::Dialect;

    #[test]
    fn backups_sit_beside_the_deploy_directory() {
        assert_eq!(backups_dir("/var/www/myapp"), "/var/www/myapp.backups");
        assert_eq!(backups_dir("/var/www/myapp/"), "/var/www/myapp.backups");
    }

    /// A Windows deploy path has to produce a Windows backups path, or the
    /// server gets `C:\site/..backups` and only some tools accept it.
    #[test]
    fn backups_follow_the_separator_of_the_path() {
        assert_eq!(backups_dir("C:\\inetpub\\site"), "C:\\inetpub\\site.backups");
        assert_eq!(backups_dir("C:\\inetpub\\site\\"), "C:\\inetpub\\site.backups");
        assert_eq!(join_remote("C:\\site.backups", "a.tar.gz"), "C:\\site.backups\\a.tar.gz");
        assert_eq!(join_remote("/var/www/site.backups", "a.tar.gz"), "/var/www/site.backups/a.tar.gz");
    }

    /// The parent is what the permission hint tells the user to fix, so it has
    /// to be right even for a directory sitting at the root.
    #[test]
    fn parent_of_a_backups_dir() {
        assert_eq!(parent_dir("/var/www/myapp.backups"), "/var/www");
        assert_eq!(parent_dir("/srv"), "/");
        assert_eq!(parent_dir("/"), "/");
        assert_eq!(parent_dir("C:\\inetpub\\site.backups"), "C:\\inetpub");
    }

    #[test]
    fn recognizes_permission_failures_only() {
        let denied = anyhow::anyhow!("remote command 'mkdir -p /var/www/x.backups' exited with 1: mkdir: cannot create directory: Permission denied");
        assert!(mentions_permission_denial(&denied));

        let other = anyhow::anyhow!("remote command 'tar czf ...' exited with 2: tar: not found");
        assert!(!mentions_permission_denial(&other), "unrelated failures must keep their own message");
    }

    /// The message has one job: say why uploading works while backing up does
    /// not, and give the command that fixes it - in the server's own idiom.
    #[test]
    fn permission_hint_names_the_parent_and_the_fix() {
        let hint = super::permission_hint(Dialect::Posix, "/var/www/myapp");
        assert!(hint.contains("/var/www/myapp.backups"), "{hint}");
        assert!(hint.contains("write access to /var/www"), "{hint}");
        assert!(hint.contains("sudo mkdir -p /var/www/myapp.backups"), "{hint}");

        let windows = super::permission_hint(Dialect::Windows, "C:\\inetpub\\site");
        assert!(windows.contains("mkdir \"C:\\inetpub\\site.backups\""), "{windows}");
        assert!(!windows.contains("sudo"), "sudo means nothing on Windows: {windows}");
    }

    /// The exact backup command, per dialect. It was never asserted before, and
    /// the POSIX-only `$(date ...)` inside it is precisely what made backups
    /// silently wrong on a Windows server.
    #[test]
    fn the_backup_command_carries_no_posix_only_syntax() {
        let posix = backup_command(Dialect::Posix, "/var/www/site", "20260812-181500.tar.gz");
        assert_eq!(
            posix,
            "mkdir -p '/var/www/site.backups' && tar czf '/var/www/site.backups/20260812-181500.tar.gz' -C '/var/www/site' ."
        );

        let windows = backup_command(Dialect::Windows, "C:\\inetpub\\site", "20260812-181500.tar.gz");
        assert!(!windows.contains("$("), "command substitution does not exist in cmd.exe: {windows}");
        assert!(!windows.contains("mkdir -p"), "cmd.exe mkdir has no -p: {windows}");
        assert!(windows.contains("if not exist"), "{windows}");
        assert!(
            windows.contains("tar czf \"C:\\inetpub\\site.backups\\20260812-181500.tar.gz\" -C \"C:\\inetpub\\site\" ."),
            "{windows}"
        );
    }

    /// The name is a sort key: `restore` picks the newest backup by sorting
    /// these strings, so the fields have to be fixed-width and big-endian.
    #[test]
    fn backup_names_sort_chronologically() {
        let name = backup_name();
        assert!(name.ends_with(".tar.gz"), "{name}");
        let stem = name.trim_end_matches(".tar.gz");
        assert_eq!(stem.len(), 15, "YYYYMMDD-HHMMSS: {name}");
        assert_eq!(stem.as_bytes()[8], b'-', "{name}");
        assert!(stem.chars().filter(|c| *c != '-').all(|c| c.is_ascii_digit()), "{name}");
        assert!("20260812-000000" < "20260812-181500", "the format has to order lexicographically");
    }

    /// Dates the deploy tooling will actually see, plus the leap-year cases
    /// that catch a wrong civil-date conversion.
    #[test]
    fn days_since_the_epoch_become_calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-02-29: a leap year despite being divisible by 100.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        // 2024-02-29, the ordinary leap case.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(20_313), (2025, 8, 13));
    }
}
