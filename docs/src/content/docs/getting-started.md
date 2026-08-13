---
title: Getting Started
description: Install turnout and run the first-time setup.
---

## Install

One line on Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.ps1 | iex
```

One line on macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.sh | sh
```

Via npm:

```bash
npm i -g turnout-cli
```

Via cargo:

```bash
cargo install turnout
```

Or download the archive for your platform from [Releases](https://github.com/lacodda/turnout/releases/latest) (Windows x86_64, Linux x86_64, macOS arm64), unpack and put `turnout` on your `PATH`.

### Installer options

Both scripts read two environment variables:

| Variable | Effect |
| --- | --- |
| `TURNOUT_VERSION` | Install this tag (e.g. `v0.8.0`) instead of the newest release |
| `TURNOUT_INSTALL_DIR` | Where the binary lands; defaults to `%LOCALAPPDATA%\Programs\turnout` on Windows and `~/.local/bin` elsewhere |

### The `tn` alias

The installers and the npm package also set up `tn` as a short second name, so `tn use web staging` is the same as `turnout use web staging`. It is skipped when something else in your `PATH` already answers to `tn`; set `TURNOUT_NO_ALIAS=1` to opt out entirely. Installing through `cargo install` gives you `turnout` only - add your own alias if you want the short form.

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

Once a day turnout also checks whether a newer release exists and mentions it at the end of a command. The lookup runs in the background, so it never delays anything; `TURNOUT_UPDATE_CHECK=0` switches it off. See [the update check](/turnout/concepts/update-check/).

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
