---
title: "env"
description: Keep the variables an app runs with on a stand - in the OS keyring, handed to dev, run and a deploy.
sidebar:
  order: 7.1
---

```bash
turnout env <set|list|remove|import> [...] [--app APP] [--server SERVER]
```

An app's `.env` is where its secrets end up: a database password, an API token, in plain text, one copy per stand, renamed back and forth by hand. `turnout env` keeps them per **app and server** instead. The values live in the OS keyring, beside the secrets of the [credentials](/turnout/reference/pass/); the catalog holds only their names.

One set, three roads:

- [`turnout dev` and `turnout run`](/turnout/reference/run/#the-apps-variables) put the set of the stand the app points at into the command's environment. Switch the stand with [`turnout use`](/turnout/reference/use/) and the next `dev` runs with the other set.
- A [deploy](/turnout/reference/deploy/#the-apps-variables) builds with the set of the stand it goes to.
- A [target](/turnout/reference/target/#env-file) that names an env file has it written on the stand, from the same set, on every deploy.

Every subcommand names the pair the same way:

| Flag | Short | Meaning |
| --- | --- | --- |
| `--app` | `-a` | The app; the app of the current directory when omitted |
| `--server` | `-s` | The stand; the one the app points at when omitted |

## set

```bash
turnout env set NAME[=VALUE]... [--app APP] [--server SERVER]
```

`NAME=VALUE` stores what is typed. `NAME` alone asks for the value without showing it, so a secret stays out of the shell history; off a terminal the value is read from standard input, one name at a time.

```bash
turnout env set LOG_LEVEL=debug FEATURE_X=1
# Stored LOG_LEVEL, FEATURE_X for 'myapp' on 'staging', in the OS keyring.

turnout env set DATABASE_URL                       # asked for, not echoed
echo "$TOKEN" | turnout env set API_TOKEN --server prod
```

A name already there gets the new value. Names are what a shell takes: letters, digits and underscores, not starting with a digit. The variable turnout hands the app the [gateway's address](/turnout/reference/app/#how-the-app-learns-the-gateway-address) in is refused: it is this machine's, and turnout sets it on every run.

## list

```bash
turnout env list [--app APP] [--server SERVER] [--show | --json]
```

| Flag | Meaning |
| --- | --- |
| `--show` | Print the variables of one app on one stand with their values, as dotenv lines |
| `--json` | Print the sets as JSON: `app`, `server`, `current` and the `variables` by name |

The sets by name, never by value - of the app named or found, or of every app when there is none around:

```bash
turnout env list
# myapp on prod (2 variables)
#   API_TOKEN
#   DATABASE_URL
# myapp on staging (3 variables) - where it points now
#   DATABASE_URL
#   FEATURE_X
#   LOG_LEVEL
```

`--show` is the explicit way to see values: one set, printed the way a dotenv file holds it.

```bash
turnout env list --show --server staging
# DATABASE_URL=postgres://app:s3cret@db.internal/app
# FEATURE_X=1
# LOG_LEVEL=debug
```

## remove

```bash
turnout env remove NAME... [--yes]
turnout env remove --all [--yes]
```

| Flag | Short | Meaning |
| --- | --- | --- |
| `--all` | | Remove every variable the app has on the stand |
| `--yes` | `-y` | Skip the confirmation prompt |

Takes the values out of the keyring and the names out of the catalog. Removing an [app](/turnout/reference/app/) or a [server](/turnout/reference/server/) takes its variables with it.

## import

```bash
turnout env import [FILE] [--app APP] [--server SERVER]
```

Stores every variable of a dotenv file - the app's own `.env` when no file is named - in one command:

```bash
turnout env import
# Stored DATABASE_URL, API_TOKEN, LOG_LEVEL for 'myapp' on 'staging', in the OS keyring.

turnout env import .env.production --server prod
```

The file is read the way its readers read it: `NAME=value` lines, `#` comments, an optional `export`, values bare, in single quotes (taken literally) or in double quotes (`\n`, `\"`, `\\` and `\$` escaped), a quoted value over several lines. A name already stored gets the file's value. The line turnout itself keeps in the app's dotenv file - the gateway's address - is passed over.

The file is left as it is, and still holds the values in plain text. Once the app runs through `turnout dev`, it no longer needs them there.

## What is stored where

- **Values** - in the OS keyring: Windows Credential Manager, macOS Keychain or Linux Secret Service. A long value is stored in pieces, since Windows holds 2560 bytes per entry; `env list --show` and every run read it back whole.
- **Names** - in `env.json` in the data directory, so `env list` asks the keyring for nothing.
- **In an export** - the names always; the values only with [`--with-secrets`](/turnout/reference/export/#taking-secrets-along), sealed with the passphrase like every other secret.

A name whose value the keyring no longer holds stops the command that needs it, and says which one to store again.

## See also

- [`turnout dev` / `run`](/turnout/reference/run/#the-apps-variables) - where the variables are handed over, and `--with` for the ones a password manager keeps
- [`turnout target`](/turnout/reference/target/#env-file) - the file a deploy writes them into on the stand
- [`turnout pass`](/turnout/reference/pass/) - the secrets credentials sign in with
