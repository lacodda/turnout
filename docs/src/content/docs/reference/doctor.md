---
title: "doctor"
description: Find out why something does not work - tools, ports, dotenv files, the gateway, stands and sessions; or, for a server, what would stop a deploy.
sidebar:
  order: 2.5
---

```bash
turnout doctor [--fix [--yes]] [--json]
turnout doctor --server [NAME] [--json]
```

| Flag | Short | Description |
| --- | --- | --- |
| `--server` | `-s` | Check a server instead of this machine; picked interactively when the name is omitted |
| `--fix` | | Repair what turnout can repair, then check again |
| `--yes` | `-y` | With `--fix`: end a process on a dev port without asking |
| `--json` | | The same checks as JSON, for scripts - see [JSON output](/turnout/concepts/json-output/) |

"Why does it not work" usually has one of a handful of answers, and each one costs ten minutes when nothing names it: a dev server left running on the app's port, so the browser shows yesterday's code; a dotenv file that still names the old gateway port; a stand that is down, or a session that lapsed overnight; a tool that is not on this shell's PATH; a password that was never stored. `doctor` looks at all of them at once and says, for each thing that is wrong, what to run.

```text
┌  Local environment
│
◇  turnout 0.25.0 · data C:\Users\me\AppData\Local\lacodda\turnout
◇  node v22.11.0 · pnpm 10.34.5
◇  keyring answers (Windows Credential Manager)
│
◇  web · project C:\work\web
▲  web · dev port 5100 is held by node.exe (pid 4242), which turnout did not start - `turnout dev web` will not run over it · fix: turnout doctor --fix
◇  web · the gateway listens on 7100
✗  web · .env.development.local says VITE_API_URL=http://localhost:7001; the gateway is at http://localhost:7100 - the app started outside turnout talks to the wrong port · fix: turnout doctor --fix
│
◇  gateway running · pid 24180 · front door http://localhost
│
◇  prod · answers · https://prod.example.com/health 200 in 84 ms
✗  web · session on prod expired - GET /api/me now answers 401 · fix: log in at http://web.localhost, or turnout cookie set web
│
◇  catalog: 2 servers, 1 credential, 1 target - every name resolves
│
└  2 problems · 1 warning · `turnout doctor --fix` repairs 2
```

`◇` is fine, `●` is worth knowing, `▲` needs a look, `✗` is broken - something will fail because of it. Every `▲` and `✗` carries its fix. The exit code is `1` when anything is `✗`, so a script or a CI step can gate on it; warnings do not change it.

## What it checks

**This machine**

- **turnout** - the version and the data directory; whether it is set up and the catalogs read.
- **Tools** - the programs your apps' commands start (`node`, `pnpm`, `yarn`, `npm`, `bun`, `deno`, `cargo`), each asked for its version through the same shell the commands run in. One that does not answer names the apps that need it.
- **The keyring** - whether the OS keyring answers. The check reads an entry that is not there; nothing is written.

**Each app**

- **The project** - the directory is there; a JavaScript project has its dependencies installed (here, or hoisted to a workspace root); its commands use the package manager its lockfile belongs to; the Node it pins in `.nvmrc` or `.node-version` is the one on PATH.
- **The dev port** - free, or held by the app's own running `dev`. Held by anything else - a dev server a closed terminal left behind, or one started by hand - it is named with its program and pid. A running `dev` whose server had to move to another port is a problem: the [front door](/turnout/reference/gateway/#the-front-door) still routes to the old one.
- **The gateway port** - free while the gateway is stopped, listened on while it runs. A gateway started before the app got its port does not listen there.
- **The dotenv file** - [`.env.development.local`](/turnout/guides/vite-dev-workflow/) says what the port says; no line further down sets the variable again (that line would win); the file is ignored by git and not committed. Git itself is asked, so `.git/info/exclude` and your global excludes count. A variable of the same name set in the shell, which would win over the file, is named too.

**The gateway** - running and answering, its front door open (when it is closed, the processes holding ports 80 and 7000 are named), its control channel answering.

**Stands and sessions**

- **Every stand** in the catalog is probed at once, as [`use`](/turnout/reference/use/) does. One an app is bound to that does not answer is a problem; one nobody uses right now is a warning. A stand whose certificate this machine does not trust is told apart from one that is down.
- **Every session** of a bound app is checked *now*: see below.

**The catalog** - every binding, server and target names things that exist; password credentials have their password stored; key files are where the credentials say; the SSH agent answers, when a credential signs in through one.

## Is the session alive?

A stand's session lapses on the stand's side - after an idle hour, overnight - and nothing local knows until the app runs into it. So `doctor` asks: the gateway sends again the last request that proved the session, with the cookies it holds now, and reports what the stand answered.

The request that proves a session is the last one that was a `GET`, carried the jar's cookies, was answered 2xx without the stand deleting any of them, and came back within two seconds. A `GET` is the only request safe to repeat unasked; a logout link deletes the cookie and so never qualifies; a long poll is too slow to. It is sent with the headers the app sent, so an app that also authenticates with a header (a bearer token) is checked the way it actually works.

