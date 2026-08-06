---
title: turnout status
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
- configured apps and servers, by count and name;
- which server each app is bound to for development *(arrives with the gateway)*;
- whether the gateway is running.

`status` never prints secrets - it only reports whether credentials exist.

```text
turnout 0.1.0
Data directory: C:\Users\me\AppData\Local\lacodda\turnout
Apps:    2 (myapp, api)
Servers: 2 (prod, staging)
Access:  saved for staging
Gateway: not running
```
