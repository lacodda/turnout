use anyhow::{Result, bail};

use crate::{front, store};

/// Open the app in the browser by its name behind the front door.
pub fn run(app: Option<String>) -> Result<()> {
    let apps = store::load_apps()?;
    let app = crate::commands::exec::resolve(&apps, app)?;
    let state = store::load_state()?;
    let Some(gateway) = state.gateway.as_ref().filter(|gateway| crate::commands::gateway::probe(gateway)) else {
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
