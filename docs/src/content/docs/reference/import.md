---
title: "import"
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
  server 'staging'
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

Every entity is matched by its own name - apps, servers, credentials, paths, targets and groups alike.

## What is refused

An import is the one way into the catalog that does not go through `add`, so everything that arrives is held to what `add` would hold it to - `--force` included. An entry that fails is left out, named with the reason, and the rest of the file still lands:

- a name that is not one (lowercase letters, digits and dashes);
- an app whose name a group here already has, or a group whose name an app has - [`use`](/turnout/reference/use/) takes either, and could only ever reach one of them;
- an app whose dotenv file is not a bare file name in the project, or whose variable is not a variable name;
- a server whose URL is not `http://` or `https://`;
- a path whose directory is not an absolute directory on a server;
- a group with a member that is not an app here, and a target naming an app, server, credential or path that is neither here nor imported - including one refused above.

```text
Imported 6 item(s).
Refused 1 item(s) this catalog cannot hold:
  app 'web': a group named 'web' exists here - app names must not clash with group names
```

## Ports

An import is the one way into the catalog that does not go through [`app add`](/turnout/reference/app/), so it holds an arriving app to the same rule: every app has a [gateway port](/turnout/reference/app/#ports-turnout-hands-out) of its own.

- An app that **arrives without a gateway port** - from an export written before v0.18.0, when the port was a question you could leave empty - is given one from 7100-7199.
- An app that **arrives on a port an app here already holds** - the other machine handed out the same number - moves to a free one. The app already here keeps its number, because its dotenv file carries it.
- A **dev port** an app here already uses is dropped from the arriving app; its next `turnout dev` picks a free one.

The import names each app it moved:

```text
Imported 9 item(s).
  'web' moved to gateway port 7101 - 7150 is 'docs''s on this machine
```

## Secrets

A file exported with `--with-secrets` asks for its passphrase, and the secrets go into this machine's OS keyring - the same place [`turnout pass`](/turnout/reference/pass/) puts them. They are never written to disk in the clear.

A secret follows its credential. When a credential of the same name already exists here and is kept, so is its secret - the file's is not written over it, and the import says so:

```text
Kept this machine's secret of 'deploy' - the credential here was kept, so the file's secret was not written.
  Re-run with --force to take the file's credential and its secret.
```

With `--force` the file's credential and its secret both replace the ones here.

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

An export written by a newer turnout is refused rather than half-read, with the version it needs. Format 4 (v0.24.0) is why: its targets can carry a [health check](/turnout/reference/target/#health-check), and an older turnout importing one would drop the check and deploy unchecked. Format 5 (v0.25.0) for the same reason: a target can say how many [backups its stand keeps](/turnout/reference/target/#backups), and an older turnout would drop the number and prune to its default.

The [deploy history](/turnout/reference/history/) does not travel: it is a record of what this machine did, not configuration.

## Files from older versions

An export written by any release since v0.11.0 imports into this one with nothing lost. That is not a promise kept by reading the code: the repository holds the file each of those releases wrote, made by its own binary, and every build imports all of them and checks that each entity and each field comes back as it left.

## Files from turnout 0.8

Exports moved to format 2 in v0.9.0, when logins and remote directories became [credentials](/turnout/reference/credential/) and [paths](/turnout/reference/path/). A file from 0.8 or earlier holds servers with a `user@host` inside them and no separate entities to put that in, so it is refused rather than imported into a catalog of servers nobody can log into. It is still plain JSON and still readable - use it as your reference while re-entering, as described in [Upgrading](/turnout/guides/upgrading/#from-08-or-older).
