---
title: Upgrading to 0.9
description: What the entity split changed, and how to move a v0.8 setup over by hand.
---

v0.9.0 split the server entity in three. If you used turnout 0.8 or earlier, the first command you run after updating refuses to start and points here:

```text
error: these settings were written by turnout 0.8 or older (schema 1), and v0.9.0 changed
how they are stored.
```

Nothing was lost and nothing was changed. This page is the way over - about ten minutes for a typical setup.

## What changed

A server used to hold everything a deploy needed: the base URL, one `user@host`, and an inline remote directory per app. That meant re-entering the same deploy account for every stand it reached, and repeating the same web root on each one.

Now there are three entities that reference each other by name:

| Was, on the server | Is now |
| --- | --- |
| `--ssh deploy@host:2200` | the host stays on the server (`--host host:2200`), the user becomes a [credential](/turnout/reference/credential/) |
| `--ssh-key ~/.ssh/id_ed25519` | the credential's `--key` |
| `--deploy-path myapp=/var/www/myapp` | a [path](/turnout/reference/path/) named e.g. `wwwroot`, then `--deploy-path myapp=wwwroot` |
| `--restart-cmd "myapp=systemctl restart myapp"` | the path's `--restart` |
| `pass set staging --kind ssh --login deploy` | `pass set` on the credential's name |

The base URL and the TLS policy did not move. See [Entities](/turnout/concepts/entities/) for the full model.

## Why there is no automatic conversion

The new entities need names, and turnout would have to invent them. A catalog full of `staging-cred-1` and `path-2` is worse than ten minutes of typing: those names are what you will read in every picker and every `deploy` line from now on.

## Moving over

### 1. Find your old values

The refusal already put a readable copy next to your data:

```text
A copy to read the old values from is in
  ~/.local/share/lacodda/turnout/settings-backup-v1
```

`servers.json` there holds the hosts, users, key files, deploy directories and restart commands; `apps.json` and `groups.json` hold the rest. They are plain JSON - open them in any editor.

Your **secrets are still in the OS keyring** and are not in those files. You will re-enter them in step 4, or copy them out of the keyring first with turnout 0.8 if you no longer remember them.

### 2. Start the new catalog

```bash
turnout setup
```

This writes a fresh `meta.json` at schema 2 alongside the old files.

### 3. Re-enter the entities

Working from the copy, in this order - servers and apps first, then the things they point at:

```bash
# from servers.json: name, url, label, ssh.host/ssh.port
turnout server add staging --url https://staging.example.com --host 10.0.0.42:2222

# from servers.json: ssh.user, ssh.key
turnout credential add staging-deploy --user deploy
turnout credential add pi --user pi --key ~/.ssh/id_ed25519

# from servers.json: deploy.<app>.path and deploy.<app>.restart
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"

# from apps.json
turnout app add myapp --path ~/projects/myapp --dist dist

# then link them
turnout server edit staging --credential staging-deploy
turnout server edit staging --deploy-path myapp=wwwroot
```

`turnout deploy-setup myapp` walks all of this in one wizard instead, offering to create the credential and path as it goes - usually the faster route for the first app.

### 4. Put the secrets back

```bash
turnout pass set staging-deploy
```

The secret now belongs to the credential rather than to a (server, kind) pair, so one `pass set` covers every stand that credential reaches.

### 5. Check it

```bash
turnout status                 # apps, servers, creds, paths
turnout server show staging    # the four parts, resolved
turnout deploy myapp --server staging --no-build
```

## Exports from 0.8

Export files moved to format 2 for the same reason, so a `turnout-export.json` written by 0.8 is refused on import with the same explanation. It is still plain JSON and still readable - use it as your source in step 3 exactly like `settings-backup-v1`.

## Rolling back

Nothing about your old data was rewritten. Installing turnout 0.8 again - `turnout self-update` cannot go backwards, so fetch it from [the releases page](https://github.com/lacodda/turnout/releases) - finds the directory exactly as it was.
