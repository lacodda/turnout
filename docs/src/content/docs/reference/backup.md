---
title: "backup / restore"
description: Back up, prune and restore an app's deploy directory on the server.
sidebar:
  order: 9.5
  label: backup
---

```bash
turnout backup  [TARGET] [--server SERVER] [--credential NAME] [--path NAME]
turnout backup prune [TARGET] [--keep N] [--server SERVER] [--credential NAME] [--path NAME] [--json]
turnout restore [TARGET] [--server SERVER] [--credential NAME] [--path NAME] [--from NAME] [--list [--json]]
```

Backups are timestamped `tar.gz` archives of the target directory, stored on the server next to it in `{dir}.backups/`. `TARGET` is a [target](/turnout/reference/target/) name or an app name, and resolves - along with server, credential and path - exactly like in [`turnout deploy`](/turnout/reference/deploy/).

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Server for this run only, overriding the target's |
| `--credential` | `-C` | Credential for this run only, overriding the target's |
| `--path` | `-p` | Named path to back up, overriding the target's |

## backup

```bash
turnout backup myapp-prod
```

Creates `{dir}.backups/YYYYMMDD-HHMMSS.tar.gz` and prints its name. `turnout deploy --backup` does the same right before touching the remote directory. Either way, the older backups beyond what the stand keeps are removed afterwards - see [How many are kept](#how-many-are-kept).

The timestamp is UTC and comes from your machine, not the server's clock, so archives taken from different places still sort next to each other correctly.

:::note[Backups need access to the parent directory]
Archives are kept *beside* the target directory, so the deploy user must be able to write to its parent - `/var/www` for a path pointing at `/var/www/myapp`, and that is usually owned by root. Deploying works regardless, which makes the first failed backup look arbitrary; turnout says so and gives you the command. Creating the directory once is enough:

```bash
sudo mkdir -p /var/www/myapp.backups && sudo chown $USER /var/www/myapp.backups
```

On a Windows server the same message names the Windows command instead.
:::

## How many are kept

A backup is the whole deploy directory, and a month of daily `deploy --backup` fills a small disk. So every command that leaves a backup on the server - `backup`, `deploy --backup`, and a [checked deploy](/turnout/reference/deploy/#health-check-and-rollback) rolled back to the backup it took - prunes the stand's backups to the **newest 5** afterwards and names what it removed:

```
◇ Backup 20261016-141000.tar.gz created in /var/www/myapp.backups
│  Removed the old backup 20261009-090000.tar.gz - the stand keeps the newest 5 backups (`turnout target edit myapp-prod --keep-backups N` changes it)
```

The number belongs to the [target](/turnout/reference/target/#backups): a backup's size and the disk it fills are the stand's, so a Pi and a VPS deploying the same path keep different counts.

```bash
turnout target edit myapp-pi --keep-backups 3
```

Only turnout's own archives - the `YYYYMMDD-HHMMSS.tar.gz` names - are counted and removed; anything else in the directory is yours and stays. A pruned name is checked against a second listing, so a removal the server did not make is never reported as made. Without a target behind the run (an app deployed through its binding, or a run with `--server` or `--path`), the stand keeps 5.

## backup prune

```bash
turnout backup prune myapp-prod             # down to what the target keeps
turnout backup prune myapp-prod --keep 2
turnout backup prune myapp-prod --keep 0    # remove them all
```

| Flag | Short | Description |
| --- | --- | --- |
| `--keep` | `-k` | How many of the newest to keep (default: the target's `--keep-backups`, else 5); `0` removes them all |
| `--server` | `-s` | Server for this run only, overriding the target's |
| `--credential` | `-C` | Credential for this run only, overriding the target's |
| `--path` | `-p` | Named path whose backups to prune, overriding the target's |
| `--json` | | Print `{"dir", "keep", "removed", "kept"}` instead - see [JSON output](/turnout/concepts/json-output/) |

The same pruning, on demand: for backups piled up before v0.25, or to free a disk now. A target that happens to be called `prune` is still reachable as `turnout backup -- prune`.

## restore

```bash
turnout restore myapp-prod --list          # what is available
turnout restore myapp-prod                 # roll back to the newest
turnout restore myapp-prod --from 20260806-120000.tar.gz
```

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Server for this run only, overriding the target's |
| `--credential` | `-C` | Credential for this run only, overriding the target's |
| `--path` | `-p` | Named path to restore into, overriding the target's |
| `--from` | `-f` | Backup archive name (defaults to the newest) |
| `--list` | `-l` | List available backups, newest first, and exit |
| `--json` | | With `--list`: print `{"dir": ..., "backups": [...]}` instead - see [JSON output](/turnout/concepts/json-output/) |

Restore empties the target directory, unpacks the chosen archive into it and runs the path's post-write command (the same one `deploy` uses) if there is one - so the service restarts on what was put back. The command runs the way a deploy runs it: in the deploy directory, and [detached from the connection](/turnout/reference/deploy/#a-restart-that-outlives-the-connection) on a Linux or macOS server. Before v0.24 a restore ran it in the home directory.

A deploy to a target with a [health check](/turnout/reference/target/#health-check) does this by itself when the check fails - see [Health check and rollback](/turnout/reference/deploy/#health-check-and-rollback).

`restore` puts back the **whole directory** as the backup caught it - uploads and data files in it included. To go back to an earlier **build** and leave the data as it is, use [`turnout rollback`](/turnout/reference/rollback/), which needs no backup on the server.

"Newest" is decided by sorting the archive names, which are fixed-width timestamps - not by the order the server listed them in. Files in the backups directory that turnout did not write are ignored, so a note you left there will never be restored over your site.

```bash
turnout restore myapp-prod -l    # quick: which backups exist on prod
```
