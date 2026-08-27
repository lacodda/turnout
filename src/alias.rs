//! The short `tn` alias, as a link rather than a second copy of the binary.
//!
//! A copy is the obvious way to give one program two names, and the wrong one.
//! It doubles the disk footprint of an install for no new code, and - worse -
//! it goes stale: `self-update` replaces the file it is running from, so the
//! copy keeps answering with the previous release under the same name. That is
//! how a user ends up with `tn --version` reporting an older version than
//! `turnout --version`, which reads as an inexplicable downgrade.
//!
//! A link has neither problem. One set of bytes on disk answers to both names,
//! so there is nothing to keep in sync:
//!
//! - **Unix:** a symlink. `install.sh` has always done this.
//! - **Windows:** a *hard* link. Symlinks there need elevation (or developer
//!   mode), which an installer has no business demanding; hard links do not,
//!   as long as both names live on the same volume - which they do, since the
//!   alias sits in the install directory beside the binary.
//!
//! The one seam is [`self_update`](crate::commands::self_update): replacing the
//! binary renames the old file aside and moves a new one in, which breaks the
//! link - the alias would keep pointing at the outgoing file. So an update
//! re-links afterwards, which is what [`refresh`] is for.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The alias name, without any platform extension.
pub const ALIAS: &str = "tn";

/// Where the alias sits for a binary at `exe`: same directory, same extension.
pub fn path_beside(exe: &Path) -> PathBuf {
    let mut alias = exe.with_file_name(ALIAS);
    if let Some(extension) = exe.extension() {
        alias.set_extension(extension);
    }
    alias
}

/// Point `alias` at `exe`, replacing whatever is already there.
///
/// Both paths must be on the same volume on Windows, which they are whenever
/// the alias is created beside the binary.
pub fn link(exe: &Path, alias: &Path) -> Result<()> {
    // A link cannot be created over an existing name, and on Windows the file
    // being replaced may be the previous alias - which is not running, so
    // removing it is allowed.
    let _ = std::fs::remove_file(alias);

    #[cfg(windows)]
    let result = std::fs::hard_link(exe, alias);
    // Relative, matching what `install.sh` writes: a symlink holding just the
    // file name survives the whole install directory being moved or renamed,
    // where one holding an absolute path would dangle.
    #[cfg(unix)]
    let result = {
        let target = exe.file_name().unwrap_or(exe.as_os_str());
        std::os::unix::fs::symlink(target, alias)
    };

    result.with_context(|| format!("cannot link {} to {}", alias.display(), exe.display()))
}

/// Re-point the alias at the binary after an update replaced it.
///
/// Best-effort by design: the alias is a convenience, and an install that
/// never had one must not grow one behind the user's back - `TURNOUT_NO_ALIAS`
/// is their choice to keep. So this only acts when an alias is already there,
/// and reports what it did for the caller to print.
///
/// An existing alias is relinked unconditionally rather than only when it has
/// gone stale. Telling the two apart means asking whether two paths are the
/// same file on disk, which Windows has no stable std API for; relinking is a
/// directory operation either way, so the check would cost more code than it
/// saves work - and code that can be wrong about whether a fix is needed.
pub fn refresh(exe: &Path) -> Outcome {
    let alias = path_beside(exe);
    if !alias.exists() {
        return Outcome::Absent;
    }
    match link(exe, &alias) {
        Ok(()) => Outcome::Relinked(alias),
        Err(err) => Outcome::Failed(alias, err.to_string()),
    }
}

/// What [`refresh`] found and did.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// No alias is installed - nothing to refresh.
    Absent,
    /// The alias was re-pointed at the new binary.
    Relinked(PathBuf),
    /// The alias is installed but could not be re-pointed; it is stale and the
    /// user has to be told, since it still answers under its own name.
    Failed(PathBuf, String),
}

