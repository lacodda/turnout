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
