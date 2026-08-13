---
title: "app"
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

| Flag | Short | Description |
| --- | --- | --- |
| `--path` | `-p` | Project directory |
| `--port` | `-P` | Local gateway port for this app |
| `--dist` | `-d` | Build artifact directory, relative to the project path |
| `--command` | `-c` | Set a command as `NAME=CMD` (repeatable); overrides detected defaults |
| `--server` | `-s` | Allow a server for this app (repeatable) |

With `NAME` or `--path` missing, an interactive wizard walks you through: it detects the project type (pnpm / yarn / npm / cargo) from lock and manifest files, proposes commands, suggests a free gateway port and lets you pick allowed servers from the catalog.

With both given, `add` is fully non-interactive (useful for scripts): commands come from detection, adjustable via `--command`.

### Where the commands come from

When the project has a `package.json`, turnout reads its actual `scripts` instead of assuming names. Each of turnout's roles takes the first script that fills it:

| Role | Script names tried, in order |
| --- | --- |
| `dev` | `dev`, `serve`, `start`, `dev:server`, `watch` |
| `build` | `build`, `build:prod`, `compile`, `dist` |
| `test` | `test`, `test:unit`, `spec` |
| `lint` | `lint`, `lint:js`, `eslint` |

So a Vue CLI project whose dev script is `serve` gets `dev -> pnpm serve`, and `turnout dev` just works. Scripts that fill no role are kept under their own name, reachable through [`turnout run`](/turnout/reference/run/):

```bash
turnout run storybook myapp
```

Projects without a `package.json` (or without `scripts`) fall back to the conventional command set for the detected manager.

```bash
turnout app add                        # wizard, from the current directory
turnout app add myapp --path ~/dev/myapp --port 7100
turnout app add api --path ~/dev/api --command "dev=make run" --server staging
turnout app add api -p ~/dev/api -c "dev=make run" -s staging   # same, short form
```

## list / show

```bash
turnout app list          # one line per app: name, path, gateway port
turnout app show myapp    # full card: commands, dist, allowed servers
```

`show` warns if the project directory no longer exists on disk. Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/); `edit` and `remove` do the same.

## edit

```bash
turnout app edit myapp                              # interactive wizard
turnout app edit myapp --port 7200                  # change one field
turnout app edit myapp --command "deploy=make ship" # add or override a command
turnout app edit myapp --command deploy=            # remove a command
turnout app edit myapp --add-server prod --rm-server staging
```

| Flag | Short | Description |
| --- | --- | --- |
| `--path` | `-p` | Project directory |
| `--port` | `-P` | Local gateway port for this app |
| `--dist` | `-d` | Build artifact directory, relative to the project path |
| `--command` | `-c` | Set a command as `NAME=CMD`, or `NAME=` to remove it (repeatable) |
| `--add-server` | `-a` | Allow a server (repeatable) |
| `--rm-server` | `-r` | Disallow a server (repeatable) |

## remove

```bash
turnout app remove myapp [--yes]
```

Removes the app from the catalog only - the project on disk is never touched.
