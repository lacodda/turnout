---
title: Settings migrations
description: How turnout upgrades its own data directory when the format changes - and what it refuses to do.
---

turnout stores its catalogs as JSON in the [data directory](/turnout/getting-started/), and `meta.json` records which shape they are in:

```json
{ "schema_version": 1 }
```

When a release changes that shape, the first command you run afterwards upgrades the directory in place and says so:

```text
Migrating settings from schema 1 to 2.
  A copy of the old files is in ~/.local/share/lacodda/turnout/settings-backup-v1
  split server credentials into their own catalog
```

Then the command you actually typed carries on. There is no separate migrate command to remember: a tool that refuses to start until you type a magic word is just a worse error message.

## Your old files are kept

Before anything is rewritten, every JSON file turnout owns is copied to `settings-backup-v<N>` inside the data directory - next to the data, not in a temp folder, so it is where you would look for it. Journals and caches are left alone; they are not what a migration touches.

Nothing deletes that folder. If a migration ever gets something wrong, copying the files back and installing the previous release puts you exactly where you were. A second migration of the same directory writes `settings-backup-v1-2` rather than overwriting the first safety net.

## Data from a newer turnout is refused

If the directory was written by a **newer** release than the one you are running, turnout stops:

```text
error: this data directory was written by a newer turnout (schema 2, this build reads 1).
Update turnout with `turnout self-update`, or point TURNOUT_DATA_DIR at a different directory.
```

This is the case worth being strict about. The files would very likely still parse - and mean something different. Reading half of a shape you do not understand is how settings get silently corrupted, so an old binary refuses rather than guesses. It happens naturally when two machines share a synced folder, or after rolling a release back.

Nothing is rewritten on the way out, including the version marker: the directory is exactly as it was when the newer turnout last touched it.

## Version by version

Migrations are single-version hops applied in order, so a directory several releases behind is upgraded one step at a time and each step only has to know the shape immediately before it. A version with no path to the present is an error rather than a silent no-op, with the honest way out - back up the directory and run [`turnout setup`](/turnout/reference/setup/) fresh.

Moving settings deliberately between machines is a different job, and [`turnout export`](/turnout/reference/export/) does it.
