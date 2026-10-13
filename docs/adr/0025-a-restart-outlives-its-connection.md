# ADR 0025: A restart outlives the connection that started it

- Status: accepted
- Date: 2026-10-13

## Context

A path's post-write command can be the longest part of a deploy: `docker compose up -d --build` rebuilds an image for minutes. Run as a plain SSH exec it belongs to the session. A field report from lyrid: the connection went half-way through a rebuild, the rebuild went with it, and the stand kept serving the old image off new sources. `nohup` did not save it - compose's children stayed in the session and the hangup reached them. `setsid` did. v0.11 made the failure say which side of the upload it fell on; the command itself still died.

## Decision

**On a POSIX server the command starts in a session of its own**, with `setsid` (`nohup` where there is none, as on macOS), in the deploy directory, run by the account's login shell exactly as sshd ran it before. Its output and then its exit code go to `~/.turnout/runs/ID.log` and `ID.code` - the home directory, the one place an account can always write; the deploy directory may be served to the web, and its parent is usually root's. The code is written to a side file and renamed, so a reader never sees half of it.

**turnout reads the run back by polling** the code, then the log from the last byte it has. The connection is a window, not the run's owner: when it drops, turnout dials again for up to a minute and reads on; when it cannot, the error says the command is still running and where its output is. A finished run removes its files. `restore` runs the command the same way, and in the deploy directory - until now it ran in the home directory.

**A Windows server runs the command attached, as before.** OpenSSH for Windows ends the session's job object on disconnect; leaving it takes launching through WMI, which is its own piece of work for the day a Windows stand runs something long.

## Consequences

- A lost connection no longer kills a rebuild half-way.
- A restart costs a launch and a few polls more than one exec - round trips on an open connection, a fraction of a second for a quick command.
- The command's output reaches the terminal as it is written, a line beside the spinner, and whole at the end, as before.
- A run that turnout could not follow to its end leaves its log on the server, named in the error.
