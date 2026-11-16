//! The variables an app runs with on a stand: `turnout env`.
//!
//! An app's `.env` is where its secrets end up - a database password, an API
//! token - in plain text, one copy per stand, renamed back and forth by hand.
//! Here they are kept per app and server: the values in the OS keyring, where
//! a credential's secret already lives ([`crate::secrets`]), and only their
//! names in a catalog (`env.json`). The catalog is what lets `env list` answer
//! without asking the keyring for anything, and what an export carries when
//! it is not asked for secrets.
//!
//! One set serves every road a variable takes: `dev` and `run` put it into
//! the command's environment, a deploy's build gets the set of the stand it
//! goes to, and a target that names an env file has that file written on the
//! stand from the same set (ADR 0035).
//!
//! A value is stored in pieces. Windows Credential Manager holds 2560 bytes
//! per entry - 1280 characters as it stores them - and a private key or a
//! signed token is longer than that. The catalog says how many pieces a
//! variable has; an empty value has none, and nothing is stored for it.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{secrets, store};

/// The variables of one app on one server, by name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EnvSet {
    pub app: String,
    pub server: String,
    #[serde(default)]
    pub variables: Vec<Variable>,
}

/// One variable: its name, and how many keyring entries hold its value.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    /// 0 for an empty value, which has nothing to store.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub parts: u32,
}

fn one() -> u32 {
    1
}

fn is_one(parts: &u32) -> bool {
    *parts == 1
}

/// The longest piece of a value one keyring entry is given, in UTF-16 units -
/// what Windows counts. Under its 1280 with room to spare.
const PART_UNITS: usize = 1000;

/// A variable's name as every shell and every dotenv reader takes it.
pub fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let starts = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if !starts || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        bail!("invalid variable name '{name}': use letters, digits and underscores, not starting with a digit");
    }
    Ok(())
}

/// The keyring account of one piece of a value. No credential can be called
/// this: their names hold no slash.
fn account(app: &str, server: &str, name: &str, part: u32) -> String {
    if part == 0 {
        format!("env/{app}/{server}/{name}")
    } else {
        format!("env/{app}/{server}/{name}/{part}")
    }
}

/// The key a variable's whole value travels under in an export's sealed
/// secrets: its first account.
pub fn sealed_key(app: &str, server: &str, name: &str) -> String {
    account(app, server, name, 0)
}

/// `app`, `server` and the variable a sealed key names, when it names one.
pub fn read_sealed_key(key: &str) -> Option<(&str, &str, &str)> {
    let mut parts = key.strip_prefix("env/")?.split('/');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(app), Some(server), Some(name), None) => Some((app, server, name)),
        _ => None,
    }
}

/// A value cut into pieces a keyring entry holds, never through a character.
fn pieces(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut units = 0;
    for c in value.chars() {
        if units + c.len_utf16() > PART_UNITS {
            out.push(std::mem::take(&mut piece));
            units = 0;
        }
        piece.push(c);
        units += c.len_utf16();
    }
    if !piece.is_empty() {
        out.push(piece);
    }
    out
}

/// Every set, in the catalog's order: by app, then by server.
pub fn sets() -> Result<Vec<EnvSet>> {
    store::load_env()
}

/// The names of the variables `app` has on `server`, in name order.
pub fn names(app: &str, server: &str) -> Result<Vec<String>> {
    Ok(find(&store::load_env()?, app, server)
        .map(|set| set.variables.iter().map(|variable| variable.name.clone()).collect())
        .unwrap_or_default())
}

fn find<'a>(sets: &'a [EnvSet], app: &str, server: &str) -> Option<&'a EnvSet> {
    sets.iter().find(|set| set.app == app && set.server == server)
}

/// What [`put`] did, by name.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stored {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
}

