//! What a build produced, on this machine: the files a deploy sends, packed
//! once however many targets they go to, and whether they are older than the
//! sources they were built from.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// Below this, packing is not worth the extra round trips: a handful of files
/// go over SFTP faster than tar + upload + untar can set itself up.
pub const ARCHIVE_THRESHOLD: u64 = 8;

/// Whether a tree of `files` files should travel as one archive.
///
/// Split out from the upload so the rule is testable without a server, and so
/// the opt-out and the threshold live in one readable place.
pub fn should_archive(files: u64, no_archive: bool) -> bool {
    !no_archive && files >= ARCHIVE_THRESHOLD
}

/// What the upload is about to send: directories to create and files to copy,
/// each with the path relative to the artifact root.
pub struct Plan {
    pub dirs: Vec<String>,
    pub entries: Vec<Entry>,
    pub files: u64,
    pub bytes: u64,
    /// When the newest file was written - the moment the build finished, as
    /// far as anything can tell afterwards.
    pub newest: Option<SystemTime>,
}

/// One file of the artifact.
pub struct Entry {
    pub path: PathBuf,
    /// Relative to the artifact root, with `/` whatever the OS.
    pub relative: String,
    pub size: u64,
}

/// Walk the artifact directory to learn its size before sending anything.
/// Directories come out parents-first so they can be created in order.
pub fn plan_upload(local_root: &Path) -> Result<Plan> {
    let mut plan = Plan {
        dirs: Vec::new(),
        entries: Vec::new(),
        files: 0,
        bytes: 0,
        newest: None,
    };
    let mut queue = vec![local_root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("cannot read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(local_root).expect("entry under root").to_string_lossy().replace('\\', "/");
            let metadata = entry.metadata().with_context(|| format!("cannot stat {}", path.display()))?;
            if metadata.is_dir() {
                plan.dirs.push(relative);
                queue.push(path);
            } else {
                plan.files += 1;
                plan.bytes += metadata.len();
                if let Ok(modified) = metadata.modified() {
                    plan.newest = plan.newest.max(Some(modified));
                }
                plan.entries.push(Entry {
                    path,
                    relative,
                    size: metadata.len(),
                });
            }
        }
    }
    // Shallow paths first: a child directory is never created before its parent.
    plan.dirs.sort_by_key(|d| d.matches('/').count());
    Ok(plan)
}

/// Build the gzipped tar in memory, entries relative to the artifact root so it
/// unpacks straight into the deploy directory.
pub fn pack(plan: &Plan) -> Result<Vec<u8>> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    for entry in &plan.entries {
        let mut file = std::fs::File::open(&entry.path).with_context(|| format!("cannot open {}", entry.path.display()))?;
        builder
            .append_file(&entry.relative, &mut file)
            .with_context(|| format!("cannot add {} to the archive", entry.path.display()))?;
    }
    let encoder = builder.into_inner().context("cannot finish the archive")?;
    encoder.finish().context("cannot compress the archive")
}

/// One app's build, ready to go out: planned once, packed at most once, and
/// sent to as many targets as name the app.
pub struct Artifact {
    pub app: String,
    pub plan: Plan,
    archive: OnceCell<Vec<u8>>,
}

impl Artifact {
    pub fn new(app: &str, plan: Plan) -> Self {
        Self {
            app: app.to_string(),
            plan,
            archive: OnceCell::new(),
        }
    }

    /// The archive, packed on first use: a deploy that goes file by file
    /// everywhere never pays for it, and the second target does not pay again.
    pub fn archive(&self) -> Result<&[u8]> {
        if self.archive.get().is_none() {
            let packed = pack(&self.plan)?;
            let _ = self.archive.set(packed);
        }
        Ok(self.archive.get().expect("the archive was just packed"))
    }
}

/// A source changed after the build that is about to be sent.
#[derive(Debug, PartialEq, Eq)]
pub struct Stale {
    /// The newest such file, relative to the project.
    pub source: String,
    /// How long after the build it changed.
    pub after: Duration,
}

/// File systems that keep two-second times (FAT, some network shares) would
/// otherwise call a source saved in the build's own second newer than it.
const SLACK: Duration = Duration::from_secs(2);

/// Directories a source walk never enters when git cannot be asked: the
/// project's own outputs and caches, not what it is built from.
const NOT_SOURCES: [&str; 5] = [".git", "node_modules", "target", ".turbo", ".cache"];

