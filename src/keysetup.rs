//! Getting a key *onto* a server, as opposed to using one that is already
//! there.
//!
//! Signing in with a key has worked since v0.10.0 (russh). What was still done
//! by hand was everything before that: generate a key, copy the public half to
//! the right file on the server, give that file the permissions sshd insists
//! on, and only then switch the credential over. `ssh-copy-id` does the Unix
//! half of it and does not exist on Windows - neither as a client nor for a
//! Windows *server*, which keeps administrators' keys somewhere else entirely.
//!
//! This module is the part of that with no network in it: which file the key
//! belongs in, whether it is already there, what the key looks like on disk.
//! The commands that carry it across the wire live in
//! [`crate::commands::key`]; keeping the decisions here is what lets the
//! Windows-server path be tested from a Linux CI box that cannot reach one.

use anyhow::{Context, Result, bail};
use russh::keys::PrivateKey;
use russh::keys::ssh_key::{Algorithm, LineEnding};

use crate::shell::Dialect;

/// Where a server keeps the authorized keys of the account we logged in as.
///
/// The Windows split is not cosmetic. OpenSSH on Windows ships a default
/// `sshd_config` with a `Match Group administrators` block pointing at one
/// shared `administrators_authorized_keys` under ProgramData, and for members
/// of that group it reads *only* that file - a key written to the user's own
/// `~/.ssh/authorized_keys` is ignored without a word. Since the deploy account
/// on a Windows stand is usually an administrator, guessing wrong here is the
/// difference between "works" and "silently keeps asking for the password".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedKeysFile {
    /// Directory holding the file, absolute on the server.
    pub dir: String,
    /// The file itself, absolute on the server.
    pub file: String,
    /// Whether this is the shared administrators file rather than the
    /// account's own.
    pub administrators: bool,
}

/// The ProgramData path OpenSSH on Windows uses for the administrators file.
///
/// Hard-coded deliberately: `%ProgramData%` is expanded by the *remote* shell,
/// and reading it back is one more round trip for a path that has not moved
/// since Windows OpenSSH shipped. If a server does move it, the check after
/// installation is what catches it.
const WINDOWS_ADMIN_DIR: &str = "C:\\ProgramData\\ssh";

impl AuthorizedKeysFile {
    /// Where the key goes, given the shell and the remote home directory.
    ///
    /// `administrator` is what the caller learned about the account (see
    /// [`reads_as_administrator`]); on POSIX it makes no difference, because
    /// root's keys live in root's own home like everyone else's.
    pub fn locate(dialect: Dialect, home: &str, administrator: bool) -> Self {
        let home = home.trim_end_matches(['/', '\\']);
        match dialect {
            Dialect::Posix => Self {
                dir: format!("{home}/.ssh"),
                file: format!("{home}/.ssh/authorized_keys"),
                administrators: false,
            },
            Dialect::Windows if administrator => Self {
                dir: WINDOWS_ADMIN_DIR.to_string(),
                file: format!("{WINDOWS_ADMIN_DIR}\\administrators_authorized_keys"),
                administrators: true,
            },
            Dialect::Windows => Self {
                dir: format!("{home}\\.ssh"),
                file: format!("{home}\\.ssh\\authorized_keys"),
                administrators: false,
            },
        }
    }
}

/// The command that asks a Windows server whether this account is an
/// administrator.
///
/// `net session` needs administrator rights and fails without them; it is the
/// check that does not depend on the group being named "Administrators", which
/// it is not on a localized Windows. Output is discarded - only the exit status
/// carries the answer.
pub const WINDOWS_ADMIN_PROBE: &str = "net session >nul 2>&1";

/// Read the administrator probe's result.
///
/// An error means the command failed, which is the answer "no" rather than a
/// problem to report: a non-administrator running `net session` is expected to
/// be refused.
pub fn reads_as_administrator(probe: Result<String>) -> bool {
    probe.is_ok()
}

