//! Who holds a port: the process listening on it.
//!
//! "The port is taken" is half an answer. A dev server left running by a
//! terminal that closed holds its port for as long as the machine is up; the
//! next `dev` lands on another port (Vite moves on to the next free one), and
//! the front door keeps showing the old server - yesterday's code, with no
//! error anywhere. What ends the confusion is a name and a number: `node.exe,
//! pid 4242`. The OS keeps the table that says it; this module reads it.
//!
//! Both loopbacks count. Node resolves `localhost` to `::1` first, so a dev
//! server often listens on IPv6 only - and a probe of `127.0.0.1` alone calls
//! that port free while it is not.

/// The process listening on a port, as far as the OS will say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// `None` when the port is listened on but the process is not ours to see
    /// (another user's, on Linux).
    pub pid: Option<u32>,
    /// The program's file name: `node.exe`, `python3`.
    pub name: Option<String>,
}

impl Holder {
    /// `node.exe (pid 4242)`, or as much of it as is known.
    pub fn describe(&self) -> String {
        match (&self.name, self.pid) {
            (Some(name), Some(pid)) => format!("{name} (pid {pid})"),
            (None, Some(pid)) => format!("pid {pid}"),
            (Some(name), None) => name.clone(),
            (None, None) => "a process turnout cannot see".to_string(),
        }
    }
}

/// The process listening on TCP `port`, on any address, or `None` when
/// nothing listens there.
pub fn holder(port: u16) -> Option<Holder> {
    imp::holder(port)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};

    use super::Holder;

    /// The address families, as winsock numbers them.
    const AF_INET: u32 = 2;
    const AF_INET6: u32 = 23;

    pub fn holder(port: u16) -> Option<Holder> {
        let pid = owner(AF_INET, port).or_else(|| owner(AF_INET6, port))?;
        Some(Holder {
            pid: Some(pid),
            name: image_name(pid),
        })
    }

    /// The pid that owns a listener on `port` in the family's table.
    fn owner(family: u32, port: u16) -> Option<u32> {
        let table = listeners(family)?;
        // The table starts with its entry count; the rows follow, aligned as
        // the row type is (four bytes - every field is a u32 or bytes).
        let count = *table.first()? as usize;
        let rows = &table[1..];
        let (row_words, port_at, pid_at) = if family == AF_INET {
            let words = size_of::<MIB_TCPROW_OWNER_PID>() / 4;
            (words, 2, 5)
        } else {
            let words = size_of::<MIB_TCP6ROW_OWNER_PID>() / 4;
            // ucLocalAddr (4 words), dwLocalScopeId, dwLocalPort ...
            (words, 5, words - 1)
        };
        (0..count).find_map(|index| {
            let row = rows.get(index * row_words..(index + 1) * row_words)?;
            // The port is in network byte order in the low 16 bits.
            let local = u16::from_be((row[port_at] & 0xFFFF) as u16);
            (local == port).then_some(row[pid_at])
        })
    }

    /// The listening half of the family's TCP table with owning pids, as u32
    /// words. The table can grow between the call that sizes it and the call
    /// that fills it; a few tries cover that.
    fn listeners(family: u32) -> Option<Vec<u32>> {
        let mut size = 0u32;
        for _ in 0..4 {
            let mut buffer = vec![0u32; (size as usize).div_ceil(4).max(1)];
            // SAFETY: `buffer` is `size` bytes or more, u32-aligned as the
            // table requires; the API writes at most `size` bytes and updates
            // it when the buffer is short.
            let result = unsafe { GetExtendedTcpTable(buffer.as_mut_ptr().cast::<c_void>(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0) };
            match result {
                NO_ERROR => return Some(buffer),
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
        }
        None
    }

    /// The file name of the program `pid` runs, when we may ask.
    fn image_name(pid: u32) -> Option<String> {
        // SAFETY: the handle is checked before use and closed on every path;
        // the buffer and its length belong to this frame.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let named = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) != 0;
            CloseHandle(handle);
            if !named {
                return None;
            }
            let path = String::from_utf16_lossy(&buffer[..length as usize]);
            path.rsplit(['\\', '/']).next().map(str::to_string)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Holder;

    pub fn holder(port: u16) -> Option<Holder> {
        let inodes: Vec<u64> = ["/proc/net/tcp", "/proc/net/tcp6"]
            .iter()
            .filter_map(|table| std::fs::read_to_string(table).ok())
            .flat_map(|text| super::listening_inodes(&text, port))
            .collect();
        if inodes.is_empty() {
            return None;
        }
        let pid = owner_of(&inodes);
        Some(Holder {
            pid,
            name: pid
                .and_then(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok())
                .map(|name| name.trim().to_string()),
        })
    }

    /// The process with a descriptor on one of the sockets. Only processes
    /// this user may look into are seen; another user's stays `None`.
    fn owner_of(inodes: &[u64]) -> Option<u32> {
        let wanted: Vec<String> = inodes.iter().map(|inode| format!("socket:[{inode}]")).collect();
        for entry in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
                continue;
            };
            let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
                continue;
            };
            for fd in fds.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path())
                    && wanted.iter().any(|socket| target.as_os_str() == socket.as_str())
                {
                    return Some(pid);
                }
            }
        }
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Holder;

    /// `lsof` ships with macOS, and its machine format names the pid and the
    /// command of a listener in two lines.
    pub fn holder(port: u16) -> Option<Holder> {
        let output = std::process::Command::new("lsof")
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-Fpc"])
            .output()
            .ok()?;
        super::read_lsof(&String::from_utf8_lossy(&output.stdout))
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use super::Holder;

    /// Other systems: whether something answers, without a name.
    pub fn holder(port: u16) -> Option<Holder> {
        let answers = |address: std::net::SocketAddr| std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(300)).is_ok();
        let v4 = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let v6 = std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port));
        (answers(v4) || answers(v6)).then_some(Holder { pid: None, name: None })
    }
}

