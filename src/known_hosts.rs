//! Which servers turnout trusts to be the ones it means: the host keys in
//! OpenSSH's own `known_hosts` file.
//!
//! Up to v0.27 turnout took whatever host key a server presented, so a deploy
//! credential's password went to anyone who could answer on the stand's
//! address. Now the first connection to a host records its key - trust on
//! first use, what `ssh -o StrictHostKeyChecking=accept-new` does - and every
//! later one must present that key or be refused.
//!
//! The file is OpenSSH's rather than one of turnout's: a key `ssh` already
//! trusts is trusted here, a key turnout learned is the one `turnout ssh` and
//! plain `ssh` then expect, and a reinstalled server is mended in one place
//! for both. Read as OpenSSH reads it - hashed names, `[host]:port`, `*` and
//! `?` patterns, negations, `@revoked` - and written as `ssh` writes it.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::{Entry, HostPatterns, Marker};
use russh::keys::{Algorithm, HashAlg, PublicKey};
use sha1::Sha1;

/// Environment override for the file; the tests and a script that keeps its
/// own trust use it.
pub const FILE_ENV: &str = "TURNOUT_KNOWN_HOSTS";

/// The `known_hosts` file host keys are checked against: `$TURNOUT_KNOWN_HOSTS`,
/// or `~/.ssh/known_hosts` - `%USERPROFILE%\.ssh\known_hosts` on Windows,
/// where OpenSSH for Windows keeps it too.
pub fn file() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(FILE_ENV) {
        return Ok(PathBuf::from(path));
    }
    // A unit test that connects to a loopback stand must not teach the
    // developer's own file a key for 127.0.0.1.
    if cfg!(test) {
        return Ok(std::env::temp_dir().join(format!("turnout-unit-{}-known_hosts", std::process::id())));
    }
    let dirs = directories::BaseDirs::new().context("cannot resolve the home directory for ~/.ssh/known_hosts")?;
    Ok(dirs.home_dir().join(".ssh").join("known_hosts"))
}

/// How the file names `host` on `port`: bare on 22, `[host]:port` elsewhere,
/// exactly as `ssh` writes and looks it up. Host names are compared in lower
/// case, as `ssh` does.
pub fn name(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 { host } else { format!("[{host}]:{port}") }
}

/// A host key as people compare it: `ED25519 SHA256:...`, the form `ssh`
/// prints when it asks.
pub fn describe(key: &PublicKey) -> String {
    let kind = match key.algorithm() {
        Algorithm::Ed25519 => "ED25519".to_string(),
        Algorithm::Rsa { .. } => "RSA".to_string(),
        Algorithm::Ecdsa { .. } => "ECDSA".to_string(),
        other => other.to_string(),
    };
    format!("{kind} {}", key.fingerprint(HashAlg::Sha256))
}

/// One line of the file that names the host.
#[derive(Debug)]
pub struct Known {
    /// 1-based, as an editor counts it.
    pub line: usize,
    pub key: PublicKey,
    revoked: bool,
}

/// What the file says about the key a host presented.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The file holds this key for the host.
    Known,
    /// The file held nothing for the host; the key is now written in.
    Learned,
    /// The file holds another key for the host, first on `line`.
    Changed { line: usize, known: String },
    /// The file marks this very key `@revoked`, on `line`.
    Revoked { line: usize },
}

