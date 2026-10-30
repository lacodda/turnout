//! `turnout export` and `turnout import`: move a setup to another machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dialoguer::Password;

use crate::model::{App, Group, Target, validate_name, validate_remote_path, validate_url};
use crate::portable::{self, Snapshot};
use crate::{pick, secrets, store};

pub fn export(path: Option<PathBuf>, with_secrets: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let servers = store::load_servers()?;
    let credentials = store::load_credentials()?;
    let remote_paths = store::load_paths()?;
    let groups = store::load_groups()?;
    let targets = store::load_targets()?;
    if apps.is_empty() && servers.is_empty() && groups.is_empty() {
        bail!("nothing to export - this machine has no apps, servers or groups yet");
    }

    let mut snapshot = Snapshot::new(apps, servers, credentials, remote_paths, groups, targets);
    if with_secrets {
        let collected = collect_secrets(&snapshot.credentials)?;
        if collected.is_empty() {
            println!("No secrets are stored on this machine; exporting configuration only.");
        } else {
            let passphrase = ask_new_passphrase(collected.len())?;
            snapshot.secrets = Some(portable::seal(&collected, &passphrase)?);
        }
    }

    let path = path.unwrap_or_else(|| PathBuf::from("turnout-export.json"));
    let json = serde_json::to_string_pretty(&snapshot)?;
    write_private(&path, &json)?;

    println!("Exported to {}", path.display());
    println!(
        "  {} app(s), {} server(s), {} credential(s), {} path(s), {} target(s), {} group(s)",
        snapshot.apps.len(),
        snapshot.servers.len(),
        snapshot.credentials.len(),
        snapshot.paths.len(),
        snapshot.targets.len(),
        snapshot.groups.len()
    );
    match &snapshot.secrets {
        Some(_) => println!("  Secrets are included, encrypted with your passphrase."),
        None if with_secrets => {}
        // Say it plainly: the file looks complete but will not restore access.
        None => println!("  Secrets are NOT included - re-run with --with-secrets to take them along."),
    }
    crate::journal::record("export", None, None, Some(&format!("{} apps", snapshot.apps.len())));
    Ok(())
}

