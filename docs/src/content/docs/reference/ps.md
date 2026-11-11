---
title: "ps"
description: Show the jobs turnout runs - dev servers, builds, deploys, the gateway and tunnels.
sidebar:
  order: 8.1
---

```bash
turnout ps [-w | --json]
```

Lists every [job](/turnout/concepts/background-jobs/) turnout knows about: what runs right now, in the background or in some other terminal, and how the last run of each command ended. The gateway and each [tunnel](/turnout/reference/tunnel/) are jobs like any other and have their rows.

```text
JOB           STATE            PID    PORT  ADDRESS                TIME
gateway       running (bg)     24180  80    http://localhost       2h 03m
web dev       ready (bg)       31412  5100  http://web.localhost   14m
api dev       running          30988  5101  http://api.localhost   9m
tunnel db     ready (bg)       31877  5432  localhost:5432         1h 40m
web build     done             -      -     -                      3m ago
web deploy    failed (exit 1)  -      -     -                      1m ago
```

## Options

| Flag | Meaning |
| --- | --- |
| `-w, --watch` | Keep the table on screen and refresh it every second; Ctrl+C to quit |
| `--json` | Print the jobs as a JSON array instead - see [below](#as-json) |

## The columns

- **JOB** - the app and the command, `gateway`, or `tunnel` and the tunnel's name. The name is what [`logs`](/turnout/reference/logs/) and [`stop`](/turnout/reference/stop/) take.
- **STATE**
  - `running` - the process is alive; `(bg)` marks a job started with `--detach` or by `gateway start`, and no mark means it runs in somebody's terminal.
  - `ready` - a server job (`dev`, a custom command) announced that it is up - see [the quiet console](/turnout/concepts/quiet-console/#commands-that-keep-running); a tunnel carries.
  - `down` - a tunnel that runs and has lost its SSH connection; it is trying again by itself, and [`tunnel list`](/turnout/reference/tunnel/#list) says why.
  - `done` / `failed (exit N)` - the job ended by itself, with that exit code; `failed (killed)` when a signal ended it; `failed (did not start)` when its program could not be started at all - its directory is gone, or there is no shell to run it with. The job's log says which.
  - `gone` - the process is no longer there and never said how it ended: killed from outside, a crash, a reboot. `turnout stop NAME` clears the row.
- **PID** - the process that owns the job: the supervisor of a background job, the `turnout` in a terminal, the gateway itself.
- **PORT / ADDRESS** - for `dev`, the app's dev port and its address through the [front door](/turnout/reference/gateway/#the-front-door) when the gateway runs (`http://localhost:PORT` when it does not); for a custom server command, the address it announced; for the gateway, its door; for a tunnel, where clients connect.
- **TIME** - how long a running job has been up, or how long ago a finished one ended.

Running jobs come first. A finished row stays until the next run of the same command replaces it.

## Watching

`--watch` redraws the table every second - a poor man's `top` for the jobs of your project. It needs a terminal; piped, `ps` without the flag prints the table once, as often as you ask.

## As JSON

`turnout ps --json` prints one object per row, in the same order - the fields a status bar or a script needs, with nothing to parse out of columns:

```json
[
  {
    "job": "web dev",
    "app": "web",
    "command": "dev",
    "state": "ready",
    "trouble": null,
    "exit_code": null,
    "never_ran": false,
    "pid": 31412,
    "port": 5100,
    "address": "http://web.localhost",
    "background": true,
    "started": "2026-10-04T09:12:44Z",
    "ended": null,
    "log": "C:\\Users\\me\\AppData\\Local\\lacodda\\turnout\\logs\\web.dev.log"
  }
]
```

`state` is one of `ready`, `running`, `done`, `failed`, `gone`; a tunnel whose connection is down is `running`, with the reason in `trouble`, which is `null` otherwise. `app` and `command` are `null` for the gateway and a tunnel. `exit_code` is set once a job ended by itself. `never_ran` is `true` for a `failed` job whose program never started, which therefore has no exit code either. Times are ISO 8601 in UTC. See [JSON output](/turnout/concepts/json-output/) for the rules every command's JSON follows.

