<p align="center"><img src="https://github.com/lacodda/turnout/raw/main/assets/banner.svg" alt="turnout - a developer's switchyard" width="720"></p>

> A developer's switchyard: point local apps at any backend stand, keep servers and secrets at hand, build and deploy - from any directory.

<p align="center">
  <a href="https://crates.io/crates/turnout"><img src="https://img.shields.io/crates/v/turnout?style=flat-square" alt="crates.io"></a>
  <a href="https://www.npmjs.com/package/turnout-cli"><img src="https://img.shields.io/npm/v/turnout-cli?style=flat-square" alt="npm"></a>
  <a href="https://github.com/lacodda/turnout/actions"><img src="https://img.shields.io/github/actions/workflow/status/lacodda/turnout/ci.yml?style=flat-square" alt="CI"></a>
  <a href="https://github.com/lacodda/turnout/blob/main/LICENSE"><img src="https://img.shields.io/github/license/lacodda/turnout?style=flat-square" alt="License"></a>
</p>

## Why

Working against several backend stands scatters the day: you `cd` into a folder to start a project, edit `.env` files across repositories to switch a stand, dig through notes for a password, and keep deploy paths in your head.

turnout keeps all of it in one place and works from any directory.

## A day in the life

Point an app at a stand:

```console
$ turnout use web staging
'web' now uses 'staging'.
The running gateway picks this up automatically.
Stand check: https://staging.example.com responded with 200 OK.
```

Nothing in the project changed - the app still talks to `localhost`, and the [gateway](https://lacodda.github.io/turnout/concepts/gateway/) routes it to the stand you picked. Your session survives the switch, because cookies are kept per app **and** stand.

Start working, from wherever you happen to be:

```console
$ cd ~/dev/web/src/components
$ turnout dev
[web] pnpm dev
```

Move the whole contour at once when the frontend and the API must agree:

```console
$ turnout use contour prod-eu
Group 'contour' now uses 'prod-eu':
  web -> prod-eu
  api -> prod-eu
```

Forgot a name? Leave it out and pick from a list that shows where things point:

```console
$ turnout use
? Switch ›
❯ contour   group: web, api
  api       -> staging
  web       -> staging
```

Ship it:

```console
$ turnout deploy web -s prod-eu -b
[web] pnpm build
✓ Connected to deploy@prod-eu.example.com:22
✓ Backup 20260809-011500.tar.gz created in /var/www/web.backups
================>          2.02 MiB/3.11 MiB · 1.81 MiB/s · eta 1s  assets/index-b3f0a1.js
Uploaded 142 files (3.11 MiB) to prod-eu:/var/www/web
✓ Ran: systemctl restart web
Deploy of 'web' to 'prod-eu' finished.
```

Every long flag has a short form, and nothing runs silently: the upload reports throughput and an ETA, and the steps that talk to the server say so while they wait.

And see what has been going on:

```console
$ turnout status
turnout 0.7.0
Data directory: ~/.local/share/lacodda/turnout
Apps:    2 (api, web)
Servers: 2 (prod-eu, staging)
Group:   contour (web, api)
Access:  saved for prod-eu
Bindings:
  api -> staging
  web -> prod-eu
Gateway: running (pid 24180; web:7100, api:7101)
Recent:
  2026-08-09T01:15:02Z  deploy         web -> prod-eu (142 files)
  2026-08-09T01:12:44Z  use            web -> prod-eu
```

## What you get

- **A dev gateway.** Apps always talk to `localhost`; turnout forwards to the selected stand over HTTP or HTTPS (self-signed certificates allowed per server), rewrites redirects, proxies WebSockets, and keeps a cookie jar per app+stand pair so switching does not log you out.
- **Secrets in the OS keyring** - Windows Credential Manager, macOS Keychain, Linux Secret Service. Copy a password to the clipboard with one command; nothing lands in a config file, and `status` only ever reports *that* a credential exists.
- **Commands from any directory.** `dev`, `build`, `test`, `lint` and any custom command run in the right project folder. Commands are taken from your actual `package.json` scripts, so a project whose dev script is `serve` still answers to `turnout dev`.
- **Deploy over SSH/SFTP** - build, upload, restart, with remote backup and restore when a release goes wrong. Artifacts travel as a single archive instead of thousands of round trips, falling back to file-by-file when the server cannot unpack one. Linux and Windows servers alike: turnout detects which shell answers SSH and phrases every remote command in it.
- **Portable settings.** `export` writes your apps, servers and groups to one file and `import` merges it on another machine; secrets come along only when you ask, sealed with a passphrase.
- **Stays current.** A once-a-day check mentions a new release without ever delaying a command, and `self-update` installs it - leaving package-manager installs to their package manager.
- **Groups.** Bind a whole contour to one stand with a single `use`.
- **Nothing to memorize.** Leave a name out and pick it from a list; in bash, Tab completes app, server and group names from your own catalogs. The short alias `tn` is installed alongside.
- **An action journal.** Every state change appends one JSON line - what happened and to which entities, never secrets or output. `tail`, `grep` and `jq` work on it directly.

## Install

**One-line installers.** Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.ps1 | iex
```

macOS / Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.sh | sh
```

**With npm:**

```bash
npm i -g turnout-cli
```

**With cargo:**

```bash
cargo install turnout
```

**Binary releases** - grab the archive for your platform from [Releases](https://github.com/lacodda/turnout/releases/latest) (Windows x86_64, Linux x86_64, macOS arm64), unpack and put `turnout` on your `PATH`.

The installers and the npm package also register the short alias `tn` (skipped if the name is already taken; `TURNOUT_NO_ALIAS=1` opts out). `cargo install` gives you `turnout` only.

Both installers take the newest release by default; set `TURNOUT_VERSION` to a tag to pin one, and `TURNOUT_INSTALL_DIR` to choose where the binary lands.

## Quick start

```bash
turnout setup                  # first-run wizard: creates the data directory
turnout app add                # register a project (detects its commands)
turnout server add             # register a stand
turnout use                    # bind one to the other
turnout gateway start          # route traffic through the gateway
```

Data lives in the platform user data directory (e.g. `%LOCALAPPDATA%\lacodda\turnout` on Windows); set `TURNOUT_DATA_DIR` to override.

Full command reference and concepts: **[lacodda.github.io/turnout](https://lacodda.github.io/turnout/)**.

## Status

Everything above works today. What is next:

- [ ] **Key-based access, set up rather than only used** - generate a key, install it on the server and verify it in one command, including the Windows administrator case
- [ ] **Credentials and paths as their own entities** - one login reused across servers, a remote directory declared once, and named builds so a deploy stays a single word
- [ ] **Background runs** - `dev --detach`, `ps`, `logs`, `stop`, OS notifications
- [ ] **Observability** - gateway request log, `doctor`, `report` for handing context to an assistant
- [ ] **Deploy consists** - atomic deploy and rollback across a group of apps

Released versions and what landed in each: [CHANGELOG on the Releases page](https://github.com/lacodda/turnout/releases).

## Documentation

The documentation site (Astro Starlight) lives in [`docs/`](https://github.com/lacodda/turnout/tree/main/docs); architecture decision records are in [`docs/adr/`](https://github.com/lacodda/turnout/tree/main/docs/adr).

## License

MIT (c) [Kirill Lakhtachev](https://lacodda.com)
