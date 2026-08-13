---
title: "gateway"
description: Run the local dev gateway that routes apps to their bound servers.
sidebar:
  order: 7
---

```bash
turnout gateway <start|run|stop>
```

The gateway listens on `localhost` - one port per app (`--port` in the app config) - and forwards every request to the server the app is currently [bound to](/turnout/reference/use/). How it treats cookies and redirects is described in [The Dev Gateway](/turnout/concepts/gateway/).

## start / stop

```bash
turnout gateway start   # background process; ports are printed
turnout gateway stop    # stop it
```

`start` launches the gateway detached and records its pid; `status` shows whether it is alive. Bindings changed with `turnout use` are picked up automatically - the gateway re-reads them on every request.

## run

```bash
turnout gateway run     # foreground, Ctrl+C to stop
```

The same server in the foreground - handy for watching it work. `start` spawns exactly this.

## Behavior notes

- **Cookie jar.** Stand cookies never reach the browser; they live in the gateway, in a separate jar per app+server pair. Switching stands swaps jars, so sessions survive switching. The jar is in-memory: restarting the gateway means logging in to the stand again.
- **Redirects.** Absolute `Location` headers pointing at the stand are rewritten to `localhost`, so the browser never escapes.
- **TLS.** Connections to the stand honor the server's TLS policy (`--insecure` accepts self-signed certificates). The gateway itself serves plain HTTP on localhost.
- **WebSocket.** Upgrade requests are proxied too: the gateway opens a matching connection to the stand (jar cookies attached, `wss` for https servers, TLS per policy) and pumps frames both ways.
- **Errors.** If the stand is down you get a plain-text `502` from the gateway saying which stand is unreachable - not a cryptic proxy failure.
