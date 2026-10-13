use anyhow::{Result, bail};

use crate::{front, store};

/// Open something in the browser: a target's stand, or an app by its name
/// behind the front door.
///
/// A target is tried first, the way `deploy` and `ssh` read a name: it names
/// a whole route, and an app that shares the name is still reachable through
/// the current directory.
pub fn run(name: Option<String>) -> Result<()> {
    if let Some(name) = &name
        && let Some(target) = store::load_targets()?.into_iter().find(|target| &target.name == name)
    {
        let server = store::load_servers()?.into_iter().find(|server| server.name == target.server).ok_or_else(|| {
            anyhow::anyhow!(
                "target '{name}' names server '{}', which is not in the catalog - see `turnout server list`",
                target.server
            )
        })?;
        let address = target.address(&server);
        println!("Opening {address}");
        return front::open_in_browser(&address);
    }
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, name)?;
    let Some(gateway) = crate::registry::gateway()?.filter(crate::commands::gateway::probe) else {
        bail!(
            "the gateway is not running - start it with `turnout gateway start`, then `turnout open {}`",
            app.name
        );
    };
    let Some(front_port) = gateway.front_port else {
        bail!(
            "the gateway is running without its front door (ports {} and {} were taken when it started) - set {} and restart it",
            front::PORT,
            front::FALLBACK_PORT,
            front::ENV_PORT
        );
    };
    let url = front::address(&app.name, front_port);
    if app.dev_port.is_none() {
        eprintln!(
            "note: '{0}' has not been started through turnout yet - the page will say so until `turnout dev {0}` runs",
            app.name
        );
    }
    println!("Opening {url}");
    front::open_in_browser(&url)
}
