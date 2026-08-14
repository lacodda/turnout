---
title: Entities
description: The turnout data model - apps, servers, credentials, paths and state.
sidebar:
  order: 1
---

turnout is built around separate entities instead of one config file that knows everything. Each has a single job, and they reference each other by name.

The rule behind the split: a thing that gets reused should be defined once. One deploy account reaches several stands; one web root exists on both the staging box and the production one. Copies of either would drift.

## App

A local product you work on.

- name (`myapp`), path to the project directory;
- commands: `dev`, `build`, `test`, `lint`, plus custom ones;
- what counts as the build artifact (e.g. a `dist` folder);
- the local gateway port the app talks to;
- which servers the app is allowed to use, and which one is currently selected for development.

## Server

A machine, and how to reach it.

- name (`staging`, `prod-eu`), base URL - what the gateway routes to;
- SSH host and port, when they differ from the URL's host;
- TLS policy (e.g. accept a self-signed certificate for this server only);
- the credential it logs in with, by name;
- which named path each app deploys into;
- a human-friendly label.

The base URL stays here rather than moving out: "which stand is this" and "which host is this" are the same answer for a stand, and the gateway needs it.

## Credential

A way to log in, on its own.

- name (`prod-deploy`), the remote user;
- how it authenticates: a password, or a private key file on this machine;
- the secret itself - never here.

Secrets live in the OS keyring - Windows Credential Manager, macOS Keychain or Linux Secret Service - under the credential's name. Config files only hold metadata; nothing sensitive is written to disk in plain form, and secrets are never printed to logs. See [`turnout credential`](/turnout/reference/credential/) and [`turnout pass`](/turnout/reference/pass/).

## Path

A named directory on a server, plus what to run after writing to it.

- name (`wwwroot`), the absolute remote directory;
- the post-write command, e.g. a service restart.

Not tied to a server on purpose: the same role - "the web root" - usually exists on every stand, and the restart command belongs to the role rather than the machine. See [`turnout path`](/turnout/reference/path/).

## State

The current working mode, kept apart from the catalogs:

- which app is bound to which server for development;
- whether the gateway is running;
- recent operations.

## How a deploy reads them

`turnout deploy myapp` resolves four things: the **app** (from the argument or the current directory), the **server** (from `--server` or the current binding), then the **credential** and **path** that server uses for this app. `--credential` and `--path` override the last two for a single run.

This split is what makes the daily workflow safe: editing a server description can never leak a password, and switching a stand never rewrites your project's `.env` files.
