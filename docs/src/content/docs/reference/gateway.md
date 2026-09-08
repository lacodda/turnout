---
title: "gateway"
description: Run the local dev gateway that routes apps to their bound servers.
sidebar:
  order: 7
---

```bash
turnout gateway <start|run|stop>
```

The gateway has two kinds of doors. Per app, it listens on `localhost` - one port per app (`--port` in the app config) - and forwards every request to the server the app is currently [bound to](/turnout/reference/use/); how it treats cookies and redirects is described in [The Dev Gateway](/turnout/concepts/gateway/). On top of that, one [front door](#the-front-door) answers by name: `http://myapp.localhost` reaches the dev server turnout started for `myapp`.

## start / stop

```bash
turnout gateway start   # background process; ports and addresses are printed
turnout gateway stop    # stop it
```

```text
Gateway started (pid 24180).
  web: http://localhost:7100
  api: http://localhost:7101
Front door: http://localhost
  web: http://web.localhost
  api: http://api.localhost
```

`start` launches the gateway detached and returns once its first port answers; only then is the pid recorded, so `status` never reports a gateway that died on the way up. Two things are refused before anything is spawned, each naming the port and the app: a port already taken by another process, and a port two apps share (`app add` and `app edit` no longer let that happen, a hand-edited catalog still can). Bindings changed with `turnout use` are picked up automatically - the gateway re-reads them on every request.

`stop` kills the recorded process. If that process is already gone - killed from outside, or died on its own - the stale record is cleared and the command succeeds, rather than failing on a pid that no longer exists.

## run

```bash
turnout gateway run     # foreground, Ctrl+C to stop
```

The same server in the foreground - handy for watching it work. `start` spawns exactly this.

## The front door

Dev servers take their port in the order they start - Vite hands out 5173, then 5174 - so "which app is on which port" is a fresh question every morning. The front door answers it by name: the gateway listens on one well-known port and routes by host, `myapp.localhost` to the dev server turnout started for `myapp`. Every name under `.localhost` resolves to loopback (RFC 6761), so nothing is added to a hosts file.

- **Port.** 80 when the machine hands it out - Windows and macOS do; Linux asks for privileges - and 7000 otherwise, in which case addresses carry the port: `http://myapp.localhost:7000`. `TURNOUT_FRONT_PORT` pins the door to a port; `0` keeps it shut. A door that cannot open is a warning at start, not a failed gateway: the per-app stand proxies are the daily flow.
- **Where the dev server is.** The port behind a name is the app's dev port: assigned from `5100-5199` on the first `turnout dev`, or pinned with `turnout app edit myapp --dev-port PORT`. `dev` hands it to the server as `PORT` and as `{port}` in the command line - see [`turnout dev`](/turnout/reference/run/).
- **Who is not running.** A name whose dev server is not answering gets a page saying so, with the command to run - not a reset connection. A name that is no app gets a 404 naming `turnout app list`.
- **Redirects and WebSocket.** A dev server redirecting to its own `localhost:PORT` is redirected back to the name; WebSocket upgrades (Vite's HMR) travel through the door too. No cookie jar on this door: the browser talks to the app by name and keeps the app's own cookies itself.
- **Browsers.** Chrome, Edge and Firefox resolve `*.localhost` themselves. Safari asks the system resolver, which on macOS does not - add `127.0.0.1 myapp.localhost` to `/etc/hosts` there.

`turnout open myapp` opens the address; `status`, `app list` and `app show` print it.

## Behavior notes

- **Cookie jar.** Stand cookies never reach the browser; they live in the gateway, in a separate jar per app+server pair. Switching stands swaps jars, so sessions survive switching. The jar is in-memory: restarting the gateway means logging in to the stand again.
- **Redirects.** Absolute `Location` headers pointing at the stand are rewritten to `localhost`, so the browser never escapes.
- **TLS.** Connections to the stand honor the server's TLS policy (`--insecure` accepts self-signed certificates). The gateway itself serves plain HTTP on localhost.
- **WebSocket.** Upgrade requests are proxied too: the gateway opens a matching connection to the stand (jar cookies attached, `wss` for https servers, TLS per policy) and pumps frames both ways.
- **Errors.** If the stand is down you get a plain-text `502` from the gateway saying which stand is unreachable - not a cryptic proxy failure.
