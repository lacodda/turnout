# ADR 0032: Every release leaves its export behind

- Status: accepted
- Date: 2026-10-30

## Context

The settings export is the one file a turnout user carries from machine to machine and from version to version. Its format has changed three times since v0.11.0 (3, 4, 5), each time additively, and the promise for 1.0 is that no change since v0.11 broke it. Until now that promise rested on reading the code: unit tests built `Snapshot` values in the shape of today's structs, which say nothing about what an older binary actually wrote.

Holding a real file of every release against the current build found a defect within the hour. An app exported before v0.18.0 could have no gateway port - the port was a question you could leave empty - and an import brought it back without one, past the schema 4 migration that had given every app a port. The same door let an arriving app land on a port an app on this machine already held: the v0.13.1 defect of two listeners on one port, reached by the one way into the catalog that skips `app add`.

## Decision

**Each release writes its own export into `tests/fixtures/exports/vX.Y.Z.json`, made by that release's binary**, through `tools/export-fixture.ps1`: a catalog built by the binary's own commands with every flag its `--help` offers - apps with and without the optional fields, three kinds of login, servers, paths, a group, targets with health checks and backup counts where the release has them, and secrets sealed with a known passphrase. The project directories are rewritten to a neutral root so no machine's paths enter the repository. The backfill from v0.11.0 to v0.26.0 was made from the published release archives.

`tests/export_fixtures.rs` imports every one into an empty data directory and exports it again with the current build. Every entity and every field of the old file must come back unchanged, the secrets must open, and every app must hold a gateway port of its own. A second test fails while the current version has no fixture of its own, so the release that changes the format is the one that freezes it.

**An import holds arriving apps to the rules of `app add`** (`front::settle_arrivals`): an app without a gateway port is given one, an app on a port held here moves to a free one, a clashing dev port is dropped for the next `dev` to assign. The apps already here keep their numbers - their dotenv files carry them. The schema 4 migration uses the same function for the apps it fills, so the rule has one implementation.

## Consequences

- "Imports from any version since v0.11" is checked on every push, on every platform CI runs.
- The release ritual has one more step: run the script after the bump. Forgetting it turns the gate red.
- A fixture is never regenerated: it is what that release wrote. A test that needs a richer catalog gets it from the next release's fixture.
- The fixtures' secrets are sealed with Argon2id at full strength, so the debug profile optimises `argon2` and `blake2` - otherwise the suite spends most of its minute deriving keys.