/// Store `pairs` as variables of `app` on `server`; a name already there gets
/// the new value.
///
/// The keyring first, the catalog after: a write that fails half-way leaves
/// values no name points at, which the next `set` of that name overwrites -
/// never a name whose value is not there.
pub fn put(app: &str, server: &str, pairs: &[(String, String)]) -> Result<Stored> {
    for (name, _) in pairs {
        validate_name(name)?;
    }
    let mut sets = store::load_env()?;
    let index = match sets.iter().position(|set| set.app == app && set.server == server) {
        Some(index) => index,
        None => {
            sets.push(EnvSet {
                app: app.to_string(),
                server: server.to_string(),
                variables: Vec::new(),
            });
            sets.len() - 1
        }
    };
    let mut stored = Stored::default();
    for (name, value) in pairs {
        let pieces = pieces(value);
        for (part, piece) in pieces.iter().enumerate() {
            secrets::set(&account(app, server, name, part as u32), piece).with_context(|| format!("cannot store {name}"))?;
        }
        let parts = pieces.len() as u32;
        let variables = &mut sets[index].variables;
        match variables.iter_mut().find(|variable| &variable.name == name) {
            Some(variable) => {
                // A shorter value leaves the old one's later pieces behind.
                for part in parts..variable.parts {
                    secrets::delete(&account(app, server, name, part))?;
                }
                variable.parts = parts;
                if !stored.replaced.contains(name) && !stored.added.contains(name) {
                    stored.replaced.push(name.clone());
                }
            }
            None => {
                variables.push(Variable { name: name.clone(), parts });
                stored.added.push(name.clone());
            }
        }
    }
    sets[index].variables.sort_by(|a, b| a.name.cmp(&b.name));
    sets.sort_by(|a, b| (&a.app, &a.server).cmp(&(&b.app, &b.server)));
    store::save_env(&sets)?;
    Ok(stored)
}

/// The variables of `app` on `server` with their values, in name order.
///
/// A name whose value the keyring does not hold is an error that says which:
/// a command started without it would fail somewhere in the app, with the
/// app's own words for a missing password.
pub fn values(app: &str, server: &str) -> Result<Vec<(String, String)>> {
    let sets = store::load_env()?;
    let Some(set) = find(&sets, app, server) else {
        return Ok(Vec::new());
    };
    set.variables
        .iter()
        .map(|variable| {
            let mut value = String::new();
            for part in 0..variable.parts {
                let piece = secrets::find(&account(app, server, &variable.name, part))?.ok_or_else(|| {
                    anyhow::anyhow!(
                        "the OS keyring does not hold {0}, a variable of '{app}' on '{server}' - store it again with `turnout env set {0} --app {app} --server {server}`",
                        variable.name
                    )
                })?;
                value.push_str(&piece);
            }
            Ok((variable.name.clone(), value))
        })
        .collect()
}

/// Remove variables of `app` on `server`: the ones named, or all of them.
/// Returns the names that went; a name that was not there is an error before
/// anything is removed.
pub fn take(app: &str, server: &str, only: Option<&[String]>) -> Result<Vec<String>> {
    let mut sets = store::load_env()?;
    let Some(index) = sets.iter().position(|set| set.app == app && set.server == server) else {
        match only {
            Some([name, ..]) => bail!("'{app}' has no variable {name} on '{server}' - see `turnout env list`"),
            _ => return Ok(Vec::new()),
        }
    };
    if let Some(only) = only {
        for name in only {
            if !sets[index].variables.iter().any(|variable| &variable.name == name) {
                bail!("'{app}' has no variable {name} on '{server}' - see `turnout env list`");
            }
        }
    }
    let goes = |variable: &Variable| only.is_none_or(|only| only.contains(&variable.name));
    let mut gone = Vec::new();
    for variable in sets[index].variables.iter().filter(|variable| goes(variable)) {
        erase(app, server, variable)?;
        gone.push(variable.name.clone());
    }
    sets[index].variables.retain(|variable| !goes(variable));
    if sets[index].variables.is_empty() {
        sets.remove(index);
    }
    store::save_env(&sets)?;
    Ok(gone)
}

fn erase(app: &str, server: &str, variable: &Variable) -> Result<()> {
    for part in 0..variable.parts {
        secrets::delete(&account(app, server, &variable.name, part))?;
    }
    Ok(())
}

