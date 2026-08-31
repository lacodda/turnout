//! Replace the running binary with the latest release.
//!
//! Only installations that came from a release archive are updated in place.
//! When turnout was installed by cargo, npm or a system package manager, that
//! tool keeps its own record of what is installed; overwriting the file behind
//! its back would leave it convinced the old version is still there. Those
//! cases print the command that actually updates them instead - see
//! [`Installation`].
//!
//! Replacing a *running* executable is the other half of the problem. Windows
//! locks it and refuses the write, so the old file is renamed aside first: a
//! rename is allowed while the image is mapped, and the leftover is swept on
//! the next run. Unix allows the unlink outright, but the same dance costs
//! nothing there and keeps one code path.
//!
//! That swap also breaks the `tn` alias, which is a link to the binary rather
//! than a copy of it (see [`crate::alias`]): the link follows the file that
//! was renamed aside. So the alias is re-pointed once the new binary is in
//! place.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::progress::Step;
use crate::update;

/// Suffix given to the outgoing binary before the new one takes its place.
const BACKUP_SUFFIX: &str = ".old";

pub fn run(assume_yes: bool, force: bool) -> Result<()> {
    let exe = std::env::current_exe().context("cannot locate the running turnout binary")?;
    let exe = dunce::canonicalize(&exe).unwrap_or(exe);

    if !force && let Some(installation) = Installation::detect(&exe) {
        println!("{}", installation.explain());
        return Ok(());
    }

    let step = Step::start("Looking up the latest release ...".to_string());
    let latest = update::fetch_latest_version()?;
    let current = env!("CARGO_PKG_VERSION");
    step.clear();

    if !update::is_version_newer(&latest, current) {
        println!("turnout {current} is already the latest release.");
        return Ok(());
    }

    println!("turnout {latest} is available (you have {current}).");
    println!("It will replace {}", exe.display());
    if !crate::pick::confirm_destructive("Update now?".to_string(), assume_yes)? {
        println!("Nothing was changed.");
        return Ok(());
    }

    let asset = Asset::for_this_platform(&latest)?;
    let step = Step::start(format!("Downloading {} ...", asset.file_name()));
    let archive = asset.download()?;
    step.update("Unpacking ...".to_string());
    let binary = asset.extract_binary(&archive)?;
    step.update("Replacing the binary ...".to_string());
    let alias = install_binary(&exe, &binary)?;
    step.done(format!("Updated to turnout {latest}"));
    if let Some(line) = alias.message() {
        println!("{line}");
    }

    crate::journal::record("self-update", None, None, Some(&format!("{current} -> {latest}")));
    println!("Run `turnout --version` in a new shell to confirm.");
    Ok(())
}

/// How turnout got onto this machine, when that is something other than a
/// release archive. Each variant owns a tool that tracks installed versions.
enum Installation {
    Cargo,
    Npm,
    SystemPackage(PathBuf),
}

impl Installation {
    /// Guess from the path of the running binary. Only confident matches count:
    /// an unrecognized location is assumed to be a release archive, which is
    /// what the installers produce and what `--force` also targets.
    fn detect(exe: &Path) -> Option<Self> {
        let text = exe.to_string_lossy().replace('\\', "/");
        // `~/.cargo/bin/turnout`, or wherever CARGO_HOME points.
        if text.contains("/.cargo/bin/") || cargo_home_bin().is_some_and(|dir| exe.starts_with(dir)) {
            return Some(Self::Cargo);
        }
        if text.contains("/node_modules/") || text.contains("/npm/") {
            return Some(Self::Npm);
        }
        // System-wide locations are owned by a package manager, and writing
        // there needs privileges we should not be asking for.
        for prefix in ["/usr/bin/", "/usr/local/bin/", "/opt/", "/bin/"] {
            if text.starts_with(prefix) {
                return Some(Self::SystemPackage(exe.to_path_buf()));
            }
        }
        None
    }

