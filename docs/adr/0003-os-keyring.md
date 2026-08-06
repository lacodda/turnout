# ADR 0003: Secrets live in the OS keyring

- Status: accepted
- Date: 2026-08-05

## Context

turnout stores logins, passwords and tokens for servers. A custom encrypted store needs a key, and that key has to live somewhere: next to the data (decorative security - the predecessor tool did exactly this) or behind a master password (kills the "copy a password in one second" workflow).

## Decision

Store secrets in the operating system keyring via the `keyring` crate: Windows Credential Manager, macOS Keychain, Linux Secret Service. turnout config files hold only metadata (login, server binding, secret kind) - never secret values. Secrets are never printed to logs or default command output.

## Consequences

- No master password, no key management, instant access guarded by the user's OS session.
- Platform-specific behavior; CI and tests must cover all three OSes, headless Linux needs a Secret Service provider.
- Secrets do not travel with config backups - documented: credentials are re-entered per machine by design.
