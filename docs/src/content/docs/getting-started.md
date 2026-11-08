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

:::caution[On Windows, use the PowerShell line]
`install.sh` carries the macOS and Linux builds only. Running it from Git Bash,
MSYS2 or Cygwin stops with a pointer to `install.ps1` rather than installing
anything.
:::

Via npm:

```bash
npm i -g turnout-cli
```

Via cargo:

```bash
cargo install turnout
```

Or download the archive for your platform from [Releases](https://github.com/lacodda/turnout/releases/latest) (Windows x86_64, Linux x86_64 and arm64, macOS arm64), unpack and put `turnout` on your `PATH`.

### Installer options

Both scripts read these environment variables:

| Variable | Effect |
| --- | --- |
| `TURNOUT_VERSION` | Install this tag (e.g. `v0.27.0`) instead of the newest release |
| `TURNOUT_INSTALL_DIR` | Where the binary lands; defaults to `%LOCALAPPDATA%\Programs\turnout` on Windows and `~/.local/bin` elsewhere |
| `TURNOUT_NO_PATH` | Leave the `PATH` alone: for an install into a scratch directory that should not end up on it |
| `TURNOUT_NO_ALIAS` | Do not make the `tn` alias |

An update over a turnout that is running - a gateway, a dev server - goes through: the running binary steps aside and keeps working, and the next start is the new one.

### The `tn` alias

The installers and the npm package also set up `tn` as a short second name, so `tn use web staging` is the same as `turnout use web staging`. It is skipped when something else in your `PATH` already answers to `tn`; set `TURNOUT_NO_ALIAS=1` to opt out entirely. Installing through `cargo install` gives you `turnout` only - add your own alias if you want the short form.

The alias is a link to the binary, not a second copy of it: a symlink on macOS and Linux, a hard link on Windows (a symlink there would need elevation, which an installer has no business asking for). One binary on disk answers to both names, so an install costs half of what two copies would and the two names can never drift apart - including across [`turnout self-update`](/turnout/reference/self-update/), which points the alias back at the new binary as its last step.

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

Besides the catalogs, the directory holds the [action journal](/turnout/concepts/journal/) and a `logs/` folder with the output of every command turnout has run for an app - see [the quiet console](/turnout/concepts/quiet-console/).

Once a day turnout also checks whether a newer release exists and mentions it at the end of a command. The lookup runs in the background, so it never delays anything; `TURNOUT_UPDATE_CHECK=0` switches it off. See [the update check](/turnout/concepts/update-check/).

## Check the state

```bash
turnout status
```

Shows the data directory, your apps and the stand each one points at, whether the gateway is running, the catalogs of servers, credentials, paths and targets, and the last few things turnout did. On a fresh setup every section says how to fill it.

## Describe your first app and server

```bash
cd ~/dev/myapp
turnout app add        # wizard: every field of the app, commands included
turnout server add     # wizard: URL, SSH host, credential, TLS policy
```

`app add` gives the app a gateway port of its own and writes the gateway's address into the project's `.env.development.local`, under the variable you chose in the wizard (`VITE_API_URL` for a Vite app, `TURNOUT_GATEWAY_URL` otherwise). Point your API client at that variable once - it is the only change your app needs. [The Vite workflow](/turnout/guides/vite-dev-workflow/) shows it end to end.

That is enough for the dev gateway. Deploying also needs to know who logs in and where files land:

```bash
turnout credential add prod-deploy --user deploy
turnout pass set prod-deploy        # the password, into the OS keyring
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"
turnout server edit prod --credential prod-deploy
turnout target add --app myapp --server prod --credential prod-deploy --path wwwroot
```

A key instead of a password: [`turnout key setup prod`](/turnout/reference/key/) makes one, puts it on the server and switches its credential over, so the password is needed once.

Or let one wizard walk all of it, offering to create the credential and path as it goes:

```bash
turnout deploy-setup myapp
```

Either way, `turnout deploy myapp-prod` deploys from any directory afterwards. See [`turnout app`](/turnout/reference/app/), [`turnout server`](/turnout/reference/server/), [`turnout credential`](/turnout/reference/credential/), [`turnout path`](/turnout/reference/path/) and [`turnout target`](/turnout/reference/target/) for the full command reference.

## A working day

```bash
turnout gateway start          # once: the gateway routes apps to their stands
turnout use myapp staging      # bind the app to a stand
turnout dev                    # from the project dir: UI local, API on staging
turnout open myapp             # the app at http://myapp.localhost
turnout use myapp prod-eu      # switch stands - no restarts, no env edits
turnout status                 # where everything points, what is running
```

`dev` keeps the console quiet until the server is up, then shows only what looks like a problem; `-v` streams everything, and `turnout logs myapp dev` has the whole output. With `--detach` it runs in the background - [`turnout ps`](/turnout/reference/ps/) lists it and [`turnout stop`](/turnout/reference/stop/) ends it. When something does not work, [`turnout doctor`](/turnout/reference/doctor/) says why.

## Next steps

- Learn the [entity model](/turnout/concepts/entities/) - apps, servers, credentials, paths, targets and state.
- See how the [dev gateway](/turnout/concepts/gateway/) routes your apps to stands.
- Updating from an older turnout - and coming from 0.8 in particular? Read [Upgrading](/turnout/guides/upgrading/).
