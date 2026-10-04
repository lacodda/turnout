# ADR 0020: JSON output is a view, not the storage

- Status: accepted
- Date: 2026-10-04

## Context

Scripts, prompt lines and status bars want turnout's state as data, and an MCP server is planned to read the same state for an assistant. Parsing the text output breaks on every wording change. Two shortcuts were at hand: print the catalog files (`apps.json`, the job records) as they are, or bolt a JSON branch onto each command next to its `println!`s.

The catalog files are storage. They omit empty fields, carry internal values (a cached shell dialect, a process birth time, the gateway's control token) and change shape with the schema - which is migrated on disk ([ADR 0007](0007-json-catalogs.md)). A script reading them would break on a migration it never heard of. A JSON branch beside the text, in turn, is a second rendering of the same facts by hand: a field added to one is forgotten in the other.

## Decision

Every command that reports builds a **view** - a plain struct deriving `Serialize` (`AppView`, `ServerView`, `JobView`, `StatusView`, ...) - and hands it, with a function that writes it as text, to one `emit`. `--json` prints the view; otherwise the text function renders the same fields. `status` is assembled from the other commands' views (`apps` is a list of `AppView`, `jobs` of `JobView`), so each fact has one shape everywhere it appears.

The views are the contract:

- every key is always present - `null` for nothing, `[]` for none;
- a list is an array of what `show` prints for one; an empty catalog is `[]`;
- times are ISO 8601 in UTC;
- secrets and the control token are never part of a view.

`--json` is a flag of each reporting command (one `Format` struct flattened into them), not a global flag: a global one would parse on `deploy` and quietly do nothing.

## Consequences

- A storage migration does not change the output; changing a view does, and is a breaking change named in the changelog.
- The future MCP server serves the same views - no third shape.
- The text and the JSON cannot drift apart: the text is rendered from the view.
- Each reporting command carries a view type, which is more code than printing - the price of one truth per fact.
