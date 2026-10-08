//! `turnout cookie`: hand the gateway a stand's session from a browser.
//!
//! The gateway logs in to a stand the way a browser would, through the login
//! page - and keeps the cookies (ADR 0002). Some stands do not allow that: the
//! login is a single sign-on page on another domain, which redirects back to
//! the stand, not to localhost. Those stands are logged in to in a browser
//! only, and the session has to be carried over by hand: copy the request's
//! `Cookie` header (or the whole request, as cURL) from the developer tools,
//! and `turnout cookie set` reads it from the clipboard.
//!
//! The values go to the gateway's memory through its control channel (ADR
//! 0021) and nowhere else: not to a file, not to the journal, not back out.

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result, bail};

use crate::cli::{CookieCommand, Format};
use crate::control::{self, JarView};
use crate::model::Gateway;
use crate::{output, store};

pub fn run(command: CookieCommand) -> Result<()> {
    match command {
        CookieCommand::Set { app, server } => set(app, server),
        CookieCommand::List { format } => list(format),
        CookieCommand::Clear { app, server } => clear(app, server),
    }
}

fn set(app: Option<String>, server: Option<String>) -> Result<()> {
    let (app, server) = target(app, server)?;
    let gateway = gateway()?;
    let (text, source) = copied()?;
    let cookies = parse(&text).with_context(|| format!("no cookies in {source}"))?;
    let names: Vec<String> = cookies.iter().map(|(name, _)| name.clone()).collect();
    control::set_jar(control_of(&gateway)?, &app, &server, cookies)?;
    crate::journal::record("cookie.set", Some(&app), Some(&server), Some(&format!("{} cookies", names.len())));
    println!(
        "The gateway now sends {} {} to '{server}' for '{app}': {}.",
        names.len(),
        if names.len() == 1 { "cookie" } else { "cookies" },
        names.join(", ")
    );
    Ok(())
}

fn clear(app: Option<String>, server: Option<String>) -> Result<()> {
    let (app, server) = target(app, server)?;
    let gateway = gateway()?;
    control::clear_jar(control_of(&gateway)?, &app, &server)?;
    crate::journal::record("cookie.clear", Some(&app), Some(&server), None);
    println!("The gateway forgot the cookies of '{app}' for '{server}' - the next request there is logged out.");
    Ok(())
}

fn list(format: Format) -> Result<()> {
    // A gateway that is not running holds no cookies: that is an answer, not
    // an error.
    let jars: Vec<JarView> = match crate::registry::gateway()?.filter(crate::commands::gateway::probe) {
        Some(gateway) => control::jars(control_of(&gateway)?)?,
        None => Vec::new(),
    };
    output::emit(format, &jars, |jars| {
        if jars.is_empty() {
            return "The gateway holds no cookies - it is not running, or nothing has logged in through it yet.".to_string();
        }
        let width = jars.iter().map(|jar| jar.app.len() + jar.server.len() + 4).max().unwrap_or(0);
        let now = crate::registry::now();
        let span = |at: &str| output::parse_iso8601(at).map(|at| crate::progress::human_span(at.abs_diff(now)));
        let lines: Vec<String> = jars
            .iter()
            .map(|jar| {
                let mut line = format!("{:width$}  {}", format!("{} -> {}", jar.app, jar.server), jar.cookies.join(", "));
                if let Some(left) = jar.expires.as_deref().and_then(span) {
                    line.push_str(&format!(" · expire in {left}"));
                }
                if let Some(answer) = &jar.last_answer
                    && let Some(ago) = span(&answer.at)
                {
                    line.push_str(&format!(" · the stand answered {} {ago} ago", answer.status));
                }
                line
            })
            .collect();
        lines.join("\n")
    })
}

/// The app, and the stand whose cookies these are: named, or the app's
/// current binding.
fn target(app: Option<String>, server: Option<String>) -> Result<(String, String)> {
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, app)?.name.clone();
    let server = match server {
        Some(server) => server,
        None => store::load_state()?
            .bindings
            .get(&app)
            .cloned()
            .with_context(|| format!("'{app}' is not bound to a server - pass --server, or bind it with `turnout use {app} SERVER`"))?,
    };
    if !store::load_servers()?.iter().any(|s| s.name == server) {
        bail!("no server named '{server}' - see `turnout server list`");
    }
    Ok((app, server))
}

/// The running gateway; the cookies live in its memory.
fn gateway() -> Result<Gateway> {
    crate::registry::gateway()?
        .filter(crate::commands::gateway::probe)
        .context("the gateway is not running - cookies live in its memory; start it with `turnout gateway start`, then run this again")
}

fn control_of(gateway: &Gateway) -> Result<&crate::registry::Control> {
    gateway
        .control
        .as_ref()
        .context("this gateway was started by an older turnout and takes no cookies - restart it: `turnout gateway stop`, then `turnout gateway start`")
}

