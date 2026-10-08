# ADR 0022: The doctor looks, and repairs only what turnout owns

- Status: accepted
- Date: 2026-10-08

## Context

Since v0.14 an app's ports and its dotenv file are turnout's, and since v0.20 its jobs run in the background. Both made the common failures quieter: a dev server a closed terminal left behind keeps its port, the next `dev` moves on to another one, and the front door shows yesterday's code without an error anywhere; a dotenv file written for an old gateway port points an app started from an IDE at a port nobody listens on. A deploy, likewise, fails at its upload or its restart for reasons the server could have told before it started - a directory owned by root, a full disk, a `sudo` that wants a password.

`turnout doctor` answers "why does it not work". The question is how far it goes: a doctor that only reports leaves the obvious repairs to the reader; one that repairs whatever it finds would end processes and rewrite files it cannot be sure about.

## Decision

The doctor **looks**. Every check prints `ok`, `info`, `warn` or `fail`, and every `warn` and `fail` names its fix - a command to run, or what to change. The exit code is 1 when a check failed, so a script can gate on it; warnings do not change it. Text and JSON are one view ([ADR 0020](0020-json-is-a-view-not-the-storage.md)).

`--fix` **repairs only what turnout owns and can prove wrong**: its block in a dotenv file, that file's line in `.gitignore`, the record of a gateway whose process is gone, a binding to a server that is not in the catalog. Then it runs every check again and reports what is left, rather than trusting its repairs.

**A process is ended only after a yes.** A process holding an app's dev port with no turnout job behind it is either left behind or somebody's own dev server; turnout cannot tell the two apart. In a terminal `--fix` asks, naming the program and the pid; off a terminal it leaves the process alone unless `--yes` was given, the way `remove -y` skips its question.

**The moment of pain gets the same check.** `dev` looks at the app's dev port before it starts, and offers to end the stray (or refuses, off a terminal) - that is when a stray does its damage.

`--server` turns the doctor on a machine. It **changes nothing there**, with one exception: a probe file is created and deleted in each deploy directory, because permissions, ACLs and read-only mounts each have a say and only a write answers all three. A directory that does not exist yet is not created; the parent a deploy would create it in is asked instead. It has no `--fix`: the repairs are an administrator's (`chown`, `icacls`, a sudoers rule), and the doctor prints them.

## Consequences

- One command finds the stray on the port, the drifted file, the down stand, the lapsed session, the missing tool and the unstored password, each with its fix.
- `--fix` is safe to run without reading: what it writes is turnout's own, and what it would kill it asks about.
- Scripts and CI can run `turnout doctor` as a gate.
- A server check signs in as each target's account, so it answers for the deploy that would actually run - not for whoever happens to have the most rights.
- The doctor's session check needs the gateway to ask the stand on its behalf; that is [ADR 0023](0023-the-jar-keeps-cookies-as-a-browser-does.md).
