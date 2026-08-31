---
title: "key"
description: Set up SSH key access to a server - generate a key, authorize it, and switch the credential over.
sidebar:
  order: 7.5
---

```bash
turnout key <setup|check> [SERVER] [--credential NAME]
```

Signing in with a key has always worked; getting the key *onto* the server was the part left to `ssh-keygen`, `ssh-copy-id` and a search engine. `turnout key setup` does the whole thing: it generates a key, authorizes it on the server, checks that it really signs in, and only then points the [credential](/turnout/reference/credential/) at it.

It is a top-level command rather than a subcommand of `server` or `credential` because it touches both: it reads a server and writes a credential.

## setup

```bash
turnout key setup [SERVER] [--credential NAME] [--key PATH]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--credential` | `-c` | Credential to give key access (defaults to the server's) |
| `--key` | `-K` | Authorize an existing private key instead of generating one |

```bash
turnout key setup prod                      # generate a key, authorize it, switch over
turnout key setup prod -c deploy            # a credential other than the server's
turnout key setup prod -K ~/.ssh/id_ed25519 # authorize a key you already have
```

The server name can be omitted for a [picker](/turnout/concepts/pickers/); the credential defaults to the server's own.

### What it does, in order

1. **Gets a key.** With no `--key` and no key on the credential, it generates an **ed25519** pair at `~/.ssh/id_ed25519_<credential>` (you are asked, and can change the path), writing the `.pub` sibling too - a key turnout makes is an ordinary key that plain `ssh` can use. A credential that already names a key file reuses it, which is how you authorize one key on a second server.
2. **Signs in with the password.** This is the last time it is needed - which is the point.
3. **Finds the right file.** Asks the server for the account's home directory and which shell answers, then writes to `~/.ssh/authorized_keys` - or, for a Windows administrator, somewhere else entirely (see below).
4. **Installs the key**, unless it is already authorized, and sets the permissions sshd insists on.
5. **Proves it works** by opening a second, separate connection that offers *only* the key.
6. **Switches the credential** to `auth = key` - and only if step 5 passed.

That last order matters: if the key does not sign in, the credential is left on its password, so a failed setup never costs you access. The stored password is kept either way; removing it is your call:

```bash
turnout pass remove prod-deploy
```

Running `setup` twice is safe. An already-authorized key is recognized - by its body, not its comment - and not appended again.

### Windows servers

OpenSSH on Windows ships an `sshd_config` with a `Match Group administrators` block, and for a member of that group it reads **only**:

```
C:\ProgramData\ssh\administrators_authorized_keys
```

A key written to that account's own `~/.ssh/authorized_keys` is ignored without a word - the sign-in just keeps asking for the password. `turnout key setup` checks whether the account is an administrator and writes to the file sshd will actually read, then fixes the file's ACL, which Windows sshd is equally strict about. See [Windows servers](/turnout/concepts/windows-servers/) for the rest of what changes on that side.

## check

```bash
turnout key check [SERVER] [--credential NAME]
```

Signs in with the credential's key and reports whether it worked. Useful after changing something on the server - it answers the question `deploy` would otherwise answer the slow way.

```bash
turnout key check prod
```

## When the key is refused

If the key installs but the server still will not take it, turnout names the file it wrote to and the causes that actually produce that symptom:

- `PubkeyAuthentication no` in `sshd_config`.
- On Linux: a group-writable home or `~/.ssh`, which sshd refuses (`chmod go-w ~ ~/.ssh`).
- On Linux: `AuthorizedKeysFile` pointing somewhere other than `~/.ssh/authorized_keys`.
- On Windows: an ACL granting more than the owner and SYSTEM, or an account that is in the administrators group after all.

The credential stays on its password in every one of those cases, so you can fix the server and run `setup` again.

## See also

- [credential](/turnout/reference/credential/) - the entity this writes to
- [pass](/turnout/reference/pass/) - the password used for the first sign-in, and the passphrase of a protected key
- [server](/turnout/reference/server/) - the entity this reads
