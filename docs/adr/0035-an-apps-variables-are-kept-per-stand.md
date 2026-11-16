# ADR 0035: An app's variables are kept per stand, and reach it by one road

- Status: accepted
- Date: 2026-11-15

## Context

turnout kept the secrets of *access* - a credential's password, a key's passphrase - in the OS keyring (ADR 0003), and left the secrets of the *app* where it found them: a `.env` in the project, in plain text, one copy per stand, renamed by hand when the stand changed, and copied to the server by `scp` or by memory. ADR 0004 refused an env-file fallback for turnout's own secrets for exactly that reason, and the app's were the larger pile.

v1.2.0 takes them in. The questions were where the values live, what a set belongs to, how it reaches a process here and a stand there, and what it means for a build.

## Decision

**A set belongs to an app and a server.** `turnout env set NAME` stores a variable of one app on one stand; the pair defaults to the app of the current directory and the stand it points at - the pair `dev` is about to run with. There is no layer shared between stands: a value the same on two stands is two values that happen to match, and a shared layer is where a production password leaks into a staging run.

**The values are in the keyring, the names in a catalog.** Each value is a keyring entry under `env/APP/SERVER/NAME` - a name no credential can have - and `env.json` holds only the names. The catalog is what lets `env list` answer without the keyring, what an export carries without secrets, and what makes a missing value an error that names the variable instead of a variable silently absent. A value is stored in pieces of at most 1000 UTF-16 units: Windows Credential Manager holds 2560 bytes per entry, and a private key or a signed token is longer. An empty value has no piece and nothing is stored for it.

**`dev` and `run` get the set of the stand the app points at; `build`, `test` and `lint` get none.** The same line the gateway's address is drawn on: a variable in the process environment overrides every dotenv file, and a bundle built here must not bake in the stand this machine happens to point at. What turnout sets itself - the gateway's address, `PORT` - wins a name, and the gateway's variable cannot be stored at all: it is this machine's, and a stored copy would travel to a stand.

**A deploy's build gets the set of the stand it goes to.** A deploy is the one build that knows where its bundle ends up. An app is built once per run (ADR 0013), so a run that takes one app to two stands, either holding variables, is refused before the build: one bundle would carry one stand's values to the other.

**A stand's env file is written from the same set, and only when its target asks.** `target edit --env-file FILE` names a file under the deploy directory or an absolute one; every deploy of the target writes the set there after the build is in place and before the restart. It is opt-in, never inferred from the set being non-empty: a deploy directory is often a web root, and a `.env` written there unasked is a file a web server hands out. The file says what the set says - an empty set still writes it - so a variable removed here does not live on there. A new file is created `0600` before a value is in it; an existing one keeps the permissions it has, since SFTP writes into the file. A rollback writes the variables as they are now: it puts back a build, not the secrets of the day it was made.

**`--with sefy:NAME` brings variables from sefy for one run, through sefy's own command line.** `sefy get NAME --stdout`, nothing else: sefy unlocks its vault as it always does and knows nothing of turnout. A record read whole is `NAME=value` lines; `VAR=sefy:NAME[#FIELD]` is one value. Nothing it hands over is stored. The `sefy:` names the source, so another store would be another word rather than another flag.

**Variables travel in an export as secrets do.** The names always; the values only with `--with-secrets`, in the same sealed block, and on import never over a value the machine already has unless forced. The data directory moves to schema 8 and the export to format 6: an older turnout would drop a target's env file and deploy a stand that starts without its variables.

## Consequences

- An app run through turnout needs no `.env`; `env import` moves an existing one over, and leaves the file for its owner to delete.
- Removing an app or a server removes its variables, from the keyring too.
- A message never quotes a line it could not parse - in a dotenv file or a sefy record it may be half of a secret.
- The env file's dialect is the common one: bare values, single quotes for the rest, double quotes with escapes only for a value holding a single quote or a line break.
- Tested through the file backend like every secret; the real Windows keyring was exercised by hand with a value of five pieces, and `--with` against a real sefy vault.
