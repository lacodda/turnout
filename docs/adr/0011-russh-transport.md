# ADR 0011: SSH on russh, and one TLS stack (rustls)

- Status: accepted
- Date: 2026-08-14
- Supersedes: the `ssh2`/libssh2 choice implicit in ADR 0001-era code

## Context

turnout's remote layer was built on the `ssh2` crate, a binding to libssh2. On this project's build machines libssh2 links the system crypto backend (WinCNG on Windows), which does **not** implement curve25519. Two failures followed, and both were latent from v0.1.0 because the affected paths were never exercised against a strict server:

- **Key auth never worked.** `userauth_pubkey_file` with an OpenSSH-format ed25519 key - the `ssh-keygen` default for years - failed with `[Session(-1)] unknown error`. Every deploy that used a key would have hit this; the owner's stands happened to use passwords.
- **Modern strict servers were unreachable at all.** A current OpenSSH configured to offer only curve25519 / post-quantum KEX (no diffie-hellman-group fallback) rejected the handshake outright: `Unable to exchange encryption keys`. This is the default trajectory of OpenSSH hardening.

The libssh2 `vendored-openssl` feature would rebuild it against OpenSSL (which has curve25519), but that needs a native Perl + OpenSSL on the build machine, absent here, and re-introduces a C build besides.

Separately, the tree carried **two TLS stacks**: rustls (via reqwest, for the HTTP proxy) and native-tls (via tokio-tungstenite, for the gateway's WebSocket proxy) - the same "C-crypto vs pure Rust" split, one library away from the same class of incompatibility.

## Decision

**Replace `ssh2` with `russh` + `russh-sftp`.** russh is pure Rust and negotiates curve25519 and ed25519 natively. This was verified before committing: a russh handshake against the exact strict server libssh2 could not reach succeeded (`KEX_OK`), and the predecessor tool `depl`, already on russh, reaches it.

russh is async; turnout's command layer is synchronous. Rather than colour the whole `deploy`/`backup`/`restore` call path async, the transport (`src/ssh.rs`) owns a current-thread tokio runtime and exposes **blocking** methods (`connect`, `exec`, `upload`, `upload_bytes`, `mkdir`) - the same shape the `ssh2::Session` it replaces had. The gateway already drives a runtime this way (ADR 0008's state-file model keeps `main` synchronous), so the pattern is not new.

**Move the WebSocket proxy to rustls too**, and drop native-tls. The gateway's accept-invalid-certs path becomes a rustls `ServerCertVerifier` that approves everything, scoped to servers explicitly marked `accept_invalid_certs` - the same opt-in as before. reqwest and russh both resolve to the `aws-lc-rs` rustls provider, so there is one provider, one TLS stack.

## Consequences

- turnout connects to current, strictly-configured OpenSSH, by key and by password. The key path works for the first time. Verified on the Pi stand: deploy (file-by-file and archive), backup, clear, restore, restart - all over an ed25519 key that libssh2 refused.
- **No C crypto left.** `ssh2`, `libssh2-sys`, `native-tls`, `openssl`, `openssl-sys`, `libz-sys` are gone from the tree and the lockfile. The only remaining C is `aws-lc` (rustls's provider), which is actively maintained and not a pain point.
- **ssh-agent support was dropped.** It rode on libssh2's `userauth_agent`; russh offers it through a separate `russh::keys::agent` integration. Rather than ship a text that promises an agent the code no longer tries, agent auth is removed and scheduled for the key-setup stage where it belongs. Auth is now: the credential's key file, or the stored password.
- **Host keys are still not pinned.** libssh2 was used without a known-hosts check, and russh's `check_server_key` returns `true` to match. A known-hosts check is a candidate for a later stage; this ADR only restores the ability to connect.
- russh pulls a RustCrypto key stack (`ssh-key`, `rsa`, `argon2 0.6-rc`, …) that is currently at release-candidate versions. These are russh's transitive choices, not turnout's; turnout's own `argon2` (export sealing) stays on stable 0.5. They stabilize when russh updates.
