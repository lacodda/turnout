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
| Create a directory | `mkdir -p dir` | `(if not exist dir mkdir dir)` |
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

`turnout server show` names it on its **Shell** line. sshd's shell does not change on its own, so the answer stays valid until the server is pointed somewhere else: a new `--url` or `--host` forgets it, and the next command asks again. If the shell does change on the same machine - someone repoints `DefaultShell` - give the server the address it already has (`turnout server edit prod --host HOST`), and turnout asks again.

When the probe itself fails, turnout assumes POSIX. That is what every release before v0.7.0 assumed unconditionally, so a broken probe degrades to the old behavior instead of to a new failure.

## Deploy paths

A Windows [path](/turnout/reference/path/) holds an ordinary drive directory:

```bash
turnout path add winroot --dir "C:\inetpub\wwwroot\myapp"
turnout target add --app myapp --server prod --credential prod-deploy --path winroot
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

## Commands in a chain

A deploy joins commands with `&&` - create the backups directory, *then* pack; check the archive is there, *then* empty the directory, *then* unpack. `&&` means the same in both shells, but two `cmd.exe` rules make a bare command misbehave inside a chain:

- **An `if` owns the rest of its line.** `if not exist B mkdir B && tar ...` makes the `tar` part of the `if` - so when `B` already exists, nothing after it runs.
- **`exit /b 0` ends the whole line**, not just its own command. A tolerant `del ... & exit /b 0` after a failed `tar` reports success.

So every Windows command turnout sends is a self-contained group in parentheses, and the tolerant ones end in `(call )` - which sets the exit code to 0 without leaving - rather than `exit /b 0`. Up to v0.23 they were bare: on a Windows server every backup after the first was skipped, a restore did nothing, and a failed unpack read as success, while every test that compared command strings passed. Since v0.24 the chains run through a real `cmd.exe` in turnout's test suite, and since v0.26 through a real Windows sshd as well - see [How this is proven](#how-this-is-proven).

## A restart runs attached

On a Linux or macOS server a deploy's restart runs [detached from the SSH connection](/turnout/reference/deploy/#a-restart-that-outlives-the-connection), so a dropped connection does not kill it. On a Windows server it still runs attached: OpenSSH for Windows ends the session's whole job object when the connection closes, and leaving that takes launching the command through WMI - a separate piece of work. Keep a long restart (an image build) off a connection you might lose, or run it with `turnout exec` from a machine that stays connected.

## What cmd.exe cannot express

`cmd.exe` quoting has no escape sequence. A path containing a double quote cannot be sent as a literal at all, and a path containing `%` risks being expanded as a variable. Rather than send a command that would target a different directory, turnout refuses up front:

```
error: cannot run this on a Windows server: 'C:\100%\site' contains a
percent sign, which cmd.exe would expand as a variable
```

POSIX servers have no such limit - single-quote escaping can express any byte - so this only ever appears against a Windows target. Renaming the directory is the fix.

## What is not different

Everything that travels over SFTP rather than through a shell has always worked on Windows, and is untouched: the file-by-file upload, creating directories for the artifact tree, and the upload progress. The restart command is passed through verbatim, so write it in the server's own shell - `net stop myapp && net start myapp` rather than `systemctl restart myapp`.

It also runs **in the deploy directory**, like on a POSIX server - but getting there takes an extra step on Windows, because `cmd.exe`'s plain `cd` will not cross drives. turnout uses `cd /d` instead, so a path on `D:` is reached correctly even when the SSH session starts on `C:`.

Authentication is unchanged as well: the credential's key file, or the password in the keyring. Note that a Windows server puts an administrator's authorized keys in `%ProgramData%\ssh\administrators_authorized_keys` with strict ACLs, not in `~/.ssh/authorized_keys` - the usual reason a key that "was added" still prompts for a password. [`turnout key setup`](/turnout/reference/key/) writes to the file sshd reads and gives it the ACL sshd accepts; [SSH access](/turnout/guides/ssh-access/#preparing-a-windows-server) has the details.

## How this is proven

Every push to turnout and every release tag runs it against a real Windows server: OpenSSH Server on a GitHub Actions Windows runner, answering with `cmd.exe` as it does out of the box. A version is not released or published unless the run passes.

The run lives a stand's whole life through, and reads every result straight off the server's disk:

- `doctor --server` finds nothing in the way, `exec` gets `cmd.exe` and passes its exit code through;
- a deploy as one archive into a directory whose path has a space, with the restart run in that directory; `--diff`, hashing the server's files there; a deploy file by file, with `--backup`;
- `backup`, `restore --list`, a restore of a stand broken by hand, `backup prune`;
- a deploy that fails its health check and is rolled back; `rollback --to` an earlier deploy, removing the file a later one brought; a deploy into a `--clear`ed directory;
- `key setup` for two administrators in turn, both keys in the shared `administrators_authorized_keys` and both signing in, and for an ordinary account, whose key goes to its own profile.

The workflow is [`windows-server.yml`](https://github.com/lacodda/turnout/blob/main/.github/workflows/windows-server.yml), the test [`tests/windows_server.rs`](https://github.com/lacodda/turnout/blob/main/tests/windows_server.rs). Its first runs found what no test without a real sshd could: the shared key file was restricted to the first administrator who set up a key, locking out every other one; an ordinary account's file was granted to a name Windows OpenSSH builds as `WORKGROUP\name`, which resolves to nobody; `restore --list --json` and `backup prune --json` printed a progress line in front of their JSON; and the shell turnout had learned was invisible and outlived a change of address. It also met Windows Server 2025's `AllowGroups` - see [preparing a Windows server](/turnout/guides/ssh-access/#who-may-sign-in-at-all).
