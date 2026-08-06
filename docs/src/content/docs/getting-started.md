---
title: Getting Started
description: Install turnout and run the first-time setup.
---

## Install

From source, for now:

```bash
git clone https://github.com/lacodda/turnout.git
cd turnout
cargo install --path .
```

The binary lands in `~/.cargo/bin`, which should be on your `PATH`. Binary releases for Windows, macOS and Linux will come with the first tagged version.

## First run

```bash
turnout setup
```

The wizard shows where turnout will keep its data and creates the directory:

| OS | Data directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\lacodda\turnout` |
| macOS | `~/Library/Application Support/lacodda/turnout` |
| Linux | `~/.local/share/lacodda/turnout` |

Set the `TURNOUT_DATA_DIR` environment variable to override the location (useful for tests and scripting). Pass `--yes` to skip prompts.

## Check the state

```bash
turnout status
```

Shows the data directory, configured apps and servers, and whether the gateway is running.

## Describe your first app and server

```bash
cd ~/dev/myapp
turnout app add        # wizard: detects the project type, proposes commands
turnout server add     # wizard: URL, SSH, TLS policy
```

See [`turnout app`](/turnout/reference/app/) and [`turnout server`](/turnout/reference/server/) for the full command reference.

## A working day

```bash
turnout gateway start          # once: the gateway routes apps to their stands
turnout use myapp staging      # bind the app to a stand
turnout dev                    # from the project dir: UI local, API on staging
turnout pass copy staging      # password on the clipboard when the stand asks
turnout use myapp prod-eu      # switch stands - no restarts, no env edits
```

## Next steps

- Learn the [entity model](/turnout/concepts/entities/) - apps, servers, credentials and state.
- See how the [dev gateway](/turnout/concepts/gateway/) routes your apps to stands.