pub fn import(path: PathBuf, force: bool) -> Result<()> {
    let text = std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    let mut snapshot: Snapshot = serde_json::from_str(&text).with_context(|| format!("{} is not a turnout export", path.display()))?;
    snapshot.check_version()?;
    // A format-2 file keeps its deploy routes inside the servers, where this
    // build has no field for them; they become named targets before anything is
    // written, so an older export still arrives able to deploy.
    let adopted = snapshot.adopt_v2_targets(&text)?;
    if adopted > 0 {
        println!("Converted {adopted} deploy route(s) from the older format into named targets.");
    }

    // Decrypt before writing anything. A wrong passphrase must not leave the
    // catalogs imported and the secrets missing - the user would see an error,
    // half a setup, and "already exists" on the retry.
    let opened = match &snapshot.secrets {
        Some(sealed) => {
            let passphrase = read_passphrase("Passphrase for the secrets in this export")?;
            Some(portable::open(sealed, &passphrase)?)
        }
        None => None,
    };

    // The import is the one way into the catalog that skips `add`, so every
    // arriving entity is held to what `add` would have held it to. Without
    // that, a group `web` here and an app `web` in the file both existed after
    // an import, and `use web prod` silently bound only the app.
    let mut report = Report::default();
    let mut groups = store::load_groups()?;
    // Apps are saved only after their ports are settled: an older export can
    // bring an app with no gateway port, or a port an app here already holds.
    let mut apps = store::load_apps()?;
    let arrived = merge(
        &mut apps,
        snapshot.apps,
        |app| app.name.clone(),
        force,
        "app",
        &mut report,
        |app| admit_app(app, &groups),
        |_| Ok(()),
    )?;
    report.ports = crate::front::settle_arrivals(&mut apps, &arrived);
    if !arrived.is_empty() {
        store::save_apps(&apps)?;
    }
    let mut servers = store::load_servers()?;
    merge(
        &mut servers,
        snapshot.servers,
        |server| server.name.clone(),
        force,
        "server",
        &mut report,
        |server| {
            validate_name(&server.name)?;
            validate_url(&server.url)
        },
        store::save_servers,
    )?;
    merge(
        &mut groups,
        snapshot.groups,
        |group| group.name.clone(),
        force,
        "group",
        &mut report,
        |group| admit_group(group, &apps),
        store::save_groups,
    )?;
    let mut credentials = store::load_credentials()?;
    let credentials_here: Vec<String> = credentials.iter().map(|credential| credential.name.clone()).collect();
    let credentials_written = merge(
        &mut credentials,
        snapshot.credentials,
        |credential| credential.name.clone(),
        force,
        "credential",
        &mut report,
        |credential| validate_name(&credential.name),
        store::save_credentials,
    )?;
    let mut paths = store::load_paths()?;
    merge(
        &mut paths,
        snapshot.paths,
        |path| path.name.clone(),
        force,
        "path",
        &mut report,
        |path| {
            validate_name(&path.name)?;
            validate_remote_path(&path.dir)
        },
        store::save_paths,
    )?;
    // Last: a target names the four entities above, so importing it after them
    // means a report of what arrived reads in dependency order - and an entity
    // refused above refuses the targets that name it, rather than leaving them
    // pointing at nothing.
    let around = Catalog {
        apps: &apps,
        servers: &servers,
        credentials: &credentials,
        paths: &paths,
    };
    merge(
        &mut store::load_targets()?,
        snapshot.targets,
        |target| target.name.clone(),
        force,
        "target",
        &mut report,
        |target| admit_target(target, &around),
        store::save_targets,
    )?;

    // A secret goes with its credential, never past it. Writing every secret
    // in the file used to replace the password of a credential whose entry the
    // import had just kept: this machine's `deploy` entry stayed, its password
    // became the other machine's, and no copy of the old one was left anywhere.
    if let Some(opened) = opened {
        for (credential, value) in &opened {
            if credentials_written.contains(credential) {
                secrets::set(credential, value)?;
                report.secrets += 1;
            } else if credentials_here.contains(credential) {
                report.secrets_kept.push(credential.clone());
            } else {
                report.secrets_left.push(credential.clone());
            }
        }
    }

    report.print();
    crate::journal::record("import", None, None, Some(&format!("{} imported", report.imported)));
    Ok(())
}

/// What an import did, so the user can see it rather than infer it.
#[derive(Default)]
struct Report {
    imported: usize,
    /// Names that already existed, kept as they were.
    skipped: Vec<String>,
    /// Entries `add` would have refused, each with the reason.
    refused: Vec<String>,
    secrets: usize,
    /// Credentials kept as they were here, whose secret in the file was
    /// therefore not written either.
    secrets_kept: Vec<String>,
    /// Secrets in the file whose credential was not imported at all.
    secrets_left: Vec<String>,
    /// Ports an arriving app was given or gave up, one line per app.
    ports: Vec<String>,
}

impl Report {
    fn print(&self) {
        if self.imported == 0 && self.skipped.is_empty() && self.refused.is_empty() {
            println!("The export was empty - nothing to import.");
            return;
        }
        println!("Imported {} item(s).", self.imported);
        if !self.skipped.is_empty() {
            println!("Skipped {} item(s) that already exist:", self.skipped.len());
            for name in &self.skipped {
                println!("  {name}");
            }
            println!("  Re-run with --force to overwrite them.");
        }
        if !self.refused.is_empty() {
            println!("Refused {} item(s) this catalog cannot hold:", self.refused.len());
            for line in &self.refused {
                println!("  {line}");
            }
        }
        for line in &self.ports {
            println!("  {line}");
        }
        if self.secrets > 0 {
            println!("Restored {} secret(s) to the OS keyring.", self.secrets);
        }
        if !self.secrets_kept.is_empty() {
            println!(
                "Kept this machine's secret of {} - the credential here was kept, so the file's secret was not written.",
                quoted(&self.secrets_kept)
            );
            println!("  Re-run with --force to take the file's credential and its secret.");
        }
        if !self.secrets_left.is_empty() {
            println!("Left out the secret of {} - its credential was not imported.", quoted(&self.secrets_left));
        }
    }
}

