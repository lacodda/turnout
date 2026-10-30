# Changelog

All notable changes to this project are documented in this file.

There is no 0.6.0. The release after 0.5.0 was numbered 0.7.0 by mistake, and
a version cannot be published below the latest one without making `cargo
install` and the changelog disagree about what is current. Nothing was withdrawn
and nothing is missing - the number is simply unused.

## [0.27.0] - 2026-10-30

### Breaking Changes

- **Say what 0.27 changes for scripts**
`cookie set` reads standard input only with `--stdin`; piped
without it, the clipboard is read. `server remove` of an app's only allowed
server needs `--force`, and `app edit --rm-server` of its last one is refused -
lift the list with `--any-server`. Without a terminal the only credential or
path is no longer taken for a missing name: pass `-C`/`-p` or the name. A
filesystem root is refused as a deploy directory - `path edit NAME --dir DIR`.
Settings and exports are unchanged.

### Bug Fixes
- Give every arriving app a port of its own
- Never write over a backup taken in the same second
- Say a job that never started did not start
- Migrate an older data directory instead of retiring it
- Write a secret only for a credential the import wrote
- Hold arriving entries to what add holds them to
- Refuse a filesystem root as a deploy directory
- Refuse a missing credential or path name without a terminal
- Check a pinned port against both kinds of port
- Look at --port before handing out a gateway port
- Hold target edit --server to the app's allow-list
- Never empty an allow-list as a side effect
- Forget a group that app remove empties
- Take the managed block out of a dotenv file left behind
- Reuse the safety copy of an unfinished migration
- Leave the update cache and the file keyring in place
- Reach the Windows OpenSSH agent at \\.\pipe\
- Sign with an RSA hash the server accepts
- Count an earlier rollback as a build the stand ran
- Count a deploy that failed while writing as written
- Let --clear pass on a directory that is not there yet
- Unpack the backup even when clearing left something
- Write the archive aside and rename it into place
- Stop waiting for a restart whose process is gone
- Write a generated key only after the password signs in
- Append the key on a line of its own, verbatim on Windows too
- Ask sudo about the restart command, not about everything
- Forget the learned shell when the address changes
- Make missing parents and keep the separator file by file
- Copy only the secret the prompt will ask for
- Remove a partly uploaded archive, and correct stale comments
- Let Ctrl+C reach a foreground gateway's shutdown on Unix
- Wait for the job's process group, not only the supervisor
- Key the stand clients by their TLS policy too
- Hand a page to the Windows shell without a command line
- Keep a failure's log and notes before the record says it ended
- Do not report a job stopped with Ctrl+C as failed
- Say when the running gateway does not listen for the app
- Point the login link at the door that actually opened
- End a port's holder only while it is the process found
- Claim a job slot under a lock
- Remove a record only while it is the one judged
- Never let a failed log line take the signal board down
- Read standard input only when --stdin asks for it
- Rewrite a redirect only from beneath the address it left
- Make comments match the code, compare the toast icon by content
- Read a port holder's birth where the holder is found
- Keep a stopped job quiet whichever process dies first
- Give a Vite app added by flags what Vite reads
- Say what --server overrides, and keep each group member once

### Build
- Adopt the line's standard

### Documentation
- Say the status in one paragraph
- One upgrading guide for every version
- Walk a newcomer through what they actually need
- Say what an export carries and how far back it imports
- Make help texts say what the commands do
- Describe the lock on a job slot
- Say what a Vite project gets by default
- One upgrading guide, and example names that are not a real stand
- Say what 0.27 changes for scripts

### Testing
- Import the export of every release since v0.11
- Isolate every turnout under test from the machine
- Hold every flag of every command to its reference page
- Make the version and installer checks fail on nothing
- Name example stands neutrally
- Let the detached job outlive a loaded suite
- Wait out ETXTBSY before handing the fake ssh client over
- Take the machine and the clock out of four tests' verdicts

### style
- Apply rustfmt to the stabilisation fixes

## [0.26.0] - 2026-10-22

### Bug Fixes
- Give Windows key files the ACL sshd expects of them
- Keep stdout for the JSON a command was asked for
- Reset a key file's ACL before restricting it

### CI
- Prove every push and tag against a real Windows sshd

### Features
- Show the learned shell and forget it on a new address

## [0.25.0] - 2026-10-16

### Features
- Deploy history, rollback to any recorded deploy, backup retention

## [0.24.1] - 2026-10-13

