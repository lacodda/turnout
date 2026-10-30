# ADR 0031: Dependabot is off on purpose

- Status: accepted
- Date: 2026-10-30

## Context

GitHub offers Dependabot on every repository: a pull request for each new version of each dependency, and another for each security advisory against one. On a project with one maintainer that hands the rhythm of updates to the bot - a dozen small pull requests a week, each with a CI run of its own, each merged on the strength of a green check rather than a reading of what changed, and a major version arriving as one more of them.

Its two halves do different jobs, though. Version updates are housekeeping and can wait for a day that suits the project. A vulnerability cannot.

## Decision

**No Dependabot version updates: no `.github/dependabot.yml`.** Dependencies are updated by hand, all at once, at most once a week, in a commit of their own - majors included, and a major that breaks the build is fixed in that commit rather than postponed by pinning the old version. A pin is kept only when a dependency cannot move yet at all, with the reason beside it (the Vue example stays on TypeScript 6 while `vue-tsc` cannot load TypeScript 7).

The security half is `.github/workflows/audit.yml`. It checks every push, every pull request and every Monday morning against the published advisories, and holds each crate's license to the list in `deny.toml`. A red audit is fixed before the next release: by the update, or - when no fixed version exists yet - by accepting the advisory in `deny.toml` with the reason written next to it.

## Consequences

- Updates arrive in batches somebody reads, not on a bot's schedule.
- An advisory published while nothing was pushed still turns the audit red on the next Monday: a red run with no commit behind it means the world moved.
- Adding a `dependabot.yml` reverses this decision and is recorded as a new one, superseding this.
