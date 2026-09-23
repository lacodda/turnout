use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cli::GatewayCommand;
use crate::model::Gateway;
use crate::registry::{self, Work};
use crate::{gateway, process, store};

pub fn run(command: GatewayCommand) -> Result<()> {
    match command {
        GatewayCommand::Start => start(),
        GatewayCommand::Run { front_port, log } => {
            // Same guard as `start`: a raw bind error (os error 10048) is cryptic.
            if let Some(running) = registry::gateway()?
                && probe(&running)
            {
                bail!("the gateway is already running (pid {}) - stop it with `turnout gateway stop`", running.pid);
            }
            gateway::run(front_port, log)
        }
        GatewayCommand::Stop => crate::commands::jobs::stop(Some(registry::GATEWAY.to_string()), None),
    }
}

/// How long a freshly spawned gateway gets to record itself and answer on its
/// first port. A local bind takes milliseconds; the margin is for a cold start
/// on a busy machine, not for a gateway that is actually stuck.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// How much of the gateway's log a failed start replays.
const TAIL_LINES: usize = 20;

/// `gateway start`: `gateway run` as a background job.
///
/// The gateway writes its own record once its ports are bound (see
/// [`gateway::run`]), so a record is a gateway that got as far as listening -
/// never one that died on its first bind.
fn start() -> Result<()> {
    let apps = store::load_apps()?;
    let ports = gateway::listening_ports(&apps)?;
    if let Some(running) = registry::gateway()? {
        bail!("the gateway is already running (pid {})", running.pid);
    }

    // Ports answering now belong to something else: the registry says no
    // gateway of ours is alive. Refusing here names the port and the app; the
    // child would only fail its bind and exit with nothing to show for it.
    for (port, app) in &ports {
        if port_answers(*port) {
            bail!("port {port} is already in use by another process - free it, or give '{app}' another port with `turnout app edit {app} --port PORT`");
        }
    }

    // The door's port is decided here and handed to the child, so the same
    // pick is not made twice with two different answers.
    let front_port = crate::front::pick_port();
    let log = registry::log_path(registry::GATEWAY)?;
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let output = std::fs::File::create(&log).with_context(|| format!("cannot write {}", log.display()))?;
    let exe = std::env::current_exe().context("cannot locate the turnout binary")?;
    let mut command = Command::new(exe);
    command.args(["gateway", "run"]);
    if let Some(port) = front_port {
        command.args(["--front-port", &port.to_string()]);
    }
    command.arg("--log").arg(&log);
    command
        .stdin(Stdio::null())
        .stdout(output.try_clone().context("cannot share the gateway log")?)
        .stderr(output);
    process::detach(&mut command);
    crate::utils::stop_inheriting_stdio();
    let mut child = command.spawn().context("cannot start the gateway process")?;

    let started = Instant::now();
    let gateway = loop {
        if let Some(status) = child.try_wait().context("cannot check on the gateway process")? {
            replay(&log);
            bail!("the gateway exited right after starting ({status}) - its output is above, and in `turnout logs gateway`");
        }
        if let Some(gateway) = registry::gateway()?.filter(|gateway| gateway.pid == child.id())
            && probe(&gateway)
        {
            break gateway;
        }
        if started.elapsed() > START_TIMEOUT {
            let _ = process::end(child.id(), None, process::Ending::Kill);
            let _ = registry::remove(registry::GATEWAY);
            replay(&log);
            bail!(
                "the gateway did not answer on port {} within {}s - run `turnout gateway run` in the foreground to see why",
                ports.keys().next().copied().unwrap_or_default(),
                START_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    crate::journal::record("gateway.start", None, None, Some(&format!("{} apps", gateway.ports.len())));
    println!("Gateway started (pid {}).", gateway.pid);
    for (port, app) in &gateway.ports {
        println!("  {app}: http://localhost:{port}");
    }
    match gateway.front_port {
        Some(front) => {
            println!("Front door: {}", crate::front::door(front));
            for app in &apps {
                println!("  {}: {}", app.name, crate::front::address(&app.name, front));
            }
        }
        None => println!(
            "Front door: closed - ports {} and {} are taken; set {} to open it elsewhere",
            crate::front::PORT,
            crate::front::FALLBACK_PORT,
            crate::front::ENV_PORT
        ),
    }
    // The dotenv files are the road for apps started past turnout; a port
    // changed since the last `app edit` is caught up here.
    for app in apps.iter().filter(|app| app.gateway_port.is_some()) {
        match crate::envfile::write(app) {
            Ok(crate::envfile::Outcome::Written(assignment)) => println!("  {}: wrote {} ({assignment})", app.name, app.env_file_name()),
            Ok(_) => {}
            Err(error) => eprintln!("warning: {}: {error:#}", app.name),
        }
    }
    Ok(())
}

/// Put what the gateway said on the way down in front of the error about it.
fn replay(log: &std::path::Path) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for line in &lines[lines.len().saturating_sub(TAIL_LINES)..] {
        eprintln!("{line}");
    }
}

/// The record `gateway run` writes about itself once it listens.
pub fn record(ports: std::collections::BTreeMap<u16, String>, front_port: Option<u16>, log: Option<std::path::PathBuf>) -> Result<()> {
    let detached = log.is_some();
    registry::save(&registry::Entry::own(Work::Gateway { ports, front_port }, detached, log))
}

/// Quick liveness check: can we open one of the recorded ports?
pub fn probe(gateway: &Gateway) -> bool {
    gateway.ports.keys().next().is_some_and(|port| port_answers(*port))
}

fn port_answers(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}
