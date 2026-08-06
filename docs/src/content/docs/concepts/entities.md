---
title: Entities
description: The turnout data model - apps, servers, credentials and state.
sidebar:
  order: 1
---

turnout is built around four separate entities instead of one config file that knows everything. Each entity has a single job, and they reference each other by name.

## App

A local product you work on.

- name (`myapp`), path to the project directory;
- commands: `dev`, `build`, `test`, `lint`, plus custom ones;
- what counts as the build artifact (e.g. a `dist` folder);
- the local gateway port the app talks to;
- which servers the app is allowed to use, and which one is currently selected for development;
- deploy metadata - where this app lands on each server.

## Server

A stand, machine or environment.

- name (`staging`, `prod-eu`), address / API URL;
- access types: HTTP(S), SSH, or both;
- TLS policy (e.g. accept a self-signed certificate for this server only);
- per-app deploy paths;
- a human-friendly label;
- a reference to a credential - never the secret itself.

## Credential

Access data, stored separately from server descriptions.

- login;
- secrets: passwords, SSH keys, API tokens;
- bound to a server (or a server+purpose pair).

Secrets live in the OS keyring - Windows Credential Manager, macOS Keychain or Linux Secret Service. Config files only hold metadata; nothing sensitive is written to disk in plain form, and secrets are never printed to logs.

## State

The current working mode, kept apart from the catalogs:

- which app is bound to which server for development;
- whether the gateway is running;
- recent operations.

This split is what makes the daily workflow safe: editing a server description can never leak a password, and switching a stand never rewrites your project's `.env` files.