/// Whether `line` is already among the authorized keys in `existing`.
///
/// Comparison is on the algorithm and the base64 body, not the whole line: the
/// comment at the end is free text that a user may have edited, and options
/// (`command=`, `from=`) may sit in front of a key someone deliberately
/// restricted. Matching on the body means an installed key is recognized in
/// both cases - and, in the second, that turnout does not quietly append an
/// unrestricted copy of a key the administrator had fenced in.
pub fn already_authorized(existing: &str, line: &str) -> bool {
    let Some(wanted) = key_body(line) else {
        return false;
    };
    existing.lines().filter_map(key_body).any(|body| body == wanted)
}

/// The `algorithm base64` core of an authorized-keys line, ignoring leading
/// options and the trailing comment.
fn key_body(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // The key type is the first field that names a known algorithm; anything
    // before it is an options list.
    line.split_whitespace()
        .zip(line.split_whitespace().skip(1))
        .find(|(kind, _)| kind.starts_with("ssh-") || kind.starts_with("ecdsa-") || kind.starts_with("sk-"))
        .map(|(kind, body)| (kind.to_string(), body.to_string()))
}

/// A freshly generated key, held in memory until both halves are placed.
pub struct GeneratedKey {
    /// The OpenSSH-format private key, exactly as it should land on disk.
    pub private: String,
    /// The single-line public key, exactly as it should land in
    /// `authorized_keys`.
    pub public: String,
}

/// Generate an ed25519 key pair.
///
/// ed25519 and nothing else: it is what every OpenSSH since 6.5 accepts, the
/// keys are short, and it sidesteps the RSA signature-algorithm negotiation
/// that made the old transport fail. The comment is what a human reads in
/// `authorized_keys` a year later, so it names the machine the key was made on.
pub fn generate(comment: &str) -> Result<GeneratedKey> {
    let mut key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).context("cannot generate an ed25519 key")?;
    key.set_comment(comment);
    let private = key.to_openssh(LineEnding::LF).context("cannot serialize the generated key")?.to_string();
    let public = key.public_key().to_openssh().context("cannot serialize the generated public key")?;
    Ok(GeneratedKey { private, public })
}

/// The public half of a key already on this machine.
///
/// Prefers the `.pub` file next to it - that is where OpenSSH puts it and it
/// carries the comment the user chose. Falls back to deriving the public key
/// from the private one, which is always possible but loses the comment, so a
/// derived line gets one naming this machine instead of an empty tail.
pub fn public_line(key_path: &str, passphrase: Option<&str>, fallback_comment: &str) -> Result<String> {
    let pub_path = format!("{key_path}.pub");
    if let Ok(text) = std::fs::read_to_string(&pub_path) {
        let line = text.trim();
        if !line.is_empty() {
            return Ok(line.to_string());
        }
    }
    let key = russh::keys::load_secret_key(key_path, passphrase).with_context(|| format!("cannot read the key file {key_path}"))?;
    let mut public = key.public_key().to_openssh().context("cannot derive the public key")?;
    if key_body(&public).is_some() && public.split_whitespace().count() < 3 {
        public.push(' ');
        public.push_str(fallback_comment);
    }
    Ok(public)
}

/// Refuse a public key line that the remote shell cannot carry literally.
///
/// A public key is base64 plus a comment, so this only ever fires on a comment
/// someone put a quote or a percent sign into - but the check is what keeps a
/// mangled line out of `authorized_keys`, where it would fail authentication
/// with no visible cause.
pub fn check_line(dialect: Dialect, line: &str) -> Result<()> {
    if let Err(reason) = dialect.reject_unquotable(line) {
        bail!("this public key cannot be installed on a cmd.exe server: {reason}");
    }
    if line.lines().count() != 1 {
        bail!("a public key must be a single line");
    }
    Ok(())
}

/// The default location for a key turnout generates for `credential`.
///
/// Under the user's `~/.ssh` because that is where every other tool looks for
/// keys - a key turnout made should still work with plain `ssh`, and one made
/// by `ssh-keygen` should still work here.
pub fn default_key_path(credential: &str) -> Result<std::path::PathBuf> {
    let home = directories::UserDirs::new().ok_or_else(|| anyhow::anyhow!("cannot locate the home directory"))?;
    Ok(home.home_dir().join(".ssh").join(format!("id_ed25519_{credential}")))
}

