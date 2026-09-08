---
title: "exec"
description: Run a command on a server, in the deploy directory.
sidebar:
  order: 8.6
---

```bash
turnout exec [NAME] [--credential NAME] [--dir DIR] -- COMMAND...
```

Runs one command on a server and streams its output; the remote exit code becomes turnout's own, so it drops into scripts. `NAME` is resolved like [`turnout ssh`](/turnout/reference/ssh/): a target, then a server, then an app, or the app of the current directory when omitted. A target's command runs in its deploy directory; a plain server's where the login lands.

| Flag | Short | Description |
| --- | --- | --- |
| `--credential` | `-C` | Log in with this credential instead of the target's or the server's |
| `--dir` | `-d` | Run in this directory instead of the target's |

Everything after `--` is the command, joined with spaces and handed to the server's shell as it is - turnout only prepends the `cd`, phrased in the shell that answers there (`cd /d` on a Windows server). Where the command is going, and as whom, is announced on stderr before dialing, so a refused connection still says where it was headed.

```bash
turnout exec web-prod -- ls -la                # in /srv/web
turnout exec pi -- df -h                       # on the server, in ~
turnout exec web-prod -- docker compose ps
turnout exec web-prod --dir /var/log -- tail -n 50 web.log
```
