---
title: "server"
description: Manage servers - the stands turnout routes to and deploys on.
sidebar:
  order: 4
---

```bash
turnout server <add|list|show|edit|remove|trust> [...]
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
turnout server show prod    # full card: URL, SSH, shell, host key, credential, which targets land here
```

**Shell** is the shell that answers SSH there - `sh`, or `cmd.exe` on a [Windows server](/turnout/concepts/windows-servers/) - as turnout learned it on the first command that needed one; until then it reads *not asked yet*. **Host key** is the key `~/.ssh/known_hosts` trusts for the server's SSH address (`ED25519 SHA256:...`), or *not seen yet* before the first connection - see [Host keys](/turnout/concepts/host-keys/). `show` also names the apps whose allow-list has the server (**Used by**) and the apps bound to it right now (**Bound**). Both take `--json` and print the same fields as JSON - `list` an array of what `show` prints for one. See [JSON output](/turnout/concepts/json-output/).

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

A new `--url` or `--host` forgets the server's shell, and the next command asks again: a server pointed at another machine may answer in another language. `--host` forgets it even unchanged - that is how a machine whose sshd now starts a different shell is asked again.

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
turnout server remove staging [--yes] [--force]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--yes` | `-y` | Skip the confirmation prompt |
| `--force` | `-f` | Remove it even when it is the only server an app may use - that app may then use any server |

Apps that allowed this server are updated automatically - the name is removed from their allow-lists. The [targets](/turnout/reference/target/) that land on it are removed with it, and apps bound to it are unbound.

An empty allow-list allows **every** server, so a server that is the only one an app may use is not removed by default: taking it off the list would quietly turn "only prod" into "anything".

```text
error: 'prod' is the only server 'web' may use: removing it would empty the allow-list, and an empty allow-list allows any server.
Allow another server first (`turnout app edit web --add-server NAME`), or pass --force to remove 'prod' and let it use any server.
```

With `--force` it goes, and the removal names the apps that may now use any server.

Credentials and paths are left alone: they are shared, and a login that also reaches three other stands should not disappear with one of them. Remove them explicitly with [`turnout credential remove`](/turnout/reference/credential/) and [`turnout path remove`](/turnout/reference/path/).

## trust

```bash
turnout server trust prod [--yes]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--yes` | `-y` | Skip the confirmation prompt |

Asks the server for its host key, without signing in, and trusts it in place of the one `~/.ssh/known_hosts` holds for that address. This is the way back in after a server was reinstalled and every connection to it is refused:

```text
$ turnout deploy web-prod
error: refusing to connect to prod.example.com:22: its host key has changed
  it presents ED25519 SHA256:Xq3v0yQ0p7Jf1oVb5Hk2mR8sTnLwZ4eD6cA9uYgI1Ek
  /home/me/.ssh/known_hosts holds ED25519 SHA256:pL2kV9mW3xR7tN1qS5dF8gH4jZ6cB0aE2uY7iO3wQ4M for it (line 12)
Someone may be intercepting the connection - or the server was reinstalled and has a new key.
Once you know it is the server: `turnout server trust prod` shows the new key and asks to trust it.

$ turnout server trust prod
prod.example.com:22 presents ED25519 SHA256:Xq3v0yQ0p7Jf1oVb5Hk2mR8sTnLwZ4eD6cA9uYgI1Ek.
/home/me/.ssh/known_hosts holds ED25519 SHA256:pL2kV9mW3xR7tN1qS5dF8gH4jZ6cB0aE2uY7iO3wQ4M for it (line 12): this is not the key turnout last saw there.
Compare it with the server's own before trusting it - there: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub (C:\ProgramData\ssh\ssh_host_ed25519_key.pub on Windows)
? Trust this key for 'prod'? (y/N) y
'prod' is ED25519 SHA256:Xq3v0yQ0p7Jf1oVb5Hk2mR8sTnLwZ4eD6cA9uYgI1Ek from now on.
```

The comparison is yours to make: a reinstalled server and someone in between look the same from here, and only the fingerprint the server itself prints tells them apart. Without a terminal the question needs `--yes`, so a script cannot trust a key by accident.

Run it before the first connection, too, to check the key rather than take it on first use. A server trusted already says so and changes nothing. The lines that named the address alone are removed, a line naming it among other hosts loses only that name, and `@revoked` lines are left as they are. `ssh` reads the same file, so [`turnout ssh`](/turnout/reference/ssh/) trusts the new key as well.
