//! What every test that runs the binary shares: a turnout that touches
//! nothing of the machine it runs on.
//!
//! One helper for every test crate, because three copies had already drifted
//! apart - one read the real OS keyring, another left the clipboard real.

// Each test crate uses a different part of this module.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The environment that keeps a turnout under test inside `data_dir`:
///
/// - its catalogs, logs and journal in the data directory;
/// - no update check reaching github.com from every `setup`;
/// - notifications into a file, not onto the desktop of whoever runs the suite;
/// - secrets into a file, not the OS keyring - a credential named `deploy` is
///   the default name, and a real one on the machine must not answer;
/// - the clipboard into a file, not over whatever the person had copied;
/// - host keys into a known_hosts of its own, not the person's ~/.ssh one.
///
/// The few tests that exercise one of these switch it back on themselves.
pub fn isolation(data_dir: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("TURNOUT_DATA_DIR", data_dir.to_path_buf()),
        ("TURNOUT_UPDATE_CHECK", PathBuf::from("0")),
        ("TURNOUT_TOASTS", toast_file(data_dir)),
        ("TURNOUT_KEYRING", PathBuf::from("insecure-file")),
        ("TURNOUT_CLIPBOARD", clipboard_file(data_dir)),
        ("TURNOUT_KNOWN_HOSTS", data_dir.join("known_hosts")),
    ]
}

/// The binary under test, isolated in `data_dir`.
pub fn turnout(data_dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("turnout").unwrap();
    cmd.envs(isolation(data_dir));
    cmd
}

/// The binary as a plain `std` process - for one that keeps running, like
/// `gateway run` - isolated the same way.
pub fn turnout_process(data_dir: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin("turnout"));
    cmd.envs(isolation(data_dir));
    cmd
}

pub fn toast_file(data_dir: &Path) -> PathBuf {
    data_dir.join("toasts.jsonl")
}

pub fn clipboard_file(data_dir: &Path) -> PathBuf {
    data_dir.join("clipboard.txt")
}

/// What turnout last put on the clipboard; `None` when it put nothing.
pub fn clipboard(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(clipboard_file(data_dir)).ok()
}
