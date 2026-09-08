//! How an app learns the gateway's address.
//!
//! The port used to be written twice: once in the app's own `.env` ("the
//! backend is on localhost:7001") and once more in `turnout app add` - and
//! the two drifted apart. The port now lives in turnout only, and the app is
//! handed the *address* by two roads that back each other up:
//!
//! - an environment variable, set on every command `turnout` starts for the
//!   app (`exec`), so a project needs no file at all when it runs through
//!   turnout;
//! - a dotenv file turnout keeps in step (this module), for the times the
//!   app is started past turnout - from an IDE, or with `pnpm dev` by hand.
//!
//! The file is `.env.development.local` by default: Vite and CRA read
//! `.env.*.local` on top of `.env` in development only, so the project's own
//! `.env` stays untouched and a production build never sees the address.
//!
//! Only one block of the file is turnout's: a header line and the assignment
//! under it. Everything else in the file is somebody else's and survives
//! every rewrite. The header is the anchor - not the variable name - so a
//! renamed variable replaces the old line rather than leaving it behind.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::model::App;

/// The line that marks turnout's block. Kept stable: it is what later runs
/// look for.
pub const HEADER: &str = "# Managed by turnout - the line below follows the app's gateway port.";

/// What `write` did with the file.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The app has no gateway port and the file held no block of ours.
    Nothing,
    /// The file already said this.
    Unchanged,
    /// The file was written and now carries this assignment.
    Written(String),
    /// The app lost its port; our block is gone, the rest of the file stays.
    Removed,
}

/// Bring the app's dotenv file in step with its gateway port.
///
/// A missing project directory is not an error here: the catalog may name a
/// directory that moved, and `app show` already warns about that.
pub fn write(app: &App) -> Result<Outcome> {
    let dir = Path::new(&app.path);
    if !dir.is_dir() {
        return Ok(Outcome::Nothing);
    }
    let path = dir.join(app.env_file_name());
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };

    let Some(url) = app.gateway_url() else {
        return match existing {
            Some(text) if text.contains(HEADER) => {
                let cleared = without_block(&text);
                if cleared.trim().is_empty() {
                    std::fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
                } else {
                    std::fs::write(&path, cleared).with_context(|| format!("cannot write {}", path.display()))?;
                }
                Ok(Outcome::Removed)
            }
            _ => Ok(Outcome::Nothing),
        };
    };

    let assignment = format!("{}={url}", app.gateway_env_name());
    let rendered = render(existing.as_deref().unwrap_or(""), &assignment);
    if existing.as_deref() == Some(rendered.as_str()) {
        return Ok(Outcome::Unchanged);
    }
    std::fs::write(&path, &rendered).with_context(|| format!("cannot write {}", path.display()))?;
    ensure_ignored(dir, app.env_file_name())?;
    Ok(Outcome::Written(assignment))
}

