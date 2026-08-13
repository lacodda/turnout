---
title: "completions"
description: Shell completion scripts for bash, zsh, fish, PowerShell and elvish.
sidebar:
  order: 10
---

```bash
turnout completions <bash|zsh|fish|powershell|elvish>
```

Prints a completion script for your shell. Wire it up once:

```bash
# bash (~/.bashrc)
eval "$(turnout completions bash)"

# zsh (~/.zshrc)
eval "$(turnout completions zsh)"

# fish
turnout completions fish > ~/.config/fish/completions/turnout.fish
```

```powershell
# PowerShell ($PROFILE)
turnout completions powershell | Out-String | Invoke-Expression
```

After that, `turnout <Tab>` completes commands, subcommands and flags.

## Live names in bash

The bash script goes one step further: where a command expects the name of an app, a server or a group, Tab completes to what is actually in your catalogs.

```bash
turnout use <Tab>            # apps and groups
turnout use web <Tab>        # servers
turnout app show <Tab>       # apps
turnout server edit <Tab>    # servers
turnout dev <Tab>            # apps
```

Flag values follow the same rule - `--server`, `--add-server`, `--app`, `--add-app` and their `--rm-` counterparts complete from the catalog.

Names come from a hidden `turnout complete` helper the script calls; it stays quiet on errors so a half-configured setup never garbles the line you are typing. The bash script registers itself for the [`tn` alias](/turnout/getting-started/#the-tn-alias) as well. Other shells get the static script - the same commands and flags, without live names.
