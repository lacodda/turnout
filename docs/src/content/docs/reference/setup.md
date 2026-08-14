---
title: "setup"
description: Initialize the data directory and walk through first-run setup.
sidebar:
  order: 1
---

```bash
turnout setup [--yes]
```

Initializes turnout on this machine: shows where the data directory will be created, asks for confirmation and creates it with a `meta.json` marker (schema version for future config migrations).

Running `setup` again on an initialized machine is safe - it reports the current location and does nothing.

## On settings this build cannot read

`setup` is the one command that still works when the data directory holds an older schema, because it is what every refusal points at. There it offers to start over instead: the old catalogs move into `settings-backup-v<N>` beside them, and an empty catalog is created at the current schema.

Nothing happens without confirmation, and nothing is deleted - the retired files are what you re-enter from. The journal stays, and secrets in the OS keyring are untouched. See [Upgrading to 0.9](/turnout/guides/upgrading-to-0-9/) for the walkthrough.

A directory written by a *newer* turnout is not offered a reset: there the fix is to update turnout, and starting fresh would throw away settings that are perfectly fine.

## Options

| Option | Description |
| --- | --- |
| `-y`, `--yes` | Skip confirmation prompts and accept defaults |

## Data directory

| OS | Path |
| --- | --- |
| Windows | `%LOCALAPPDATA%\lacodda\turnout` |
| macOS | `~/Library/Application Support/lacodda/turnout` |
| Linux | `~/.local/share/lacodda/turnout` |

Override with the `TURNOUT_DATA_DIR` environment variable.