    fn explain(&self) -> String {
        match self {
            Self::Cargo => "turnout was installed with cargo, which keeps its own record of what is installed.\n\n\
                 Update it with:\n  cargo install turnout --force\n\n\
                 (`--force` is what lets cargo overwrite the existing binary.)"
                .to_string(),
            Self::Npm => "turnout was installed from npm, which keeps its own record of what is installed.\n\n\
                 Update it with:\n  npm install -g turnout-cli@latest"
                .to_string(),
            Self::SystemPackage(path) => format!(
                "turnout lives in {}, which belongs to your system package manager.\n\n\
                 Update it the way you installed it, or install a private copy with:\n  \
                 cargo install turnout\n\n\
                 Use `turnout self-update --force` to replace the file anyway.",
                path.display()
            ),
        }
    }
}

fn cargo_home_bin() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME").map(|home| PathBuf::from(home).join("bin"))
}

/// The release asset for the platform this binary was built for, and how to
/// get the executable out of it.
struct Asset {
    tag: String,
    /// Rust target triple, matching what the release workflow packages.
    target: &'static str,
    kind: ArchiveKind,
}

#[derive(Clone, Copy, PartialEq)]
enum ArchiveKind {
    Zip,
    TarGz,
}

impl Asset {
    fn for_this_platform(version: &str) -> Result<Self> {
        // Mirrors the matrix in .github/workflows/release.yml; anything else
        // has no prebuilt binary to download.
        let (target, kind) = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => ("x86_64-pc-windows-msvc", ArchiveKind::Zip),
            ("linux", "x86_64") => ("x86_64-unknown-linux-gnu", ArchiveKind::TarGz),
            ("macos", "aarch64") => ("aarch64-apple-darwin", ArchiveKind::TarGz),
            (os, arch) => bail!("no prebuilt turnout binary for {os}/{arch} - build it yourself with `cargo install turnout --force`"),
        };
        Ok(Self {
            tag: format!("v{version}"),
            target,
            kind,
        })
    }

    /// `turnout-v0.5.0-x86_64-pc-windows-msvc` - both the archive stem and the
    /// directory inside it, as the release workflow packages them.
    fn stem(&self) -> String {
        format!("turnout-{}-{}", self.tag, self.target)
    }

    fn file_name(&self) -> String {
        match self.kind {
            ArchiveKind::Zip => format!("{}.zip", self.stem()),
            ArchiveKind::TarGz => format!("{}.tar.gz", self.stem()),
        }
    }

    fn url(&self) -> String {
        format!("https://github.com/lacodda/turnout/releases/download/{}/{}", self.tag, self.file_name())
    }

    fn download(&self) -> Result<Vec<u8>> {
        let url = self.url();
        crate::utils::run_blocking(async {
            let client = reqwest::Client::builder()
                .user_agent(concat!("turnout/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(300))
                .build()?;
            let response = client.get(&url).send().await.with_context(|| format!("cannot download {url}"))?;
            if !response.status().is_success() {
                bail!("{url} returned {}", response.status());
            }
            Ok(response.bytes().await.context("the download was interrupted")?.to_vec())
        })
    }

    /// Pull the `turnout` executable out of the archive into a temporary file
    /// next to where it will land - a rename across filesystems would fail.
    fn extract_binary(&self, archive: &[u8]) -> Result<PathBuf> {
        let wanted = if cfg!(windows) { "turnout.exe" } else { "turnout" };
        let bytes = match self.kind {
            ArchiveKind::Zip => extract_from_zip(archive, wanted)?,
            ArchiveKind::TarGz => extract_from_tar_gz(archive, wanted)?,
        };
        if bytes.is_empty() {
            bail!("the downloaded archive contained an empty {wanted}");
        }
        let path = std::env::temp_dir().join(format!("{}-{wanted}", self.stem()));
        std::fs::write(&path, &bytes).with_context(|| format!("cannot write {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).with_context(|| format!("cannot make {} executable", path.display()))?;
        }
        Ok(path)
    }
}

fn extract_from_zip(archive: &[u8], wanted: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive)).context("the download is not a valid zip archive")?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        if entry.is_file() && entry.name().rsplit('/').next() == Some(wanted) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).context("cannot read the binary out of the archive")?;
            return Ok(bytes);
        }
    }
    bail!("no {wanted} inside the downloaded archive")
}

