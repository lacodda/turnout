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

    /// How this manager invokes a package script.
    fn run_prefix(self) -> Option<&'static str> {
        match self {
            ProjectKind::Pnpm => Some("pnpm"),
            ProjectKind::Yarn => Some("yarn"),
            // npm needs the explicit `run` for anything but its few built-ins.
            ProjectKind::Npm => Some("npm run"),
            ProjectKind::Cargo | ProjectKind::Unknown => None,
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

/// turnout's roles, and the script names that commonly fill them - in
/// preference order, so a project with both `dev` and `start` maps `dev`.
const ROLES: &[(&str, &[&str])] = &[
    ("dev", &["dev", "serve", "start", "dev:server", "watch"]),
    ("build", &["build", "build:prod", "compile", "dist"]),
    ("test", &["test", "test:unit", "spec"]),
    ("lint", &["lint", "lint:js", "eslint"]),
];

/// Commands for a project: real `package.json` scripts when they are there,
/// otherwise the manager's conventional defaults.
pub fn commands_for(path: &Path, kind: ProjectKind) -> BTreeMap<String, String> {
    match read_scripts(path) {
        Some(scripts) if !scripts.is_empty() => from_scripts(&scripts, kind),
        _ => default_commands(kind),
    }
}

/// Script names declared in `package.json`, in file order.
fn read_scripts(path: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let scripts = json.get("scripts")?.as_object()?;
    Some(scripts.keys().cloned().collect())
}

/// Map declared scripts onto turnout's roles. A script that fills no role is
/// still offered under its own name, so `turnout run storybook` works.
fn from_scripts(scripts: &[String], kind: ProjectKind) -> BTreeMap<String, String> {
    let Some(prefix) = kind.run_prefix() else {
        return default_commands(kind);
    };
    let mut commands = BTreeMap::new();
    let mut claimed: Vec<&str> = Vec::new();

    for (role, candidates) in ROLES {
        if let Some(script) = candidates.iter().find(|c| scripts.iter().any(|s| s == *c)) {
            commands.insert(role.to_string(), format!("{prefix} {script}"));
            claimed.push(script);
        }
    }
    for script in scripts {
        if !claimed.contains(&script.as_str()) && !commands.contains_key(script) {
            commands.insert(script.clone(), format!("{prefix} {script}"));
        }
    }
    commands
}

/// Default command set for a detected project kind; the user can override any of them.
fn default_commands(kind: ProjectKind) -> BTreeMap<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_fill_turnout_roles() {
        let scripts: Vec<String> = ["serve", "build", "test"].iter().map(|s| s.to_string()).collect();
        let commands = from_scripts(&scripts, ProjectKind::Pnpm);
        // `serve` is this project's dev script.
        assert_eq!(commands.get("dev").unwrap(), "pnpm serve");
        assert_eq!(commands.get("build").unwrap(), "pnpm build");
        assert!(!commands.contains_key("lint"));
    }

    #[test]
    fn dev_wins_over_start_and_extras_are_kept() {
        let scripts: Vec<String> = ["start", "dev", "storybook"].iter().map(|s| s.to_string()).collect();
        let commands = from_scripts(&scripts, ProjectKind::Npm);
        assert_eq!(commands.get("dev").unwrap(), "npm run dev");
        // Unclaimed scripts stay reachable through `turnout run`.
        assert_eq!(commands.get("storybook").unwrap(), "npm run storybook");
        assert_eq!(commands.get("start").unwrap(), "npm run start");
    }

    #[test]
    fn projects_without_scripts_keep_the_defaults() {
        let commands = commands_for(Path::new("does-not-exist"), ProjectKind::Cargo);
        assert_eq!(commands.get("build").unwrap(), "cargo build --release");
    }
}
