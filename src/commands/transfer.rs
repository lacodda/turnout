//! `turnout export` and `turnout import`: move a setup to another machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use dialoguer::Password;

use crate::portable::{self, Snapshot};
use crate::{pick, secrets, store};

pub fn export(path: Option<PathBuf>, with_secrets: bool) -> Result<()> {
    let apps = store::load_apps()?;
    let servers = store::load_servers()?;
    let credentials = store::load_credentials()?;
    let groups = store::load_groups()?;
    if apps.is_empty() && servers.is_empty() && groups.is_empty() {
        bail!("nothing to export - this machine has no apps, servers or groups yet");
    }

    let mut snapshot = Snapshot::new(apps, servers, credentials, groups);
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
        "  {} app(s), {} server(s), {} group(s), {} access record(s)",
        snapshot.apps.len(),
        snapshot.servers.len(),
        snapshot.groups.len(),
        snapshot.credentials.len()
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
    let snapshot: Snapshot = serde_json::from_str(&text).with_context(|| format!("{} is not a turnout export", path.display()))?;
    snapshot.check_version()?;

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

    let mut report = Report::default();
    merge(
        &mut store::load_apps()?,
        snapshot.apps,
        |app| app.name.clone(),
        force,
        "app",
        &mut report,
        store::save_apps,
    )?;
    merge(
        &mut store::load_servers()?,
        snapshot.servers,
        |server| server.name.clone(),
        force,
        "server",
        &mut report,
        store::save_servers,
    )?;
    merge(
        &mut store::load_groups()?,
        snapshot.groups,
        |group| group.name.clone(),
        force,
        "group",
        &mut report,
        store::save_groups,
    )?;
    merge(
        &mut store::load_credentials()?,
        snapshot.credentials,
        |credential| format!("{}/{}", credential.server, credential.kind),
        force,
        "access",
        &mut report,
        store::save_credentials,
    )?;

    if let Some(opened) = opened {
        for (account, value) in &opened {
            let Some((server, kind)) = account.split_once('/') else {
                continue;
            };
            secrets::set(server, kind, value)?;
        }
        report.secrets = opened.len();
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
    secrets: usize,
}

impl Report {
    fn print(&self) {
        if self.imported == 0 && self.skipped.is_empty() {
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
        if self.secrets > 0 {
            println!("Restored {} secret(s) to the OS keyring.", self.secrets);
        }
    }
}

/// Add incoming items to `existing`, keeping what is already there unless
/// `force` says otherwise, then save through `save`.
fn merge<T, K, S>(existing: &mut Vec<T>, incoming: Vec<T>, key: K, force: bool, kind: &str, report: &mut Report, save: S) -> Result<()>
where
    K: Fn(&T) -> String,
    S: Fn(&[T]) -> Result<()>,
{
    if incoming.is_empty() {
        return Ok(());
    }
    for item in incoming {
        let name = key(&item);
        match existing.iter().position(|other| key(other) == name) {
            Some(index) if force => {
                existing[index] = item;
                report.imported += 1;
            }
            Some(_) => report.skipped.push(format!("{kind} '{name}'")),
            None => {
                existing.push(item);
                report.imported += 1;
            }
        }
    }
    save(existing)
}

/// Read every stored secret named by the access catalog.
///
/// A missing secret is not an error: the catalog records that access exists,
/// and the keyring may legitimately not have it on this machine.
fn collect_secrets(credentials: &[crate::model::Credential]) -> Result<BTreeMap<String, String>> {
    let mut collected = BTreeMap::new();
    for credential in credentials {
        if let Ok(value) = secrets::get(&credential.server, &credential.kind) {
            collected.insert(format!("{}/{}", credential.server, credential.kind), value);
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
fn write_private(path: &Path, contents: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("cannot write {}", path.display()))?;
        return file.write_all(contents.as_bytes()).with_context(|| format!("cannot write {}", path.display()));
    }
    #[cfg(not(unix))]
    std::fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{App, Server};

    fn app(name: &str, path: &str) -> App {
        App {
            name: name.to_string(),
            path: path.to_string(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: None,
            servers: Vec::new(),
        }
    }

    fn server(name: &str) -> Server {
        Server {
            name: name.to_string(),
            label: None,
            url: "https://staging.example.com".to_string(),
            ssh: None,
            accept_invalid_certs: false,
            deploy: BTreeMap::new(),
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
        )
        .unwrap();
        assert_eq!(existing[0].path, "/imported/path");
        assert_eq!(report.imported, 1);
        assert!(report.skipped.is_empty());
    }

    /// Access records are keyed by server *and* kind: the same server can hold
    /// a password and a token, and importing one must not hide the other.
    #[test]
    fn access_records_are_keyed_by_server_and_kind() {
        let credential = |server: &str, kind: &str| crate::model::Credential {
            server: server.to_string(),
            kind: kind.to_string(),
            login: "deploy".to_string(),
        };
        let mut existing = vec![credential("pi", "ssh")];
        let mut report = Report::default();
        merge(
            &mut existing,
            vec![credential("pi", "password"), credential("pi", "ssh")],
            |c| format!("{}/{}", c.server, c.kind),
            false,
            "access",
            &mut report,
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(existing.len(), 2, "a different kind on the same server is a different record");
        assert_eq!(report.imported, 1);
        assert_eq!(report.skipped, vec!["access 'pi/ssh'"]);
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
            |_| panic!("nothing to save"),
        )
        .unwrap();
        assert_eq!(report.imported, 0);
    }
}