fn extract_from_tar_gz(archive: &[u8], wanted: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().context("the download is not a valid tar.gz archive")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.file_name().and_then(|n| n.to_str()) == Some(wanted) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).context("cannot read the binary out of the archive")?;
            return Ok(bytes);
        }
    }
    bail!("no {wanted} inside the downloaded archive")
}

/// Put the downloaded binary in place and leave the `tn` alias pointing at it.
///
/// The two belong together: the swap renames the outgoing file aside, which
/// breaks the link the alias is, so an install that stops after the swap
/// leaves `tn` answering with the previous release under the same name. They
/// are one function so that neither can be done without the other - the field
/// report this fixes was exactly that half-update.
fn install_binary(exe: &Path, replacement: &Path) -> Result<crate::alias::Outcome> {
    replace_running_binary(exe, replacement)?;
    Ok(crate::alias::refresh(exe))
}

/// Swap `replacement` in for `exe`, moving the running image aside first.
///
/// The old binary is only removed on a later run: on Windows it is still
/// mapped and cannot be deleted, and on Unix keeping it costs one stale file
/// until the next command sweeps it.
fn replace_running_binary(exe: &Path, replacement: &Path) -> Result<()> {
    let backup = backup_path(exe);
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(exe, &backup).with_context(|| format!("cannot move the running binary aside to {} - is turnout running elsewhere?", backup.display()))?;

    // From here on a failure would leave no binary at all, so put the old one
    // back rather than exiting with the path empty.
    if let Err(err) = std::fs::rename(replacement, exe).or_else(|_| std::fs::copy(replacement, exe).map(|_| ())) {
        let _ = std::fs::rename(&backup, exe);
        return Err(err).with_context(|| format!("cannot install the new binary at {}", exe.display()));
    }
    let _ = std::fs::remove_file(replacement);
    Ok(())
}

fn backup_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Delete the previous binary left behind by an earlier update.
///
/// Called from `main` on every command rather than from `self-update` alone:
/// after an update the user has no reason to run `self-update` again, and the
/// leftover is the size of a whole binary. Best-effort - if it is somehow
/// still locked, the next command tries again.
pub fn sweep_backup() {
    if let Ok(exe) = std::env::current_exe() {
        sweep_backups_beside(&exe);
    }
}

