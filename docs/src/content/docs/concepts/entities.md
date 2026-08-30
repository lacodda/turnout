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

## Target

A named deploy route: which app, to which server, logging in as which credential, landing in which path.

- name (`myapp-prod`, defaulting to `APP-SERVER`);
- the app, server, credential and path it joins, each by name.

Before v0.11.0 this join lived inside the server, as a map from app name to path name. That put it in the wrong place: the pair it describes is "this app, on this server", not "this server, for whichever apps"; it could not be listed, renamed or reused on its own; and a deploy that wanted anything other than the default had to be re-described with flags every time rather than named once. Pulling it out into its own entity makes it a thing you can list, `show`, rename and address directly - `turnout deploy myapp-prod` from anywhere, no project directory or binding required. See [`turnout target`](/turnout/reference/target/).

## State

The current working mode, kept apart from the catalogs:

- which app is bound to which server for development;
- whether the gateway is running;
- recent operations.

## How a deploy reads them

`turnout deploy` resolves a **target**: from the argument, if it names one directly; from the argument as an app name, using that app's target on its current binding; or, with no argument, from the current directory's app and its current binding, if that pair has a target. A target, once found, supplies the **server**, **credential** and **path**; `--server`, `--credential` and `--path` override any of the three for a single run.

If the app-and-binding pair resolves to no target at all, turnout asks which path to use and offers to save the answer as one - so the question is asked once, not on every deploy.

This split is what makes the daily workflow safe: editing a server description can never leak a password, and switching a stand never rewrites your project's `.env` files.
