//! Completion scripts.
//!
//! clap generates the static part - subcommands and flags. For bash we append a
//! wrapper that fills in live entity names (apps, servers, credentials, paths,
//! groups) by calling the hidden `turnout complete` helper, and falls back to
//! clap's own function everywhere else. Other shells get the static script
//! unchanged.

use anyhow::Result;
use clap::CommandFactory;
use clap_complete::Shell;

use crate::cli::Cli;

pub fn run(shell: Shell) -> Result<()> {
    let mut command = Cli::command();
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, "turnout", &mut script);
    print!("{}", String::from_utf8_lossy(&script));
    if shell == Shell::Bash {
        print!("{BASH_DYNAMIC}");
    }
    Ok(())
}

/// Wraps clap's `_turnout` so a positional name completes to what actually
/// exists in the catalogs. Keyed off the command words typed so far; anything
/// not listed here falls through to the generated completion untouched.
const BASH_DYNAMIC: &str = r#"
# --- turnout: dynamic entity names -------------------------------------------
_turnout_names() {
    turnout complete "$1" 2>/dev/null
}

_turnout_dynamic() {
    local cur prev words cword
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    # Flags keep clap's own completion; only bare positionals are ours.
    if [[ "${cur}" == -* ]]; then
        _turnout "$@"
        return
    fi

    local kind=""
    case "${COMP_WORDS[1]}" in
        use)
            # `use TARGET SERVER`: first word is an app or a group, second a server.
            if [[ ${COMP_CWORD} -eq 2 ]]; then kind="targets"; else kind="servers"; fi
            ;;
        dev|build|test|lint|deploy|deploy-setup|backup|restore)
            [[ ${COMP_CWORD} -eq 2 ]] && kind="apps"
            ;;
        run)
            # `run COMMAND [APP]`
            if [[ ${COMP_CWORD} -eq 2 ]]; then kind="commands"; elif [[ ${COMP_CWORD} -eq 3 ]]; then kind="apps"; fi
            ;;
        app)
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(show|edit|remove)$ ]] && kind="apps"
            ;;
        server)
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(show|edit|remove)$ ]] && kind="servers"
            ;;
        group)
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(show|edit|remove)$ ]] && kind="groups"
            ;;
        credential|cred)
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(show|edit|remove)$ ]] && kind="credentials"
            ;;
        path)
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(show|edit|remove)$ ]] && kind="paths"
            ;;
        pass)
            # `pass` works on credentials since the v0.9.0 split; `list` takes no name.
            [[ ${COMP_CWORD} -eq 3 && "${COMP_WORDS[2]}" =~ ^(set|copy|remove)$ ]] && kind="credentials"
            ;;
    esac

    # Values for the flags that name an entity.
    case "${prev}" in
        --server|--add-server|--rm-server) kind="servers" ;;
        --app|--add-app|--rm-app) kind="apps" ;;
        --credential) kind="credentials" ;;
        --path) kind="paths" ;;
    esac

    if [[ -n "${kind}" ]]; then
        COMPREPLY=( $(compgen -W "$(_turnout_names "${kind}")" -- "${cur}") )
        return 0
    fi
    _turnout "$@"
}

# Registered for the `tn` alias too - the word positions are the same either way.
if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _turnout_dynamic -o nosort -o bashdefault -o default turnout tn
else
    complete -F _turnout_dynamic -o bashdefault -o default turnout tn
fi
"#;
