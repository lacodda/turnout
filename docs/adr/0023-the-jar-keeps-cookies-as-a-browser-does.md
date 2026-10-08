# ADR 0023: The jar keeps cookies as a browser does, and can ask whether they still work

- Status: accepted
- Date: 2026-10-08

## Context

[ADR 0002](0002-cookie-jar-gateway.md) put the stands' cookies in the gateway and promised that cookie semantics would be honoured. Up to v0.22 the jar kept `name=value` and nothing else. A stand that ended a session with `Set-Cookie: sid=deleted; Max-Age=0` got `sid=deleted` back on every request after it; a cookie past its `Expires` was still sent; a cookie set for `Path=/api` went to every path.

v0.23's doctor also needs to know whether a session is alive *now*. The gateway learns that a session lapsed only when the app runs into it, and the morning after an idle night is exactly when nobody has yet.

## Decision

**The jar is an RFC 6265 cookie store** - `cookie_store`, which implements the storage and matching rules whole: the lenient date formats, `Max-Age` over `Expires`, the default path, domain matching for names and IP addresses, `Secure`. Writing those rules again would be a second, less tested copy. The dependency comes without its public suffix list and its serialisation; its own dependencies were already in the tree. Cookies are matched against the `http(s)` address of a request - a WebSocket's upgrade included, which is an HTTP request and gets the HttpOnly and Secure cookies a browser would send with it.

Cookies handed over with `cookie set` carry no attributes, so they are kept host-only, for every path, for the session.

**The jar remembers the request that proved the session**: the last one that was a `GET`, carried the jar's cookies, was answered 2xx without any cookie being deleted, and came back within two seconds - with the headers the app sent it. Only a `GET` is safe to send again unasked; a logout link deletes the cookie and never qualifies; a long poll would hold the check up.

**The session check sends that request again** through the gateway's control channel ([ADR 0021](0021-a-control-channel-for-the-gateway.md)): `POST /jar/{app}/{server}/check`, token-guarded like the rest. The stand's answer is the verdict - 2xx alive; 401, 403, 440 or a redirect expired; 5xx or silence the stand is down - and it also goes where every answer goes: the signal board and the jar. With no proving request yet, the verdict is "unproven", never a guess from `/`, which a single-page app answers with 200 whoever asks.

## Consequences

- A session the stand ends is ended in the gateway too, and an expired cookie stops travelling.
- `cookie list` and the doctor say when the first cookie expires and what the stand last answered.
- The check is as honest as the app's own traffic: it asks what the app asked, so an app that authenticates with a bearer header as well as cookies is checked the way it works.
- The proving request stays in the gateway's memory with the cookies; it never touches the disk and is never printed beyond its path.
- A stand that answers the same `GET` differently for reasons other than the session (a resource that was deleted) reads as "unclear", not as expired.
