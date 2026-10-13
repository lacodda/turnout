# ADR 0026: A diff asks the files, not a manifest

- Status: accepted
- Date: 2026-10-13

## Context

`deploy --diff` answers "what would this deploy change" before it writes. The cheap way is a manifest: every deploy leaves a list of the files it sent and their hashes, and the next one compares against the list. The list is wrong the first time someone edits a file on the server by hand, restores a backup, or deploys with an older turnout - and then it is a second account of the server that disagrees with the first.

## Decision

**The server is asked what it has.** One command lists every file under the deploy directory with its size - `find -printf` where it exists, `wc -c` where it does not (BusyBox, BSD), PowerShell on Windows. Only the files that could be unchanged - the same path and the same size on both sides - are hashed, with SHA-256, on both sides; the hash commands are split so none outgrows the server's shell. A deploy directory often holds far more than the build sends - uploads, a compose file, a database volume - and hashing all of it to compare a few hundred files would be the slow part of a command meant to be quick.

A server with neither `sha256sum` nor `shasum` is compared by size, and the report says so. Files the server has and the build does not are counted as removed with `--clear` and as staying without it, because that is what the deploy would do.

## Consequences

- The diff is right however the files got there.
- It costs a listing and a few hash commands per target - more than reading one file, less than hashing the directory.
- `--diff` writes nothing, and `--json` gives a script the same comparison.
