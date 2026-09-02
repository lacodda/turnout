---
title: "credential"
description: Manage credentials - who logs in, and with a password, a key file or the SSH agent.
sidebar:
  order: 5
---

```bash
turnout credential <add|list|show|edit|remove> [...]
turnout cred <...>        # same thing, shorter
```

A **credential** is a way to log in: a remote user, and how it proves itself - with a password, a private key file, or a key held by a running SSH agent. The secret itself never lives here - it goes into the OS keyring under the credential's name, managed with [`turnout pass`](/turnout/reference/pass/). An `agent` credential stores nothing at all: the agent holds the key, and turnout only asks it to sign.

Credentials are free-standing. One deploy account usually reaches several stands, so it is defined once and every [server](/turnout/reference/server/) that accepts it points at the same name.

## add

```bash
turnout credential add [NAME] [--user USER] [--auth password|key|agent] [--key PATH]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--user` | `-u` | Remote user this logs in as |
| `--auth` | `-a` | How it authenticates: `password`, `key` or `agent` |
| `--key` | `-K` | Private key file (implies `--auth key`) |

Passing `--key` is enough to mean key authentication; `--auth` is only needed to be explicit, to switch back, or to choose `agent` - which has no key file to infer it from.

```bash
turnout credential add                                     # wizard
turnout credential add prod-deploy --user deploy           # password auth
turnout credential add pi --user pi --key ~/.ssh/id_ed25519
turnout credential add work --user deploy --auth agent     # the SSH agent signs
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
turnout credential edit pi --auth agent                    # let the agent sign
```

| Flag | Short | Description |
| --- | --- | --- |
| `--user` | `-u` | Remote user |
| `--auth` | `-a` | `password`, `key` or `agent` |
| `--key` | `-K` | Private key file (empty value removes it) |

Key authentication without a key file is refused at edit time rather than at connect time.

## remove

```bash
turnout credential remove prod-deploy [--yes]
```

Removes the credential **and** its stored secret. Servers that pointed at it are listed before the confirmation and cleared afterwards, so none is left naming something that is gone.

## How authentication is attempted

For an `auth = key` credential, the key file is used, with the stored secret - if there is one - as its passphrase. An unprotected key needs no secret at all.

For an `auth = password` credential, the password stored in the OS keyring is sent to the server.

For an `auth = agent` credential, turnout asks the running [SSH agent](/turnout/concepts/ssh-agent/) to sign. It offers the agent's keys in turn and stops at the first one the server accepts, so an agent holding several keys is normal. Nothing is read from disk and nothing is stored in the keyring.
