---
title: "logs"
description: Print a job's output - a background dev server, the last build, the gateway.
sidebar:
  order: 8.2
---

```bash
turnout logs [APP] [COMMAND] [-f | --failed] [-n N]
turnout logs gateway [-f]
```

Prints the output a [job](/turnout/concepts/background-jobs/) wrote to its log file - both streams, in the order they came. Every job keeps one, whether it ran in the background, in a terminal under the [quiet console](/turnout/concepts/quiet-console/) or with `-v`.

## Options

| Flag | Meaning |
| --- | --- |
| `-f, --follow` | Keep printing as the job writes; return when the job ends |
| `-n, --lines <N>` | Only the last `N` lines |
| `--failed` | The output of the last failure instead of the latest run - kept even after the job ran again |

## Which job

- **`APP COMMAND`** - that command of that app: `turnout logs web build`.
- **`APP`** alone - the app's running job, or, when nothing of it runs, the one that ran last.
- **Nothing** - the app of the current directory, the same way [`turnout dev`](/turnout/reference/run/) finds it.
- **`gateway`** - the gateway's own output: the ports it listens on, the door, a WebSocket to a stand that failed. An app that happens to be called `gateway` is reached by naming its command as well: `turnout logs gateway dev`.

A gateway started with `turnout gateway run` writes to the terminal it runs in and keeps no log; `logs` says so.

## The last failure

A run that fails keeps a copy of its log aside, and the next run does not touch it. `--failed` prints that copy - the output a failure's [notification](/turnout/concepts/notifications/) points at, whether or not the job has been retried since:

```bash
turnout logs web build --failed         # why the last failed build failed
turnout logs web --failed -n 20         # the latest failure of any of web's jobs
```

With only an app named, it is the latest failure among the app's jobs. A job that never failed says so.

## Following

`--follow` prints what is there, then everything the job writes after, and returns when the job ends - no Ctrl+C needed for a build. A new run of the same command starts its log afresh; a follow in progress starts over with it.

```bash
turnout dev web --detach
turnout logs web -f         # the server's output as it comes, until it stops
turnout logs web build -n 20
```
