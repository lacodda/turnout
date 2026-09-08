use anyhow::Result;

use crate::remote;

/// Run one command on a server, in its deploy directory when the name is a
/// target, streaming the output and exiting with the remote code.
pub fn run(name: Option<String>, credential: Option<String>, dir: Option<String>, command: Vec<String>) -> Result<()> {
    let host = remote::resolve_host(name, credential)?;
    let command = command.join(" ");
    // Announced before dialing, so a refused connection still says where it
    // was going and as whom.
    let dir = dir.or_else(|| host.dir.clone());
    match &dir {
        Some(dir) => eprintln!("[{}] {dir}$ {command}", host.label()),
        None => eprintln!("[{}] {command}", host.label()),
    }
    let session = remote::connect(&host.server, &host.credential)?;
    // An explicit --dir wins over the target's; a plain server name has no
    // directory and the command runs where the login lands.
    let remote_command = match &dir {
        Some(dir) => {
            let dialect = remote::dialect(&session, &host.server);
            remote::check_quotable(dialect, &[dir])?;
            dialect.run_in(dir, &command)
        }
        None => command.clone(),
    };
    let code = session.run(&remote_command)?;
    crate::journal::record("exec", Some(&host.server.name), None, Some(&command));
    // Transparent in scripts: the remote exit code is ours.
    std::process::exit(code as i32);
}
