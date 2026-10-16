//! `rollback`: put a stand back on the build an earlier deploy sent.
//!
//! The build goes out again from this machine's kept copy, through the same
//! checked deploy as any other - a backup when the target checks, the restart,
//! the check, and a rollback of the rollback if that fails (ADR 0027). What a
//! server-side backup would also put back - uploads, a database's files in the
//! deploy directory - is left as it is: a rollback is about the code.

use anyhow::{Result, bail};

use crate::artifact::{self, Artifact};
use crate::commands::deploy::{self, After, Options, Shipment};
use crate::history::{self, Outcome, Record, Source};
use crate::progress;
use crate::remote::{self, Overrides, Resolved};
use crate::store;

pub fn run(target: Option<String>, to: Option<String>) -> Result<()> {
    let store = history::Store::open()?;
    let records = store.all()?;
    let (record, route) = match to {
        Some(to) => {
            let id = history::parse_id(&to)?;
            let record = records
                .iter()
                .find(|record| record.id == id)
                .ok_or_else(|| anyhow::anyhow!("no deploy #{id} is recorded on this machine - see `turnout history`"))?;
            let route = route_of(record)?;
            if let Some(name) = target {
                let named = remote::resolve(Some(name.clone()), Overrides::default())?;
                if !record.on(&named.server.name, &named.path.dir) {
                    bail!(
                        "deploy #{id} went to {} on '{}', not to where '{name}' deploys ({} on '{}')",
                        record.dir,
                        record.server,
                        named.path.dir,
                        named.server.name
                    );
                }
            }
            (record, route)
        }
        None => {
            let route = remote::resolve(target, Overrides::default())?;
            (history::previous(&records, &route.server.name, &route.path.dir)?, route)
        }
    };
    if record.outcome != Outcome::Deployed {
        bail!(
            "deploy #{} never went through ({}), so there is no build of it a stand ran - pick one that did from `turnout history`",
            record.id,
            record.outcome.word()
        );
    }
    let digest = record.build.as_deref().unwrap_or_default();
    let Some(kept) = store.build(digest) else {
        bail!(
            "the build #{} sent is no longer kept - turnout keeps the builds of the last {} deploys to each stand",
            record.id,
            history::KEEP_BUILDS
        );
    };
    let archive = std::fs::read(&kept)?;

    // The build's files, as a deploy walks them: the upload is the same code
    // path whether it goes as this archive or file by file.
    let unpacked = Scratch::new(record.id)?;
    artifact::unpack(&archive, &unpacked.0)?;
    let plan = artifact::plan_upload(&unpacked.0)?;
    let own = artifact::files_in(&archive)?;
    let (remove, unknown) = history::added_since(&store, &records, record, &own)?;
    let source = Source {
        version: record.version.clone(),
        commit: record.commit.clone(),
    };
    let build = Artifact::packed(&route.app.name, plan, archive, source);

    println!("{}", describe(record));
    if !unknown.is_empty() {
        let numbers: Vec<String> = unknown.iter().map(|id| format!("#{id}")).collect();
        progress::warn(&format!(
            "the builds of {} are no longer kept, so the files only they added stay on the stand",
            numbers.join(", ")
        ));
    }
    let shipment = Shipment {
        artifact: &build,
        remove,
        rollback_of: Some(record.id),
    };
    deploy::deploy_one(&route, &shipment, &Options::default(), After { open: false, offer: false })
}

/// The route a recorded deploy took, as the catalogs name it today.
///
/// Through its target when the target still leads to the same stand; else
/// through the app, server, credential and path it named. Either way the
/// stand has to be the one the deploy went to: a path whose directory moved
/// since would send the old build somewhere it never ran.
fn route_of(record: &Record) -> Result<Resolved> {
    let by_target = match &record.target {
        Some(name) if store::load_targets()?.iter().any(|target| &target.name == name) => {
            Some(remote::resolve(Some(name.clone()), Overrides::default())?).filter(|route| record.on(&route.server.name, &route.path.dir))
        }
        _ => None,
    };
    let route = match by_target {
        Some(route) => route,
        None => remote::resolve(
            Some(record.app.clone()),
            Overrides {
                server: Some(record.server.clone()),
                credential: Some(record.credential.clone()),
                path: Some(record.path.clone()),
            },
        )?,
    };
    if route.app.name != record.app || !record.on(&route.server.name, &route.path.dir) {
        bail!(
            "deploy #{} went to {} on '{}', and its path '{}' leads to {} on '{}' now - the build would land somewhere it never ran",
            record.id,
            record.dir,
            record.server,
            record.path,
            route.path.dir,
            route.server.name
        );
    }
    Ok(route)
}

/// The line that says which build is going back: `#12 · 1.4.0 · 1a2b3c4 ·
/// deployed 3d 04h ago`.
fn describe(record: &Record) -> String {
    let mut parts = vec![format!("#{}", record.id)];
    parts.extend(record.version.clone());
    parts.extend(record.commit.as_ref().map(history::Commit::short));
    if let Some(at) = crate::output::parse_iso8601(&record.at) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or(at);
        parts.push(format!("deployed {} ago", progress::human_span(now.saturating_sub(at))));
    }
    format!("Sending the build of {} again", parts.join(" · "))
}

/// A scratch directory for an unpacked build, gone when it goes out of scope.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(id: u64) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!("turnout-rollback-{}-{id}", std::process::id()));
        // Left over by a run that was killed: its files would join this build.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
