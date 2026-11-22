# ADR 0038: A copy tops up a folder, removes nothing, and hands what it wrote to the folder's group

- Status: accepted
- Date: 2026-11-22

## Context

A service on a stand often takes its input from a folder: an inbox it reads files from and moves them on to `done/` or `refused/`, an import directory, a drop folder. Filling it from a laptop was `scp` by hand or a call through the service's own API. A deploy is the wrong tool for it: a deploy directory is the build's, and `--clear` makes it so; an inbox belongs to the service that empties it.

## Decision

**`turnout copy FROM SERVER:DIR` sends the files of a local folder that the server's folder does not have, or has otherwise, and removes nothing there.** Named after `rclone copy` (new and changed go, nothing is deleted), not `sync`, which most tools take to mean deleting what the source lacks. `SERVER` is a server or a target, as `tunnel` and `exec` take it; a relative `DIR` is under the target's deploy directory, or where the login lands on a server, as `scp` reads it.

**Which files differ is asked the way `deploy --diff` asks it** (ADR 0037): the folder's files with their sizes, and a hash of the candidates on both sides. With no hash tool on the server, same-size files go again: sending a file twice is harmless to a service that knows what it took, leaving an edit behind is not.

**A file appears whole or not at all.** It travels as `.NAME.turnout-part` beside its place, and once every file has arrived they are renamed into place - `mv -f` on POSIX, which a reader sees as the old file or the new and never as half of either; `move /y` on Windows. A copy that fails on the way removes what it sent under those names.

**On POSIX, what the copy wrote belongs to the folder's group and the group may write it.** The service that empties an inbox often runs as an account of its own - a container's user - and a file it can read but not move is stuck there. The group is the one of the folder the files land in, or of the nearest folder above it that exists, asked before anything is made; files and the folders the copy made get `chgrp` to it and `g+w`. Only paths the deploying account owns are touched: a folder that was there keeps whatever its owner set. A `chgrp` the account cannot make - it is not in the group - is named in a warning, with what to do. There is no flag for it: handing a shared folder's files to its group is what writing into a shared folder means. A Windows server needs nothing: the files take the folder's ACL.

## Consequences

- A file the service took and moved on is new again to the next copy and goes again; the service has to know it, as an inbox reader does by the file's contents.
- The listing walks the whole folder, `done/` included, on every copy.
- The copy writes no record in the deploy history: it is not a build of an app.
