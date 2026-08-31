# ADR 0014: Key setup is its own command, and switches the credential only after the key is proven

- Status: accepted
- Date: 2026-08-31

## Context

Signing in with a key has worked since v0.10.0, when the transport moved to russh (ADR 0011). What had never worked was *getting a key onto a server*. That was left to the user: run `ssh-keygen`, run `ssh-copy-id` - which does not exist on Windows, in either direction - fix the permissions sshd silently insists on, then come back and run `turnout credential edit --auth key --key PATH`. Four tools and a search engine for one intention.

Two things made this worth building rather than documenting.

**A Windows server is not a Linux server with different paths.** OpenSSH on Windows ships an `sshd_config` whose `Match Group administrators` block points at one shared file:

```
C:\ProgramData\ssh\administrators_authorized_keys
```

For a member of that group sshd reads *only* that file. A key written to that account's own `~/.ssh/authorized_keys` is ignored without a word - the sign-in just keeps asking for the password, which looks exactly like a wrong key. The deploy account on a Windows stand is usually an administrator, so the wrong guess is the common case, and the symptom points nowhere near the cause. This is the same shape as the `tar` field report behind ADR's dialect work: the tool was fine, the question was malformed.

**The order of operations decides whether a failure costs you access.** Setting `auth = key` on the credential is the last irreversible step. Doing it before the key is known to work turns a server-side misconfiguration into a locked door: the password that still works is no longer the one turnout will offer.

Where the command should live was the open question. The key is a property of a `Credential` - `auth`, `user` and `key` all live there. But it is installed on a `Server`, and the server is what the user is thinking about ("give me key access to prod"). Three placements were considered: `turnout server key setup`, `turnout credential key setup`, and a top-level `turnout key`.

## Decision

**`turnout key setup` is a top-level command, and the credential is switched to `auth = key` only after a second, key-only connection has succeeded.**

On placement: the operation reads a server and writes a credential, so it belongs to neither. A subcommand of `server` would be quietly editing a credential; a subcommand of `credential` would take the server as a mandatory argument to something that looks like a catalog edit. This is the reasoning of ADR 0013 applied one level up - an operation that joins two entities is addressed on its own, not smuggled into one of them.

The steps run in this order, and the order is the design:

1. **Check there is a way in at all** - a password credential with nothing in the keyring is refused here, before anything is created.
2. **Get a key**: generate an ed25519 pair (with its `.pub` sibling, so it is an ordinary key that plain `ssh` can use), or reuse the one the credential already names.
3. **Sign in with the password** - the last time it is needed.
4. **Ask the server where the file is**: its shell dialect, its home directory, and on Windows whether this account is an administrator.
5. **Install**, unless the key is already authorized, then set the permissions.
6. **Prove it** with a fresh connection offering *only* the key.
7. **Switch the credential** - and only now.

Step 6 is why `Session::open_with_key` exists beside `Session::connect`. Checking through the credential would sign in with the password, which is still what the credential says, and report success without having tested anything.

**The administrator question is asked, not assumed.** `net session` succeeds only with administrator rights and does not depend on the group's name, which is localized. A refusal is the answer "no", not an error.

**Membership is by key body, not by line.** An already-installed key is recognized by its algorithm and base64 body, ignoring the comment and any leading options. So a second run appends nothing, and a key someone deliberately fenced in with `from=` or `command=` is not quietly re-added without its restrictions.

**ssh-agent support is not part of this.** It was dropped with libssh2 in v0.10.0 and its return is a stage of its own: it is a third kind of `Auth`, with its own Windows behaviour through Pageant. Bundling it here would have made one version out of two themes.

## Consequences

- `turnout key setup` and `turnout key check` are new; nothing is removed, and a credential set up by hand still works exactly as before.
- A failed setup never costs access: the credential is left on its password, and the error names the file that was written plus the causes that actually produce a refused key - `PubkeyAuthentication no`, a group-writable home, a `AuthorizedKeysFile` pointing elsewhere, a too-permissive Windows ACL.
- The Windows administrator path is decided by a pure function over (dialect, home, administrator), so it is tested from a Linux CI box that cannot reach a Windows server - the same split ADR's dialect work established.
- The stored password is kept after the switch. It is unused but still valid, and it is the way back in if the key is lost; removing it is a separate, deliberate `turnout pass remove`.
- A live run against a real stand caught what the tests had not: with no password stored, the command generated the key *first* and failed afterwards, leaving a key nobody asked for that the next run then refused to start because of. Hence step 1 being a step. The test that holds it now runs the real binary and looks at the disk, because that is the level the defect lived at.