/// The leftovers to remove for a binary at `exe`.
///
/// Two of them, not one: an update runs under whichever name the user typed
/// and leaves a `.old` beside *that* name, so a machine where both names have
/// been used to update carries two. Each is the size of a whole binary, and
/// nothing will ever come back for either.
///
/// The second one is the counterpart rather than "the alias": asking for the
/// alias while running as the alias names the file itself, and `turnout.old`
/// would then never be collected.
fn sweep_backups_beside(exe: &Path) {
    let _ = std::fs::remove_file(backup_path(exe));
    if let Some(other) = crate::alias::counterpart(exe) {
        let _ = std::fs::remove_file(backup_path(&other));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_installs_are_recognized() {
        let exe = PathBuf::from("/home/dev/.cargo/bin/turnout");
        assert!(matches!(Installation::detect(&exe), Some(Installation::Cargo)));
        let windows = PathBuf::from(r"C:\Users\dev\.cargo\bin\turnout.exe");
        assert!(matches!(Installation::detect(&windows), Some(Installation::Cargo)));
    }

    #[test]
    fn npm_and_system_installs_are_recognized() {
        let npm = PathBuf::from("/usr/lib/node_modules/turnout-cli/turnout");
        assert!(matches!(Installation::detect(&npm), Some(Installation::Npm)));
        let system = PathBuf::from("/usr/bin/turnout");
        assert!(matches!(Installation::detect(&system), Some(Installation::SystemPackage(_))));
    }

    /// Where the installers put it, and anywhere unfamiliar: those are the
    /// cases self-update handles itself.
    #[test]
    fn archive_installs_are_updated_in_place() {
        let windows = PathBuf::from(r"C:\Users\dev\AppData\Local\Programs\turnout\turnout.exe");
        assert!(Installation::detect(&windows).is_none());
        let unix = PathBuf::from("/home/dev/.local/bin/turnout");
        assert!(Installation::detect(&unix).is_none());
    }

    /// Every hint has to name a command the user can actually run.
    #[test]
    fn every_explanation_names_a_command() {
        assert!(Installation::Cargo.explain().contains("cargo install turnout --force"));
        assert!(Installation::Npm.explain().contains("npm install -g turnout-cli"));
        assert!(
            Installation::SystemPackage(PathBuf::from("/usr/bin/turnout"))
                .explain()
                .contains("/usr/bin/turnout")
        );
    }

    /// The asset name must match what the release workflow uploads, or every
    /// update would 404.
    #[test]
    fn asset_names_match_the_release_workflow() {
        let asset = Asset {
            tag: "v0.5.0".to_string(),
            target: "x86_64-pc-windows-msvc",
            kind: ArchiveKind::Zip,
        };
        assert_eq!(asset.file_name(), "turnout-v0.5.0-x86_64-pc-windows-msvc.zip");
        assert!(asset.url().ends_with("/download/v0.5.0/turnout-v0.5.0-x86_64-pc-windows-msvc.zip"));

        let unix = Asset {
            tag: "v0.5.0".to_string(),
            target: "x86_64-unknown-linux-gnu",
            kind: ArchiveKind::TarGz,
        };
        assert_eq!(unix.file_name(), "turnout-v0.5.0-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn this_platform_has_a_known_asset() {
        // The host running the tests is one of the three released targets.
        let asset = Asset::for_this_platform("0.5.0").expect("a supported platform");
        assert!(asset.file_name().starts_with("turnout-v0.5.0-"));
    }

    #[test]
    fn the_backup_sits_beside_the_binary() {
        let exe = PathBuf::from("/home/dev/.local/bin/turnout");
        assert_eq!(backup_path(&exe), PathBuf::from("/home/dev/.local/bin/turnout.old"));
    }

    /// A zip laid out like the release archive: binary inside a named folder.
    #[test]
    fn the_binary_is_found_inside_a_zip_folder() {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file("turnout-v0.5.0-x86_64-pc-windows-msvc/README.md", options).unwrap();
            std::io::Write::write_all(&mut writer, b"not the binary").unwrap();
            writer.start_file("turnout-v0.5.0-x86_64-pc-windows-msvc/turnout.exe", options).unwrap();
            std::io::Write::write_all(&mut writer, b"MZ fake binary").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(extract_from_zip(&buffer, "turnout.exe").unwrap(), b"MZ fake binary");
        assert!(extract_from_zip(&buffer, "turnout").is_err());
    }

    #[test]
    fn the_binary_is_found_inside_a_tar_gz_folder() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let payload = b"ELF fake binary";
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "turnout-v0.5.0-x86_64-unknown-linux-gnu/turnout", &payload[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            use std::io::Write;
            let mut encoder = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            encoder.write_all(&tar_bytes).unwrap();
            encoder.finish().unwrap();
        }
        assert_eq!(extract_from_tar_gz(&gz, "turnout").unwrap(), b"ELF fake binary");
        assert!(extract_from_tar_gz(&gz, "turnout.exe").is_err());
    }

    /// Corrupt input must fail cleanly rather than panic on a running binary.
    #[test]
    fn a_corrupt_archive_is_an_error() {
        assert!(extract_from_zip(b"not a zip at all", "turnout.exe").is_err());
        assert!(extract_from_tar_gz(b"not a gzip at all", "turnout").is_err());
    }

    /// The field report of 19.08, end to end: `turnout` updated but `tn` kept
    /// answering with the previous release, because the swap breaks the link
    /// the alias is and nothing put it back.
    #[test]
    fn an_update_leaves_the_alias_on_the_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"old version").unwrap();
        let alias = crate::alias::path_beside(&exe);
        crate::alias::link(&exe, &alias).unwrap();
        let replacement = dir.path().join("staged-turnout");
        std::fs::write(&replacement, b"new version").unwrap();

        let outcome = install_binary(&exe, &replacement).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new version");
        assert_eq!(
            std::fs::read(&alias).unwrap(),
            b"new version",
            "the alias still answers with the release the update replaced"
        );
        assert!(matches!(outcome, crate::alias::Outcome::Relinked(_)));
    }

    /// Both leftovers go, not just the one beside the running name. Found on
    /// the owner's machine 19.08: `turnout.exe.old` had been swept, while a
    /// `tn.exe.old` from an update run under the alias name sat there for
    /// weeks - a whole binary nobody was ever going to collect.
    #[test]
    fn the_sweep_takes_the_alias_leftover_too() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout.exe");
        std::fs::write(&exe, b"binary").unwrap();
        let exe_backup = backup_path(&exe);
        let alias_backup = backup_path(&crate::alias::path_beside(&exe));
        std::fs::write(&exe_backup, b"previous turnout").unwrap();
        std::fs::write(&alias_backup, b"previous tn").unwrap();

        sweep_backups_beside(&exe);

        assert!(!exe_backup.exists());
        assert!(!alias_backup.exists(), "tn.exe.old survives the sweep and never gets collected");
        assert!(exe.exists(), "the sweep must not touch the binary itself");
    }

    /// The field report of 31.08: on the owner's second Windows machine
    /// `turnout self-update` succeeded and `tn` was gone afterwards -
    /// "command not found", not a stale version.
    ///
    /// The update was run under the *alias* name. `current_exe()` is then
    /// `tn.exe`, so the swap renames `tn.exe` aside and installs the new
    /// binary as `tn.exe` - and `path_beside` of `tn.exe` is `tn.exe` itself,
    /// so the refresh has no second name to point anywhere. `turnout.exe` is
    /// left as the outgoing release, unlinked. Whichever of the two the shell
    /// resolved first then decided what the user saw.
    #[test]
    fn an_update_run_under_the_alias_keeps_both_names_working() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        let alias = crate::alias::path_beside(&exe);
        std::fs::write(&exe, b"old version").unwrap();
        crate::alias::link(&exe, &alias).unwrap();
        let replacement = dir.path().join("staged-turnout");
        std::fs::write(&replacement, b"new version").unwrap();

        // Run the update the way `tn self-update` does: current_exe is the alias.
        install_binary(&alias, &replacement).unwrap();

        assert!(alias.exists(), "the alias must survive an update run under its own name");
        assert!(exe.exists(), "the primary name must survive an update run under the alias");
        assert_eq!(std::fs::read(&alias).unwrap(), b"new version");
        assert_eq!(
            std::fs::read(&exe).unwrap(),
            b"new version",
            "the primary name was left on the release the update replaced"
        );
    }

    /// An install that never had an alias must not grow one from an update.
    #[test]
    fn an_update_does_not_invent_an_alias() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"old version").unwrap();
        let replacement = dir.path().join("staged-turnout");
        std::fs::write(&replacement, b"new version").unwrap();

        let outcome = install_binary(&exe, &replacement).unwrap();

        assert!(!crate::alias::path_beside(&exe).exists());
        assert!(matches!(outcome, crate::alias::Outcome::Absent));
    }

    /// The swap must leave a working binary in place, and the outgoing one
    /// beside it for the next run to sweep.
    #[test]
    fn replacing_keeps_the_old_binary_beside_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("turnout");
        std::fs::write(&exe, b"old").unwrap();
        let replacement = dir.path().join("new-turnout");
        std::fs::write(&replacement, b"new").unwrap();

        replace_running_binary(&exe, &replacement).unwrap();

        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(backup_path(&exe)).unwrap(), b"old");
        assert!(!replacement.exists(), "the staged file should have been consumed");
    }
}
