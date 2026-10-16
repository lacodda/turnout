# ADR 0028: A stand keeps a number of backups

- Status: accepted
- Date: 2026-10-16

## Context

Every `deploy --backup` and `backup` left an archive of the whole deploy directory beside it, and nothing ever removed one. A month of daily deploys to a Raspberry Pi with `--backup` was enough to fill its card.

## Decision

**Every command that leaves a backup prunes the stand's backups afterwards** to the newest N: `backup`, `deploy --backup`, and a checked deploy rolled back to the backup it took. What it removed is named. `backup prune [--keep N]` does the same on demand, and `--keep 0` removes them all.

**N belongs to the target** (`--keep-backups`, default 5), not to the path and not to turnout as one constant. A backup is the whole directory - kilobytes for a static site, gigabytes beside a database - and the disk it fills is the stand's: one path deployed to a Pi and to a VPS wants two numbers, which is what a target is (the health check and the address live there for the same reason). A run with no target behind it keeps 5. Zero is refused as a setting; it would remove each backup as it is taken.

Only turnout's own archives - `YYYYMMDD-HHMMSS.tar.gz` - are counted or removed. The removal is confirmed by listing the directory again, since `del` on a Windows server reports success whatever it did.

The field makes the data directory schema 7 and the export format 5: a turnout before v0.25 would drop the number when it edited or imported the target, and the next prune would cut the stand back to 5.

## Consequences

- A backup habit no longer fills a disk.
- Raising the number is one `target edit`; lowering it takes effect at the next backup or prune.
- Backups taken before v0.25 are pruned the first time a new one is left beside them.
