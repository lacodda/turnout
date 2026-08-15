//! The SSH transport, on russh.
//!
//! Replaces libssh2 (the `ssh2` crate), which was built against the system
//! crypto backend and could not negotiate curve25519 - so it failed to connect
//! to any current, strictly-configured OpenSSH at all, and could never use an
//! ed25519 key. russh is pure Rust and speaks the modern algorithms natively.
//!
//! russh is async; the rest of turnout's remote layer is synchronous, and the
//! commands that drive it (`deploy`, `backup`, `restore`) read top to bottom
//! without a runtime in sight. Rather than colour that whole call path async,
//! this module keeps a runtime *inside* the session and exposes blocking
//! methods, the same shape the `ssh2::Session` it replaces had. The gateway
//! already drives its own runtime this way (`gateway.rs`), so the pattern is
//! not new to the codebase.

use std::cell::OnceCell;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client::{self, Handle, Msg};
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, load_secret_key};
use russh::{Channel, ChannelMsg};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::runtime::Runtime;

use crate::model::{Auth, Credential, Server};
use crate::secrets;

/// Accepts whatever host key the server presents.
///
/// turnout does not pin host keys (it never has - libssh2 was used without a
/// known-hosts check too), so the honest thing is to say so in one place rather
/// than pretend otherwise. Host-key trust is a candidate for a later stage; for
/// now the transport's job is to connect where the old one could not.
struct AcceptAnyHostKey;

impl client::Handler for AcceptAnyHostKey {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, _key: &PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// A live SSH connection, plus the runtime that drives it.
///
/// The runtime is owned so that every blocking method below has somewhere to
/// `block_on`; dropping the session drops the runtime and closes the
/// connection.
pub struct Session {
    /// One SFTP subsystem for the whole session, opened on first use. A
    /// file-by-file deploy uploads every file in the tree; opening the
    /// subsystem per file would pay a channel-open plus subsystem handshake
    /// for each one.
    sftp: OnceCell<SftpSession>,
    runtime: Runtime,
    handle: Handle<AcceptAnyHostKey>,
}

impl Session {
    /// Open a session to `server` as `credential`.
    ///
    /// Auth order mirrors the old behaviour: an explicit key first, otherwise a
    /// stored password. (Agent support rode on libssh2's `userauth_agent` and
    /// is not carried over here - it can return in the key-setup stage that
    /// needs it.)
    pub fn connect(server: &Server, credential: &Credential) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the SSH runtime")?;
        let host = server.ssh_host();
        let port = server.port;
        let handle = runtime.block_on(authenticate(&host, port, credential))?;
        Ok(Self {
            sftp: OnceCell::new(),
            runtime,
            handle,
        })
    }

    /// The session's SFTP subsystem, opened on first use and shared after.
    fn sftp(&self) -> Result<&SftpSession> {
        if self.sftp.get().is_none() {
            let opened = self.runtime.block_on(open_sftp(&self.handle))?;
            // The cell was just seen empty and Session is not shared across
            // threads, so this set cannot lose a race.
            let _ = self.sftp.set(opened);
        }
        Ok(self.sftp.get().expect("the SFTP cell was just filled"))
    }

    /// Run a remote command, returning its stdout. A non-zero exit becomes an
    /// error carrying the remote stderr.
    pub fn exec(&self, command: &str) -> Result<String> {
        self.runtime.block_on(exec(&self.handle, command))
    }

    /// Upload one file over SFTP, reporting each chunk as it leaves.
    ///
    /// The callback is what keeps the progress bar honest: bytes are credited
    /// as they are written, in bounded pieces.
    pub fn upload(&self, local: &Path, remote: &str, on_chunk: impl FnMut(u64)) -> Result<u64> {
        let sftp = self.sftp()?;
        self.runtime.block_on(upload(sftp, local, remote, on_chunk))
    }

    /// Upload an in-memory buffer over SFTP, reporting each chunk. The archive
    /// route builds its `tar.gz` in memory, so it never touches local disk.
    pub fn upload_bytes(&self, bytes: &[u8], remote: &str, on_chunk: impl FnMut(u64)) -> Result<u64> {
        let sftp = self.sftp()?;
        self.runtime.block_on(upload_bytes(sftp, bytes, remote, on_chunk))
    }

    /// Create a remote directory, tolerating only "already exists".
    ///
    /// SFTP has no `-p`, so an existing directory answers with an error and
    /// has to be told apart from a real refusal: a permission problem
    /// swallowed here would resurface as a baffling failure on the first
    /// upload into the missing directory.
    pub fn mkdir(&self, remote: &str) -> Result<()> {
        let sftp = self.sftp()?;
        self.runtime.block_on(async {
            if let Err(err) = sftp.create_dir(remote).await {
                match sftp.metadata(remote).await {
                    Ok(existing) if existing.is_dir() => {}
                    _ => return Err(err).with_context(|| format!("cannot create the directory {remote} on the server")),
                }
            }
            Ok(())
        })
    }
}

