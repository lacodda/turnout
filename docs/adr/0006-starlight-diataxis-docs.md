# ADR 0006: Docs - Astro Starlight structured by Diátaxis

- Status: accepted
- Date: 2026-08-05

## Context

A public tool lives or dies by documentation. The author's previous project used mdBook; it works but the maintainer prefers Starlight's look, built-in search and component system. Content needs a structure that separates learning from reference.

## Decision

The documentation site is Astro Starlight in `docs/` (isolated `package.json`), structured by Diátaxis: Getting Started (tutorial), Guides (how-to), Reference (one page per command), Concepts (explanation - entities, gateway). Built and deployed to GitHub Pages by CI only; build artifacts are never committed. Rule: a change to the CLI surface and its documentation land in the same commit.

## Consequences

- Node toolchain appears in the repo, but stays contained in `docs/` and out of the Rust build.
- Docs cannot silently rot: the same-commit rule is enforced in review.
- ADRs live next to the site in `docs/adr/` as plain Markdown, outside the Starlight content tree.