/// `'a', 'b'` - names as a sentence lists them.
fn quoted(names: &[String]) -> String {
    names.iter().map(|name| format!("'{name}'")).collect::<Vec<_>>().join(", ")
}

/// Add incoming items to `existing`, keeping what is already there unless
/// `force` says otherwise, then save through `save`. Returns the names of the
/// items written.
///
/// An item about to be written is first put to `admit`, the checks `add`
/// makes; one it fails is refused and reported with the reason, and the rest
/// of the import goes on.
#[allow(clippy::too_many_arguments)]
fn merge<T, K, A, S>(existing: &mut Vec<T>, incoming: Vec<T>, key: K, force: bool, kind: &str, report: &mut Report, admit: A, save: S) -> Result<Vec<String>>
where
    K: Fn(&T) -> String,
    A: Fn(&T) -> Result<()>,
    S: Fn(&[T]) -> Result<()>,
{
    if incoming.is_empty() {
        return Ok(Vec::new());
    }
    let mut written = Vec::new();
    for item in incoming {
        let name = key(&item);
        let at = existing.iter().position(|other| key(other) == name);
        if at.is_some() && !force {
            report.skipped.push(format!("{kind} '{name}'"));
            continue;
        }
        if let Err(reason) = admit(&item) {
            report.refused.push(format!("{kind} '{name}': {reason:#}"));
            continue;
        }
        match at {
            Some(index) => existing[index] = item,
            None => existing.push(item),
        }
        written.push(name);
    }
    report.imported += written.len();
    save(existing)?;
    Ok(written)
}

/// The catalogs a target is checked against once everything else arrived.
struct Catalog<'a> {
    apps: &'a [App],
    servers: &'a [crate::model::Server],
    credentials: &'a [crate::model::Credential],
    paths: &'a [crate::model::Path],
}

/// What `app add` refuses: a name that is not one, a name a group already
/// has, and a dotenv file or variable turnout could not write safely - the
/// file is written inside the project, and a name with a slash would land
/// outside it.
fn admit_app(app: &App, groups: &[Group]) -> Result<()> {
    validate_name(&app.name)?;
    if groups.iter().any(|group| group.name == app.name) {
        bail!("a group named '{}' exists here - app names must not clash with group names", app.name);
    }
    if let Some(env_var) = &app.gateway_env {
        super::app::validate_env_name(env_var.clone())?;
    }
    if let Some(env_file) = &app.env_file {
        super::app::validate_env_file(env_file.clone())?;
    }
    Ok(())
}

/// What `group add` refuses: a bad name, an app's name, and members that are
/// not apps of this catalog.
fn admit_group(group: &Group, apps: &[App]) -> Result<()> {
    validate_name(&group.name)?;
    if apps.iter().any(|app| app.name == group.name) {
        bail!("an app named '{}' exists here - group names must not clash with app names", group.name);
    }
    if group.apps.is_empty() {
        bail!("it has no apps");
    }
    if let Some(missing) = group.apps.iter().find(|member| !apps.iter().any(|app| &app.name == *member)) {
        bail!("its member '{missing}' is not an app here");
    }
    Ok(())
}

/// What `target add` refuses: a bad name, and any of the four parts missing.
fn admit_target(target: &Target, around: &Catalog) -> Result<()> {
    validate_name(&target.name)?;
    let parts = [
        ("app", &target.app, around.apps.iter().any(|a| a.name == target.app)),
        ("server", &target.server, around.servers.iter().any(|s| s.name == target.server)),
        ("credential", &target.credential, around.credentials.iter().any(|c| c.name == target.credential)),
        ("path", &target.path, around.paths.iter().any(|p| p.name == target.path)),
    ];
    if let Some((kind, name, _)) = parts.iter().find(|(_, _, present)| !present) {
        bail!("it names {kind} '{name}', which is neither here nor imported");
    }
    Ok(())
}

/// Read every stored secret named by the credential catalog.
///
/// A missing secret is not an error: a credential may authenticate by an
/// unprotected key, or the keyring on this machine may legitimately not hold it.
fn collect_secrets(credentials: &[crate::model::Credential]) -> Result<BTreeMap<String, String>> {
    let mut collected = BTreeMap::new();
    for credential in credentials {
        if let Ok(value) = secrets::get(&credential.name) {
            collected.insert(credential.name.clone(), value);
        }
    }
    Ok(collected)
}

