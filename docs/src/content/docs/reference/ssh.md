---
title: "ssh"
description: Open an SSH session on a server with nothing to remember.
sidebar:
  order: 8.7
---

```bash
turnout ssh [NAME] [--credential NAME]
```

Opens an interactive SSH session with host, port, user and key taken from the catalogs. `NAME` is tried as a [target](/turnout/reference/target/), then as a server, then as an app; with no name, the app of the current directory. A target's session lands in its deploy directory; a plain server's in the account's home.

| Flag | Short | Description |
| --- | --- | --- |
| `--credential` | `-C` | Log in with this credential instead of the target's or the server's |

The session itself belongs to the system `ssh` client - a terminal is its job, and turnout composes the invocation and gets out of the way. What the prompt will ask for is one paste away: a stored password (or key passphrase) is copied to the clipboard before the client starts. A key credential travels as `-i`; an agent credential needs nothing, the client asks the agent itself.

Landing in the deploy directory needs the server's shell, which the catalog knows after the first remote command (a deploy, an `exec`); when it does not, turnout asks with one extra login, and a login that fails only costs the directory.

`TURNOUT_SSH` names another client binary when `ssh` is not the one on PATH.

```bash
turnout ssh web-prod       # the target: user@host, in /srv/web
turnout ssh pi             # the server, as its own credential, in ~
turnout ssh                # from inside a project: its bound server
```
