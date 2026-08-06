# ADR 0004: No env-file generation fallback

- Status: accepted
- Date: 2026-08-05

## Context

The predecessor tool switched stands by writing `.env.development.local` into each project. It works, but scatters state across repositories, requires dev-server restarts, and leaves projects pointing at stale stands. With the gateway (ADR 0002), apps point at localhost permanently and the binding lives in turnout.

## Decision

turnout does not generate or modify env files in user projects - not as a mode, not as a transition path. The gateway is the only stand-switching mechanism.

## Consequences

- One source of truth for "which stand am I on"; no split-brain between env files and gateway bindings.
- Projects' env files become stable and committable.
- turnout is not useful for stand switching until the gateway ships - accepted: a half-measure would linger forever.
