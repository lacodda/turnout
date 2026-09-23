//! Knowing a process again, and ending it.
//!
//! The job registry keeps pids, and a pid is only a number: after the process
//! it named is gone, the OS hands the same number to the next process that
//! starts. A registry that trusts the number alone reports a dead dev server
//! as running because some unrelated program now has its pid - and `stop`
//! kills that program, tree and all. After a reboot that is not an edge case,
//! it is the usual state of every record.
//!
//! So every record carries the process's *birth*: the moment the OS says it
//! started, in the OS's own units. A pid names the recorded process only while
//! the process behind it has the same birth. Nothing in turnout signals a pid
//! without asking that first.

use anyhow::{Result, bail};

/// When the process with `pid` started, or `None` when there is no such
/// process (or it has exited and only its corpse is left).
///
/// The value is opaque and only ever compared with itself: a FILETIME on
/// Windows, clock ticks since boot on Linux, microseconds since the epoch on
/// macOS.
pub fn birth(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    imp::birth(pid)
}

/// Whether `pid` still names the process that was born at `born`.
pub fn is_alive(pid: u32, born: u64) -> bool {
    birth(pid) == Some(born)
}

/// This process's own birth, for the record it is about to write.
pub fn own_birth() -> u64 {
    // A running process can always read its own start time; the fallback only
    // exists so a platform quirk degrades into "never matches" rather than a
    // panic in the middle of a command.
    birth(std::process::id()).unwrap_or_default()
}

/// The process group this process leads, if it leads one (Unix).
///
/// A shell with job control puts every command it starts into a group of its
/// own, and the terminal's Ctrl+C goes to exactly that group - turnout and
/// everything it started. Signalling it from elsewhere is therefore the same
/// as pressing Ctrl+C in that terminal. When turnout does *not* lead its group
/// (a script, a pipeline), the group is somebody else's, and `None` says so:
/// signalling it would take the caller down too.
pub fn own_group() -> Option<i32> {
    imp::own_group()
}

/// How a recorded job is ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ending {
    /// The job runs in a terminal of its own: end it the way Ctrl+C there
    /// would, so that turnout gets to put that terminal back.
    Interrupt,
    /// The job runs in the background: end it outright.
    Terminate,
    /// It was asked and did not go.
    Kill,
}

/// Make a command start detached: out of this terminal and its Ctrl+C, and
/// alive after turnout returns.
///
/// Windows: a console of its own that nobody sees, and a process group of its
/// own. Not `DETACHED_PROCESS` - that leaves the child with no console at
/// all, and the first console program it starts (`cmd`, `node`) is given a
/// fresh, visible window. Unix: a session of its own (`setsid`), so the
/// terminal closing does not hang it up, and so it leads a process group that
/// `stop` can signal whole.
pub fn detach(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe, which is all pre_exec asks.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// End the process `pid` and everything it started.
///
/// `group` is the process group to signal on Unix; without one only `pid`
/// itself is signalled. On Windows the tree is found by parentage and killed
/// whole - a console process in another console cannot be sent a Ctrl+C.
///
/// Refuses numbers that cannot name a process before they reach the OS: `kill`
/// reads a pid as a C `int`, so 4294967295 arrives as -1, and signalling -1
/// means every process the user owns. That is how a CI runner died once.
pub fn end(pid: u32, group: Option<i32>, how: Ending) -> Result<()> {
    if pid <= 1 || pid > i32::MAX as u32 {
        bail!("refusing to signal pid {pid}: not a process id");
    }
    if let Some(group) = group
        && group <= 1
    {
        bail!("refusing to signal process group {group}: not a group of ours");
    }
    imp::end(pid, group, how)
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result, bail};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    use super::Ending;

    pub fn birth(pid: u32) -> Option<u64> {
        // SAFETY: the handle is checked before use and closed on every path;
        // the out-parameters are plain structs owned by this frame.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            // A process that has exited keeps its object - and its pid - for as
            // long as anyone holds a handle to it. That is a corpse, not a job.
            let mut code = 0u32;
            let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let timed = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            CloseHandle(handle);
            (running && timed).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
        }
    }

    pub fn own_group() -> Option<i32> {
        None
    }

    pub fn end(pid: u32, _group: Option<i32>, _how: Ending) -> Result<()> {
        let output = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output()
            .context("cannot run taskkill")?;
        if !output.status.success() {
            bail!("taskkill failed: {}", String::from_utf8_lossy(&output.stderr).trim());
        }
        Ok(())
    }
}

#[cfg(unix)]
mod imp {
    use anyhow::Result;

    use super::Ending;

    #[cfg(target_os = "linux")]
    pub fn birth(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat(&stat)
    }

