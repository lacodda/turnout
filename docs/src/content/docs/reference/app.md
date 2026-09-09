---
title: "app"
description: Manage apps - the local projects turnout works with.
sidebar:
  order: 3
---

```bash
turnout app <add|list|show|edit|remove> [...]
```

An **app** is a local project: its path, its commands (`dev`, `build`, ...), its gateway port and the servers it is allowed to use. See [Entities](/turnout/concepts/entities/).

## add

```bash
turnout app add [NAME] [--path DIR] [--port PORT] [--env-var NAME] [--env-file FILE] [--dev-port PORT] [--dist DIR]
                [--command NAME=CMD]... [--server SERVER]...
```

| Flag | Short | Description |
| --- | --- | --- |
| `--path` | `-p` | Project directory |
| `--port` | `-P` | Local gateway port for this app |
| `--env-var` | `-e` | Variable that carries the gateway URL to the app's commands (default `TURNOUT_GATEWAY_URL`) |
| `--env-file` | | Dotenv file turnout keeps in step with the port, relative to the project (default `.env.development.local`) |
| `--dev-port` | | Port the dev server listens on; assigned from `5100-5199` on the first `dev` when unset, `0` hands it back |
| `--dist` | `-d` | Build artifact directory, relative to the project path |
| `--command` | `-c` | Set a command as `NAME=CMD` (repeatable); overrides detected defaults |
| `--server` | `-s` | Allow a server for this app (repeatable) |

