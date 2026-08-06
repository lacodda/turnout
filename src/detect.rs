use std::collections::BTreeMap;
use std::path::Path;

/// Project flavor guessed from lock/manifest files; drives default commands.
#[derive(Clone, Copy, PartialEq)]
pub enum ProjectKind {
    Pnpm,
    Yarn,
    Npm,
    Cargo,
    Unknown,
}

impl ProjectKind {
    pub fn label(self) -> &'static str {
        match self {
            ProjectKind::Pnpm => "pnpm",
            ProjectKind::Yarn => "yarn",
            ProjectKind::Npm => "npm",
            ProjectKind::Cargo => "cargo",
            ProjectKind::Unknown => "unknown",
        }
    }
}

pub fn detect(path: &Path) -> ProjectKind {
    if path.join("pnpm-lock.yaml").exists() {
        ProjectKind::Pnpm
    } else if path.join("yarn.lock").exists() {
        ProjectKind::Yarn
    } else if path.join("package-lock.json").exists() || path.join("package.json").exists() {
        ProjectKind::Npm
    } else if path.join("Cargo.toml").exists() {
        ProjectKind::Cargo
    } else {
        ProjectKind::Unknown
    }
}

/// Default command set for a detected project kind; the user can override any of them.
pub fn default_commands(kind: ProjectKind) -> BTreeMap<String, String> {
    let pairs: &[(&str, &str)] = match kind {
        ProjectKind::Pnpm => &[("dev", "pnpm dev"), ("build", "pnpm build"), ("test", "pnpm test"), ("lint", "pnpm lint")],
        ProjectKind::Yarn => &[("dev", "yarn dev"), ("build", "yarn build"), ("test", "yarn test"), ("lint", "yarn lint")],
        ProjectKind::Npm => &[
            ("dev", "npm run dev"),
            ("build", "npm run build"),
            ("test", "npm test"),
            ("lint", "npm run lint"),
        ],
        ProjectKind::Cargo => &[
            ("dev", "cargo run"),
            ("build", "cargo build --release"),
            ("test", "cargo test"),
            ("lint", "cargo clippy --all-targets -- -D warnings"),
        ],
        ProjectKind::Unknown => &[],
    };
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
