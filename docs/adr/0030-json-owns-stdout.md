# ADR 0030: JSON owns stdout

- Status: accepted
- Date: 2026-10-22

## Context

Off a terminal, turnout's progress degrades to one plain line per step, on stdout, so a CI log reads as a receipt. Commands that print JSON were each expected to stay quiet around it. `restore --list --json` and `backup prune --json` did not: a step that ended without a result line of its own still printed what it had been doing, and the JSON arrived as `Connecting to deploy@host:22 ...` followed by the object - unparseable by the scripts `--json` exists for. Nothing caught it until a test parsed their output against a real server.

## Decision

**When the command line asks for JSON, every plain progress line goes to stderr.** `main` looks for `--json` among turnout's own arguments - before `--`, after which the words belong to a command run on the server - and tells the progress module so before the command starts. One decision for every command, present and future, instead of each command remembering.

Progress on a terminal is untouched, and so is plain output without `--json`: a deploy piped to a log still reads as before.

## Consequences

- `turnout ... --json | jq` gets JSON and nothing else, whatever steps the command takes.
- The progress lines of a JSON run are still there, on stderr, for whoever reads the log.
