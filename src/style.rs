//! How text output looks on a terminal: a few colours, and links.
//!
//! Every helper here returns plain text when stdout is not a terminal, or
//! when `NO_COLOR` asks for none (the `console` crate decides that, the same
//! decision cliclack's spinners make), so piped output and logs stay clean.
//!
//! Links are OSC 8 hyperlinks: the text stays what it is, a click opens the
//! target. They are written only where the terminal is known to understand
//! them - a terminal that does not would print the escape sequence instead.

use std::io::IsTerminal;
use std::sync::OnceLock;

use console::Style;

/// Forces hyperlinks on (`1`) or off (`0`) whatever the terminal is - the
/// variable other command-line tools read for the same question.
pub const FORCE_HYPERLINK_ENV: &str = "FORCE_HYPERLINK";

fn paint(style: Style, text: &str) -> String {
    style.apply_to(text).to_string()
}

/// A section heading: `Gateway`, `Jobs`.
pub fn heading(text: &str) -> String {
    paint(Style::new().bold(), text)
}

/// The name of something in a catalog.
pub fn name(text: &str) -> String {
    paint(Style::new().cyan(), text)
}

/// Something working as it should.
pub fn good(text: &str) -> String {
    paint(Style::new().green(), text)
}

/// Something that needs a look but does not stop the work.
pub fn warn(text: &str) -> String {
    paint(Style::new().yellow(), text)
}

/// Something broken.
pub fn bad(text: &str) -> String {
    paint(Style::new().red(), text)
}

/// The secondary part of a line: times, hints, counts.
pub fn dim(text: &str) -> String {
    paint(Style::new().dim(), text)
}

/// `text` as a link to `target` where the terminal can click one.
pub fn link(text: &str, target: &str) -> String {
    if !hyperlinks() || target.is_empty() {
        return text.to_string();
    }
    // OSC 8 ; params ; URI ST, the text, and the same with an empty URI to end
    // the link. ST is ESC \, which every terminal that knows OSC 8 accepts.
    format!("\x1b]8;;{target}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// A URL that is its own text, clickable where the terminal allows.
pub fn url(url: &str) -> String {
    link(url, url)
}

/// Pad `text` to `width` columns, judging the width by `plain` - the text
/// without the escape sequences styling put around it.
pub fn pad(text: &str, plain: &str, width: usize) -> String {
    let fill = width.saturating_sub(plain.chars().count());
    format!("{text}{}", " ".repeat(fill))
}

fn hyperlinks() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        hyperlinks_for(std::io::stdout().is_terminal(), &env)
    })
}

/// Whether to write links, given whether stdout is a terminal and the
/// environment.
///
/// No terminal reports whether it understands OSC 8, so the answer comes from
/// what the terminal says about itself in the environment - the same table
/// the `supports-hyperlinks` crate keeps, written out here rather than pulled
/// in for one function. tmux is left out on purpose: it drops the sequence
/// unless told to pass it through.
fn hyperlinks_for(terminal: bool, env: &dyn Fn(&str) -> Option<String>) -> bool {
    if let Some(forced) = env(FORCE_HYPERLINK_ENV) {
        return forced != "0";
    }
    if !terminal {
        return false;
    }
    if env("DOMTERM").is_some() || env("WT_SESSION").is_some() || env("KONSOLE_VERSION").is_some() {
        return true;
    }
    if env("TERM_PROGRAM").is_some_and(|program| ["Hyper", "iTerm.app", "terminology", "WezTerm", "vscode", "ghostty"].contains(&program.as_str())) {
        return true;
    }
    if env("VTE_VERSION")
        .and_then(|version| version.parse::<u32>().ok())
        .is_some_and(|version| version >= 5000)
    {
        return true;
    }
    env("TERM").is_some_and(|term| ["xterm-kitty", "alacritty", "foot", "xterm-ghostty"].contains(&term.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| value.to_string())
    }

    /// Links go where the terminal says it reads them, and nowhere else: a
    /// pipe never gets one, a terminal that keeps quiet about itself neither.
    #[test]
    fn links_are_written_only_for_terminals_that_read_them() {
        assert!(hyperlinks_for(true, &env_of(&[("WT_SESSION", "1")])), "Windows Terminal");
        assert!(hyperlinks_for(true, &env_of(&[("TERM_PROGRAM", "vscode")])));
        assert!(hyperlinks_for(true, &env_of(&[("VTE_VERSION", "7600")])));
        assert!(
            !hyperlinks_for(true, &env_of(&[("VTE_VERSION", "4600")])),
            "VTE before 0.50 prints the sequence"
        );
        assert!(!hyperlinks_for(true, &env_of(&[])), "an unknown terminal");
        assert!(!hyperlinks_for(true, &env_of(&[("TERM_PROGRAM", "tmux")])));
        assert!(!hyperlinks_for(false, &env_of(&[("WT_SESSION", "1")])), "a pipe inside Windows Terminal");
    }

    /// The override wins both ways, pipe or not.
    #[test]
    fn the_override_decides_when_it_is_set() {
        assert!(hyperlinks_for(false, &env_of(&[("FORCE_HYPERLINK", "1")])));
        assert!(!hyperlinks_for(true, &env_of(&[("FORCE_HYPERLINK", "0"), ("WT_SESSION", "1")])));
    }

    #[test]
    fn padding_counts_the_plain_text() {
        assert_eq!(pad("\x1b[36mweb\x1b[0m", "web", 5), "\x1b[36mweb\x1b[0m  ");
        assert_eq!(pad("abcdef", "abcdef", 3), "abcdef", "never cut");
    }
}