/// Write a private key to disk with owner-only permissions.
///
/// The permissions are the point: OpenSSH refuses a key file others can read,
/// and a key written world-readable would work through turnout (which loads the
/// file itself) while failing through `ssh` - a difference that is maddening to
/// diagnose.
pub fn write_private_key(path: &std::path::Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    }
    if path.exists() {
        bail!("{} already exists - remove it or point the credential at it instead", path.display());
    }
    std::fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))?;
    restrict_key_file(path)?;
    Ok(())
}

/// Make a key file readable only by its owner.
#[cfg(unix)]
fn restrict_key_file(path: &std::path::Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).with_context(|| format!("cannot restrict {}", path.display()))
}

/// On Windows the file inherits the profile directory's ACL, which is already
/// owner-only for `%USERPROFILE%\.ssh`; there is no mode to set.
#[cfg(not(unix))]
fn restrict_key_file(_path: &std::path::Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Windows administrator's key must go to the shared ProgramData file:
    /// sshd's default `Match Group administrators` block reads that one and
    /// ignores the account's own, so writing to `~/.ssh` there installs a key
    /// that will never be used.
    #[test]
    fn a_windows_administrator_gets_the_programdata_file() {
        let placed = AuthorizedKeysFile::locate(Dialect::Windows, "C:\\Users\\deploy", true);
        assert!(placed.administrators);
        assert_eq!(placed.dir, "C:\\ProgramData\\ssh");
        assert_eq!(placed.file, "C:\\ProgramData\\ssh\\administrators_authorized_keys");
    }

    /// An ordinary Windows account keeps its keys in its own profile.
    #[test]
    fn an_ordinary_windows_account_gets_its_own_file() {
        let placed = AuthorizedKeysFile::locate(Dialect::Windows, "C:\\Users\\deploy", false);
        assert!(!placed.administrators);
        assert_eq!(placed.file, "C:\\Users\\deploy\\.ssh\\authorized_keys");
    }

    /// POSIX has no such split - root's keys live in root's home like anyone
    /// else's, so the administrator answer must not change the path.
    #[test]
    fn posix_ignores_the_administrator_question() {
        let user = AuthorizedKeysFile::locate(Dialect::Posix, "/home/pi", false);
        let root = AuthorizedKeysFile::locate(Dialect::Posix, "/root", true);
        assert_eq!(user.file, "/home/pi/.ssh/authorized_keys");
        assert_eq!(root.file, "/root/.ssh/authorized_keys");
        assert!(!root.administrators);
    }

    /// `echo $HOME` on a server whose home is `/` - or a shell that appends a
    /// separator - must not produce a doubled slash, which some sshd builds
    /// treat as a different path than the one they check permissions on.
    #[test]
    fn a_trailing_separator_does_not_double() {
        assert_eq!(
            AuthorizedKeysFile::locate(Dialect::Posix, "/home/pi/", false).file,
            "/home/pi/.ssh/authorized_keys"
        );
        assert_eq!(
            AuthorizedKeysFile::locate(Dialect::Windows, "C:\\Users\\deploy\\", false).file,
            "C:\\Users\\deploy\\.ssh\\authorized_keys"
        );
    }

    /// Installing twice must leave one entry. The comment is not part of the
    /// identity: the same key with a different comment is the same key.
    #[test]
    fn an_installed_key_is_recognized_whatever_its_comment() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample turnout@desktop";
        let existing = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample someone-elses-comment\n";
        assert!(already_authorized(existing, line));
    }

    /// A key someone restricted with options is still that key. Appending an
    /// unrestricted copy would quietly undo the restriction, which is worse
    /// than a duplicate line.
    #[test]
    fn a_key_behind_options_still_counts_as_installed() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample turnout@desktop";
        let existing = "from=\"10.0.0.0/8\",no-pty ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample locked-down\n";
        assert!(already_authorized(existing, line));
    }

    /// A different key is not installed, however similar the file looks.
    #[test]
    fn a_different_key_is_not_installed() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIExample turnout@desktop";
        let existing = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDifferent other\n# a comment\n\n";
        assert!(!already_authorized(existing, line));
        assert!(!already_authorized("", line));
    }

    /// An empty or commented file is simply not holding the key.
    #[test]
    fn comments_and_blank_lines_hold_no_keys() {
        assert!(!already_authorized("# nothing here\n\n   \n", "ssh-ed25519 AAAAB body"));
    }

    /// Generation produces a usable pair: a private key the loader accepts and
    /// a public line `authorized_keys` would take.
    #[test]
    fn generates_a_usable_ed25519_pair() {
        let key = generate("turnout@test").expect("generate a key");
        assert!(key.private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(key.public.starts_with("ssh-ed25519 "));
        assert!(key.public.ends_with(" turnout@test"), "{}", key.public);
        assert_eq!(key.public.lines().count(), 1);

        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("id_ed25519");
        write_private_key(&path, &key.private).expect("write the key");
        let loaded = russh::keys::load_secret_key(path.display().to_string(), None).expect("the written key loads back");
        assert_eq!(loaded.public_key().to_openssh().expect("serialize"), key.public);
    }

    /// Two generated keys must differ - a fixed seed would hand every user of
    /// turnout the same private key.
    #[test]
    fn every_generated_key_is_new() {
        let first = generate("turnout@test").expect("generate");
        let second = generate("turnout@test").expect("generate");
        assert_ne!(first.public, second.public);
    }

    /// Overwriting a key file would destroy access to every server that
    /// already trusts it, so an existing path is refused rather than replaced.
    #[test]
    fn an_existing_key_file_is_never_overwritten() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("id_ed25519");
        std::fs::write(&path, "precious").expect("seed the file");
        let error = write_private_key(&path, "new key").expect_err("must refuse").to_string();
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).expect("read back"), "precious");
    }

    /// A key file on this machine is published from its `.pub` sibling, which
    /// is where OpenSSH puts it and carries the comment the user chose.
    #[test]
    fn the_public_half_comes_from_the_pub_file_when_there_is_one() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("id_ed25519");
        let key = generate("original@comment").expect("generate");
        write_private_key(&path, &key.private).expect("write the key");
        std::fs::write(path.with_extension("").with_file_name("id_ed25519.pub"), "ssh-ed25519 AAAAB chosen@comment\n").expect("write the pub");

        let line = public_line(&path.display().to_string(), None, "turnout@here").expect("read the public half");
        assert_eq!(line, "ssh-ed25519 AAAAB chosen@comment");
    }

    /// Without a `.pub` file the public half is derived from the private key,
    /// and gets a comment naming this machine rather than none at all.
    #[test]
    fn the_public_half_is_derived_when_there_is_no_pub_file() {
        let scratch = tempfile::tempdir().expect("a scratch directory");
        let path = scratch.path().join("id_ed25519");
        let key = generate("").expect("generate");
        write_private_key(&path, &key.private).expect("write the key");

        let line = public_line(&path.display().to_string(), None, "turnout@here").expect("derive the public half");
        assert!(line.starts_with("ssh-ed25519 "), "{line}");
        assert!(line.ends_with(" turnout@here"), "{line}");
    }

    /// Nothing that cmd.exe would mangle may reach `authorized_keys`: a broken
    /// line there fails authentication with no visible cause.
    #[test]
    fn a_line_cmd_cannot_carry_is_refused() {
        let good = "ssh-ed25519 AAAAB turnout@desktop";
        assert!(check_line(Dialect::Windows, good).is_ok());
        assert!(check_line(Dialect::Posix, good).is_ok());
        assert!(check_line(Dialect::Windows, "ssh-ed25519 AAAAB say \"hi\"").is_err());
        assert!(check_line(Dialect::Windows, "ssh-ed25519 AAAAB 100%done").is_err());
        assert!(check_line(Dialect::Posix, "ssh-ed25519 AAAAB one\nssh-ed25519 AAAAB two").is_err());
    }

    /// The administrator probe answers by exit status: being refused is the
    /// answer "no", not a failure to report.
    #[test]
    fn a_refused_probe_means_not_an_administrator() {
        assert!(reads_as_administrator(Ok(String::new())));
        assert!(!reads_as_administrator(Err(anyhow::anyhow!("exited with 1"))));
    }
}
