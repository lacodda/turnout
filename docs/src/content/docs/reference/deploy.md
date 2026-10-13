---
title: "deploy"
description: Deploy an app to a server over SSH/SFTP - build, upload, restart, check, and roll back what fails.
sidebar:
  order: 9
---

```bash
turnout deploy [TARGET...] [-s SERVER] [-C CREDENTIAL] [-p PATH] [-n] [-b] [-c] [-A] [--diff [--json]] [-o] [-v | -d | --foreground]
```

Builds the app, uploads the artifacts to the target directory over SFTP, restarts the service and - when the target says how - checks that the stand works, rolling it back when it does not. One command, using the same apps, servers, credentials and paths as everything else.

## Options

| Flag | Meaning |
| --- | --- |
| `-s, --server <SERVER>` | Server for this run only, overriding the target's |
| `-C, --credential <NAME>` | Credential for this run only, overriding the target's |
| `-p, --path <NAME>` | Named path for this run only, overriding the target's |
| `-n, --no-build` | Skip the build step and upload what is already built |
| `-b, --backup` | Archive the remote directory before touching it |
| `-c, --clear` | Empty the remote directory before uploading |
| `-A, --no-archive` | Upload file by file instead of packing the artifacts into one archive |
| `--diff` | Show what the deploy would change on the server, and write nothing - see [Before writing](#before-writing---diff) |
| `--json` | With `--diff`: print the comparison as JSON |
| `-o, --open` | Open the stand in the browser once the deploy is through |
| `-v, --verbose` | Stream the build's output in full instead of hiding it behind a loader |
| `-d, --detach` | Deploy in the background and give the terminal back at once - see [In the background](#in-the-background) |
| `--foreground` | Stay in this terminal even when `TURNOUT_DETACH` covers `deploy` |

## What happens

1. **Resolve.** `TARGET` is a [target](/turnout/reference/target/) name (`myapp-prod`) or an app name (`myapp`, using that app's target on its current [binding](/turnout/reference/use/)). Name several to deploy them in one go - see [Several targets](#several-targets). With no argument, the app comes from the current directory and the server from its binding; if that pair has a target, it is used. If it does not, turnout asks which path to deploy into on a terminal, then offers to save the answer as a target so it is asked once rather than every time. `--server`, `--credential` or `--path` override the resolved target's fields for this run only.
2. **Build.** The app's `build` command runs (skip with `--no-build`); a failing build aborts the deploy, printing the output that explains why. While it succeeds the build shows a loader and its elapsed time, the same way [`turnout build`](/turnout/reference/run/) does - see [the quiet console](/turnout/concepts/quiet-console/). `--verbose` streams it instead.
3. **Plan.** The artifact directory is walked to count files and bytes, so the upload can report a percentage and an ETA. An empty artifact directory aborts the deploy before anything is sent. A deploy that did not build checks whether a source changed since the last build - see [A build older than its sources](#a-build-older-than-its-sources).
4. **Connect.** SSH to the server's host and port as the credential's user, with the credential's key file or the password stored in the keyring under its name.
5. **Backup** *(with `--backup`, or when the target has a health check)*: the remote directory is archived first - see [`turnout backup`](/turnout/reference/backup/). A first deploy, whose directory is not there yet, has nothing to back up.
6. **Clear** *(only with `--clear`)*: the target directory is emptied.
7. **Upload.** The app's artifact directory (`--dist`) is copied to the path's directory - as one archive when that is faster, otherwise file by file. See [How the upload travels](#how-the-upload-travels).
8. **Restart.** If the path has a post-write command, it runs on the server **in the deploy directory** and its output is shown - a command does not need its own `cd` to reach the files it just received. It runs detached from the SSH connection, so a dropped connection does not kill it - see [A restart that outlives the connection](#a-restart-that-outlives-the-connection).
9. **Check** *(when the target has a health check)*: the stand is asked until it works or the wait runs out; if it does not, the deploy is **rolled back** - see [Health check and rollback](#health-check-and-rollback).

:::note[Interrupted after the upload]
A deploy that fails between the upload and the restart says so explicitly: the files are already on the server, but the service is still running what it had. Run `turnout deploy` again, or restart by hand if the new files are already good enough.
:::

## Health check and rollback

A deploy that uploaded and restarted has not yet shown that the stand works. A target with a [health check](/turnout/reference/target/#health-check) shows it, and a deploy that cannot is put back the way it was:

```bash
turnout target edit myapp-prod --health /health
turnout deploy myapp-prod
```

```
◇ Connected to deploy@prod.example.com:22
◇ Backup 20261013-101500.tar.gz kept for a rollback until the check passes
◇ Packed 214 files → 6.10 MiB
◇ Uploaded 6.10 MiB in 3.4s · 1.81 MiB/s
◇ Unpacked 214 files into /var/www/myapp
◇ Restarted: docker compose up -d
▲ the health check failed: GET https://prod.example.com/health answered 502 after 60s of asking
◇ Rolled back /var/www/myapp to 20261013-101500.tar.gz
◇ Restarted: docker compose up -d
◇ Healthy again: GET https://prod.example.com/health - 200 in 41 ms
└ Deploy of 'myapp' to 'prod' failed
error: the health check failed: ... - rolled back to 20261013-101500.tar.gz: the stand works again (200 in 41 ms)
```

- **The backup is taken for the rollback** even without `--backup`, and removed once the check passes - otherwise every checked deploy would leave an archive behind until the disk is full. With `--backup` it stays, as always.
- **A restart that fails** rolls back too: the check is the word for "it works", and a failed restart does not give it.
- **The stand is asked again** after the rollback. If the old build works, the deploy broke it; if the old build fails as well, the cause is somewhere else, and the error says so.
- **A first deploy** has nothing to roll back to - the directory was not there before it. A failed check says so and leaves the files in place.
- The deploy exits with an error either way. In the [background](#in-the-background) that is a `myapp deploy failed` [notification](/turnout/concepts/notifications/) whose line names the rollback.

## Before writing: `--diff`

```bash
turnout deploy myapp-prod --diff
```

```
◇ Compared 214 files with /var/www/myapp
│  34 to update · 2 to add · 178 unchanged · +1.21 MiB
│  update: index.html, assets/index-3f2a.js, assets/vendor-91bc.js, ..., and 26 more
│  add:    assets/report-77de.js, assets/report-77de.css
│  2 files on the server are not in the build - they stay (--clear removes them)
└ Nothing was written - deploy without --diff to make these changes
```

Builds as a deploy would (unless `--no-build`), then compares the build with what the server has - and stops. The answer comes from the files on the server themselves, not from a list turnout keeps: one written by the last deploy would be wrong the first time someone edited a file on the server by hand. Every file and its size come back in one pass, and only the files that could be the same - same path, same size - are hashed (SHA-256) on both sides, so a deploy directory full of uploads or a database volume does not slow the comparison down.

With `--clear`, files on the server that the build lacks are counted as removed. A server with neither `sha256sum` nor `shasum` is compared by size, and the report says so. `--json` prints the same comparison for a script: one object per target with `added`, `updated`, `unchanged`, `extra`, `delta` (bytes) and `compared` (`content` or `size`).

## Several targets

```bash
turnout deploy web-stage web-prod
```

Each app is built **once**, before any server is touched, and packed once: two targets of one app get the same archive. The targets go out in the order named, each with its own checklist. The first one that fails stops the rest - deploying staging, then production, must not ship to production what just failed on staging - and the ones not attempted are named.

`--server`, `--credential` and `--path` change one route, so they take a single target. In the background, the targets of one command must share an app: a [background job](/turnout/concepts/background-jobs/) belongs to one.

## A restart that outlives the connection

A path's post-write command is often the longest part of a deploy - `docker compose up -d --build` rebuilds an image for minutes. Run as a plain SSH command it belongs to the connection, and a laptop lid, a Wi-Fi hop or Ctrl+C takes it down half-way: the files are new, the image is old.

On a Linux or macOS server the command is started detached instead, with `setsid` (`nohup` where there is none), in a session of its own. Its output and exit code go to files under `~/.turnout/runs` on the server, and turnout reads them back - the newest line shows beside the spinner. The connection is only a window onto the run: when it drops, turnout dials again for up to a minute and reads on. When it cannot, the error says the command is still running and where its output is. A finished run leaves nothing behind.

The command runs in the account's login shell, in the deploy directory, exactly as before. On a Windows server it still runs attached - see [Windows servers](/turnout/concepts/windows-servers/).

## A build older than its sources

With `--no-build` - or for an app with no `build` command - a deploy sends whatever the last build left. When a source changed after that build, the deploy says so before it starts, and names the file:

```
▲ dist is older than its sources: src/App.vue changed 12m after the last build - deploy without --no-build to build it first
```

The sources are the files git tracks or would track - asked of git itself, so an ignored `.env.local` or a tool's cache is not mistaken for an edit; outside a repository the project is walked, leaving out `node_modules`, `target` and `.git`. The artifact directory and the dotenv file turnout itself writes never count. It is a warning, not a refusal: shipping a known older build is sometimes the point.

## How the upload travels

A dist directory is typically thousands of small files, and SFTP pays a round trip for every one of them. So turnout packs the artifacts into a single `tar.gz`, sends that, and unpacks it on the server:

```
◇ Packed 43 files → 78.32 KiB
◇ Uploaded 78.32 KiB in 0.4s · 196.00 KiB/s
◇ Unpacked 43 files into /var/www/site
```

On a small test deploy over a local network this halved the wall time; the gap widens with the file count and the latency, which is where real deploys live.

The archive is written inside the deploy directory as `.turnout-upload.tar.gz` and removed as soon as it is unpacked - including when unpacking fails. It goes there rather than beside the directory because that is the one location a deploy already knows it can write to; the parent is usually `/var/www`, owned by root.

Files are unpacked **over** whatever is already there, exactly like the file-by-file upload: names present in both are replaced, anything else is left alone. Use `--clear` when you want the directory emptied first.

turnout falls back to sending files one at a time, without failing, when:

- `tar` does not answer on the server (checked before anything is sent, and reported as a note);
- the artifact directory holds fewer than 8 files, where the round trips saved do not pay for the packing;
- you pass `--no-archive`.

Windows servers take the archive route too - `tar.exe` has shipped in `System32` since Windows 10 1803. See [Windows servers](/turnout/concepts/windows-servers/).

## Progress

Every step that would otherwise be silent - the SSH handshake, the shell probe, the backup, the clear, the packing, the upload, the unpacking, the restart - is a line in a live checklist: a spinner while it runs, a checked summary when it ends. The upload itself is a byte bar with a percentage, the current rate and an ETA:

```
┌ Deploying myapp → prod
│
◇ Connected to deploy@prod.example.com:22
◇ Packed 214 files → 6.10 MiB
◆ Uploading [███████████░░░░░░░░░░░░░]  47% · 2.87 MiB/6.10 MiB · 1.81 MiB/s · eta 2s
│
```

Once everything lands, the checklist reads as a receipt of what happened:

```
┌ Deploying myapp → prod
│
◇ Connected to deploy@prod.example.com:22
◇ Packed 214 files → 6.10 MiB
◇ Uploaded 6.10 MiB in 3.4s · 1.81 MiB/s
◇ Unpacked 214 files into /var/www/myapp
◇ Restarted: systemctl restart myapp
│
└ Deploy of 'myapp' to 'prod' finished
```

The build runs above the frame under a loader of its own, in the same style - see [the quiet console](/turnout/concepts/quiet-console/); `--verbose` streams it instead. When stdout is not a terminal - a pipe, a CI log, a file - the frame, the spinners and the bar are replaced by one plain line per step, and the build streams, so logs stay readable.

## Configuration

The settings live on four entities - the artifact directory on the app, the host on the server, the login in a credential, the directory in a path - plus the secret in the keyring. One wizard walks all of them:

```bash
turnout deploy-setup [APP] [--server SERVER]
```

It asks for the artifact directory (suggesting `dist`, `build`, `out` or `public` if one already exists), the SSH `host[:port]`, which credential logs in, which path the files land in, and - only for a password credential - a secret to store in the keyring. Existing credentials and paths are offered to pick from, or a new one can be defined inline. Existing values come pre-filled, so it doubles as an edit pass. Nothing is written until every answer is in.

The wizard needs a terminal. In scripts, set the same fields directly:

```bash
turnout app edit myapp --dist dist                # what to upload
turnout server add prod --url https://prod.example.com --host prod.example.com
turnout credential add prod-deploy --user deploy
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"
turnout target add --app myapp --server prod --credential prod-deploy --path wwwroot
echo "$PASSWORD" | turnout pass set prod-deploy   # for a password credential
```

Setting a *POSIX* remote directory from Git Bash needs care: it rewrites `/var/www/myapp` into a local path before turnout sees it, and turnout refuses that rather than deploying somewhere nobody chose. Run those lines from PowerShell, or prefix them with `MSYS_NO_PATHCONV=1`. A Windows directory (`C:\inetpub\myapp`) is unaffected. See [`turnout path`](/turnout/reference/path/) and [Windows servers](/turnout/concepts/windows-servers/).

## Examples

```bash
turnout deploy                          # from the project dir, to the bound server
turnout deploy myapp-prod               # by target name, from any directory
turnout deploy myapp --server prod      # by app name, on a chosen server
turnout deploy myapp-prod --backup --clear   # archive, then clean, then upload
turnout deploy myapp-prod --no-build         # upload what is already built
turnout deploy myapp-prod -p staging-root    # this run only: a different path
turnout deploy myapp-prod -C root            # this run only: a different login
turnout deploy myapp-prod --diff             # what would change - nothing is written
turnout deploy myapp-stage myapp-prod        # one build, two targets, in that order
turnout deploy myapp-prod --open             # then open the stand
```

## In the background

`--detach` runs the whole deploy - build, upload, restart, check - as a [background job](/turnout/concepts/background-jobs/) and returns at once. Everything a picker would ask is settled first, in your terminal; the background then deploys exactly that target, by name, even if the app's binding changes meanwhile.

```bash
turnout deploy web-prod --detach
turnout ps                  # web deploy  running (bg)
turnout logs web deploy -f  # the checklist, one plain line per step
```

A background deploy has nobody to type a password to: give the credential a stored secret, a key or the SSH agent first. When it ends, a [notification](/turnout/concepts/notifications/) says so: `web deployed`, and a click opens the stand - or `web deploy failed`, with the reason, and a click opens the log. How it ended is in `ps` too (`done`, or `failed (exit 1)`), and a failure's output in `turnout logs web deploy --failed`.

`TURNOUT_DETACH=deploy` sends every deploy to the background without `-d`; `--foreground` keeps one here.
