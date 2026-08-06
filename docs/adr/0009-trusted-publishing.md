# ADR 0009: Registry publishing via OIDC trusted publishing

- Status: accepted
- Date: 2026-08-06

## Context

Releases were published to crates.io and npm by hand: `cargo publish` needed a
long-lived token on the maintainer's machine, `npm publish` additionally
demanded an interactive OTP. Both steps regularly lagged behind the GitHub
Release. Storing registry tokens as repository secrets would automate this but
leaves long-lived credentials that can leak and must be rotated.

## Decision

A `Publish` workflow runs on every published GitHub Release (and on manual
dispatch with a tag) and publishes both registries via OIDC trusted publishing,
so no credentials are stored anywhere:

- crates.io: `rust-lang/crates-io-auth-action` exchanges the workflow's OIDC
  identity for a short-lived token consumed by `cargo publish`.
- npm: npm >= 11.5.1 performs the OIDC exchange itself during `npm publish`
  and attaches build provenance automatically.

Each registry is configured once (crate/package settings -> Trusted
Publishing) to trust `lacodda/turnout` + `publish.yml`.

## Consequences

- A release becomes fully automatic: tag push -> binaries -> GitHub Release ->
  both registries; the maintainer's only per-release act is pushing the tag.
- No secrets to rotate or leak; npm packages gain provenance attestations.
- Re-publishing an existing version fails the job (registries are immutable);
  the manual dispatch input exists for recovery after partial failures.
- The npm job publishes the wrapper only; it expects the GitHub Release assets
  to exist, which the `release` event guarantees.
