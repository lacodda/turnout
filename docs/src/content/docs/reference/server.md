---
title: "server"
description: Manage servers - the stands turnout routes to and deploys on.
sidebar:
  order: 4
---

```bash
turnout server <add|list|show|edit|remove> [...]
```

A **server** is a machine: its base URL, the SSH host and port, a TLS policy and a human-friendly label. Who logs in is a [credential](/turnout/reference/credential/), and where files land is a [path](/turnout/reference/path/) - a server points at both by name. See [Entities](/turnout/concepts/entities/).

## add

```bash
turnout server add [NAME] [--url URL] [--label TEXT]
                   [--host HOST[:PORT]] [--credential NAME] [--insecure]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--url` | `-u` | Base URL, e.g. `https://staging.example.com` |
| `--label` | `-l` | Human-friendly label |
| `--host` | `-H` | SSH host and port when they differ from the URL's host |
| `--credential` | `-c` | Credential used to log in here |
| `--insecure` | `-i` | Accept self-signed or invalid TLS certificates for this server |

`--host` is optional: with no value, SSH connects to the URL's own host on port 22, which is right for most stands. Set it when the stand answers HTTP on one name and SSH on another, or on a non-standard port.

With `NAME` or `--url` missing, an interactive wizard asks for each field, including which credential to use and whether to accept self-signed TLS certificates.

```bash
turnout server add                     # wizard
turnout server add staging --url https://staging.example.com --label "Team staging"
turnout server add prod --url https://prod.example.com --host ssh.prod.example.com:2200
turnout server add lab --url https://10.0.0.42 --insecure   # self-signed cert
turnout server add lab -u https://10.0.0.42 -i             # same, short form
```

## list / show

```bash
turnout server list         # one line per server: name, URL, label
turnout server show prod    # full card: URL, SSH, credential, which targets land here
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
| `--host` | `-H` | SSH host and port (empty value falls back to the URL's host) |
| `--credential` | `-c` | Credential used to log in here (empty value unsets it) |
| `--insecure` | `-i` | Accept invalid TLS certificates for this server |
| `--secure` | `-S` | Require valid TLS certificates for this server |

Who logs in:

```bash
turnout server edit prod --credential prod-deploy
turnout server edit prod -c prod-deploy   # same, short form
```

:::note[Where app-to-path routing went]
A server used to carry a `--deploy-path APP=PATH` map from app to path. That relationship is now a [target](/turnout/reference/target/) - a named entity of its own, so it can be listed, renamed and reused instead of living as a hidden field on the server:

```bash
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"
turnout target add --app myapp --server prod --credential prod-deploy --path wwwroot
```
:::

## remove

```bash
turnout server remove staging [--yes]
```

Apps that allowed this server are updated automatically - the name is removed from their allow-lists.

Credentials and paths are left alone: they are shared, and a login that also reaches three other stands should not disappear with one of them. Remove them explicitly with [`turnout credential remove`](/turnout/reference/credential/) and [`turnout path remove`](/turnout/reference/path/).
