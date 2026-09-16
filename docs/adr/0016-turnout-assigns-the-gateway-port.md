# ADR 0016: turnout assigns the gateway port, and assigns it early

- Status: accepted
- Date: 2026-09-16

## Context

An app in turnout has two ports. The **dev port** is where its dev server listens; the **gateway port** is where turnout's per-app proxy listens, and it is the address the app is handed so that its own code never names a stand.

Since v0.15.0 the dev port is turnout's to assign: it is taken from `5100-5199` on the first `turnout dev` and kept from then on. The gateway port was not. It was a question in the `app add` wizard ("Local gateway port (empty for none)") with a suggested free number, and a `--port` flag.

That question has no good answer. The user has no basis for preferring 7100 to 7103 - any free port does the job, and *which* ports are free is something turnout can check and the user cannot. It was also the first question in the form with a wrong answer available: pressing enter on "empty for none" produced an app with no address at all, whose `{gateway}` commands then refused to run and whose dotenv file held nothing. The owner named ports "the most unpleasant part" of the tool, and v0.14-v0.15 had already taken the *dev* port out of sight; the gateway port was the last one left in the user's hands.

The suggestion the wizard offered was also quietly wrong. `free_gateway_port` walked upward from 7100 looking only for a number **no app in the catalog held** - it never tried to bind. A port some other process on the machine was listening on was therefore offered as free, accepted, and only surfaced later as the gateway failing to bind that listener.

## Decision

**turnout assigns the gateway port from `7100-7199` when an app is registered, and never asks for it. `--port` pins a specific one; `--port 0` hands it back.**

Two things follow from this, and the second is the one that took thought.

**The range is probed, not just counted.** A port is offered only if no app holds it (of either kind - the two ranges are checked against one list) *and* `TcpListener::bind` succeeds on it. This is the same function the dev port uses; the two now differ only in which range they walk.

**The gateway port is assigned early - at `app add` - while the dev port stays late, at the first `dev`.** The plan for this stage said "as for the dev port", and following that literally would have meant assigning it on first use. That is wrong here, because the two ports differ in when something needs the number:

- Nothing needs a dev port until a dev server is actually started. Assigning it at `dev` costs nothing.
- The gateway port is written into the app's dotenv file (`VITE_API_URL=http://localhost:7100`) the moment the app is saved, and substituted into `{gateway}` command lines. An app whose port arrived only when the gateway first started would be an app whose `.env.development.local` is empty in the meantime - and the whole point of that file is the path where the developer runs `pnpm dev` from an IDE, with turnout not in the loop at all.

So the rule is not "assign lazily" but **assign before anything can ask**, and for these two ports that lands in different places.

## Consequences

- The wizard is one question shorter, and no path through it produces an app without an address. The form's gate - every field of `App` is either walked or named as deliberately outside the form - carries `gateway_port` in the second list with this reason.
- Schema 3 -> 4 gives every app that has no gateway port one. **A port already written is kept exactly as it stands**, including a number from outside the range: it is already in a dotenv file, and possibly in a `.env` a colleague copied, so renumbering would be a silent break dressed as an upgrade. The migration only fills gaps.
- `--port 0` is now how an app ends up with no gateway port. That is a real configuration - an app that talks to its stand directly - so it stays reachable, just no longer by pressing enter.
- The ranges are a hundred wide each. Exhausting one is an error naming `--port`, not a silent app without an address.
- Because the port is no longer something the user chose, it is no longer presented as the app's identity. `status`, `app list` and `app show` lead with `http://NAME.localhost` and print the port as the **spare** address behind it.
