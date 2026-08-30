---
title: "self-update"
description: Replace turnout with the latest release, or find out which command actually updates your installation.
---

Updates turnout itself to the newest release.

```bash
turnout self-update
```

```text
turnout 0.10.4 is available (you have 0.10.3).
It will replace /home/dev/.local/bin/turnout
Update now? yes
Updated to turnout 0.10.4
Alias /home/dev/.local/bin/tn updated too.
```

| Flag | Short | Effect |
| --- | --- | --- |
| `--yes` | `-y` | Skip the confirmation prompt |
| `--force` | `-f` | Replace the binary even when a package manager owns it |

It exits without doing anything when you are already on the latest release.

## Installations it does not touch

Only installations that came from a release archive - the [installers](/turnout/getting-started/) or a manually unpacked release - are replaced in place. When cargo, npm or a system package manager put turnout there, that tool keeps its own record of what is installed, and overwriting the file behind its back would leave it convinced the old version is still present.

Those cases print the command that actually updates them:

```text
$ turnout self-update
turnout was installed with cargo, which keeps its own record of what is installed.

Update it with:
  cargo install turnout --force
```

`--force` replaces the binary anyway. It exists for the case where the guess is wrong, not as the normal path - if the guess was right, the package manager's records go stale.

## Replacing a running binary

The executable being replaced is the one currently running. Windows keeps it locked and refuses the write, so the outgoing binary is renamed to `turnout.exe.old` first - a rename is allowed while the image is mapped - and the new one takes its place.

That leftover is deleted by the next turnout command you run, whatever it is, since by then it is no longer the running image. Unix does not need the dance, but follows the same path so there is one behaviour to reason about.

If installing the new binary fails partway, the old one is moved back: the command never leaves you without a working turnout.

## The `tn` alias

The [short alias](/turnout/getting-started/#the-tn-alias) is a link to the binary rather than a copy of it, so both names always answer with the same code - there is nothing to keep in sync, and an install costs one binary instead of two.

The swap above breaks that link, though: it renames the file the alias points at. So the alias is pointed back at the new binary as the last step of an update, and the command says so when it does.

An install without an alias does not grow one - `TURNOUT_NO_ALIAS=1` is a choice the update has no business overruling. And if the relink fails, the command says that too, because a stale alias keeps answering under its own name:

```text
Warning: /home/dev/.local/bin/tn still points at the previous version and could not be relinked (permission denied).
Re-run the installer to fix it.
```

## Updating while the gateway or a dev server is running

You do not have to stop anything first. Both `turnout gateway start` and `turnout dev` leave a second turnout process running from the same executable, and the update succeeds anyway - for the same reason as above. The lock is on the file, not on the name, so the running process keeps executing the image that was renamed out of the way.

What it does mean is that **already-running processes keep running the old code**. They were loaded into memory before the swap and nothing reaches back into them:

| Running | After `self-update` | To pick up the new version |
| --- | --- | --- |
| `turnout gateway` | Keeps serving, on the old code | `turnout gateway stop && turnout gateway start` |
| `turnout dev` | Keeps running, on the old code | Stop it (Ctrl+C) and start it again |

Nothing breaks either way - a restart is only needed when the new release changes something you actually want in those processes.

One consequence worth knowing: while the old image is still executing, its `.old` file cannot be deleted. The sweep skips it silently and removes it after the process has exited.

## Where the release comes from

The tag is read from the `/releases/latest` redirect on github.com and the matching archive is downloaded from that release - the same source the installers use, and deliberately not `api.github.com`, whose unauthenticated rate limit is shared by everyone behind your IP address. See [the update check](/turnout/concepts/update-check/) for the rest of that reasoning.

Prebuilt binaries exist for x86-64 Windows, x86-64 Linux and Apple Silicon macOS. On any other platform the command says so and points at `cargo install turnout --force`.
