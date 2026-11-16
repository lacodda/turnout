---
title: "dev / build / test / lint / run"
description: Run app commands from any directory.
sidebar:
  order: 8
  label: dev / run
---

```bash
turnout dev   [APP] [-v | -d | --foreground] [-o] [-w SOURCE]...
turnout build [APP] [-v | -d | --foreground]
turnout test  [APP] [-v | -d | --foreground]
turnout lint  [APP] [-v | -d | --foreground]
turnout run COMMAND [APP] [-v | -d | --foreground] [-o] [-w SOURCE]...
```

Runs the app's named command in its project directory - no `cd` required. `dev`, `build`, `test` and `lint` are shortcuts for the standard commands; `run` executes any command defined in the app config (see [`turnout app`](/turnout/reference/app/)).

## Options

| Flag | Meaning |
| --- | --- |
| `-v, --verbose` | Stream the command's output in full instead of hiding it behind a loader |
| `-d, --detach` | Run in the background and give the terminal back at once - see [In the background](#in-the-background) |
| `--foreground` | Stay in this terminal even when `TURNOUT_DETACH` sends the command to the background |
| `-o, --open` | Open the app's front door in the browser once the server is up (`dev` and `run`) |
| `-w, --with <SOURCE>` | Add variables from a sefy record for this run (`dev` and `run`, repeatable) - see [From a password manager](#from-a-password-manager---with) |

## How it behaves

- **App resolution.** Pass the app name, or omit it and let turnout find the app whose directory contains your current one - `turnout dev` from anywhere inside the project just works. Outside any known project a terminal gets a [picker](/turnout/concepts/pickers/) instead of an error.
- **The gateway address rides along.** `dev` and every custom command get the app's gateway URL as an environment variable (the name is per app - see [`turnout app`](/turnout/reference/app/#how-the-app-learns-the-gateway-address)); `build`, `test` and `lint` deliberately do not, so a production bundle never bakes in localhost. `{gateway}` and `{gateway_port}` in a command line are replaced before it runs.
- **The app's variables ride along too.** `dev` and every custom command get what [`turnout env`](/turnout/reference/env/) holds for the app on the stand it points at - see [The app's variables](#the-apps-variables).
- **`dev` owns the dev server's port.** The first `dev` assigns the app a port from `5100-5199` and keeps it; every `dev` after that hands it over as `PORT` and as `{port}` in the command line, and prints the app's address - `http://myapp.localhost` through the gateway's [front door](/turnout/reference/gateway/#the-front-door). A dev command that mentions neither is noted: a server that ignores `PORT` stays on its own port, out of the door's reach.
- **The console is quiet by default.** `build`, `test` and `lint` show a loader and their elapsed time; `dev` and `run` show one until the server reports itself, then only errors and warnings. A failure prints its output on the spot. Everything is written to a log file either way - see [the quiet console](/turnout/concepts/quiet-console/) for what is held back and where it goes. `-v` streams it all, and so does any non-terminal output, so pipes and CI logs are unchanged.
- **The dev port has to be free.** Before `dev` starts, the app's port is checked: a process turnout did not start sitting on it - a dev server a closed terminal left behind, or one started by hand - would push the new server to another port while the front door kept showing the old one. In a terminal `dev` names the process and offers to end it; elsewhere it stops and says how. [`turnout doctor`](/turnout/reference/doctor/#dev-refuses-to-start-over-a-stray) finds the same thing for every app at once.
- **One copy at a time.** A command that is already running for the app - in the background or in another terminal - is refused rather than started twice: two runs would truncate each other's log, and two dev servers of one app fight over its port. [`turnout ps`](/turnout/reference/ps/) shows what runs.
- **Exit codes pass through.** `turnout build` exits with the build's own code, so it drops into scripts and CI without surprises.
- **Ctrl+C is clean.** The interrupt goes to the tool itself; turnout waits for it to die, then puts the terminal back the way it was - echo, line input, cursor - so the prompt you get back works even when a dev server left the console raw. On Windows a second Ctrl+C force-kills the tool's whole process tree (some dev servers ignore the first); on Linux and macOS each Ctrl+C reaches the tool's whole process group as the terminal sends it. The exit code is the conventional `130`. Whatever survives the interrupt dies with turnout: on Windows every spawned command runs in a kill-on-close job object, so no stray helper processes linger.

## The app's variables

```bash
turnout env set DATABASE_URL --app myapp      # asked for, kept in the OS keyring
turnout dev myapp
# [myapp] 1 variable of 'staging' (turnout env)
# [myapp] pnpm dev --port 5100
```

What [`turnout env`](/turnout/reference/env/) holds for the app on the stand it points at is put into the environment of `dev` and of every custom command - no `.env` file, nothing in plain text on disk. The set follows the binding: after `turnout use myapp prod`, the next `dev` runs with the variables stored for `prod`. An app that points nowhere, or has nothing stored, runs as before.

`build`, `test` and `lint` get none, for the reason they get no gateway address: a variable in the process environment overrides every dotenv file, and a bundle built here must not bake in the stand this machine happens to point at. The build a [deploy](/turnout/reference/deploy/#the-apps-variables) runs knows where it goes, and gets that stand's variables.

What turnout sets itself - the gateway's address and `PORT` - wins a name over a stored variable.

### From a password manager: `--with`

Some secrets are not the app's to keep: a personal token, a key shared across projects. They live in a password manager already, and `--with` takes them from [sefy](https://github.com/lacodda/sefy) for the one run - nothing is copied into turnout's keyring.

| Source | What arrives |
| --- | --- |
| `sefy:NAME` | Every variable of a record that holds `NAME=value` lines - a note kept as a dotenv file |
| `VAR=sefy:NAME` | The record's own secret - a login's password, a token - as `VAR` |
| `VAR=sefy:NAME#FIELD` | Another field of the record as `VAR`: `DB_USER=sefy:db#login` |

```bash
turnout dev myapp --with sefy:myapp-dev
turnout run migrate myapp -w DATABASE_URL=sefy:staging-db -w DB_USER=sefy:staging-db#login
```

`NAME` is what `sefy get` takes: an item's id, its exact title, or text to search for. turnout runs `sefy get NAME --stdout` and reads the answer; sefy unlocks its vault the way it always does - through its agent, or by asking for the master password in this terminal - and knows nothing of turnout. A variable from `--with` wins over a stored one of the same name, and a later `--with` over an earlier.

sefy has to be on the `PATH`; `TURNOUT_SEFY` names the program when it is somewhere else.

## In the background

`--detach` hands the command to a background [supervisor](/turnout/concepts/background-jobs/#the-supervisor) and returns: the dev server keeps running, the terminal is yours again.

```bash
turnout dev myapp --detach
# [myapp] pnpm dev --port 5100
# [myapp] http://myapp.localhost -> dev server port 5100
# myapp dev runs in the background (pid 31412)
#   output: turnout logs myapp dev -f
#   stop:   turnout stop myapp dev
```

It watches for most of a second first, so a command that fails at once - a typo, a missing `node_modules` - comes back with its output and its exit code instead of a "runs in the background". `--open` still opens the page once the server is up, and a [notification](/turnout/concepts/notifications/) says so too - or says the build finished, or failed and why.

To make the background the default for some commands, name them in `TURNOUT_DETACH` (`build,test`, or `all`); `--foreground` keeps a single run here. See [always in the background](/turnout/concepts/background-jobs/#always-in-the-background). What runs is in [`turnout ps`](/turnout/reference/ps/), its output in [`turnout logs`](/turnout/reference/logs/), and [`turnout stop`](/turnout/reference/stop/) ends it with everything it started. `-d` and `-v` do not go together: a background job has no console to stream to - `logs -f` is its stream.

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
