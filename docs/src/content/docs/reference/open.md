---
title: "open"
description: Open an app in the browser by its name.
sidebar:
  order: 8.5
---

```bash
turnout open [APP]
```

Opens `http://APP.localhost` - the app's address behind the gateway's [front door](/turnout/reference/gateway/#the-front-door) - in the default browser. The app is resolved the same way `dev` resolves it: by name, or from the current directory.

The gateway has to be running: the door is part of it. When the door opened on a port other than 80 (Linux without privileges, or 80 taken), the address carries that port - `http://myapp.localhost:7000` - and `open` knows which one from the gateway's record.

An app that has not been started through turnout yet has no dev server behind its name; the page says so and names the command to run.

```bash
turnout open myapp
# Opening http://myapp.localhost
```
