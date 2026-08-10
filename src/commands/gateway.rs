use std::collections::BTreeMap;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::cli::GatewayCommand;
use crate::model::Gateway;
use crate::{gateway, store};

pub fn run(command: GatewayCommand) -> Result<()> {
    match command {
        GatewayCommand::Start => start(),
        GatewayCommand::Run => {
            // Same guard as `start`: a raw bind error (os error 10048) is cryptic.
            if let Some(running) = &store::load_state()?.gateway
                && probe(running)
            {
                bail!("the gateway is already running (pid {}) - stop it with `turnout gateway stop`", running.pid);
            }
            gateway::run()
        }
        GatewayCommand::Stop => stop(),
    }
}

fn start() -> Result<()> {
    let ports: BTreeMap<u16, String> = store::load_apps()?
        .into_iter()
        .filter_map(|app| app.gateway_port.map(|port| (port, app.name)))
        .collect();
    if ports.is_empty() {
        bail!("no apps with a gateway port - set one with `turnout app edit NAME --port PORT`");
    }
    let mut state = store::load_state()?;
    if let Some(running) = &state.gateway
        && probe(running)
    {
        bail!("the gateway is already running (pid {})", running.pid);
    }

    let exe = std::env::current_exe().context("cannot locate the turnout binary")?;
    let mut command = Command::new(exe);
    command
        .args(["gateway", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let child = command.spawn().context("cannot start the gateway process")?;

    state.gateway = Some(Gateway {
        pid: child.id(),
        ports: ports.clone(),
    });
    store::save_state(&state)?;
    crate::journal::record("gateway.start", None, None, Some(&format!("{} apps", ports.len())));
    println!("Gateway started (pid {}).", child.id());
    for (port, app) in ports {
        println!("  {app}: http://localhost:{port}");
    }
    Ok(())
}

fn stop() -> Result<()> {
    let mut state = store::load_state()?;
    let Some(running) = state.gateway.take() else {
        println!("The gateway is not running.");
        return Ok(());
    };
    kill(running.pid)?;
    store::save_state(&state)?;
    crate::journal::record("gateway.stop", None, None, None);
    println!("Gateway stopped (pid {}).", running.pid);
    Ok(())
}

/// Quick liveness check: can we open one of the recorded ports?
pub fn probe(gateway: &Gateway) -> bool {
    gateway.ports.keys().next().is_some_and(|port| {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], *port));
        std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(300)).is_ok()
    })
}

#[cfg(windows)]
fn kill(pid: u32) -> Result<()> {
    let output = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .context("cannot run taskkill")?;
    if !output.status.success() {
        bail!("taskkill failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(not(windows))]
fn kill(pid: u32) -> Result<()> {
    let output = Command::new("kill").arg(pid.to_string()).output().context("cannot run kill")?;
    if !output.status.success() {
        bail!("kill failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}
