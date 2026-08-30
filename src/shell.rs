//! What language the far side of an SSH connection speaks.
//!
//! Every remote command turnout runs used to be a POSIX `sh` string built with
//! `format!` at the call site. That works until the server answers SSH with
//! `cmd.exe`, which is the default on Windows with OpenSSH: single quotes stop
//! being quotes and become part of the filename, `mkdir -p` is a syntax error,
//! and a probe like `command -v tar >/dev/null 2>&1` can never succeed - not
//! even when `tar.exe` is sitting in System32, which it has been since Windows
//! 10 1803. A field report showed exactly that: turnout reported "no usable tar
//! on the server" about a server that had tar all along. It had not failed to
//! find it; it had failed to *ask*.
//!
//! So the dialect is decided once per session and every command is built
//! through it. Two things follow from that, both deliberate:
//!
//! 1. **The probe has to be dialect-neutral.** It runs before we know the
//!    answer, so it cannot use syntax that only one side understands. See
//!    [`PROBE`].
//! 2. **Command strings are pure functions of the dialect.** They take values
//!    and return a string, with no live [`crate::ssh::Session`] anywhere near
//!    them. That is what makes the Windows half testable from a Linux CI box and
//!    from a developer machine that cannot reach a Windows server at all.

use std::fmt;

/// How to talk to a server, decided once and then reused.
///
/// The Windows arm is deliberately *not* "PowerShell": OpenSSH on Windows
/// answers with `cmd.exe` unless the administrator changed `DefaultShell`, and
/// assuming the friendlier shell is how you end up with commands that work on
/// the maintainer's machine and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dialect {
    /// `sh`-compatible: Linux, macOS, BSD, and Windows servers whose SSH shell
    /// has been pointed at bash.
    #[default]
    Posix,
    /// `cmd.exe` on Windows.
    Windows,
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dialect::Posix => "posix",
            Dialect::Windows => "windows",
        })
    }
}

/// A single command that both shells can run, whose *output* names the dialect.
///
/// This is the one string in the codebase that cannot be built through a
/// dialect, because it is what decides which dialect to use. It works by
/// exploiting the one thing the two shells disagree about most usefully:
/// variable syntax.
///
/// - `cmd.exe` expands `%COMSPEC%` (always set, always a path ending in
///   `cmd.exe`) and leaves `$SHELL` alone as literal text.
/// - A POSIX shell expands neither `%COMSPEC%` nor `%OS%`, and passes them
///   through unchanged as ordinary words.
///
/// `echo` exists in both and quotes nothing away, so the reply is unambiguous:
/// a line mentioning `cmd.exe` can only have come from `cmd.exe`.
pub const PROBE: &str = "echo %COMSPEC%";

/// Read the dialect out of a [`PROBE`] reply.
///
/// A POSIX shell echoes the literal `%COMSPEC%` back, because `%` means nothing
/// to it. `cmd.exe` substitutes the path to itself. Anything unrecognizable is
/// treated as POSIX: that is what every server was assumed to be before this
/// existed, so an odd reply degrades to the old behavior rather than to a new
/// failure.
pub fn read_probe(reply: &str) -> Dialect {
    let reply = reply.trim().to_ascii_lowercase();
    if reply.contains("cmd.exe") { Dialect::Windows } else { Dialect::Posix }
}

impl Dialect {
    /// Quote one value so the shell treats it as a single literal word.
    ///
    /// POSIX gets single quotes, where the only special character left is the
    /// quote itself. `cmd.exe` has no such escape: it uses double quotes, and a
    /// value containing a double quote cannot be expressed at all - hence
    /// [`Dialect::reject_unquotable`], which is called before any value reaches
    /// a command.
    pub fn quote(&self, value: &str) -> String {
        match self {
            Dialect::Posix => format!("'{}'", value.replace('\'', "'\\''")),
            Dialect::Windows => format!("\"{value}\""),
        }
    }

