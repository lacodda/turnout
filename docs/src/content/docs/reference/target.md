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
                          [--url URL] [--health URL | --health-command CMD] [--health-wait SECS]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--app` | `-a` | The app whose artifacts travel |
| `--server` | `-s` | The server they land on |
| `--credential` | `-C` | The credential that logs in (defaults to the server's) |
| `--path` | `-p` | The named path they are written to |
| `--url` | `-u` | Where the deployed app answers, when that is not the server's URL - see [Address](#address) |
| `--health` | | After a deploy, `GET` this: a path under the target's address (`/health`) or a full URL - see [Health check](#health-check) |
| `--health-command` | | After a deploy, run this on the server, in the deploy directory, instead |
| `--health-wait` | | How long the check keeps asking, in seconds (default 60) |

Anything missing is asked for on a terminal, with the server's own credential pre-selected. Every part must already exist: a target names entities, it does not create them.

**The name defaults to `APP-SERVER`.** Both halves were chosen by you, so the generated name is one you recognize - `myapp-prod`, `web-staging`. Pass a name to use your own. A name already taken gains a numeric suffix rather than overwriting, because two targets for one pair is a real shape - a staging root beside the live one - and losing either silently would be worse than an ugly name.

## list / show

```bash
turnout target list          # name, app -> server, credential and path
turnout target show myapp-prod   # full card: the directory files land in, the address, the health check
```

Both take `--json` and print the same fields as JSON - `list` an array of what `show` prints for one. See [JSON output](/turnout/concepts/json-output/).

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
| `--url` | `-u` | Its own address; empty clears it |
| `--health` | | A URL check; empty clears the check |
| `--health-command` | | A command check; empty clears the check |
| `--health-wait` | | How long the check keeps asking; keeps the probe as it is |

Each health flag changes its own part: a new probe keeps the wait, a wait keeps the probe. A wait with no check to belong to is refused rather than dropped.

The app is not editable: a target that changes which app it deploys is a different route, and renaming one into another silently is how a deploy ends up somewhere nobody chose. Add a new target instead.

## Address

A server's URL is the stand the [gateway](/turnout/reference/gateway/) routes an app's requests to - usually an API. What a deploy puts on that machine is often seen elsewhere: an admin front end on its own host name, a site under a path. `--url` says where:

```bash
turnout target edit admin-prod --url https://admin.example.com
turnout open admin-prod          # opens https://admin.example.com
```

[`open`](/turnout/reference/open/), `deploy --open` and a health check given as a path all use it; a target without one uses its server's URL.

## Health check

A deploy that uploads and restarts has not yet shown that the stand works. A target's health check is how it shows it - after the restart, turnout asks until the stand answers or the wait runs out:

```bash
turnout target edit myapp-prod --health /health                       # GET {address}/health
turnout target edit myapp-prod --health-command "curl -fsS localhost:3000/health"
turnout target edit myapp-prod --health-wait 120                     # a slow starter
```

- **A URL** passes when it answers below 400 within the wait. Redirects are not followed, and a 404 fails it - a health address that is not there is a deploy that lost its routes. A self-signed certificate is accepted only when the server is marked `--insecure`.
- **A command** runs on the server, in the deploy directory, the way a path's restart does; exit code 0 passes. Useful when the stand only listens on localhost, or when "it works" means something an HTTP answer cannot say.

A deploy that fails its check - or whose restart fails - is **rolled back**: see [Health check and rollback](/turnout/reference/deploy/#health-check-and-rollback).

A run that overrides the server or the path (`deploy myapp-prod --path other`) goes to a different stand, so the target's check is not asked there, and the deploy says so.

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
