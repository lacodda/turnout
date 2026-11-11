---
title: "tunnel"
description: Reach a port the server reaches from a port here - a database, a cache, an admin page - over SSH, in the background.
sidebar:
  order: 8.8
---

```bash
turnout tunnel NAME@SERVER PORTS [--credential NAME]
turnout tunnel list [--json]
turnout tunnel stop [NAME]... [--all]
```

Opens a tunnel: a port on this machine that reaches a port the server reaches, carried over SSH. The stand's database answers at `localhost:5432` here, for whatever client asks - `psql`, a GUI, an app's own `DATABASE_URL` - without opening the database to the network and without a terminal kept open on `ssh -L`.

```console
$ turnout tunnel db@staging 5432
✓ Tunnel db open: localhost:5432 -> staging:5432
  pid 31877 · log: turnout logs tunnel db
  close: turnout tunnel stop db
```

The tunnel runs in the background, as a [job](/turnout/concepts/background-jobs/), until it is closed: the terminal that opened it is free at once and can be closed. It shows in [`ps`](/turnout/reference/ps/) as `tunnel db`, in [`status`](/turnout/reference/status/) under **Tunnels**, and `turnout logs tunnel db` prints what it has to say.

## Opening one

- **`NAME@SERVER`** - `NAME` is how `list` and `stop` know the tunnel: lowercase letters, digits and dashes. `SERVER` is a server or a [target](/turnout/reference/target/); a target goes through its server with its credential. `SERVER` alone names the tunnel `SERVER-PORT` - `turnout tunnel staging 5432` opens `staging-5432`.
- **`PORTS`** - read the way `ssh -L` reads them:

| Form | Listens here on | The server connects to |
| --- | --- | --- |
| `5432` | `localhost:5432` | its own port 5432 |
| `15432:5432` | `localhost:15432` | its own port 5432 |
| `15432:db.internal:5432` | `localhost:15432` | `db.internal:5432` - a machine only the server reaches |
| `6380:[fd00::7]:6379` | `localhost:6380` | an IPv6 address, in brackets |

| Flag | Short | Description |
| --- | --- | --- |
| `--credential` | `-C` | Sign in with this credential instead of the target's or the server's |

The tunnel signs in the way the credential says - its key, the password in the keyring, the [SSH agent](/turnout/concepts/ssh-agent/) - and checks the server's [host key](/turnout/concepts/host-keys/) as every connection of turnout's does. Nothing is asked: a tunnel in the background has nobody to ask, which is also why it does not run the system `ssh`.

Before it says it is open, a tunnel asks the server once whether it reaches the far end, so what cannot work is refused on the spot rather than at the first client:

```console
$ turnout tunnel db@staging 5433
error: 'staging' cannot connect to localhost:5433: nothing listens on port 5433 there - is the service running?
```

The other refusals say their cure the same way: a port here that something else holds (with the process that holds it, and a command with another port), a server whose `sshd` forwards nothing (`AllowTcpForwarding` in its `sshd_config`), a credential the server rejects.

**Opening the same tunnel again** - the same name, server, credential and ports - is not an error: `turnout tunnel` says it is open already and returns, so a script can open what it needs before it starts. The same name for a different route is refused until the first tunnel is closed.

## Where it listens

On the loopback only - `127.0.0.1`, and `::1` where the machine has it - never on the network. Both, because `localhost` is either: a client that resolves it to `::1` first, as Node and the JVM may, reaches the tunnel the same as one that tries `127.0.0.1`. A port somebody holds on any address is refused rather than shared.

Ports below 1024 need administrator rights on Linux; the refusal there suggests one above them.

## When the connection drops

One SSH connection carries all of a tunnel's connections, and it is expected to drop: a laptop sleeps, a server reboots, a network changes. The tunnel notices within a minute - it asks the server every 15 seconds when nothing else crosses the connection - and connects again by itself: at once, then after a pause that grows to 30 seconds. A client that arrives in between is carried over a fresh connection when the server answers, and gets a closed socket when it does not, never a hang.

While it is down, the tunnel says so: `down` in `tunnel list` and `status`, with the reason, and one [notification](/turnout/concepts/notifications/) - not one per attempt, and not again for ten minutes if the connection flaps. The next connection signs in with the catalogs as they are then: a server moved to another address, a password changed with [`pass set`](/turnout/reference/pass/), are what it uses.

## list

```console
$ turnout tunnel list
NAME   STATE  LOCAL            REMOTE                       TIME
cache  down   localhost:6379   staging:6379                 2h 10m
  ! cannot reach staging.example.com:22: connection refused (for 3m) - turnout keeps trying
db     open   localhost:5432   staging:5432                 2h 14m
admin  open   localhost:18080  admin.internal:80 via prod   5m
```

- **STATE** - `open`: listening and carrying. `down`: listening, its SSH connection lost - the line under it says why and for how long. `failed`: the tunnel ended by itself, which only a fault of its own does; `gone`: its process vanished without a word - killed from outside, a reboot. Both of the last two say how to read why and clear the row.
- **REMOTE** - `staging:5432` is the server's own port; `admin.internal:80 via prod` an address beyond the server.
- **TIME** - how long the tunnel has been open.

`--json` prints the same rows as an array of objects - `name`, `server`, `credential`, `local` (`localhost:5432`), `local_port`, `host`, `port`, `state`, `trouble` (why it is down, else `null`), `down_since`, `pid`, `started`, `log`. Times are ISO 8601 in UTC; see [JSON output](/turnout/concepts/json-output/).

## stop

```bash
turnout tunnel stop db           # one
turnout tunnel stop db cache     # several
turnout tunnel stop --all        # every tunnel
```

| Flag | Short | Description |
| --- | --- | --- |
| `--all` | `-a` | Close every tunnel |

Closes the tunnel and frees its port; with no name, a terminal gets a picker. A tunnel that is gone or failed has only its row left, and `stop` clears it. [`turnout stop tunnel db`](/turnout/reference/stop/) does the same, the way `stop` ends any job.
