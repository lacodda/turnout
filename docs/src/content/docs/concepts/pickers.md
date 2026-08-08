---
title: Pickers
description: Leave a name out and turnout asks - on a terminal only, never in scripts.
---

Most commands take the name of an app, a server or a group. You rarely have to remember it: leave the name out and turnout shows a list to pick from.

```bash
turnout use            # pick the app (or group), then the server
turnout app show       # pick the app
turnout pass copy      # pick which stored access to copy
```

The list is not just names - it carries the context needed to choose:

```
Switch
> web       -> staging
  admin     -> prod-eu
  api          (unbound)
  contour   group: web, admin
```

## The rule

A picker appears **only when both stdin and stderr are a terminal**. Anywhere else - a pipe, a CI job, a `cron` line - the command fails exactly as it did before:

```bash
$ turnout app show < /dev/null
error: app name is required outside an interactive terminal
```

That is deliberate: a prompt nobody can answer would hang a build forever. Scripts keep passing names explicitly and behave as they always have.

## What each command narrows down

- [`use`](/turnout/reference/use/) offers apps **and** groups; once an app is chosen, the server list shrinks to the ones that app is allowed to use.
- [`dev`, `build`, `test`, `lint`, `run`](/turnout/reference/run/) still resolve the app from the current directory first - the picker only steps in when you are outside any known project.
- [`pass`](/turnout/reference/pass/) picks a stored credential, so the list shows only servers that actually have access saved. When a server holds a single kind it is taken without asking.
- [`app`](/turnout/reference/app/), [`server`](/turnout/reference/server/) and [`group`](/turnout/reference/group/) pick from their own catalog for `show`, `edit` and `remove`.