impl Outcome {
    /// The line to print after an update, if any.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Absent => None,
            Self::Relinked(alias) => Some(format!("Alias {} updated too.", alias.display())),
            Self::Failed(alias, err) => Some(format!(
                "Warning: {} still points at the previous version and could not be relinked ({err}).\n\
                 Re-run the installer to fix it.",
                alias.display()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows paths are only paths on Windows: elsewhere `C:\dir\turnout.exe`
    /// is one filename with backslashes in it, so the two cases have to be
    /// asserted on the platform that parses them.
    #[test]
    #[cfg(windows)]
    fn the_alias_sits_beside_the_binary_with_the_same_extension() {
        let exe = PathBuf::from(r"C:\Users\dev\AppData\Local\Programs\turnout\turnout.exe");
        assert_eq!(path_beside(&exe), PathBuf::from(r"C:\Users\dev\AppData\Local\Programs\turnout\tn.exe"));
    }

    #[test]
    #[cfg(unix)]
    fn the_alias_sits_beside_the_binary() {
        let exe = PathBuf::from("/home/dev/.local/bin/turnout");
        assert_eq!(path_beside(&exe), PathBuf::from("/home/dev/.local/bin/tn"));
    }

    /// Whatever the platform, the alias is a sibling of the binary and keeps
    /// its extension - that is what makes it answer as a command.
    #[test]
    fn the_alias_is_a_sibling_that_keeps_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join(if cfg!(windows) { "turnout.exe" } else { "turnout" });
        let alias = path_beside(&exe);

        assert_eq!(alias.parent(), exe.parent());
        assert_eq!(alias.extension(), exe.extension());
        assert_eq!(alias.file_stem().unwrap(), ALIAS);
    }

    /// The whole point of the link: one set of bytes answers to both names, so
    /// replacing the content through one name shows through the other.
    #[test]
    fn a_linked_alias_shares_the_binary_it_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"version one").unwrap();
        let alias = path_beside(&exe);

        link(&exe, &alias).unwrap();

        assert_eq!(std::fs::read(&alias).unwrap(), b"version one");
        // Identity, observed rather than asked about: writing through one name
        // shows through the other only if there is one file behind both. A
        // copy would still read "version one" here.
        std::fs::write(&exe, b"version two").unwrap();
        assert_eq!(
            std::fs::read(&alias).unwrap(),
            b"version two",
            "the alias must be the same file as the binary, not a copy of it"
        );
    }

    /// An install without an alias must not grow one: `TURNOUT_NO_ALIAS` is a
    /// choice the user made, and an update is no place to overrule it.
    #[test]
    fn refresh_leaves_an_aliasless_install_alone() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"binary").unwrap();

        assert_eq!(refresh(&exe), Outcome::Absent);
        assert!(!path_beside(&exe).exists());
    }

    /// The regression this whole module exists for: after an update swapped the
    /// binary, an alias that is still a stale copy has to be re-pointed.
    #[test]
    fn refresh_relinks_an_alias_left_behind_by_an_update() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        let alias = path_beside(&exe);
        // What an installer before this change produced: a plain copy.
        std::fs::write(&exe, b"old version").unwrap();
        std::fs::copy(&exe, &alias).unwrap();
        // What self-update then did to the binary, leaving the copy behind.
        std::fs::write(&exe, b"new version").unwrap();
        assert_eq!(std::fs::read(&alias).unwrap(), b"old version");

        assert!(matches!(refresh(&exe), Outcome::Relinked(_)));

        assert_eq!(std::fs::read(&alias).unwrap(), b"new version");
        // And it is a link now, so the next update needs no rescue.
        std::fs::write(&exe, b"newer still").unwrap();
        assert_eq!(std::fs::read(&alias).unwrap(), b"newer still");
    }

    /// Refreshing an already-healthy alias is a no-op, not a breakage: the
    /// relink is unconditional, so it runs on every update.
    #[test]
    fn refreshing_a_healthy_alias_leaves_it_working() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"binary").unwrap();
        let alias = path_beside(&exe);
        link(&exe, &alias).unwrap();

        assert!(matches!(refresh(&exe), Outcome::Relinked(_)));

        std::fs::write(&exe, b"next release").unwrap();
        assert_eq!(std::fs::read(&alias).unwrap(), b"next release");
    }

    /// A stale alias must never fail quietly: it keeps answering to its own
    /// name, so the user has to hear about it.
    #[test]
    fn a_failed_relink_names_the_alias_and_a_way_out() {
        let outcome = Outcome::Failed(PathBuf::from("/home/dev/.local/bin/tn"), "permission denied".to_string());
        let message = outcome.message().expect("a failure has to be reported");
        assert!(message.contains("/home/dev/.local/bin/tn"));
        assert!(message.contains("previous version"));
        assert!(message.contains("installer"));
    }

    #[test]
    fn a_successful_relink_says_so() {
        let message = Outcome::Relinked(PathBuf::from("/home/dev/.local/bin/tn")).message().unwrap();
        assert!(message.contains("/home/dev/.local/bin/tn"));
    }
}
