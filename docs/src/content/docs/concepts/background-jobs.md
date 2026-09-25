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

## How you hear back

A background job says how it went with a [desktop notification](/turnout/concepts/notifications/): the server is up, the build finished, the deploy went out - or it failed, with the line that says why. A click takes you to the page, the stand or the log. One notification per outcome, none for progress, none for a job you stopped yourself.

## Always in the background

`TURNOUT_DETACH` makes the background the default for the commands it names, so nobody has to remember `-d`:

```bash
export TURNOUT_DETACH=build,deploy     # these two, always
export TURNOUT_DETACH=all              # every command that can detach
```

Names are commands as turnout runs them - `dev`, `build`, `test`, `lint`, `deploy`, or a custom command's own name (`storybook`), with its case. `all` (or `1`, `true`, `yes`, `on`) covers everything; an empty value, `0`, `false`, `no`, `off` or `none` covers nothing. A name that is neither a built-in command nor any app's is reported, rather than silently matching nothing.

The job still stays in the terminal when:

- **You say so.** `--foreground` keeps this one run here, and `-v` does too - streaming the output in full means being where it streams to.
- **There is no terminal.** Piped, redirected, in CI: the job runs in line, the way it always did. A preference for the background must not turn `turnout build && turnout deploy` in a script into a race between the two.

A job sent to the background by the preference says so, so it never looks like a run that forgot to wait:

```console
$ turnout build web
[web] pnpm build
web build runs in the background (pid 30112) - TURNOUT_DETACH covers 'build'; --foreground keeps it here
  output: turnout logs web build -f
  stop:   turnout stop web build
```

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

A supervisor has no console anybody reads, so its own troubles go into the job's log too - a command that could not be started at all, a notification that could not be shown - as lines starting with `turnout:`.

## A failure is kept

The next run of a job starts its log afresh - and a background failure is usually read after the fact, by which time it may have been retried. So a run that fails leaves a copy of its log in `failed/` beside it:

```text
logs/web.build.log          # the latest run
logs/failed/web.build.log   # the latest failure, until the next one
```

The failure's notification opens that copy, and `turnout logs web build --failed` prints it - still there after the retry succeeded. A run stopped with Ctrl+C is not a failure and keeps nothing.
