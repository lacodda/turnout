---
title: turnout self-update
description: Replace turnout with the latest release, or find out which command actually updates your installation.
---

Updates turnout itself to the newest release.

```bash
turnout self-update
```

```text
turnout 0.5.0 is available (you have 0.4.1).
It will replace /home/dev/.local/bin/turnout
Update now? yes
Updated to turnout 0.5.0
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

## Where the release comes from

The tag is read from the `/releases/latest` redirect on github.com and the matching archive is downloaded from that release - the same source the installers use, and deliberately not `api.github.com`, whose unauthenticated rate limit is shared by everyone behind your IP address. See [the update check](/turnout/concepts/update-check/) for the rest of that reasoning.

Prebuilt binaries exist for x86-64 Windows, x86-64 Linux and Apple Silicon macOS. On any other platform the command says so and points at `cargo install turnout --force`.
