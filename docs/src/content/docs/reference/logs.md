---
title: "logs"
description: Print a job's output - a background dev server, the last build, the gateway.
sidebar:
  order: 8.2
---

```bash
turnout logs [APP] [COMMAND] [-f] [-n N]
turnout logs gateway [-f]
```

Prints the output a [job](/turnout/concepts/background-jobs/) wrote to its log file - both streams, in the order they came. Every job keeps one, whether it ran in the background, in a terminal under the [quiet console](/turnout/concepts/quiet-console/) or with `-v`.

## Options

| Flag | Meaning |
| --- | --- |
| `-f, --follow` | Keep printing as the job writes; return when the job ends |
| `-n, --lines <N>` | Only the last `N` lines |

## Which job

- **`APP COMMAND`** - that command of that app: `turnout logs web build`.
- **`APP`** alone - the app's running job, or, when nothing of it runs, the one that ran last.
- **Nothing** - the app of the current directory, the same way [`turnout dev`](/turnout/reference/run/) finds it.
- **`gateway`** - the gateway's own output: the ports it listens on, the door, a WebSocket to a stand that failed. An app that happens to be called `gateway` is reached by naming its command as well: `turnout logs gateway dev`.

A gateway started with `turnout gateway run` writes to the terminal it runs in and keeps no log; `logs` says so.

## Following

`--follow` prints what is there, then everything the job writes after, and returns when the job ends - no Ctrl+C needed for a build. A new run of the same command starts its log afresh; a follow in progress starts over with it.

```bash
turnout dev web --detach
turnout logs web -f         # the server's output as it comes, until it stops
turnout logs web build -n 20
```
