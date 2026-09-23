---
title: "stop"
description: Stop a job - an app's background dev server, a build, a deploy, or the gateway.
sidebar:
  order: 8.3
---

```bash
turnout stop [APP] [COMMAND]
turnout stop gateway
```

Ends a running [job](/turnout/concepts/background-jobs/) and everything it started - the dev server, its watchers, esbuild's helper processes - and removes it from [`ps`](/turnout/reference/ps/).

## Which job

- **`APP`** - every running job of the app: its `dev`, a `storybook`, a deploy in progress.
- **`APP COMMAND`** - that one: `turnout stop web storybook`.
- **Nothing** - the app of the current directory.
- **`gateway`** - the gateway; the same as [`turnout gateway stop`](/turnout/reference/gateway/#start--stop).

## How a job is ended

- **In the background.** The job's whole process tree is ended: on Windows by parentage and by the job object its supervisor keeps it in, on Linux and macOS through the process group the job leads.
- **In another terminal.** A `turnout dev` running in a terminal of its own is interrupted the way Ctrl+C there would do it, so that turnout gets to put that terminal back. On Linux and macOS this needs turnout to lead its process group, which every interactive shell arranges; a turnout started from a script shares somebody else's group, and `stop` refuses rather than signal the script too - stop it where it runs. On Windows the tree is ended outright.
- **A job that does not go.** After five seconds it is killed.

Nothing is signalled on the strength of a number alone: a record names its process by pid **and** by the moment that process started, so a pid the system has since handed to another program is never taken for the job. A record whose process is gone is cleared with a note instead:

```text
$ turnout stop web
web dev (pid 31412) was no longer running - cleared its record.
```

A job that finished by itself is left alone - its row in `ps` is the last word on how it went, and the next run replaces it.
