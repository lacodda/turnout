---
title: turnout import
description: Read a file written by turnout export, merging it into this machine's setup.
---

Reads a file written by [`turnout export`](/turnout/reference/export/).

```bash
turnout import turnout-export.json
```

```text
Imported 4 item(s).
Skipped 2 item(s) that already exist:
  app 'web'
  server 'pi'
  Re-run with --force to overwrite them.
Restored 2 secret(s) to the OS keyring.
```

| Flag | Short | Effect |
| --- | --- | --- |
| `--force` | `-f` | Overwrite entries that already exist instead of keeping them |

## Merging, not replacing

Anything whose name is new is added. Anything that already exists is **kept as it is** and listed at the end, so an import can never quietly redirect an app you are working in or point a server at a different host.

On a fresh machine that distinction never comes up - everything is new, and the whole setup lands in one command. It matters when you import onto a machine that already has its own contour, which is also what makes a repeated import harmless: run it twice and the second run changes nothing.

`--force` is the opposite instruction: incoming entries win. Use it when the file is the source of truth, for instance restoring a backup over a setup you no longer trust.

Apps, servers and groups are matched by name; access records by server **and** kind, so a password and an SSH key on the same server stay separate records.

## Secrets

A file exported with `--with-secrets` asks for its passphrase, and the secrets go into this machine's OS keyring - the same place [`turnout pass`](/turnout/reference/pass/) puts them. They are never written to disk in the clear.

Decryption happens **before** anything is written. A wrong passphrase therefore leaves the machine exactly as it was, rather than importing the catalogs and failing on the secrets - which would hand you half a setup and then greet the retry with "already exists" on everything.

A wrong passphrase and an altered file report the same error, because the authentication tag genuinely cannot tell them apart:

```text
error: cannot decrypt the secrets: wrong passphrase, or the file has been altered
```

## In scripts

With no terminal the passphrase is read from stdin:

```bash
printf '%s' "$PASSPHRASE" | turnout import backup.json
```

An export written by a newer turnout is refused rather than half-read, with the version it needs.
