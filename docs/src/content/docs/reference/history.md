---
title: "history"
description: The deploys made from this machine - when, where, which build, and how each ended.
sidebar:
  order: 9.2
---

```bash
turnout history [TARGET | APP] [-n N] [--json]
turnout history --show DEPLOY [--json]
turnout history --diff FROM TO [--json]
```

Every [`turnout deploy`](/turnout/reference/deploy/) that reaches the server is recorded under a number - the one the deploy's last line names (`Deploy #14 of 'myapp' to 'prod' finished`). `history` reads those records back.

| Flag | Short | Description |
| --- | --- | --- |
| `--limit` | `-n` | How many of the newest deploys to list (default 20) |
| `--show` | | One deploy in full, by its number |
| `--diff` | | The commits between two deploys of one app |
| `--json` | | Print JSON instead - see [JSON output](/turnout/concepts/json-output/) |

## The table

```bash
turnout history myapp-prod
```

```
#   WHEN        TARGET      VERSION  COMMIT         FILES  SIZE      TOOK    RESULT
12  3d 04h ago  myapp-prod  1.4.0    1a2b3c4        214    6.10 MiB  14s     deployed
13  2d 01h ago  myapp-prod  1.4.1    9f8e7d6        214    6.12 MiB  8.4s    check failed, rolled back
14  5h 12m ago  myapp-prod  1.4.1    5c6d7e8-dirty  216    6.15 MiB  1m 02s  uploaded, restart failed
15  5h 01m ago  myapp-prod  1.4.0    1a2b3c4        214    6.10 MiB  13s     deployed (rollback to #12)
```

Oldest first, so the newest deploy is the line above your prompt. A name narrows the list to that target, or to every target of that app.

- **VERSION** is the `version` of the project's `package.json` - or of the `[package]` in its `Cargo.toml` - and **COMMIT** is git's `HEAD`, both read just before the build. `-dirty` means the working tree held changes the commit does not: that build cannot be made again from the commit alone.
- **TOOK** is the server's part: from connecting to the end of the check. The build, which several targets share, is not in it.
- **RESULT** says how it ended, and for a deploy that did not go through, which part gave:

| Result | What the stand runs now |
| --- | --- |
| `deployed` | This build |
| `check failed, rolled back` / `restart failed, rolled back` | What it ran before - the deploy [put it back](/turnout/reference/deploy/#health-check-and-rollback) |
| `uploaded, restart failed` | The **old** service on the **new** files: they went out, and the post-write command did not finish |
| `uploaded, check failed` | This build, failing its check, on a first deploy with nothing to roll back to |
| `failed` | What it ran before - nothing was written |

The `uploaded, …` lines are the ones that look fine from outside and are not, and the reason the record keeps what the restart printed.

## One deploy

```bash
turnout history --show 14
```

```
Deploy #14 · myapp-prod: myapp → prod
  When:      2026-10-16T09:12:40Z (5h 12m ago)
  Lands in:  wwwroot (/var/www/myapp)
  Build:     1.4.1 · 5c6d7e8-dirty · 216 files, 6.15 MiB · kept - `turnout rollback --to 14` sends it again
  Took:      1m 02s
  Result:    uploaded, restart failed
  Error:     `docker compose up -d --build` exited with 1: no space left on device
  Restart:   docker compose up -d --build (failed)
    #12 [web 4/6] RUN pnpm install --frozen-lockfile
    ...
    no space left on device
```

The restart's last 100 lines are kept - the same lines the deploy showed, there when you come back to find out why the stand did not change. `#14` works as well as `14`.

## The commits between two deploys

```bash
turnout history --diff 12 14
```

```
#12 (1a2b3c4) → #14 (5c6d7e8-dirty): 3 commits
  5c6d7e8 Cache the report query
  4b5c6d7 Show the export date
  3a4b5c6 Bump the chart library
#14 was built with changes that were never committed, on top of 5c6d7e8.
```

Asked of the app's own repository (`git log FROM..TO`), so both commits have to be in it - one that was rebased away is not, and the error says so. The order is yours: `--diff 14 12` lists what going from #14 back to #12 takes away, which is what a rollback does.

## Where the records live

In `deploys/` under the [data directory](/turnout/concepts/settings-migration/), one file per deploy, beside the builds kept for [`rollback`](/turnout/reference/rollback/). The records are kept for good - they are small. The builds are kept for the last 10 deploys to each stand, so `rollback --to` reaches that far back; the table's `--json` says per deploy whether its build is still there (`build_kept`).

The history is this machine's: a deploy made from another computer, or by hand, is not in it. Unlike the [journal](/turnout/concepts/journal/), a record keeps command output - the restart's - the way a job's log does, on this machine only.