fn ask_new_passphrase(count: usize) -> Result<String> {
    let passphrase = if pick::interactive() {
        println!("{count} secret(s) will be encrypted with a passphrase.");
        println!("There is no way to recover them without it.");
        Password::new()
            .with_prompt("Passphrase")
            .with_confirmation("Repeat to confirm", "Passphrases do not match")
            .interact()?
    } else {
        read_passphrase_from_stdin()?
    };
    if passphrase.is_empty() {
        bail!("an empty passphrase would leave the secrets unprotected - nothing was written");
    }
    Ok(passphrase)
}

/// Ask for an existing passphrase, or take it from stdin when scripted.
fn read_passphrase(prompt: &str) -> Result<String> {
    if pick::interactive() {
        return Ok(Password::new().with_prompt(prompt).interact()?);
    }
    read_passphrase_from_stdin()
}

/// Scripted use: the passphrase arrives on stdin so it never lands in shell
/// history or a process listing, the same way `turnout pass set` takes a
/// secret. Without this, moving a machine could not be automated at all.
fn read_passphrase_from_stdin() -> Result<String> {
    use std::io::Read;
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer).context("cannot read the passphrase from stdin")?;
    let passphrase = buffer.trim_end_matches(['\r', '\n']).to_string();
    if passphrase.is_empty() {
        bail!("no passphrase on stdin - pipe it in, e.g. `echo -n SECRET | turnout import file.json`");
    }
    Ok(passphrase)
}

/// Write the export readable only by its owner.
///
/// Even without secrets this file lists hosts, logins and deploy paths, and it
/// is written into whatever directory the user happened to be in.
#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(contents.as_bytes()).with_context(|| format!("cannot write {}", path.display()))
}

