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
/// A repository is not always the app's own directory. In a monorepo the app
/// sits at `repo/apps/web` and the `.git` is levels up, so looking for a
/// `.gitignore` beside the dotenv file found nothing, concluded this was not
/// a repository at all, and left `.env.development.local` showing up as
/// untracked in every `git status` - the defect found in the demo under
/// `examples/`.
///
/// So the search walks up from the app to the repository root and writes into
/// the nearest `.gitignore` on the way, spelling the entry relative to *that*
/// file's directory. Only within the repository: a `.gitignore` above the
/// root belongs to somebody else.
///
/// When the repository has no `.gitignore` anywhere on that path, one is
/// started at its root, which is where a repository's ignores belong. A
/// directory under no repository and with no ignore file above it is still
/// left alone - a `.gitignore` invented there is turnout's litter.
fn ensure_ignored(dir: &Path, file_name: &str) -> Result<()> {
    let Some((gitignore, entry)) = ignore_target(dir, file_name) else {
        return Ok(());
    };
    let existing = match std::fs::read_to_string(&gitignore) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", gitignore.display())),
    };
    if existing.lines().any(|line| covers(line.trim(), &entry)) {
        return Ok(());
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&entry);
    text.push('\n');
    std::fs::write(&gitignore, text).with_context(|| format!("cannot write {}", gitignore.display()))
}

/// The `.gitignore` to write into and the entry to write, or `None` when
/// nothing on the way up says this is a repository at all.
///
/// The nearest existing `.gitignore` between the app directory and the
/// repository root wins, falling back to a new one at the root. The walk
/// stops at the root, so a `.gitignore` belonging to an outer repository is
/// never reached - by construction, not by a check. A `.gitignore` beside the
/// app with no `.git` above it anywhere still counts: the user put it there,
/// and writing into a file that exists is never litter. The entry is spelled
/// relative to the chosen file with forward slashes, the only separator git
/// reads on every platform.
fn ignore_target(dir: &Path, file_name: &str) -> Option<(PathBuf, String)> {
    let mut nearest: Option<PathBuf> = None;
    let mut root: Option<&Path> = None;
    let mut at = Some(dir);
    while let Some(current) = at {
        if nearest.is_none() && current.join(".gitignore").is_file() {
            nearest = Some(current.to_path_buf());
        }
        // `.git` is a directory in an ordinary clone and a file in a worktree
        // or a submodule; either marks the root, and the walk stops there.
        if current.join(".git").exists() {
            root = Some(current);
            break;
        }
        at = current.parent();
    }
    // The walk stops at the root, so anything `nearest` holds was found at or
    // below it - an outer repository's `.gitignore` is never a candidate, by
    // construction rather than by a check.
    let holder = match (nearest, root) {
        (Some(found), _) => found,
        // A repository with no ignore file anywhere on the path: start one
        // where a repository's ignores belong.
        (None, Some(root)) => root.to_path_buf(),
        (None, None) => return None,
    };
    let relative = dir.strip_prefix(&holder).ok()?;
    let mut entry = String::new();
    for part in relative.components() {
        entry.push_str(&part.as_os_str().to_string_lossy());
        entry.push('/');
    }
    entry.push_str(file_name);
    Some((holder.join(".gitignore"), entry))
}

/// Whether a `.gitignore` line already covers the entry, read the way git
/// reads one.
///
/// Two kinds of line, told apart the way git tells them apart - by whether
/// the pattern contains a slash:
///
/// * **No slash** (`.env.development.local`, `.env*.local`, `node_modules`):
///   it matches a *name* at any depth. Matching the file's own name covers
///   the file; matching a directory on the way to it covers everything under
///   that directory, the file included.
/// * **With a slash** (`apps/web/.env.development.local`,
///   `examples/*/.env.development.local`): it is anchored at the ignore
///   file's own directory, so it is matched segment by segment from the
///   start. A pattern shorter than the entry names a directory above it and
///   covers it; one longer names something below the file and covers nothing.
///
/// Blank lines, comments (`#...`) and negations (`!...`) cover nothing, and
/// need no test of their own to be turned away: `#` and `!` are ordinary
/// characters to [`ignores`], and no segment of a path turnout wrote begins
/// with one, so such a line simply matches nothing. Reading a negation as
/// "covered" would be exactly backwards - it un-ignores the file - and
/// falling through is already the safe answer.
fn covers(pattern: &str, entry: &str) -> bool {
    // A trailing slash means "a directory", which changes what the pattern
    // may match, not whether it matches this path; a leading one anchors it,
    // which it already is once it contains a slash of its own.
    let anchored = pattern.trim_end_matches('/').starts_with('/');
    let pattern = pattern.trim_end_matches('/').trim_start_matches('/');
    let entry_parts: Vec<&str> = entry.split('/').collect();
    if !pattern.contains('/') && !anchored {
        // At any depth: the file's own name, or a directory it lives under.
        return entry_parts.iter().any(|part| ignores(pattern, part));
    }
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    if pattern_parts.len() > entry_parts.len() {
        return false;
    }
    pattern_parts.iter().zip(&entry_parts).all(|(pattern, part)| ignores(pattern, part))
}

