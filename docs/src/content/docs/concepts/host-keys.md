---
title: Host keys
description: How turnout knows a server is the one it means - the host keys in OpenSSH's own known_hosts, learned on first contact and held to after.
---

Every SSH server proves who it is with a host key. turnout checks that key before anything leaves your machine - before a password, before a signature - so a deploy credential is never handed to whoever happens to answer on a stand's address.

## Trusted on first contact

The first connection to an address writes the key it presented into `~/.ssh/known_hosts`. Every connection after that must present the same key, or it is refused:

```text
error: refusing to connect to prod.example.com:22: its host key has changed
  it presents ED25519 SHA256:Xq3v0yQ0p7Jf1oVb5Hk2mR8sTnLwZ4eD6cA9uYgI1Ek
  /home/me/.ssh/known_hosts holds ED25519 SHA256:pL2kV9mW3xR7tN1qS5dF8gH4jZ6cB0aE2uY7iO3wQ4M for it (line 12)
Someone may be intercepting the connection - or the server was reinstalled and has a new key.
Once you know it is the server: `turnout server trust prod` shows the new key and asks to trust it.
```

This is what `ssh -o StrictHostKeyChecking=accept-new` does: a new host is taken on its word once, a known one is held to it. To check a key rather than take it on first use, run [`turnout server trust`](/turnout/reference/server/#trust) before the first connection and compare the fingerprint it shows with the one the server prints.

`turnout server show` names the key a server is trusted with, on its **Host key** line.

## One file with `ssh`

The file is OpenSSH's own - `%USERPROFILE%\.ssh\known_hosts` on Windows, where OpenSSH for Windows keeps it too - rather than one of turnout's:

- a server `ssh` already trusts is trusted by turnout from the first command;
- a key turnout learned is the key [`turnout ssh`](/turnout/reference/ssh/) and plain `ssh` expect;
- a reinstalled server is mended once, for both.

It is read the way `ssh` reads it: hashed host names (`HashKnownHosts`), `[host]:port` for a port other than 22, `*` and `?` patterns, negated patterns, and `@revoked` lines, which refuse the key they name. `@cert-authority` lines trust nothing here: turnout checks host keys, not host certificates. Lines it cannot read - a key type it does not know - are passed over, as `ssh` passes over them.

## Which key a server presents

A server usually has several host keys - ED25519, ECDSA, RSA - and presents the one the client asks for first. turnout asks first for the kinds the file already holds for that address, the way `ssh` does, so a server known by its RSA key is met by its RSA key rather than refused for presenting a new ED25519 one. Any key the file holds for the address settles it, whatever its kind: presenting a different kind is not a way around the check.

## In scripts and tests

`TURNOUT_KNOWN_HOSTS` names another file. turnout reads and writes that one instead, and `turnout ssh` hands it to `ssh` as `UserKnownHostsFile`, so the two still agree.
