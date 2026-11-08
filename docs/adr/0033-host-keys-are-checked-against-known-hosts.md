# ADR 0033: Host keys are checked against OpenSSH's known_hosts

- Status: accepted
- Date: 2026-11-08
- Supersedes the "host keys are still not pinned" consequence of [ADR 0011](0011-russh-transport.md)

## Context

Up to v0.27 turnout accepted any host key a server presented (`AcceptAnyHostKey`). ADR 0011 recorded it as a known gap, inherited from libssh2. For a deploy tool that signs in with stored passwords it is the gap that matters: anyone able to answer on a stand's address - a hostile network, a DNS mistake, an address reused by another machine - receives the credential's password in the clear of the encrypted channel it set up itself. 1.0 is the release that promises no known debts, so it closes this one.

Two questions shaped the fix: where trusted keys live, and what happens the first time a host is met.

## Decision

**Keys live in OpenSSH's own `~/.ssh/known_hosts`** (`%USERPROFILE%\.ssh\known_hosts` on Windows), not in a file of turnout's. `turnout ssh` already runs the system `ssh`, which reads that file; a second file would let the two disagree about the same server, and a reinstalled server would have to be mended twice. `TURNOUT_KNOWN_HOSTS` names another file for tests and scripts, and `turnout ssh` passes it to `ssh` as `UserKnownHostsFile`.

**A host met for the first time is trusted on first use and recorded; a known host must present a key on record.** This is OpenSSH's `StrictHostKeyChecking=accept-new`. Asking on every first contact would stop background jobs and scripts, and an unattended deploy cannot answer a prompt. Refusing unknown hosts outright would make every new stand a manual step with nothing to compare against. A changed key is refused with the presented and the recorded fingerprints and the command that mends it.

**The file is read as OpenSSH reads it** - hashed names (HMAC-SHA1, through the `hmac` and `sha1` crates russh already builds), `[host]:port`, glob and negated patterns, `@revoked` - with `ssh-key`'s line parser. russh's own `known_hosts` helpers were not used: they fail every check on one unreadable line, ignore patterns and markers, and count line numbers wrong past a comment.

**Any recorded key for the address settles it, whatever its algorithm**, and the client asks first for the algorithms on record. Otherwise an interceptor holding only an RSA key could pass as a server known by its ED25519 key, and a legitimate server known by RSA would be refused for presenting ED25519 first.

**`turnout server trust NAME` replaces the recorded key with the one presented now**, after showing it and how to read the server's own fingerprint, behind a confirmation (`--yes` without a terminal). It removes only what names that address: other hosts on a shared line keep their entry, `@revoked` lines stay.

## Consequences

- A server reinstalled with new host keys refuses every command until `turnout server trust` runs - the intended cost.
- The first connection to each address writes to `~/.ssh/known_hosts`, creating `~/.ssh` if needed.
- The check runs inside the key exchange, before authentication: a refused host never sees the password or a signature.
- Unit tests read a per-process scratch file and integration tests one inside their data directory; the suite never touches the developer's own file.
- Host certificates are not supported: a certificate is checked by the key it certifies, and `@cert-authority` lines trust nothing.