/// The text to read cookies from, and where it came from: standard input when
/// something is piped in, the clipboard otherwise.
///
/// "Piped in" means there is text there, not merely that standard input is
/// not a terminal: a shell started by an IDE or a task runner has no terminal
/// and nothing on its input either, and the clipboard is what was meant.
fn copied() -> Result<(String, &'static str)> {
    if !std::io::stdin().is_terminal() {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).context("cannot read standard input")?;
        if !text.trim().is_empty() {
            return Ok((text, "standard input"));
        }
    }
    let text = arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.get_text())
        .context("cannot read the clipboard - copy the Cookie header first, or pipe it in")?;
    Ok((text, "the clipboard"))
}

/// Cookie attributes: a `Set-Cookie` header has them, a `Cookie` header never.
const ATTRIBUTES: [&str; 7] = ["path", "domain", "expires", "max-age", "samesite", "secure", "httponly"];

/// The cookies in text copied from a browser's developer tools.
///
/// Takes the forms the tools hand out: the `Cookie` request header (with or
/// without its name), or the whole request copied as cURL - for bash, where
/// the cookies ride in `-H 'cookie: ...'` or `-b '...'`; for cmd, the same
/// wrapped in `^"`; or for PowerShell, one `System.Net.Cookie(...)` each.
pub fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let text = text.trim();
    if text.contains("System.Net.Cookie(") {
        return non_empty(powershell(text));
    }
    let header = if looks_like_curl(text) {
        from_curl(text).context("the cURL command carries no cookies - copy a request the browser sent while logged in")?
    } else {
        header_value(text)?
    };
    non_empty(pairs(&header)?)
}

fn non_empty(cookies: Vec<(String, String)>) -> Result<Vec<(String, String)>> {
    if cookies.is_empty() {
        bail!("copy the request's Cookie header, or the whole request as cURL, from the browser's developer tools");
    }
    Ok(cookies)
}

fn looks_like_curl(text: &str) -> bool {
    text.get(..5).is_some_and(|start| start.eq_ignore_ascii_case("curl "))
}

/// The value of a `Cookie` header: the text itself, or the `cookie:` line of
/// several copied headers.
fn header_value(text: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    let line = match lines.as_slice() {
        [] => bail!("the copied text is empty"),
        [one] => *one,
        many => many
            .iter()
            .find(|line| strip_name(line).is_some())
            .copied()
            .context("several lines were copied, and none of them is the Cookie header")?,
    };
    Ok(strip_name(line).unwrap_or(line).to_string())
}

/// `cookie: a=1` without its name; `None` when the line names another header.
fn strip_name(line: &str) -> Option<&str> {
    let (name, value) = line.split_once(':')?;
    name.trim().eq_ignore_ascii_case("cookie").then(|| value.trim())
}

/// `name=value; name=value` as pairs.
fn pairs(header: &str) -> Result<Vec<(String, String)>> {
    let mut cookies = Vec::new();
    for part in header.split(';').map(str::trim).filter(|part| !part.is_empty()) {
        let (name, value) = part.split_once('=').unwrap_or((part, ""));
        let name = name.trim();
        if ATTRIBUTES.contains(&name.to_ascii_lowercase().as_str()) {
            bail!("this is a Set-Cookie header, an answer's - copy the Cookie header of a request instead");
        }
        if name.is_empty() || !part.contains('=') {
            bail!("'{part}' is not a cookie: expected name=value");
        }
        if name.chars().chain(value.chars()).any(|c| c.is_control() || (c.is_whitespace() && c != ' ')) || name.contains(' ') {
            bail!("the cookie '{name}' holds characters a Cookie header cannot carry");
        }
        cookies.push((name.to_string(), value.trim().to_string()));
    }
    Ok(cookies)
}

/// The cookies of a request copied as cURL, as one header value.
fn from_curl(text: &str) -> Option<String> {
    let words = shell_words(text);
    let mut found = Vec::new();
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-H" | "--header" => {
                if let Some(value) = words.next().and_then(|header| strip_name(header)) {
                    found.push(value.to_string());
                }
            }
            "-b" | "--cookie" => {
                // Without `=` it names a cookie file, which is not ours to read.
                if let Some(value) = words.next().filter(|value| value.contains('=')) {
                    found.push(value.clone());
                }
            }
            _ => {}
        }
    }
    (!found.is_empty()).then(|| found.join("; "))
}

