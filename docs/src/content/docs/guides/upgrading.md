---
title: Upgrading
description: What changes when turnout updates, what it does to your settings by itself, and how to move a 0.8 setup over by hand.
---

For almost every upgrade there is nothing to do:

```bash
turnout self-update
```

The next command you run brings the settings up to date by itself, says in one line per change what it did, and keeps a copy of what it rewrote. The one exception is a setup from **turnout 0.8 or older**, which has to be re-entered - [below](#from-08-or-older).

## What updates by itself

turnout's settings live in the data directory (`turnout status` names it), and `meta.json` there records which shape - the *schema* - they are in. A newer turnout reads the schema, applies each step between it and its own in order, and records the new one. A step that rewrites files first copies the directory into `settings-backup-vN` beside it, where `N` is the schema it started from.

| Your settings were last written by | The first command of a newer turnout |
| --- | --- |
| 0.9 - 0.10 | turns each server's per-app deploy directories into named [targets](/turnout/reference/target/), `{app}-{server}` - the server's login comes along, and a server with no login is named so you can add its target by hand |
| 0.11 - 0.17 | gives a [gateway port](/turnout/reference/app/#ports-turnout-hands-out) from 7100-7199 to every app that had none - the port used to be a question you could leave empty; ports already set keep their numbers |
| 0.18 - 0.19 | moves the record of a running gateway into the job registry, next to every other [background job](/turnout/concepts/background-jobs/) - a gateway running through the update keeps running |
| 0.20 - 0.23 | rewrites nothing: from here on a target can carry a [health check](/turnout/reference/target/#health-check) |
| 0.24 | rewrites nothing: from here on a target can say how many [backups](/turnout/reference/target/#backups) its stand keeps |

Running `turnout setup` on an older directory does the same - it migrates and says *already set up*. See [Settings migration](/turnout/concepts/settings-migration/) for the rules behind the steps.

### Going back

An older turnout refuses settings a newer one has written, rather than reading half of them: dropping a health check it does not know would deploy unchecked. `self-update` only goes forward, so going back means installing that release from [the releases page](https://github.com/lacodda/turnout/releases) and giving it settings it can read:

- after an update that **rewrote** files (from 0.10, 0.17 or 0.19 and earlier), the copy taken before the step is in `settings-backup-vN` - put it in place of the data directory;
- after one that **rewrote nothing** (from 0.20 on), there is no copy: the files are what the older release wrote, plus whatever newer features you used since. If you used none - no [health check](/turnout/reference/target/#health-check) on a target for 0.20 - 0.23, no [backup count](/turnout/reference/target/#backups) for 0.24 - write the schema that release used (5 for 0.20 - 0.23, 6 for 0.24) into `meta.json`. If you did, that release cannot keep them, and refusing was right.

## Exports

A file written by [`turnout export`](/turnout/reference/export/) imports into any newer turnout:

- **From 0.11 on**, nothing is lost: the repository keeps the export each release wrote with its own binary, and every build imports all of them and checks that each entity and each field comes back as it left.
- **From 0.9 - 0.10**, the servers' deploy directories become named targets on the way in, by the same rule as the migration.
- An app that arrives without a gateway port, or on a port an app on this machine already holds, is given a free one - [`import`](/turnout/reference/import/#ports) names each.
- A file from **0.8** is refused, for the reason below. It is still plain JSON - use it as your source in step 3.

## From 0.8 or older

v0.9.0 split the server in three, and settings from 0.8 cannot be converted without inventing names. The first command after updating refuses to start and points here:

```text
error: these settings were written by turnout 0.8 or older (schema 1), and v0.9.0 changed
how they are stored.
```

Nothing was lost and nothing was changed. This is the way over - about ten minutes for a typical setup.

### What changed

A server used to hold everything a deploy needed: the base URL, one `user@host`, and an inline remote directory per app. The same deploy account was re-entered for every stand it reached, and the same web root repeated on each one.

Now each part is an entity of its own, and a [target](/turnout/reference/target/) names the four a deploy needs:

| Was, on the server | Is now |
| --- | --- |
| the base URL, the TLS policy | the same, on the [server](/turnout/reference/server/) |
| `--ssh deploy@host:2200` | the host on the server (`--host host:2200`), the user in a [credential](/turnout/reference/credential/) |
| `--ssh-key ~/.ssh/id_ed25519` | the credential's `--key` |
| `--deploy-path myapp=/var/www/myapp` | a [path](/turnout/reference/path/) (`--dir /var/www/myapp`), and a target joining app, server, credential and path |
| `--restart-cmd "myapp=systemctl restart myapp"` | the path's `--restart`, run in the deploy directory |
| `pass set staging --kind ssh --login deploy` | [`pass set`](/turnout/reference/pass/) on the credential's name |

See [Entities](/turnout/concepts/entities/) for the whole model.

### Why there is no automatic conversion

The new entities need names, and turnout would have to invent them. A catalog full of `staging-cred-1` and `path-2` is worse than ten minutes of typing: those names are what you will read in every picker and every `deploy` line from then on. (The later migration from 0.10 does name targets by itself - both halves of `{app}-{server}` are names you typed.)

### 1. Find your old values

The refusal already put a readable copy next to your data:

```text
A copy to read the old values from is in
  ~/.local/share/lacodda/turnout/settings-backup-v1
```

`servers.json` there holds the hosts, users, key files, deploy directories and restart commands; `apps.json` and `groups.json` hold the rest. They are plain JSON - open them in any editor.

Your **secrets are still in the OS keyring** and are not in those files. You attach them again in step 4, or copy them out with turnout 0.8 first if you no longer remember them.

### 2. Start the new catalog

```bash
turnout setup
```

On settings from 0.8, `setup` explains what starting over means and asks to confirm. Yes moves the old catalogs into `settings-backup-v1` - the folder the refusal named - and starts an empty catalog. No changes nothing at all. The journal is kept, and the keyring is not touched.

### 3. Re-enter the entities

Working from the copy - servers and apps first, then what joins them:

```bash
# from servers.json: name, url, label, ssh.host/ssh.port
turnout server add staging --url https://staging.example.com --host deploy.staging.example.com:2222

# from servers.json: ssh.user, ssh.key
turnout credential add staging-deploy --user deploy
turnout server edit staging --credential staging-deploy

# from servers.json: deploy.<app>.path and deploy.<app>.restart
turnout path add wwwroot --dir /var/www/myapp --restart "systemctl restart myapp"

# from apps.json - the gateway port is handed out, no need to carry it over
turnout app add myapp --path ~/projects/myapp --dist dist --server staging

# the deploy itself: which app, to which server, as whom, into which path
turnout target add myapp-staging --app myapp --server staging --path wwwroot
```

[`turnout deploy-setup myapp`](/turnout/reference/deploy/#configuration) walks the same in one wizard, offering to create the credential, the path and the target as it goes - usually the faster route for the first app.

### 4. Put the secrets back

```bash
turnout pass set staging-deploy
```

The secret belongs to the credential now, not to a (server, kind) pair: one `pass set` covers every stand that credential reaches. A key with no passphrase needs nothing here - or switch a password login to a key altogether with [`turnout key setup`](/turnout/reference/key/).

### 5. Check it

```bash
turnout status                          # apps, servers, credentials, paths, targets
turnout target show myapp-staging       # the four parts, resolved
turnout doctor --server staging         # can it sign in, write the directory, run the restart?
turnout deploy myapp-staging --no-build
```

### Rolling back

Nothing about your old data was rewritten. Installing turnout 0.8 again from [the releases page](https://github.com/lacodda/turnout/releases) and moving `settings-backup-v1` back finds everything exactly as it was.
