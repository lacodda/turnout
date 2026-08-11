---
title: The update check
description: How turnout notices a new release without ever making you wait - and how to switch it off.
---

Once a day turnout looks up its own latest release and, if you are behind, says so at the end of a command:

```text
turnout 0.5.0 is available (you have 0.4.1).
  Update with `turnout self-update`
```

That is the whole feature. It never interrupts, never prompts, and never changes what a command does. Acting on it is [`turnout self-update`](/turnout/reference/self-update/).

## It does not slow anything down

The lookup does **not** happen while you wait. A command that finds the cached answer stale spawns a detached background process and exits immediately; that process does the network call and writes the result to `update-check.json` in the [data directory](/turnout/getting-started/).

The consequence is worth stating plainly: the notice you see describes what the *previous* run found. A brand-new installation says nothing on its first command, and tells you about an update on the next one. That is the price of never adding latency to a command you actually asked for.

## When it stays quiet

- **More often than once a day.** The cached answer is reused; only a stale cache triggers a lookup.
- **Output is piped.** The notice goes to stderr and only when that is a terminal, so it never lands in the middle of something a script is parsing.
- **Nothing is newer.** Versions are compared numerically, so `0.10.0` correctly beats `0.9.0`, and a final release beats its own release candidate.
- **The lookup failed.** No network, no DNS, a proxy in the way - the attempt is recorded and retried tomorrow, not on every command. Nothing is printed and no command fails because of it.

## Turning it off

Set `TURNOUT_UPDATE_CHECK` to `0`, `false`, `no` or `off`:

```bash
export TURNOUT_UPDATE_CHECK=0
```

Nothing is then looked up, cached or printed. The check also switches itself off when `CI` is set, where the notice has no reader and every run starts from a clean machine.

## Where the version comes from

The tag is read from the `Location` header of the `/releases/latest` redirect on github.com - not from `api.github.com`, whose unauthenticated calls are capped at 60 per hour **per IP address**. Behind a shared address that cap is reached by other people, and a check that fails for everyone at once is worse than no check. A redirect that does not end in a version tag is ignored rather than reported as a release.

Nothing is sent along with the request beyond a `turnout/<version>` user agent: no identifiers, no telemetry, no record of which apps or servers you have.
