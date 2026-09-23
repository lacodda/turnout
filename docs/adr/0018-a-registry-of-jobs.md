# ADR 0018: a registry of jobs, one file per job, known by pid and birth

- Status: accepted
- Date: 2026-09-23

## Context

v0.20.0 lets `dev`, `build`, `test`, `lint`, `run` and `deploy` run in the background (`--detach`) and adds `ps`, `logs` and `stop` over them. Until then the only process turnout kept track of was the gateway, as one field in `state.json` holding its pid. That was already fragile - `stop` killed whatever the recorded pid named - and it did not scale to many jobs written by many processes.

Three questions had to be answered: who runs a detached job, where its record lives, and how a record knows its process again.

## Decision

**A detached job is run by a supervisor.** `--detach` starts turnout again, detached, as the hidden `turnout job-run`. The supervisor claims the job's record and runs the command through the same `job::run` a foreground run uses - output captured into the log, the ready detector watching - and writes down when the server came up and how the job ended. `deploy --detach` supervises `turnout deploy` itself, with every choice a picker would make already settled by name. The parent waits for the claim, then a moment more, so a job that fails at once reports its output and exit code in the terminal that asked.

**Every job has a record, one file per job.** `jobs/{key}.json` in the data directory, where the key is `APP.COMMAND` (the command `%`-escaped) or `gateway`. Foreground jobs are recorded too, so `ps` answers "what is alive", not "what did I detach". The gateway records itself once its ports are bound; `gateway start` and `stop` are thin wrappers over that record. The record moved out of `state.json` in schema 5.

**A record names its process by pid and by birth** - the start time the OS reports for it: a FILETIME on Windows, the start tick in `/proc/PID/stat` on Linux, `proc_pidinfo` on macOS. The process is the recorded one only while both match. Nothing is signalled otherwise.

**Stopping ends the tree.** On Windows, `taskkill /T /F` plus the supervisor's kill-on-close job object. On Unix, the supervisor runs in a session of its own (`setsid`) and `stop` signals its process group; a foreground turnout that leads its group (every interactive shell arranges that) is sent SIGINT, the same as Ctrl+C in its terminal, and one that does not is refused rather than signal somebody else's group.

## Consequences

**No write races.** A supervisor marking its server ready, a build in another terminal finishing and the gateway starting all write at once. With one shared file the loser of each read-modify-write drops the other's news; with a file per job, each record has one writer, and a write is a rename.

**A reused pid is harmless.** After a reboot every recorded pid names *some* process. Without the birth, `ps` would call a dead dev server running and `stop` would kill an unrelated program, tree and all. With it, the record reads `gone` and `stop` clears it.

**A slot, not a history.** A key holds the last run of that command; the next run takes the record and truncates the log. Records and logs are bounded by the number of commands, not by time, and a second concurrent run of the same job is refused - it would truncate the first one's log.

**Logs got new names.** `{app}-{command}.log` let `a` running `b-c` and `a-b` running `c` share a file, and squashed `test:e2e` into `test-e2e`'s. The key's dot and escaping make both impossible. `TURNOUT_LOG_DIR` moves the logs; the record keeps the path, so `logs` finds them afterwards.

**`taskkill`'s exit code is not the verdict.** In a job's tree the supervisor dies first and its job object takes the children, so `taskkill /T` routinely fails to find processes it listed. A signal is judged failed only if the recorded process is still alive after it.

**Every record is externally tagged JSON.** An internally tagged enum is read through a buffer that keeps map keys as strings, and the gateway's port map has numbers for keys: written fine, never read back. A record that does not parse is reported, not silently treated as absent.