/// Check `key` presented by `host:port` against the file at `path`, and learn
/// it when the file knows nothing of the host.
///
/// Any key the file holds for the host settles it, whatever its algorithm: a
/// server whose ED25519 key is known and which presents an RSA one is
/// refused, the way an interceptor that only has an RSA key would be. The
/// transport asks for the known algorithms first ([`algorithms`]), so a
/// server that still has its key presents it.
pub fn check(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<Verdict> {
    let name = name(host, port);
    let known = entries_for(&read(path)?, &name);
    if let Some(revoked) = known.iter().find(|k| k.revoked && same_key(&k.key, key)) {
        return Ok(Verdict::Revoked { line: revoked.line });
    }
    let trusted: Vec<&Known> = known.iter().filter(|k| !k.revoked).collect();
    if trusted.iter().any(|k| same_key(&k.key, key)) {
        return Ok(Verdict::Known);
    }
    if let Some(first) = trusted.first() {
        return Ok(Verdict::Changed {
            line: first.line,
            known: describe(&first.key),
        });
    }
    learn(path, &name, key)?;
    Ok(Verdict::Learned)
}

/// The keys the file trusts for `host:port`, in file order.
pub fn trusted(path: &Path, host: &str, port: u16) -> Result<Vec<Known>> {
    Ok(entries_for(&read(path)?, &name(host, port)).into_iter().filter(|k| !k.revoked).collect())
}

/// The algorithms of the keys the file trusts for the host, to be asked for
/// first: a server offers several host keys, and the one it would pick by
/// default need not be the one recorded here.
pub fn algorithms(path: &Path, host: &str, port: u16) -> Vec<Algorithm> {
    trusted(path, host, port)
        .map(|known| known.into_iter().map(|k| k.key.algorithm()).collect())
        .unwrap_or_default()
}

/// Put `key` in the place of whatever the file trusts for `host:port`: the
/// lines naming the host alone go, a line naming it among others loses only
/// that name, and the key is appended. `@revoked` lines stay - revocation is
/// not turnout's to lift.
///
/// Written beside the file and renamed over it, so an interrupted run leaves
/// the old file whole rather than half of it.
pub fn replace(path: &Path, host: &str, port: u16, key: &PublicKey) -> Result<usize> {
    let name = name(host, port);
    let text = read(path)?;
    let mut kept = String::with_capacity(text.len());
    let mut forgotten = 0;
    for raw in text.lines() {
        let entry = parse(raw).filter(|entry| entry.marker().is_none() && names(entry, &name));
        match entry.as_ref().map(Entry::host_patterns) {
            Some(HostPatterns::HashedName { .. }) => forgotten += 1,
            // Only a line that names the host itself is edited: a wildcard
            // covering it covers other hosts too, and is the owner's to keep.
            Some(HostPatterns::Patterns(patterns)) if patterns.iter().any(|p| pattern_is(p, &name)) => {
                forgotten += 1;
                let rest: Vec<&str> = patterns.iter().filter(|p| !pattern_is(p, &name)).map(String::as_str).collect();
                if !rest.is_empty() {
                    let key = entry.as_ref().map(Entry::public_key).expect("a pattern list comes from an entry");
                    let written = key.to_openssh().context("cannot write a host key in OpenSSH's form")?;
                    kept.push_str(&format!("{} {written}\n", rest.join(",")));
                }
            }
            _ => {
                kept.push_str(raw);
                kept.push('\n');
            }
        }
    }
    kept.push_str(&format!("{name} {}\n", openssh(key)?));
    write_whole(path, &kept)?;
    Ok(forgotten)
}

/// The file's text; a file that is not there yet knows nothing.
fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// One line as OpenSSH reads it, or `None` for a comment, a blank line and a
/// line it cannot read - a key type nobody here knows says nothing about the
/// host being checked, so it is passed over rather than failing every
/// connection.
fn parse(raw: &str) -> Option<Entry> {
    let content = raw.split_once('#').map_or(raw, |(before, _)| before);
    // Fields are separated by any run of blanks; the parser takes one space.
    let normal = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if normal.is_empty() {
        return None;
    }
    normal.parse().ok()
}

/// Every line naming `name`, certificate authorities aside: turnout does not
/// take host certificates, so a CA line trusts nothing here.
fn entries_for(text: &str, name: &str) -> Vec<Known> {
    text.lines()
        .enumerate()
        .filter_map(|(index, raw)| parse(raw).map(|entry| (index + 1, entry)))
        .filter(|(_, entry)| entry.marker() != Some(&Marker::CertAuthority) && names(entry, name))
        .map(|(line, entry)| Known {
            line,
            revoked: entry.marker() == Some(&Marker::Revoked),
            key: entry.public_key().clone(),
        })
        .collect()
}

/// Whether a line's host field names `name`, by OpenSSH's rules: a hashed
/// name is the HMAC-SHA1 of the name under its salt; a pattern list matches
/// when a pattern does and no negated one does.
fn names(entry: &Entry, name: &str) -> bool {
    match entry.host_patterns() {
        HostPatterns::HashedName { salt, hash } => Hmac::<Sha1>::new_from_slice(salt)
            .map(|mac| mac.chain_update(name.as_bytes()).verify_slice(hash).is_ok())
            .unwrap_or(false),
        HostPatterns::Patterns(patterns) => {
            let mut matched = false;
            for pattern in patterns {
                if let Some(negated) = pattern.strip_prefix('!') {
                    if glob(&negated.to_ascii_lowercase(), name) {
                        return false;
                    }
                } else if glob(&pattern.to_ascii_lowercase(), name) {
                    matched = true;
                }
            }
            matched
        }
    }
}

/// Whether `pattern` is `name` itself rather than a wildcard that covers it:
/// what `replace` takes out of a list naming several hosts.
fn pattern_is(pattern: &str, name: &str) -> bool {
    pattern.eq_ignore_ascii_case(name)
}

/// `*` and `?` the way OpenSSH's `match_pattern` reads them.
fn glob(pattern: &str, text: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some((b'*', rest)) => (0..=t.len()).any(|skip| go(rest, &t[skip..])),
            Some((b'?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    go(pattern.as_bytes(), text.as_bytes())
}

/// Whether two keys are one key. An RSA key's algorithm carries the hash it
/// was last used with, which says nothing about the key; the key data does.
pub fn same_key(a: &PublicKey, b: &PublicKey) -> bool {
    a.key_data() == b.key_data()
}

/// `ssh-ed25519 AAAA...`, without a comment: what `ssh` appends.
fn openssh(key: &PublicKey) -> Result<String> {
    let mut plain = key.clone();
    plain.set_comment("");
    plain.to_openssh().context("cannot write the host key in OpenSSH's form")
}

/// Append `name key` to the file, creating `~/.ssh` the way `ssh` would.
fn learn(path: &Path, name: &str, key: &PublicKey) -> Result<()> {
    let line = format!("{name} {}\n", openssh(key)?);
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    // A file whose last line has no newline would have this one glued to it.
    let glue = match std::fs::read(path) {
        Ok(bytes) if !bytes.is_empty() && !bytes.ends_with(b"\n") => "\n",
        _ => "",
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot write the host key to {}", path.display()))?;
    file.write_all(format!("{glue}{line}").as_bytes())
        .with_context(|| format!("cannot write the host key to {}", path.display()))
}

/// Replace the file's contents through a sibling and a rename.
fn write_whole(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    // One name per write, not per process: two replacements at once must not
    // write into the same sibling.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nth = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let sibling = path.with_extension(format!("turnout-{}-{nth}", std::process::id()));
    std::fs::write(&sibling, text).with_context(|| format!("cannot write {}", sibling.display()))?;
    std::fs::rename(&sibling, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// `~/.ssh`, made readable by its owner alone where that is a thing to ask -
/// `ssh` refuses a directory others can write to.
fn create_private_dir(dir: &Path) -> Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};

    use super::*;

    fn key(seed: u8) -> PublicKey {
        let pair = Ed25519Keypair::from_seed(&[seed; 32]);
        russh::keys::PrivateKey::new(KeypairData::Ed25519(pair), "").unwrap().public_key().clone()
    }

    fn line(name: &str, key: &PublicKey) -> String {
        format!("{name} {}", openssh(key).unwrap())
    }

    /// A hashed entry for `name` with a fixed salt, as `ssh-keygen -H` writes.
    fn hashed(name: &str, key: &PublicKey) -> String {
        let salt = vec![9u8; 20];
        let mut mac = Hmac::<Sha1>::new_from_slice(&salt).unwrap();
        mac.update(name.as_bytes());
        let mut hash = [0u8; 20];
        hash.copy_from_slice(&mac.finalize().into_bytes());
        format!("{} {}", HostPatterns::HashedName { salt, hash }.to_string(), openssh(key).unwrap())
    }

    fn file_with(lines: &[String]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        (dir, path)
    }

    #[test]
    fn a_first_contact_writes_the_key_and_the_next_finds_it() {
        let dir = tempfile::tempdir().unwrap();
        // Through a directory that is not there yet, the way ~/.ssh is not
        // on a machine that never ran ssh.
        let path = dir.path().join(".ssh").join("known_hosts");
        assert_eq!(check(&path, "Stand.Example.com", 2222, &key(1)).unwrap(), Verdict::Learned);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, format!("{}\n", line("[stand.example.com]:2222", &key(1))));
        assert_eq!(check(&path, "stand.example.com", 2222, &key(1)).unwrap(), Verdict::Known);
        // Port 22 is another entry, written bare.
        assert_eq!(check(&path, "stand.example.com", 22, &key(1)).unwrap(), Verdict::Learned);
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with(&format!("\n{}\n", line("stand.example.com", &key(1))))
        );
    }

    #[test]
    fn another_key_for_a_known_host_is_refused_and_not_learned() {
        let (_dir, path) = file_with(&["# a comment line counts as a line".into(), line("stand.example.com", &key(1))]);
        let before = std::fs::read_to_string(&path).unwrap();
        let verdict = check(&path, "stand.example.com", 22, &key(2)).unwrap();
        assert_eq!(
            verdict,
            Verdict::Changed {
                line: 2,
                known: describe(&key(1))
            }
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before, "a changed key must not be written in");
    }

    #[test]
    fn hashed_names_patterns_and_negations_are_read_as_openssh_reads_them() {
        let (_dir, path) = file_with(&[
            hashed("[stand.example.com]:2222", &key(1)),
            format!("*.example.org,!secret.example.org {}", openssh(&key(3)).unwrap()),
            // Blanks other than one space, and a trailing comment.
            format!("db.example.net\t {}   # the database", openssh(&key(4)).unwrap()),
            "not a key line at all".into(),
        ]);
        assert_eq!(check(&path, "stand.example.com", 2222, &key(1)).unwrap(), Verdict::Known);
        assert!(matches!(
            check(&path, "stand.example.com", 2222, &key(2)).unwrap(),
            Verdict::Changed { line: 1, .. }
        ));
        assert_eq!(check(&path, "web.example.org", 22, &key(3)).unwrap(), Verdict::Known);
        assert!(matches!(
            check(&path, "web.example.org", 22, &key(5)).unwrap(),
            Verdict::Changed { line: 2, .. }
        ));
        // Negated: the wildcard line says nothing about this host.
        assert_eq!(check(&path, "secret.example.org", 22, &key(5)).unwrap(), Verdict::Learned);
        assert_eq!(check(&path, "db.example.net", 22, &key(4)).unwrap(), Verdict::Known);
    }

    #[test]
    fn a_revoked_key_is_refused_and_a_certificate_authority_trusts_nothing() {
        let (_dir, path) = file_with(&[
            format!("@revoked stand.example.com {}", openssh(&key(1)).unwrap()),
            format!("@cert-authority *.example.com {}", openssh(&key(2)).unwrap()),
        ]);
        assert_eq!(check(&path, "stand.example.com", 22, &key(1)).unwrap(), Verdict::Revoked { line: 1 });
        // The CA line is not a host key of web.example.com, so this is a
        // first contact.
        assert_eq!(check(&path, "web.example.com", 22, &key(2)).unwrap(), Verdict::Learned);
    }

    #[test]
    fn replacing_forgets_the_host_and_keeps_everything_else() {
        let (_dir, path) = file_with(&[
            "# kept".into(),
            line("stand.example.com", &key(1)),
            format!("other.example.com,stand.example.com {}", openssh(&key(1)).unwrap()),
            hashed("stand.example.com", &key(1)),
            format!("@revoked stand.example.com {}", openssh(&key(9)).unwrap()),
            line("other.example.com", &key(3)),
        ]);
        assert_eq!(replace(&path, "stand.example.com", 22, &key(2)).unwrap(), 3);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            [
                "# kept".to_string(),
                line("other.example.com", &key(1)),
                format!("@revoked stand.example.com {}", openssh(&key(9)).unwrap()),
                line("other.example.com", &key(3)),
                line("stand.example.com", &key(2)),
            ]
            .join("\n")
                + "\n"
        );
        assert_eq!(check(&path, "stand.example.com", 22, &key(2)).unwrap(), Verdict::Known);
        assert_eq!(check(&path, "other.example.com", 22, &key(1)).unwrap(), Verdict::Known);
    }

    #[test]
    fn a_learned_line_never_glues_onto_a_last_line_without_a_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        std::fs::write(&path, line("other.example.com", &key(3))).unwrap();
        check(&path, "stand.example.com", 22, &key(1)).unwrap();
        assert_eq!(check(&path, "other.example.com", 22, &key(3)).unwrap(), Verdict::Known);
        assert_eq!(check(&path, "stand.example.com", 22, &key(1)).unwrap(), Verdict::Known);
    }

    #[test]
    fn the_known_algorithms_are_the_trusted_keys_ones() {
        let (_dir, path) = file_with(&[line("stand.example.com", &key(1))]);
        assert_eq!(algorithms(&path, "stand.example.com", 22), vec![Algorithm::Ed25519]);
        assert!(algorithms(&path, "unknown.example.com", 22).is_empty());
    }

    #[test]
    fn unit_tests_never_reach_the_real_file() {
        if std::env::var_os(FILE_ENV).is_some() {
            return;
        }
        let path = file().unwrap();
        assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
    }
}
