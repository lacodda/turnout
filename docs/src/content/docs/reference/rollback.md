---
title: "rollback"
description: Put a stand back on the build of an earlier deploy - the one before what it runs, or any one you name.
sidebar:
  order: 9.3
---

```bash
turnout rollback [TARGET]
turnout rollback [TARGET] --to DEPLOY
```

Sends the build an earlier deploy sent - the very archive, kept on this machine - to the same stand again, through the same checklist a deploy runs: a backup when the target has a [health check](/turnout/reference/target/#health-check), the upload, the restart, the check, and a rollback of the rollback if that fails.

| Flag | Description |
| --- | --- |
| `--to DEPLOY` | The deploy whose build to send, as [`turnout history`](/turnout/reference/history/) numbers it (`12` or `#12`) |

## Back one step

```bash
turnout rollback myapp-prod
```

```
Sending the build of #12 · 1.4.0 · 1a2b3c4 · deployed 3d 04h ago again
┌ Rolling back myapp-prod to #12: myapp → prod
◇ Connected to deploy@prod.example.com:22
◇ Backup 20261016-141000.tar.gz kept for a rollback until the check passes
◇ Packed 214 files → 6.10 MiB
◇ Uploaded 6.10 MiB in 3.1s · 1.97 MiB/s
◇ Unpacked 214 files into /var/www/myapp
◇ Removed 2 files the newer builds added
◇ Restarted: docker compose up -d
◇ Healthy: GET https://prod.example.com/health - 200 in 38 ms
└ Deploy #15 of 'myapp' to 'prod' finished
```

`TARGET` resolves like [`deploy`](/turnout/reference/deploy/)'s: a target, an app on its bound server, or the app of the current directory. turnout goes to the deploy **before the one the stand runs now**:

- The stand runs the newest deploy there that left files - a `deployed` one, or an `uploaded, restart failed` one, whose files are what is on the server. Deploys that failed or were rolled back changed nothing and are passed over.
- A rollback stands for the deploy it sent again, so `rollback` twice goes two steps back, not back and forth.

## To a deploy you name

```bash
turnout rollback --to 12
```

The record says where #12 went, so no target is needed; name one and turnout checks it is the same stand. Only a deploy that went through can be sent again.

## What it puts back, and what it leaves

A rollback is about the **build**: it sends #12's files and removes the files that **later deploys to this stand** added and #12's build does not have - a chunk a newer version introduced, say. Files no deploy sent - uploads, a `.env` written on the server, a database's files - are left exactly as they are.

That is the difference from [`turnout restore`](/turnout/reference/backup/#restore), which puts the whole directory back as a backup caught it - data included - and works only where a backup was taken. A rollback needs no backup on the server at all: the build comes from this machine.

- **Builds are kept for the last 10 deploys to each stand.** Older ones say `no longer kept` in [`history --show`](/turnout/reference/history/#one-deploy), and a rollback to them is refused. A build sent to two stands is kept once.
- **When a later deploy's build is gone**, the files only it added cannot be known; the rollback says which deploys those are, and leaves their files.
- **The route is the catalog's today.** If the path now points at another directory, the rollback is refused rather than sending the build somewhere it never ran.
- **The history is this machine's**, so the build of a deploy made elsewhere is not here to send.

The rollback is a deploy of its own: it is recorded with its number, shows as `deployed (rollback to #12)` in the history, and fails - and is rolled back - the same way any deploy does.
