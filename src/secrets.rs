use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result, bail};

use crate::paths;

/// Selects the secret backend. Unset or empty: the OS keyring.
/// `insecure-file`: a plain JSON file in the data directory - ONLY for tests
/// and throwaway environments; the value is stored unprotected.
pub const BACKEND_ENV: &str = "TURNOUT_KEYRING";

const KEYRING_SERVICE: &str = "turnout";
const FILE_STORE: &str = "insecure-secrets.json";

enum Backend {
    Keyring,
    InsecureFile,
}

fn backend() -> Result<Backend> {
    match std::env::var(BACKEND_ENV).unwrap_or_default().as_str() {
        "" => Ok(Backend::Keyring),
        "insecure-file" => Ok(Backend::InsecureFile),
        other => bail!("unsupported {BACKEND_ENV} value '{other}' (expected empty or 'insecure-file')"),
    }
}

fn account(server: &str, kind: &str) -> String {
    format!("{server}/{kind}")
}

pub fn set(server: &str, kind: &str, value: &str) -> Result<()> {
    match backend()? {
        Backend::Keyring => keyring::Entry::new(KEYRING_SERVICE, &account(server, kind))?
            .set_password(value)
            .context("cannot write the secret to the OS keyring"),
        Backend::InsecureFile => {
            let mut store = read_file_store()?;
            store.insert(account(server, kind), value.to_string());
            write_file_store(&store)
        }
    }
}

pub fn get(server: &str, kind: &str) -> Result<String> {
    let missing = || anyhow::anyhow!("no secret stored for '{server}' ({kind}) - run `turnout pass set {server}`");
    match backend()? {
        Backend::Keyring => match keyring::Entry::new(KEYRING_SERVICE, &account(server, kind))?.get_password() {
            Ok(value) => Ok(value),
            Err(keyring::Error::NoEntry) => Err(missing()),
            Err(err) => Err(err).context("cannot read the secret from the OS keyring"),
        },
        Backend::InsecureFile => read_file_store()?.get(&account(server, kind)).cloned().ok_or_else(missing),
    }
}

pub fn delete(server: &str, kind: &str) -> Result<()> {
    match backend()? {
        Backend::Keyring => match keyring::Entry::new(KEYRING_SERVICE, &account(server, kind))?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(err).context("cannot delete the secret from the OS keyring"),
        },
        Backend::InsecureFile => {
            let mut store = read_file_store()?;
            store.remove(&account(server, kind));
            write_file_store(&store)
        }
    }
}

fn read_file_store() -> Result<BTreeMap<String, String>> {
    let path = paths::data_dir()?.join(FILE_STORE);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))
}

fn write_file_store(store: &BTreeMap<String, String>) -> Result<()> {
    let path = paths::data_dir()?.join(FILE_STORE);
    fs::write(&path, serde_json::to_string_pretty(store)?).with_context(|| format!("cannot write {}", path.display()))
}
