use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};

use crate::model::{App, Credential, Server};
use crate::shell::{self, Dialect};
use crate::ssh::Session;
use crate::store;

/// Everything a remote operation needs, resolved from the catalogs.
///
/// The four parts became free-standing entities in v0.9.0 and gained a name in
/// v0.11.0: a [`crate::model::Target`] *is* this tuple, and `target` below says
/// which one it came from when it came from one at all.
pub struct Resolved {
    pub app: App,
    pub server: Server,
    pub credential: Credential,
    pub path: crate::model::Path,
    /// The named target this was resolved from, when it was resolved from one.
    ///
    /// `None` after an override changed a field, or when the tuple was
    /// assembled from the binding with nothing saved yet - which is what the
    /// offer to save a target keys on.
    pub target: Option<String>,
    /// Where the deployed app answers: the target's own address, else the
    /// server's URL.
    pub address: String,
    /// How a deploy proves it worked, when the target says.
    pub health: Option<crate::model::HealthCheck>,
    /// Why the target's health check is not part of this run, when it has one
    /// and an override moved the run to another stand.
    pub unchecked: Option<String>,
    /// How many backups the stand keeps: the target's word, else
    /// [`DEFAULT_KEEP_BACKUPS`].
    pub keep_backups: u32,
}

impl Resolved {
    /// How a connection is announced: the account and the machine it reaches.
    pub fn connection_label(&self) -> String {
        format!("{}@{}:{}", self.credential.user, self.server.ssh_host(), self.server.port)
    }
}

/// A machine to reach and the account to reach it as - what `ssh` and
/// `exec` need, with the deploy directory when the name carried one.
pub struct Host {
    pub server: Server,
    pub credential: Credential,
    /// The target's deploy directory, when the name was a target (or an app
    /// with one); a plain server has none and the login lands in its home.
    pub dir: Option<String>,
}

impl Host {
    pub fn label(&self) -> String {
        format!("{}@{}:{}", self.credential.user, self.server.ssh_host(), self.server.port)
    }
}

/// Resolve what `turnout ssh NAME` and `turnout exec NAME` reach.
///
/// The name is tried as a target, then as a server, then as an app (whose
/// target on the bound server is taken, exactly as `deploy` does); with no
/// name, the app of the current directory. Targets before servers for the
/// same reason as in [`resolve`]: a target names the whole route, and a
/// server that shares the name is still reachable by its own route.
///
/// A plain server logs in with its own credential; `credential` overrides
/// it for this run, and is the only way in when the server names none.
pub fn resolve_host(name: Option<String>, credential: Option<String>) -> Result<Host> {
    if let Some(name) = &name {
        if store::load_targets()?.iter().any(|t| &t.name == name) {
            let resolved = resolve(
                Some(name.clone()),
                Overrides {
                    credential,
                    ..Default::default()
                },
            )?;
            return Ok(Host {
                server: resolved.server,
                credential: resolved.credential,
                dir: Some(resolved.path.dir),
            });
        }
        if let Some(server) = store::load_servers()?.into_iter().find(|s| &s.name == name) {
            let credential_name = credential.or_else(|| server.credential.clone()).ok_or_else(|| {
                anyhow::anyhow!("server '{name}' has no credential - pass --credential NAME, or set one with `turnout server edit {name} --credential NAME`")
            })?;
            let credential = store::load_credentials()?
                .into_iter()
                .find(|c| c.name == credential_name)
                .ok_or_else(|| anyhow::anyhow!("no credential named '{credential_name}' - see `turnout credential list`"))?;
            return Ok(Host { server, credential, dir: None });
        }
    }
    let resolved = resolve(
        name,
        Overrides {
            credential,
            ..Default::default()
        },
    )?;
    Ok(Host {
        server: resolved.server,
        credential: resolved.credential,
        dir: Some(resolved.path.dir),
    })
}

/// What a caller may override for a single command.
#[derive(Default)]
pub struct Overrides {
    pub server: Option<String>,
    pub credential: Option<String>,
    pub path: Option<String>,
}

impl Overrides {
    /// Whether anything was actually overridden.
    fn any(&self) -> bool {
        self.server.is_some() || self.credential.is_some() || self.path.is_some()
    }
}

