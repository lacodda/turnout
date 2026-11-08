//! Guards the facts that must agree before a version is published.
//!
//! turnout ships to three places - GitHub, crates.io and npm - each of which
//! renders its own copy of the metadata. They drift silently, and the drift is
//! only visible after publishing, when it is too late to take back.
//!
//! What every product of the line shares - one version across the manifests
//! and the npm wrapper's binary tag, one description, one README with absolute
//! links, only real commands in it, a page per command, the docs site where
//! `homepage` says - is held by `lyrn check`, in the gate and in the Shopfronts
//! workflow. What is turnout's own stays here: the installers, the `tn` alias,
//! the icon's levels and the README's status sample.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read(path: impl AsRef<Path>) -> String {
        let path = repo_root().join(path);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// Extracts a top-level `key = "value"` from Cargo.toml.
    ///
    /// Deliberately naive: it reads only the `[package]` block, which is all
    /// these checks need, and avoids adding a TOML parser as a dev-dependency.
    fn cargo_field(key: &str) -> String {
        let manifest = read("Cargo.toml");
        for line in manifest.lines() {
            let line = line.trim();
            // Stop at the next section: `version` also appears under [lib],
            // [[bin]] and in every dependency.
            if line.starts_with('[') && line != "[package]" {
                break;
            }
            let Some((name, value)) = line.split_once('=') else { continue };
            // Exact match, so `rust-version` cannot answer a lookup for `version`.
            if name.trim() != key {
                continue;
            }
            return value.trim().trim_matches('"').to_string();
        }
        panic!("`{key}` not found in the [package] block of Cargo.toml");
    }

    #[test]
    fn the_unix_installer_redirects_windows_shells() {
        // Field report from kasl, 19.08: run in Git Bash on Windows, the
        // script matched no case arm and answered "No prebuilt binary for
        // MINGW64_NT-…", which reads as "unsupported platform" although a
        // Windows release exists - it is just installed by the other script.
        let installer = read("tools/install.sh");

        for shell in ["MINGW*", "MSYS*", "CYGWIN*"] {
            assert!(
                installer.contains(shell),
                "install.sh does not recognise {shell}; Windows shells fall through to the                  generic 'no prebuilt binary' message"
            );
        }
        assert!(
            installer.contains("install.ps1"),
            "install.sh does not name the PowerShell installer, leaving Windows users at a dead end"
        );
    }

    #[test]
    fn installers_name_the_crate_that_actually_exists() {
        // A `cargo install <name>` fallback that does not resolve is worse
        // than none: in kasl the suggestion named `kasl` while the crate is
        // published as `kasl-cli`, so the advice in the error message failed.
        let crate_name = cargo_field("name");

        // Counted, so the check cannot pass by finding nothing to check: a
        // reworded fallback (`cargo  install`, a variable) would otherwise
        // turn this test into one that asserts over an empty set.
        let mut suggestions = 0;
        for file in ["tools/install.sh", "tools/install.ps1"] {
            let text = read(file);
            for (line_no, line) in text.lines().enumerate() {
                let Some(at) = line.find("cargo install ") else { continue };
                suggestions += 1;
                // Trim shell quoting around the suggestion, e.g. `... turnout" >&2`.
                let named = line[at + "cargo install ".len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
                assert_eq!(
                    named,
                    crate_name,
                    "{file} line {} suggests `cargo install {named}`, but the crate is `{crate_name}`",
                    line_no + 1
                );
            }
        }
        assert!(
            suggestions > 0,
            "no installer suggests `cargo install {crate_name}` any more - the fallback for a platform without a prebuilt binary is gone, or this check no longer recognises it"
        );
    }

    /// The `tn` alias must be a link, never a second copy of the binary.
    ///
    /// Field report, 19.08: after `self-update`, `tn --version` still printed
    /// the previous release. The alias was a copy the installer made, and
    /// self-update only replaces the file it is running from - so the same
    /// name kept answering with old code, which reads as an unexplained
    /// downgrade. A copy also doubles the install for no new code.
    /// The Windows installer edits the user PATH in the registry and keeps its
    /// type. `[Environment]::SetEnvironmentVariable` rewrites the value as
    /// REG_SZ, after which entries like `%JAVA_HOME%\bin` stop expanding and
    /// silently break - on a machine that has none, "works for me" proves
    /// nothing. Found by installing rigger, whose installer had the same line.
    #[test]
    fn the_windows_installer_keeps_the_path_type() {
        let windows = read("tools/install.ps1");
        assert!(
            !windows.contains("SetEnvironmentVariable"),
            "install.ps1 must not call [Environment]::SetEnvironmentVariable - it stores the user PATH as REG_SZ and breaks %VAR% entries"
        );
        assert!(
            windows.contains("DoNotExpandEnvironmentNames") && windows.contains("-Type ExpandString"),
            "install.ps1 must read the raw PATH and write it back as ExpandString"
        );
        assert!(
            windows.contains("0x1A"),
            "install.ps1 must broadcast WM_SETTINGCHANGE so open shells see the new PATH"
        );
    }

    /// One image inside an `.ico`: its declared size and the PNG payload.
    fn ico_images(ico: &[u8]) -> Vec<(u32, &[u8])> {
        // Header: reserved (2), type (2), count (2); then 16 bytes per entry.
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        (0..count)
            .map(|index| {
                let at = 6 + index * 16;
                let entry = &ico[at..at + 16];
                // A zero width means 256: the field is one byte.
                let size = if entry[0] == 0 { 256 } else { entry[0] as u32 };
                let length = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
                let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
                (size, &ico[offset..offset + length])
            })
            .collect()
    }

    /// Whether a PNG shows the filled S tile rather than the plated mark.
    ///
    /// A quarter of the way across, vertically centred: inside the hexagon,
    /// clear of the code. The S tile is the brand colour there (bright), the
    /// M/L plate is near-black - the same glance the eye makes in a taskbar.
    fn is_filled_tile(png: &[u8]) -> (bool, u32) {
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("an icon image is not a PNG");
        let mut buf = vec![0; reader.output_buffer_size().expect("png buffer size")];
        let info = reader.next_frame(&mut buf).expect("cannot decode an icon image");
        let channels = info.color_type.samples();
        assert_eq!(info.bit_depth, png::BitDepth::Eight, "icon images are exported as 8-bit RGBA");
        let (x, y) = (info.width as usize / 4, info.height as usize / 2);
        let at = (y * info.width as usize + x) * channels;
        let pixel = &buf[at..at + channels];
        if channels == 4 {
            assert!(pixel[3] > 40, "the {}px image is transparent where the tile should be", info.width);
        }
        let brightness = u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
        (brightness > 180, info.width)
    }

    /// The level rule of the line, held against the actual pixels.
    ///
    /// The exporter used to take the S tile - a hexagon filled with the brand
    /// colour - for every size, so the taskbar (48px) and the desktop (48-96px)
    /// showed a coloured lozenge instead of the mark. S reads at 27px and
    /// below; from 28px up the plated mark (M, then L) must be there.
    #[test]
    fn every_icon_size_carries_the_level_that_reads_at_it() {
        let ico = std::fs::read(repo_root().join("assets/icon.ico")).expect("assets/icon.ico is missing");
        let images = ico_images(&ico);
        assert!(!images.is_empty(), "the .ico has no images");
        for (size, png) in images {
            let (filled, width) = is_filled_tile(png);
            assert_eq!(width, size, "the {size}px entry holds a {width}px image");
            if size <= 27 {
                assert!(
                    filled,
                    "the {size}px image is not the filled S tile; below 28px the outline collapses into noise"
                );
            } else {
                assert!(
                    !filled,
                    "the {size}px image is the filled S tile, not the plated mark - the level rule puts S at 27px and below"
                );
            }
        }
        // The touch icon is drawn at 180px: L territory, and the docs site
        // shows the same file.
        let touch = std::fs::read(repo_root().join("assets/apple-touch-icon.png")).expect("apple-touch-icon.png is missing");
        let (filled, width) = is_filled_tile(&touch);
        assert_eq!(width, 180);
        assert!(!filled, "apple-touch-icon.png is the filled S tile at 180px");
        let docs_copy = std::fs::read(repo_root().join("docs/public/apple-touch-icon.png")).expect("docs copy is missing");
        assert_eq!(touch, docs_copy, "docs/public/apple-touch-icon.png drifted from assets/");
        // The notification icon is drawn at 16 DIP in a toast's header: S,
        // whatever the pixel size of the file that covers display scaling.
        let toast = std::fs::read(repo_root().join("assets/toast-icon.png")).expect("assets/toast-icon.png is missing");
        let (filled, width) = is_filled_tile(&toast);
        assert_eq!(width, 64);
        assert!(filled, "toast-icon.png is not the filled S tile; a toast header draws it at 16 DIP");
    }

    /// Largest first: Windows picks by closest size and ignores order, but
    /// readers that take the first entry verbatim exist.
    #[test]
    fn the_largest_icon_image_comes_first() {
        let ico = std::fs::read(repo_root().join("assets/icon.ico")).expect("assets/icon.ico is missing");
        let sizes: Vec<u32> = ico_images(&ico).into_iter().map(|(size, _)| size).collect();
        let mut sorted = sizes.clone();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(sizes, sorted, "the images are not ordered largest first");
    }

    #[test]
    fn the_alias_is_a_link_rather_than_a_second_binary() {
        let unix = read("tools/install.sh");
        assert!(
            unix.contains("ln -sf turnout"),
            "install.sh must symlink the alias; a copy goes stale on the next self-update"
        );

        let windows = read("tools/install.ps1");
        assert!(
            windows.contains("-ItemType HardLink"),
            "install.ps1 must hard-link the alias (symlinks need elevation on Windows); a plain copy goes stale on the next self-update"
        );
        // A copy is still the fallback for filesystems without hard links, so
        // the check is that the link is tried first, not that no copy exists.
        let link_at = windows.find("-ItemType HardLink").expect("checked above");
        let copy_at = windows
            .find("Copy-Item $binary $alias")
            .expect("install.ps1 has no copy fallback for the alias - or this check lost track of it");
        assert!(
            link_at < copy_at,
            "install.ps1 copies the alias before trying to link it - the copy would always win"
        );
    }

    /// A running turnout.exe - a gateway, a dev server - cannot be overwritten,
    /// so an update that copies over it fails exactly when turnout is in use.
    /// Both names step aside before the copy: `tn.exe` is the same file, and a
    /// process started as `tn` holds it just the same.
    #[test]
    fn the_windows_installer_moves_a_running_binary_aside() {
        let windows = read("tools/install.ps1");
        let moved = windows
            .find("foreach ($old in $binary, $alias)")
            .expect("install.ps1 no longer moves turnout.exe and tn.exe aside before the copy");
        let copied = windows
            .find("Copy-Item (Join-Path $tmp \"$name\\turnout.exe\") $binary")
            .expect("install.ps1 no longer copies the binary from the archive");
        assert!(moved < copied, "install.ps1 copies the new binary before moving the running one aside");
        assert!(windows.contains("Move-Item $old \"$old.old-"), "install.ps1 must rename, not delete: a running binary cannot be deleted");
    }

    /// `TURNOUT_NO_PATH` is how a release check installs into a scratch
    /// directory without leaving that directory on the user's PATH; both
    /// installers have to honour it, and still make the alias there.
    #[test]
    fn the_installers_leave_the_path_alone_when_asked() {
        let windows = read("tools/install.ps1");
        let guard = windows.find("if (-not $env:TURNOUT_NO_PATH)").expect("install.ps1 ignores TURNOUT_NO_PATH");
        let edit = windows.find("Set-ItemProperty -Path \"HKCU:\\Environment\"").expect("install.ps1 no longer edits the PATH");
        assert!(guard < edit, "install.ps1 edits the PATH before reading TURNOUT_NO_PATH");
        let unix = read("tools/install.sh");
        assert!(unix.contains("TURNOUT_NO_PATH"), "install.sh ignores TURNOUT_NO_PATH");
    }

    /// The release archive carries one binary. Shipping a second copy under
    /// the alias name would double every download for no new code - the whole
    /// reason the alias is a link.
    #[test]
    fn the_alias_is_not_packaged_as_a_second_binary() {
        let manifest = read("Cargo.toml");
        assert!(
            !manifest.contains("[[bin]]"),
            "Cargo.toml declares an extra binary; the `tn` alias is a link made at install time, not a packaged copy"
        );

        let workflow = read(".github/workflows/release.yml");
        assert!(
            !workflow.contains("tn.exe") && !workflow.contains("/tn "),
            "release.yml packages a `tn` binary; the alias is created by the installer as a link"
        );
    }

    /// The `turnout status` sample in the README prints a version number, and a
    /// version number in prose goes stale the moment the next release ships.
    /// Checked here rather than by eye: the doc sweep at the end of a stage is
    /// the step most likely to be rushed, and this is the one line in the README
    /// that is wrong by default.
    ///
    /// Any major, not `turnout 0.` alone: matched that way, a stale
    /// `turnout 1.0.0` after the next release would pass, and so would a
    /// README whose sample had been deleted - an empty set has nothing stale.
    #[test]
    fn the_readme_sample_shows_the_current_version() {
        let readme = read("README.md");
        let version = cargo_field("version");
        let samples: Vec<&str> = readme
            .lines()
            .map(str::trim)
            .filter(|line| line.strip_prefix("turnout ").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit())))
            .collect();
        assert!(
            !samples.is_empty(),
            "the README has no `turnout <version>` line in its status sample - this check guards nothing"
        );
        let stale: Vec<&str> = samples.into_iter().filter(|line| !line.contains(&version)).collect();
        assert!(
            stale.is_empty(),
            "the README shows an old version in a sample: {stale:?} (this release is {version})"
        );
    }
}
