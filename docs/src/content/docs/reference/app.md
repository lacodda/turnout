---
title: turnout app
description: Manage apps - the local projects turnout works with.
sidebar:
  order: 3
---

```bash
turnout app <add|list|show|edit|remove> [...]
```

An **app** is a local project: its path, its commands (`dev`, `build`, ...), its gateway port and the servers it is allowed to use. See [Entities](/turnout/concepts/entities/).

## add

```bash
turnout app add [NAME] [--path DIR] [--port PORT] [--dist DIR]
                [--command NAME=CMD]... [--server SERVER]...
```

With `NAME` or `--path` missing, an interactive wizard walks you through: it detects the project type (pnpm / yarn / npm / cargo) from lock and manifest files, proposes standard commands, suggests a free gateway port and lets you pick allowed servers from the catalog.

With both given, `add` is fully non-interactive (useful for scripts): commands come from detection, adjustable via `--command`.

```bash
turnout app add                        # wizard, from the current directory
turnout app add myapp --path ~/dev/myapp --port 7100
turnout app add api --path ~/dev/api --command "dev=make run" --server staging
```

## list / show

```bash
turnout app list          # one line per app: name, path, gateway port
turnout app show myapp    # full card: commands, dist, allowed servers
```

`show` warns if the project directory no longer exists on disk.

## edit

```bash
turnout app edit myapp                              # interactive wizard
turnout app edit myapp --port 7200                  # change one field
turnout app edit myapp --command "deploy=make ship" # add or override a command
turnout app edit myapp --command deploy=            # remove a command
turnout app edit myapp --add-server prod --rm-server staging
```

## remove

```bash
turnout app remove myapp [--yes]
```

Removes the app from the catalog only - the project on disk is never touched.
