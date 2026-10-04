---
title: "cookie"
description: Hand the gateway a stand's session from a browser - for stands you can only log in to there.
sidebar:
  order: 7.2
---

```bash
turnout cookie set [APP] [--server SERVER]
turnout cookie list [--json]
turnout cookie clear [APP] [--server SERVER]
```

The [gateway](/turnout/concepts/gateway/#cookie-jar-per-appserver) keeps the stands' cookies for your apps, so a login made through it survives every switch. Usually that login happens in the app itself, through the gateway, and nothing else is needed.

Some stands do not allow that. Their login is a single sign-on page on another domain, which sends you back to the stand's own address - not to `localhost` - and the session lands in the browser instead of the gateway. Those stands are logged in to in a browser only. `cookie set` carries the session over.

## set

1. Log in to the stand in your browser, at its own address.
2. Open the developer tools, the **Network** tab, and reload. Pick any request to the stand.
3. Copy it - either the value of the **`Cookie`** request header, or the whole request with **Copy as cURL** (bash or cmd) or **Copy as PowerShell**.
4. Run:

```bash
turnout cookie set myapp
```

```text
The gateway now sends 3 cookies to 'staging' for 'myapp': csrftoken, lang, sessionid.
```

The next request `myapp` makes through the gateway carries that session.

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | The stand the cookies belong to (defaults to the app's current binding) |

`APP` defaults to the app of the current directory. The text is read from standard input when something is piped in, and from the clipboard otherwise:

```bash
pbpaste | turnout cookie set myapp                   # macOS
Get-Clipboard | turnout cookie set myapp             # PowerShell
turnout cookie set myapp < copied-request.txt
```

`set` replaces what the gateway held for that app on that stand - the browser's `Cookie` header is the whole session, and an old cookie mixed into a new session is how sessions break. A `Set-Cookie` header (what a server *answers* with) is refused: it carries attributes, not the session a request sends.

The cookies go to the running gateway's memory and nowhere else: not to a file, not to the [journal](/turnout/concepts/journal/) (which records that a `cookie.set` happened, never the values), never back out. A restarted gateway has forgotten them, as it forgets every session - set them again. If the gateway is not running, `set` says so and changes nothing.

## list

```bash
turnout cookie list
```

```text
myapp -> staging  csrftoken, lang, sessionid
admin -> staging  sessionid
```

Which apps the gateway holds cookies for, and the cookies' names - never their values. `--json` prints `[{"app", "server", "cookies": [names]}]`.

## clear

```bash
turnout cookie clear myapp
```

Forget the app's cookies for its stand: the next request there is logged out. `--server` picks another stand than the current one.

## When the session lapses

When a stand refuses the cookies the gateway holds, the gateway [says so](/turnout/concepts/gateway/#when-a-stand-misbehaves): a line in `turnout status`, `prod login` in [the prompt line](/turnout/guides/prompt-line/), and one desktop notification that names this command. A fresh `cookie set` - or a login through the gateway - clears it.

## How it reaches the gateway

The gateway listens on one more loopback port, chosen by the system, for requests about itself; its port and a random token are in the gateway's [job record](/turnout/concepts/background-jobs/), which only your user can read. Every request must carry the token, so a web page - which can send requests to a `localhost` port but cannot read your files - cannot plant a session in the gateway or read which ones it holds. A gateway started by turnout before 0.22 has no such port; restart it once.
