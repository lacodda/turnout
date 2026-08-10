---
title: turnout use
description: Bind an app to a server for development - the daily switch command.
sidebar:
  order: 6
---

```bash
turnout use [APP] [SERVER] [--no-check]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--no-check` | `-n` | Skip the stand reachability check |

The daily command: point an app at a stand. The binding is written to turnout's state; a running [gateway](/turnout/reference/gateway/) picks it up on the very next request - no restarts, no env edits.

```bash
turnout use myapp staging     # work against staging
turnout use myapp prod-eu     # switch - sessions for staging stay alive in the jar
```

Leave either argument out on a terminal and turnout asks. Picking the app first narrows the server list to the ones that app is allowed to use - see [Pickers](/turnout/concepts/pickers/).

```bash
turnout use                   # pick the app (or group), then the server
```

After switching, `use` probes the stand with a quick HTTP request and reports whether it is reachable. `--no-check` (`-n`) skips the probe:

```bash
turnout use myapp staging -n   # skip the reachability check
```

If the app has an allow-list of servers, only those are accepted; an empty list means the app may use any server from the catalog.

`use` also takes a [group](/turnout/reference/group/) name - every member of the group is bound in one go:

```bash
turnout use contour staging   # the whole contour switches together
```

The current bindings are always visible in [`turnout status`](/turnout/reference/status/).
