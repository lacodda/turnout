//! `history`: the deploys made from this machine - as a table, one in full,
//! or the commits between two of them.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::cli::Format;
use crate::history::{self, Check, Outcome, Record, Restart};
use crate::output;
use crate::progress::{human_bytes, human_duration, human_span};

pub fn run(name: Option<String>, limit: usize, show: Option<String>, diff: Option<Vec<String>>, format: Format) -> Result<()> {
    let store = history::Store::open()?;
    let records = store.all()?;
    if let Some(id) = show {
        let id = history::parse_id(&id)?;
        let record = find(&records, id)?;
        return output::emit(format, &view(&store, record), detail);
    }
    if let Some(pair) = diff {
        let [from, to] = pair.as_slice() else {
            bail!("--diff takes two deploys: FROM and TO");
        };
        let from = find(&records, history::parse_id(from)?)?;
        let to = find(&records, history::parse_id(to)?)?;
        return output::emit(format, &commits_between(from, to)?, |view| view.text());
    }
    let mut chosen: Vec<&Record> = match &name {
        Some(name) => records
            .iter()
            .filter(|record| record.target.as_deref() == Some(name) || &record.app == name)
            .collect(),
        None => records.iter().collect(),
    };
    if chosen.len() > limit {
        chosen.drain(..chosen.len() - limit);
    }
    let views: Vec<DeployView> = chosen.into_iter().map(|record| view(&store, record)).collect();
    output::emit(format, &views, |views| {
        if !views.is_empty() {
            return table(views);
        }
        match &name {
            Some(name) => format!("No deploys of '{name}' are recorded on this machine."),
            None => "No deploys recorded yet - every `turnout deploy` from now on is.".to_string(),
        }
    })
}

fn find(records: &[Record], id: u64) -> Result<&Record> {
    records
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| anyhow::anyhow!("no deploy #{id} is recorded on this machine - see `turnout history`"))
}

/// One deploy, as `history` reports it: every key always there.
#[derive(Serialize)]
pub struct DeployView {
    pub id: u64,
    pub at: String,
    pub app: String,
    pub target: Option<String>,
    pub server: String,
    pub path: String,
    pub dir: String,
    pub files: u64,
    pub bytes: u64,
    pub seconds: f64,
    /// `deployed`, `rolled_back`, `partial` or `failed`.
    pub outcome: &'static str,
    pub error: Option<String>,
    pub version: Option<String>,
    pub commit: Option<String>,
    /// The working tree held changes the commit does not.
    pub dirty: bool,
    pub restart: Option<Restart>,
    pub check: Option<Check>,
    /// The deploy whose build this one sent again.
    pub rollback_of: Option<u64>,
    /// Whether its build is still kept, so `rollback --to` can send it.
    pub build_kept: bool,
    #[serde(skip)]
    kind: Outcome,
}

fn view(store: &history::Store, record: &Record) -> DeployView {
    DeployView {
        id: record.id,
        at: record.at.clone(),
        app: record.app.clone(),
        target: record.target.clone(),
        server: record.server.clone(),
        path: record.path.clone(),
        dir: record.dir.clone(),
        files: record.files,
        bytes: record.bytes,
        seconds: (record.millis as f64 / 100.0).round() / 10.0,
        outcome: record.outcome.word(),
        error: record.error.clone(),
        version: record.version.clone(),
        commit: record.commit.as_ref().map(|commit| commit.sha.clone()),
        dirty: record.commit.as_ref().is_some_and(|commit| commit.dirty),
        restart: record.restart.clone(),
        check: record.check.clone(),
        rollback_of: record.rollback_of,
        build_kept: record.build.as_deref().is_some_and(|digest| store.build(digest).is_some()),
        kind: record.outcome,
    }
}

impl DeployView {
    /// Where it went: the target, or `app → server`.
    fn label(&self) -> String {
        self.target.clone().unwrap_or_else(|| format!("{} → {}", self.app, self.server))
    }