    /// The start time out of `/proc/PID/stat`, or `None` for a zombie.
    ///
    /// The second field is the command name in parentheses, and a name may
    /// itself contain spaces and parentheses - so fields are counted from the
    /// *last* closing parenthesis. After it come the state (field 3) and, 19
    /// fields later, the start time (field 22).
    #[cfg(any(target_os = "linux", test))]
    pub fn parse_stat(stat: &str) -> Option<u64> {
        let rest = &stat[stat.rfind(')')? + 1..];
        let mut fields = rest.split_whitespace();
        let state = fields.next()?;
        if state == "Z" || state == "X" {
            return None;
        }
        fields.nth(18)?.parse().ok()
    }

    #[cfg(target_os = "macos")]
    pub fn birth(pid: u32) -> Option<u64> {
        // SAFETY: proc_pidinfo writes at most `size` bytes into `info`, which
        // is a zeroed struct of exactly that size owned by this frame.
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            let written = libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, std::ptr::from_mut(&mut info).cast(), size);
            if written != size || info.pbi_status == libc::SZOMB {
                return None;
            }
            Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
        }
    }

    /// Other Unixes: alive or not, with no way to tell a reused pid apart.
    /// Every such pid reads as born at zero, which the record then carries.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn birth(pid: u32) -> Option<u64> {
        // SAFETY: signal 0 only checks that the pid exists.
        (unsafe { libc::kill(pid as libc::pid_t, 0) } == 0).then_some(0)
    }

    pub fn own_group() -> Option<i32> {
        // SAFETY: both calls only read this process's own ids.
        let (group, own) = unsafe { (libc::getpgrp(), libc::getpid()) };
        (group == own).then_some(group)
    }

    pub fn end(pid: u32, group: Option<i32>, how: Ending) -> Result<()> {
        let signal = match how {
            Ending::Interrupt => libc::SIGINT,
            Ending::Terminate => libc::SIGTERM,
            Ending::Kill => libc::SIGKILL,
        };
        // A negative pid addresses the group; `end` has already refused every
        // number that would turn into "all processes" here.
        let target = group.map_or(pid as libc::pid_t, |group| -group);
        // SAFETY: plain kill(2) on a number validated by the caller.
        if unsafe { libc::kill(target, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            anyhow::bail!(
                "cannot signal {}: {error}",
                group.map_or(format!("pid {pid}"), |group| format!("process group {group}"))
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The process running this test is alive, keeps one birth, and is known
    /// again by it.
    #[test]
    fn a_live_process_is_known_by_its_birth() {
        let pid = std::process::id();
        let born = birth(pid).expect("this process is running");
        assert_eq!(birth(pid), Some(born));
        assert!(is_alive(pid, born));
        // The same pid with any other birth is some other process.
        assert!(!is_alive(pid, born.wrapping_add(1)));
    }

    /// A pid no process has is not alive, and neither is zero.
    #[test]
    fn a_pid_nobody_has_is_not_alive() {
        assert_eq!(birth(0), None);
        assert_eq!(birth(i32::MAX as u32), None);
    }

    /// A child that has exited is dead, even while this process still holds
    /// its handle (Windows) or has not reaped it yet (Unix).
    #[test]
    fn an_exited_child_is_not_alive() {
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "exit 0"]);
            command
        } else {
            std::process::Command::new("true")
        };
        let child = command.spawn().unwrap();
        let pid = child.id();
        // Poll without reaping: the corpse must already read as dead.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while birth(pid).is_some() {
            assert!(std::time::Instant::now() < deadline, "pid {pid} still reads as alive after exiting");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(child);
    }

    /// Numbers that cannot name a process or one of our groups never reach
    /// the OS; the message says so rather than whatever the tool made of them.
    #[test]
    fn end_refuses_a_number_that_is_not_a_pid() {
        for pid in [0, 1, u32::MAX, i32::MAX as u32 + 1] {
            let error = end(pid, None, Ending::Terminate).unwrap_err().to_string();
            assert!(error.contains("not a process id"), "pid {pid}: {error}");
        }
        for group in [-1, 0, 1] {
            let error = end(4242, Some(group), Ending::Terminate).unwrap_err().to_string();
            assert!(error.contains("not a group of ours"), "group {group}: {error}");
        }
    }

    /// `/proc/PID/stat` is read from the last parenthesis, so a command name
    /// with spaces and brackets in it does not shift the fields.
    #[cfg(unix)]
    #[test]
    fn the_stat_line_is_read_past_a_hostile_name() {
        let fields = |state: &str| format!("4242 (a) b (c) {state} 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 987654 20 21");
        assert_eq!(imp::parse_stat(&fields("S")), Some(987_654));
        assert_eq!(imp::parse_stat(&fields("R")), Some(987_654));
        assert_eq!(imp::parse_stat(&fields("Z")), None);
        assert_eq!(imp::parse_stat("garbage"), None);
    }
}
