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
turnout use myapp staging   # bind the app to a stand
turnout dev myapp           # UI runs locally, API goes to staging
turnout use myapp prod-eu   # tomorrow: switch - no restarts, no env edits
```

*(The `use`, `dev` and gateway commands are under development - see the [roadmap](https://github.com/lacodda/turnout#roadmap).)*
