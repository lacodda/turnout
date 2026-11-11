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
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client::{self, Handle, Msg};
use russh::keys::{Algorithm, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate, load_secret_key};
use russh::{Channel, ChannelMsg};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::runtime::Runtime;

use crate::agent;
use crate::known_hosts::{self, Verdict};
use crate::model::{Auth, Credential, Server};
use crate::secrets;

/// Checks the host key a server presents against `known_hosts`, and learns
/// the key of a host met for the first time (see [`crate::known_hosts`]).
///
/// The check runs inside the key exchange, before a password or a signature
/// leaves this machine: a server that is not the one recorded gets nothing.
struct HostKeyCheck {
    file: PathBuf,
    host: String,
    port: u16,
    /// The server's name in the catalog, for the command a refusal suggests.
    server: Option<String>,
}

impl client::Handler for HostKeyCheck {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, presented: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = presented_key(presented);
        match known_hosts::check(&self.file, &self.host, self.port, &key)? {
            Verdict::Known | Verdict::Learned => Ok(true),
            Verdict::Changed { line, known } => {
                let trust = match &self.server {
                    Some(name) => format!("`turnout server trust {name}`"),
                    None => "`turnout server trust NAME`".to_string(),
                };
                bail!(
                    "{REFUSED} {host}:{port}: its host key has changed\n  \
                     it presents {presented}\n  \
                     {file} holds {known} for it (line {line})\n\
                     Someone may be intercepting the connection - or the server was reinstalled and has a new key.\n\
                     Once you know it is the server: {trust} shows the new key and asks to trust it.",
                    host = self.host,
                    port = self.port,
                    presented = known_hosts::describe(&key),
                    file = self.file.display(),
                )
            }
            Verdict::Revoked { line } => bail!(
                "{REFUSED} {}:{}: its host key {} is marked @revoked in {} (line {line})",
                self.host,
                self.port,
                known_hosts::describe(&key),
                self.file.display()
            ),
        }
    }
}

/// How a refusal over the host key begins; [`authenticate`] tells it from a
/// network failure by it.
const REFUSED: &str = "refusing to connect to";

/// Whether a failed connection was the host key check refusing the server -
/// which no password or key can mend, and `turnout server trust` can.
pub fn is_host_key_refusal(err: &anyhow::Error) -> bool {
    err.to_string().starts_with(REFUSED)
}

/// The key a server presented: a host certificate is checked by the key it
/// certifies - turnout trusts keys, not certificate authorities.
fn presented_key(presented: &PublicKeyOrCertificate) -> PublicKey {
    match presented {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        PublicKeyOrCertificate::Certificate(certificate) => PublicKey::from(certificate.public_key().clone()),
    }
}