/// Whether a source in `project` changed after `built`, the newest file of
/// the artifact in `dist`.
///
/// The sources are what git tracks or would track - asked of git itself, so
/// an ignored `.env.local` or a cache the tool writes is not mistaken for an
/// edit. Outside a repository the project is walked, leaving out the usual
/// outputs. The artifact directory and `skip` (the dotenv file turnout itself
/// writes on every switch) never count.
pub fn stale(project: &Path, dist: &Path, built: SystemTime, skip: &[&Path]) -> Option<Stale> {
    let sources = git_sources(project).unwrap_or_else(|| walked_sources(project));
    let dist = dist.strip_prefix(project).unwrap_or(dist);
    let newest = sources
        .into_iter()
        .filter(|relative| !relative.starts_with(dist) && !skip.iter().any(|skip| relative == skip))
        .filter_map(|relative| {
            let modified = std::fs::metadata(project.join(&relative)).ok()?.modified().ok()?;
            Some((modified, relative))
        })
        .max_by_key(|(modified, _)| *modified)?;
    let after = newest.0.duration_since(built).ok().filter(|after| *after > SLACK)?;
    Some(Stale {
        source: newest.1.to_string_lossy().replace('\\', "/"),
        after,
    })
}

/// The files git tracks or would track in `project`, relative to it; `None`
/// when git cannot answer (no repository, no git).
fn git_sources(project: &Path) -> Option<Vec<PathBuf>> {
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard", "-z"])
        .current_dir(project)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| PathBuf::from(String::from_utf8_lossy(name).into_owned()))
            .collect(),
    )
}

fn walked_sources(project: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![project.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    if !NOT_SOURCES.iter().any(|skip| entry.file_name() == *skip) {
                        pending.push(path);
                    }
                }
                Ok(_) => {
                    if let Ok(relative) = path.strip_prefix(project) {
                        found.push(relative.to_path_buf());
                    }
                }
                Err(_) => {}
            }
        }
    }
    found
}

