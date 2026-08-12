//! The portable snapshot behind `turnout export` and `turnout import`.
//!
//! Catalogs travel as plain JSON: they are configuration, and a file the user
//! can read, diff and edit is worth more than an opaque blob.
//!
//! Secrets are the exception and only travel when asked for (`--with-secrets`).
//! They are sealed with a passphrase before they reach the file, because an
//! export is exactly the kind of file that gets attached to a chat, synced to a
//! cloud folder or left in Downloads. Plain-text secrets in it would undo the
//! point of keeping them in the OS keyring at all.
//!
//! The sealed form is Argon2id for the key and ChaCha20-Poly1305 for the
//! payload: the passphrase is the weak link, so the KDF is the part that has to
//! be slow, and the AEAD is what makes a tampered file fail loudly instead of
//! decrypting into nonsense.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::{App, Credential, Group, Server};

/// Bumped when the shape below changes in a way an older turnout cannot read.
const FORMAT_VERSION: u32 = 1;

/// Argon2id parameters. Deliberately above the crate defaults: this guards a
/// file that can be copied and attacked offline for as long as an attacker
/// likes, and a second of work on export is a price the user pays once.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_PARALLELISM: u32 = 4;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// Everything `export` writes and `import` reads.
#[derive(Serialize, Deserialize)]
pub struct Snapshot {
    /// Format version, not the turnout version - see [`FORMAT_VERSION`].
    pub version: u32,
    /// Which turnout produced this, for diagnosing a file after the fact.
    pub exported_by: String,
    #[serde(default)]
    pub apps: Vec<App>,
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub credentials: Vec<Credential>,
    #[serde(default)]
    pub groups: Vec<Group>,
    /// Sealed secrets, present only when exported with `--with-secrets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<SealedSecrets>,
}

/// The encrypted secret bundle: `server/kind` -> secret, sealed as one unit.
///
/// Sealing the map as a whole rather than value by value keeps the number of
/// stored accesses out of a casual reader's view, and means one passphrase
/// prompt instead of one per secret.
#[derive(Serialize, Deserialize)]
pub struct SealedSecrets {
    /// Key derivation, spelled out so a future turnout can still open old files
    /// after the defaults change.
    pub kdf: Kdf,
    /// Base64, standard alphabet with padding - JSON has no byte type.
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Serialize, Deserialize)]
pub struct Kdf {
    /// Only `argon2id` today; named so an unknown value fails clearly.
    pub algorithm: String,
    pub salt: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Snapshot {
    pub fn new(apps: Vec<App>, servers: Vec<Server>, credentials: Vec<Credential>, groups: Vec<Group>) -> Self {
        Self {
            version: FORMAT_VERSION,
            exported_by: format!("turnout {}", env!("CARGO_PKG_VERSION")),
            apps,
            servers,
            credentials,
            groups,
            secrets: None,
        }
    }

    /// Reject a file from a future turnout instead of silently importing the
    /// parts that happen to still parse.
    pub fn check_version(&self) -> Result<()> {
        if self.version > FORMAT_VERSION {
            bail!(
                "this export uses format version {} but this turnout understands up to {FORMAT_VERSION} - update turnout with `turnout self-update`",
                self.version
            );
        }
        Ok(())
    }
}

/// Encrypt `secrets` under `passphrase`.
pub fn seal(secrets: &BTreeMap<String, String>, passphrase: &str) -> Result<SealedSecrets> {
    use chacha20poly1305::aead::{Aead, KeyInit};

    let plaintext = serde_json::to_vec(secrets)?;
    let salt = random_bytes(SALT_LEN);
    let key = derive_key(passphrase, &salt)?;
    let nonce_bytes = random_bytes(NONCE_LEN);

    let nonce = chacha20poly1305::Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| anyhow::anyhow!("cannot build a nonce"))?;
    let cipher = chacha20poly1305::ChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_slice())
        .map_err(|_| anyhow::anyhow!("cannot encrypt the secrets"))?;

    Ok(SealedSecrets {
        kdf: Kdf {
            algorithm: "argon2id".to_string(),
            salt: encode_base64(&salt),
            memory_kib: KDF_MEMORY_KIB,
            iterations: KDF_ITERATIONS,
            parallelism: KDF_PARALLELISM,
        },
        nonce: encode_base64(&nonce_bytes),
        ciphertext: encode_base64(&ciphertext),
    })
}

/// Decrypt what [`seal`] produced.
///
/// A wrong passphrase and a tampered file are indistinguishable here, and both
/// come back as the same error - which is the honest answer, since the AEAD tag
/// cannot tell the difference either.
pub fn open(sealed: &SealedSecrets, passphrase: &str) -> Result<BTreeMap<String, String>> {
    use chacha20poly1305::aead::{Aead, KeyInit};

    if sealed.kdf.algorithm != "argon2id" {
        bail!("unsupported key derivation '{}' in this export", sealed.kdf.algorithm);
    }
    let salt = decode_base64(&sealed.kdf.salt).context("the export has a malformed salt")?;
    let nonce_bytes = decode_base64(&sealed.nonce).context("the export has a malformed nonce")?;
    if nonce_bytes.len() != NONCE_LEN {
        bail!("the export has a malformed nonce");
    }
    let ciphertext = decode_base64(&sealed.ciphertext).context("the export has malformed ciphertext")?;

    let nonce = chacha20poly1305::Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| anyhow::anyhow!("the export has a malformed nonce"))?;
    let key = derive_key_with(passphrase, &salt, sealed.kdf.memory_kib, sealed.kdf.iterations, sealed.kdf.parallelism)?;
    let cipher = chacha20poly1305::ChaCha20Poly1305::new((&key).into());
    let plaintext = cipher
        .decrypt(&nonce, ciphertext.as_slice())
        .map_err(|_| anyhow::anyhow!("cannot decrypt the secrets: wrong passphrase, or the file has been altered"))?;

    serde_json::from_slice(&plaintext).context("the decrypted secrets are not valid JSON")
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    derive_key_with(passphrase, salt, KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_PARALLELISM)
}

