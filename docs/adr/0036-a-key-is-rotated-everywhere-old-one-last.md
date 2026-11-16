# ADR 0036: A key is rotated on every server at once, the old one removed last

- Status: accepted
- Date: 2026-11-15

## Context

`key setup` (ADR 0014) puts a key on one server. A credential is free-standing (ADR 0010) and one key signs in to every server that takes it - which is the point, and also what makes replacing the key a chore with a trap in it: done by hand, server after server, there is a stretch where some stands admit only the old key and some only the new, and the catalog can point at one of them.

## Decision

**`turnout key rotate CREDENTIAL` works on every server the credential signs in to** - the servers that name it and the ones a target reaches with it - in an order that keeps access whole at every point:

1. the new key is authorized on every server, signing in with the old one;
2. the new key signs in to every server by itself, in fresh sessions that offer nothing else (the check `key setup` makes);
3. the credential is switched to the new key;
4. the old key is taken off the servers, through the new one.

**Before step 3 a failure undoes everything**: the new key is withdrawn from the servers that got it, its file is removed, the credential never moved. **After step 3 nothing can lock the credential out**: a server that keeps the old key's line is named and the command fails, and what is left is a line to remove.

**It is all the servers or none.** A server that is down stops the rotation before it changes anything. A flag to leave one out was considered and dropped: the server left behind would admit only a key the catalog no longer names, and getting back in would be handwork with the old key file - the half-rotated state the command exists to prevent.

**The old key's line is removed by replacing the file whole.** The keys that stay are written beside `authorized_keys` and put in its place - moved over it on POSIX, where that is one step and the mode is set first; copied over it on Windows, where a copy keeps the ACL `administrators_authorized_keys` is only read under. A connection lost half-way leaves the file as it was, not cut short with the key that just signed in gone from it. The line is found by the key's body, as `key setup` finds it: a comment edited by hand or options in front do not hide it.

**No key file is overwritten or deleted.** The new key goes beside the old under the next free name; the old file stays, because turnout cannot know what else uses it. A passphrase stored for the old key is removed - the new key has none.

## Consequences

- Rotating asks first: whatever else signs in to those servers with the old key loses access, which is what a rotation is for.
- An `auth = agent` credential is refused - its key is the agent's - and a password credential is sent to `key setup`.
- Proven on the Windows server CI stands up (ADR 0029), for an administrator's shared file and an ordinary account's own, and live against OpenSSH on Linux.
