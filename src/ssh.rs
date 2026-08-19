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
    ///
    /// The credential is resolved to [`AuthMaterial`] before dialing: a missing
    /// key file or an unstored password fails fast, without a network round
    /// trip that could not have succeeded anyway.
    pub fn connect(server: &Server, credential: &Credential) -> Result<Self> {
        let material = auth_material(credential)?;
        Self::open(&server.ssh_host(), server.port, &credential.user, material)
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
}

/// Deliberately manual: a derived impl would print the password.
impl std::fmt::Debug for AuthMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            AuthMaterial::Key(_) => "AuthMaterial::Key",
            AuthMaterial::Password(_) => "AuthMaterial::Password(<redacted>)",
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
    };

    if !ok.success() {
        bail!("SSH authentication failed for '{user}' - the server rejected the credential");
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
    }

    impl server::Server for Factory {
        type Handler = TestHandler;

        fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> TestHandler {
            TestHandler {
                root: self.root.clone(),
                subsystem_opens: self.subsystem_opens.clone(),
                channels: HashMap::new(),
            }
        }
    }

    struct TestHandler {
        root: PathBuf,
        subsystem_opens: Arc<AtomicUsize>,
        channels: HashMap<ChannelId, ServerChannel<ServerMsg>>,
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
            Ok(if user == USER && password == PASSWORD {
                AuthAnswer::Accept
            } else {
                rejected()
            })
        }

        async fn auth_publickey(&mut self, user: &str, _key: &russh::keys::PublicKey) -> Result<AuthAnswer, Self::Error> {
            // russh verifies the signature; the handler only says whether the
            // user may sign in with a key at all.
            Ok(if user == USER { AuthAnswer::Accept } else { rejected() })
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
