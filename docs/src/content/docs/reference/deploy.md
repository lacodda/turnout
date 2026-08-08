---
title: turnout deploy
description: Deploy an app to a server over SSH/SFTP - build, upload, restart.
sidebar:
  order: 9
---

```bash
turnout deploy [APP] [--server SERVER] [--no-build] [--backup] [--clear]
```

Builds the app, uploads the artifacts to the server's deploy directory over SFTP and optionally restarts the service - one command, using the same apps, servers and secrets as everything else.

## What happens

1. **Resolve.** The app comes from the argument or the current directory; the target server from `--server` or the app's current [binding](/turnout/reference/use/).
2. **Build.** The app's `build` command runs (skip with `--no-build`); a failing build aborts the deploy.
3. **Connect.** SSH to the server's configured `user@host:port`. Auth order: the configured key file (`--ssh-key`), then agent keys (ssh-agent / Pageant), then the password stored in the keyring (`--kind ssh`, falling back to `password`).
4. **Backup** *(only with `--backup`)*: the remote directory is archived first - see [`turnout backup`](/turnout/reference/backup/).
5. **Clear** *(only with `--clear`)*: the remote deploy directory is emptied.
6. **Upload.** The app's artifact directory (`--dist`) is copied recursively; created directories and a file/byte summary are reported.
7. **Restart.** If a post-deploy command is configured for this app, it runs on the server and its output is shown.

## Configuration

The settings live on two entities - the artifact directory on the app, the SSH access and the per-app target on the server - plus the secret in the keyring. One wizard walks all of them:

```bash
turnout deploy-setup [APP] [--server SERVER]
```

It asks for the artifact directory (suggesting `dist`, `build`, `out` or `public` if one already exists), the SSH `user@host[:port]`, an optional key file, the remote directory, the post-deploy command, and - only when no key was given - a password to store in the keyring. Existing values come pre-filled, so it doubles as an edit pass. Nothing is written until every answer is in.

The wizard needs a terminal. In scripts, set the same fields directly:

```bash
turnout app edit myapp --dist dist                # what to upload
turnout server edit prod --ssh deploy@prod.example.com
turnout server edit prod --deploy-path myapp=/var/www/myapp
turnout server edit prod --restart-cmd "myapp=systemctl restart myapp"
echo "$PASSWORD" | turnout pass set prod --kind ssh --login deploy   # if no agent key
```

## Examples

```bash
turnout deploy                          # from the project dir, to the bound server
turnout deploy myapp --server prod      # explicit target
turnout deploy myapp --backup --clear   # archive, then clean, then upload
turnout deploy myapp --no-build         # upload what is already built
```
