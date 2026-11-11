# ADR 0034: A tunnel is a job on turnout's own SSH

- Status: accepted
- Date: 2026-11-11

## Context

v1.1.0 adds tunnels: `turnout tunnel db@staging 5432` makes a port the server reaches answer on a port here - a database, a cache, an admin page that listens only on the stand's loopback. Until now the way to get there was `ssh -L` in a terminal kept open for the day, with the host, the user and the key retyped from the catalog turnout already holds.

Three questions shaped it: what carries the bytes, where a running tunnel is known, and what a tunnel does when its connection drops - which, for something that runs all day on a laptop, is not an exception but a schedule.

## Decision

**The bytes ride turnout's own SSH (ADR 0011), on russh's `direct-tcpip` channels**, not the system `ssh -L`. A tunnel runs in the background, and a process in the background has nobody to type a password; russh already signs in every way a credential can - a key, the keyring's password, the agent - and checks the host key against `known_hosts` (ADR 0033). The system client would bring a second way of signing in with its own gaps: a password credential would not work at all, and an agent would work only where `ssh` finds the same one. `turnout ssh` stays on the system client because a terminal is its job; a tunnel has no terminal.

**A tunnel is a job in the registry**, the way the gateway is (ADR 0018): a turnout process of its own (`tunnel run`, started detached), a record under `jobs/tunnel@NAME.json`, a log under `logs/`. So `ps`, `status`, `logs` and `stop` know it without a mechanism of their own. The key holds an `@`, which an app's command key never does, so a tunnel and an app's job never share a slot. The record is written only once the tunnel listens, has signed in and has heard from the far end - a record is a tunnel that works.

**Tunnels are not saved in the catalogs.** A named, open tunnel is what the task asks for, and the catalogs are what an export carries, under a format that is a contract (ADR 0032). Opening the same tunnel again is a success that changes nothing, so a script - or a later `tn up` - opens what it needs without remembering whether it did.

**The ports read as `ssh -L` reads them** - `PORT`, `LOCAL:PORT`, `LOCAL:HOST:PORT` - one grammar developers already know instead of flags for each part. A tunnel listens on the loopback only, on `127.0.0.1` and on `::1` where the machine has it: `localhost` is either, and a client that tries `::1` first must not get "connection refused". A port held on any address is refused, since on Windows a bind to `127.0.0.1` beside a listener on `0.0.0.0` would quietly take its connections.

**Before it says it is open, the tunnel asks the server once to connect to the far end.** A typo in a port, a service that is down, an sshd with `AllowTcpForwarding no` are refused on the spot, each with its own words, instead of showing up as the first client's dropped connection.

**A lost connection does not end the tunnel.** Keepalives every 15 seconds, three unanswered and the connection is taken for lost - the only way to notice one that a sleep or a network change cut without a word. The tunnel then connects again by itself, at once and then after a pause that doubles up to 30 seconds; a client that arrives in between triggers an attempt of its own instead of waiting for the next one. While it is down the record carries the reason (`trouble`, a field any job may use), and one notification goes out per episode, not again within ten minutes. Every new connection reads the catalogs and the keyring afresh, so a server moved or a password changed is what the next one uses.

## Consequences

- A tunnel outlives the terminal that opened it, and is closed with `turnout tunnel stop` or `turnout stop tunnel NAME`; it does not survive a reboot.
- `ps --json` and `status --json` gain a `trouble` field and `status --json` a `tunnels` list; both additive (ADR 0020).
- An older turnout reading a registry with a tunnel's record warns that it cannot read that record and goes on without it.
- Every connection a tunnel carries shares one SSH connection, and with it one TCP connection's bandwidth.
- Tested end to end against an SSH server in the test process that is taken down and brought back, and live against OpenSSH in a container, `AllowTcpForwarding no` included.
