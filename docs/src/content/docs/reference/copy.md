---
title: "copy"
description: Top up a folder on a server from a folder here - the new and changed files go, nothing there is removed.
sidebar:
  order: 9.4
---

```bash
turnout copy FROM SERVER:DIR [-C CREDENTIAL] [--diff]
```

Sends the files of a folder here that a folder on the server does not have yet, or has otherwise, over the same SSH connection a deploy uses - and removes nothing there. It is for a folder something on the server takes files from: an inbox a service reads, a drop folder for imports, a shared directory that only ever grows.

```bash
turnout copy ~/receipts nas:/srv/books/inbox/me@example.com
```

```text
┌ Copying /home/me/receipts → nas:/srv/books/inbox/me@example.com
│
◇ Connected to deploy@nas.example.com:22
◇ 3 of 41 files to send: 2 new, 1 changed, 38 there already
│  new: 2026-11/receipt-0412.xml, 2026-11/receipt-0413.xml
│  changed: 2026-10/receipt-0398.xml
◇ Sent 3 files (14.20 KiB) in 0.1s · 142.00 KiB/s
◇ Put 3 files in place
│
└ Copied 3 files to nas:/srv/books/inbox/me@example.com
```

## Arguments

- **`FROM`** - a folder on this machine; its files go with their paths below it.
- **`SERVER:DIR`** - a [server](/turnout/reference/server/) or a [target](/turnout/reference/target/), a colon, and the folder there. An absolute `DIR` is taken as written. A relative one is under the target's deploy directory, or - on a server - where the login lands, the way `scp` reads it. `TARGET:` alone is the target's deploy directory. A Windows server's folder is written its own way: `win1:C:\drop`.

| Flag | Short | Description |
| --- | --- | --- |
| `--credential` | `-C` | Sign in with this credential instead of the target's or the server's |
| `--diff` | | Show what would be sent - and send nothing |

Folders that are not there yet - the server's folder itself, or one below it - are made.

## What goes

What the server has is asked of the server, the way [`deploy --diff`](/turnout/reference/deploy/#before-writing---diff) asks it: every file of the folder with its size in one pass, and a SHA-256 of each file that could be the same - same path, same size - on both sides. A file that is new or differs goes; one that is the same stays as it is.

**Nothing is removed**, and nothing the server holds is written over unless it is a file of `FROM` with other contents. A file that is gone from the server - a service took it and moved it on - is new to it again, and goes again on the next copy; a service that takes files has to know a file it already took, as one that reads an inbox does.

A server with neither `sha256sum` nor `shasum` cannot tell a changed file from an unchanged one of the same size, so those go again too, and the report says so: sending a file twice is harmless, leaving an edit behind is not.

## A file appears whole

Each file travels under a hidden name of its own beside where it goes - `.receipt-0412.xml.turnout-part` - and only when every one has arrived are they renamed into place, one rename each. Whatever watches the folder sees a file whole or not at all, never one still being written, and a copy that fails on the way takes the files it sent with it.

## Who may write it

A service that takes files from a folder often runs as an account of its own - a container's user, say - and a file it can read but not move stays in its inbox for ever. So on a Linux or macOS server, what the copy writes - the files, and the folders it makes - belongs to **the group of the folder it lands in**, and that group may write it (`g+w`). The group is asked of the folder before anything is made; when the folder is not there yet, of the nearest one above it that is.

Only what the copy wrote is touched: a folder that was there keeps the owner and mode it has. The deploying account has to be in that group to give a file to it; when it is not, the copy names what it could not give away - add the account to the group on the server, once:

```bash
sudo usermod -aG books deploy     # then sign in again
```

On a Windows server the files take the folder's ACL, as every file written there does.

## Examples

```bash
turnout copy ./receipts nas:/srv/books/inbox/me@example.com   # top up an inbox
turnout copy ./receipts nas:/srv/books/inbox/me@example.com --diff   # what would go
turnout copy ./seed myapp-stage:uploads      # into the deploy directory's uploads/
turnout copy ./drop win1:C:\drop -C admin    # a Windows server, another login
```
