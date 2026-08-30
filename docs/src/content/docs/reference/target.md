---
title: "target"
description: Manage targets - named deploy targets joining an app, a server, a credential and a path.
sidebar:
  order: 6.5
---

```bash
turnout target <add|list|show|edit|rename|remove> [...]
```

A **target** is a named deploy route: which [app](/turnout/reference/app/) goes to which [server](/turnout/reference/server/), logging in as which [credential](/turnout/reference/credential/), landing in which [path](/turnout/reference/path/). It is what [`turnout deploy`](/turnout/reference/deploy/) addresses.

```bash
turnout target add --app myapp --server prod --credential prod-deploy --path wwwroot
turnout deploy myapp-prod        # from any directory
```

Before v0.11.0 this relationship lived inside the server as a map from app to path. It had no name, so it could not be listed, renamed or reused, and a deploy had to be re-described every time it differed from the default. See [ADR 0013](https://github.com/lacodda/turnout/blob/main/docs/adr/0013-named-builds.md).

## add

```bash
turnout target add [NAME] [--app APP] [--server SERVER] [--credential NAME] [--path NAME]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--app` | `-a` | The app whose artifacts travel |
| `--server` | `-s` | The server they land on |
| `--credential` | `-C` | The credential that logs in (defaults to the server's) |
| `--path` | `-p` | The named path they are written to |

Anything missing is asked for on a terminal, with the server's own credential pre-selected. Every part must already exist: a target names entities, it does not create them.

**The name defaults to `APP-SERVER`.** Both halves were chosen by you, so the generated name is one you recognize - `myapp-prod`, `web-staging`. Pass a name to use your own. A name already taken gains a numeric suffix rather than overwriting, because two targets for one pair is a real shape - a staging root beside the live one - and losing either silently would be worse than an ugly name.

## list / show

```bash
turnout target list          # name, app -> server, credential and path
turnout target show myapp-prod   # full card, including the directory files land in
```

Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/).

`turnout server show` lists the targets that land on a server, and `turnout path show` the ones that write into a path - the relationship reads from every side.

## edit

```bash
turnout target edit myapp-prod --path staging-root
turnout target edit myapp-prod --credential root
turnout target edit myapp-prod                  # interactive wizard
```

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | The server it deploys to |
| `--credential` | `-C` | The credential it logs in with |
| `--path` | `-p` | The path it writes into |

The app is not editable: a target that changes which app it deploys is a different route, and renaming one into another silently is how a deploy ends up somewhere nobody chose. Add a new target instead.

## rename

```bash
turnout target rename myapp-prod live
```

Only the handle moves; the route stays exactly as it was. Useful when the generated `APP-SERVER` name stops matching how you talk about it.

## remove

```bash
turnout target remove myapp-prod [--yes]
```

The app, server, credential and path it named are untouched - only the connection between them is gone.

The reverse also holds: removing any of the four takes the targets that named it, because a target missing one of its parts cannot deploy. `turnout path remove` and `turnout server remove` list what goes with them before asking.

## Deploying without a target

`turnout deploy` with no target still works: the app comes from the current directory, the server from the [binding](/turnout/reference/use/), and the credential from the server. If no target exists for that pair, turnout asks which path to deploy into, and offers to save the answer as a target once the deploy succeeds - so it is asked once, not every time.

In a script there is nobody to ask, so pass `--path` (and `--credential` if it differs from the server's), or create the target up front.
