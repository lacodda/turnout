# ADR 0007: Catalogs as one JSON file per entity kind

- Status: accepted
- Date: 2026-08-05

## Context

Apps and servers (later credentials metadata and state) need durable storage in the data directory. Options: one file for everything (the predecessor's approach - editing anything risks everything), a file per entity instance (many tiny files, awkward listing), or a database (overkill for tens of records, opaque to hand editing).

## Decision

One pretty-printed JSON array per entity kind: `apps.json`, `servers.json`; runtime state will live apart from catalogs (`state.json`). Files are written atomically (temp file + rename). `meta.json` carries the schema version that governs future migrations. The CLI is the primary interface; hand-editing the JSON is the supported advanced fallback, which pretty-printing keeps practical.

## Consequences

- Human-readable, diffable, trivially backed up - the whole setup is a folder copy.
- Entity kinds stay isolated: a broken edit in `servers.json` cannot corrupt apps.
- Cross-file references (app allow-lists name servers) are validated by the CLI, not the storage layer; `server remove` updates referencing apps.
- Not concurrency-safe across simultaneous turnout processes; acceptable for a single-user tool, revisit if the gateway daemon needs shared writes.
