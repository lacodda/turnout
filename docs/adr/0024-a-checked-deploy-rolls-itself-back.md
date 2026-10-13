# ADR 0024: A checked deploy rolls itself back

- Status: accepted
- Date: 2026-10-13

## Context

Up to v0.23 a deploy ended with the restart. Whether the stand worked afterwards was for the person to find out - by opening it, or by hearing about it. `--backup` and `restore` existed, but a rollback was a decision someone had to make, at the moment they were least able to: after noticing.

## Decision

**A target may carry a health check** - one probe and how long to ask it. The probe is a URL (`/health` under the target's address, or a full one) or a command run on the server in the deploy directory. It is an enum, so a check that both fetches and runs cannot be written; the wait is per target, because a service decides how long it takes to start. A URL passes below 400 without following redirects - stricter than the "is the stand there" probe of `use`, which takes anything below 500: a check the owner named is their word for "it works", and a health address answering 404 is a deploy that lost its routes.

**A deploy to a checked target backs up first, asks after the restart, and rolls back on a failure** - a failed check or a failed restart alike. The backup is taken even without `--backup` and removed once the check passes; otherwise every checked deploy would leave an archive behind until the disk is full. After the rollback the check is asked again: a stand that works on the old build was broken by this deploy, and one that does not was broken before it. A first deploy has nothing to roll back to, and says so.

**The address lives on the target too.** A server's URL is the stand the gateway routes an app's API calls to; what a deploy puts on that machine is often seen elsewhere. `open`, `deploy --open` and a relative health URL use the target's address, falling back to the server's.

The catalog moves to schema 6 and exports to format 4, though nothing is rewritten: an older turnout would drop the check without a word and deploy unchecked, and the bump is what makes it refuse instead.

## Consequences

- A checked deploy never leaves the stand on a build that fails its check, as long as the server can archive and unpack the deploy directory.
- A deploy to a checked target takes a backup's time and space, briefly.
- An override that moves the run to another server or path drops the check for that run, and says so: the check describes this target's stand.
- The failure travels as an ordinary failed deploy - in the background, a failure notification whose line names the rollback.
