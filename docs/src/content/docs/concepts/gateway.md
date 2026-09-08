---
title: The Dev Gateway
description: How turnout routes local apps to remote stands without touching their env files.
sidebar:
  order: 2
---

The core idea of turnout:

> Local apps always talk to `localhost` (their gateway port).
> The gateway routes requests to whichever stand the app is currently bound to.

Switching a stand is not editing `.env` files across repositories - it is one command that changes the app→server binding in one place. Your projects' env files stay stable and committable, pointing at `localhost` forever.

## How the app learns the address

The port is written once, in the app's turnout config. The app receives the address by two roads that back each other up (details on the [`turnout app`](/turnout/reference/app/#how-the-app-learns-the-gateway-address) page):

- every command turnout starts for the app - `dev`, a custom `run` - carries it in an environment variable whose name the app chooses (`VITE_API_URL`, `REACT_APP_API_URL`, ...);
- a dotenv file turnout keeps in step - `.env.development.local` by default - covers the app when it is started from an IDE or by hand.

The file is read by Vite and CRA in development only, so a production build never sees `localhost`, and the project's own `.env` is never touched.

## Cookie jar per app+server

The browser talks only to the gateway and holds only the gateway's own session. Cookies issued by stands are kept inside the gateway, in a separate jar for every app+server pair.

This is what makes switching seamless:

- switch from `staging` to `prod-eu` and back - your `staging` login session is still alive;
- stand cookie domains and login redirects never leak into the browser;
- no more "why am I suddenly logged out" after touching env files.

## What the gateway handles

Real stands are messy, so the gateway is built for:

- **HTTPS with self-signed certificates** - TLS verification is configured per server;
- **redirects** - `Location` headers are rewritten so the browser never escapes to the stand's real address;
- **WebSocket** and live streams;
- **clear errors** - if a stand is down, you see "stand unreachable", not a cryptic proxy failure.

Response bodies are not rewritten: apps are expected to use relative URLs for API calls, which is the norm for SPA setups.

## Daily flow

```bash
turnout gateway start       # once per workday
turnout use myapp staging   # bind the app to a stand
turnout use myapp prod-eu   # switch - no restarts, no env edits
```

See [`turnout use`](/turnout/reference/use/) and [`turnout gateway`](/turnout/reference/gateway/) for details.