/// Resolve a deploy target: a named one, or the app's target on the server it
/// is bound to - with `--server`/`--credential`/`--path` overriding fields for
/// this run only.
///
/// `name` is a target name when one matches, and an app name otherwise. The two
/// are separate catalogs, so a lookup has to choose: targets win, because
/// `turnout deploy web-prod` naming a target is the point of v0.11.0, and an app
/// that happens to share the name is still reachable through its own target.
pub fn resolve(name: Option<String>, overrides: Overrides) -> Result<Resolved> {
    let targets = store::load_targets()?;
    if let Some(name) = &name
        && let Some(target) = targets.iter().find(|t| &t.name == name)
    {
        return from_target(target.clone(), overrides);
    }

    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, name)?.clone();
    let server_name = match &overrides.server {
        Some(name) => name.clone(),
        None => store::load_state()?.bindings.get(&app.name).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "nowhere to deploy '{0}': name a target (see `turnout target list`), pass --server, \
                 or bind one with `turnout use {0} SERVER`",
                app.name
            )
        })?,
    };

    // The app's target on that server, when there is one. Two targets for the
    // same pair is a legitimate shape - a staging root beside the live one - and
    // picking one arbitrarily would deploy somewhere nobody chose.
    let mut matching = targets.iter().filter(|t| t.app == app.name && t.server == server_name);
    let found = match (matching.next(), matching.next()) {
        (Some(only), None) => Some(only.clone()),
        (Some(first), Some(second)) => bail!(
            "'{}' has more than one target on '{server_name}' ('{}', '{}') - name the one to deploy",
            app.name,
            first.name,
            second.name
        ),
        (None, _) => None,
    };

    match found {
        Some(target) => from_target(target, overrides),
        None => assemble(app, server_name, overrides),
    }
}

/// Load the four entities a target names, applying single-run overrides.
fn from_target(target: crate::model::Target, overrides: Overrides) -> Result<Resolved> {
    let overridden = overrides.any();
    let app = store::load_apps()?
        .into_iter()
        .find(|a| a.name == target.app)
        .ok_or_else(|| dangling(&target.name, "app", &target.app, "turnout app list"))?;
    let server_name = overrides.server.unwrap_or_else(|| target.server.clone());
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| dangling(&target.name, "server", &server_name, "turnout server list"))?;
    app.check_allowed(&server.name)?;
    let credential_name = overrides.credential.unwrap_or_else(|| target.credential.clone());
    let credential = store::load_credentials()?
        .into_iter()
        .find(|c| c.name == credential_name)
        .ok_or_else(|| dangling(&target.name, "credential", &credential_name, "turnout credential list"))?;
    let path_name = overrides.path.unwrap_or_else(|| target.path.clone());
    let path = store::load_paths()?
        .into_iter()
        .find(|p| p.name == path_name)
        .ok_or_else(|| dangling(&target.name, "path", &path_name, "turnout path list"))?;
    check_deploy_dir(&path)?;
    // The address and the check describe the stand - the server and the
    // directory. Another login reaches the same stand, so they stay; another
    // server or directory is a different stand, and checking it against this
    // target's address would roll back a deploy for somebody else's outage.
    let same_stand = server.name == target.server && path.name == target.path;
    let unchecked = match &target.health {
        Some(health) if !same_stand => Some(format!(
            "this run lands somewhere else than target '{}' does, so its health check ({}) is not asked",
            target.name,
            health.describe(&target.url.clone().unwrap_or_else(|| server.url.clone()))
        )),
        _ => None,
    };
    let (address, health, keep_backups) = if same_stand {
        (
            target.address(&server),
            target.health.clone(),
            target.keep_backups.unwrap_or(DEFAULT_KEEP_BACKUPS),
        )
    } else {
        (server.url.clone(), None, DEFAULT_KEEP_BACKUPS)
    };
    Ok(Resolved {
        app,
        server,
        credential,
        path,
        // An overridden run is not this target any more, and recording it as one
        // would let a `--path other` run be journaled under the target's name.
        target: (!overridden).then_some(target.name),
        address,
        health,
        unchecked,
        keep_backups,
    })
}

/// A target naming an entity that is no longer in the catalog.
///
/// Removing a server or a path leaves the targets that used it pointing at
/// nothing; saying which target broke is the difference between a fix and a
/// hunt.
fn dangling(target: &str, kind: &str, name: &str, list: &str) -> anyhow::Error {
    anyhow::anyhow!("target '{target}' names {kind} '{name}', which is not in the catalog - see `{list}`")
}

