---
title: Getting Started
description: Install turnout and run the first-time setup.
---

## Install

One line:

```powershell
# Windows (PowerShell)
irm https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.ps1 | iex
```

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.sh | sh
```

Other ways:

```bash
npm i -g turnout-cli     # via npm
cargo install turnout    # via cargo
```

Or download the archive for your platform from [Releases](https://github.com/lacodda/turnout/releases/latest) (Windows x86_64, Linux x86_64, macOS arm64), unpack and put `turnout` on your `PATH`.

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
