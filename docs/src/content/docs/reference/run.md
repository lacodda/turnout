---
title: "dev / build / test / lint / run"
description: Run app commands from any directory.
sidebar:
  order: 8
  label: dev / run
---

```bash
turnout dev   [APP] [-v] [-o]
turnout build [APP] [-v]
turnout test  [APP] [-v]
turnout lint  [APP] [-v]
turnout run COMMAND [APP] [-v] [-o]
```

Runs the app's named command in its project directory - no `cd` required. `dev`, `build`, `test` and `lint` are shortcuts for the standard commands; `run` executes any command defined in the app config (see [`turnout app`](/turnout/reference/app/)).

## Options

| Flag | Meaning |
| --- | --- |
| `-v, --verbose` | Stream the command's output in full instead of hiding it behind a loader |
| `-o, --open` | Open the app's front door in the browser once the server is up (`dev` and `run`) |

## How it behaves

- **App resolution.** Pass the app name, or omit it and let turnout find the app whose directory contains your current one - `turnout dev` from anywhere inside the project just works. Outside any known project a terminal gets a [picker](/turnout/concepts/pickers/) instead of an error.
- **The gateway address rides along.** `dev` and every custom command get the app's gateway URL as an environment variable (the name is per app - see [`turnout app`](/turnout/reference/app/#how-the-app-learns-the-gateway-address)); `build`, `test` and `lint` deliberately do not, so a production bundle never bakes in localhost. `{gateway}` and `{gateway_port}` in a command line are replaced before it runs.
- **`dev` owns the dev server's port.** The first `dev` assigns the app a port from `5100-5199` and keeps it; every `dev` after that hands it over as `PORT` and as `{port}` in the command line, and prints the app's address - `http://myapp.localhost` through the gateway's [front door](/turnout/reference/gateway/#the-front-door). A dev command that mentions neither is noted: a server that ignores `PORT` stays on its own port, out of the door's reach.
- **The console is quiet by default.** `build`, `test` and `lint` show a loader and their elapsed time; `dev` and `run` show one until the server reports itself, then only errors and warnings. A failure prints its output on the spot. Everything is written to a log file either way - see [the quiet console](/turnout/concepts/quiet-console/) for what is held back and where it goes. `-v` streams it all, and so does any non-terminal output, so pipes and CI logs are unchanged.
- **Exit codes pass through.** `turnout build` exits with the build's own code, so it drops into scripts and CI without surprises.
- **Ctrl+C is clean.** The interrupt goes to the tool itself; turnout waits for it to die, then puts the terminal back the way it was - echo, line input, cursor - so the prompt you get back works even when a dev server left the console raw. A second Ctrl+C force-kills the tool's whole process tree (some dev servers ignore the first). The exit code is the conventional `130`. Whatever survives the interrupt dies with turnout: on Windows every spawned command runs in a kill-on-close job object, so no stray helper processes linger.

## Opening the page

`--open` waits for the server to come up and then opens the app's front door - the same address [`turnout open`](/turnout/reference/open/) uses, so the page arrives through the gateway at the app's own name rather than at whichever port the dev server grabbed. It needs the gateway running; without it, turnout says so instead of guessing an address.

It works with `-v` too: verbosity decides what the console shows, not whether the browser opens.

```bash
cd ~/dev/myapp/src/components
turnout dev                 # runs myapp's dev command in ~/dev/myapp

turnout build myapp         # from anywhere
turnout run deploy myapp    # custom command from the app config

turnout dev myapp --open
# [myapp] pnpm dev --port 5100
# [myapp] http://myapp.localhost -> dev server port 5100
# ◇  myapp ready in 1.4s - http://myapp.localhost
```
