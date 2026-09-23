---
title: Background jobs
description: What runs in the background, how turnout keeps track of it, and why a pid is never trusted on its own.
---

A dev server holds a terminal for as long as it runs; two apps and the gateway hold three. `--detach` gives the terminal back: the job goes on in the background, and turnout keeps a record of it that [`ps`](/turnout/reference/ps/), [`logs`](/turnout/reference/logs/) and [`stop`](/turnout/reference/stop/) read.

```console
$ turnout dev web --detach
[web] pnpm dev --port 5100
[web] http://web.localhost -> dev server port 5100
web dev runs in the background (pid 31412)
  output: turnout logs web dev -f
  stop:   turnout stop web dev

$ turnout ps
JOB      STATE         PID    PORT  ADDRESS               TIME
gateway  running (bg)  24180  80    http://localhost      2h 03m
web dev  ready (bg)    31412  5100  http://web.localhost  12s
```

`dev`, `build`, `test`, `lint`, `run` and `deploy` all take `-d` / `--detach`.

## What "at once" means

`--detach` returns once the job has started - and a little after: it watches for most of a second more, because the jobs that fail do it right away. A typo in the command, a missing `node_modules`: those come back to the terminal that asked, with their output and their exit code, instead of a cheerful "runs in the background" that sends you to `ps` to discover it never ran. A job that *finishes* that quickly says so too.

## The supervisor

A background job is not the command itself set loose. turnout starts itself again, detached, as a supervisor, and the supervisor runs the command exactly the way a foreground run would - output captured into the [log file](/turnout/concepts/quiet-console/#the-log-file), the server watched until it says it is up - and writes down how it ended. A command simply let go would take none of that with it: not whether the server came up, not its exit code, and on Windows not even a way to take its whole process tree down again.

On Windows the supervisor has a console nobody sees, so the tools it starts never pop up a window of their own, and its job's tree lives in a kill-on-close job object. On Linux and macOS it runs in a session of its own, so closing the terminal does not take it along.

## The registry

Every job has a record: background jobs, jobs running in a terminal, and the gateway - which is simply the first job, not a mechanism of its own. The records are files under `jobs/` in the [data directory](/turnout/getting-started/), one per job:

```text
jobs/gateway.json
jobs/web.dev.json
jobs/web.build.json
```

One file each rather than one shared file, because they are written by different processes at once - a supervisor noting its server is up, a build in another terminal noting it finished - and a shared file would let one of them silently overwrite the other's news.

A record names a *slot*, not a run: the next `turnout build web` takes over `web.build` and its log. Two runs of the same job at once are refused - they would truncate each other's log, and two dev servers of one app fight over its port:

```text
error: web dev is already running (pid 31412) - see `turnout ps`, stop it with `turnout stop web dev`
```

## A pid is only a number

A record keeps the process id - and the moment the operating system says that process started. A pid outlives its process: the system hands the number to the next program that starts, and after a reboot every recorded pid names *something*. A registry that trusted the number alone would show a dead dev server as running because some unrelated program has its pid now, and `stop` would kill that program.

So a record's process is the one with that pid **and** that start time. When they no longer match, the job is `gone` in `ps`, `stop` clears it without signalling anything, and a new run takes the slot as if it were free.

## Logs

Each job's output goes to `logs/APP.COMMAND.log` (the gateway's to `logs/gateway.log`) under the data directory, or wherever `TURNOUT_LOG_DIR` points. The record remembers which file it wrote, so `turnout logs` finds it even after the variable changed.
