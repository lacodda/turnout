---
title: turnout completions
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
