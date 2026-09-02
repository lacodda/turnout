---
title: The SSH agent
description: Signing in with a key an agent already holds unlocked - one passphrase per session instead of one per command.
sidebar:
  order: 4
---

A private key protected by a passphrase is the sensible way to keep a key, and the annoying way to use one: every single sign-in asks for it. An **SSH agent** is the program that solves this. It holds the unlocked key in memory and signs on its behalf, so the passphrase is typed once per login session rather than once per command.

turnout speaks to whatever agent is already running. It does not start one, hold keys of its own, or ask for a passphrase - if `ssh` on this machine can use the agent, so can turnout.

## Using it

Set a [credential](/turnout/reference/credential/) to `agent` auth:

```bash
turnout credential add work --user deploy --auth agent
turnout credential edit prod-deploy --auth agent      # switch an existing one
```

There is nothing else to configure. An `agent` credential has no key file and no stored secret, because the key never leaves the agent:

```bash
turnout pass set work        # refused: the agent holds the key, not turnout
```

Add the key to the agent the ordinary way, and check that the server takes it:

```bash
ssh-add ~/.ssh/id_ed25519    # once per login session
turnout key check prod --credential work
```

## Where the agent lives

Which door the agent answers is a property of the machine, not of turnout, so it tries the ones that platform actually uses.

| Platform | How turnout reaches the agent |
| --- | --- |
| Linux, macOS | The Unix socket named by `SSH_AUTH_SOCK` |
| Windows | Pageant first, then the OpenSSH agent service on its named pipe |

On Windows both are common - Pageant comes with PuTTY and is what Git for Windows tends to talk to, while the OpenSSH agent is a Windows service - so turnout tries Pageant and falls back to the service rather than making you declare which one you run.

Starting an agent, if none is:

```bash
eval $(ssh-agent)            # Linux, macOS
```

```powershell
Start-Service ssh-agent      # Windows, once, as administrator
```

## Several keys at once

An agent usually holds more than one key, and a server authorizes at most one of them. turnout offers the agent's keys in turn and stops at the first the server accepts - the same thing `ssh` does. Holding four keys and having the right one third is normal and costs nothing.

## When it does not work

The three failures are told apart on purpose, because each is fixed somewhere different:

- **No agent is running.** turnout names the way to start one on this platform, rather than reporting a connection problem.
- **The agent is running but holds no keys.** The fix is `ssh-add` on this machine - nothing about the server is wrong, so nothing points there.
- **The agent holds keys and the server accepts none of them.** The message says how many keys were offered and names them, so it is clear the agent was reached and the missing piece is authorization on the server.

## See also

- [credential](/turnout/reference/credential/) - where `auth = agent` is set
- [key](/turnout/reference/key/) - authorizing a key file on a server
- [pass](/turnout/reference/pass/) - secrets for the other two auth kinds
