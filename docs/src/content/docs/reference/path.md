---
title: "path"
description: Manage paths - named remote directories to deploy into.
sidebar:
  order: 6
---

```bash
turnout path <add|list|show|edit|remove> [...]
```

A **path** is a named directory on a server, plus the command to run after writing to it. Like [credentials](/turnout/reference/credential/), paths are free-standing: the same web root usually exists on the staging box and the production one, and the post-write command belongs to the directory's role rather than to any one machine.

A [server](/turnout/reference/server/) points an app at a path by name:

```bash
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"
turnout server edit prod --deploy-path myapp=wwwroot
turnout server edit staging --deploy-path myapp=wwwroot   # the same path, reused
```

## add

```bash
turnout path add [NAME] [--dir DIR] [--restart CMD]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--dir` | `-d` | Absolute directory on the server |
| `--restart` | `-r` | Command to run on the server after writing here |

With `NAME` or `--dir` missing, an interactive wizard asks for each field.

The directory is **on the server**, so it must be absolute. Both kinds are accepted, matching the servers the path is used on:

```bash
turnout path add wwwroot --dir /var/www/myapp            # POSIX server
turnout path add winroot --dir "C:\inetpub\wwwroot\myapp"  # Windows server
```

A relative path is refused rather than stored. See [Windows servers](/turnout/concepts/windows-servers/) for what else changes against a Windows target.

:::caution[Git Bash rewrites remote paths]
On Windows, Git Bash (MSYS2) turns arguments that look like POSIX paths into Windows paths before turnout ever sees them, so `--dir /var/www/myapp` arrives as `C:/Program Files/Git/var/www/myapp`. turnout recognizes the Git installation prefix and refuses the value, because storing it would deploy to a directory nobody chose - a genuine Windows path like `C:\inetpub\myapp` carries no such prefix and is stored as typed. Any of these gets the POSIX path through:

```bash
MSYS_NO_PATHCONV=1 turnout path add wwwroot --dir /var/www/myapp
turnout path add wwwroot --dir //var/www/myapp   # doubled slash, stored as /var/www/myapp
```

PowerShell, cmd and every non-Windows shell pass the path through unchanged.
:::

## list / show

```bash
turnout path list          # one line per path: name, directory, post-write command
turnout path show wwwroot  # full card, plus which server/app pairs deploy into it
```

Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/).

## edit

```bash
turnout path edit wwwroot --dir /srv/www/myapp
turnout path edit wwwroot --restart "systemctl reload nginx"
turnout path edit wwwroot --restart ""     # run nothing after writing
turnout path edit wwwroot                  # interactive wizard
```

| Flag | Short | Description |
| --- | --- | --- |
| `--dir` | `-d` | Absolute directory on the server |
| `--restart` | `-r` | Post-write command (empty value removes it) |

Editing a path changes it for every server that uses it - which is the point, and worth remembering before widening one.

## remove

```bash
turnout path remove wwwroot [--yes]
```

Server/app pairs that deployed into it are listed before the confirmation and unlinked afterwards. **The directory on the server is never touched** - turnout stops tracking the name, nothing more.