/// Connect and authenticate, returning the live handle.
async fn authenticate(host: &str, port: u16, credential: &Credential) -> Result<Handle<AcceptAnyHostKey>> {
    let config = Arc::new(client::Config::default());
    let mut handle = client::connect(config, (host, port), AcceptAnyHostKey)
        .await
        .with_context(|| format!("cannot reach {host}:{port}"))?;

    let ok = match credential.auth {
        Auth::Key => {
            let path = credential.key.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "credential '{0}' authenticates by key but has no key file - set one with `turnout credential edit {0} --key PATH`",
                    credential.name
                )
            })?;
            // A passphrase-protected key needs its passphrase; an unprotected
            // one does not, and asking for one that was never stored is not an
            // error. The secret stored under the credential's name is that
            // passphrase when the key needs one.
            let passphrase = secrets::get(&credential.name).ok();
            let key = load_secret_key(path, passphrase.as_deref()).with_context(|| format!("cannot read the key file {path}"))?;
            // None lets russh pick the signature algorithm; only RSA needs an
            // explicit SHA-2 choice, and ed25519 - the common case - ignores it.
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            handle
                .authenticate_publickey(&credential.user, key)
                .await
                .context("key authentication failed")?
        }
        Auth::Password => {
            let password =
                secrets::get(&credential.name).map_err(|_| anyhow::anyhow!("no password stored - save one with `turnout pass set {}`", credential.name))?;
            handle
                .authenticate_password(&credential.user, password)
                .await
                .context("password authentication failed")?
        }
    };

    if !ok.success() {
        bail!("SSH authentication failed for '{}' - the server rejected the credential", credential.user);
    }
    Ok(handle)
}

/// Run a command over a fresh channel and collect stdout, stderr and the exit
/// status.
async fn exec(handle: &Handle<AcceptAnyHostKey>, command: &str) -> Result<String> {
    let mut channel: Channel<Msg> = handle
        .channel_open_session()
        .await
        .with_context(|| format!("cannot open a channel for '{command}'"))?;
    channel
        .exec(true, command)
        .await
        .with_context(|| format!("cannot run remote command '{command}'"))?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut code = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }

    // A channel that closes without ever reporting an exit status is a dropped
    // connection, not a success; defaulting to zero here would let a deploy
    // severed mid-command report a clean finish.
    let Some(code) = code else {
        let stderr = String::from_utf8_lossy(&stderr);
        let detail = if stderr.trim().is_empty() {
            String::new()
        } else {
            format!(": {}", stderr.trim())
        };
        bail!("remote command '{command}' ended without an exit status - the connection likely dropped{detail}");
    };
    if code != 0 {
        bail!("remote command '{command}' exited with {code}: {}", String::from_utf8_lossy(&stderr).trim());
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Open an SFTP subsystem over a fresh channel.
async fn open_sftp(handle: &Handle<AcceptAnyHostKey>) -> Result<SftpSession> {
    let channel = handle.channel_open_session().await.context("cannot open an SFTP channel")?;
    channel.request_subsystem(true, "sftp").await.context("cannot start the SFTP subsystem")?;
    SftpSession::new(channel.into_stream()).await.context("cannot open SFTP")
}

/// How much goes over the wire between two progress callbacks. Small enough
/// that the bar moves several times a second on a home uplink, large enough
/// that the accounting is noise next to the SFTP round trips.
const CHUNK: usize = 64 * 1024;

/// Stream a local file to `remote` in bounded chunks.
async fn upload(sftp: &SftpSession, local: &Path, remote: &str, on_chunk: impl FnMut(u64)) -> Result<u64> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(local).await.with_context(|| format!("cannot open {}", local.display()))?;
    let mut remote_file = open_remote(sftp, remote).await?;
    let mut buffer = vec![0u8; CHUNK];
    let mut on_chunk = on_chunk;
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).await.with_context(|| format!("cannot read {}", local.display()))?;
        if read == 0 {
            break;
        }
        write_chunk(&mut remote_file, &buffer[..read], remote).await?;
        on_chunk(read as u64);
        total += read as u64;
    }
    finish(&mut remote_file, remote).await?;
    Ok(total)
}

/// Stream an in-memory buffer to `remote` in the same bounded chunks.
async fn upload_bytes(sftp: &SftpSession, bytes: &[u8], remote: &str, mut on_chunk: impl FnMut(u64)) -> Result<u64> {
    let mut remote_file = open_remote(sftp, remote).await?;
    for chunk in bytes.chunks(CHUNK) {
        write_chunk(&mut remote_file, chunk, remote).await?;
        on_chunk(chunk.len() as u64);
    }
    finish(&mut remote_file, remote).await?;
    Ok(bytes.len() as u64)
}

async fn open_remote(sftp: &SftpSession, remote: &str) -> Result<russh_sftp::client::fs::File> {
    sftp.open_with_flags(remote, OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE)
        .await
        .with_context(|| format!("cannot create {remote} on the server"))
}

async fn write_chunk(file: &mut russh_sftp::client::fs::File, chunk: &[u8], remote: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    file.write_all(chunk).await.with_context(|| format!("cannot upload to {remote}"))
}

async fn finish(file: &mut russh_sftp::client::fs::File, remote: &str) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    file.flush().await.with_context(|| format!("cannot finish uploading {remote}"))
}
