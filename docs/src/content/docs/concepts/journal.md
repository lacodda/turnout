---
title: The action journal
description: What turnout did, one JSON line per action - and what it deliberately never records.
---

Every action that changes state appends a line to `journal.jsonl` in the [data directory](/turnout/getting-started/): switching a binding, adding or removing an app or server, deploying, backing up, restoring, starting or stopping the gateway.

```json
{"at":"2026-08-08T22:06:55Z","action":"use","app":"web","server":"staging"}
{"at":"2026-08-08T22:07:31Z","action":"deploy","app":"web","server":"staging","detail":"142 files"}
```

One object per line, so it is `tail`-able, `grep`-able and `jq`-able without a parser:

```bash
jq -r 'select(.action=="deploy") | "\(.at) \(.app) -> \(.server)"' journal.jsonl
```

The five most recent entries also show up at the end of [`turnout status`](/turnout/reference/status/).

## What it never contains

The journal records **that** something happened, never the values involved. There are no secrets, no logins, no command output and no file contents in it - `turnout pass` is not journaled at all. Entries carry a timestamp, an action, the entity names and a short note like a file count or a backup name.

That is a design rule, not a filter: nothing in the writing path ever receives a secret to begin with. It means the file is safe to read, attach to a bug report or hand to an assistant.

## Size

At 1 MiB the file rotates: the current journal becomes `journal.jsonl.1` and a fresh one starts. One previous generation is kept, so the journal cannot grow without bound.

Writing is best-effort - if the journal cannot be written, the command you actually ran still succeeds.
