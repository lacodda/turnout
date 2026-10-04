# ADR 0021: A token-guarded control channel for the gateway

- Status: accepted
- Date: 2026-10-04

## Context

[ADR 0008](0008-state-file-over-ipc.md) keeps the gateway free of IPC: `use` writes `state.json`, and the gateway reads it per request. v0.22 needs two more things from a running gateway.

- **Its observations** - a stand answering 502, a session the stand refused - must reach `status`. These are facts about the gateway, and the gateway can write them where everything else about it already lives: its own job record ([ADR 0018](0018-a-registry-of-jobs.md)), of which it is the one writer.
- **Cookies from a browser** must reach its jar, for stands whose single sign-on cannot be passed through the gateway. These are session secrets, and the jar is in memory precisely so that they never touch the disk ([ADR 0002](0002-cookie-jar-gateway.md)). A hand-off file the gateway picks up would put them there, if only for a while, and could not confirm the hand-off at all.

## Decision

Observations travel the ADR 0008 way: the gateway writes its signals into its job record when they change, through one writer task so the order of changes is kept; `status` reads the file.

Cookies travel over a **control channel**: one more listener of the gateway, on a loopback port the system picks, speaking plain HTTP - the gateway is an HTTP server already, so there is nothing platform-specific in it. The gateway generates a 32-byte random token on start and writes the port and the token into its job record. Every request must carry the token as a bearer credential; the comparison does not stop early.

The channel has three requests: replace the jar of an app on a server, empty it, and list the jars - by cookie *name*. Values go in and never come out.

## Consequences

- A web page can send a request to any `localhost` port but cannot read the user's files, so it can neither plant a session nor learn which ones exist. The record sits in the user's data directory with the user's permissions.
- Cookies stay in memory only; a restarted gateway forgets them, as it forgets sessions made through it.
- `cookie set` gets an answer - the gateway took the cookies, or why not.
- A gateway started by an older turnout has no channel; `cookie` asks for a restart.
- The channel is a protocol inside one binary: client and server ship together, so it is not versioned. Later needs that are about the running gateway (the doctor's cookie check, a TUI) go through it too, rather than through a second mechanism.