    /// Whether this value can be expressed as a literal in this dialect.
    ///
    /// `cmd.exe` quoting has no escape for `"` and no way to carry `%`
    /// without risking expansion, so a path containing either is refused up
    /// front instead of being silently mangled into a different path. POSIX can
    /// express anything.
    pub fn reject_unquotable(&self, value: &str) -> Result<(), String> {
        match self {
            Dialect::Posix => Ok(()),
            Dialect::Windows if value.contains('"') => Err(format!("'{value}' contains a double quote, which cmd.exe cannot quote")),
            Dialect::Windows if value.contains('%') => Err(format!("'{value}' contains a percent sign, which cmd.exe would expand as a variable")),
            Dialect::Windows => Ok(()),
        }
    }

    /// Create a directory, parents included, succeeding if it already exists.
    ///
    /// `mkdir -p` has no `cmd.exe` equivalent: bare `mkdir` creates parents
    /// anyway but fails when the directory exists, so the existence check comes
    /// first and the whole thing is a conditional.
    pub fn mkdir_p(&self, dir: &str) -> String {
        let quoted = self.quote(dir);
        match self {
            Dialect::Posix => format!("mkdir -p {quoted}"),
            Dialect::Windows => format!("if not exist {quoted} mkdir {quoted}"),
        }
    }

    /// Remove everything *inside* a directory, leaving the directory itself.
    ///
    /// The directory has to survive: it is the deploy target, someone may own
    /// it or have granted rights on it, and re-creating it can silently change
    /// both. On POSIX that means `find -mindepth 1`; on Windows, the pair of
    /// `del` and `rd` that between them cover files and subdirectories.
    ///
    /// Both arms tolerate an empty directory rather than reporting failure.
    pub fn clear_dir(&self, dir: &str) -> String {
        let quoted = self.quote(dir);
        match self {
            Dialect::Posix => format!("find {quoted} -mindepth 1 -maxdepth 1 -exec rm -rf {{}} +"),
            // `del /q /s` empties the files, `for /d ... rd /s /q` the
            // subdirectories. The trailing `exit /b 0` keeps "nothing to
            // delete" from surfacing as a failed deploy step.
            Dialect::Windows => format!("del /f /q /s {quoted}\\* >nul 2>&1 & for /d %i in ({quoted}\\*) do @rd /s /q \"%i\" & exit /b 0"),
        }
    }

    /// Delete a single file, saying nothing if it was not there.
    pub fn remove_file(&self, path: &str) -> String {
        let quoted = self.quote(path);
        match self {
            Dialect::Posix => format!("rm -f {quoted}"),
            Dialect::Windows => format!("del /f /q {quoted} >nul 2>&1 & exit /b 0"),
        }
    }

    /// Unpack a gzipped tar into a directory.
    ///
    /// Both arms call `tar`. Windows has shipped bsdtar as `tar.exe` since
    /// Windows 10 1803, and it reads `.tar.gz` - the field report confirmed
    /// bsdtar 3.5.2 on the very server that was told it had "no usable tar".
    /// This is why the fix here is a dialect and not a second archive format:
    /// the tool was always there, only the question was malformed.
    pub fn untar(&self, archive: &str, into: &str) -> String {
        format!("tar xzf {} -C {}", self.quote(archive), self.quote(into))
    }

    /// Pack a directory's contents into `archive`, which must not be inside it.
    pub fn tar_czf(&self, archive: &str, from_dir: &str) -> String {
        format!("tar czf {} -C {} .", self.quote(archive), self.quote(from_dir))
    }

    /// List a directory's entries, one per line, empty when it does not exist.
    ///
    /// "Missing directory is not an error" is the contract: it just means no
    /// backups have been taken yet, which is a normal state and not something
    /// to fail a command over.
    pub fn list_dir(&self, dir: &str) -> String {
        let quoted = self.quote(dir);
        match self {
            Dialect::Posix => format!("ls -1 {quoted} 2>/dev/null || true"),
            // `/b` is bare format - names only, no header or byte totals.
            Dialect::Windows => format!("dir /b {quoted} 2>nul & exit /b 0"),
        }
    }

