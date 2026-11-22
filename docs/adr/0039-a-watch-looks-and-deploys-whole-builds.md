# ADR 0039: A watch looks at the files, and deploys a change once it has settled

- Status: accepted
- Date: 2026-11-22

## Context

Keeping a stand on the newest build meant a `turnout deploy` after every build. A watch that deploys by itself has three questions to answer: how it learns of a change, when a change is a whole build and not half of one, and where it runs and reports.

## Decision

**It looks rather than listens.** Twice a second it notes the size and modification time of every watched file. Change notifications differ by OS, do not arrive from network shares, WSL mounts or folders bind-mounted into a container, and a library for them would still have to be taught to ignore the build's own writes. Looking is one path on every OS and file system, and needs no new dependency.

**A change counts once the files have been still for 1.5 seconds**, and an empty directory is never deployed: a build writes in bursts, and a bundler empties its directory before it writes. The watched files are looked at afresh after each deploy, so what the deploy itself wrote - the build of `--build` - does not count as a change.

**By default it watches the artifact directory**, whatever writes it. `turnout build` of the app has a record, and the watch reads it: while the build runs the watch waits, and a build that failed after the change began is not deployed - what it left is half a build. That is "deploy after a green build" without a flag for it. **`--build` watches the sources instead** - what git tracks or would track, the list the stale-build warning reads - builds on every change and deploys only a green build.

**It is a job of the app, `APP.watch`, and owns its record the way a tunnel does** (ADR 0034): `turnout watch` starts `turnout watch ... --log FILE` detached and returns once the record is claimed; `--foreground` runs the same loop in the terminal. There is no command line under it for the job supervisor to run, only turnout looking at files. `ps`, `logs` and `stop` take it as any job of the app.

**One target per watch, one watch per app.** Sending the same change to two stands as soon as a file is saved is a decision for a person.

**Each deploy is an ordinary deploy**: recorded in the history, checked and rolled back as its target says, only what changed sent (ADR 0037). Each one is a notification - one that went through opens the stand, one that failed names why and opens the watch's log - and a failure stays in `ps` as the job's trouble until a deploy goes through again. A failed deploy never ends the watch.

## Consequences

- Looking costs a walk of the watched files twice a second; for the sources that is a `git ls-files` each time.
- A build run outside turnout gives no word about itself, so for it the watch goes by the files alone - the quiet period and the empty-directory rule.
- Ctrl+C in the foreground ends the watch after the deploy under way; `stop` ends a background one at once, and a deploy it cuts short goes unrecorded and may have left part of a build on the stand - the next deploy sends what is missing.
