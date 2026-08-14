---
title: "pass"
description: Manage the secrets credentials authenticate with, stored in the OS keyring.
sidebar:
  order: 7
---

```bash
turnout pass <set|copy|list|remove> [...]
```

A secret belongs to a [credential](/turnout/reference/credential/). The credential holds the metadata - who logs in, and whether by key or password - while the value itself lives only in the OS keyring: Windows Credential Manager, macOS Keychain or Linux Secret Service. Secrets are never written to config files and never printed unless you explicitly ask.

Every subcommand takes a credential name, or offers a [picker](/turnout/concepts/pickers/) when you omit it.

## set

```bash
turnout pass set [CREDENTIAL]
```

Interactively: type the secret twice (hidden input). For an `auth = key` credential the prompt asks for the key's passphrase instead, which is what the secret means there.

In scripts, pipe the secret via stdin so it never lands in shell history:

```bash
echo "$SECRET" | turnout pass set prod-deploy
```

Running `set` again replaces the stored value.

## copy

```bash
turnout pass copy [CREDENTIAL] [--user] [--show]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--user` | `-u` | Copy the user name instead of the secret |
| `--show` | `-s` | Print to stdout instead of copying to the clipboard |

Copies the secret to the clipboard and prints only a confirmation. `--show` prints the value to stdout instead - an explicit opt-in for terminals without a clipboard (e.g. over SSH).

```bash
turnout pass copy prod-deploy          # secret -> clipboard
turnout pass copy prod-deploy --user   # user    -> clipboard
turnout pass copy prod-deploy -s       # print the secret to stdout
```

## list

```bash
turnout pass list
```

One line per credential: name, user, auth kind, and whether a secret is stored. Never the secrets themselves.

## remove

```bash
turnout pass remove [CREDENTIAL] [--yes]
```

Deletes the secret from the keyring. **The credential itself stays** - rotating a password should not cost you the account. Use [`turnout credential remove`](/turnout/reference/credential/) to drop both.

:::caution[Test backend]
`TURNOUT_KEYRING=insecure-file` switches secrets to a plain JSON file in the data directory. It exists for tests and throwaway environments only - the value is stored unprotected. Leave the variable unset in real use.
:::
