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
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{Channel, ChannelMsg};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::runtime::Runtime;

use crate::agent;
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

    async fn check_server_key(&mut self, _key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
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
    ///
    /// The credential is resolved to [`AuthMaterial`] before dialing: a missing
    /// key file or an unstored password fails fast, without a network round
    /// trip that could not have succeeded anyway.
    pub fn connect(server: &Server, credential: &Credential) -> Result<Self> {
        let material = auth_material(credential)?;
        Self::open(&server.ssh_host(), server.port, &credential.user, material)
    }

    /// Open a session authenticating with one named key file, and nothing else.
    ///
    /// [`Session::connect`] goes through the credential, which is exactly what
    /// the key-setup check must not do: it runs *before* the credential is
    /// switched over, so going through it would sign in with the password and
    /// prove nothing about the key. Offering only the key is what makes a
    /// success mean the server accepted it.
    pub fn open_with_key(host: &str, port: u16, user: &str, key_path: &str, passphrase: Option<&str>) -> Result<Self> {
        Self::open(host, port, user, key_material(key_path, passphrase)?)
    }

    /// Open a session with already-resolved auth material.
    fn open(host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("cannot start the SSH runtime")?;
        let handle = runtime.block_on(authenticate(host, port, user, material))?;
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

/// A credential resolved to what the wire can use: the key loaded, the
/// password fetched. Resolution happens before dialing (see
/// [`Session::connect`]) and touches the keyring; everything after it is pure
/// network, which is what makes [`authenticate`] testable against a local
/// server.
enum AuthMaterial {
    Key(PrivateKeyWithHashAlg),
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
    // None lets russh pick the signature algorithm; only RSA needs an
    // explicit SHA-2 choice, and ed25519 - the common case - ignores it.
    Ok(AuthMaterial::Key(PrivateKeyWithHashAlg::new(Arc::new(key), None)))
}

/// Connect and authenticate, returning the live handle.
async fn authenticate(host: &str, port: u16, user: &str, material: AuthMaterial) -> Result<Handle<AcceptAnyHostKey>> {
    let config = Arc::new(client::Config::default());
    let mut handle = client::connect(config, (host, port), AcceptAnyHostKey)
        .await
        .with_context(|| format!("cannot reach {host}:{port}"))?;

    let ok = match material {
        AuthMaterial::Key(key) => handle.authenticate_publickey(user, key).await.context("key authentication failed")?,
        AuthMaterial::Password(password) => handle.authenticate_password(user, password).await.context("password authentication failed")?,
        AuthMaterial::Agent => return authenticate_with_agent(handle, user).await,
    };

    if !ok.success() {
        bail!("SSH authentication failed for '{user}' - the server rejected the credential");
    }
    Ok(handle)
}

/// Authenticate by asking the agent to sign, offering its keys in turn.
///
/// The agent may hold many keys and the server accepts at most one of them, so
/// a refusal of the first is not a failure - only running out of keys is. This
/// is what `ssh` itself does, and why the loop exists rather than a single
/// attempt on the first identity.
async fn authenticate_with_agent(mut handle: Handle<AcceptAnyHostKey>, user: &str) -> Result<Handle<AcceptAnyHostKey>> {
    let mut agent = agent::connect().await?;
    let identities = agent::identities(&mut agent).await?;
    if identities.is_empty() {
        return Err(agent::no_identities());
    }

    let mut offered = Vec::new();
    for identity in &identities {
        let public = identity.public_key().into_owned();
        offered.push(agent::describe(identity));
        // Errors here are the agent failing to sign - a locked agent, a key
        // pulled out mid-run - not the server saying no. Either way the next
        // key is worth trying; what matters is whether any of them got in.
        if let Ok(result) = handle.authenticate_publickey_with(user, public, None, &mut agent).await
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

#[cfg(test)]
mod tests {
    //! The transport is exercised against a real SSH server: russh's server
    //! side, in-process, on a loopback port. Nothing is mocked below the
    //! protocol - auth, exec and SFTP all cross an actual TCP connection, so
    //! these tests fail for the same reasons a live stand would.

    use std::collections::HashMap;
    use std::io::{Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    /// One running test server: its port, the directory SFTP writes into, and
    /// a counter of how many times the SFTP subsystem was opened.
    struct Stand {
        port: u16,
        root: tempfile::TempDir,
        subsystem_opens: Arc<AtomicUsize>,
    }

    impl Stand {
        fn spawn() -> Self {
            Self::spawn_with(true)
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
            Self::spawn_full(false, Some(authorized))
        }

        fn spawn_with(passwords_accepted: bool) -> Self {
            Self::spawn_full(passwords_accepted, None)
        }

        fn spawn_full(passwords_accepted: bool, authorized: Option<russh::keys::PublicKey>) -> Self {
            let root = tempfile::tempdir().expect("a scratch directory for the SFTP root");
            let subsystem_opens = Arc::new(AtomicUsize::new(0));
            let served_root = root.path().to_path_buf();
            let served_opens = subsystem_opens.clone();
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
                        ..Default::default()
                    });
                    let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.expect("bind a loopback port");
                    report.send(socket.local_addr().expect("the bound address").port()).expect("report the port");
                    let mut factory = Factory {
                        root: served_root,
                        subsystem_opens: served_opens,
                        passwords_accepted,
                        authorized,
                    };
                    let _ = server::Server::run_on_socket(&mut factory, config, &socket).await;
                });
            });
            let port = learn.recv().expect("the server reports its port");
            Self { port, root, subsystem_opens }
        }

        fn session(&self) -> Session {
            Session::open("127.0.0.1", self.port, USER, AuthMaterial::Password(PASSWORD.into())).expect("password sign-in on loopback")
        }
    }

    struct Factory {
        root: PathBuf,
        subsystem_opens: Arc<AtomicUsize>,
        passwords_accepted: bool,
        /// When set, the only public key this stand accepts.
        authorized: Option<russh::keys::PublicKey>,
    }

    impl server::Server for Factory {
        type Handler = TestHandler;

        fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> TestHandler {
            TestHandler {
                root: self.root.clone(),
                subsystem_opens: self.subsystem_opens.clone(),
                channels: HashMap::new(),
                passwords_accepted: self.passwords_accepted,
                authorized: self.authorized.clone(),
            }
        }
    }

    struct TestHandler {
        root: PathBuf,
        subsystem_opens: Arc<AtomicUsize>,
        channels: HashMap<ChannelId, ServerChannel<ServerMsg>>,
        passwords_accepted: bool,
        authorized: Option<russh::keys::PublicKey>,
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

        async fn auth_publickey(&mut self, user: &str, key: &russh::keys::PublicKey) -> Result<AuthAnswer, Self::Error> {
            // russh verifies the signature; the handler only says whether the
            // user may sign in with this key at all.
            let key_allowed = match &self.authorized {
                Some(authorized) => authorized == key,
                None => true,
            };
            Ok(if user == USER && key_allowed { AuthAnswer::Accept } else { rejected() })
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
            if let Some(text) = command.strip_prefix("echo ") {
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
            };
            tokio::spawn(async move {
                russh_sftp::server::run(channel.into_stream(), handler).await;
            });
            Ok(())
        }
    }

    /// An SFTP server over a scratch directory: just enough of the protocol
    /// for what the transport uses - open/write/close, mkdir, stat.
    struct TestSftp {
        root: PathBuf,
        files: HashMap<String, std::fs::File>,
    }

    fn resolve(root: &Path, remote: &str) -> PathBuf {
        root.join(remote.trim_start_matches('/'))
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
                .open(resolve(&self.root, &filename))
                .map_err(|_| StatusCode::Failure)?;
            self.files.insert(filename.clone(), file);
            Ok(SftpHandle { id, handle: filename })
        }

        async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
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
            std::fs::create_dir(resolve(&self.root, &path)).map_err(|_| StatusCode::Failure)?;
            Ok(done(id))
        }

        async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
            let metadata = std::fs::metadata(resolve(&self.root, &path)).map_err(|_| StatusCode::NoSuchFile)?;
            Ok(Attrs {
                id,
                attrs: FileAttributes::from(&metadata),
            })
        }
    }

    #[test]
    fn exec_returns_stdout_on_a_zero_exit() {
        let stand = Stand::spawn();
        let session = stand.session();
        assert_eq!(session.exec("echo ready").expect("echo succeeds"), "ready\n");
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

    /// The check that ends `turnout key setup` has to prove the *key* works,
    /// which it can only do on a connection the password could not have opened.
    /// So the stand here refuses passwords outright, the way a hardened sshd
    /// does: a pass means the key was accepted and nothing else was.
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
        assert!(error.contains("mine@laptop"), "the offered key is named: {error}");
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
    #[cfg(unix)]
    #[test]
    fn no_agent_at_all_names_how_to_start_one() {
        let _guard = serial_env();
        let previous = std::env::var("SSH_AUTH_SOCK").ok();
        // SAFETY: serialised by the guard above.
        unsafe { std::env::remove_var("SSH_AUTH_SOCK") };
        let error = match Session::open("127.0.0.1", 1, USER, AuthMaterial::Agent) {
            Ok(_) => panic!("there is no agent"),
            Err(error) => format!("{error:#}"),
        };
        if let Some(value) = previous {
            unsafe { std::env::set_var("SSH_AUTH_SOCK", value) };
        }
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

        Session::open_with_key("127.0.0.1", stand.port, USER, &key_path.display().to_string(), None).expect("the key signs in");
    }

    /// A key that does not exist must fail as a missing file, not as a network
    /// problem: `key setup` resolves the key before it dials anywhere.
    #[test]
    fn a_named_key_that_is_not_there_fails_before_dialing() {
        let error = match Session::open_with_key("127.0.0.1", 1, USER, "definitely/not/a/key", None) {
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
}
