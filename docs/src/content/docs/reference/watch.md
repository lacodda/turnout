---
title: "watch"
description: Deploy an app again whenever its build changes - in the background, with a notification for every deploy.
sidebar:
  order: 9.1
---

```bash
turnout watch [TARGET] [-s SERVER] [-C CREDENTIAL] [-p PATH] [-b] [--foreground]
```

Keeps a stand on the app's newest build: every time the build changes, it is [deployed](/turnout/reference/deploy/) to the target - only the files that changed - and a [notification](/turnout/concepts/notifications/) says how it went. The watch runs in the background, as a [job](/turnout/concepts/background-jobs/), until it is stopped.

```bash
turnout watch myapp-stage
```

```text
myapp watch runs in the background (pid 30412): a change to dist deploys it to myapp-stage
  output: turnout logs myapp watch -f
  stop:   turnout stop myapp watch
```

It shows in [`ps`](/turnout/reference/ps/) as `myapp watch`, [`logs`](/turnout/reference/logs/) prints what it did, and [`stop`](/turnout/reference/stop/) ends it - `turnout stop myapp watch`, or `turnout stop myapp` with the app's other jobs.

## Options

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Server to use instead of the target's (without a target: instead of the app's binding) |
| `--credential` | `-C` | Credential to use instead of the target's (the server's when there is no target) |
| `--path` | `-p` | Named path to use instead of the target's (picked on a terminal when there is no target) |
| `--build` | `-b` | Watch the sources instead: build on every change, and deploy only a green build |
| `--foreground` | | Stay in this terminal instead of the background; Ctrl+C ends the watch |

`TARGET` is read as `deploy` reads it: a [target](/turnout/reference/target/), or an app to use its target on the bound server; with none, the app of the current directory. One watch per app, to one target: the same change going to two stands at once is a decision for a person, not for a file being saved.

## What it watches

**The build, by default** - the app's artifact directory (`--dist`). Whatever writes it - [`turnout build`](/turnout/reference/run/), a bundler's own `--watch`, an IDE - the stand gets it:

```bash
turnout watch myapp-stage
pnpm vite build --watch        # every rebuild goes to the stand
```

While `turnout build` of the app runs, the watch waits for it, and a build that fails is not deployed: what it left in the directory is half a build. A build run some other way gives no such word, so the watch goes by the files alone.

**The sources, with `--build`.** The watch builds the app itself whenever a source changes, and deploys only a green build; a red one is a notification with the line that says why, and nothing is sent. The sources are what git tracks or would track - an ignored file, `node_modules` and the artifact directory never count, nor does the dotenv file turnout writes - the same list the deploy's [stale-build warning](/turnout/reference/deploy/#a-build-older-than-its-sources) reads. The build gets the stand's [variables](/turnout/reference/deploy/#the-apps-variables), as a deploy's build always does.

## When a change counts

The watch looks at the files twice a second - their sizes and modification times - rather than waiting to be told: change notifications do not reach it from a network share, a WSL mount or a folder bind-mounted into a container, and looking works the same on every one of them.

A change counts once the files have stayed as they are for a moment and a half: a build writes in bursts, and half a build must never go out. A directory that a bundler emptied before writing is never deployed - the watch waits for what comes next. What the watch itself writes while it deploys - the build of `--build` - is not a change to deploy again.

## Each deploy

Each one is an ordinary deploy, with everything a deploy does: only the files the stand does not have yet are sent, the target's [health check](/turnout/reference/deploy/#health-check-and-rollback) runs and rolls a failure back, the [env file](/turnout/reference/target/#env-file) is written, and the deploy goes into [`history`](/turnout/reference/history/) - [`rollback`](/turnout/reference/rollback/) can take the stand back to any of them.

Every deploy is a notification: one that went through opens the stand, one that failed names the reason and opens the watch's log - and the watch goes on, deploying the next change. Until a deploy goes through again, the failure shows in `ps`:

```console
$ turnout ps
JOB           STATE    PID    PORT  TIME    ADDRESS
myapp watch   running  30412  -     1h 12m  -
  ! the last deploy failed: cannot reach staging.example.com:22: connection refused
```

## In the foreground

`--foreground` keeps the watch in the terminal, its deploys printed as they happen, until Ctrl+C. A deploy under way when Ctrl+C comes is finished first: a stand left with half a build is worse than a watch that ends a few seconds late. `stop` ends a background watch at once, even mid-deploy: a deploy cut short that way goes unrecorded and may leave part of a build on the stand - the next deploy, or the next watch, sends what is missing.

## Examples

```bash
turnout watch                             # the app of this directory, to its bound server
turnout watch myapp-stage                 # whatever writes dist goes to myapp-stage
turnout watch myapp-stage --build         # build on every saved source, deploy what is green
turnout watch myapp --server stage-2      # the app's target, on another stand
turnout watch myapp-stage --foreground    # in this terminal, until Ctrl+C
turnout stop myapp watch                  # end it
```
