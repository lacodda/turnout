---
title: "key"
description: Set up SSH key access to a server - generate a key, authorize it, switch the credential over - and replace a key on every server it signs in to.
sidebar:
  order: 7.5
---

```bash
turnout key <setup|check> [SERVER] [--credential NAME]
turnout key rotate [CREDENTIAL] [--yes]
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

1. **Gets a key.** With no `--key` and no key on the credential, it generates an **ed25519** pair for `~/.ssh/id_ed25519_<credential>` (you are asked, and can change the path) - in memory for now. A credential that already names a key file reuses it, which is how you authorize one key on a second server. A key already at the path is never overwritten: one that opens without a passphrase - most often the key an earlier run wrote before its install or its check failed - is used instead, so running the command again after fixing the server just works and the server does not collect a key per attempt. One locked by a passphrase is refused before anything is asked of the server.
2. **Signs in with the password.** This is the last time it is needed - which is the point. A wrong password, a server that does not answer or an account sshd does not let in stops the setup here, with nothing written on either machine.
3. **Finds the right file.** Asks the server for the account's home directory and which shell answers, then writes to `~/.ssh/authorized_keys` - or, for a Windows administrator, somewhere else entirely (see below). A generated key is written to disk now, with its `.pub` sibling - a key turnout makes is an ordinary key that plain `ssh` can use.
4. **Installs the key**, unless it is already authorized, and sets the permissions sshd insists on.
5. **Proves it works** by opening a second, separate connection that offers *only* the key.
6. **Switches the credential** to `auth = key` - and only if step 5 passed.

That last order matters: if the key does not sign in, the credential is left on its password, so a failed setup never costs you access. The stored password is kept either way; removing it is your call:

```bash
turnout pass remove prod-deploy
```

Running `setup` twice is safe. An already-authorized key is recognized - by its body, not its comment - and not appended again.

The key is appended on a line of its own - a file whose last line has no newline, as one saved by Notepad, gets one first - and exactly as it is: whatever the comment holds (`&`, `<`, `%`, quotes) reaches the file unchanged, on a Windows server too.

### Windows servers

OpenSSH on Windows ships an `sshd_config` with a `Match Group administrators` block, and for a member of that group it reads **only**:

```
C:\ProgramData\ssh\administrators_authorized_keys
```

A key written to that account's own `~/.ssh/authorized_keys` is ignored without a word - the sign-in just keeps asking for the password. `turnout key setup` checks whether the account is an administrator and writes to the file sshd will actually read, then fixes the file's ACL, which Windows sshd is equally strict about: the shared file is granted to the Administrators group and SYSTEM, so every administrator can add a key to it and sshd accepts it for all of them. See [Windows servers](/turnout/concepts/windows-servers/) for the rest of what changes on that side.

An `auth = agent` credential is refused: it already signs in by key, and switching it to a generated key file would trade a working login for a lesser one. To authorize an agent key on one more server, use `ssh-copy-id`, or point the credential at a key file first.

## check

```bash
turnout key check [SERVER] [--credential NAME]
```

Signs in with the credential's key and reports whether it worked. Useful after changing something on the server - it answers the question `deploy` would otherwise answer the slow way.

```bash
turnout key check prod
```

For an `auth = agent` credential this checks the [SSH agent](/turnout/concepts/ssh-agent/) route instead: the agent is asked to sign, exactly as the daily commands do.

## rotate

```bash
turnout key rotate [CREDENTIAL] [--yes]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--yes` | `-y` | Skip the confirmation prompt |

Replaces a credential's key on **every server it signs in to**: the servers that name it as their credential, and the ones a [target](/turnout/reference/target/) reaches with it. For when the laptop the key was on is gone, when someone who had it left, or when a year has passed.

```bash
turnout key rotate deploy
# ◇  The new key is authorized as deploy@prod.example.com (/home/deploy/.ssh/authorized_keys)
# ◇  The new key is authorized as deploy@staging.example.com (/home/deploy/.ssh/authorized_keys)
# ◇  The new key signs in as deploy@prod.example.com
# ◇  The new key signs in as deploy@staging.example.com
# ◇  The old key no longer signs in as deploy@prod.example.com
# ◇  The old key no longer signs in as deploy@staging.example.com
# └  'deploy' now signs in with ~/.ssh/id_ed25519_deploy_2
```

### The order

By hand, a rotation is a new key and then every server in turn - and half-way through, some stands admit only the old key and some only the new. Here the order keeps access whole at every point:

1. **The new key is authorized on every server**, signing in with the old one. A new ed25519 key is generated; no key file is ever overwritten, so it goes beside the old one under the next free name.
2. **The new key signs in to every server**, by itself, in fresh connections that offer nothing else.
3. **Only then is the credential switched** to the new key.
4. **Only then is the old key taken off the servers** - last, through the new one.

Anything that fails before step 3 is undone: the new key is withdrawn from the servers that already had it, its file is removed, and the credential never moved - every server is on the old key as before. A server that is down therefore stops the rotation before it changes anything; rotate once it is back, or take it out of the catalog first if it is gone for good.

After step 3 nothing can lock the credential out. A server that would not give up the old key's line is named, the command ends with an error, and what is left is that one line to remove.

### What it leaves

- The old key's line is removed wherever it is in `authorized_keys` - under any comment, behind any options - and every other line stays as it was. The file is replaced whole rather than rewritten in place, so a connection lost half-way leaves it as it was.
- The **old key file stays on this machine**. No server here takes it any more; delete it once nothing else - a Git host, a server outside the catalog - uses it.
- A passphrase stored for the old key is removed from the keyring: the new key has none.
- Whatever else signed in to those servers with the old key - another machine holding a copy of the file - loses access. That is what a rotation is for, and why it asks first.

A credential on a password has no key to rotate ([`key setup`](#setup) gives it one), and an `auth = agent` credential is refused: the key lives in the agent, where turnout never sees it.

## When the key is refused

If the key installs but the server still will not take it, turnout names the file it wrote to and the causes that actually produce that symptom:

- `PubkeyAuthentication no` in `sshd_config`.
- On Linux: a group-writable home or `~/.ssh`, which sshd refuses (`chmod go-w ~ ~/.ssh`).
- On Linux: `AuthorizedKeysFile` pointing somewhere other than `~/.ssh/authorized_keys`.
- On Windows: an ACL that lets someone besides the Administrators group and SYSTEM write to the file, or an account that is in the administrators group after all.

The credential stays on its password in every one of those cases, so you can fix the server and run `setup` again.

## See also

- [credential](/turnout/reference/credential/) - the entity this writes to
- [pass](/turnout/reference/pass/) - the password used for the first sign-in, and the passphrase of a protected key
- [SSH agent](/turnout/concepts/ssh-agent/) - signing in with a key the agent already holds unlocked
- [server](/turnout/reference/server/) - the entity this reads
