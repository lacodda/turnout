---
title: The stand in your prompt
description: Put the stand your app points at, the gateway and the running jobs into a shell prompt or a status bar with turnout status --short.
sidebar:
  order: 3
---

Which stand is this terminal's app talking to? Is the gateway up? Is the dev server still running? `turnout status --short` answers all three in one line, made for a prompt:

```text
prod · gw ok · 2 dev
```

In an app's directory the stand is that app's; elsewhere it is every stand an app is bound to. When the gateway has noticed something, the stand says it: `prod 502`, `prod down`, `prod login`. The rest of the format is on the [`status`](/turnout/reference/status/#the-prompt-line) page.

The command reads local files only - never the network, never an update check - and prints nothing at all when there is nothing to say, so the segment simply disappears.

## starship

In `~/.config/starship.toml`:

```toml
[custom.turnout]
command = "turnout status --short"
when = true
format = "([$output]($style) )"
style = "bold purple"
```

The parentheses make the group conditional: with nothing printed, not even the space after it is left. On Windows add `shell = ["cmd", "/C"]`, so starship does not start a PowerShell for every prompt just to run one command.

## tmux

In `~/.tmux.conf`:

```bash
set -g status-right '#(turnout status --short) '
set -g status-interval 5
```

## PowerShell

In your profile (`$PROFILE`), around your existing prompt:

```powershell
$TurnoutPrompt = $function:prompt
function prompt {
    $line = turnout status --short
    if ($line) { Write-Host "[$line] " -NoNewline -ForegroundColor DarkCyan }
    & $TurnoutPrompt
}
```

## bash and zsh

```bash
# ~/.bashrc
PS1='$(s=$(turnout status --short); [ -n "$s" ] && printf "[%s] " "$s")'"$PS1"
```

```zsh
# ~/.zshrc
setopt PROMPT_SUBST
PROMPT='$(s=$(turnout status --short); [[ -n $s ]] && print -n "[$s] ")'"$PROMPT"
```

## A status bar that wants more

A bar that draws its own pieces - waybar, polybar, a Rainmeter skin - reads `turnout status --json` instead and picks the fields it needs: `.apps[].server`, `.gateway.state`, `.gateway.signals`, `.jobs`. See [JSON output](/turnout/concepts/json-output/).
