# ADR 0027: A deploy is recorded, and its build is kept for a rollback

- Status: accepted
- Date: 2026-10-16

## Context

Until v0.25 a deploy left one line in the journal - `deploy web staging 142 files` - and nothing else. Which commit went out, whether the restart printed an error, which deploy to go back to: none of it could be asked afterwards. The worst case was the one the field report from lyrid described - the files went out, the restart did not finish, and from outside the stand looked deployed.

Going back was `restore`: put the deploy directory back as a backup caught it. That needs a backup taken before every deploy (a tar of the whole directory, each time), takes the directory's data back too - uploads, a database's files written since - and fills the disk of the server it is meant to protect.

## Decision

**Every deploy that reaches the server is recorded**, whatever its outcome, as a file of its own in `deploys/` of the data directory, under a number claimed by creating the file - two background deploys finishing together cannot take the same one, as the job registry keeps a file per job (ADR 0018). A record holds the route, the project's version and git commit read before the build (with whether the working tree matched it), the size, how long the server's part took, the outcome - `deployed`, `rolled_back`, `partial` (files written, the restart or check not through) or `failed` (nothing written) - the restart's last lines and the check's answer. The journal keeps its one line and names the record; it does not grow command output.

**The build is kept on this machine**, beside the records, named by the SHA-256 of the archive that went out: two targets of one build share a file, and the record of a rollback points at the build it sent again. A deploy that went file by file packs its build for this. The builds of the newest 10 deploys to each stand - a server and a directory - are kept; a build written within the hour is never pruned, since its record may still be on the way.

**A rollback sends a kept build again**, through the same checked deploy as any other, and then removes the files that later deploys to the stand wrote and the old build does not have. Files no deploy sent are left alone: a rollback is about the code, not the data. A plain `rollback` goes to the deploy before the one whose build the stand runs - the newest that left files, with a rollback standing for the deploy it re-sent - so two rollbacks go two steps back.

## Consequences

- `history` answers what went where, from which commit, and how it ended; `history --diff` asks the app's repository for the commits between two deploys.
- A rollback needs no backup on the server, and does not take back the data a backup would.
- The history and the builds are this machine's: a deploy made elsewhere is not in them, and `export` does not carry them.
- The local disk holds up to ten builds per stand, compressed; the records are kept for good.
