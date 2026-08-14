---
title: "backup / restore"
description: Back up and restore an app's deploy directory on the server.
sidebar:
  order: 9.5
  label: backup
---

```bash
turnout backup  [APP] [--server SERVER] [--credential NAME] [--path NAME]
turnout restore [APP] [--server SERVER] [--credential NAME] [--path NAME] [--from NAME] [--list]
```

Backups are timestamped `tar.gz` archives of the target directory, stored on the server next to it in `{dir}.backups/`. App, server, credential and path resolve exactly like in [`turnout deploy`](/turnout/reference/deploy/).

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Target server (defaults to the app's current binding) |
| `--credential` | `-C` | Credential to log in with (defaults to the server's) |
| `--path` | `-p` | Named path to back up (defaults to the server's for this app) |

## backup

```bash
turnout backup myapp --server prod
```

Creates `{dir}.backups/YYYYMMDD-HHMMSS.tar.gz` and prints its name. `turnout deploy --backup` does the same right before touching the remote directory.

The timestamp is UTC and comes from your machine, not the server's clock, so archives taken from different places still sort next to each other correctly.

:::note[Backups need access to the parent directory]
Archives are kept *beside* the target directory, so the deploy user must be able to write to its parent - `/var/www` for a path pointing at `/var/www/myapp`, and that is usually owned by root. Deploying works regardless, which makes the first failed backup look arbitrary; turnout says so and gives you the command. Creating the directory once is enough:

```bash
sudo mkdir -p /var/www/myapp.backups && sudo chown $USER /var/www/myapp.backups
```

On a Windows server the same message names the Windows command instead.
:::

## restore

```bash
turnout restore myapp --server prod --list          # what is available
turnout restore myapp --server prod                 # roll back to the newest
turnout restore myapp --server prod --from 20260806-120000.tar.gz
```

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Target server (defaults to the app's current binding) |
| `--credential` | `-C` | Credential to log in with (defaults to the server's) |
| `--path` | `-p` | Named path to restore into (defaults to the server's for this app) |
| `--from` | `-f` | Backup archive name (defaults to the newest) |
| `--list` | `-l` | List available backups and exit |

Restore empties the target directory, unpacks the chosen archive into it and runs the path's post-write command (the same one `deploy` uses) if there is one - so a rollback also restarts the service.

"Newest" is decided by sorting the archive names, which are fixed-width timestamps - not by the order the server listed them in. Files in the backups directory that turnout did not write are ignored, so a note you left there will never be restored over your site.

```bash
turnout restore myapp -s prod -l    # quick: which backups exist on prod
```