/// The client configuration for `host:port`: the host key algorithms
/// `known_hosts` already holds for it asked for first. A server offers
/// several host keys and presents the one the client prefers; without this, a
/// server known by its RSA key would present a new ED25519 one and be refused
/// as changed.
fn client_config(file: &Path, host: &str, port: u16) -> client::Config {
    let known = known_hosts::algorithms(file, host, port);
    let mut keys = russh::Preferred::default().key.into_owned();
    // Stable: within each side the default order stands.
    let rsa = |a: &Algorithm| matches!(a, Algorithm::Rsa { .. });
    keys.sort_by_key(|algorithm| !known.iter().any(|k| k == algorithm || (rsa(k) && rsa(algorithm))));
    client::Config {
        preferred: russh::Preferred {
            key: std::borrow::Cow::Owned(keys),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The host key `host:port` presents, without signing in: what `turnout
/// server trust` shows before it is trusted. Asked for with the preferences a
/// session uses, so it is the key a session would meet.
pub fn host_key(host: &str, port: u16) -> Result<PublicKey> {
    struct Capture(Arc<std::sync::Mutex<Option<PublicKey>>>);
    impl client::Handler for Capture {
        type Error = anyhow::Error;
        async fn check_server_key(&mut self, presented: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
            *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(presented_key(presented));
            Ok(true)
        }
    }
    let file = known_hosts::file()?;
    let seen = Arc::new(std::sync::Mutex::new(None));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the SSH runtime")?;
    runtime.block_on(async {
        let handle = client::connect(Arc::new(client_config(&file, host, port)), (host, port), Capture(seen.clone()))
            .await
            .with_context(|| format!("cannot reach {host}:{port}"))?;
        let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
        anyhow::Ok(())
    })?;
    let key = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    key.ok_or_else(|| anyhow::anyhow!("{host}:{port} presented no host key"))
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
    handle: Handle<HostKeyCheck>,
}

impl Session {
    /// Open a session to `server` as `credential`.
    ///
    /// The credential's kind is the one way in: its key file, its stored
    /// password, or the keys of the SSH agent ([`crate::agent`]). Nothing falls
    /// back from one to another - a refused key is not followed by the
    /// password, which after `key setup` is still stored but no longer meant
    /// to be used.
    ///
    /// The credential is resolved to [`AuthMaterial`] before dialing: a missing
    /// key file or an unstored password fails fast, without a network round
    /// trip that could not have succeeded anyway.
    pub fn connect(server: &Server, credential: &Credential) -> Result<Self> {
        let material = auth_material(credential)?;
        Self::open_as(Some(&server.name), &server.ssh_host(), server.port, &credential.user, material)
    }

    /// Open a session authenticating with one named key file, and nothing else.
    ///
    /// [`Session::connect`] goes through the credential, which is exactly what
    /// the key-setup check must not do: it runs *before* the credential is
    /// switched over, so going through it would sign in with the password and
    /// prove nothing about the key. Offering only the key is what makes a
    /// success mean the server accepted it.
    pub fn open_with_key(server: &Server, user: &str, key_path: &str, passphrase: Option<&str>) -> Result<Self> {
        Self::open_as(Some(&server.name), &server.ssh_host(), server.port, user, key_material(key_path, passphrase)?)
    }

    /// Open a session with already-resolved auth material, to an address the
    /// catalog has no name for.
    #[cfg(test)]
    fn open(host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Self> {
        Self::open_as(None, host, port, user, material)
    }

    /// Open a session to the server the catalog calls `server` - the name a
    /// refused host key gives in its advice.
    fn open_as(server: Option<&str>, host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Self> {
        Self::open_checked(known_hosts::file()?, server, host, port, user, material)
    }

    /// [`Session::open_as`] with the host key checked against `file`.
    fn open_checked(file: PathBuf, server: Option<&str>, host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Self> {
        let check = HostKeyCheck {
            file,
            host: host.to_string(),
            port,
            server: server.map(str::to_string),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the SSH runtime")?;
        let handle = runtime.block_on(authenticate(check, user, material))?;
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
        self.exec_bytes(command).map(|stdout| String::from_utf8_lossy(&stdout).into_owned())
    }

    /// [`Session::exec`] with stdout as it came, byte for byte: what reads a
    /// remote file in pieces has to count bytes, and a character split across
    /// two pieces would be mangled by decoding either half alone.
    pub fn exec_bytes(&self, command: &str) -> Result<Vec<u8>> {
        self.runtime.block_on(exec(&self.handle, command))
    }

    /// Run a remote command with its output streamed to this terminal as it
    /// arrives, and its exit code returned rather than judged: `turnout exec`
    /// is transparent, the way `turnout run` is for local commands.
    pub fn run(&self, command: &str) -> Result<u32> {
        let mut stdout = std::io::stdout();
        let mut stderr = std::io::stderr();
        self.runtime.block_on(run(&self.handle, command, &mut stdout, &mut stderr))
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
        self.runtime.block_on(mkdir(sftp, remote))
    }

    /// [`Session::mkdir`] for `remote` and every parent it is missing, the
    /// way `mkdir -p` does - which SFTP has no word for.
    ///
    /// The paths are split on both separators: a Windows server's are
    /// written with `\`. A drive (`C:`) and the root are never created.
    pub fn mkdir_all(&self, remote: &str) -> Result<()> {
        let sftp = self.sftp()?;
        let remote = remote.trim_end_matches(['/', '\\']);
        let parents = remote.match_indices(['/', '\\']).map(|(at, _)| &remote[..at]);
        self.runtime.block_on(async {
            for dir in parents.chain(std::iter::once(remote)) {
                if dir.is_empty() || dir.ends_with(':') {
                    continue;
                }
                mkdir(sftp, dir).await?;
            }
            Ok(())
        })
    }
}

/// How often a tunnel's connection asks the server whether it is still there
/// when nothing else crosses it; [`KEEPALIVE_MISSES`] unanswered asks in a
/// row and the connection is taken for lost.
///
/// A connection a laptop's sleep or a rebooted router cut leaves no word
/// behind: without asking, a tunnel would hand its next client a connection
/// that is dead and only find out when that client gave up.
const KEEPALIVE: std::time::Duration = std::time::Duration::from_secs(15);
const KEEPALIVE_MISSES: usize = 3;

/// An SSH connection that carries TCP: what a tunnel forwards its local
/// connections over (see [`crate::tunnel`]).
///
/// Async, unlike [`Session`]: a tunnel accepts connections and copies their
/// bytes on a runtime of its own, and the connection has to live on the same
/// one. The way in is the credential's, exactly as for a [`Session`].
pub struct Forwarder {
    handle: Handle<HostKeyCheck>,
}

/// Why a forwarded connection could not be opened.
#[derive(Debug)]
pub enum OpenFailure {
    /// The server tried and could not connect to the address: nothing
    /// listens there, or the server cannot resolve the name.
    Unreachable,
    /// sshd forwards nothing for this account (`AllowTcpForwarding`).
    Prohibited,
    /// Some other refusal of the server's, in its words.
    Refused(String),
    /// The SSH connection itself is gone.
    Lost(String),
}

impl Forwarder {
    /// Connect to `server` as `credential`, for forwarding.
    ///
    /// The credential is resolved on every call - its key file read, its
    /// password taken from the keyring - so a tunnel that reconnects after a
    /// `pass set` signs in with the new password.
    pub async fn connect(server: &Server, credential: &Credential) -> Result<Self> {
        // Off the runtime: a keyring client blocks, and on Linux it may run
        // a runtime of its own, which cannot be started from inside this one.
        let owned = credential.clone();
        let material = tokio::task::spawn_blocking(move || auth_material(&owned))
            .await
            .context("cannot read the credential")??;
        Self::connect_checked(
            known_hosts::file()?,
            Some(&server.name),
            &server.ssh_host(),
            server.port,
            &credential.user,
            material,
        )
        .await
    }

    async fn connect_checked(file: PathBuf, server: Option<&str>, host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Self> {
        let mut config = client_config(&file, host, port);
        config.keepalive_interval = Some(KEEPALIVE);
        config.keepalive_max = KEEPALIVE_MISSES;
        // A database's small messages go out as they are written, not when
        // Nagle's algorithm thinks there are enough of them.
        config.nodelay = true;
        let check = HostKeyCheck {
            file,
            host: host.to_string(),
            port,
            server: server.map(str::to_string),
        };
        Ok(Self {
            handle: authenticate_with(check, user, material, config).await?,
        })
    }

    /// Whether the connection has ended - closed by the server, or given up
    /// on after the keepalives went unanswered.
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Ask the server to connect to `host:port` and carry that connection;
    /// `origin` is the local client it is for, which sshd logs.
    pub async fn open(&self, host: &str, port: u16, origin: std::net::SocketAddr) -> Result<russh::ChannelStream<Msg>, OpenFailure> {
        use russh::ChannelOpenFailure as Failure;
        match self
            .handle
            .channel_open_direct_tcpip(host, u32::from(port), origin.ip().to_string(), u32::from(origin.port()))
            .await
        {
            Ok(channel) => Ok(channel.into_stream()),
            Err(russh::Error::ChannelOpenFailure(Failure::ConnectFailed)) => Err(OpenFailure::Unreachable),
            Err(russh::Error::ChannelOpenFailure(Failure::AdministrativelyProhibited)) => Err(OpenFailure::Prohibited),
            Err(russh::Error::ChannelOpenFailure(Failure::Other { reason, .. })) if !reason.trim().is_empty() => Err(OpenFailure::Refused(reason)),
            Err(russh::Error::ChannelOpenFailure(other)) => Err(OpenFailure::Refused(format!("{other:?}"))),
            Err(err) => Err(OpenFailure::Lost(err.to_string())),
        }
    }

    /// End the connection, politely.
    pub async fn close(&self) {
        let _ = self.handle.disconnect(russh::Disconnect::ByApplication, "", "").await;
    }
}

/// Create one remote directory, tolerating only "already exists".
async fn mkdir(sftp: &SftpSession, remote: &str) -> Result<()> {
    if let Err(err) = sftp.create_dir(remote).await {
        match sftp.metadata(remote).await {
            Ok(existing) if existing.is_dir() => {}
            _ => return Err(err).with_context(|| format!("cannot create the directory {remote} on the server")),
        }
    }
    Ok(())
}

/// A credential resolved to what the wire can use: the key loaded, the
/// password fetched. Resolution happens before dialing (see
/// [`Session::connect`]) and touches the keyring; everything after it is pure
/// network, which is what makes [`authenticate`] testable against a local
/// server.
enum AuthMaterial {
    /// The key alone: which hash an RSA key signs with is the server's to
    /// say, and it can only be asked once the connection is up.
    Key(Arc<PrivateKey>),
    Password(String),
    /// Nothing to carry: the keys stay in the agent, which is asked for them
    /// at the moment of authentication.
    Agent,
}

/// Deliberately manual: a derived impl would print the password.
impl std::fmt::Debug for AuthMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AuthMaterial::Key(_) => "AuthMaterial::Key",
            AuthMaterial::Password(_) => "AuthMaterial::Password(<redacted>)",
            AuthMaterial::Agent => "AuthMaterial::Agent",
        })
    }
}

/// Resolve `credential` to its auth material.
fn auth_material(credential: &Credential) -> Result<AuthMaterial> {
    match credential.auth {
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
            key_material(path, passphrase.as_deref())
        }
        Auth::Password => {
            let password =
                secrets::get(&credential.name).map_err(|_| anyhow::anyhow!("no password stored - save one with `turnout pass set {}`", credential.name))?;
            Ok(AuthMaterial::Password(password))
        }
        // Nothing to resolve here: the agent is reached inside
        // `authenticate`, on the runtime, because connecting to it is async.
        Auth::Agent => Ok(AuthMaterial::Agent),
    }
}

/// Load a private key file into auth material.
fn key_material(path: &str, passphrase: Option<&str>) -> Result<AuthMaterial> {
    let key = load_secret_key(path, passphrase).with_context(|| format!("cannot read the key file {path}"))?;
    Ok(AuthMaterial::Key(Arc::new(key)))
}

/// The hash an RSA key signs with on this connection; `None` for every other
/// key, which has one signature algorithm and ignores the question.
///
/// For RSA, `None` means the legacy `ssh-rsa` - SHA-1 - which OpenSSH 8.8 and
/// later refuse. Up to v0.26 every RSA key signed that way (the comment here
/// claimed `None` let russh choose), so an RSA key never got into a current
/// sshd. The server names the RSA hashes it takes in `server-sig-algs`, and the
/// best of those is used. A server that names nothing predates the extension
/// (RFC 8308) and with it SHA-2 signatures: for that one SHA-1 is right, and
/// it is what `ssh` itself falls back to.
async fn rsa_hash(handle: &Handle<HostKeyCheck>, algorithm: Algorithm, best: &mut Option<Option<HashAlg>>) -> Result<Option<HashAlg>> {
    if !algorithm.is_rsa() {
        return Ok(None);
    }
    if best.is_none() {
        let answer = handle
            .best_supported_rsa_hash()
            .await
            .context("cannot learn which RSA signatures the server accepts")?;
        *best = Some(answer.flatten());
    }
    Ok(best.flatten())
}

/// Connect and authenticate, returning the live handle.
async fn authenticate(check: HostKeyCheck, user: &str, material: AuthMaterial) -> Result<Handle<HostKeyCheck>> {
    let config = client_config(&check.file, &check.host, check.port);
    authenticate_with(check, user, material, config).await
}

/// [`authenticate`] with the client configuration given - a tunnel's asks
/// the server whether it is still there.
async fn authenticate_with(check: HostKeyCheck, user: &str, material: AuthMaterial, config: client::Config) -> Result<Handle<HostKeyCheck>> {
    let (host, port) = (check.host.clone(), check.port);
    let config = Arc::new(config);
    let mut handle = match client::connect(config, (host.as_str(), port), check).await {
        Ok(handle) => handle,
        // A refused host key is the whole message: "cannot reach" in front
        // of it would send the reader looking at the network.
        Err(err) if is_host_key_refusal(&err) => return Err(err),
        Err(err) => return Err(err).with_context(|| format!("cannot reach {host}:{port}")),
    };

    let ok = match material {
        AuthMaterial::Key(key) => {
            let hash = rsa_hash(&handle, key.algorithm(), &mut None).await?;
            handle
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
                .await
                .context("key authentication failed")?
        }
        AuthMaterial::Password(password) => handle.authenticate_password(user, password).await.context("password authentication failed")?,
        AuthMaterial::Agent => return authenticate_with_agent(handle, user).await,
    };

    if !ok.success() {
        // SSH gives no reason, and the two causes look the same from here: a
        // wrong secret, or an account sshd will not let in whatever it
        // offers. The second is the default on Windows Server 2025.
        bail!(
            "SSH authentication failed for '{user}' - the server rejected the credential: a wrong password or key, \
             or an account sshd does not let in (AllowUsers/AllowGroups in sshd_config - Windows Server 2025 \
             admits only administrators and the 'OpenSSH Users' group)"
        );
    }
    Ok(handle)
}

/// Authenticate by asking the agent to sign, offering its keys in turn.
///
/// The agent may hold many keys and the server accepts at most one of them, so
/// a refusal of the first is not a failure - only running out of keys is. This
/// is what `ssh` itself does, and why the loop exists rather than a single
/// attempt on the first identity.
async fn authenticate_with_agent(mut handle: Handle<HostKeyCheck>, user: &str) -> Result<Handle<HostKeyCheck>> {
    let mut agent = agent::connect().await?;
    let identities = agent::identities(&mut agent).await?;
    if identities.is_empty() {
        return Err(agent::no_identities());
    }

    let mut offered = Vec::new();
    // Asked of the server at most once, and only if the agent holds an RSA key.
    let mut best = None;
    for identity in &identities {
        let public = identity.public_key().into_owned();
        offered.push(agent::describe(identity));
        let hash = rsa_hash(&handle, public.algorithm(), &mut best).await?;
        // Errors here are the agent failing to sign - a locked agent, a key
        // pulled out mid-run - not the server saying no. Either way the next
        // key is worth trying; what matters is whether any of them got in.
        if let Ok(result) = handle.authenticate_publickey_with(user, public, hash, &mut agent).await
            && result.success()
        {
            return Ok(handle);
        }
    }

    // Naming the keys that were offered is the difference between "fix your
    // server" and "add the right key here": the agent is running and holds
    // keys, they are simply not the ones this server authorizes.
    bail!(
        "SSH authentication failed for '{user}' - the agent offered {} key{}, none accepted by the server: {}",
        offered.len(),
        if offered.len() == 1 { "" } else { "s" },
        offered.join(", ")
    );
}

/// Run a command over a fresh channel and collect stdout, stderr and the exit
/// status.
async fn exec(handle: &Handle<HostKeyCheck>, command: &str) -> Result<Vec<u8>> {
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
    Ok(stdout)
}

/// Run a command over a fresh channel, copying stdout and stderr through as
/// they arrive, and return the exit status.
///
/// A channel that closes without an exit status is a dropped connection,
/// exactly as in [`exec`]; nothing here reads a missing status as success.
async fn run(handle: &Handle<HostKeyCheck>, command: &str, stdout: &mut impl Write, stderr: &mut impl Write) -> Result<u32> {
    let mut channel: Channel<Msg> = handle
        .channel_open_session()
        .await
        .with_context(|| format!("cannot open a channel for '{command}'"))?;
    channel
        .exec(true, command)
        .await
        .with_context(|| format!("cannot run remote command '{command}'"))?;

    let mut code = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                stdout.write_all(&data).context("cannot write the remote output")?;
                stdout.flush().ok();
            }
            ChannelMsg::ExtendedData { data, ext: 1 } => {
                stderr.write_all(&data).context("cannot write the remote output")?;
                stderr.flush().ok();
            }
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
            _ => {}
        }
    }
    code.ok_or_else(|| anyhow::anyhow!("remote command '{command}' ended without an exit status - the connection likely dropped"))
}