/// The SHA-256 of a file, as lowercase hex - the form `sha256sum` and
/// PowerShell's `Get-FileHash` (lowercased) print.
pub fn sha256(path: &Path) -> Result<String> {
    use sha2::Digest;
    use std::io::Read;
    let mut hasher = sha2::Sha256::new();
    let mut file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).with_context(|| format!("cannot read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan drives both the progress total and the order of remote mkdirs,
    /// so it has to count every nested file and list parents before children.
    #[test]
    fn plans_a_nested_tree() {
        let root = tempfile::tempdir().expect("temp dir");
        let deep = root.path().join("assets").join("img");
        std::fs::create_dir_all(&deep).expect("create dirs");
        std::fs::write(root.path().join("index.html"), "hello").expect("write");
        std::fs::write(deep.join("logo.svg"), "12345678").expect("write");

        let plan = plan_upload(root.path()).expect("plan");

        assert_eq!(plan.files, 2);
        assert_eq!(plan.bytes, 13);
        assert_eq!(plan.dirs, vec!["assets", "assets/img"]);
        let mut relative: Vec<_> = plan.entries.iter().map(|e| e.relative.as_str()).collect();
        relative.sort_unstable();
        assert_eq!(relative, vec!["assets/img/logo.svg", "index.html"]);
        assert!(plan.newest.is_some(), "the build's moment is read off its files");
    }

    /// A real dist directory is thousands of files and always packs; a couple
    /// of files are faster sent as they are. `--no-archive` overrides both.
    #[test]
    fn only_trees_worth_packing_are_packed() {
        assert!(should_archive(2_000, false), "a dist directory packs");
        assert!(should_archive(ARCHIVE_THRESHOLD, false), "the threshold itself packs");
        assert!(!should_archive(ARCHIVE_THRESHOLD - 1, false), "just below it does not");
        assert!(!should_archive(1, false), "a single file never packs");
        assert!(!should_archive(2_000, true), "--no-archive wins over any size");
    }

    /// The archive has to unpack straight into the deploy directory, so entries
    /// are stored relative to the artifact root - an absolute or `./`-prefixed
    /// path would land the files somewhere else entirely.
    #[test]
    fn the_archive_holds_relative_paths_and_content() {
        let root = tempfile::tempdir().expect("temp dir");
        let deep = root.path().join("assets").join("img");
        std::fs::create_dir_all(&deep).expect("create dirs");
        std::fs::write(root.path().join("index.html"), "<!doctype html>").expect("write");
        std::fs::write(deep.join("logo.svg"), "<svg/>").expect("write");

        let plan = plan_upload(root.path()).expect("plan");
        let archive = pack(&plan).expect("pack");

        let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(&archive));
        let mut tar = tar::Archive::new(decoder);
        let mut found = std::collections::BTreeMap::new();
        for entry in tar.entries().expect("entries") {
            use std::io::Read;
            let mut entry = entry.expect("entry");
            let path = entry.path().expect("path").to_string_lossy().replace('\\', "/");
            let mut content = String::new();
            entry.read_to_string(&mut content).expect("read");
            found.insert(path, content);
        }

        assert_eq!(found.get("index.html").map(String::as_str), Some("<!doctype html>"));
        assert_eq!(found.get("assets/img/logo.svg").map(String::as_str), Some("<svg/>"));
        assert_eq!(found.len(), 2, "only files travel; directories come from the paths: {found:?}");
        assert!(
            !found.keys().any(|p| p.starts_with('/') || p.starts_with("./")),
            "paths must be relative: {found:?}"
        );
    }

    /// Compression is the point: a dist directory of text files should not
    /// cross the wire at full size.
    #[test]
    fn the_archive_is_smaller_than_the_tree() {
        let root = tempfile::tempdir().expect("temp dir");
        for index in 0..20 {
            std::fs::write(root.path().join(format!("chunk-{index}.js")), "console.log('hello world');\n".repeat(200)).expect("write");
        }
        let plan = plan_upload(root.path()).expect("plan");
        let archive = pack(&plan).expect("pack");
        assert!(
            (archive.len() as u64) < plan.bytes / 4,
            "expected real compression, got {} from {}",
            archive.len(),
            plan.bytes
        );
    }

    /// Two targets of one app send the same bytes: the archive is packed on
    /// the first ask and handed out again on the second.
    #[test]
    fn an_artifact_packs_once() {
        let root = tempfile::tempdir().expect("temp dir");
        std::fs::write(root.path().join("index.html"), "hello").expect("write");
        let artifact = Artifact::new("web", plan_upload(root.path()).unwrap());
        let first = artifact.archive().unwrap().as_ptr();
        let second = artifact.archive().unwrap().as_ptr();
        assert_eq!(first, second, "the second target must not pack again");
    }

    #[test]
    fn an_empty_tree_plans_nothing() {
        let root = tempfile::tempdir().expect("temp dir");
        let plan = plan_upload(root.path()).expect("plan");
        assert_eq!(plan.files, 0);
        assert!(plan.entries.is_empty());
        assert_eq!(plan.newest, None);
    }

    fn touch(path: &Path, at: SystemTime) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x").unwrap();
        std::fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
    }

    /// A source saved after the build makes the build stale, and the message
    /// names it; the build's own output and turnout's dotenv file never do.
    #[test]
    fn a_source_newer_than_the_build_is_named() {
        let project = tempfile::tempdir().unwrap();
        let built = SystemTime::now() - Duration::from_secs(3600);
        touch(&project.path().join("dist/index.html"), built);
        touch(&project.path().join("src/main.ts"), built - Duration::from_secs(60));
        touch(&project.path().join(".env.development.local"), built + Duration::from_secs(600));
        touch(&project.path().join("node_modules/x/index.js"), built + Duration::from_secs(600));
        let dist = project.path().join("dist");
        let skip = [Path::new(".env.development.local")];
        assert_eq!(stale(project.path(), &dist, built, &skip), None, "nothing the build reads changed");

        touch(&project.path().join("src/App.vue"), built + Duration::from_secs(720));
        let found = stale(project.path(), &dist, built, &skip).expect("stale");
        assert_eq!(found.source, "src/App.vue");
        assert_eq!(found.after.as_secs(), 720);
    }

    /// A save in the build's own second is not "after the build" on a file
    /// system that rounds to two seconds.
    #[test]
    fn a_save_within_the_slack_is_not_stale() {
        let project = tempfile::tempdir().unwrap();
        let built = SystemTime::now() - Duration::from_secs(60);
        touch(&project.path().join("src/a.ts"), built + Duration::from_secs(1));
        assert_eq!(stale(project.path(), &project.path().join("dist"), built, &[]), None);
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, "abc").unwrap();
        assert_eq!(sha256(&file).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