/// Assemble a tuple with no named target behind it: the binding gives the
/// server, the server its credential, and the path still has to be named.
///
/// This is what a first deploy looks like before anything is saved. It ends in
/// a `Resolved` with `target: None`, which is the caller's cue to offer saving
/// it.
fn assemble(app: App, server_name: String, overrides: Overrides) -> Result<Resolved> {
    let server = store::load_servers()?
        .into_iter()
        .find(|s| s.name == server_name)
        .ok_or_else(|| anyhow::anyhow!("no server named '{server_name}' - see `turnout server list`"))?;
    app.check_allowed(&server.name)?;

    let credential_name = overrides.credential.or_else(|| server.credential.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "server '{0}' has no credential - set one with `turnout server edit {0} --credential NAME`, \
             or pass --credential for this command only",
            server.name
        )
    })?;
    let credential = store::load_credentials()?
        .into_iter()
        .find(|c| c.name == credential_name)
        .ok_or_else(|| anyhow::anyhow!("no credential named '{credential_name}' - see `turnout credential list`"))?;

    let paths = store::load_paths()?;
    let path_name = match overrides.path {
        Some(name) => name,
        None => {
            // A terminal gets the picker; a script gets the way to say it
            // without one. Saving the answer as a target is what stops the next
            // run from asking again.
            crate::pick::ensure_interactive(&format!(
                "no target for '{}' on '{}': create one with `turnout target add`, or pass --path",
                app.name, server.name
            ))?;
            eprintln!("No target for '{}' on '{}' yet.", app.name, server.name);
            crate::pick::path(&paths, "Deploy into which path")?
        }
    };
    let path = paths
        .into_iter()
        .find(|p| p.name == path_name)
        .ok_or_else(|| anyhow::anyhow!("no path named '{path_name}' - see `turnout path list`"))?;
    check_deploy_dir(&path)?;

    Ok(Resolved {
        address: server.url.clone(),
        app,
        server,
        credential,
        path,
        target: None,
        health: None,
        unchecked: None,
        keep_backups: DEFAULT_KEEP_BACKUPS,
    })
}

/// A path whose directory is a filesystem root reaches no remote command.
///
/// `path add` refuses one, but a catalog written before it did - or edited by
/// hand - still holds whatever it holds, and this is where every deploy,
/// backup and restore reads it: the last point before `--clear` would empty
/// the server's `/`.
fn check_deploy_dir(path: &crate::model::Path) -> Result<()> {
    crate::model::refuse_filesystem_root(&path.dir)
        .map_err(|err| anyhow::anyhow!("path '{0}': {err:#}\nChange it with `turnout path edit {0} --dir DIR`.", path.name))
}

/// Open a session to `server` as `credential`.
///
/// The transport lives in [`crate::ssh`]; this is the entry the rest of the
/// remote layer calls. The credential's kind - key file, stored password or
/// SSH agent - is the one way it signs in (see [`Session::connect`]).
pub fn connect(server: &Server, credential: &Credential) -> Result<Session> {
    Session::connect(server, credential)
}

