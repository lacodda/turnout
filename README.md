<p align="center"><img src="assets/banner.svg" alt="turnout - a developer's switchyard" width="720"></p>

> A developer's switchyard: point local apps at any backend stand, keep servers and secrets at hand, build and deploy - from any directory.

<p align="center">
  <a href="https://crates.io/crates/turnout"><img src="https://img.shields.io/crates/v/turnout?style=flat-square" alt="crates.io"></a>
  <a href="https://www.npmjs.com/package/turnout-cli"><img src="https://img.shields.io/npm/v/turnout-cli?style=flat-square" alt="npm"></a>
  <a href="https://github.com/lacodda/turnout/actions"><img src="https://img.shields.io/github/actions/workflow/status/lacodda/turnout/ci.yml?style=flat-square" alt="CI"></a>
  <a href="https://github.com/lacodda/turnout/blob/main/LICENSE"><img src="https://img.shields.io/github/license/lacodda/turnout?style=flat-square" alt="License"></a>
</p>

## Why

Day-to-day work with several backend stands is scattered:

- to run a project you have to `cd` into its folder;
- to switch a stand you edit `.env` files across repositories;
- to deploy you must remember configs, passwords and server paths;
- to copy a login or password you dig through notes and chats.

turnout puts all of it into one CLI that works from any directory:

- **Dev gateway.** Your apps always talk to `localhost`; turnout routes requests to the selected stand. Switching a stand is one command - and sessions survive it, because the gateway keeps a cookie jar per app+server pair.
- **Secrets in the OS keyring** (Windows Credential Manager / macOS Keychain / Linux Secret Service). Copy a password to the clipboard with one command; nothing is stored in plain files.
- **App commands anywhere.** `dev`, `build`, `test`, `lint` and custom commands run in the right project folder for you.
- **Deploy.** Build, package, upload over SSH/SFTP, restart the service - using the same apps, servers and secrets.

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

The installers and the npm package also register the short alias `tn` (skipped if the name is already taken; `TURNOUT_NO_ALIAS=1` opts out).

## Quick start

```bash
turnout setup    # first-run wizard: creates the data directory
turnout status   # what turnout knows right now
```

Data lives in the platform user data directory (e.g. `%LOCALAPPDATA%\lacodda\turnout` on Windows); set `TURNOUT_DATA_DIR` to override.

## Roadmap

- [x] CLI skeleton: `setup`, `status`
- [x] Apps and servers: CRUD with interactive wizards
- [x] Secrets: OS keyring storage, copy to clipboard
- [x] Dev gateway: per-stand cookie jars, self-signed HTTPS, redirect rewriting
- [x] `use` - bind an app to a stand with one command
- [x] Gateway: WebSocket proxying
- [x] App commands: `dev` / `build` / `test` / `lint` from any directory
- [x] Deploy: build, upload over SSH/SFTP, restart
- [x] Deploy: remote backup and restore
- [x] Shell completions
- [x] App groups: switch a whole contour with one `use`
- [ ] Comfort: config templates and migrations, environment profiles

## Documentation

The documentation site (Astro Starlight) lives in [`docs/`](docs/); architecture decision records are in [`docs/adr/`](docs/adr/).

## License

MIT (c) [Kirill Lakhtachev](https://lacodda.com)