fn derive_key_with(passphrase: &str, salt: &[u8], memory_kib: u32, iterations: u32, parallelism: u32) -> Result<[u8; 32]> {
    let params =
        argon2::Params::new(memory_kib, iterations, parallelism, Some(32)).map_err(|err| anyhow::anyhow!("invalid key derivation parameters: {err}"))?;
    let argon2 = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|err| anyhow::anyhow!("cannot derive a key from the passphrase: {err}"))?;
    Ok(key)
}

fn random_bytes(len: usize) -> Vec<u8> {
    use rand::Rng;
    let mut bytes = vec![0u8; len];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

/// Minimal standard base64. A dependency for this alone is not worth it, and
/// the alphabet has not changed since 1987.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    let value = |c: u8| -> Result<u32> {
        Ok(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => bail!("invalid base64"),
        })
    };
    let bytes: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !bytes.len().is_multiple_of(4) {
        bail!("invalid base64");
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let padding = chunk.iter().filter(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && chunk[3] != b'=') {
            bail!("invalid base64");
        }
        let mut n = 0u32;
        for &c in chunk {
            n = n << 6 | if c == b'=' { 0 } else { value(c)? };
        }
        out.push((n >> 16 & 255) as u8);
        if padding < 2 {
            out.push((n >> 8 & 255) as u8);
        }
        if padding < 1 {
            out.push((n & 255) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("staging/password".to_string(), "hunter2".to_string()),
            ("prod/token".to_string(), "ghp_example".to_string()),
        ])
    }

    #[test]
    fn secrets_survive_a_round_trip() {
        let sealed = seal(&secrets(), "correct horse battery staple").unwrap();
        let opened = open(&sealed, "correct horse battery staple").unwrap();
        assert_eq!(opened, secrets());
    }

    /// The whole point of the passphrase: the file is useless without it.
    #[test]
    fn a_wrong_passphrase_fails() {
        let sealed = seal(&secrets(), "right").unwrap();
        let err = open(&sealed, "wrong").unwrap_err().to_string();
        assert!(err.contains("wrong passphrase"), "{err}");
    }

    /// An export can sit in Downloads or a cloud folder; a reader must not be
    /// able to pick values out of it.
    #[test]
    fn no_secret_value_appears_in_the_sealed_form() {
        let sealed = seal(&secrets(), "passphrase").unwrap();
        let json = serde_json::to_string(&sealed).unwrap();
        assert!(!json.contains("hunter2"), "{json}");
        assert!(!json.contains("ghp_example"), "{json}");
        assert!(!json.contains("staging/password"), "even the account names must not leak: {json}");
    }

    /// Same input twice must not produce the same file: a fresh salt and nonce
    /// are what stop two exports from being comparable.
    #[test]
    fn sealing_twice_gives_different_ciphertext() {
        let first = seal(&secrets(), "passphrase").unwrap();
        let second = seal(&secrets(), "passphrase").unwrap();
        assert_ne!(first.ciphertext, second.ciphertext);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.kdf.salt, second.kdf.salt);
    }

    /// AEAD's job: a file edited in transit must fail loudly, not decrypt into
    /// something subtly wrong.
    #[test]
    fn a_tampered_file_is_rejected() {
        let mut sealed = seal(&secrets(), "passphrase").unwrap();
        let mut bytes = decode_base64(&sealed.ciphertext).unwrap();
        bytes[0] ^= 0x01;
        sealed.ciphertext = encode_base64(&bytes);
        assert!(open(&sealed, "passphrase").is_err());
    }

    #[test]
    fn an_unknown_kdf_is_refused() {
        let mut sealed = seal(&secrets(), "passphrase").unwrap();
        sealed.kdf.algorithm = "scrypt".to_string();
        let err = open(&sealed, "passphrase").unwrap_err().to_string();
        assert!(err.contains("unsupported key derivation"), "{err}");
    }

    /// Empty is a legitimate state - no secrets stored - and must not become a
    /// special case that panics somewhere downstream.
    #[test]
    fn an_empty_secret_set_round_trips() {
        let empty = BTreeMap::new();
        let sealed = seal(&empty, "passphrase").unwrap();
        assert_eq!(open(&sealed, "passphrase").unwrap(), empty);
    }

    #[test]
    fn base64_round_trips_every_length() {
        for len in 0..32 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let encoded = encode_base64(&bytes);
            assert_eq!(encoded.len() % 4, 0, "encoding must be padded: {encoded}");
            assert_eq!(decode_base64(&encoded).unwrap(), bytes, "failed at length {len}");
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors: the padding cases are where hand-rolled
        // encoders usually go wrong.
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(decode_base64("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
    }

    #[test]
    fn malformed_base64_is_an_error() {
        assert!(decode_base64("Zg=").is_err(), "length must be a multiple of 4");
        assert!(decode_base64("Zm9v!!!!").is_err(), "invalid characters");
        assert!(decode_base64("Z===").is_err(), "too much padding");
    }

    /// A file from a newer turnout must be refused rather than half-imported.
    #[test]
    fn a_future_format_is_refused() {
        let mut snapshot = Snapshot::new(Vec::new(), Vec::new(), Vec::new(), Vec::new());
        assert!(snapshot.check_version().is_ok());
        snapshot.version = FORMAT_VERSION + 1;
        let err = snapshot.check_version().unwrap_err().to_string();
        assert!(err.contains("self-update"), "the error must say how to move forward: {err}");
    }
}
