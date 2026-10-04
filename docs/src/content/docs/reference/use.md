---
title: "use"
description: Bind an app to a server for development - the daily switch command.
sidebar:
  order: 6
---

```bash
turnout use [APP] [SERVER] [--no-check]
turnout use [APP] -
turnout use -
```

| Flag | Short | Description |
| --- | --- | --- |
| `--no-check` | `-n` | Skip the stand health check |

The daily command: point an app at a stand. The binding is written to turnout's state; a running [gateway](/turnout/reference/gateway/) picks it up on the very next request - no restarts, no env edits.

```bash
turnout use myapp staging     # work against staging
turnout use myapp prod-eu     # switch - sessions for staging stay alive in the jar
```

If the app has an allow-list of servers, only those are accepted; an empty list means the app may use any server from the catalog.

`use` also takes a [group](/turnout/reference/group/) name - every member of the group is bound in one go:

```bash
turnout use contour staging   # the whole contour switches together
```

## Picking, with the stands' health

Leave either argument out on a terminal and turnout asks. Picking the app first narrows the server list to the ones that app is allowed to use - see [Pickers](/turnout/concepts/pickers/).

Before the server list opens, every stand on it is asked once, all at the same time, and the answer is shown beside its name - so a stand that is off is not the one you pick by accident:

```text
? Point 'myapp' at ›
❯ staging  https://staging.example.com  up, 84 ms
  prod-eu  https://eu.example.com       down, 502
  qa       https://qa.example.com       unreachable
```

The stand you pick is not asked again.

## The health check

After a switch, `use` says whether the stand is there - one `GET` of its `/health`, the path most backends answer on:

```text
$ turnout use myapp staging
'myapp' now uses 'staging'.
The running gateway picks this up automatically.
'staging' is up: https://staging.example.com/health answered 200 in 84 ms.
```

- **up** - any answer below 500. A stand without a `/health` endpoint answers 404, and that still says it is there.
- **down** - 502, 503 or 504: the stand's web server is there, the application behind it is not.
- **unreachable** - no answer within 3 seconds: the machine is off, out of reach, or the name does not resolve.

The check never refuses the switch - you may be switching to a stand that is about to come back. `--no-check` (`-n`) skips it, in the picker too.

## Going back

`-` in place of the server takes the app back to the server it used before its last switch; `-` alone does that for whatever the last `use` switched. Like `cd -`, it toggles:

```bash
turnout use myapp prod-eu     # try the other stand
turnout use -                 # back to staging
turnout use -                 # and to prod-eu again
turnout use contour -         # a group goes back member by member
```

A member that had no server before stays where it is, and says so. Removing a server unbinds every app pointed at it - now or before - so `-` never leads to a stand that is gone.

The current bindings are always visible in [`turnout status`](/turnout/reference/status/), and the stand an app points at is in your prompt with [`status --short`](/turnout/guides/prompt-line/).
