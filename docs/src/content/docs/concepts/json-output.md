---
title: JSON output
description: Every command that reports takes --json - the same fields as its text, as a contract for scripts, status bars and tools.
---

Every command that reports something takes `--json` and prints the same thing as JSON:

| Command | Prints |
| --- | --- |
| [`status --json`](/turnout/reference/status/#as-json) | The whole view: apps, groups, gateway and its signals, running jobs, catalogs, recent actions |
| [`doctor --json`](/turnout/reference/doctor/#as-json) | Every check: its state, what it is about, the fix; the counts; with `--server`, the deploy step a problem stops |
| [`ps --json`](/turnout/reference/ps/#as-json) | An array of jobs |
| `app`, `server`, `credential`, `path`, `target`, `group` - `list --json` / `show NAME --json` | An array of entities / one entity |
| [`pass list --json`](/turnout/reference/pass/#list) | The credentials, as `credential list --json` |
| [`cookie list --json`](/turnout/reference/cookie/#list) | The jars the gateway holds, by cookie name, with their expiry and the stand's last answer |
| [`restore --list --json`](/turnout/reference/backup/) | The backups of a target, newest first |
| [`backup prune --json`](/turnout/reference/backup/#backup-prune) | What a prune removed and how many are left |
| [`history --json`](/turnout/reference/history/) / `history --show N --json` | An array of deploys / one deploy: target, build (version, commit, `dirty`), outcome, the restart and the check, whether its build is still kept |
| [`history --diff A B --json`](/turnout/reference/history/#the-commits-between-two-deploys) | The two ends and the commits `added` and `removed` between them |
| [`deploy --diff --json`](/turnout/reference/deploy/#before-writing---diff) | What a deploy would change, per target: files to add, update and leave, and the bytes it adds |

```bash
turnout app list --json | jq -r '.[] | select(.server == "prod") | .name'
turnout ps --json | jq '[.[] | select(.state == "ready")] | length'
```

## One view, two renderings

The text and the JSON are made from one value. A command builds what it knows into a view and either prints that view as JSON or writes it as text - so a field cannot appear in one and be missing from the other, and what a script reads is exactly what you see. The same views will serve turnout's MCP server.

## The rules

- **Every key is always there.** Something absent is `null`, an empty list is `[]` - never a missing key. A script can index without checking first.
- **A list is an array of what `show` prints for one.** `app list --json` is `[...]` of the objects `app show NAME --json` prints; an empty catalog is `[]`, not a sentence.
- **Times are ISO 8601 in UTC**, `2026-10-04T09:12:44Z`; how long ago something happened is for the reader to work out. A duration turnout measured - how long a deploy took - is a number of `seconds`.
- **Names are the catalog's names**; enumerations are lowercase words with underscores (`session_expired`, `not_answering`).
- **Secrets never appear** - a credential says whether a secret is stored (`secret_stored`), the cookie list names cookies without their values, and the gateway's control token is not in any of it.

The JSON is not the catalog files. Those are storage: they change shape with the schema and are migrated when they do. The JSON is what turnout promises to whoever reads its output, and it is kept apart from storage so a migration does not break a script ([ADR 0020](https://github.com/lacodda/turnout/blob/main/docs/adr/0020-json-is-a-view-not-the-storage.md)). Fields may be added; removing or renaming one is a breaking change, named in the changelog with the way forward.

`--json` exists only on commands that report - `turnout deploy --json` without `--diff` is an error, not a flag quietly ignored.
