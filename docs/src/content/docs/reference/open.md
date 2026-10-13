---
title: "open"
description: Open a target's stand, or an app by its name, in the browser.
sidebar:
  order: 8.5
---

```bash
turnout open [TARGET | APP]
```

Opens a page in the default browser: a [target](/turnout/reference/target/)'s stand, or an app behind the gateway's front door. A name is tried as a target first, the way [`deploy`](/turnout/reference/deploy/) reads it.

## A target

```bash
turnout open myapp-prod
# Opening https://admin.example.com
```

Opens where the target's deploy is seen: its own address when it has one (`turnout target edit myapp-prod --url URL`), otherwise its server's URL. The natural next step after a deploy - which can also do it itself with `turnout deploy myapp-prod --open`.

## An app

```bash
turnout open myapp
# Opening http://myapp.localhost
```

Opens `http://APP.localhost` - the app's address behind the gateway's [front door](/turnout/reference/gateway/#the-front-door). The app is resolved the same way `dev` resolves it: by name, or from the current directory.

The gateway has to be running: the door is part of it. When the door opened on a port other than 80 (Linux without privileges, or 80 taken), the address carries that port - `http://myapp.localhost:7000` - and `open` knows which one from the gateway's record.

An app that has not been started through turnout yet has no dev server behind its name; the page says so and names the command to run.
