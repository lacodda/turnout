---
title: "ps"
description: Show the jobs turnout runs - dev servers, builds, deploys and the gateway.
sidebar:
  order: 8.1
---

```bash
turnout ps [-w]
```

Lists every [job](/turnout/concepts/background-jobs/) turnout knows about: what runs right now, in the background or in some other terminal, and how the last run of each command ended. The gateway is a job like any other and has its row.

```text
JOB           STATE            PID    PORT  ADDRESS                TIME
gateway       running (bg)     24180  80    http://localhost       2h 03m
web dev       ready (bg)       31412  5100  http://web.localhost   14m
api dev       running          30988  5101  http://api.localhost   9m
web build     done             -      -     -                      3m ago
web deploy    failed (exit 1)  -      -     -                      1m ago
```

## Options

| Flag | Meaning |
| --- | --- |
| `-w, --watch` | Keep the table on screen and refresh it every second; Ctrl+C to quit |

## The columns

- **JOB** - the app and the command, or `gateway`. The name is what [`logs`](/turnout/reference/logs/) and [`stop`](/turnout/reference/stop/) take.
- **STATE**
  - `running` - the process is alive; `(bg)` marks a job started with `--detach` or by `gateway start`, and no mark means it runs in somebody's terminal.
  - `ready` - a server job (`dev`, a custom command) announced that it is up - see [the quiet console](/turnout/concepts/quiet-console/#commands-that-keep-running).
  - `done` / `failed (exit N)` - the job ended by itself, with that exit code; `failed (killed)` when a signal ended it.
  - `gone` - the process is no longer there and never said how it ended: killed from outside, a crash, a reboot. `turnout stop NAME` clears the row.
- **PID** - the process that owns the job: the supervisor of a background job, the `turnout` in a terminal, the gateway itself.
- **PORT / ADDRESS** - for `dev`, the app's dev port and its address through the [front door](/turnout/reference/gateway/#the-front-door) when the gateway runs (`http://localhost:PORT` when it does not); for a custom server command, the address it announced; for the gateway, its door.
- **TIME** - how long a running job has been up, or how long ago a finished one ended.

Running jobs come first. A finished row stays until the next run of the same command replaces it.

## Watching

`--watch` redraws the table every second - a poor man's `top` for the jobs of your project. It needs a terminal; piped, `ps` without the flag prints the table once, as often as you ask.
