# Changelog

All notable changes to this project are documented in this file.

## [0.5.0] - 2026-08-11

### Documentation
- Explain updating with the gateway or dev running

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
