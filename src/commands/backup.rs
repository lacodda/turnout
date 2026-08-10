use anyhow::{Result, bail};

use crate::progress::Step;
use crate::remote;

pub fn backup(app_name: Option<String>, server_name: Option<String>) -> Result<()> {
    let target = remote::resolve(app_name, server_name)?;
    let ssh = remote::require_ssh(&target.server)?;
    let deploy = remote::require_deploy_target(&target.server, &target.app.name)?;
    let step = Step::start(format!("Connecting to {}@{}:{} ...", ssh.user, ssh.host, ssh.port));
    let session = remote::connect(ssh, &target.server.name)?;
    step.update(format!("Backing up {} ...", deploy.path));
    let name = remote::exec(&session, &remote::backup_command(&deploy.path))?;
    step.done(format!("Backup {} created in {}", name.trim(), remote::backups_dir(&deploy.path)));
    crate::journal::record("backup", Some(&target.app.name), Some(&target.server.name), Some(name.trim()));
    Ok(())
}

pub fn restore(app_name: Option<String>, server_name: Option<String>, from: Option<String>, list: bool) -> Result<()> {
    let target = remote::resolve(app_name, server_name)?;
    let ssh = remote::require_ssh(&target.server)?;
    let deploy = remote::require_deploy_target(&target.server, &target.app.name)?;
    let backups_raw = remote::backups_dir(&deploy.path);
    let backups = remote::shell_quote(&backups_raw);
    let step = Step::start(format!("Connecting to {}@{}:{} ...", ssh.user, ssh.host, ssh.port));
    let session = remote::connect(ssh, &target.server.name)?;
    step.clear();

    if list {
        let output = remote::exec(&session, &format!("ls -1 {backups} 2>/dev/null || true"))?;
        if output.trim().is_empty() {
            println!("No backups yet in {backups_raw} - create one with `turnout backup {}`.", target.app.name);
        } else {
            println!("{}", output.trim_end());
        }
        return Ok(());
    }

    let name = match from {
        Some(name) => name,
        None => {
            let latest = remote::exec(&session, &format!("ls -1 {backups} 2>/dev/null | sort | tail -n 1"))?;
            let latest = latest.trim().to_string();
            if latest.is_empty() {
                bail!("no backups in {backups_raw} - create one with `turnout backup {}`", target.app.name);
            }
            latest
        }
    };
    let archive = remote::shell_quote(&format!("{backups_raw}/{name}"));
    let dir = remote::shell_quote(deploy.path.trim_end_matches('/'));
    let step = Step::start(format!("Restoring {name} into {} ...", deploy.path));
    remote::exec(
        &session,
        &format!("test -f {archive} && find {dir} -mindepth 1 -maxdepth 1 -exec rm -rf {{}} + && tar xzf {archive} -C {dir}"),
    )?;
    step.done(format!("Restored {name} into {}", deploy.path));
    crate::journal::record("restore", Some(&target.app.name), Some(&target.server.name), Some(&name));
    if let Some(restart) = &deploy.restart {
        let step = Step::start(format!("Running: {restart}"));
        let output = remote::exec(&session, restart)?;
        step.done(format!("Ran: {restart}"));
        if !output.trim().is_empty() {
            println!("{}", output.trim_end());
        }
    }
    Ok(())
}