/// The file's text with turnout's block set to `assignment`.
///
/// Pure, so the shape of the result is testable without a disk: foreign lines
/// keep their order, a block is replaced in place, a file without one gets
/// the block appended after a blank line, and rendering twice changes
/// nothing.
pub fn render(existing: &str, assignment: &str) -> String {
    let block = format!("{HEADER}\n{assignment}\n");
    let lines: Vec<&str> = existing.lines().collect();
    if let Some(at) = lines.iter().position(|line| *line == HEADER) {
        // The block is the header and the assignment under it; a header
        // with nothing under it (a hand edit) is still a block.
        let after = if lines.get(at + 1).is_some_and(|line| is_assignment(line)) {
            at + 2
        } else {
            at + 1
        };
        let mut out = String::new();
        for line in &lines[..at] {
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(&block);
        for line in &lines[after..] {
            out.push_str(line);
            out.push('\n');
        }
        return out;
    }
    if lines.is_empty() {
        return block;
    }
    let mut out = String::new();
    for line in &lines {
        out.push_str(line);
        out.push('\n');
    }
    if !lines.last().is_some_and(|line| line.trim().is_empty()) {
        out.push('\n');
    }
    out.push_str(&block);
    out
}

/// The file's text with turnout's block taken out.
fn without_block(existing: &str) -> String {
    let lines: Vec<&str> = existing.lines().collect();
    let Some(at) = lines.iter().position(|line| *line == HEADER) else {
        return existing.to_string();
    };
    let after = if lines.get(at + 1).is_some_and(|line| is_assignment(line)) {
        at + 2
    } else {
        at + 1
    };
    let mut kept: Vec<&str> = lines[..at].to_vec();
    kept.extend_from_slice(&lines[after..]);
    // The blank line that separated the block from what came before it
    // belongs to the block.
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn is_assignment(line: &str) -> bool {
    let line = line.trim_start();
    !line.is_empty() && !line.starts_with('#') && line.contains('=')
}

/// Keep the dotenv file out of git.
///
/// Appends the file name to `.gitignore` when the project has one (or is a
/// repository without one). A directory that is not a repository is left
/// alone: a `.gitignore` there would be turnout's litter, not the user's.
fn ensure_ignored(dir: &Path, file_name: &str) -> Result<()> {
    let gitignore = dir.join(".gitignore");
    let existing = match std::fs::read_to_string(&gitignore) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !dir.join(".git").exists() {
                return Ok(());
            }
            String::new()
        }
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", gitignore.display())),
    };
    if existing.lines().any(|line| ignores(line.trim(), file_name)) {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(file_name);
    text.push('\n');
    std::fs::write(&gitignore, text).with_context(|| format!("cannot write {}", gitignore.display()))
}

/// Whether a `.gitignore` line already covers the file. The two common
/// spellings the file has - `.env.development.local` and the `.env*.local`
/// / `*.local` globs every framework template ships - are matched as a
/// literal and as a glob with `*` only; anything fancier is nobody's ignore
/// line for a dotenv file.
fn ignores(pattern: &str, file_name: &str) -> bool {
    let pattern = pattern.trim_start_matches('/');
    if pattern == file_name {
        return true;
    }
    if !pattern.contains('*') || pattern.contains('/') {
        return false;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = file_name;
    if let Some(first) = parts.first() {
        let Some(after) = rest.strip_prefix(first) else { return false };
        rest = after;
    }
    if let Some(last) = parts.last() {
        let Some(before) = rest.strip_suffix(last) else { return false };
        rest = before;
    }
    parts[1..parts.len() - 1].iter().all(|part| match rest.find(part) {
        Some(at) => {
            rest = &rest[at + part.len()..];
            true
        }
        None => false,
    })
}

/// The command line with turnout's placeholders filled in.
///
/// `{gateway}` is the URL, `{gateway_port}` the number. Both need a port;
/// a command that asks for one on an app without it is a configuration
/// error and says so, rather than running with the braces left in.
pub fn substitute(command_line: &str, app: &App) -> Result<String> {
    const PLACEHOLDERS: [&str; 2] = ["{gateway}", "{gateway_port}"];
    if !PLACEHOLDERS.iter().any(|p| command_line.contains(p)) {
        return Ok(command_line.to_string());
    }
    let Some(port) = app.gateway_port else {
        bail!(
            "the command uses {{gateway}} but app '{0}' has no gateway port - set one with `turnout app edit {0} --port PORT`",
            app.name
        );
    };
    Ok(command_line
        .replace("{gateway}", &format!("http://localhost:{port}"))
        .replace("{gateway_port}", &port.to_string()))
}

/// Where the app's dotenv file lives, for messages.
pub fn path_of(app: &App) -> PathBuf {
    Path::new(&app.path).join(app.env_file_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn app(port: Option<u16>) -> App {
        App {
            name: "web".into(),
            path: "/tmp/web".into(),
            commands: BTreeMap::new(),
            dist_dir: None,
            gateway_port: port,
            gateway_env: None,
            env_file: None,
            servers: Vec::new(),
        }
    }

    #[test]
    fn an_empty_file_gets_the_block_alone() {
        assert_eq!(render("", "X=1"), format!("{HEADER}\nX=1\n"));
    }

    #[test]
    fn foreign_lines_survive_in_order_and_the_block_lands_after_a_blank_line() {
        let existing = "A=1\n# theirs\nB=2\n";
        let out = render(existing, "X=1");
        assert_eq!(out, format!("A=1\n# theirs\nB=2\n\n{HEADER}\nX=1\n"));
        // A file that already ends in a blank line gets no second one.
        assert_eq!(render("A=1\n\n", "X=1"), format!("A=1\n\n{HEADER}\nX=1\n"));
    }

    #[test]
    fn the_block_is_replaced_in_place_including_a_renamed_variable() {
        let first = render("A=1\n", "OLD=http://localhost:7001");
        let second = render(&first, "NEW=http://localhost:7002");
        assert_eq!(second, format!("A=1\n\n{HEADER}\nNEW=http://localhost:7002\n"));
        assert!(!second.contains("OLD="), "the renamed variable left its old line behind");
        // Lines after the block survive too.
        let with_tail = format!("{first}TAIL=1\n");
        let out = render(&with_tail, "X=1");
        assert_eq!(out, format!("A=1\n\n{HEADER}\nX=1\nTAIL=1\n"));
    }

    #[test]
    fn rendering_twice_changes_nothing() {
        let once = render("A=1\n", "X=1");
        assert_eq!(render(&once, "X=1"), once);
    }

    #[test]
    fn a_header_without_an_assignment_is_still_the_block() {
        // The line under the header is the block's by definition; a header
        // followed by anything that is not an assignment has lost its line.
        let out = render(&format!("A=1\n{HEADER}\n# note\nB=2\n"), "X=1");
        assert_eq!(out, format!("A=1\n{HEADER}\nX=1\n# note\nB=2\n"));
        let out = render(&format!("A=1\n{HEADER}\n"), "X=1");
        assert_eq!(out, format!("A=1\n{HEADER}\nX=1\n"));
    }

    #[test]
    fn removing_the_block_keeps_the_rest_and_its_own_blank_line_goes_with_it() {
        let text = render("A=1\n", "X=1");
        assert_eq!(without_block(&text), "A=1\n");
        assert_eq!(without_block(&render("", "X=1")), "");
        assert_eq!(without_block("A=1\n"), "A=1\n");
    }

    #[test]
    fn gitignore_patterns_that_cover_the_file_are_recognised() {
        for pattern in [".env.development.local", "/.env.development.local", ".env*.local", "*.local", ".env.*", "*"] {
            assert!(ignores(pattern, ".env.development.local"), "{pattern} should cover the file");
        }
        for pattern in [".env", ".env.local", "*.log", "build/*.local", ".env.production.local"] {
            assert!(!ignores(pattern, ".env.development.local"), "{pattern} should not cover the file");
        }
    }

    #[test]
    fn placeholders_take_the_port_and_refuse_without_one() {
        let out = substitute("vite --api {gateway} --p {gateway_port}", &app(Some(7001))).unwrap();
        assert_eq!(out, "vite --api http://localhost:7001 --p 7001");
        assert_eq!(substitute("pnpm dev", &app(None)).unwrap(), "pnpm dev");
        let error = substitute("x {gateway}", &app(None)).unwrap_err().to_string();
        assert!(error.contains("has no gateway port"), "{error}");
    }

    #[test]
    fn write_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "node_modules\n").unwrap();
        let mut app = app(Some(7001));
        app.path = dir.path().display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        assert_eq!(write(&app).unwrap(), Outcome::Written("VITE_API_URL=http://localhost:7001".into()));
        assert_eq!(write(&app).unwrap(), Outcome::Unchanged);
        let file = dir.path().join(".env.development.local");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            format!("{HEADER}\nVITE_API_URL=http://localhost:7001\n")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "node_modules\n.env.development.local\n"
        );
        // Losing the port takes the block out; an otherwise empty file goes.
        app.gateway_port = None;
        assert_eq!(write(&app).unwrap(), Outcome::Removed);
        assert!(!file.exists(), "an empty managed file was left behind");
        assert_eq!(write(&app).unwrap(), Outcome::Nothing);
    }

    #[test]
    fn write_leaves_a_directory_without_git_unlittered() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(Some(7001));
        app.path = dir.path().display().to_string();
        write(&app).unwrap();
        assert!(!dir.path().join(".gitignore").exists(), "a .gitignore was invented outside a repository");
    }
}
