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

Running `setup` again on an initialized machine is safe - it reports the current location, and settings written by an older turnout are brought up to date there, the way any command would ([Upgrading](/turnout/guides/upgrading/#what-updates-by-itself)).

## On settings this build cannot read

`setup` is the one command that still works on settings from turnout 0.8 or older (schema 1), which no migration reads, because it is what their refusal points at. There it offers to start over instead: the old catalogs move into `settings-backup-v<N>` beside them, and an empty catalog is created at the current schema.

Nothing happens without confirmation, and nothing is deleted - the retired files are what you re-enter from. The journal and the update check's cache stay, secrets in the OS keyring are untouched, and so are secrets kept in the file keyring (`insecure-secrets.json`) - they are in the data directory, but they are not catalogs. See [Upgrading](/turnout/guides/upgrading/#from-08-or-older) for the walkthrough.

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