/// An app or a server left the catalog: its variables go with it. Returns how
/// many went.
///
/// Best-effort on the keyring: the entity is being removed either way, and a
/// keyring that refuses must not leave the catalogs half-changed. What it
/// would not delete is named.
pub fn forget(is_gone: impl Fn(&EnvSet) -> bool) -> Result<usize> {
    let mut sets = store::load_env()?;
    let mut count = 0;
    for set in sets.iter().filter(|set| is_gone(*set)) {
        for variable in &set.variables {
            if let Err(err) = erase(&set.app, &set.server, variable) {
                eprintln!("warning: {} of '{}' on '{}' stays in the keyring: {err:#}", variable.name, set.app, set.server);
            }
            count += 1;
        }
    }
    if sets.iter().any(|set| is_gone(set)) {
        sets.retain(|set| !is_gone(set));
        store::save_env(&sets)?;
    }
    Ok(count)
}

/// Read dotenv text into pairs, in the file's order.
///
/// What every reader of such a file agrees on: `NAME=value` lines, `#`
/// comments, an optional `export `, a value bare, in single quotes (taken
/// literally) or in double quotes (with `\n`, `\r`, `\t`, `\"`, `\\` and `\$`
/// escaped); a quoted value may run over several lines. The block turnout
/// keeps in an app's dotenv file ([`crate::envfile`]) is passed over: that
/// line is the gateway's address on this machine, not a variable of the app.
pub fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let number = at + 1;
        let line = lines[at].trim_start_matches('\u{feff}').trim();
        at += 1;
        if line == crate::envfile::HEADER {
            // The assignment under the header is the block's.
            if lines.get(at).is_some_and(|next| next.contains('=') && !next.trim_start().starts_with('#')) {
                at += 1;
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map_or(line, str::trim_start);
        // The line itself is not quoted back: what stands where a variable
        // was expected may be half of a secret.
        let Some((name, rest)) = line.split_once('=') else {
            bail!("line {number} is not NAME=value");
        };
        let name = name.trim();
        validate_name(name).with_context(|| format!("line {number}"))?;
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                // Up to the closing quote, on this line or a later one.
                let mut body = rest[1..].to_string();
                loop {
                    if let Some(end) = closing(&body, quote) {
                        body.truncate(end);
                        break;
                    }
                    let Some(next) = lines.get(at) else {
                        bail!("line {number}: the quote opened for {name} is never closed");
                    };
                    body.push('\n');
                    body.push_str(next);
                    at += 1;
                }
                if quote == '"' { unescape(&body) } else { body }
            }
            // Bare: up to a comment that follows a space.
            _ => rest.split(" #").next().unwrap_or_default().trim_end().to_string(),
        };
        pairs.push((name.to_string(), value));
    }
    Ok(pairs)
}

/// Where the quote that closes a quoted value is, if it is in `body`.
fn closing(body: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (at, c) in body.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote == '"' {
            escaped = true;
        } else if c == quote {
            return Some(at);
        }
    }
    None
}

