---
title: "status"
description: Show what turnout knows - apps, servers, bindings, gateway state.
sidebar:
  order: 2
---

```bash
turnout status
```

One command that answers "what is going on":

- the turnout version and data directory location;
- whether turnout is set up on this machine;
- configured apps, servers and groups, by count and name;
- which server each app is bound to for development;
- whether the gateway is running;
- the five most recent actions from the [journal](/turnout/concepts/journal/).

`status` never prints secrets - it only reports whether credentials exist.

```text
turnout 0.7.0
Data directory: C:\Users\me\AppData\Local\lacodda\turnout
Apps:    2 (myapp, api)
Servers: 2 (prod, staging)
Access:  saved for staging
Bindings:
  myapp -> staging
Gateway: not running
Recent:
  2026-08-08T22:07:31Z  deploy         myapp -> staging (142 files)
  2026-08-08T22:06:55Z  use            myapp -> staging
```
