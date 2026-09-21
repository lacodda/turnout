---
title: The quiet console
description: What turnout shows while a command runs, what it holds back, and where the rest of it goes.
---

A build spends two minutes filling the scrollback with progress nobody reads, and the one line that mattered scrolled past forty seconds ago. What a person wants from `turnout build` is a line saying it is building and a line saying it finished - and, on the rare failure, the whole output right there, without running it again.

So every command turnout starts for an app is captured rather than passed straight through. The output always goes to a [log file](#the-log-file); only what reaches the terminal changes.

## Commands that finish

`build`, `test`, `lint`, and the build step inside [`deploy`](/turnout/reference/deploy/) show a loader with the elapsed time, and nothing else:

```
[myapp] pnpm build
◇  Building myapp finished in 24s
```

When one fails, the output appears immediately - the end of it on the terminal, all of it in the log:

```
[myapp] pnpm build
■  Building myapp failed after 3.2s
src/app.ts:14:8 - error TS2304: Cannot find name 'wat'.
full output: C:\Users\me\AppData\Local\lacodda\turnout\logs\myapp-build.log
```

## Commands that keep running

`dev`, and any custom command through [`run`](/turnout/reference/run/), show a loader until the server says it is up, and from then on only the lines that look like a problem:

```
[myapp] pnpm dev --port 5100
[myapp] http://myapp.localhost -> dev server port 5100
◇  myapp ready in 1.4s - http://myapp.localhost
```

Hot-reload chatter stays out of the way; an error or a warning from the server comes through as it happens. Everything, chatter included, is in the log.

**A server that never announces itself** keeps its console. turnout recognises the three shapes it can: Vite's `ready in`, Next's `compiled`, and - for anything else - the first line carrying an `http://` address, which is what a server prints when it starts listening. If none of those arrives within ninety seconds, turnout stops waiting, prints what it held back and streams from there on, exactly as it did before.

## Seeing everything

`-v` / `--verbose` turns the loader off and streams the output in full. So does anything that is not a terminal: a pipe, a file, a CI job. Piped output is byte for byte what it always was, and exit codes pass through either way.

```bash
turnout build myapp -v      # the whole build, as it happens
turnout build myapp | tee out.log   # streams, no loader, nothing to strip
```

## The log file

Every job writes both of its streams to `logs/` in the [data directory](/turnout/getting-started/), one file per app and command:

```
logs/myapp-build.log
logs/myapp-dev.log
logs/myapp-test-e2e.log
```

The next run of the same command replaces the file rather than growing it - the question a log answers is "what did the last one say". A command name that is not a file name (`test:e2e`) is made into one.

The log is written whatever the console does, including under `-v`. Hiding output is only acceptable when none of it is lost.