fn unescape(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other @ ('"' | '\\' | '$')) => out.push(other),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Pairs as dotenv text, one line each.
///
/// A plain value is written bare. Anything else goes into single quotes,
/// which every reader - dotenv libraries, Docker Compose, systemd, a shell -
/// takes literally: a `$` in a password stays a `$`. Only a value with a
/// single quote or a line break of its own needs double quotes and escapes.
pub fn render(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, value) in pairs {
        out.push_str(name);
        out.push('=');
        let plain = value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '@' | '%' | '+' | ',' | '=' | '-'));
        if plain {
            out.push_str(value);
        } else if !value.contains(['\'', '\n', '\r']) {
            out.push('\'');
            out.push_str(value);
            out.push('\'');
        } else {
            out.push('"');
            for c in value.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    other => out.push(other),
                }
            }
            out.push('"');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn names_are_what_a_shell_takes() {
        for good in ["A", "_x", "DATABASE_URL", "vite_Api2"] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in ["", "2X", "MY-VAR", "A B", "A=B", "ключ"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    /// The shapes a real `.env` holds, each read the way its readers read it.
    #[test]
    fn a_dotenv_file_is_read_as_its_readers_read_it() {
        let text = "\u{feff}# a comment\n\
                    PLAIN=value\n\
                    export EXPORTED=1\n\
                    \n\
                    SPACED = padded value   \n\
                    COMMENTED=value # not part of it\n\
                    HASH=a#b\n\
                    EMPTY=\n\
                    SINGLE='lit $HOME \\n'\n\
                    DOUBLE=\"line\\nnext \\\"q\\\" \\$5\"\n\
                    URL=postgres://u:p@host:5432/db?sslmode=require\n";
        assert_eq!(
            parse(text).unwrap(),
            [
                pair("PLAIN", "value"),
                pair("EXPORTED", "1"),
                pair("SPACED", "padded value"),
                pair("COMMENTED", "value"),
                pair("HASH", "a#b"),
                pair("EMPTY", ""),
                pair("SINGLE", "lit $HOME \\n"),
                pair("DOUBLE", "line\nnext \"q\" $5"),
                pair("URL", "postgres://u:p@host:5432/db?sslmode=require"),
            ]
        );
    }

    /// A private key pasted into a file: quoted, over several lines.
    #[test]
    fn a_quoted_value_runs_over_lines() {
        let text = "KEY=\"-----BEGIN-----\nabc\ndef\n-----END-----\"\nAFTER=1\nLIT='a\nb'\n";
        assert_eq!(
            parse(text).unwrap(),
            [pair("KEY", "-----BEGIN-----\nabc\ndef\n-----END-----"), pair("AFTER", "1"), pair("LIT", "a\nb")]
        );
        let open = parse("A=1\nKEY=\"never closed\nB=2\n").unwrap_err().to_string();
        assert!(open.contains("line 2") && open.contains("KEY"), "{open}");
    }

    #[test]
    fn a_line_that_is_no_assignment_is_named() {
        let err = parse("A=1\njust words\n").unwrap_err().to_string();
        assert!(err.contains("line 2"), "{err}");
        let err = format!("{:#}", parse("MY-VAR=1\n").unwrap_err());
        assert!(err.contains("line 1") && err.contains("MY-VAR"), "{err}");
    }

    /// The gateway's address on this machine is turnout's line, not a
    /// variable of the app: an import must not carry localhost to a stand.
    #[test]
    fn the_block_turnout_keeps_is_passed_over() {
        let text = format!("A=1\n\n{}\nVITE_API_URL=http://localhost:7100\nB=2\n", crate::envfile::HEADER);
        assert_eq!(parse(&text).unwrap(), [pair("A", "1"), pair("B", "2")]);
    }

    /// What is written reads back as it was - through this parser, and by the
    /// rules every other reader shares.
    #[test]
    fn what_is_written_reads_back() {
        let pairs = [
            pair("PLAIN", "abc-123_./:@%+,="),
            pair("EMPTY", ""),
            pair("SPACES", "two words"),
            pair("DOLLAR", "pa$$word #1"),
            pair("QUOTE", "it's \"here\""),
            pair("LINES", "one\ntwo\r\nthree \\ end"),
            pair("BOTH", "'\"$\\"),
        ];
        let text = render(&pairs);
        assert_eq!(parse(&text).unwrap(), pairs, "{text}");
        assert!(text.contains("PLAIN=abc-123_./:@%+,=\n"), "{text}");
        assert!(text.contains("DOLLAR='pa$$word #1'\n"), "a dollar stays literal: {text}");
        assert!(text.contains("EMPTY=\n"), "{text}");
        assert_eq!(text.lines().count(), pairs.len(), "one line per variable: {text}");
    }

    /// A value is cut between characters, and every piece fits an entry.
    #[test]
    fn a_long_value_is_stored_in_pieces() {
        assert!(pieces("").is_empty());
        assert_eq!(pieces("short"), ["short"]);
        let long: String = "ab😀".repeat(900);
        let cut = pieces(&long);
        assert!(cut.len() > 1);
        assert!(cut.iter().all(|piece| piece.encode_utf16().count() <= PART_UNITS));
        assert_eq!(cut.concat(), long);
    }

    #[test]
    fn a_sealed_key_names_its_variable() {
        let key = sealed_key("web", "staging", "DATABASE_URL");
        assert_eq!(read_sealed_key(&key), Some(("web", "staging", "DATABASE_URL")));
        assert_eq!(read_sealed_key("deploy"), None, "a credential's name is not one");
        assert_eq!(read_sealed_key("env/web/staging/KEY/1"), None, "a later piece is not a variable");
    }
}
