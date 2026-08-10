---
title: turnout server
description: Manage servers - the stands turnout routes to and deploys on.
sidebar:
  order: 4
---

```bash
turnout server <add|list|show|edit|remove> [...]
```

A **server** is a stand, machine or environment: its base URL, optional SSH access, a TLS policy and a human-friendly label. See [Entities](/turnout/concepts/entities/).

## add

```bash
turnout server add [NAME] [--url URL] [--label TEXT]
                   [--ssh USER@HOST[:PORT]] [--insecure]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--url` | `-u` | Base URL, e.g. `https://staging.example.com` |
| `--label` | `-l` | Human-friendly label |
| `--ssh` | `-s` | SSH access for deploy and remote operations |
| `--insecure` | `-i` | Accept self-signed or invalid TLS certificates for this server |

With `NAME` or `--url` missing, an interactive wizard asks for each field, including whether to accept self-signed TLS certificates for this server.

```bash
turnout server add                     # wizard
turnout server add staging --url https://staging.example.com --label "Team staging"
turnout server add prod --url https://prod.example.com --ssh deploy@prod.example.com:2200
turnout server add lab --url https://10.0.0.42 --insecure   # self-signed cert
turnout server add lab -u https://10.0.0.42 -i             # same, short form
```

## list / show

```bash
turnout server list         # one line per server: name, URL, label
turnout server show prod    # full card: URL, SSH, TLS policy, which apps use it
```

Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/); `edit` and `remove` do the same.

## edit

```bash
turnout server edit prod                 # interactive wizard
turnout server edit prod --url https://prod-eu.example.com
turnout server edit lab --secure         # require valid TLS certificates again
turnout server edit lab --insecure       # accept invalid TLS certificates
```

| Flag | Short | Description |
| --- | --- | --- |
| `--url` | `-u` | Base URL |
| `--label` | `-l` | Human-friendly label |
| `--ssh` | `-s` | SSH access for deploy and remote operations |
| `--ssh-key` | `-K` | Private key file for SSH auth (empty value removes it) |
| `--insecure` | `-i` | Accept invalid TLS certificates for this server |
| `--secure` | `-S` | Require valid TLS certificates for this server |
| `--deploy-path` | `-d` | Set an app's deploy directory as `APP=DIR`, or `APP=` to remove (repeatable) |
| `--restart-cmd` | `-r` | Set an app's post-deploy command as `APP=CMD`, or `APP=` to remove (repeatable) |

SSH auth for deploy can use a private key file (tried before the agent and the stored password):

```bash
turnout server edit prod --ssh-key ~/.ssh/id_ed25519
turnout server edit prod --ssh-key ""    # remove, fall back to agent/password
```

Deploy targets - where each app lands on this server and what runs afterwards (see [`turnout deploy`](/turnout/reference/deploy/)):

```bash
turnout server edit prod --deploy-path myapp=/var/www/myapp
turnout server edit prod --restart-cmd "myapp=systemctl restart myapp"
turnout server edit prod --deploy-path myapp=      # remove the target
turnout server edit prod -d myapp=/var/www/myapp -r "myapp=systemctl restart myapp"  # same, short form
```

## remove

```bash
turnout server remove staging [--yes]
```

Apps that allowed this server are updated automatically - the name is removed from their allow-lists.

:::note
Credentials are a separate entity kept in the OS keyring - a server only references them. Credential commands arrive in the next milestone; see the [roadmap](https://github.com/lacodda/turnout#roadmap).
:::