/// Run a remote command and return its stdout; a non-zero exit becomes an error
/// carrying the remote stderr.
pub fn exec(session: &Session, command: &str) -> Result<String> {
    session.exec(command)
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
pub fn dialect(session: &Session, server: &Server) -> Result<Dialect> {
    if let Some(known) = server.shell {
        return Ok(known);
    }
    let Ok(reply) = exec(session, shell::PROBE) else {
        return Ok(Dialect::default());
    };
    // Not remembered: once the administrator points sshd back at cmd.exe,
    // the next command asks again and finds it.
    if shell::is_powershell(&reply) {
        return Err(shell::powershell_refusal(&server.name));
    }
    let learned = shell::read_probe(&reply);
    remember_dialect(&server.name, learned);
    Ok(learned)
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
    let (year, month, day) = crate::utils::civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}.tar.gz", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// The first name of `stamp`'s second that no backup in `taken` has:
/// `20260812-181500.tar.gz`, then `20260812-181500_02.tar.gz` and on.
///
/// Names are per second, and two backups of one stand in one second are an
/// ordinary script - `deploy --backup` followed by `backup`, or two checked
/// deploys back to back. Without this the second `tar czf` wrote over the
/// first. The suffix starts with `_`, which sorts after the `.` of a bare
/// name, so string order stays time order and `restore` still finds the
/// newest by sorting.
pub fn free_backup_name(stamp: &str, taken: &[String]) -> String {
    let stem = stamp.trim_end_matches(".tar.gz");
    std::iter::once(format!("{stem}.tar.gz"))
        .chain((2..).map(|n| format!("{stem}_{n:02}.tar.gz")))
        .find(|name| !taken.contains(name))
        .expect("an unbounded sequence has a free name")
}

/// Create a timestamped tar.gz of the deploy directory.
///
/// The name is known up front because this side chose it, so nothing has to
/// be parsed back out of the output. The chain refuses to write over an
/// archive already there: [`free_backup_name`] avoids every name the server
/// listed, so only another backup taken in the same instant can trip it, and
/// that has to fail rather than replace a backup without a word.
///
/// `tar` writes to a name [`is_backup_name`] does not take - the archive's
/// own, with this process's id and `.part` after it - and the archive is
/// renamed into place only once `tar` is through; a `tar` that failed takes
/// its partial file with it. Up to v0.26 it wrote straight to the final name,
/// so a backup cut short by a full disk stayed as a truncated archive under a
/// valid name: the next `restore` took it for the newest, and the prune
/// counted it as one of the backups the stand keeps.
///
/// GNU `tar` exits 1 for "file changed as we read it": a stand is live while
/// it is backed up, and a log or an upload written meanwhile is enough. The
/// archive is whole then - only that one file may be caught mid-write - so on
/// POSIX that exit is taken as success when the `tar` is GNU's. Refusing it
/// failed every checked deploy to a busy stand before it began. bsdtar (macOS,
/// Windows) has no such code: its 1 is a failure, and stays one.
pub fn backup_command(dialect: Dialect, deploy_path: &str, archive_name: &str) -> String {
    let dir = deploy_path.trim_end_matches(['/', '\\']);
    let backups = backups_dir(deploy_path);
    let archive = join_remote(&backups, archive_name);
    let partial = join_remote(&backups, &format!("{archive_name}.{}.part", std::process::id()));
    let (b, a, p, d) = (dialect.quote(&backups), dialect.quote(&archive), dialect.quote(&partial), dialect.quote(dir));
    match dialect {
        Dialect::Posix => {
            let taken = "echo 'a backup by this name is already there' >&2; exit 1";
            let script = format!(
                "mkdir -p {b} || exit 1; if [ -e {a} ]; then {taken}; fi; \
                 tar czf {p} -C {d} .; s=$?; \
                 if [ $s -eq 1 ] && tar --version 2>/dev/null | grep -q 'GNU tar'; then s=0; fi; \
                 if [ $s -ne 0 ]; then rm -f {p}; exit $s; fi; \
                 if [ -e {a} ]; then rm -f {p}; {taken}; fi; \
                 mv {p} {a}"
            );
            format!("sh -c {}", dialect.quote(&script))
        }
        // Each group is self-contained, as `and_then` needs on `cmd.exe`; an
        // `exit /b 1` inside one ends the whole line with the failure.
        Dialect::Windows => [
            dialect.mkdir_p(&backups),
            dialect.nothing_at(&archive),
            format!("({} || (del /f /q {p} >nul 2>&1 & exit /b 1))", dialect.tar_czf(&partial, dir)),
            format!("(if exist {a} (del /f /q {p} >nul 2>&1 & exit /b 1))"),
            format!("move {p} {a} >nul"),
        ]
        .into_iter()
        .reduce(|chain, next| dialect.and_then(&chain, &next))
        .expect("the chain has links"),
    }
}

/// Run the backup, explaining the one failure that surprises everyone.
///
/// Archives live *beside* the deploy directory, so creating them needs write
/// access to its parent - typically `/var/www`, owned by root. Uploading works
/// regardless, which makes a bare "permission denied" from `mkdir` look
/// arbitrary; the hint below is what actually unblocks it.
pub fn run_backup(session: &Session, dialect: Dialect, deploy_path: &str) -> Result<String> {
    check_quotable(dialect, &[deploy_path, &backups_dir(deploy_path)])?;
    let name = free_backup_name(&backup_name(), &list_backups(session, dialect, deploy_path)?);
    exec(session, &backup_command(dialect, deploy_path, &name)).map(|_| name).map_err(|err| {
        if mentions_permission_denial(&err) {
            anyhow::anyhow!(permission_hint(dialect, deploy_path))
        } else {
            err
        }
    })
}

/// Put a backup back: empty the deploy directory, then unpack the archive
/// into it. The archive is checked for first, so a wrong name empties nothing.
///
/// Returns the entries the emptying could not remove - a file another
/// process holds, a directory this account may not write to. The archive is
/// unpacked regardless: with one stuck file, refusing to unpack would leave
/// the stand with nothing at all instead of the backup plus that file.
pub fn restore_backup(session: &Session, dialect: Dialect, deploy_dir: &str, name: &str) -> Result<Vec<String>> {
    let backups = backups_dir(deploy_dir);
    check_quotable(dialect, &[deploy_dir, &backups, name])?;
    let archive = join_remote(&backups, name);
    let dir = deploy_dir.trim_end_matches(['/', '\\']);
    let left = exec(
        session,
        &dialect.and_then(
            &dialect.file_exists(&archive),
            &dialect.and_then(&dialect.clear_dir_listing_the_rest(dir), &dialect.untar(&archive, dir)),
        ),
    )?;
    Ok(left.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect())
}

/// What to say about the entries a restore could not remove: they stay in
/// the directory beside the backup's files.
pub fn not_removed_note(dir: &str, left: &[String]) -> String {
    let plural = if left.len() == 1 { "y" } else { "ies" };
    format!(
        "{} entr{plural} in {dir} could not be removed before the backup was unpacked, and stay beside its files: {}",
        left.len(),
        left.join(", ")
    )
}

/// Delete one backup archive.
pub fn remove_backup(session: &Session, dialect: Dialect, deploy_dir: &str, name: &str) -> Result<()> {
    exec(session, &dialect.remove_file(&join_remote(&backups_dir(deploy_dir), name))).map(|_| ())
}

/// Backups a stand keeps when its target names no number (ADR 0028).
pub const DEFAULT_KEEP_BACKUPS: u32 = 5;

/// Whether a listing entry is one of our archives: `YYYYMMDD-HHMMSS.tar.gz`,
/// or `YYYYMMDD-HHMMSS_NN.tar.gz` for a later one of the same second.
///
/// The backups directory belongs to the user: a note left in it must never
/// be restored as if it were an archive, nor pruned as if it were ours.
pub fn is_backup_name(entry: &str) -> bool {
    let Some(stem) = entry.strip_suffix(".tar.gz") else {
        return false;
    };
    let (stamp, sequence) = match stem.split_once('_') {
        Some((stamp, sequence)) => (stamp, Some(sequence)),
        None => (stem, None),
    };
    let stamp_ok = stamp.len() == 15 && stamp.chars().enumerate().all(|(i, c)| if i == 8 { c == '-' } else { c.is_ascii_digit() });
    let sequence_ok = sequence.is_none_or(|n| n.len() == 2 && n.chars().all(|c| c.is_ascii_digit()));
    stamp_ok && sequence_ok
}

/// Our archives in a directory listing, newest first.
///
/// The names are fixed-width timestamps, so string order is time order - no
/// date parsing, and no dependence on the order the server listed them in
/// (`dir /b` and `ls -1` do not agree on that).
pub fn backups_in(listing: &str) -> Vec<String> {
    let mut names: Vec<String> = listing.lines().map(str::trim).filter(|line| is_backup_name(line)).map(str::to_string).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names
}

/// The backups of a deploy directory, newest first; none when the backups
/// directory is not there yet.
pub fn list_backups(session: &Session, dialect: Dialect, deploy_dir: &str) -> Result<Vec<String>> {
    let backups = backups_dir(deploy_dir);
    check_quotable(dialect, &[&backups])?;
    Ok(backups_in(&exec(session, &dialect.list_dir(&backups))?))
}

/// What a prune did.
pub struct Pruned {
    /// The archives that went, newest first.
    pub removed: Vec<String>,
    /// How many are left.
    pub kept: usize,
}

/// Remove all but the newest `keep` backups of a deploy directory.
///
/// The directory is listed again afterwards and the report says what is
/// gone by that listing: `del` on a Windows server answers success whatever
/// happened, and a prune that claims a removal it did not make is the kind
/// of report that fills a disk.
pub fn prune_backups(session: &Session, dialect: Dialect, deploy_dir: &str, keep: u32) -> Result<Pruned> {
    let before = list_backups(session, dialect, deploy_dir)?;
    let doomed = before.get(keep as usize..).unwrap_or_default();
    if doomed.is_empty() {
        return Ok(Pruned {
            removed: Vec::new(),
            kept: before.len(),
        });
    }
    let backups = backups_dir(deploy_dir);
    let paths: Vec<String> = doomed.iter().map(|name| join_remote(&backups, name)).collect();
    remove_paths(session, dialect, &paths)?;
    let after = list_backups(session, dialect, deploy_dir)?;
    let removed: Vec<String> = before.iter().filter(|name| !after.contains(name)).cloned().collect();
    if let Some(stuck) = doomed.iter().find(|name| after.contains(name)) {
        bail!(
            "could not remove the backup {stuck} from {backups} - removed {} of {}",
            removed.len(),
            doomed.len()
        );
    }
    Ok(Pruned { removed, kept: after.len() })
}

/// How long one removal command may grow: well under `cmd.exe`'s 8191
/// characters, which is the tightest line any server here has.
const REMOVE_LINE: usize = 6_000;

/// Delete files on the server, in as few commands as the line allows. A
/// file that is not there is not an error.
pub fn remove_paths(session: &Session, dialect: Dialect, paths: &[String]) -> Result<()> {
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    check_quotable(dialect, &refs)?;
    let mut chunk: Vec<&str> = Vec::new();
    let mut length = 0;
    for path in refs {
        if !chunk.is_empty() && length + path.len() + 3 > REMOVE_LINE {
            exec(session, &dialect.remove_files(&chunk))?;
            chunk.clear();
            length = 0;
        }
        length += path.len() + 3;
        chunk.push(path);
    }
    if !chunk.is_empty() {
        exec(session, &dialect.remove_files(&chunk))?;
    }
    Ok(())
}

/// Whether the deploy directory is there yet - a first deploy creates it, and
/// has nothing to back up.
pub fn dir_exists(session: &Session, dialect: Dialect, dir: &str) -> bool {
    exec(session, &dialect.dir_exists(dir)).is_ok()
}

/// Run a path's post-write command in its directory, detached from this
/// connection where the server allows it (see [`crate::detached`]), and
/// return its output. `connect` opens a new connection when this one drops;
/// `progress` hears the newest line of output.
pub fn restart(
    session: &mut Session,
    dialect: Dialect,
    path: &crate::model::Path,
    restart: &str,
    connect: &dyn Fn() -> Result<Session>,
    progress: impl FnMut(&str),
) -> Result<String> {
    check_quotable(dialect, &[&path.dir])?;
    crate::detached::run(session, dialect, &path.dir, restart, connect, progress)
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
    use super::{backup_command, backup_name, backups_dir, free_backup_name, join_remote, mentions_permission_denial, parent_dir};
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
        let pid = std::process::id();
        let posix = backup_command(Dialect::Posix, "/var/www/site", "20260812-181500.tar.gz");
        assert!(posix.starts_with("sh -c '"), "{posix}");
        assert!(
            posix.contains(&format!(
                "tar czf '\\''/var/www/site.backups/20260812-181500.tar.gz.{pid}.part'\\'' -C '\\''/var/www/site'\\'' ."
            )),
            "tar writes aside: {posix}"
        );
        assert!(
            posix.ends_with(&format!(
                "mv '\\''/var/www/site.backups/20260812-181500.tar.gz.{pid}.part'\\'' '\\''/var/www/site.backups/20260812-181500.tar.gz'\\'''"
            )),
            "and the archive is renamed into place last: {posix}"
        );

        let windows = backup_command(Dialect::Windows, "C:\\inetpub\\site", "20260812-181500.tar.gz");
        assert!(!windows.contains("$("), "command substitution does not exist in cmd.exe: {windows}");
        assert!(!windows.contains("mkdir -p"), "cmd.exe mkdir has no -p: {windows}");
        assert!(windows.contains("if not exist"), "{windows}");
        assert!(
            windows.contains(&format!(
                "tar czf \"C:\\inetpub\\site.backups\\20260812-181500.tar.gz.{pid}.part\" -C \"C:\\inetpub\\site\" ."
            )),
            "{windows}"
        );
        assert!(
            windows.ends_with(&format!(
                "move \"C:\\inetpub\\site.backups\\20260812-181500.tar.gz.{pid}.part\" \"C:\\inetpub\\site.backups\\20260812-181500.tar.gz\" >nul"
            )),
            "{windows}"
        );
        // The partial file is never taken for a backup.
        assert!(!super::is_backup_name(&format!("20260812-181500.tar.gz.{pid}.part")));
    }

    /// A backup whose `tar` fails leaves nothing behind: no truncated archive
    /// under a name `restore` would take for the newest, and no partial file.
    /// Up to v0.26 the archive was written straight to its name and stayed.
    ///
    /// What fails it: on Windows a file held open without sharing, on POSIX
    /// a file without read permission - `tar` archives the rest, then exits
    /// with an error.
    #[test]
    fn a_failed_backup_leaves_no_archive() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            // root reads through any permission, so nothing would fail.
            return;
        }
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("index.html"), "v1").unwrap();
        let unreadable = site.join("unreadable.txt");
        std::fs::write(&unreadable, "secret").unwrap();
        #[cfg(windows)]
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().read(true).share_mode(0).open(&unreadable).unwrap()
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        let dir = site.display().to_string();
        let outcome = super::run_backup(&session, dialect, &dir);

        #[cfg(windows)]
        drop(held);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(outcome.is_err(), "a tar that failed is a failed backup: {outcome:?}");
        let backups = std::path::PathBuf::from(super::backups_dir(&dir));
        let left: Vec<String> = std::fs::read_dir(&backups)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.is_empty(), "nothing of the failed backup stays: {left:?}");
    }

    /// GNU tar's exit 1 - a file changed while it was read - leaves a whole
    /// archive, and is a backup; the same 1 from any other tar is a failure.
    /// A stand-in `tar` answers for each, written by the stand's own shell so
    /// that no descriptor of this test process is open on it when it runs.
    #[cfg(unix)]
    #[test]
    fn gnu_tar_s_changed_file_is_a_backup_and_nothing_else_s_exit_1_is() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        let dir = site.display().to_string();
        for (version, kept) in [("tar (GNU tar) 1.34", true), ("bsdtar 3.7.2 - libarchive 3.7.2", false)] {
            let fake = root.path().join(if kept { "gnu" } else { "bsd" });
            let script = format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\necho archive > \"$2\"\nexit 1\n");
            let fake_q = Dialect::Posix.quote(&fake.display().to_string());
            session
                .exec(&format!(
                    "mkdir -p {fake_q} && printf '%s' {} > {fake_q}/tar && chmod +x {fake_q}/tar",
                    Dialect::Posix.quote(&script)
                ))
                .unwrap();
            let name = format!("20261030-12000{}.tar.gz", u8::from(kept));
            let command = backup_command(Dialect::Posix, &dir, &name);
            let outcome = session.exec(&format!("PATH={fake_q}:\"$PATH\" {command}"));
            let archive = std::path::PathBuf::from(super::backups_dir(&dir)).join(&name);
            assert_eq!(outcome.is_ok(), kept, "{version}: {outcome:?}");
            assert_eq!(archive.is_file(), kept, "{version}");
        }
    }

    /// The chains a deploy sends, run through this machine's own shell - sh,
    /// or cmd.exe on Windows. Each one failed on cmd.exe until v0.24 while
    /// every string test passed: a second backup was skipped because its
    /// directory existed, a restore did nothing because its archive did, and
    /// a failed unpack read as success.
    #[test]
    fn the_chains_hold_in_a_real_shell() {
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        std::fs::create_dir_all(site.join("assets")).unwrap();
        std::fs::write(site.join("index.html"), "v1").unwrap();
        std::fs::write(site.join("assets").join("app.js"), "one").unwrap();
        let dir = site.display().to_string();

        assert!(super::dir_exists(&session, dialect, &dir));
        assert!(!super::dir_exists(&session, dialect, &format!("{dir}-not-there")));
        std::fs::write(root.path().join("a-file"), "x").unwrap();
        assert!(
            !super::dir_exists(&session, dialect, &root.path().join("a-file").display().to_string()),
            "a file is not a directory"
        );

        let first = super::run_backup(&session, dialect, &dir).unwrap();
        // No pause: two backups in one second are an ordinary script, and the
        // second must take a name of its own instead of writing over the first.
        std::fs::write(site.join("index.html"), "v2").unwrap();
        std::fs::write(site.join("extra.txt"), "only in v2").unwrap();
        let second = super::run_backup(&session, dialect, &dir).unwrap();
        assert_ne!(first, second);
        let backups = std::path::PathBuf::from(super::backups_dir(&dir));
        assert!(backups.join(&first).is_file() && backups.join(&second).is_file(), "both backups exist");
        // The guard: a name already on the server is never written over.
        let again = super::backup_command(dialect, &dir, &first);
        assert!(super::exec(&session, &again).is_err(), "an existing archive must stop the backup");

        super::restore_backup(&session, dialect, &dir, &first).unwrap();
        assert_eq!(std::fs::read_to_string(site.join("index.html")).unwrap(), "v1", "the restore put v1 back");
        assert!(!site.join("extra.txt").exists(), "and emptied what v1 did not have");
        assert_eq!(std::fs::read_to_string(site.join("assets").join("app.js")).unwrap(), "one");
        assert!(
            super::restore_backup(&session, dialect, &dir, "20000101-000000.tar.gz").is_err(),
            "no such backup"
        );
        assert!(site.join("index.html").exists(), "a missing backup empties nothing");

        let broken = site.join(".turnout-upload.tar.gz");
        std::fs::write(&broken, "not an archive").unwrap();
        let archive = broken.display().to_string();
        let unpack = dialect.and_then(&dialect.untar(&archive, &dir), &dialect.remove_file(&archive));
        assert!(super::exec(&session, &unpack).is_err(), "a failed unpack must fail the chain");

        super::remove_backup(&session, dialect, &dir, &second).unwrap();
        assert!(!backups.join(&second).exists());
    }

    /// One entry the clearing cannot remove must not cost the restore. Up to
    /// v0.26 the POSIX chain stopped there - after emptying everything else -
    /// and unpacked nothing; the Windows one went on and said nothing about
    /// what was left. Now both unpack, and both name what stayed.
    ///
    /// What cannot be removed: on Windows a file this process holds open
    /// without sharing, on POSIX a directory without write permission.
    #[test]
    fn a_restore_unpacks_past_what_it_could_not_remove() {
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            // root deletes through any permission, so nothing can be stuck.
            return;
        }
        let stand = crate::ssh::tests::Stand::spawn_shell();
        let session = stand.session();
        let dialect = crate::shell::read_probe(&session.exec(crate::shell::PROBE).unwrap());
        let root = tempfile::tempdir().unwrap();
        let site = root.path().join("site");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("index.html"), "v1").unwrap();
        let dir = site.display().to_string();
        let backup = super::run_backup(&session, dialect, &dir).unwrap();

        std::fs::write(site.join("index.html"), "v2").unwrap();
        let stuck = site.join("stuck");
        std::fs::create_dir_all(&stuck).unwrap();
        std::fs::write(stuck.join("held.txt"), "x").unwrap();
        #[cfg(windows)]
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().read(true).share_mode(0).open(stuck.join("held.txt")).unwrap()
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o555)).unwrap();
        }

        let left = super::restore_backup(&session, dialect, &dir, &backup);

        #[cfg(windows)]
        drop(held);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stuck, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let left = left.expect("the restore goes on past what it could not remove");
        assert_eq!(std::fs::read_to_string(site.join("index.html")).unwrap(), "v1", "the backup was unpacked");
        assert_eq!(left, ["stuck"], "and what stayed is named");
        let note = super::not_removed_note(&dir, &left);
        assert!(note.contains("1 entry") && note.contains("stuck"), "{note}");
    }

    #[test]
    fn a_second_backup_in_one_second_takes_the_next_name() {
        let taken = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(free_backup_name("20261030-120000.tar.gz", &[]), "20261030-120000.tar.gz");
        assert_eq!(
            free_backup_name("20261030-120000.tar.gz", &taken(&["20261030-120000.tar.gz"])),
            "20261030-120000_02.tar.gz"
        );
        assert_eq!(
            free_backup_name("20261030-120000.tar.gz", &taken(&["20261030-120000.tar.gz", "20261030-120000_02.tar.gz"])),
            "20261030-120000_03.tar.gz"
        );
        // Sorted as `restore` and `prune` sort them, newest first.
        let listing = "20261030-120001.tar.gz\n20261030-120000_02.tar.gz\n20261030-120000.tar.gz\n20261030-115959.tar.gz\n";
        assert_eq!(
            super::backups_in(listing),
            [
                "20261030-120001.tar.gz",
                "20261030-120000_02.tar.gz",
                "20261030-120000.tar.gz",
                "20261030-115959.tar.gz"
            ]
        );
        assert!(super::is_backup_name("20261030-120000_02.tar.gz"));
        assert!(!super::is_backup_name("20261030-120000_2.tar.gz"), "two digits");
        assert!(!super::is_backup_name("20261030-120000_ab.tar.gz"));
        assert!(!super::is_backup_name("20261030-120000_.tar.gz"));
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
}