/// Whether one `.gitignore` path segment matches one segment of a path.
///
/// A literal, or a glob with `*` - the only wildcard that appears in an
/// ignore line for a dotenv file. Anything fancier (`?`, character classes,
/// `**`) is nobody's ignore line for one, and falling through means turnout
/// adds an entry that was already covered rather than skipping one that was
/// not - the harmless direction.
fn ignores(pattern: &str, file_name: &str) -> bool {
    if pattern == file_name {
        return true;
    }
    if !pattern.contains('*') {
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
/// `{gateway}` is the gateway URL, `{gateway_port}` its number, `{port}`
/// the app's own dev-server port. Each needs its port to exist; a command
/// that asks for one the app does not have is a configuration error and
/// says so, rather than running with the braces left in.
pub fn substitute(command_line: &str, app: &App) -> Result<String> {
    let mut out = command_line.to_string();
    if out.contains("{gateway}") || out.contains("{gateway_port}") {
        let Some(port) = app.gateway_port else {
            bail!(
                "the command uses {{gateway}} but app '{0}' has no gateway port - set one with `turnout app edit {0} --port PORT`",
                app.name
            );
        };
        out = out
            .replace("{gateway}", &format!("http://localhost:{port}"))
            .replace("{gateway_port}", &port.to_string());
    }
    if out.contains("{port}") {
        let Some(port) = app.dev_port else {
            bail!(
                "the command uses {{port}} but app '{0}' has no dev port yet - it is assigned by `turnout dev {0}`, or pinned with `turnout app edit {0} --dev-port PORT`",
                app.name
            );
        };
        out = out.replace("{port}", &port.to_string());
    }
    Ok(out)
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
            dev_port: None,
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
        // A bare pattern matches the file at any depth, which is how git
        // reads one - so the same line covers the app at the root and the app
        // two directories down.
        for entry in [".env.development.local", "apps/web/.env.development.local"] {
            for pattern in [".env.development.local", ".env*.local", "*.local", ".env.*", "*"] {
                assert!(covers(pattern, entry), "{pattern} should cover {entry}");
            }
            for pattern in [
                "",
                "# .env.development.local",
                "!.env.development.local",
                ".env",
                ".env.local",
                "*.log",
                ".env.production.local",
            ] {
                assert!(!covers(pattern, entry), "{pattern:?} should not cover {entry}");
            }
        }
        // A leading slash anchors the pattern at the ignore file's own
        // directory, so it covers the app beside it and not one two levels
        // down. Reading it as unanchored would skip an entry git still needs.
        assert!(covers("/.env.development.local", ".env.development.local"));
        assert!(!covers("/.env.development.local", "apps/web/.env.development.local"));
        // A blank line, a comment and a negation are not covers. Taking one
        // for a cover would make turnout skip an entry git needs - worst of
        // all for `!`, which un-ignores the very file being filed.
        for pattern in ["", "   ", "!*", "#*", "!*.local", "!.env*.local", "!apps/*/.env.development.local", "# *.local"] {
            assert!(!covers(pattern, "apps/web/.env.development.local"), "{pattern:?} is not a cover");
        }
    }

    /// A pattern with a slash is anchored, and read segment by segment - the
    /// case that made turnout append an entry its own repository already
    /// ignored through `examples/*/.env.development.local`.
    #[test]
    fn an_anchored_glob_path_is_recognised_as_covering_the_entry() {
        let entry = "examples/vue-demo/.env.development.local";
        for pattern in [
            "examples/vue-demo/.env.development.local",
            "/examples/vue-demo/.env.development.local",
            "examples/*/.env.development.local",
            "examples/*/.env*.local",
            "examples/vue-demo",
            "examples/vue-demo/",
            "examples",
        ] {
            assert!(covers(pattern, entry), "{pattern} should cover {entry}");
        }
        for pattern in [
            // A different app under the same parent.
            "examples/react-demo/.env.development.local",
            // A different parent.
            "docs/*/.env.development.local",
            // Longer than the entry: it names something below the file.
            "examples/vue-demo/.env.development.local/deeper",
            // Anchored one level too shallow - `examples` is not `vue-demo`.
            "vue-demo/.env.development.local",
        ] {
            assert!(!covers(pattern, entry), "{pattern} should not cover {entry}");
        }
    }

    #[test]
    fn placeholders_take_the_port_and_refuse_without_one() {
        let out = substitute("vite --api {gateway} --p {gateway_port}", &app(Some(7001))).unwrap();
        assert_eq!(out, "vite --api http://localhost:7001 --p 7001");
        assert_eq!(substitute("pnpm dev", &app(None)).unwrap(), "pnpm dev");
        let error = substitute("x {gateway}", &app(None)).unwrap_err().to_string();
        assert!(error.contains("has no gateway port"), "{error}");
        // The dev port is its own placeholder with its own complaint.
        let error = substitute("vite --port {port}", &app(Some(7001))).unwrap_err().to_string();
        assert!(error.contains("has no dev port yet"), "{error}");
        let mut with_dev = app(Some(7001));
        with_dev.dev_port = Some(5100);
        assert_eq!(
            substitute("vite --port {port} --api {gateway}", &with_dev).unwrap(),
            "vite --port 5100 --api http://localhost:7001"
        );
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

    /// The monorepo defect: with the `.git` levels above the app, the entry
    /// goes into the repository's `.gitignore` spelled as a path, not into a
    /// new file beside the app - and not nowhere, which is what used to happen.
    #[test]
    fn a_monorepo_app_is_ignored_from_the_repository_root() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join(".gitignore"), "node_modules\n").unwrap();
        let project = repo.path().join("apps").join("web");
        std::fs::create_dir_all(&project).unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(),
            "node_modules\napps/web/.env.development.local\n"
        );
        assert!(!project.join(".gitignore").exists(), "a second .gitignore appeared beside the app");
        // Writing twice must not append the entry twice.
        write(&app).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(),
            "node_modules\napps/web/.env.development.local\n"
        );
    }

    /// The nearest `.gitignore` wins over the root's, and its entry is
    /// relative to it - a package with its own ignores keeps them local.
    #[test]
    fn the_nearest_gitignore_inside_the_repository_takes_the_entry() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join(".gitignore"), "node_modules\n").unwrap();
        let package = repo.path().join("apps");
        let project = package.join("web");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(package.join(".gitignore"), "dist\n").unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(
            std::fs::read_to_string(package.join(".gitignore")).unwrap(),
            "dist\nweb/.env.development.local\n"
        );
        assert_eq!(std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(), "node_modules\n");
    }

    /// A repository with no `.gitignore` anywhere gets one at its root, where
    /// a repository's ignores belong - not beside the app.
    #[test]
    fn a_repository_without_any_gitignore_gets_one_at_its_root() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        let project = repo.path().join("apps").join("web");
        std::fs::create_dir_all(&project).unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(),
            "apps/web/.env.development.local\n"
        );
        assert!(!project.join(".gitignore").exists());
    }

    /// A glob in the repository root already covers the file: nothing is
    /// appended, whichever way the file is spelled from there.
    #[test]
    fn a_root_glob_already_covers_a_nested_app() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        std::fs::write(repo.path().join(".gitignore"), "*.local\n").unwrap();
        let project = repo.path().join("apps").join("web");
        std::fs::create_dir_all(&project).unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(), "*.local\n");
    }

    /// A `.git` *file* - a worktree or a submodule - is a repository root too.
    #[test]
    fn a_worktree_marker_file_counts_as_a_repository() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join(".git"), "gitdir: /elsewhere/.git/worktrees/w\n").unwrap();
        let project = repo.path().join("app");
        std::fs::create_dir_all(&project).unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(std::fs::read_to_string(repo.path().join(".gitignore")).unwrap(), "app/.env.development.local\n");
    }

    /// An ignore file the user keeps outside any repository is honoured where
    /// it stands - writing into a file that already exists is never litter,
    /// and it is the case the old behaviour covered.
    #[test]
    fn a_gitignore_without_a_repository_is_still_written_to() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "node_modules\n").unwrap();
        let mut app = app(Some(7001));
        app.path = dir.path().display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "node_modules\n.env.development.local\n"
        );
    }

    /// A `.gitignore` above the repository root is an outer repository's; the
    /// entry belongs to this one, at its own root.
    #[test]
    fn a_gitignore_outside_the_root_is_not_written_to() {
        let outer = tempfile::tempdir().unwrap();
        std::fs::write(outer.path().join(".gitignore"), "outer\n").unwrap();
        let repo = outer.path().join("inner");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        let project = repo.join("app");
        std::fs::create_dir_all(&project).unwrap();

        let mut app = app(Some(7001));
        app.path = project.display().to_string();
        app.gateway_env = Some("VITE_API_URL".into());
        write(&app).unwrap();

        assert_eq!(std::fs::read_to_string(outer.path().join(".gitignore")).unwrap(), "outer\n");
        assert_eq!(std::fs::read_to_string(repo.join(".gitignore")).unwrap(), "app/.env.development.local\n");
    }
}