/// The socket inodes listening on `port` in a `/proc/net/tcp{,6}` table.
///
/// Each line after the header: `sl local rem st ... inode`, the local address
/// as `HEX_ADDR:HEX_PORT`, the state as hex - `0A` is LISTEN.
#[cfg(any(target_os = "linux", test))]
fn listening_inodes(table: &str, port: u16) -> Vec<u64> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let local_port = u16::from_str_radix(fields.get(1)?.rsplit(':').next()?, 16).ok()?;
            (local_port == port && *fields.get(3)? == "0A").then(|| fields.get(9)?.parse().ok())?
        })
        .collect()
}

/// The first listener in `lsof -Fpc` output: a `p` line with the pid, then a
/// `c` line with the command.
#[cfg(any(target_os = "macos", test))]
fn read_lsof(output: &str) -> Option<Holder> {
    let mut pid = None;
    for line in output.lines() {
        if let Some(number) = line.strip_prefix('p') {
            if pid.is_some() {
                break;
            }
            pid = number.trim().parse().ok();
        } else if let Some(command) = line.strip_prefix('c')
            && pid.is_some()
        {
            return Some(Holder {
                pid,
                name: Some(command.trim().to_string()),
            });
        }
    }
    pid.map(|pid| Holder { pid: Some(pid), name: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test itself listens; the table names it.
    #[test]
    fn a_listener_is_found_with_its_own_pid() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let holder = holder(port).expect("the port is listened on");
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert_eq!(holder.pid, Some(std::process::id()), "{holder:?}");
            assert!(holder.name.is_some(), "the test binary has a name: {holder:?}");
        }
        drop(listener);
        // Asked a few times, not once: on a busy Linux machine the closed
        // socket's row can outlive it for a moment, listed with no process
        // holding it (seen once in a container, 13.10.2026, with the suite
        // running in parallel). A port that stays held fails as before.
        let freed = (0..20).any(|_| {
            let gone = super::holder(port).is_none();
            if !gone {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            gone
        });
        assert!(freed, "nobody listens once the listener is gone: {:?}", super::holder(port));
    }

    /// An IPv6-only listener holds its port too - the way Node listens on
    /// `localhost`.
    #[test]
    fn a_listener_on_the_ipv6_loopback_is_found() {
        let Ok(listener) = std::net::TcpListener::bind("[::1]:0") else {
            return; // no IPv6 on this machine
        };
        let port = listener.local_addr().unwrap().port();
        assert!(holder(port).is_some(), "port {port} on ::1 read as free");
    }

    #[test]
    fn the_proc_table_is_read_for_listeners_only() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
           0: 0100007F:13BA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 41234 1 0000000000000000 100 0 0 10 0\n\
           1: 0100007F:13BA 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 41299 1 0000000000000000 20 4 30 10 -1\n\
           2: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1001 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(listening_inodes(table, 5050), vec![41234], "an established connection is not a listener");
        assert_eq!(listening_inodes(table, 22), vec![1001]);
        assert!(listening_inodes(table, 80).is_empty());
    }

    #[test]
    fn lsof_output_names_the_first_listener() {
        assert_eq!(
            read_lsof("p4242\ncnode\np5000\ncother\n"),
            Some(Holder {
                pid: Some(4242),
                name: Some("node".into())
            })
        );
        assert_eq!(read_lsof(""), None);
    }

    #[test]
    fn a_holder_is_described_with_what_is_known() {
        let full = Holder {
            pid: Some(4242),
            name: Some("node.exe".into()),
        };
        assert_eq!(full.describe(), "node.exe (pid 4242)");
        assert_eq!(Holder { pid: None, name: None }.describe(), "a process turnout cannot see");
    }
}
