# ADR 0010: Servers, credentials and paths as three entities

- Status: accepted
- Date: 2026-08-14

## Context

Through v0.8 a `Server` carried everything a remote operation needed: the base URL, one `Ssh { host, port, user, key }`, and a `BTreeMap<app, DeployTarget>` holding an inline directory and restart command per app. Secrets were keyed in the OS keyring by `server/kind`.

That shape made three things impossible to say without repeating yourself:

- **One account, several machines.** A deploy user reaching four stands was entered four times, and its password stored four times under four keyring accounts. Rotating it meant four `pass set` runs.
- **One directory role, several machines.** `/var/www/myapp` and its `systemctl restart myapp` were retyped per server, and drifted.
- **Two accounts, one machine.** Deploying as `deploy` while running maintenance as `root` had nowhere to live: the server held exactly one user.

v0.10.0 introduces named builds (app + server + credential + path). Those cannot be named until the parts exist separately.

## Decision

Split into three entities, referenced by name:

- **Server** - `name, label, url, host: Option<String>, port, accept_invalid_certs, shell, credential: Option<String>, deploy: BTreeMap<app, path_name>`.
- **Credential** - `name, user, auth: Password | Key, key: Option<String>`. Free-standing; the secret lives in the keyring under the credential's *name*.
- **Path** - `name, dir, restart: Option<String>`. Free-standing, deliberately **not** owned by a server.

Two boundary calls are worth recording:

**The base URL stays on the server.** It is what the gateway routes to, and for a stand "which machine" and "which URL" are one answer. `host` is optional and falls back to the URL's host, so the common case needs no second entry. Moving the URL out would have split the gateway's model as well, for no gain.

**Paths are not scoped to a server.** Scoping would let the picker filter by server and prevent a mismatched pairing, at the cost of duplicating every shared directory per machine - the exact duplication this ADR removes. The mismatch is cheap to catch at link time (`server edit --deploy-path` validates both halves exist) and the duplication is not.

Secrets move to the credential name because the secret belongs to the account, not to the machine it happens to reach.

## Consequences

- `deploy`, `backup` and `restore` resolve four entities instead of two, and take `--credential` / `--path` to override the last two for one run. That tuple is exactly what a v0.10.0 build will name.
- `pass` operates on credential names; `--kind` and `--login` are gone. `pass remove` deletes the secret and keeps the credential - rotating a password should not cost the account.
- Removing a server no longer deletes credentials, since they are shared. Removing a credential clears it from the servers that named it; removing a path unlinks the same way and never touches the directory on the server.
- **No automatic migration** (see ADR 0005 for the same reasoning applied to the predecessor's configs): the new entities need names, and generated ones like `prod-cred-1` would be worse than ten minutes of re-entry. Schema 1 directories are refused with instructions and a readable copy set aside; the originals are untouched, so v0.8 keeps working if reinstalled. Export format moves to 2 and refuses version-1 files for the same reason.
- The migration machinery shipped in v0.5.0 is unused by this change and stays for the next break that *can* be automated. It gained one flag: a step may declare that it rewrites nothing, so a refusal leaves no backup folder behind for an operation that never ran.
