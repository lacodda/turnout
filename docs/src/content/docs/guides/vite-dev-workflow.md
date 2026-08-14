---
title: Daily Development with a Vite App
description: Set up a Vue 3 + Vite project once, then switch between backend stands with one command.
sidebar:
  order: 1
---

This guide walks through the full path for a typical frontend setup: a Vue 3 + Vite app whose API lives on two stands - `https://api.example.com` and `https://api2.example.org`. The goal: switching stands becomes one command, and your project's env files never change again.

## One-time setup

### 1. Describe the stands

```bash
turnout server add main --url https://api.example.com --label "Main API"
turnout server add second --url https://api2.example.org
```

Add `--insecure` for a stand with a self-signed certificate.

### 2. Save access (optional)

```bash
turnout credential add main-login --user myname   # who logs in
turnout pass set main-login                       # hidden secret, into the OS keyring
```

Later `turnout pass copy main-login` puts the password on your clipboard whenever the stand asks for it.

### 3. Register the app

From the project directory:

```bash
cd ~/dev/myshop
turnout app add
```

The wizard detects the package manager from the lock file, proposes the standard commands (`pnpm dev`, `pnpm build`, ...), suggests a free gateway port - say **7100** - and lets you pick the allowed servers. The same, scripted:

```bash
turnout app add myshop --path . --port 7100 --server main --server second
```

### 4. Point the app at the gateway - once and forever

This is the core idea: the project knows only its gateway port, never a stand address. In `.env.development` (safe to commit - it never changes again):

```ini
VITE_API_URL=http://localhost:7100
```

```ts
// src/api.ts
import axios from "axios";

export const api = axios.create({ baseURL: import.meta.env.VITE_API_URL });
```

If your stand is strict about CORS or the `Origin` header, proxy through Vite instead - requests become same-origin:

```ts
// vite.config.ts
export default defineConfig({
  server: {
    proxy: { "/api": "http://localhost:7100" },
  },
});
```

with `VITE_API_URL=/api`. Either way, only `localhost` appears in the project.

## Every day

```bash
turnout use myshop main     # which stand to work against
# 'myshop' now uses 'main'.
# Stand check: https://api.example.com responded with 200 OK.

turnout gateway start       # once per workday
#   myshop: http://localhost:7100

turnout dev                 # from anywhere inside the project
# [myshop] pnpm dev  ->  Vite on http://localhost:5173
```

Open `localhost:5173`: the UI is local, every API call travels through `localhost:7100` to the stand. Log in to the stand - its session cookie never reaches your browser; it lands in the gateway's jar for the `myshop`+`main` pair.

### Switch stands

```bash
turnout use myshop second
```

That's it. No Vite restart, no env edits: the very next request goes to `api2.example.org`, because the gateway re-reads the binding on every request. Need the password for the other stand? `turnout pass copy second` - it is on the clipboard.

And when you come back with `turnout use myshop main`, you are **still logged in**: the `main` cookies were waiting in their own jar.

### See the whole picture

```text
$ turnout status
turnout 0.9.0
Data directory: C:\Users\me\AppData\Local\lacodda\turnout
Apps:    1 (myshop)
Servers: 2 (main, second)
Creds:   1 (main-login)
Paths:   none yet
Bindings:
  myshop -> second
Gateway: running (pid 18324; myshop:7100)
```

## What the gateway handles for you

- Stand redirects (`Location: https://api.example.com/...`) come back rewritten to `http://localhost:7100/...` - the browser never escapes to the stand.
- WebSocket connections are proxied the same way (`wss` for https stands), with the same jar cookies.
- A stand that is down answers with a clear `502: stand 'main' is unreachable` instead of a cryptic proxy error.
- If the gateway itself restarts, the only cost is logging in to the stand again - jars live in the process memory.

## When a second app appears

Give it its own port (`--port 7101`), then group the contour and switch everything at once:

```bash
turnout group add front --app myshop --app admin
turnout use front second
```
