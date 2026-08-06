# turnout

> A developer's switchyard: point local apps at any backend stand, keep servers and secrets at hand, build and deploy - from any directory.

<p align="center">
  <a href="https://github.com/lacodda/turnout/actions"><img src="https://img.shields.io/github/actions/workflow/status/lacodda/turnout/ci.yml?style=flat-square" alt="CI"></a>
  <a href="https://github.com/lacodda/turnout/blob/main/LICENSE"><img src="https://img.shields.io/github/license/lacodda/turnout?style=flat-square" alt="License"></a>
</p>

**Status: early development.** The CLI skeleton is in place; the entity model, secrets and the dev gateway are being built - see the roadmap below.

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

From source, for now:

```bash
git clone https://github.com/lacodda/turnout.git
cd turnout
cargo install --path .
```

Binary releases for Windows, macOS and Linux will come with the first tagged version.

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
- [ ] Dev gateway: per-stand cookie jars, self-signed HTTPS, WebSocket
- [ ] `use` - bind an app to a stand with one command
- [ ] App commands: `dev` / `build` / `test` / `lint` from any directory
- [ ] Deploy: build, package, upload, restart
- [ ] Comfort: rich `status`, shell completions, app groups, config migrations

## Documentation

The documentation site (Astro Starlight) lives in [`docs/`](docs/); architecture decision records are in [`docs/adr/`](docs/adr/).

## License

MIT (c) [Kirill Lakhtachev](https://lacodda.com)