/// Open an SFTP subsystem over a fresh channel.
async fn open_sftp(handle: &Handle<HostKeyCheck>) -> Result<SftpSession> {
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

#[cfg(test)]
pub(crate) mod tests {
    //! The transport is exercised against a real SSH server: russh's server
    //! side, in-process, on a loopback port. Nothing is mocked below the
    //! protocol - auth, exec and SFTP all cross an actual TCP connection, so
    //! these tests fail for the same reasons a live stand would.
    //!
    //! The server is shared with the doctor's tests: [`Stand::spawn_shell`]
    //! runs what it is sent through this machine's own shell, which is how
    //! the commands the doctor builds for a server get run for real.

    use std::collections::HashMap;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};

    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::server::{self, Auth as AuthAnswer, ChannelOpenHandle, Msg as ServerMsg, Session as ServerSession};
    use russh::{Channel as ServerChannel, ChannelId};
    use russh_sftp::protocol::{Attrs, FileAttributes, Handle as SftpHandle, OpenFlags, Status, StatusCode, Version};

    use super::*;

    const USER: &str = "deploy";
    const PASSWORD: &str = "sesame";

    /// A deterministic ed25519 key: no OS randomness, no key material in the
    /// repository - the seed is just bytes.
    fn ed25519_key(seed: u8, comment: &str) -> russh::keys::PrivateKey {
        let pair = Ed25519Keypair::from_seed(&[seed; 32]);
        russh::keys::PrivateKey::new(KeypairData::Ed25519(pair), comment).expect("an ed25519 key from a fixed seed")
    }

    /// A deterministic RSA key, for the same reason: the primes come from a
    /// seeded generator, not from the OS and not from a key in the tree.
    fn rsa_key(seed: u8, comment: &str) -> russh::keys::PrivateKey {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::from_seed([seed; 32]);
        let pair = russh::keys::ssh_key::private::RsaKeypair::random(&mut rng, 2048).expect("an RSA key from a fixed seed");
        russh::keys::PrivateKey::new(KeypairData::Rsa(pair), comment).expect("an RSA key")
    }

    /// The catalog's view of a stand on this machine.
    fn loopback(port: u16) -> Server {
        Server {
            name: "stand".into(),
            label: None,
            url: "http://127.0.0.1".into(),
            host: Some("127.0.0.1".into()),
            port,
            accept_invalid_certs: false,
            shell: None,
            credential: None,
        }
    }

    /// One running test server: its port, the directory SFTP writes into, and
    /// a counter of how many times the SFTP subsystem was opened.
    pub(crate) struct Stand {
        port: u16,
        root: tempfile::TempDir,
        subsystem_opens: Arc<AtomicUsize>,
        /// How many bytes of a file SFTP takes before it refuses to write;
        /// unlimited until a test says otherwise.
        write_limit: Arc<AtomicU64>,
        /// Whether the stand forwards TCP (`direct-tcpip`), as sshd does
        /// unless `AllowTcpForwarding no` says otherwise.
        forwarding: Arc<AtomicBool>,
    }

    impl Stand {
        pub(crate) fn spawn() -> Self {
            Self::spawn_with(true)
        }

        /// A stand whose commands run in this machine's shell - `cmd /C` on
        /// Windows, `sh -c` elsewhere, the shells sshd hands a command to on
        /// either - with their real output and exit codes.
        pub(crate) fn spawn_shell() -> Self {
            Self::spawn_full(true, None, true)
        }

        /// A stand that refuses passwords, the way a hardened sshd does.
        ///
        /// This is what makes a key test prove anything: if the server would
        /// take the password too, a "the key signs in" result could have come
        /// from either method.
        fn spawn_key_only() -> Self {
            Self::spawn_with(false)
        }

        /// A key-only stand that authorizes exactly one public key.
        ///
        /// The plain key-only stand takes any key the right user signs with,
        /// which is enough to prove "a key got in" but not "*this* key did".
        /// Agent auth offers several keys in turn, so proving the loop picks
        /// the accepted one needs a server that refuses the others.
        ///
        /// Unix-only alongside the agent tests that call it: without the cfg
        /// it is dead code on Windows, which `clippy -D warnings` refuses.
        #[cfg(unix)]
        fn spawn_authorizing(authorized: russh::keys::PublicKey) -> Self {
            Self::spawn_full(false, Some(authorized), false)
        }

        fn spawn_with(passwords_accepted: bool) -> Self {
            Self::spawn_full(passwords_accepted, None, false)
        }

        fn spawn_full(passwords_accepted: bool, authorized: Option<russh::keys::PublicKey>, real_shell: bool) -> Self {
            let root = tempfile::tempdir().expect("a scratch directory for the SFTP root");
            let subsystem_opens = Arc::new(AtomicUsize::new(0));
            let served_root = root.path().to_path_buf();
            let served_opens = subsystem_opens.clone();
            let write_limit = Arc::new(AtomicU64::new(u64::MAX));
            let served_limit = write_limit.clone();
            let forwarding = Arc::new(AtomicBool::new(true));
            let served_forwarding = forwarding.clone();
            let (report, learn) = mpsc::channel();
            // The server outlives every session the test opens; the thread is
            // reclaimed when the test process exits.
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("the test server runtime");
                runtime.block_on(async move {
                    let config = Arc::new(server::Config {
                        auth_rejection_time: std::time::Duration::ZERO,
                        auth_rejection_time_initial: Some(std::time::Duration::ZERO),
                        keys: vec![ed25519_key(7, "test host key")],
                        // What OpenSSH 8.8 and later announce: the SHA-2 RSA
                        // signatures, and no `ssh-rsa`. `server-sig-algs` is
                        // made from this list, so a client that reads it sees
                        // exactly what the handler below accepts.
                        preferred: russh::Preferred {
                            key: std::borrow::Cow::Owned(vec![
                                Algorithm::Ed25519,
                                Algorithm::Rsa { hash: Some(HashAlg::Sha512) },
                                Algorithm::Rsa { hash: Some(HashAlg::Sha256) },
                            ]),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind a loopback port");
                    report.send(socket.local_addr().expect("the bound address").port()).expect("report the port");
                    let mut factory = Factory {
                        root: served_root,
                        subsystem_opens: served_opens,
                        write_limit: served_limit,
                        forwarding: served_forwarding,
                        passwords_accepted,
                        authorized,
                        real_shell,
                    };
                    let _ = server::Server::run_on_socket(&mut factory, config, &socket).await;
                });
            });
            let port = learn.recv().expect("the server reports its port");
            Self {
                port,
                root,
                subsystem_opens,
                write_limit,
                forwarding,
            }
        }

        /// From now on, refuse to forward TCP - sshd with
        /// `AllowTcpForwarding no`.
        pub(crate) fn refuse_forwarding(&self) {
            self.forwarding.store(false, Ordering::SeqCst);
        }

        /// The stand's port on the loopback.
        pub(crate) fn port(&self) -> u16 {
            self.port
        }

        /// From now on, refuse an SFTP write that would take a file past
        /// `bytes` - the shape of a disk that fills up mid-upload.
        pub(crate) fn refuse_writes_past(&self, bytes: u64) {
            self.write_limit.store(bytes, Ordering::SeqCst);
        }

        /// The home directory of the account the stand signs in, for a stand
        /// that runs a real shell. Only the POSIX side keeps anything there.
        #[cfg(unix)]
        pub(crate) fn home(&self) -> &std::path::Path {
            self.root.path()
        }

        pub(crate) fn session(&self) -> Session {
            Session::open("127.0.0.1", self.port, USER, AuthMaterial::Password(PASSWORD.into())).expect("password sign-in on loopback")
        }
    }

    struct Factory {
        root: PathBuf,
        subsystem_opens: Arc<AtomicUsize>,
        write_limit: Arc<AtomicU64>,
        forwarding: Arc<AtomicBool>,
        passwords_accepted: bool,
        /// When set, the only public key this stand accepts.
        authorized: Option<russh::keys::PublicKey>,
        real_shell: bool,
    }

    impl server::Server for Factory {
        type Handler = TestHandler;

        fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> TestHandler {
            TestHandler {
                root: self.root.clone(),
                subsystem_opens: self.subsystem_opens.clone(),
                write_limit: self.write_limit.clone(),
                forwarding: self.forwarding.clone(),
                channels: HashMap::new(),
                passwords_accepted: self.passwords_accepted,
                authorized: self.authorized.clone(),
                real_shell: self.real_shell,
            }
        }
    }

    struct TestHandler {
        root: PathBuf,
        subsystem_opens: Arc<AtomicUsize>,
        write_limit: Arc<AtomicU64>,
        forwarding: Arc<AtomicBool>,
        channels: HashMap<ChannelId, ServerChannel<ServerMsg>>,
        passwords_accepted: bool,
        authorized: Option<russh::keys::PublicKey>,
        real_shell: bool,
    }

    fn rejected() -> AuthAnswer {
        AuthAnswer::Reject {
            proceed_with_methods: None,
            partial_success: false,
        }
    }

    impl server::Handler for TestHandler {
        type Error = anyhow::Error;

        async fn auth_password(&mut self, user: &str, password: &str) -> Result<AuthAnswer, Self::Error> {
            Ok(if self.passwords_accepted && user == USER && password == PASSWORD {
                AuthAnswer::Accept
            } else {
                rejected()
            })
        }

        async fn auth_publickey(&mut self, user: &str, key: server::VerifiedPublicKey<'_>) -> Result<AuthAnswer, Self::Error> {
            // russh verifies the signature; the handler only says whether the
            // user may sign in with this key at all.
            //
            // Compared by key *data*, not by the whole `PublicKey`: that type
            // carries a comment as well, and `PartialEq` includes it. An agent
            // returns its identities with an empty comment (the ADD_IDENTITY
            // frame encodes one), so comparing the structs would reject the
            // very key the agent had just been handed. sshd matches on the key
            // material for the same reason - a comment is a label, not part of
            // the identity.
            let key_allowed = match &self.authorized {
                Some(authorized) => authorized.key_data() == key.public_key.key_data(),
                None => true,
            };
            // A SHA-1 RSA signature is refused, as OpenSSH 8.8 and later
            // refuse it: an RSA key that signs in here proves the client
            // chose a hash the server accepts.
            let sha1 = *key.algorithm == Algorithm::Rsa { hash: None };
            Ok(if user == USER && key_allowed && !sha1 {
                AuthAnswer::Accept
            } else {
                rejected()
            })
        }

        async fn channel_open_session(
            &mut self,
            channel: ServerChannel<ServerMsg>,
            reply: ChannelOpenHandle,
            _session: &mut ServerSession,
        ) -> Result<(), Self::Error> {
            self.channels.insert(channel.id(), channel);
            reply.accept().await;
            Ok(())
        }

        /// A tiny command language instead of a shell: `echo TEXT` prints,
        /// `fail` exits 3 with stderr, `vanish` closes the channel without an
        /// exit status - the shape of a connection dropped mid-command.
        async fn exec_request(&mut self, channel: ChannelId, data: &[u8], session: &mut ServerSession) -> Result<(), Self::Error> {
            let command = String::from_utf8_lossy(data).into_owned();
            session.channel_success(channel)?;
            if self.real_shell {
                #[cfg(windows)]
                let mut shell = {
                    let mut shell = std::process::Command::new("cmd");
                    // `raw_arg`: cmd reads its command line itself, and the
                    // quoting std would add around it is not what sshd sends.
                    std::os::windows::process::CommandExt::raw_arg(&mut shell, format!("/C {command}"));
                    // System32 first, as sshd's own environment has it: a
                    // developer shell that puts Git's `usr/bin` ahead would
                    // hand `tar` to GNU tar, which reads `C:\...` as a host.
                    let system = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
                    let path = std::env::var("PATH").unwrap_or_default();
                    shell.env("PATH", format!("{system}\\System32;{path}"));
                    shell
                };
                #[cfg(not(windows))]
                let mut shell = {
                    let mut shell = std::process::Command::new("sh");
                    shell.args(["-c", &command]);
                    shell
                };
                // The stand's scratch directory is the account's home, so what a
                // command keeps under `~` stays out of the developer's own.
                let output = shell.env("HOME", &self.root).stdin(std::process::Stdio::null()).output()?;
                session.data(channel, output.stdout)?;
                if !output.stderr.is_empty() {
                    session.extended_data(channel, 1, output.stderr)?;
                }
                session.exit_status_request(channel, output.status.code().unwrap_or(1) as u32)?;
            } else if let Some(text) = command.strip_prefix("echo ") {
                session.data(channel, format!("{text}\n").into_bytes())?;
                session.exit_status_request(channel, 0)?;
            } else if command == "fail" {
                session.extended_data(channel, 1, &b"boom"[..])?;
                session.exit_status_request(channel, 3)?;
            } else if command != "vanish" {
                session.exit_status_request(channel, 0)?;
            }
            session.eof(channel)?;
            session.close(channel)?;
            Ok(())
        }

        /// Forwarding as sshd does it: connect to the address asked for, and
        /// carry that connection over the channel - or say why not.
        async fn channel_open_direct_tcpip(
            &mut self,
            channel: ServerChannel<ServerMsg>,
            host_to_connect: &str,
            port_to_connect: u32,
            _originator_address: &str,
            _originator_port: u32,
            reply: ChannelOpenHandle,
            _session: &mut ServerSession,
        ) -> Result<(), Self::Error> {
            if !self.forwarding.load(Ordering::SeqCst) {
                reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited).await;
                return Ok(());
            }
            let Ok(port) = u16::try_from(port_to_connect) else {
                reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
                return Ok(());
            };
            match tokio::net::TcpStream::connect((host_to_connect, port)).await {
                Ok(mut tcp) => {
                    reply.accept().await;
                    tokio::spawn(async move {
                        let mut stream = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                    });
                }
                Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
            }
            Ok(())
        }

        async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut ServerSession) -> Result<(), Self::Error> {
            if name != "sftp" {
                session.channel_failure(channel_id)?;
                return Ok(());
            }
            self.subsystem_opens.fetch_add(1, Ordering::SeqCst);
            let channel = self.channels.remove(&channel_id).expect("the subsystem channel was opened first");
            session.channel_success(channel_id)?;
            let handler = TestSftp {
                root: self.root.clone(),
                files: HashMap::new(),
                real_paths: self.real_shell,
                write_limit: self.write_limit.clone(),
            };
            tokio::spawn(async move {
                russh_sftp::server::run(channel.into_stream(), handler).await;
            });
            Ok(())
        }
    }

    /// An SFTP server over a scratch directory: just enough of the protocol
    /// for what the transport uses - open/write/close, mkdir, stat.
    ///
    /// A stand whose commands run in a real shell takes remote paths as they
    /// are, so a file SFTP writes is the file the shell then finds - a deploy
    /// uploads and unpacks through both.
    struct TestSftp {
        root: PathBuf,
        files: HashMap<String, std::fs::File>,
        real_paths: bool,
        write_limit: Arc<AtomicU64>,
    }

    impl TestSftp {
        fn resolve(&self, remote: &str) -> PathBuf {
            if self.real_paths {
                PathBuf::from(remote)
            } else {
                self.root.join(remote.trim_start_matches('/'))
            }
        }
    }

    fn done(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: "en-US".into(),
        }
    }

    impl russh_sftp::server::Handler for TestSftp {
        type Error = StatusCode;

        fn unimplemented(&self) -> Self::Error {
            StatusCode::OpUnsupported
        }

        async fn init(&mut self, _version: u32, _extensions: HashMap<String, String>) -> Result<Version, Self::Error> {
            Ok(Version::new())
        }

        async fn open(&mut self, id: u32, filename: String, _pflags: OpenFlags, _attrs: FileAttributes) -> Result<SftpHandle, Self::Error> {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(self.resolve(&filename))
                .map_err(|_| StatusCode::Failure)?;
            self.files.insert(filename.clone(), file);
            Ok(SftpHandle { id, handle: filename })
        }

        async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
            if offset + data.len() as u64 > self.write_limit.load(Ordering::SeqCst) {
                return Err(StatusCode::Failure);
            }
            let file = self.files.get_mut(&handle).ok_or(StatusCode::Failure)?;
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.write_all(&data))
                .map_err(|_| StatusCode::Failure)?;
            Ok(done(id))
        }

        async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
            self.files.remove(&handle);
            Ok(done(id))
        }

        async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
            std::fs::create_dir(self.resolve(&path)).map_err(|_| StatusCode::Failure)?;
            Ok(done(id))
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            let metadata = std::fs::metadata(self.resolve(&path)).map_err(|_| StatusCode::NoSuchFile)?;
            Ok(Attrs {
                id,
                attrs: FileAttributes::from(&metadata),
            })
        }
    }

    /// The first session to a host writes its key down, and the next one
    /// finds it there. A file of the test's own: the process-wide one is
    /// shared by every test running beside it.
    #[test]
    fn the_first_session_learns_the_host_key_and_the_next_finds_it() {
        let stand = Stand::spawn();
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("known_hosts");
        let key = ed25519_key(7, "test host key").public_key().clone();
        let open = || Session::open_checked(file.clone(), None, "127.0.0.1", stand.port, USER, AuthMaterial::Password(PASSWORD.into()));
        open().expect("a first session");
        let trusted = known_hosts::trusted(&file, "127.0.0.1", stand.port).unwrap();
        assert_eq!(trusted.len(), 1, "one line for the new host");
        assert_eq!(known_hosts::describe(&trusted[0].key), known_hosts::describe(&key));
        open().expect("a second session");
        assert_eq!(
            known_hosts::trusted(&file, "127.0.0.1", stand.port).unwrap().len(),
            1,
            "a known host is not written twice"
        );
    }

    /// A server presenting another key than the one on record is refused
    /// before the password is sent - the stand would accept it, so a session
    /// here would mean the check let it through.
    #[test]
    fn a_changed_host_key_is_refused_before_signing_in() {
        let stand = Stand::spawn();
        let scratch = tempfile::tempdir().unwrap();
        let file = scratch.path().join("known_hosts");
        let other = ed25519_key(8, "someone else").public_key().clone();
        known_hosts::replace(&file, "127.0.0.1", stand.port, &other).unwrap();
        let open = || {
            Session::open_checked(
                file.clone(),
                Some("stand"),
                "127.0.0.1",
                stand.port,
                USER,
                AuthMaterial::Password(PASSWORD.into()),
            )
        };
        let error = match open() {
            Ok(_) => panic!("a session to a server with another host key"),
            Err(err) => format!("{err:#}"),
        };
        assert!(
            error.starts_with(&format!("refusing to connect to 127.0.0.1:{}: its host key has changed", stand.port)),
            "{error}"
        );
        assert!(error.contains(&known_hosts::describe(&other)), "the key on record is named: {error}");
        assert!(error.contains("`turnout server trust stand`"), "the way out is named: {error}");
        assert!(!error.contains("cannot reach"), "a refusal is not a network failure: {error}");

        // Trusted again, it signs in.
        known_hosts::replace(&file, "127.0.0.1", stand.port, &host_key("127.0.0.1", stand.port).unwrap()).unwrap();
        open().expect("the trusted key signs in");
    }

    /// What `server trust` shows is the key a session meets.
    #[test]
    fn the_host_key_is_read_without_signing_in() {
        let stand = Stand::spawn_key_only();
        let key = host_key("127.0.0.1", stand.port).expect("the host key, with no credential at all");
        assert_eq!(known_hosts::describe(&key), known_hosts::describe(ed25519_key(7, "").public_key()));
    }

    #[test]
    fn exec_returns_stdout_on_a_zero_exit() {
        let stand = Stand::spawn();
        let session = stand.session();
        assert_eq!(session.exec("echo ready").expect("echo succeeds"), "ready\n");
    }

    /// `run` streams and reports; it does not judge. A failing command's
    /// stderr and code come back as they are, so `turnout exec` can be the
    /// remote command's transparent stand-in.
    #[test]
    fn run_streams_both_channels_and_returns_the_code() {
        let stand = Stand::spawn();
        let session = Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Password(PASSWORD.into())).expect("a session");
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = session
            .runtime
            .block_on(run(&session.handle, "echo hi there", &mut out, &mut err))
            .expect("echo runs");
        assert_eq!((code, out.as_slice(), err.as_slice()), (0, &b"hi there\n"[..], &b""[..]));
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = session
            .runtime
            .block_on(run(&session.handle, "fail", &mut out, &mut err))
            .expect("a failing command still returns");
        assert_eq!((code, out.as_slice(), err.as_slice()), (3, &b""[..], &b"boom"[..]));
        let dropped = session
            .runtime
            .block_on(run(&session.handle, "vanish", &mut Vec::new(), &mut Vec::new()))
            .unwrap_err()
            .to_string();
        assert!(dropped.contains("without an exit status"), "{dropped}");
    }

    #[test]
    fn a_nonzero_exit_becomes_an_error_carrying_stderr() {
        let stand = Stand::spawn();
        let session = stand.session();
        let error = session.exec("fail").expect_err("exit 3 is a failure").to_string();
        assert!(error.contains("exited with 3"), "{error}");
        assert!(error.contains("boom"), "{error}");
    }

    #[test]
    fn a_channel_without_an_exit_status_is_a_dropped_connection_not_a_success() {
        let stand = Stand::spawn();
        let session = stand.session();
        let error = session.exec("vanish").expect_err("no exit status must not read as success").to_string();
        assert!(error.contains("without an exit status"), "{error}");
        assert!(error.contains("connection likely dropped"), "{error}");
    }

    #[test]
    fn a_wrong_password_reports_the_rejection() {
        let stand = Stand::spawn();
        let error = match Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Password("wrong".into())) {
            Ok(_) => panic!("the server must reject a wrong password"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("rejected the credential"), "{error}");
    }

    #[test]
    fn an_unreachable_server_names_the_address() {
        // Port 1 on loopback: reliably closed, refuses instantly.
        let error = match Session::open("127.0.0.1", 1, USER, AuthMaterial::Password(PASSWORD.into())) {
            Ok(_) => panic!("nothing listens on port 1"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("cannot reach 127.0.0.1:1"), "{error}");
    }

    #[test]
    fn a_key_file_signs_in() {
        let stand = Stand::spawn();
        let scratch = tempfile::tempdir().expect("a scratch directory for the key");
        let key_path = scratch.path().join("id_ed25519");
        let openssh = ed25519_key(42, "test client key")
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("serialize the key");
        std::fs::write(&key_path, openssh.as_bytes()).expect("write the key file");

        let material = key_material(&key_path.display().to_string(), None).expect("load the key file");
        Session::open("127.0.0.1", stand.port, USER, material).expect("key sign-in on loopback");
    }

    /// An RSA key signs with a SHA-2 hash the server names. The stand refuses
    /// `ssh-rsa` (SHA-1) as OpenSSH 8.8 does, and refuses passwords, so a pass
    /// here is the key, signed the way a current sshd takes it. Up to v0.26
    /// every RSA signature was SHA-1 and this sign-in failed.
    #[test]
    fn an_rsa_key_signs_in_where_sha1_is_refused() {
        let stand = Stand::spawn_key_only();
        let scratch = tempfile::tempdir().expect("a scratch directory for the key");
        let key_path = scratch.path().join("id_rsa");
        let openssh = rsa_key(31, "rsa client")
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("serialize the key");
        std::fs::write(&key_path, openssh.as_bytes()).expect("write the key file");

        Session::open_with_key(&loopback(stand.port), USER, &key_path.display().to_string(), None).expect("the RSA key signs in with SHA-2");
    }

    /// A real SSH agent, in-process on a loopback socket.
    ///
    /// russh ships an agent server as well as a client, so the agent side of
    /// these tests speaks the actual protocol over an actual socket - the same
    /// standard Pageant and `ssh-agent` speak. Nothing about signing is
    /// simulated: the agent holds the private key and produces the signature
    /// the SSH server then verifies.
    ///
    /// Unix only, and deliberately so. The client reaches a real agent through
    /// `SSH_AUTH_SOCK`, or on Windows through Pageant and a named pipe - none
    /// of which a test can point at a temporary stand. What is covered here is
    /// the part that is identical on every platform: offering identities in
    /// turn, signing, and the failures. The Windows doors are covered by the
    /// live run on the owner machine.
    #[cfg(unix)]
    struct AgentStand {
        socket: PathBuf,
        _dir: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl AgentStand {
        /// Start an agent and load `keys` into it, in order.
        fn spawn(keys: Vec<russh::keys::PrivateKey>) -> Self {
            use tokio_stream::wrappers::UnixListenerStream;

            let dir = tempfile::tempdir().expect("a scratch directory for the agent socket");
            let socket = dir.path().join("agent.sock");
            let served = socket.clone();
            let (ready, wait) = mpsc::channel();
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("the agent runtime");
                runtime.block_on(async move {
                    let listener = tokio::net::UnixListener::bind(&served).expect("bind the agent socket");
                    ready.send(()).expect("report that the agent is listening");
                    let _ = russh::keys::agent::server::serve(UnixListenerStream::new(listener), ()).await;
                });
            });
            wait.recv().expect("the agent starts listening");

            let stand = Self { socket, _dir: dir };
            // The keystore starts empty: an agent holds what was added to it,
            // exactly the way `ssh-add` fills a real one.
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
            runtime.block_on(async {
                let mut client = russh::keys::agent::client::AgentClient::connect_uds(&stand.socket)
                    .await
                    .expect("connect to the test agent");
                for key in &keys {
                    client.add_identity(key, &[]).await.expect("add the key to the agent");
                }
            });
            stand
        }

        /// Point `SSH_AUTH_SOCK` at this agent for the duration of `body`.
        ///
        /// The variable is process-wide, so these tests must not run beside
        /// each other; `serial_env` is the lock that keeps them apart.
        fn with_env<T>(&self, body: impl FnOnce() -> T) -> T {
            let _guard = serial_env();
            let previous = std::env::var("SSH_AUTH_SOCK").ok();
            // SAFETY: the guard above serialises every test that touches this
            // variable, so no other thread reads it while it is being set.
            unsafe { std::env::set_var("SSH_AUTH_SOCK", &self.socket) };
            let outcome = body();
            match previous {
                Some(value) => unsafe { std::env::set_var("SSH_AUTH_SOCK", value) },
                None => unsafe { std::env::remove_var("SSH_AUTH_SOCK") },
            }
            outcome
        }
    }

    /// The lock serialising every test that edits the process environment.
    #[cfg(unix)]
    fn serial_env() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The point of the whole stage: a key the agent holds signs in to a
    /// server that refuses passwords, and turnout never reads a key file.
    #[cfg(unix)]
    #[test]
    fn an_agent_key_signs_in_where_the_password_cannot() {
        let key = ed25519_key(21, "agent key");
        let stand = Stand::spawn_authorizing(key.public_key().clone());
        let agent = AgentStand::spawn(vec![key]);

        // The password is refused by this stand, so a pass below cannot have
        // come from anywhere except the agent.
        let refused = Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Password(PASSWORD.into()));
        assert!(refused.is_err(), "this stand must not accept passwords");

        agent.with_env(|| {
            Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Agent).expect("the agent key signs in");
        });
    }

    /// An RSA key in the agent is asked for a SHA-2 signature too: the agent
    /// signs with whatever hash the request names, and a request naming none
    /// gets SHA-1, which the stand refuses as OpenSSH 8.8 does.
    #[cfg(unix)]
    #[test]
    fn an_rsa_key_in_the_agent_signs_in_where_sha1_is_refused() {
        let key = rsa_key(32, "agent rsa key");
        let stand = Stand::spawn_authorizing(key.public_key().clone());
        let agent = AgentStand::spawn(vec![key]);

        agent.with_env(|| {
            Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Agent).expect("the agent's RSA key signs in with SHA-2");
        });
    }

    /// An agent commonly holds several keys and a server authorizes one of
    /// them. Stopping at the first refusal would make agent auth work only for
    /// people whose accepted key happens to be offered first.
    #[cfg(unix)]
    #[test]
    fn the_accepted_key_is_found_behind_keys_the_server_refuses() {
        let accepted = ed25519_key(23, "the one that works");
        let stand = Stand::spawn_authorizing(accepted.public_key().clone());
        // Two refused keys ahead of it, so a first-only attempt fails here.
        let agent = AgentStand::spawn(vec![ed25519_key(24, "wrong one"), ed25519_key(25, "also wrong"), accepted]);

        agent.with_env(|| {
            Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Agent).expect("the accepted key is reached");
        });
    }

    /// When nothing the agent holds is accepted, the message has to say that
    /// the agent was reached and what it offered - otherwise the user goes
    /// hunting for a network or server fault that is not there.
    ///
    /// What it can say about each key is the algorithm, not the comment: a key
    /// is added to an agent over `ADD_IDENTITY`, whose comment field this
    /// client encodes empty, so the comment does not survive the trip. CI
    /// caught the first version of this test asserting on a name the protocol
    /// had already dropped. `describe` falls back to the algorithm for exactly
    /// this case, and a real `ssh-agent` populated by `ssh-add` does carry the
    /// comment, which is when the fuller form shows up.
    #[cfg(unix)]
    #[test]
    fn keys_the_server_refuses_are_named_in_the_failure() {
        // Authorizes a key the agent does not hold, so every offer is refused.
        let stand = Stand::spawn_authorizing(ed25519_key(26, "never offered").public_key().clone());
        let agent = AgentStand::spawn(vec![ed25519_key(27, "mine@laptop")]);

        let error = agent.with_env(|| match Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Agent) {
            Ok(_) => panic!("the server authorizes no key this agent holds"),
            Err(error) => format!("{error:#}"),
        });
        assert!(error.contains("the agent offered 1 key"), "{error}");
        assert!(error.contains("none accepted by the server"), "{error}");
        // The agent was reached, so the message must not read as a network or
        // server fault - that is the whole point of naming what was offered.
        assert!(error.contains("ssh-ed25519"), "the offered key is described: {error}");
        assert!(!error.contains("cannot reach"), "the agent was reached: {error}");
    }

    /// A running agent with an empty keystore is its own situation: the fix is
    /// `ssh-add` on this machine, not anything about the server.
    #[cfg(unix)]
    #[test]
    fn an_agent_holding_nothing_says_so() {
        let stand = Stand::spawn_key_only();
        let agent = AgentStand::spawn(Vec::new());

        let error = agent.with_env(|| match Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Agent) {
            Ok(_) => panic!("an empty agent cannot sign in"),
            Err(error) => format!("{error:#}"),
        });
        assert!(error.contains("holds no keys"), "{error}");
        assert!(error.contains("ssh-add"), "{error}");
    }

    /// With no agent at all, the failure names the variable to set rather than
    /// blaming the server.
    ///
    /// Asserted against `agent::connect` rather than through `Session::open`,
    /// because the session dials the server *before* it authenticates: with an
    /// unreachable port the connection error wins and the agent is never
    /// reached, so going through the session would test the wrong failure.
    /// CI caught exactly that - the first version of this test passed a dead
    /// port and asserted on a message that path cannot produce.
    #[cfg(unix)]
    #[test]
    fn no_agent_at_all_names_how_to_start_one() {
        let _guard = serial_env();
        let previous = std::env::var("SSH_AUTH_SOCK").ok();
        // SAFETY: serialised by the guard above.
        unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime");
        let outcome = runtime.block_on(async { super::agent::connect().await.map(|_| ()) });
        if let Some(value) = previous {
            unsafe { std::env::set_var("SSH_AUTH_SOCK", value) };
        }
        let error = match outcome {
            Ok(()) => panic!("there is no agent to connect to"),
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("SSH_AUTH_SOCK"), "{error}");
        assert!(error.contains("ssh-agent"), "{error}");
    }

    /// The agent credential resolves without touching the keyring or a file:
    /// there is nothing to resolve until the connection is being made.
    #[test]
    fn an_agent_credential_needs_neither_key_file_nor_secret() {
        let credential = Credential {
            name: "by-agent".into(),
            user: USER.into(),
            auth: Auth::Agent,
            key: None,
        };
        let material = auth_material(&credential).expect("an agent credential resolves with nothing stored");
        assert!(matches!(material, AuthMaterial::Agent), "{material:?}");
    }

    /// The check that ends `turnout key setup` has to prove the *key* works,
    /// which it can only do on a connection the password could not have opened.
    /// So the stand here refuses passwords outright, the way a hardened sshd
    /// does: a pass means the key was accepted and nothing else was.
    #[test]
    fn a_named_key_file_signs_in_where_the_password_cannot() {
        let stand = Stand::spawn_key_only();
        let scratch = tempfile::tempdir().expect("a scratch directory for the key");
        let key_path = scratch.path().join("id_ed25519");
        let openssh = ed25519_key(11, "key-only client")
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("serialize the key");
        std::fs::write(&key_path, openssh.as_bytes()).expect("write the key file");

        // The password is the right one, and still gets nowhere: that is what
        // makes the next assertion mean something.
        let refused = Session::open("127.0.0.1", stand.port, USER, AuthMaterial::Password(PASSWORD.into()));
        assert!(refused.is_err(), "this stand must not accept passwords");

        Session::open_with_key(&loopback(stand.port), USER, &key_path.display().to_string(), None).expect("the key signs in");
    }

    /// A key that does not exist must fail as a missing file, not as a network
    /// problem: `key setup` resolves the key before it dials anywhere.
    #[test]
    fn a_named_key_that_is_not_there_fails_before_dialing() {
        let error = match Session::open_with_key(&loopback(1), USER, "definitely/not/a/key", None) {
            Ok(_) => panic!("there is no such key"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("cannot read the key file"), "{error}");
        assert!(!error.contains("cannot reach"), "the key is resolved before the connection: {error}");
    }

    #[test]
    fn a_key_credential_without_a_key_file_names_the_fix() {
        let credential = Credential {
            name: "deployer".into(),
            user: USER.into(),
            auth: Auth::Key,
            key: None,
        };
        let error = auth_material(&credential).expect_err("no key file to load").to_string();
        assert!(error.contains("has no key file"), "{error}");
        assert!(error.contains("turnout credential edit deployer"), "{error}");
    }

    #[test]
    fn an_unreadable_key_file_is_reported_with_its_path() {
        let error = key_material("definitely/not/a/key", None).expect_err("the file does not exist").to_string();
        assert!(error.contains("cannot read the key file definitely/not/a/key"), "{error}");
    }

    #[test]
    fn mkdir_tolerates_an_existing_directory_but_not_a_refusal() {
        let stand = Stand::spawn();
        let session = stand.session();

        session.mkdir("/dist").expect("create a fresh directory");
        session.mkdir("/dist").expect("an existing directory is not an error");
        assert!(stand.root.path().join("dist").is_dir());

        // A missing parent is a real refusal: nothing exists at the path.
        let error = session.mkdir("/no/parent/here").expect_err("a missing parent is a refusal").to_string();
        assert!(error.contains("cannot create the directory /no/parent/here"), "{error}");

        // So is a file squatting on the name: mkdir failed and the path is
        // not a directory - this must not pass for "already exists".
        std::fs::write(stand.root.path().join("taken"), b"x").expect("plant a file in the way");
        let error = session.mkdir("/taken").expect_err("a file in the way is a refusal").to_string();
        assert!(error.contains("cannot create the directory /taken"), "{error}");
    }

    /// Every missing parent is made, and what exists is passed over; a file
    /// in the way is still a refusal.
    #[test]
    fn mkdir_all_makes_the_parents_a_directory_is_missing() {
        let stand = Stand::spawn();
        let session = stand.session();
        session.mkdir_all("/srv/deep/er/").expect("create the whole chain");
        assert!(stand.root.path().join("srv").join("deep").join("er").is_dir());
        session.mkdir_all("/srv/deep/er/still").expect("an existing chain is not an error");
        assert!(stand.root.path().join("srv").join("deep").join("er").join("still").is_dir());

        std::fs::write(stand.root.path().join("srv").join("taken"), b"x").unwrap();
        let error = session.mkdir_all("/srv/taken/below").expect_err("a file in the way").to_string();
        assert!(error.contains("cannot create the directory /srv/taken"), "{error}");
    }

    #[test]
    fn upload_streams_a_file_in_bounded_chunks_and_credits_every_byte() {
        let stand = Stand::spawn();
        let session = stand.session();

        // Larger than three chunks, and not a multiple of CHUNK.
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let scratch = tempfile::tempdir().expect("a scratch directory for the payload");
        let local = scratch.path().join("payload.bin");
        std::fs::write(&local, &payload).expect("write the payload");

        let mut chunks = Vec::new();
        let total = session.upload(&local, "/payload.bin", |sent| chunks.push(sent)).expect("upload");

        assert_eq!(total, payload.len() as u64);
        assert_eq!(chunks.iter().sum::<u64>(), payload.len() as u64, "every byte is credited exactly once");
        assert!(chunks.len() >= 4, "200 KB must cross the wire in several bounded pieces, got {}", chunks.len());
        assert!(chunks.iter().all(|&sent| sent <= CHUNK as u64), "no chunk exceeds the bound");
        assert_eq!(std::fs::read(stand.root.path().join("payload.bin")).expect("read what arrived"), payload);
    }

    #[test]
    fn upload_bytes_arrives_intact_over_one_shared_sftp_subsystem() {
        let stand = Stand::spawn();
        let session = stand.session();

        session.upload_bytes(b"alpha", "/a.txt", |_| {}).expect("first upload");
        session.upload_bytes(b"beta", "/b.txt", |_| {}).expect("second upload");

        assert_eq!(std::fs::read(stand.root.path().join("a.txt")).expect("a.txt arrived"), b"alpha");
        assert_eq!(std::fs::read(stand.root.path().join("b.txt")).expect("b.txt arrived"), b"beta");
        // The v0.10.1 fix: one SFTP subsystem per session, however many files.
        assert_eq!(stand.subsystem_opens.load(Ordering::SeqCst), 1, "the SFTP subsystem is opened once and shared");
    }

    /// A loopback server that sends back whatever it is sent - what the
    /// forwarding tests reach through the stand.
    fn echo_server(runtime: &Runtime) -> u16 {
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind the echo server");
            let port = listener.local_addr().expect("its address").port();
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = listener.accept().await {
                    tokio::spawn(async move {
                        let (mut read, mut write) = socket.split();
                        let _ = tokio::io::copy(&mut read, &mut write).await;
                    });
                }
            });
            port
        })
    }

    fn forwarder(runtime: &Runtime, stand: &Stand) -> Forwarder {
        runtime
            .block_on(Forwarder::connect_checked(
                known_hosts::file().unwrap(),
                None,
                "127.0.0.1",
                stand.port(),
                USER,
                AuthMaterial::Password(PASSWORD.into()),
            ))
            .expect("password sign-in on loopback")
    }

    fn origin() -> std::net::SocketAddr {
        std::net::SocketAddr::from(([127, 0, 0, 1], 50000))
    }

    /// A forwarded connection carries bytes both ways, and the end of one
    /// side reaches the other: the echo server sees the end of what was sent
    /// and closes, and the read here ends with it.
    #[test]
    fn a_forwarded_connection_carries_bytes_both_ways() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let stand = Stand::spawn();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let echo = echo_server(&runtime);
        let forwarder = forwarder(&runtime, &stand);
        let back = runtime.block_on(async {
            let mut stream = forwarder.open("127.0.0.1", echo, origin()).await.expect("the stand reaches the echo server");
            stream.write_all(b"ping through the stand").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut back = Vec::new();
            tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_to_end(&mut back))
                .await
                .expect("the end of the echo reaches this side")
                .unwrap();
            back
        });
        assert_eq!(back, b"ping through the stand");
        assert!(!forwarder.is_closed());
        runtime.block_on(forwarder.close());
    }

    /// The server's two refusals are told apart: nothing listening at the
    /// address, and an sshd that forwards nothing at all.
    #[test]
    fn a_refused_forward_says_why() {
        let stand = Stand::spawn();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        // A port nothing listens on: bound and let go.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let forwarder = forwarder(&runtime, &stand);
        let refused = |why: &str| match runtime.block_on(forwarder.open("127.0.0.1", closed, origin())) {
            Ok(_) => panic!("{why}, and the forward was opened"),
            Err(failure) => failure,
        };
        let failure = refused("nothing listens there");
        assert!(matches!(failure, OpenFailure::Unreachable), "{failure:?}");

        stand.refuse_forwarding();
        let failure = refused("forwarding is off");
        assert!(matches!(failure, OpenFailure::Prohibited), "{failure:?}");
    }
}
