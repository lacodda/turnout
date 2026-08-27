# Changelog

All notable changes to this project are documented in this file.

There is no 0.6.0. The release after 0.5.0 was numbered 0.7.0 by mistake, and
a version cannot be published below the latest one without making `cargo
install` and the changelog disagree about what is current. Nothing was withdrawn
and nothing is missing - the number is simply unused.

## [0.10.4] - 2026-08-27

### Bug Fixes
- Make the tn alias a link instead of a copy of the binary
## [0.10.3] - 2026-08-19

### Bug Fixes
- Point Windows shells at the PowerShell installer
## [0.10.2] - 2026-08-19

### Documentation
- Include the flake fix in 0.10.2

### Testing
- Cover the transport against an in-process russh server
- Pin the v0.10.1 fixes the patch shipped without
- Retry ETXTBSY when running the freshly copied binary
## [0.10.1] - 2026-08-15

### Bug Fixes
- Reuse the SFTP session and report failures honestly
- Complete credentials and paths since the v0.9 split
- Journal edits and group changes, drop the agent label
- Forward websocket close frames, stop cloning headers

### Refactoring
- Dedupe the shared helpers grown apart since v0.9
## [0.10.0] - 2026-08-14

### Breaking Changes

- **Move the SSH transport to russh, unify TLS on rustls**
ssh-agent auth is dropped. It rode on libssh2's
userauth_agent; russh reaches an agent through a separate integration, deferred
to the key-setup stage. Authentication is now the credential's key file or the
stored password. A key credential that relied on the agent needs its key file
set explicitly (`turnout credential edit NAME --key PATH`). See ADR 0011.

### Features
- Move the SSH transport to russh, unify TLS on rustls
## [0.9.1] - 2026-08-14

### Bug Fixes
- Clear the way out of a data directory this build refuses
## [0.9.0] - 2026-08-14

### Breaking Changes

- **Split servers into servers, credentials and paths**
schema 1 data directories are refused, not migrated - the new
entities need names turnout cannot invent. The refusal sets a readable copy
aside in settings-backup-v1 and names the commands to re-enter with; the
originals are untouched. Exports move to format 2 for the same reason.

### Documentation
- Document credentials, paths and the move off the old shape

### Features
- Split servers into servers, credentials and paths
## [0.8.0] - 2026-08-13

### Bug Fixes
- Restore the terminal and kill the child tree on Ctrl+C

### Documentation
- Strip the product prefix from page titles

### Testing
- Hold port reservations until handoff
## [0.7.1] - 2026-08-13

### Documentation
- Record that 0.6.0 does not exist

### Features
- Render progress as a clack-style checklist
## [0.7.0] - 2026-08-12

### Documentation
- Cover Windows servers and refresh the versions in examples

### Features
- Speak the server's own shell dialect
## [0.5.0] - 2026-08-11

### Bug Fixes
- Split write_private per platform instead of returning early

### Documentation
- Explain updating with the gateway or dev running
- Refresh versions and cover this release on the landing pages

### Features
- Notice new releases without delaying a command
- Update the binary from the latest release
- Export and import a setup between machines
- Send the artifacts as one archive
- Migrate the data directory when the schema changes
## [0.4.1] - 2026-08-11

### Bug Fixes
- Resolve the release tag without the GitHub API
## [0.4.0] - 2026-08-11

### Bug Fixes
- One README across GitHub, crates.io and npm
- Keep byte units consistent between bar and summary
- Declare the MSRV that actually builds
- Refuse remote paths that the shell rewrote
- Explain why a backup was refused

### Documentation
- Show the daily workflow and refresh the status
- Rework the landing page and header spacing

### Features
- Report progress through every remote step
- Give every long flag a short form

### Refactoring
- Route every prompt through one interactivity guard

### Testing
- Add a demo app to deploy against a real server
- Hold the port instead of rebinding its number
## [0.3.0] - 2026-08-09

### Bug Fixes
- Download the binary lazily when postinstall is skipped

### CI
- Publish to crates.io and npm via OIDC trusted publishing
- Publish on tag push instead of the release event

### Features
- Apply the lacodda line identity
- Pick missing names interactively
- Complete app, server and group names in bash
- Install a short tn alias next to turnout
- Take commands from package.json scripts
- Add a setup wizard for deploy configuration
- Record actions to journal.jsonl
## [0.2.2] - 2026-08-06

### Bug Fixes
- Extract reliably on Windows and decouple wrapper version
- Suggest `gateway stop` when the port is taken
- Canonicalize project directories when storing and spawning

### CI
- Update actions to node 24 majors

### Documentation
- Add the daily Vite workflow how-to
- Move OS labels out of copyable install blocks

### Testing
- Cover drive-letter canonicalization and busy gateway port
## [0.2.0] - 2026-08-06

### Documentation
- Install via cargo install turnout, add the crates.io badge

### Features
- Support a private key file for SSH auth
- Proxy WebSocket connections
- Back up and restore the deploy directory on the server
- App groups switch a whole contour with one use
- One-line installers and the turnout-cli npm wrapper
## [0.1.0] - 2026-08-06

### Bug Fixes
- Drop the extra data segment from the data directory
- Resolve the app by canonical paths

### Build
- Exclude docs and CI config from the crates.io package

### CI
- Add cross-platform checks, docs deploy and git-cliff config
- Build tagged releases for three platforms

### Documentation
- Add Starlight site skeleton and founding ADRs

### Features
- Bootstrap CLI with setup and status commands
- Add app and server catalogs with wizards
- Store server access with secrets in the OS keyring
- Add the dev gateway with per-stand cookie jars and use
- Run app commands from any directory
- Deploy apps over SSH/SFTP with per-app server targets
- Add shell completion scripts for five shells