With `NAME` or `--path` missing, the [wizard](#the-wizard) walks every field of the app, so nothing here needs a flag.

A gateway port belongs to exactly one app: `add` and `edit` refuse a port another app already holds and name that app. Two apps on one port would leave the gateway unable to bind the second listener.

## The wizard

`turnout app add` with nothing to go on, and `turnout app edit NAME` with no flags, open the same form. It asks for **every field an app has**, in order: the project directory, the commands, the gateway port, the variable that carries the gateway URL, the dotenv file, the dev server port, the build artifact directory and the allowed servers. `add` starts from an empty app and offers a free port; `edit` starts from the stored one, with its current values as the defaults - pressing enter through the form changes nothing.

Empty means *not set*: clearing the gateway port unsets it, and an empty dev port hands the port back so the next `dev` assigns a fresh one. Questions that would name nothing are skipped - without a gateway port there is no address to carry, so the variable and the dotenv file are not asked for.

### Commands

The commands question shows what the app has and works on the list:

```
Commands of 'myshop':
  build     pnpm build
  dev       pnpm dev --port {port}
  storybook pnpm storybook
? Commands >
  Done
  Add a command
  Change a command line
  Rename a command
  Remove a command
```

**Add** offers turnout's own roles first - `dev`, `build`, `test`, `lint`, the ones run by name - and takes any other name for a command reachable through [`turnout run`](/turnout/reference/run/). **Change** edits a command line with the current one already filled in.

Detection proposes; it does not rule. On `add`, and on an `edit` that moves the app to another directory, turnout reads the project and fills in the roles it recognises - without ever overwriting a command set by hand. What it proposes is simply there in the list, to be corrected, renamed or removed.

Flags do the same non-interactively, for scripts: `--command NAME=CMD` sets one and `--command NAME=` removes it.

## How the app learns the gateway address

The port is set once, here. The app is handed the *address* (`http://localhost:PORT`) by two roads, so the project's own `.env` never carries it:

- **A variable on every command turnout runs.** `turnout dev`, and any custom command run through `turnout run`, get `VARIABLE=http://localhost:PORT` in their environment. `build`, `test` and `lint` do not: a variable in the process environment overrides every dotenv file whatever the mode, and a production bundle must not bake in localhost. The variable's name is per app (`--env-var`), because the framework decides what the app can see - Vite exposes only `VITE_*` to the client, Create React App only `REACT_APP_*`. The wizard suggests `VITE_API_URL` for a Vite project and `TURNOUT_GATEWAY_URL` otherwise.
- **A dotenv file turnout keeps in step.** For the times the app is started past turnout - from an IDE, or with `pnpm dev` by hand - `app add`, `app edit` and `gateway start` write the same assignment into `.env.development.local` (or the file named with `--env-file`). Vite and CRA read `.env.*.local` on top of `.env` in development only, so the project's `.env` stays untouched and committable. Only one block of the file is turnout's: a header line and the assignment under it; everything else in the file survives every rewrite, and unsetting the port takes the block out again. The file name is appended to `.gitignore` when the project is a git repository.

Commands can also name the address inline: `{gateway}` in a command line is replaced with the URL and `{gateway_port}` with the number before the command runs - `vite --api {gateway}` works on every platform without `$VAR` or `%VAR%` syntax. A command that uses a placeholder on an app without a port is refused with the way to set one.

## The app's address

Every app answers at `http://NAME.localhost` through the gateway's [front door](/turnout/reference/gateway/#the-front-door), whatever port its dev server took. The port behind the name is the app's **dev port**: assigned from `5100-5199` on the first `turnout dev` and kept from then on, or pinned with `--dev-port` (a project that insists on 3000, say). `--dev-port 0` hands an assigned port back; the next `dev` picks a fresh one. Like a gateway port, a dev port belongs to one app.

`dev` hands the port to the server as the `PORT` variable and as `{port}` in the command line. Vite does not read `PORT`, so a Vite project's detected dev command already ends in `--port {port}` (`pnpm dev --port {port}`, `npm run dev -- --port {port}`); a project registered before that gets the same with `turnout app edit myapp --command "dev=pnpm dev --port {port}"`.

`app list` shows the address next to apps that have a dev port; `app show` prints it with the port; `turnout open myapp` opens it.

```bash
turnout app add myshop --path ~/dev/myshop --port 7001 --env-var VITE_API_URL
# Wrote ~/dev/myshop/.env.development.local (VITE_API_URL=http://localhost:7001).
```

With both given, `add` is fully non-interactive (useful for scripts): commands come from detection, adjustable via `--command`.

### Where the commands come from

When the project has a `package.json`, turnout reads its actual `scripts` instead of assuming names. Each of turnout's roles takes the first script that fills it:

| Role | Script names tried, in order |
| --- | --- |
| `dev` | `dev`, `serve`, `start`, `dev:server`, `watch` |
| `build` | `build`, `build:prod`, `compile`, `dist` |
| `test` | `test`, `test:unit`, `spec` |
| `lint` | `lint`, `lint:js`, `eslint` |

So a Vue CLI project whose dev script is `serve` gets `dev -> pnpm serve`, and `turnout dev` just works. Scripts that fill no role are kept under their own name, reachable through [`turnout run`](/turnout/reference/run/):

```bash
turnout run storybook myapp
```

Projects without a `package.json` (or without `scripts`) fall back to the conventional command set for the detected manager.

```bash
turnout app add                        # wizard, from the current directory
turnout app add myapp --path ~/dev/myapp --port 7100
turnout app add api --path ~/dev/api --command "dev=make run" --server staging
turnout app add api -p ~/dev/api -c "dev=make run" -s staging   # same, short form
```

## list / show

```bash
turnout app list          # one line per app: name, path, gateway port, address
turnout app show myapp    # full card: commands, dist, allowed servers
```

`show` warns if the project directory no longer exists on disk. Omit the name on a terminal and turnout offers a [picker](/turnout/concepts/pickers/); `edit` and `remove` do the same.

## edit

```bash
turnout app edit myapp                              # the wizard, over every field
turnout app edit myapp --port 7200                  # change one field; the dotenv file follows
turnout app edit myapp --env-var REACT_APP_API_URL  # the name the framework can see
turnout app edit myapp --command "deploy=make ship" # add or override a command
turnout app edit myapp --command deploy=            # remove a command
turnout app edit myapp --add-server prod --rm-server staging
```

| Flag | Short | Description |
| --- | --- | --- |
| `--path` | `-p` | Project directory |
| `--port` | `-P` | Local gateway port for this app |
| `--env-var` | `-e` | Variable that carries the gateway URL to the app's commands (default `TURNOUT_GATEWAY_URL`) |
| `--env-file` | | Dotenv file turnout keeps in step with the port, relative to the project (default `.env.development.local`) |
| `--dev-port` | | Port the dev server listens on; assigned from `5100-5199` on the first `dev` when unset, `0` hands it back |
| `--dist` | `-d` | Build artifact directory, relative to the project path |
| `--command` | `-c` | Set a command as `NAME=CMD`, or `NAME=` to remove it (repeatable) |
| `--add-server` | `-a` | Allow a server (repeatable) |
| `--rm-server` | `-r` | Disallow a server (repeatable) |

## remove

```bash
turnout app remove myapp [--yes]
```

Removes the app from the catalog only - the project on disk is never touched.
