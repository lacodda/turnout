# ADR 0037: A deploy sends what the stand does not have, and nothing else

- Status: accepted
- Date: 2026-11-22

## Context

Up to v1.2 every deploy sent the whole build: one archive of the artifact directory, or every file over SFTP. `deploy --diff` (v0.24) already knew better - the server lists the deploy directory with sizes, the files that could be the same are hashed on both sides, and the answer says which files are new, changed, unchanged or extra. A bundler that names its chunks by their contents changes a handful of files per build, so most of what crossed the wire was already there. `turnout watch` (ADR 0039) deploys on every change, which makes the waste the common case.

## Decision

**Every deploy compares first and sends the new and the changed files only** - by the same route as before (one archive at 8 files and above, file by file below), with the same comparison `--diff` prints. There is no flag to send everything: two ways of doing one thing would be a setting where one way is right.

**The whole build goes when the comparison cannot be trusted:** on a first deploy (nothing to compare with), when the listing fails, and when the server has neither `sha256sum` nor `shasum`. Compared by size alone, a same-size edit reads as unchanged, and a deploy must never leave a changed file behind because it looked the same from outside.

**`--clear` removes by name.** With a comparison, the files of the directory the build does not have are removed, then the folders that leaves empty (`rmdir`/`rd` without `/s`, deepest first, so a folder with anything left in it is never touched), and only then does the build go - so a file of the build can take the place of a folder the stand had. The end is the one emptying gave: the directory holds the build and nothing else. The target's env file is not removed but written over, so it keeps the mode its owner gave it. Without a comparison, `--clear` empties the directory as before.

**What is recorded and kept is the whole build.** The history's archive is the build, not what crossed the wire this time: a rollback sends what that deploy put on the stand, and it too sends only what the stand lacks of it.

**A torn connection is replaced before anything is tidied.** A deploy whose write failed asks its session whether it still works and opens a new one if not; what arrived of the archive is removed, the backup taken for a rollback goes, and a rollback runs over the new session. Up to v1.2 all of that ran on the dead session and failed - shown by tearing a real connection (lacodda-tear) under a deploy.

## Consequences

- A deploy costs one listing and the hashes of the candidate files more than before, on both sides; the upload it saves is usually far larger. A deploy directory holding much more than the build (uploads, a database volume) is listed, not hashed.
- The archive of a part is packed separately from the build's own archive, which is still packed once for the record.
- An unchanged file on the stand keeps its modification time: it is not written again.
