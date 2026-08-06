# ADR 0008: Bindings travel through the state file, not IPC

- Status: accepted
- Date: 2026-08-05

## Context

`turnout use` must retarget a gateway that runs as a separate background process. Classic options - a control socket, named pipes, signals - are platform-divergent (Windows vs Unix) and add a protocol to version.

## Decision

There is no IPC. `use` writes `state.json`; the gateway re-reads the binding (and the server catalog) on every request. A request costs a microsecond-scale local file read, which is negligible for a single-developer proxy.

`gateway start` records the child pid and ports in the same state file; liveness is checked by probing a recorded port, and `stop` kills the recorded pid.

## Consequences

- `use` needs zero coordination: the very next request goes to the new stand.
- Catalog edits (server URL, TLS policy) also apply without a gateway restart.
- The state file is the single source of truth for "who points where" - visible, diffable, debuggable.
- A stale pid after a crash is possible; `status` reports "recorded but not responding" and `stop` clears it.
- Per-request file reads would be wrong for a high-traffic proxy; this is a deliberate single-user trade-off.
