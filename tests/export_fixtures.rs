//! A settings export from every release since v0.11.0 imports into this one.
//!
//! `tests/fixtures/exports/vX.Y.Z.json` is the file that release's own binary
//! wrote for a catalog made through its own commands (`tools/export-fixture.ps1`
//! makes them, and the release ritual adds one per version). Each is imported
//! here into an empty data directory and exported again: everything the old
//! file said must come back unchanged, its secrets must open, and the catalog
//! must hold what this build promises of every catalog - however old the file.
//!
//! This is the proof behind "no breaking change to the settings format since
//! v0.11": a change that drops a field, renames one or refuses an old shape
//! turns one of these red.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;

/// What `tools/export-fixture.ps1` seals every fixture's secrets with.
const PASSPHRASE: &str = "fixture-passphrase";

fn fixtures() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exports");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("cannot read {}: {err}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

fn turnout(data_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("turnout").unwrap();
    cmd.env("TURNOUT_DATA_DIR", data_dir)
        .env("TURNOUT_KEYRING", "insecure-file")
        .env("TURNOUT_UPDATE_CHECK", "0")
        .env("TURNOUT_TOASTS", data_dir.join("toasts.jsonl"));
    cmd
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{} is not JSON: {err}", path.display()))
}

/// The entities of one kind, by name.
fn by_name<'a>(export: &'a Value, kind: &str) -> BTreeMap<&'a str, &'a Value> {
    export[kind]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| (item["name"].as_str().expect("every entity has a name"), item))
                .collect()
        })
        .unwrap_or_default()
}

/// Import `fixture` into a fresh data directory, then export it again with
/// this build. Returns the re-export and the data directory it came from.
fn round_trip(fixture: &Path) -> (Value, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    turnout(dir.path()).args(["setup", "--yes"]).assert().success();
    let imported = turnout(dir.path()).arg("import").arg(fixture).write_stdin(PASSPHRASE).output().unwrap();
    assert!(
        imported.status.success(),
        "{} does not import:\n{}",
        fixture.display(),
        String::from_utf8_lossy(&imported.stderr)
    );

    let again = dir.path().join("again.json");
    turnout(dir.path()).args(["export", "--output"]).arg(&again).assert().success();
    (read_json(&again), dir)
}

#[test]
fn every_fixture_imports_whole() {
    let fixtures = fixtures();
    assert!(fixtures.len() >= 20, "the fixtures since v0.11.0 are missing: {fixtures:?}");

    for fixture in &fixtures {
        let name = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let old = read_json(fixture);
        let (new, dir) = round_trip(fixture);
        nothing_is_lost(&name, &old, &new);
        every_app_has_a_gateway_port_of_its_own(&name, &new);
        the_secrets_arrive(&name, dir.path());
    }
}

/// Everything the old file said comes back from this build unchanged: no
/// entity dropped, no field renamed, no value rewritten.
fn nothing_is_lost(name: &str, old: &Value, new: &Value) {
    for kind in ["apps", "servers", "credentials", "paths", "groups", "targets"] {
        let before = by_name(old, kind);
        let after = by_name(new, kind);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "{name}: the {kind} that arrived are not the {kind} that left"
        );
        for (entity, fields) in &before {
            for (field, value) in fields.as_object().unwrap() {
                assert_eq!(&after[entity][field], value, "{name}: {kind} '{entity}' came back with a different `{field}`");
            }
        }
    }
}

/// Before v0.18.0 an app could have no gateway port, and before v0.13.1 two
/// apps could share one. This build gives every app a port of its own
/// (schema 4), and an import must not bring back what the migration took away.
fn every_app_has_a_gateway_port_of_its_own(name: &str, new: &Value) {
    let ports: Vec<u64> = by_name(new, "apps")
        .iter()
        .map(|(app, fields)| {
            fields["gateway_port"]
                .as_u64()
                .unwrap_or_else(|| panic!("{name}: app '{app}' arrived without a gateway port"))
        })
        .collect();
    let mut unique = ports.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ports.len(), "{name}: two apps share a gateway port: {ports:?}");
}

/// Sealed secrets from every release open with the passphrase they were
/// sealed with, and land under the credentials that own them.
fn the_secrets_arrive(name: &str, data_dir: &Path) {
    let stored = read_json(&data_dir.join("insecure-secrets.json"));
    assert_eq!(stored["deploy"], "fixture-password", "{name}: the password did not arrive");
    assert_eq!(stored["ci-key"], "fixture-key-passphrase", "{name}: the key's passphrase did not arrive");
}

/// The release ritual writes this version's fixture after the bump. Without
/// it, the next format change could ship with nothing holding the format of
/// this one in place.
#[test]
fn this_version_has_a_fixture() {
    let version = env!("CARGO_PKG_VERSION");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/exports/v{version}.json"));
    assert!(
        fixture.exists(),
        "no export fixture for v{version}: run `pwsh tools/export-fixture.ps1 -Binary target/debug/turnout{}`",
        std::env::consts::EXE_SUFFIX
    );
    assert_eq!(
        read_json(&fixture)["exported_by"],
        format!("turnout {version}"),
        "{} was not written by this version",
        fixture.display()
    );
}
