---
title: turnout group
description: App groups - switch a whole contour with one use command.
sidebar:
  order: 6.5
---

```bash
turnout group <add|list|show|edit|remove> [...]
```

A **group** is a named set of apps. Its purpose is one command:

```bash
turnout use contour staging    # every app in the group now points at staging
```

`use` accepts a group name anywhere an app name works; each member's server allow-list is validated before anything is bound, so the switch is all-or-nothing.

## add

```bash
turnout group add                                # wizard: pick apps from a list
turnout group add contour --app web --app api    # scripted
```

Group names must not clash with app names.

## list / show

```bash
turnout group list           # groups and their members
turnout group show contour   # members and where each one points now
```

## edit / remove

```bash
turnout group edit contour --add-app admin --rm-app api
turnout group remove contour [--yes]     # apps themselves are untouched
```

Removing an app from the catalog removes it from its groups too; a group left empty disappears.