/// Split a copied command line into words, the way the shell it was copied
/// for would: bash quoting (`'...'`, `"..."`, `$'...'`), or cmd's, where
/// every special character is escaped with `^`.
fn shell_words(text: &str) -> Vec<String> {
    // cmd first becomes bash: `^"` is a quote, `^X` is X. `\^"` inside a
    // quoted cmd word is an escaped quote.
    let text = if text.contains("^\"") {
        let mut plain = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '^' {
                if let Some(next) = chars.next() {
                    plain.push(next);
                }
            } else {
                plain.push(c);
            }
        }
        plain
    } else {
        text.to_string()
    };
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '\'' => break,
                        '\\' => match chars.next() {
                            Some('n') => word.push('\n'),
                            Some('t') => word.push('\t'),
                            Some(other) => word.push(other),
                            None => {}
                        },
                        c => word.push(c),
                    }
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                if !matches!(next, '"' | '\\' | '$' | '`') {
                                    word.push('\\');
                                }
                                word.push(next);
                            }
                        }
                        c => word.push(c),
                    }
                }
            }
            // A line continuation: bash's backslash-newline, cmd's caret
            // already gone.
            '\\' if matches!(chars.peek(), Some('\n' | '\r')) => {}
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// `New-Object System.Net.Cookie("name", "value", ...)`, as "Copy as
/// PowerShell" writes them.
fn powershell(text: &str) -> Vec<(String, String)> {
    text.split("System.Net.Cookie(")
        .skip(1)
        .filter_map(|call| {
            let mut quoted = call.split('"').skip(1).step_by(2);
            Some((quoted.next()?.to_string(), quoted.next()?.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_and_values(text: &str) -> Vec<(String, String)> {
        parse(text).unwrap_or_else(|err| panic!("{err:#}: {text}"))
    }

    fn pair(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn a_header_value_is_read_with_or_without_its_name() {
        let expected = vec![pair("sid", "abc123"), pair("csrftoken", "x=y")];
        assert_eq!(names_and_values("sid=abc123; csrftoken=x=y"), expected);
        assert_eq!(names_and_values("Cookie: sid=abc123; csrftoken=x=y\n"), expected);
        assert_eq!(names_and_values("cookie:sid=abc123;csrftoken=x=y;"), expected);
    }

    /// The developer tools copy several headers at once; the cookie line is
    /// the one that counts.
    #[test]
    fn the_cookie_line_is_found_among_other_headers() {
        let copied = "accept: application/json\ncookie: sid=abc123\nuser-agent: Mozilla/5.0";
        assert_eq!(names_and_values(copied), vec![pair("sid", "abc123")]);
        assert!(parse("accept: application/json\nuser-agent: Mozilla/5.0").is_err());
    }

    #[test]
    fn a_request_copied_as_curl_for_bash_gives_its_cookies() {
        let chrome = "curl 'https://staging.example.com/api/me' \\\n  -H 'accept: */*' \\\n  -b 'sid=abc123; lang=en' \\\n  -H 'user-agent: Mozilla/5.0'";
        assert_eq!(names_and_values(chrome), vec![pair("sid", "abc123"), pair("lang", "en")]);
        let firefox = "curl 'https://staging.example.com/api/me' -H 'Accept: */*' -H 'Cookie: sid=abc123'";
        assert_eq!(names_and_values(firefox), vec![pair("sid", "abc123")]);
        let ansi = "curl 'https://staging.example.com/' -H $'cookie: sid=a\\'b'";
        assert_eq!(names_and_values(ansi), vec![pair("sid", "a'b")]);
    }

    #[test]
    fn a_request_copied_as_curl_for_cmd_gives_its_cookies() {
        // Chrome's "Copy as cURL (cmd)": every special character behind a
        // caret, `%` included, and a caret at each line's end.
        let cmd =
            "curl ^\"https://staging.example.com/api/me^\" ^\n  -H ^\"accept: */*^\" ^\n  -b ^\"sid=abc^%123; lang=en^\" ^\n  -H ^\"user-agent: Mozilla/5.0^\"";
        assert_eq!(names_and_values(cmd), vec![pair("sid", "abc%123"), pair("lang", "en")]);
        let header = "curl ^\"https://staging.example.com/^\" -H ^\"cookie: sid=abc123^\"";
        assert_eq!(names_and_values(header), vec![pair("sid", "abc123")]);
    }

    #[test]
    fn a_request_copied_as_powershell_gives_its_cookies() {
        let copied = r#"$session = New-Object Microsoft.PowerShell.Commands.WebRequestSession
$session.Cookies.Add((New-Object System.Net.Cookie("sid", "abc123", "/", "staging.example.com")))
$session.Cookies.Add((New-Object System.Net.Cookie("lang", "en", "/", "staging.example.com")))
Invoke-WebRequest -UseBasicParsing -Uri "https://staging.example.com/api/me" -WebSession $session"#;
        assert_eq!(names_and_values(copied), vec![pair("sid", "abc123"), pair("lang", "en")]);
    }

    /// The wrong header, an empty copy and a request without cookies are
    /// refused with what to copy instead - never taken as an empty jar.
    #[test]
    fn what_is_not_a_request_cookie_is_refused() {
        let set_cookie = parse("sid=abc123; Path=/; HttpOnly").unwrap_err().to_string();
        assert!(set_cookie.contains("Set-Cookie"), "{set_cookie}");
        assert!(parse("").is_err());
        assert!(parse("   ").is_err());
        assert!(parse("curl 'https://staging.example.com/' -H 'accept: */*'").is_err());
        assert!(parse("curl 'https://x/' -b cookies.txt").is_err(), "a cookie file is not cookies");
        assert!(parse("just some words").is_err());
    }
}
