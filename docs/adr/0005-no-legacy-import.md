# ADR 0005: No import from the predecessor tool

- Status: accepted
- Date: 2026-08-05

## Context

turnout replaces a private deploy/env tool with its own config format. An `import` command would carry that legacy format - relevant to exactly one person - into a public CLI's surface, docs and maintenance burden forever.

## Decision

turnout ships no importer for the predecessor's configs, in any form. The author's one-time migration happens by hand outside the product.

## Consequences

- Public CLI surface stays clean; no dead command for every other user.
- Entity design is unconstrained by the legacy format.