/// Windows has no mode bits to set here; the file inherits the directory ACL.
#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Server;

    fn app(name: &str, path: &str) -> App {
        App {
            name: name.to_string(),
            path: path.to_string(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: None,
            gateway_env: None,
            env_file: None,
            dev_port: None,
            servers: Vec::new(),
        }
    }

    fn server(name: &str) -> Server {
        Server {
            name: name.to_string(),
            label: None,
            url: "https://staging.example.com".to_string(),
            host: None,
            port: 22,
            accept_invalid_certs: false,
            credential: None,
            shell: None,
        }
    }

    #[test]
    fn new_items_are_added() {
        let mut existing = vec![app("web", "/old")];
        let mut report = Report::default();
        merge(
            &mut existing,
            vec![app("api", "/api")],
            |a| a.name.clone(),
            false,
            "app",
            &mut report,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(existing.len(), 2);
        assert_eq!(report.imported, 1);
        assert!(report.skipped.is_empty());
    }

    /// The local setup wins by default: an import must never silently redirect
    /// an app the user is working in.
    #[test]
    fn existing_items_are_kept_and_reported() {
        let mut existing = vec![app("web", "/local/path")];
        let mut report = Report::default();
        merge(
            &mut existing,
            vec![app("web", "/imported/path")],
            |a| a.name.clone(),
            false,
            "app",
            &mut report,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].path, "/local/path", "the local entry must survive");
        assert_eq!(report.imported, 0);
        assert_eq!(report.skipped, vec!["app 'web'"]);
    }

    #[test]
    fn force_overwrites() {
        let mut existing = vec![app("web", "/local/path")];
        let mut report = Report::default();
        merge(
            &mut existing,
            vec![app("web", "/imported/path")],
            |a| a.name.clone(),
            true,
            "app",
            &mut report,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(existing[0].path, "/imported/path");
        assert_eq!(report.imported, 1);
        assert!(report.skipped.is_empty());
    }

    /// Credentials are keyed by their own name since v0.9.0 - the same account
    /// reaching two stands is one record, and a second one with a different
    /// name is a different login even if it reaches the same machine.
    #[test]
    fn credentials_are_keyed_by_name() {
        let credential = |name: &str, user: &str| crate::model::Credential {
            name: name.to_string(),
            user: user.to_string(),
            auth: crate::model::Auth::Password,
            key: None,
        };
        let mut existing = vec![credential("pi-deploy", "deploy")];
        let mut report = Report::default();
        merge(
            &mut existing,
            vec![credential("pi-root", "root"), credential("pi-deploy", "someone-else")],
            |c| c.name.clone(),
            false,
            "credential",
            &mut report,
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(existing.len(), 2, "a different name is a different login");
        assert_eq!(existing[0].user, "deploy", "the local entry must survive");
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, vec!["credential 'pi-deploy'"]);
    }

    #[test]
    fn an_empty_import_saves_nothing() {
        let mut existing = vec![server("pi")];
        let mut report = Report::default();
        // The save closure panics: an empty incoming list must not reach it.
        merge(
            &mut existing,
            Vec::new(),
            |s| s.name.clone(),
            false,
            "server",
            &mut report,
            |_| Ok(()),
            |_| panic!("nothing to save"),
        )
        .unwrap();
        assert_eq!(report.imported, 0);
    }

    /// An entry `add` would refuse is refused here too, named with the
    /// reason, and the rest of the import still lands.
    #[test]
    fn an_entry_add_would_refuse_is_refused_and_named() {
        let mut existing = Vec::new();
        let mut report = Report::default();
        let written = merge(
            &mut existing,
            vec![app("Not Valid", "/a"), app("api", "/api")],
            |a| a.name.clone(),
            true,
            "app",
            &mut report,
            |a| admit_app(a, &[]),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(written, vec!["api".to_string()]);
        assert_eq!(existing.len(), 1);
        assert_eq!(report.refused.len(), 1, "{:?}", report.refused);
        assert!(report.refused[0].starts_with("app 'Not Valid': invalid name"), "{:?}", report.refused);
    }

    /// App and group names share one namespace: `use NAME` takes either.
    #[test]
    fn an_app_and_a_group_cannot_share_a_name() {
        let group = Group {
            name: "web".into(),
            apps: vec!["api".into()],
        };
        let refused = admit_app(&app("web", "/web"), std::slice::from_ref(&group)).unwrap_err().to_string();
        assert!(refused.contains("a group named 'web' exists here"), "{refused}");
        let refused = admit_group(&group, &[app("web", "/web"), app("api", "/api")]).unwrap_err().to_string();
        assert!(refused.contains("an app named 'web' exists here"), "{refused}");
        assert!(admit_group(&group, &[app("api", "/api")]).is_ok());
        let refused = admit_group(&group, &[]).unwrap_err().to_string();
        assert!(refused.contains("member 'api' is not an app here"), "{refused}");
    }

    /// The dotenv file is written inside the project; a name with a slash in
    /// it would put turnout's line somewhere else on the disk.
    #[test]
    fn an_arriving_dotenv_file_stays_inside_the_project() {
        let mut outside = app("web", "/web");
        outside.env_file = Some("../../.bashrc".into());
        assert!(admit_app(&outside, &[]).is_err());
    }

    /// A target is refused when a part it names is neither here nor arrived.
    #[test]
    fn a_target_needs_all_four_parts() {
        let target = Target {
            name: "web-prod".into(),
            app: "web".into(),
            server: "prod".into(),
            credential: "deploy".into(),
            path: "wwwroot".into(),
            url: None,
            health: None,
            keep_backups: None,
        };
        let credential = crate::model::Credential {
            name: "deploy".into(),
            user: "deploy".into(),
            auth: crate::model::Auth::Password,
            key: None,
        };
        let path = crate::model::Path {
            name: "wwwroot".into(),
            dir: "/var/www/web".into(),
            restart: None,
        };
        let apps = [app("web", "/web")];
        let servers = [server("prod")];
        let credentials = [credential];
        let mut around = Catalog {
            apps: &apps,
            servers: &servers,
            credentials: &credentials,
            paths: &[],
        };
        let refused = admit_target(&target, &around).unwrap_err().to_string();
        assert!(refused.contains("path 'wwwroot'"), "{refused}");
        let paths = [path];
        around.paths = &paths;
        assert!(admit_target(&target, &around).is_ok());
    }
}
