---
title: Settings migrations
description: How turnout upgrades its own data directory when the format changes - and what it refuses to do.
---

turnout stores its catalogs as JSON in the [data directory](/turnout/getting-started/), and `meta.json` records which shape they are in:

```json
{ "schema_version": 2 }
```

When a release changes that shape, the first command you run afterwards upgrades the directory in place and says so:

```text
Migrating settings from schema 2 to 3.
  A copy of the old files is in ~/.local/share/lacodda/turnout/settings-backup-v2
  <what the step changed>
```

Then the command you actually typed carries on. There is no separate migrate command to remember: a tool that refuses to start until you type a magic word is just a worse error message.

## When a change cannot be migrated

Sometimes the new shape needs information the old one does not contain, and inventing it would be worse than stopping. That is what happened in **v0.9.0**: a server used to carry one `user@host` and an inline directory per app, and those became [credentials](/turnout/reference/credential/) and [paths](/turnout/reference/path/) - named entities. turnout has no way to pick names you will still recognize six months from now.

So a schema-1 directory is refused rather than converted. The refusal names what changed, sets a readable copy of the old catalogs aside in `settings-backup-v1`, and lists the commands to re-enter them with. Your original files are untouched, so installing v0.8 again puts you exactly back. The walkthrough is in [Upgrading to 0.9](/turnout/guides/upgrading-to-0-9/).

## Your old files are kept

Before anything is rewritten, every JSON file turnout owns is copied to `settings-backup-v<N>` inside the data directory - next to the data, not in a temp folder, so it is where you would look for it. Journals and caches are left alone; they are not what a migration touches.

Nothing deletes that folder. If a migration ever gets something wrong, copying the files back and installing the previous release puts you exactly where you were. A second migration of the same directory writes `settings-backup-v1-2` rather than overwriting the first safety net.

## Data from a newer turnout is refused

If the directory was written by a **newer** release than the one you are running, turnout stops:

```text
error: this data directory was written by a newer turnout (schema 3, this build reads 2).
Update turnout with `turnout self-update`, or point TURNOUT_DATA_DIR at a different directory.
```

This is the case worth being strict about. The files would very likely still parse - and mean something different. Reading half of a shape you do not understand is how settings get silently corrupted, so an old binary refuses rather than guesses. It happens naturally when two machines share a synced folder, or after rolling a release back.

Nothing is rewritten on the way out, including the version marker: the directory is exactly as it was when the newer turnout last touched it.

## Version by version

Migrations are single-version hops applied in order, so a directory several releases behind is upgraded one step at a time and each step only has to know the shape immediately before it. A version with no path to the present is an error rather than a silent no-op, with the honest way out - back up the directory and run [`turnout setup`](/turnout/reference/setup/) fresh.

Moving settings deliberately between machines is a different job, and [`turnout export`](/turnout/reference/export/) does it.
