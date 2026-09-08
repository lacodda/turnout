use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

/// How long a freshly spawned gateway gets to answer on its first port.
/// A local bind takes milliseconds; the margin is for a cold start on a
/// busy machine, not for a gateway that is actually stuck.
const START_TIMEOUT: Duration = Duration::from_secs(5);

fn start() -> Result<()> {
    let apps = store::load_apps()?;
    let ports = gateway::listening_ports(&apps)?;
    let mut state = store::load_state()?;
    if let Some(running) = &state.gateway
        && probe(running)
    {
        bail!("the gateway is already running (pid {})", running.pid);
    }

    // Ports answering now belong to something else: the record above says no
    // gateway of ours is alive. Refusing here names the port and the app; the
    // child would only fail its bind and exit with nothing to show for it.
    for (port, app) in &ports {
        if port_answers(*port) {
            bail!("port {port} is already in use by another process - free it, or give '{app}' another port with `turnout app edit {app} --port PORT`");
        }
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
    crate::utils::stop_inheriting_stdio();
    let mut child = command.spawn().context("cannot start the gateway process")?;

    // Do not take the spawn for the start. The child binds its ports after
    // this returns; when one is taken it exits at once, and recording its pid
    // would leave `status` calling a dead process alive and `stop` failing
    // on it. Wait for the first port to answer, or for the child to give up.
    let gateway = Gateway {
        pid: child.id(),
        ports: ports.clone(),
    };
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().context("cannot check on the gateway process")? {
            bail!("the gateway exited right after starting ({status}) - run `turnout gateway run` in the foreground to see why");
        }
        if probe(&gateway) {
            break;
        }
        if started.elapsed() > START_TIMEOUT {
            let _ = kill(child.id());
            bail!(
                "the gateway did not answer on port {} within {}s - run `turnout gateway run` in the foreground to see why",
                gateway.ports.keys().next().copied().unwrap_or_default(),
                START_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    state.gateway = Some(gateway);
    store::save_state(&state)?;
    crate::journal::record("gateway.start", None, None, Some(&format!("{} apps", ports.len())));
    println!("Gateway started (pid {}).", child.id());
    for (port, app) in ports {
        println!("  {app}: http://localhost:{port}");
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

fn stop() -> Result<()> {
    let mut state = store::load_state()?;
    let Some(running) = state.gateway.take() else {
        println!("The gateway is not running.");
        return Ok(());
    };
    // A record whose process is gone (killed from outside, or died on its
    // own) is stale, not an error: forget it rather than fail on the kill and
    // leave the record to fail the same way next time.
    if let Err(error) = kill(running.pid) {
        if probe(&running) {
            return Err(error);
        }
        store::save_state(&state)?;
        println!("The gateway (pid {}) was no longer running - cleared the stale record.", running.pid);
        return Ok(());
    }
    store::save_state(&state)?;
    crate::journal::record("gateway.stop", None, None, None);
    println!("Gateway stopped (pid {}).", running.pid);
    Ok(())
}

/// Quick liveness check: can we open one of the recorded ports?
pub fn probe(gateway: &Gateway) -> bool {
    gateway.ports.keys().next().is_some_and(|port| port_answers(*port))
}

fn port_answers(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_millis(300)).is_ok()
}

/// Signal the recorded gateway process.
///
/// The pid comes from `state.json`, which a hand edit or a corrupt write can
/// turn into anything. Numbers that cannot name a process are refused before
/// they reach a tool that would read them differently: `kill` parses the pid
/// into a C `int`, so 4294967295 arrives as -1, and `kill -1` signals every
/// process the user owns. That is how the whole CI runner died once.
fn kill(pid: u32) -> Result<()> {
    if pid <= 1 || pid > i32::MAX as u32 {
        bail!("refusing to signal pid {pid}: not a process id");
    }
    kill_process(pid)
}

#[cfg(windows)]
fn kill_process(pid: u32) -> Result<()> {
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
fn kill_process(pid: u32) -> Result<()> {
    let output = Command::new("kill").arg(pid.to_string()).output().context("cannot run kill")?;
    if !output.status.success() {
        bail!("kill failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Numbers that cannot name a process never reach the OS tool; the
    /// message says so rather than reporting whatever the tool made of them.
    #[test]
    fn kill_refuses_a_number_that_is_not_a_pid() {
        for pid in [0, 1, u32::MAX, i32::MAX as u32 + 1] {
            let error = kill(pid).unwrap_err().to_string();
            assert!(error.contains("not a process id"), "pid {pid}: {error}");
        }
    }
}