| What the stand answers | The line |
| --- | --- |
| 2xx | `◇` alive - and when the first cookie expires, if any does |
| 401, 403, 440, or a redirect (the way to a login page) | `✗` expired - log in through the gateway, or [`turnout cookie set`](/turnout/reference/cookie/) |
| 5xx, or nothing | `▲` the stand is down, so the session cannot be told |
| no cookies held | `●` nobody has logged in through the gateway |
| cookies held, no request has used them yet | `●` nothing to check - after `cookie set`, say |

What the stand answers also goes where every answer goes: an expired session raises the gateway's [signal](/turnout/concepts/gateway/#when-a-stand-misbehaves), in `status`, the prompt line and one notification.

## --fix

`--fix` repairs what is turnout's own to repair, then runs every check again and shows what is left:

- **the dotenv block** - written as the port says, or taken out when the app has no port;
- **`.gitignore`** - the dotenv file added to the nearest one in the repository;
- **a gateway record** whose process is gone;
- **a binding** to a server that is not in the catalog;
- **a process on an app's dev port** - only after a yes. turnout cannot tell a dev server left behind from one somebody is using right now, so it asks, naming the program and the pid. Off a terminal it leaves the process alone unless `--yes` is given.

```text
✓  web: ended node.exe (pid 4242), which held dev port 5100
✓  web: wrote VITE_API_URL=http://localhost:7100 to .env.development.local

┌  Local environment
...
```

Everything else - a stand that is down, a password to store, a file that is committed to git - names the command for you to run.

## `dev` refuses to start over a stray

The moment the stray process hurts is when `dev` starts: the new server moves on to another port, the front door keeps showing the old one, and the browser shows yesterday's code with no error anywhere. So [`turnout dev`](/turnout/reference/run/) checks the app's dev port first. In a terminal it offers to end what holds it; elsewhere it stops and names it:

```text
error: dev port 5100 of 'web' is held by node.exe (pid 4242), which turnout did not start - a dev server left behind, or one started by hand. Over it, the front door would keep showing that one. End it (`turnout doctor --fix`), or pin another port with `turnout app edit web --dev-port PORT`
```

## --server

```bash
turnout doctor --server prod
```

```text
┌  Server prod
│
◇  prod · sshd answers on prod.example.com:22 in 40 ms
◇  deploy · deploy@ signs in with key ~/.ssh/id_ed25519 · shell: sh
◇  deploy · tar · tar (GNU tar) 1.35
│
✗  wwwroot · /var/www/app is not writable for deploy, owned by root · fix: sudo chown -R deploy '/var/www/app'
◇  wwwroot · 34.2 GiB free (the last build is 6.1 MiB)
✗  wwwroot · restart `sudo systemctl restart app` - sudo asks deploy for a password, and a deploy has nobody to type it · fix: allow the command without a password for deploy: a NOPASSWD rule in sudoers
│
◇  prod · the web server answers · https://prod.example.com/health 200 in 84 ms
│
└  2 problems · a deploy would stop at the upload step
```

Everything a deploy to that server needs, asked as the account each [target](/turnout/reference/target/) signs in with - or the server's own credential when no target goes there:

- **sshd** answers on the SSH host and port;
- **the sign-in** works, and which shell answers (`sh`, or `cmd.exe` on a [Windows server](/turnout/concepts/windows-servers/));
- **tar** is there - without it a deploy sends the files one by one;
- **each deploy directory** is writable for that account (or, when it does not exist yet, the parent a deploy would create it in), with the command that grants the right - `chown` on Linux, `icacls` on Windows;
- **free space** there, against twice the size of the app's last build - the archive and the files unpacked from it sit side by side for a moment;
- **the restart command** finds its programs as the deploy runs it - unattended, as that account, in the deploy directory - each command of a chain like `cd /srv/app && docker compose up -d` on its own, shell built-ins left out; and when it goes through `sudo`, sudo does not ask for a password;
- **the web server** answers at the stand's address;
- each target's app names a build directory.

The closing line names the step the first problem would stop a deploy at: `start`, `connect`, `upload` or `restart`.

Nothing on the server changes. The one write is a probe file, created and deleted in the deploy directory: permissions, ACLs and read-only mounts all have a say, and writing is the only way to know. A directory that is not there yet is not created. On Windows the checks run in PowerShell, handed over encoded, since `cmd.exe` has no quoting that would carry them intact.

## As JSON

```json
{
  "version": "0.25.0",
  "server": null,
  "checks": [
    {
      "section": "apps",
      "check": "dev_port",
      "state": "warn",
      "subject": "web",
      "message": "dev port 5100 is held by node.exe (pid 4242), which turnout did not start - `turnout dev web` will not run over it",
      "fix": "turnout doctor --fix",
      "fixable": true
    }
  ],
  "problems": 0,
  "warnings": 1,
  "deploy_stops_at": null,
  "fixed": []
}
```

`state` is `ok`, `info`, `warn` or `fail`; `check` is a stable word for each kind of check (`tools`, `keyring`, `dev_port`, `gateway_port`, `env_file`, `stand`, `session`, `credential`, `sshd`, `sign_in`, `writable`, `disk`, `restart`, `web`, ...); `subject` is the app, server, credential, path or target the line is about. With `--server`, `server` is its name and `deploy_stops_at` the step.

```bash
turnout doctor --json | jq -r '.checks[] | select(.state == "fail") | "\(.subject): \(.message)"'
```
