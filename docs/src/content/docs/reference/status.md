---
title: "status"
description: Show what turnout knows - where every app points, the gateway and what it noticed, running jobs, the catalogs.
sidebar:
  order: 2
---

```bash
turnout status [--short | --json]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--short` | `-s` | One line for a shell prompt or a status bar |
| `--json` | | The same view as JSON, for scripts - see [JSON output](/turnout/concepts/json-output/) |

One command that answers "what is going on", in sections:

```text
turnout 0.24.1  C:\Users\me\AppData\Local\lacodda\turnout

Apps
  admin  -> dev      http://admin.localhost  spare :7101 (VITE_API_URL)
  api       unbound  http://api.localhost    spare :7102 (TURNOUT_GATEWAY_URL)
  web    -> prod     http://web.localhost    spare :7100 (VITE_API_URL)

Groups
  contour  web -> prod, admin -> dev

Gateway  running · pid 24180 · up 2h 05m
  front door  http://localhost
  ! 'dev' answers 502 - the stand is down, not the app (3m)
  ! the 'prod' session of 'web' expired - log in again, or hand it a cookie with `turnout cookie set web` (12m)

Jobs  2 running - `turnout ps` shows the rest
  web dev      ready        14m  http://web.localhost
  admin build  running      31s

Catalog
  servers      dev, prod
  credentials  deploy
  paths        wwwroot
  targets      web-prod

Recent
   5m ago  use            web -> prod
  42m ago  deploy         admin -> dev (142 files)
```

- **Apps** - every app, the server it is bound to, and the address to open it at: the [front door](/turnout/reference/gateway/#the-front-door) while the gateway runs, the app's own gateway port otherwise. The spare port is the way in when the door is shut, with the variable that carries it to the app.
- **Groups** - each [group](/turnout/reference/group/) and where its members point.
- **Gateway** - whether it runs, its door, and the [signals](/turnout/concepts/gateway/#when-a-stand-misbehaves) it has up: a stand that answers 502 or nothing, a session the stand refused. A signal stays until the gateway sees the opposite, and goes by itself.
- **Jobs** - the [jobs](/turnout/reference/ps/) of your apps that are running right now, in the background or in other terminals.
- **Catalog** - the servers, credentials, paths and targets by name.
- **Recent** - the last five entries of the [journal](/turnout/concepts/journal/), newest first.

On a terminal the sections have colour and the addresses are links - a click opens them in terminals that support it (Windows Terminal, VS Code, iTerm2, WezTerm, Konsole, GNOME Terminal and others). `NO_COLOR` turns the colour off; `FORCE_HYPERLINK=0` or `1` overrides the guess about links. Piped, the output is plain text.

`status` never prints secrets - it only reports which credentials exist.

## The prompt line

`--short` prints one line for a shell prompt or a status bar:

```text
$ turnout status --short
prod · gw ok · 2 dev
```

- **the stand** - in an app's directory, the server that app points at (`web unbound` when it points nowhere); elsewhere, every server an app is bound to. A stand the gateway has a signal about says so: `prod 502`, `prod down` when it does not answer, `prod login` when the session lapsed.
- **the gateway** - `gw ok`, `gw down` when its process lives but does not answer, `gw off` when it is not running and an app is bound.
- **the jobs** - running jobs counted by command: `2 dev`, `1 build`.

Nothing to say prints nothing - not even an empty line - so the segment disappears. The line is quick: it reads local files only, never the network, and never checks for updates. Wiring it into starship, tmux and PowerShell: [the prompt line guide](/turnout/guides/prompt-line/).

## As JSON

`--json` prints the whole view - every field the sections above are made from:

```bash
turnout status --json | jq -r '.apps[] | "\(.name) -> \(.server)"'
turnout status --json | jq '.gateway.signals'
```

The top level has `version`, `data_dir`, `set_up`, `apps` (as [`app show --json`](/turnout/reference/app/#list--show) prints each one), `groups`, `gateway` (`state`, `pid`, `started`, `front_door`, `signals`), `jobs` (as [`ps --json`](/turnout/reference/ps/#as-json)), `catalog` and `recent`. The rules every command's JSON follows are on [JSON output](/turnout/concepts/json-output/).
