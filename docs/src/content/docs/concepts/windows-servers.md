---
title: Windows servers
description: How turnout talks to a server that answers SSH with cmd.exe - shell detection, paths, archives and the limits of cmd quoting.
sidebar:
  order: 3
---

turnout deploys to Windows servers the same way it deploys to Linux ones: `turnout deploy`, `--backup`, `--clear`, `turnout restore`. What changes is invisible from the command line - the shell on the other end, and the language turnout has to speak to it.

## Which shell answers

OpenSSH on Windows hands your command to **`cmd.exe`**, unless an administrator has changed sshd's `DefaultShell`. That matters more than it sounds: `cmd.exe` and a POSIX shell disagree about nearly every character that makes a command work.

| | POSIX (`sh`) | `cmd.exe` |
| --- | --- | --- |
| Quoting | `'one arg'` | `"one arg"` - single quotes are ordinary characters |
| Create a directory | `mkdir -p dir` | `if not exist dir mkdir dir` |
| Variables | `$VAR`, `$(command)` | `%VAR%`, no command substitution |
| Empty a directory | `find dir -mindepth 1 -delete` | `del /f /q /s` plus `rd /s /q` |

Sending POSIX syntax to `cmd.exe` does not usually fail loudly. It does something else quietly - a path wrapped in single quotes becomes a *different path*, one that includes the quote marks.

## How turnout finds out

On the first command that needs a shell, turnout asks:

```
echo %COMSPEC%
```

This is the one question both shells answer without choking. `cmd.exe` expands `%COMSPEC%` into the path to itself; a POSIX shell has no `%` expansion and echoes the text back unchanged. The reply decides the dialect, and every command turnout sends afterwards is phrased in it.

The answer is stored on the server entry, so it costs one round trip once rather than one per command:

```json
{
  "name": "prod",
  "shell": "windows"
}
```

sshd's shell does not change on its own, so the cached answer stays valid. If it ever does change - someone repoints `DefaultShell` - remove the `shell` field from the server entry to have it probed again.

When the probe itself fails, turnout assumes POSIX. That is what every release before v0.7.0 assumed unconditionally, so a broken probe degrades to the old behavior instead of to a new failure.

## Deploy paths

A Windows [path](/turnout/reference/path/) holds an ordinary drive directory:

```bash
turnout path add winroot --dir "C:\inetpub\wwwroot\myapp"
turnout server edit prod --deploy-path myapp=winroot
```

Drive letters, backslashes and UNC shares (`\\fileserver\share\myapp`) are all accepted. Spaces are fine too - every value is quoted before it reaches the server.

There is one shape turnout still refuses, and it is worth knowing why. Git Bash rewrites POSIX-looking arguments into local paths before the program sees them, so typing `/var/www/myapp` there can arrive as `C:/Program Files/Git/var/www/myapp` - a directory nobody chose. turnout recognizes that by the Git installation prefix spliced in front, not by the drive letter, and says so:

```
error: 'C:/Program Files/Git/var/www/myapp' looks like Git Bash rewrote
'var/www/myapp' into a local path before turnout saw it.
```

The ways out are in the message: prefix with `MSYS_NO_PATHCONV=1`, double the leading slash (`//var/www/myapp`), or use PowerShell. A genuine Windows path like `C:\inetpub\myapp` carries no such prefix and is taken at face value.

## Archives

Windows has shipped `tar.exe` (bsdtar) in `System32` since Windows 10 1803, and it reads `.tar.gz`. So the fast path - pack the artifacts locally, send one file, unpack on the server - works on Windows exactly as it does on Linux, and no second archive format is involved.

Backups are `.tar.gz` too, named `YYYYMMDD-HHMMSS.tar.gz`. The timestamp is generated **locally**: the old command asked the server for the time with `$(date +%Y%m%d-%H%M%S)`, which `cmd.exe` would have taken literally, naming the archive `$(date`. Restoring picks the newest backup by sorting those names, also locally, because `ls -1 | sort | tail -n 1` is a POSIX pipeline with no `cmd.exe` equivalent.

## What cmd.exe cannot express

`cmd.exe` quoting has no escape sequence. A path containing a double quote cannot be sent as a literal at all, and a path containing `%` risks being expanded as a variable. Rather than send a command that would target a different directory, turnout refuses up front:

```
error: cannot run this on a Windows server: 'C:\100%\site' contains a
percent sign, which cmd.exe would expand as a variable
```

POSIX servers have no such limit - single-quote escaping can express any byte - so this only ever appears against a Windows target. Renaming the directory is the fix.

## What is not different

Everything that travels over SFTP rather than through a shell has always worked on Windows, and is untouched: the file-by-file upload, creating directories for the artifact tree, and the upload progress. The restart command is passed through verbatim, so write it in the server's own shell - `net stop myapp && net start myapp` rather than `systemctl restart myapp`.

Authentication is unchanged as well: the credential's key file, or the password in the keyring. Note that a Windows server puts an administrator's authorized keys in `%ProgramData%\ssh\administrators_authorized_keys` with strict ACLs, not in `~/.ssh/authorized_keys` - the usual reason a key that "was added" still prompts for a password.
