---
title: "export"
description: Write apps, servers, credentials, paths and groups to a file, optionally including secrets under a passphrase.
---

Writes this machine's setup to a single JSON file.

```bash
turnout export
turnout export --output ~/turnout-backup.json
```

```text
Exported to turnout-export.json
  3 app(s), 2 server(s), 2 credential(s), 2 path(s), 1 group(s)
  Secrets are NOT included - re-run with --with-secrets to take them along.
```

| Flag | Short | Effect |
| --- | --- | --- |
| `--output PATH` | `-o` | Where to write; defaults to `turnout-export.json` here |
| `--with-secrets` | `-S` | Include stored secrets, encrypted with a passphrase |

Read it back with [`turnout import`](/turnout/reference/import/).

## What travels by default

Apps, servers, groups, and the **credentials** and **paths** they point at - who logs in and where files land, but never the secret values themselves.

The file is plain JSON, on purpose: it is configuration, and something you can read, diff and edit by hand is worth more than an opaque blob. It is written readable only by you, because even without secrets it lists hosts, logins and deploy directories.

Import it on the other machine and everything works except the parts that need a secret - those report it as missing until you run [`turnout pass set`](/turnout/reference/pass/) there.

## Taking secrets along

```bash
turnout export --with-secrets
```

You are asked for a passphrase (twice), and every stored secret is encrypted into the file as one sealed block. **There is no recovery**: lose the passphrase and the secrets in that file are gone. Nothing else about the export changes.

An export is exactly the kind of file that gets attached to a chat, synced to a cloud folder or forgotten in Downloads. So the sealed block gives up nothing on inspection - not the values, and not even how many secrets are stored:

```json
"secrets": {
  "kdf": { "algorithm": "argon2id", "salt": "...", "memory_kib": 65536, "iterations": 3, "parallelism": 4 },
  "nonce": "...",
  "ciphertext": "..."
}
```

The key comes from Argon2id, deliberately tuned above the library defaults - the passphrase is the weak link, and this file can be attacked offline for as long as someone likes. The payload is ChaCha20-Poly1305, so a file that has been altered in transit fails loudly on import instead of decrypting into something subtly wrong.

## In scripts

Both the prompt and the confirmation are skipped when there is no terminal; the passphrase is read from stdin instead, so it never lands in shell history or a process listing:

```bash
printf '%s' "$PASSPHRASE" | turnout export --with-secrets --output backup.json
```