    /// `1a2b3c4`, `1a2b3c4-dirty`, or `-`.
    fn short_commit(&self) -> String {
        match &self.commit {
            Some(sha) => history::Commit {
                sha: sha.clone(),
                dirty: self.dirty,
            }
            .short(),
            None => "-".to_string(),
        }
    }

    /// How it ended, in words: the outcome, and for one that did not go
    /// through, which part gave.
    fn result(&self) -> String {
        let restart_failed = self.restart.as_ref().is_some_and(|restart| !restart.ok);
        let check_failed = self.check.as_ref().is_some_and(|check| !check.ok);
        match self.kind {
            Outcome::Deployed => match self.rollback_of {
                Some(id) => format!("deployed (rollback to #{id})"),
                None => "deployed".to_string(),
            },
            Outcome::RolledBack if restart_failed => "restart failed, rolled back".to_string(),
            Outcome::RolledBack => "check failed, rolled back".to_string(),
            // The case the record is for: the files went out, and the service
            // still runs what it had.
            Outcome::Partial if restart_failed => "uploaded, restart failed".to_string(),
            Outcome::Partial if check_failed => "uploaded, check failed".to_string(),
            Outcome::Partial => "uploaded, then failed".to_string(),
            Outcome::Failed => "failed".to_string(),
        }
    }

