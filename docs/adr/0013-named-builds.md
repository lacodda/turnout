# ADR 0013: A deploy target is a named build, not a field on the server

- Status: accepted
- Date: 2026-08-30

## Context

Since the v0.9.0 split (ADR 0010) a deploy needs four things: the app whose artifacts travel, the server they land on, the credential that logs in, and the path they are written to. Three of them were already free-standing entities. The fourth relationship - *which path this app uses on this server* - stayed inside the server as a map:

```json
{ "name": "prod", "url": "...", "deploy": { "web": "wwwroot", "api": "api-root" } }
```

Resolving a deploy therefore read from three places: the argument or the current directory gave the app, the `use` binding gave the server, and the server itself gave the credential and the path. That works, and it shipped for two minor versions. Three things were wrong with it in the long run.

**The connection has no name.** `turnout deploy web --server prod --credential root --path staging-root` is a sentence the user retypes; there is nowhere to say "this combination is the thing I call `web-staging`". Everything downstream on the roadmap wants that name: the health-check URL of v0.19 is a property of a deploy target, the deploy journal of v0.20 wants to record which target ran, and the "compositions" of 1.x are lists of deploy targets. Each of those would otherwise have to key on an anonymous four-tuple.

**The server owns a relationship it is not part of.** `deploy` is a map from app to path; the server contributes nothing to the pair except being the third coordinate. This is the same shape ADR 0010 rejected when it pulled `user@host` and the per-app directories out of the server - a relationship hidden inside one of its participants cannot be addressed, listed, or reused.

**A deploy could be assembled but never stored.** `turnout deploy web -s prod -C root -p staging-root` works, and the next run has to say it again, because there is no entity for what was just described.

The alternative considered was to add builds alongside `server.deploy` and let both resolve a deploy - no migration, nothing taken away. It was rejected: two ways to say the same thing means every later feature has to implement both, and 1.0 would freeze the pair. With no third-party users yet, the cost of breaking is a migration written once, and the cost of not breaking is paid by every subsequent release.

## Decision

**A `Build` is the deploy target: a named `(app, server, credential, path)`. It is the only way a deploy is addressed, and `Server::deploy` is removed.**

- Builds live in their own catalog, `builds.json`, like every other entity (ADR 0007).
- `turnout deploy NAME` resolves the whole four-tuple from the build, from any directory.
- `turnout deploy` with no name still works the way it did: the app comes from the current directory, the server from the `use` binding, and the credential from the server. Where that used to end in `server.deploy`, it now finds the app's build for that server. When there is none, the pickers ask, and the result is offered for saving as a build.
- `--server`, `--credential` and `--path` remain single-run overrides. They now override the build's fields rather than the server's, which is the same promise from the user's side: "this run only, use that instead".
- The schema goes to 3 with a real migration. Every `server.deploy[app] = path` entry becomes a build named `{app}-{server}`, taking the credential from the server it was found on.

**The migration names things, and this is not a contradiction of ADR 0005.** The refusal to migrate schema 1 stood on a specific point: the v0.9.0 split had to invent names for entities the user had never named - a credential called `prod-cred-1` is a name nobody recognizes six months later. Here every part of the generated name was chosen by the user already: `web` is their app, `prod` is their server, and `web-prod` is the concatenation. Nothing is invented, only joined. The same generator serves the "save this as a build?" prompt, so it is one mechanism, exercised on both paths.

A build whose generated name collides with an existing one gets a numeric suffix rather than overwriting - a collision means two servers or two apps produced the same pair, and losing one silently is worse than an ugly name.

## Consequences

- `turnout server edit --deploy-path` is gone; `turnout build add` replaces it. `server show` no longer lists deploy targets, `build list` does.
- The four-way join lives in one place, so `deploy`, `backup` and `restore` resolve identically - they already shared `remote::resolve`, which now reads one entity instead of three.
- Later features attach to a named thing: a health-check URL, a deploy history entry and a composition member are all "a build", not a tuple that has to be re-derived.
- Anyone on schema 2 is migrated on the first command, with a copy of the old files set aside as every rewriting migration does, and a line naming each build that was created.
- This is the first migration that actually rewrites. The machinery for it - the safety copy, the error wrapping - existed since v0.9.0 and had never run; it runs now.
