---
title: turnout dev / build / test / lint / run
description: Run app commands from any directory.
sidebar:
  order: 8
  label: turnout dev / run
---

```bash
turnout dev   [APP]
turnout build [APP]
turnout test  [APP]
turnout lint  [APP]
turnout run COMMAND [APP]
```

Runs the app's named command in its project directory - no `cd` required. `dev`, `build`, `test` and `lint` are shortcuts for the standard commands; `run` executes any command defined in the app config (see [`turnout app`](/turnout/reference/app/)).

- **App resolution.** Pass the app name, or omit it and let turnout find the app whose directory contains your current one - `turnout dev` from anywhere inside the project just works.
- **Transparent output.** The command's stdout/stderr stream through untouched; turnout's own one-line status goes to stderr.
- **Exit codes pass through.** `turnout build` exits with the build's own code, so it drops into scripts and CI without surprises.

```bash
cd ~/dev/myapp/src/components
turnout dev                 # runs myapp's dev command in ~/dev/myapp

turnout build myapp         # from anywhere
turnout run deploy myapp    # custom command from the app config
```