    fn ago(&self) -> String {
        match output::parse_iso8601(&self.at) {
            Some(at) => format!("{} ago", human_span(now().saturating_sub(at))),
            None => self.at.clone(),
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// The table, oldest first: the newest deploy is the line above the prompt.
fn table(views: &[DeployView]) -> String {
    let mut rows = vec![["#", "WHEN", "TARGET", "VERSION", "COMMIT", "FILES", "SIZE", "TOOK", "RESULT"].map(str::to_string)];
    for view in views {
        rows.push([
            view.id.to_string(),
            view.ago(),
            view.label(),
            view.version.clone().unwrap_or_else(|| "-".to_string()),
            view.short_commit(),
            view.files.to_string(),
            human_bytes(view.bytes),
            human_duration(std::time::Duration::from_secs_f64(view.seconds)),
            view.result(),
        ]);
    }
    let columns = rows[0].len();
    let widths: Vec<usize> = (0..columns)
        .map(|column| rows.iter().map(|row| row[column].chars().count()).max().unwrap_or(0))
        .collect();
    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (column, cell) in row.iter().enumerate() {
                if column + 1 == columns {
                    line.push_str(cell);
                } else {
                    let pad = widths[column] - cell.chars().count();
                    line.push_str(cell);
                    line.push_str(&" ".repeat(pad + 2));
                }
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One deploy in full.
fn detail(view: &DeployView) -> String {
    let mut lines = vec![format!("Deploy #{} · {}: {} → {}", view.id, view.label(), view.app, view.server)];
    lines.push(format!("  When:      {} ({})", view.at, view.ago()));
    lines.push(format!("  Lands in:  {} ({})", view.path, view.dir));
    let mut build = Vec::new();
    build.extend(view.version.clone());
    build.extend(view.commit.as_ref().map(|_| view.short_commit()));
    build.push(format!(
        "{} file{}, {}",
        view.files,
        if view.files == 1 { "" } else { "s" },
        human_bytes(view.bytes)
    ));
    // Only a deploy that went through is one a rollback sends again; a
    // partial one keeps its build for the files it left, not as a way back.
    match (view.kind, view.build_kept) {
        (Outcome::Deployed, true) => build.push(format!("kept - `turnout rollback --to {}` sends it again", view.id)),
        (Outcome::Deployed | Outcome::Partial, false) => build.push("no longer kept".to_string()),
        _ => {}
    }
    lines.push(format!("  Build:     {}", build.join(" · ")));
    lines.push(format!("  Took:      {}", human_duration(std::time::Duration::from_secs_f64(view.seconds))));
    lines.push(format!("  Result:    {}", view.result()));
    if let Some(check) = &view.check {
        let word = if check.ok { "passed" } else { "failed" };
        lines.push(format!("  Check:     {word} - {}", check.answer));
    }
    if let Some(error) = &view.error {
        lines.push(format!("  Error:     {}", indent(error, "             ")));
    }
    if let Some(restart) = &view.restart {
        let word = if restart.ok { "ran" } else { "failed" };
        lines.push(format!("  Restart:   {} ({word})", restart.command));
        if !restart.output.trim().is_empty() {
            lines.push(format!("    {}", indent(restart.output.trim_end(), "    ")));
        }
    }
    lines.join("\n")
}

/// Every line after the first set under the first.
fn indent(text: &str, by: &str) -> String {
    text.lines().collect::<Vec<_>>().join(&format!("\n{by}"))
}

/// What `history --diff` reports.
#[derive(Serialize)]
struct CommitsView {
    app: String,
    from: End,
    to: End,
    /// In TO and not in FROM, newest first - what the later deploy brought.
    added: Vec<CommitLine>,
    /// In FROM and not in TO - what going from one to the other takes away,
    /// as a rollback does.
    removed: Vec<CommitLine>,
}

#[derive(Serialize)]
struct End {
    id: u64,
    commit: String,
    dirty: bool,
}

#[derive(Serialize)]
struct CommitLine {
    sha: String,
    subject: String,
}

impl CommitsView {
    fn text(&self) -> String {
        let short = |end: &End| {
            history::Commit {
                sha: end.commit.clone(),
                dirty: end.dirty,
            }
            .short()
        };
        let (from, to) = (&self.from, &self.to);
        let mut lines = Vec::new();
        if from.commit == to.commit {
            lines.push(format!("#{} and #{} were built from the same commit, {}.", from.id, to.id, short(to)));
        } else {
            let plural = |count: usize| if count == 1 { "" } else { "s" };
            lines.push(format!(
                "#{} ({}) → #{} ({}): {} commit{}",
                from.id,
                short(from),
                to.id,
                short(to),
                self.added.len(),
                plural(self.added.len())
            ));
            lines.extend(self.added.iter().map(|line| format!("  {} {}", line.sha, line.subject)));
            if !self.removed.is_empty() {
                lines.push(format!(
                    "and #{} does not have {} commit{} #{} had:",
                    to.id,
                    self.removed.len(),
                    plural(self.removed.len()),
                    from.id
                ));
                lines.extend(self.removed.iter().map(|line| format!("  {} {}", line.sha, line.subject)));
            }
        }
        for end in [from, to].into_iter().filter(|end| end.dirty) {
            lines.push(format!(
                "#{} was built with changes that were never committed, on top of {}.",
                end.id,
                &end.commit[..end.commit.len().min(7)]
            ));
        }
        lines.join("\n")
    }
}

/// The commits between the builds of two deploys of one app, asked of the
/// app's own repository.
fn commits_between(from: &Record, to: &Record) -> Result<CommitsView> {
    if from.app != to.app {
        bail!(
            "#{} deployed '{}' and #{} deployed '{}' - commits compare within one app",
            from.id,
            from.app,
            to.id,
            to.app
        );
    }
    let end = |record: &Record| {
        record.commit.as_ref().map(|commit| End {
            id: record.id,
            commit: commit.sha.clone(),
            dirty: commit.dirty,
        })
    };
    let (Some(start), Some(finish)) = (end(from), end(to)) else {
        let without = if from.commit.is_none() { from.id } else { to.id };
        bail!("#{without} has no commit - its project was not a git repository when it was deployed");
    };
    let app = crate::store::load_apps()?
        .into_iter()
        .find(|app| app.name == from.app)
        .ok_or_else(|| anyhow::anyhow!("app '{}' is not in the catalog any more - its repository is where the commits are", from.app))?;
    let project = crate::utils::project_dir(Path::new(&app.path))?;
    let added = log(&project, &start.commit, &finish.commit, from, to)?;
    let removed = log(&project, &finish.commit, &start.commit, from, to)?;
    Ok(CommitsView {
        app: app.name,
        from: start,
        to: finish,
        added,
        removed,
    })
}

/// `git log BASE..TIP`, as short SHAs and subjects.
fn log(project: &Path, base: &str, tip: &str, from: &Record, to: &Record) -> Result<Vec<CommitLine>> {
    let output = std::process::Command::new("git")
        .args(["log", "--no-decorate", "--format=%h%x09%s", &format!("{base}..{tip}")])
        .current_dir(project)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| format!("cannot run git in {}", project.display()))?;
    if !output.status.success() {
        bail!(
            "git cannot compare #{} and #{} in {}: {} - a commit rebased away, or never fetched here, is not in the repository",
            from.id,
            to.id,
            project.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(sha, subject)| CommitLine {
            sha: sha.to_string(),
            subject: subject.to_string(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::a_record;

    fn a_view(record: &Record) -> DeployView {
        let store = history::Store::at(std::env::temp_dir().join("turnout-no-such-store"));
        view(&store, record)
    }

    /// The case the record exists for: files up, restart failed - the table
    /// says so in so many words, not as a plain failure.
    #[test]
    fn the_result_names_the_part_that_gave() {
        let mut record = a_record(7, "/srv/site", Outcome::Partial, Some("abc"));
        record.restart = Some(Restart {
            command: "docker compose up -d".into(),
            ok: false,
            output: "`docker compose up -d` exited with 1: no space left".into(),
        });
        assert_eq!(a_view(&record).result(), "uploaded, restart failed");
        record.restart = None;
        record.check = Some(Check {
            ok: false,
            answer: "answered 503".into(),
        });
        assert_eq!(a_view(&record).result(), "uploaded, check failed");
        record.outcome = Outcome::RolledBack;
        assert_eq!(a_view(&record).result(), "check failed, rolled back");
        let mut rollback = a_record(8, "/srv/site", Outcome::Deployed, Some("abc"));
        rollback.rollback_of = Some(3);
        assert_eq!(a_view(&rollback).result(), "deployed (rollback to #3)");
    }

    /// One line per deploy, columns aligned, and the restart's output in the
    /// detail - the record keeps what the deploy's checklist printed.
    #[test]
    fn the_table_and_the_detail() {
        let mut first = a_record(1, "/srv/site", Outcome::Deployed, None);
        first.version = Some("1.4.0".into());
        first.commit = Some(history::Commit {
            sha: "1a2b3c4d5e6f".into(),
            dirty: true,
        });
        let second = a_record(12, "/srv/site", Outcome::Failed, None);
        let text = table(&[a_view(&first), a_view(&second)]);
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("#   WHEN"), "{text}");
        assert!(
            lines[1].contains("1.4.0") && lines[1].contains("1a2b3c4-dirty") && lines[1].ends_with("deployed"),
            "{text}"
        );
        assert!(lines[2].starts_with("12  ") && lines[2].ends_with("failed"), "{text}");
        assert_eq!(lines[1].find("web-stand"), lines[2].find("web-stand"), "columns line up:\n{text}");

        first.restart = Some(Restart {
            command: "systemctl restart site".into(),
            ok: true,
            output: "Restarting\nStarted".into(),
        });
        let mut partial = a_view(&a_record(13, "/srv/site", Outcome::Partial, Some("abc")));
        partial.build_kept = true;
        assert!(
            !detail(&partial).contains("rollback --to"),
            "a rollback refuses a partial deploy, so its card must not offer one: {}",
            detail(&partial)
        );
        let shown = detail(&a_view(&first));
        assert!(shown.contains("Restart:   systemctl restart site (ran)"), "{shown}");
        assert!(shown.contains("    Restarting\n    Started"), "{shown}");
        assert!(shown.contains("Build:     1.4.0 · 1a2b3c4-dirty · 3 files"), "{shown}");
    }
}
