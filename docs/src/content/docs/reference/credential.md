---
title: "credential"
description: Manage credentials - who logs in, and with a key or a password.
sidebar:
  order: 5
---

```bash
turnout credential <add|list|show|edit|remove> [...]
turnout cred <...>        # same thing, shorter
```

A **credential** is a way to log in: a remote user, and whether it authenticates with a password or a private key file. The secret itself never lives here - it goes into the OS keyring under the credential's name, managed with [`turnout pass`](/turnout/reference/pass/).

Credentials are free-standing. One deploy account usually reaches several stands, so it is defined once and every [server](/turnout/reference/server/) that accepts it points at the same name.

## add

```bash
turnout credential add [NAME] [--user USER] [--auth password|key] [--key PATH]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--user` | `-u` | Remote user this logs in as |
| `--auth` | `-a` | How it authenticates: `password` or `key` |
| `--key` | `-K` | Private key file (implies `--auth key`) |

Passing `--key` is enough to mean key authentication; `--auth` is only needed to be explicit or to switch back.

```bash
turnout credential add                                     # wizard
turnout credential add prod-deploy --user deploy           # password or agent
turnout credential add pi --user pi --key ~/.ssh/id_ed25519
```

With `NAME` or `--user` missing, an interactive wizard asks for each field.

## list / show

```bash
turnout credential list          # one line per credential: name, user, auth kind
turnout credential show prod-deploy
```

`show` prints the user, the auth kind, the key file if there is one, whether a secret is stored, and which servers use it. It never prints the secret - use [`turnout pass copy`](/turnout/reference/pass/) for that.

Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/).

## edit

```bash
turnout credential edit prod-deploy                        # interactive wizard
turnout credential edit prod-deploy --user deployer
turnout credential edit pi --key ~/.ssh/id_ed25519_new
turnout credential edit pi --auth password --key ""        # back to a password
```

| Flag | Short | Description |
| --- | --- | --- |
| `--user` | `-u` | Remote user |
| `--auth` | `-a` | `password` or `key` |
| `--key` | `-K` | Private key file (empty value removes it) |

Key authentication without a key file is refused at edit time rather than at connect time.

## remove

```bash
turnout credential remove prod-deploy [--yes]
```

Removes the credential **and** its stored secret. Servers that pointed at it are listed before the confirmation and cleared afterwards, so none is left naming something that is gone.

## How authentication is attempted

For an `auth = key` credential, the key file is used, with the stored secret - if there is one - as its passphrase. An unprotected key needs no secret at all.

For an `auth = password` credential, the SSH agent (ssh-agent, Pageant) is tried first, and the stored secret only if the agent has nothing to offer. That order means an agent-managed key keeps working without anything in the keyring.
