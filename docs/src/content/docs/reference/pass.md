---
title: "pass"
description: Manage access to servers - logins and secrets in the OS keyring.
sidebar:
  order: 5
---

```bash
turnout pass <set|copy|show|list|remove> [...]
```

Access to a server is a **credential**: a login plus a secret. The login and the secret's kind are metadata (`credentials.json`); the secret value itself lives only in the OS keyring - Windows Credential Manager, macOS Keychain or Linux Secret Service. Secrets are never written to config files and never printed unless you explicitly ask.

A server can hold several credentials distinguished by `--kind` (`set` defaults to `password`; use e.g. `token` or `ssh` for others).

`copy`, `show` and `remove` take the server and the kind from a [picker](/turnout/concepts/pickers/) when you omit them; when a server holds exactly one kind it is used without asking.

## set

```bash
turnout pass set [SERVER] [--kind KIND] [--login LOGIN]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--kind` | `-k` | What this access is: password, token, ssh, ... |
| `--login` | `-l` | Login for this credential |

Interactively: pick the server from the catalog, enter the login and type the secret twice (hidden input). In scripts, pass the server and `--login` and pipe the secret via stdin so it never lands in shell history:

```bash
echo "$SECRET" | turnout pass set staging --login deploy
```

Running `set` again for the same server and kind updates both login and secret.

## copy

```bash
turnout pass copy [SERVER] [--kind KIND] [--login] [--show]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--kind` | `-k` | Access kind; when omitted the picker offers every stored kind |
| `--login` | `-l` | Copy the login instead of the secret |
| `--show` | `-s` | Print to stdout instead of copying to the clipboard |

Copies the secret to the clipboard and prints only a confirmation. `--login` copies the login instead. `--show` prints the value to stdout instead of copying - an explicit opt-in for terminals without a clipboard (e.g. over SSH).

```bash
turnout pass copy staging            # secret -> clipboard
turnout pass copy staging --login    # login  -> clipboard
turnout pass copy staging -k token -s  # quick: print a token to stdout
```

## show / list

```bash
turnout pass show [SERVER]  # kinds and logins for one server - never secrets
turnout pass list           # all stored access metadata
```

## remove

```bash
turnout pass remove [SERVER] [--kind KIND] [--yes]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--kind` | `-k` | Access kind; when omitted the picker offers every stored kind |

Deletes the secret from the keyring and the metadata from the catalog. `turnout server remove` does this automatically for all of the server's credentials.

:::caution[Test backend]
`TURNOUT_KEYRING=insecure-file` switches secrets to a plain JSON file in the data directory. It exists for tests and throwaway environments only - the value is stored unprotected. Leave the variable unset in real use.
:::
