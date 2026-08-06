# ADR 0002: Dev gateway keeps a cookie jar per app+server

- Status: accepted
- Date: 2026-08-05

## Context

The gateway routes a local app to a selected remote stand. Stands issue session cookies on login. If the browser held those cookies, switching stands would send one stand's session to another: at best a logout, at worst subtle auth bugs. Naively expiring cookies on switch forces a re-login every time and loses sessions.

## Decision

The browser talks only to the gateway and holds only the gateway's own session. Cookies issued by stands are stored inside the gateway, in a separate jar for every app+server pair. On switch, the gateway swaps jars; returning to a previous stand restores its live session.

Response bodies are not rewritten - apps are expected to use relative API URLs. `Location` headers of redirects are rewritten so the browser never leaves localhost.

## Consequences

- Stand switching is seamless and sessions survive it - the core UX promise of turnout.
- The gateway becomes stateful; jars are part of runtime state, not user catalogs.
- Cookie semantics (domain, path, expiry, Secure/HttpOnly) must be honored inside the jar implementation.
- Apps that embed absolute stand URLs in response bodies are out of scope; documented as a known limitation.