    /// Whether `path` is an existing file; the command succeeds if it is.
    pub fn file_exists(&self, path: &str) -> String {
        let quoted = self.quote(path);
        match self {
            Dialect::Posix => format!("test -f {quoted}"),
            Dialect::Windows => format!("if not exist {quoted} exit /b 1"),
        }
    }

    /// Join two commands so the second runs only if the first succeeded.
    ///
    /// `&&` means the same thing in both shells; the method exists so callers
    /// never hand-assemble command strings, which is the habit that let the
    /// POSIX assumption spread in the first place.
    pub fn and_then(&self, first: &str, second: &str) -> String {
        format!("{first} && {second}")
    }

    /// Run `command` with `dir` as the working directory.
    ///
    /// A path's post-write command is defined as "what to do after writing to
    /// this directory", so it runs *in* that directory - up to v0.10 it ran
    /// wherever the SSH session landed, which is the home directory, and every
    /// path had to open with a `cd` of its own to compensate.
    ///
    /// `cd` on Windows does not change drive unless told to: `cd C:\\site` from
    /// `D:` prints the path and stays put, so a compose file on another drive
    /// would never be found. `/d` is what makes it move.
    pub fn run_in(&self, dir: &str, command: &str) -> String {
        let quoted = self.quote(dir);
        match self {
            Dialect::Posix => format!("cd {quoted} && {command}"),
            Dialect::Windows => format!("cd /d {quoted} && {command}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Dialect, PROBE, read_probe};

    /// A path's post-write command runs in the deploy directory, which is what
    /// "after writing here" always meant. The Windows form needs `/d` or it
    /// silently stays on the current drive.
    #[test]
    fn a_command_runs_in_the_directory_it_belongs_to() {
        assert_eq!(
            Dialect::Posix.run_in("/srv/app", "docker compose up -d"),
            "cd '/srv/app' && docker compose up -d"
        );
        let windows = Dialect::Windows.run_in("C:\\site", "docker compose up -d");
        assert_eq!(windows, "cd /d \"C:\\site\" && docker compose up -d");
        assert!(windows.contains("/d"), "without /d cmd.exe does not change drive: {windows}");
    }

    /// A directory with a space in it is one argument, or the command runs
    /// somewhere else entirely.
    #[test]
    fn a_directory_with_a_space_stays_one_argument() {
        assert_eq!(Dialect::Posix.run_in("/srv/my app", "ls"), "cd '/srv/my app' && ls");
        assert_eq!(Dialect::Windows.run_in("C:\\my site", "dir"), "cd /d \"C:\\my site\" && dir");
    }

    /// The probe has to survive both shells, so it may not contain syntax that
    /// one of them would choke on: no quotes, no redirection, no operators.
    #[test]
    fn the_probe_is_neutral() {
        assert!(!PROBE.contains('\''), "single quotes are literal in cmd.exe: {PROBE}");
        assert!(!PROBE.contains('>'), "redirection differs between the shells: {PROBE}");
        assert!(!PROBE.contains("&&"), "keep the probe to a single command: {PROBE}");
    }

    /// The exact replies the two shells give. A POSIX shell has no `%`
    /// expansion, so it echoes the variable name back verbatim.
    #[test]
    fn reads_the_replies_the_shells_actually_give() {
        assert_eq!(read_probe("C:\\Windows\\system32\\cmd.exe"), Dialect::Windows);
        assert_eq!(read_probe("%COMSPEC%"), Dialect::Posix);
    }

    /// Case varies with how the variable was set; the reply is a path, and
    /// Windows paths are not case-sensitive.
    #[test]
    fn recognizes_cmd_whatever_the_case() {
        assert_eq!(read_probe("C:\\WINDOWS\\SYSTEM32\\CMD.EXE"), Dialect::Windows);
    }

    /// An unreadable reply must land on the behavior that predates this module,
    /// not on a new kind of failure.
    #[test]
    fn an_unrecognizable_reply_stays_posix() {
        assert_eq!(read_probe(""), Dialect::Posix);
        assert_eq!(read_probe("some login banner"), Dialect::Posix);
    }

    #[test]
    fn quotes_per_dialect() {
        assert_eq!(Dialect::Posix.quote("/var/www/my app"), "'/var/www/my app'");
        assert_eq!(Dialect::Posix.quote("it's"), "'it'\\''s'");
        assert_eq!(Dialect::Windows.quote("C:\\inetpub\\my site"), "\"C:\\inetpub\\my site\"");
    }

    /// cmd.exe cannot express these, so they are refused rather than mangled
    /// into a path that means something else.
    #[test]
    fn windows_refuses_what_it_cannot_quote() {
        assert!(Dialect::Windows.reject_unquotable("C:\\ok\\path").is_ok());
        assert!(Dialect::Windows.reject_unquotable("C:\\say \"hi\"").is_err());
        assert!(Dialect::Windows.reject_unquotable("C:\\%TEMP%\\x").is_err());
        // POSIX quoting has an escape for every byte.
        assert!(Dialect::Posix.reject_unquotable("it's \"quoted\" 100%").is_ok());
    }

    /// The deploy directory itself must survive a clear: it may carry ownership
    /// or ACLs that re-creating it would quietly drop.
    #[test]
    fn clearing_keeps_the_directory_itself() {
        let posix = Dialect::Posix.clear_dir("/var/www/site");
        assert!(posix.contains("-mindepth 1"), "{posix}");
        assert!(!posix.contains("rm -rf '/var/www/site'"), "the directory itself must not be removed: {posix}");

        let windows = Dialect::Windows.clear_dir("C:\\site");
        assert!(windows.contains("\\*"), "only the contents are targeted: {windows}");
        assert!(
            !windows.contains("rd /s /q \"C:\\site\""),
            "the directory itself must not be removed: {windows}"
        );
    }

    /// An empty or missing directory is a normal state, not a failed step.
    #[test]
    fn listing_and_clearing_tolerate_nothing_there() {
        assert!(Dialect::Posix.list_dir("/backups").contains("|| true"));
        assert!(Dialect::Windows.list_dir("C:\\backups").contains("exit /b 0"));
        assert!(Dialect::Windows.clear_dir("C:\\site").contains("exit /b 0"));
        assert!(Dialect::Windows.remove_file("C:\\x.tar.gz").contains("exit /b 0"));
    }

    /// Windows has had a working tar since 1803; the bug was the question, not
    /// the tool, so both dialects call the same program.
    #[test]
    fn both_dialects_use_tar() {
        assert!(Dialect::Posix.untar("/tmp/a.tar.gz", "/var/www").starts_with("tar xzf "));
        assert!(Dialect::Windows.untar("C:\\a.tar.gz", "C:\\site").starts_with("tar xzf "));
        assert_eq!(Dialect::Windows.untar("C:\\a.tar.gz", "C:\\site"), "tar xzf \"C:\\a.tar.gz\" -C \"C:\\site\"");
    }

    /// Every value that reaches a command goes through the dialect's quoting -
    /// a path with a space must not split into two arguments.
    #[test]
    fn paths_with_spaces_stay_one_argument() {
        assert!(Dialect::Windows.mkdir_p("C:\\my site").contains("\"C:\\my site\""));
        assert!(Dialect::Posix.mkdir_p("/var/my site").contains("'/var/my site'"));
        assert!(Dialect::Windows.file_exists("C:\\my site\\a.txt").contains("\"C:\\my site\\a.txt\""));
    }

    /// `mkdir -p` succeeds on an existing directory; the cmd.exe arm has to
    /// match that, since a first deploy and a repeat deploy both call it.
    #[test]
    fn mkdir_tolerates_an_existing_directory() {
        assert!(Dialect::Posix.mkdir_p("/var/www/site").starts_with("mkdir -p"));
        assert!(Dialect::Windows.mkdir_p("C:\\site").starts_with("if not exist"));
    }
}
