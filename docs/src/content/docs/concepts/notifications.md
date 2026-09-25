---
title: Notifications
description: How a background job tells you it is done - what it says, where a click takes you, and what each desktop allows.
---

A job in your terminal speaks there. A [job in the background](/turnout/concepts/background-jobs/) has no terminal, so when something happens that you would want to know, it tells you on the desktop - once, with somewhere to go:

| What happened | The notification | A click opens |
| --- | --- | --- |
| A dev server (or any `run` command) came up | `myapp is ready` - its address and how long it took | The address: the app's front door, or the one the server printed |
| A build, test, lint or custom command finished | `myapp build finished` - how long it took | Its log |
| A deploy went out | `myapp deployed` - the stand and how long it took | The stand's address |
| Anything failed | `myapp build failed` - the line of output most likely to say why, the exit code, and the command that shows the rest | The log of that failure, [kept aside](/turnout/concepts/background-jobs/#a-failure-is-kept) |
| A server that had come up ended by itself | `myapp stopped` - how long it ran | Its log |

The line quoted for a failure is the last one of the output that mentions an error, leaving out what npm, yarn or pnpm add about the script afterwards - `npm error code 2` is true and never the reason.

## What does not send one

- **A job in your terminal.** You are looking at its result already; the terminal is where it speaks.
- **A job you stopped.** `turnout stop` takes the supervisor down with the job, and you know why it ended.
- **Progress.** One notification when the outcome is known, never one per step.

A job that fails within its first moment is reported in the terminal that started it *and* in a notification: `--detach` waits most of a second for exactly those, and the notification is how every background job ends, whether or not somebody was still watching.

## Where a click goes, per desktop

Most carry buttons too, for the places next to the one a click opens: **Open in browser** where there is an address, **Show log**, and **Show folder** for the app's project directory. How much of that a desktop lets through differs, and this is the honest table:

| | The notification | Click and buttons | After turnout exited |
| --- | --- | --- | --- |
| **Windows 10/11** | Under turnout's own name and icon | Both work | Yes - also from the notification centre, hours later |
| **Linux** (GNOME, KDE, anything speaking the freedesktop protocol) | Under turnout's name, with its icon | Both work where the notification server supports actions - GNOME and KDE do | For 15 minutes after the job ended |
| **macOS** | Under the name macOS gives command-line tools | Neither: the text names the command to run instead | - |

**Windows.** The click is handled by Windows itself, so it works however long ago the job ended. A log opens the way a double-click on it would: with the program your system has for `.log` files - if it has never been asked which one, it asks now. turnout registers itself as a sender of notifications for your user (`HKEY_CURRENT_USER\Software\Classes\AppUserModelId\lacodda.turnout`), which is what puts its name and icon on the notification - and what lets you switch turnout's notifications off on their own in *Settings → System → Notifications*.

**Linux.** A click travels back over D-Bus to the process that showed the notification, and a process that has exited cannot open anything. So the background supervisor stays around after its job - 15 minutes at most - to answer a click, then goes. Without a notification server (a headless box, an SSH session) there is nothing to show on; the job's log says so, and the job is not affected.

**macOS.** The notification centre reports a click only to an application's main event loop, which a command-line tool does not run. The notification shows; the command in it - `turnout logs myapp build --failed` - is the way to the log.

## Turning them off

Through the desktop, not through turnout: *Settings → System → Notifications* on Windows (turnout has an entry of its own), the notification settings of your desktop on Linux, *System Settings → Notifications* on macOS. A notification is only sent for jobs in the background, so a turnout that never detaches never sends one.
