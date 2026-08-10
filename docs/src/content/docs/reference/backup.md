---
title: turnout backup / restore
description: Back up and restore an app's deploy directory on the server.
sidebar:
  order: 9.5
  label: turnout backup
---

```bash
turnout backup  [APP] [--server SERVER]
turnout restore [APP] [--server SERVER] [--from NAME] [--list]
```

Backups are timestamped `tar.gz` archives of the deploy directory, stored on the server next to it in `{deploy-path}.backups/`. App and server resolve exactly like in [`turnout deploy`](/turnout/reference/deploy/).

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Target server (defaults to the app's current binding) |

## backup

```bash
turnout backup myapp --server prod
```

Creates `{deploy-path}.backups/YYYYMMDD-HHMMSS.tar.gz` and prints its name. `turnout deploy --backup` does the same right before touching the remote directory.

## restore

```bash
turnout restore myapp --server prod --list          # what is available
turnout restore myapp --server prod                 # roll back to the newest
turnout restore myapp --server prod --from 20260806-120000.tar.gz
```

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Target server (defaults to the app's current binding) |
| `--from` | `-f` | Backup archive name (defaults to the newest) |
| `--list` | `-l` | List available backups and exit |

Restore empties the deploy directory, unpacks the chosen archive into it and runs the app's post-deploy command (the same one `deploy` uses) if configured - so a rollback also restarts the service.

```bash
turnout restore myapp -s prod -l    # quick: which backups exist on prod
```
