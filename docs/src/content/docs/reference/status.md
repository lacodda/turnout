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
- configured apps and servers *(catalogs are under development)*;
- which server each app is bound to for development;
- whether the gateway is running.

`status` never prints secrets - it only reports whether credentials exist.
