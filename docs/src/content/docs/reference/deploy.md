---
title: "deploy"
description: Deploy an app to a server over SSH/SFTP - build, upload, restart.
sidebar:
  order: 9
---

```bash
turnout deploy [TARGET] [-s SERVER] [-C CREDENTIAL] [-p PATH] [-n] [-b] [-c]
```

Builds the app, uploads the artifacts to the target directory over SFTP and optionally restarts the service - one command, using the same apps, servers, credentials and paths as everything else.

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

## What happens

1. **Resolve.** `TARGET` is a [target](/turnout/reference/target/) name (`myapp-prod`) or an app name (`myapp`, using that app's target on its current [binding](/turnout/reference/use/)). With no argument, the app comes from the current directory and the server from its binding; if that pair has a target, it is used. If it does not, turnout asks which path to deploy into on a terminal, then offers to save the answer as a target so it is asked once rather than every time. `--server`, `--credential` or `--path` override the resolved target's fields for this run only.
2. **Build.** The app's `build` command runs (skip with `--no-build`); a failing build aborts the deploy. The build tool's own output streams through unchanged.
3. **Plan.** The artifact directory is walked to count files and bytes, so the upload can report a percentage and an ETA. An empty artifact directory aborts the deploy before anything is sent.
4. **Connect.** SSH to the server's host and port as the credential's user, with the credential's key file or the password stored in the keyring under its name.
5. **Backup** *(only with `--backup`)*: the remote directory is archived first - see [`turnout backup`](/turnout/reference/backup/).
6. **Clear** *(only with `--clear`)*: the target directory is emptied.
7. **Upload.** The app's artifact directory (`--dist`) is copied to the path's directory - as one archive when that is faster, otherwise file by file. See [How the upload travels](#how-the-upload-travels).
8. **Restart.** If the path has a post-write command, it runs on the server **in the deploy directory** and its output is shown - a command does not need its own `cd` to reach the files it just received.

:::note[Interrupted after the upload]
A deploy that fails between the upload and the restart says so explicitly: the files are already on the server, but the service is still running what it had. Run `turnout deploy` again, or restart by hand if the new files are already good enough.
:::

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

The build tool's own output streams above the frame, unchanged. When stdout is not a terminal - a pipe, a CI log, a file - the frame, the spinners and the bar are replaced by one plain line per step, so logs stay readable.

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
```