### Bug Fixes
- Name a health path Git Bash rewrote, and take //path for /path

## [0.24.0] - 2026-10-13

### Bug Fixes
- Quote a silent command's exit code without a dangling colon
- Hash a Windows server's files with .NET, not Get-FileHash

### Documentation
- Describe health checks and rollback, --diff, several targets and the detached restart

### Features
- Checked deploys that roll back, --diff, several targets, a restart that outlives the connection

### Testing
- Keep unit tests out of the real data directory, wait out a listener row that outlives its socket

## [0.23.0] - 2026-10-08

### Documentation
- Describe the doctor, the cookie jar's semantics and the session check

### Features
- Doctor, a jar that honours Set-Cookie, dev that minds its port

## [0.22.0] - 2026-10-04

### Documentation
- Describe status, the prompt line, JSON output, gateway signals and cookies

### Features
- Status in sections and a prompt line, --json, use -, gateway signals, cookie set

## [0.21.0] - 2026-09-25

### Bug Fixes
- Build the toast XML only for Windows and the tests

### Build
- Give the Windows main thread the 8 MiB stack other platforms have
- Keep the changelog's own commit out and space its releases

### Documentation
- Add the 404 page and i18n collection the build warned about
- Describe notifications, kept failures and the background default

### Features
- Notify when a background job is done, keep failed logs

## [0.20.0] - 2026-09-23

### Documentation
- Describe background jobs, ps, logs and stop

### Features
- Run jobs in the background and keep a registry of them

## [0.19.0] - 2026-09-21

### Bug Fixes
- Ignore the dotenv file from the repository, not the app folder

### Documentation
- Replace the status checklist with what ships today
- Describe the quiet console and what it keeps

### Features
- Hide a job's output behind a loader and keep it in a log

## [0.18.0] - 2026-09-16

### Documentation
- The port is turnout's to hand out

### Features
- Hand out the gateway port instead of asking for it
- Lead with the front door and call the port a spare

## [0.17.0] - 2026-09-09

### Documentation
- Show the wizard, not just the flag tables
- Record why the wizard is a list of fields

### Features
- Walk every field of an app in one form

## [0.16.1] - 2026-09-08

### Bug Fixes
- Dial the dev server by name, not by 127.0.0.1

### Features
- Vue and react demos that show the gateway at work

## [0.16.0] - 2026-09-08

### Features
- A hand on the server with ssh and exec

## [0.15.0] - 2026-09-08

### Features
- One address per app behind a front door

## [0.14.0] - 2026-09-08

### Features
- Hand the app the gateway address instead of the port

## [0.13.1] - 2026-09-07

### Bug Fixes
- Keep one port per app and report a gateway that never came up
- Build the icon from the level that reads at each size
- Keep the user PATH type on Windows
- Refuse to signal a pid that cannot be a process

## [0.13.0] - 2026-09-02

### Bug Fixes
- Match agent keys by key data, not by comment
- Assert what the agent protocol actually returns

### Features
- Sign in with a key held by the SSH agent

## [0.12.1] - 2026-08-31

### Bug Fixes
- Keep both names working when the update runs as tn

## [0.12.0] - 2026-08-31

### Features
- Set up SSH key access with one command

## [0.11.0] - 2026-08-30

### Breaking Changes

- **Address a deploy by a named target instead of a field on the server**
`turnout server edit --deploy-path` is gone and `Server`
no longer has a `deploy` field. Data directories migrate automatically
from schema 2 to 3: every `server.deploy[app] = path` becomes a target
named `{app}-{server}`, taking the server's credential. A server with a
deploy entry but no credential could never have deployed and yields no
target - the migration names those so they can be re-created. Export and
import move to format 3; a format-2 file converts its routes on import.

Naming things here does not contradict the schema-1 refusal. That one had
to invent a credential out of a `user@host` string, producing names like
`prod-cred-1` that mean nothing months later. Both halves of
`{app}-{server}` were typed by the user; this joins them, through the
same generator the interactive prompt uses. ADR 0013.

### Features
- Address a deploy by a named target instead of a field on the server

## [0.10.4] - 2026-08-27

### Bug Fixes
- Make the tn alias a link instead of a copy of the binary

### Testing
- Assert each platform's paths on that platform

## [0.10.3] - 2026-08-19

### Bug Fixes
- Point Windows shells at the PowerShell installer

## [0.10.2] - 2026-08-19

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

