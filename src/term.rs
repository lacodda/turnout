//! Terminal hygiene around interrupts and child processes.
//!
//! Two things used to go wrong on Ctrl+C. The child's process tree survived in
//! pieces - cmd.exe does not forward termination to grandchildren, so a dev
//! server's helpers (esbuild services and the like) kept running with no
//! console. And the console itself stayed half-broken: dev servers switch
//! stdin to raw mode and hide the cursor, an interrupt skips their cleanup,
//! and the shell that gets the prompt back swallows keystrokes.
//!
//! The cure has three parts. The original terminal state is captured once at
//! startup and can be put back on any exit path. A Ctrl+C handler restores it
//! when no child is running (spinners and pickers), and otherwise steps aside:
//! the interrupt reaches the child by itself, the child dies, and the normal
//! return path cleans up. On Windows every spawned child is confined to a job
//! object that kills the whole tree when turnout exits, however it exits.
//!
//! Everything here is a no-op when stdin/stdout are not a terminal.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Set while `run_in_dir` waits on a child; the interrupt handler consults it.
static CHILD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// How many Ctrl+C arrived while a child was active; the second one means the
/// child ignored the first, and the user wants the tree gone now.
static INTERRUPTS: AtomicU32 = AtomicU32::new(0);
/// Whether the current command was interrupted at all, for exit codes.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Capture the terminal state and install the Ctrl+C handler.
///
/// Called once, at the top of `main`, before anything touches the terminal.
pub fn init() {
    imp::capture();
    imp::install_handler();
}

/// Put the terminal back the way `init` found it: input modes, cursor, colors.
///
/// Safe to call any number of times; does nothing when there was no terminal
/// to capture.
pub fn restore() {
    imp::restore();
}

/// Whether Ctrl+C arrived during this command, for a conventional exit code.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// Mark a child as running: from here Ctrl+C belongs to the child, and the
/// handler must not tear turnout down under it.
pub fn child_begin() {
    INTERRUPTS.store(0, Ordering::SeqCst);
    CHILD_ACTIVE.store(true, Ordering::SeqCst);
}

/// The child is gone: restore the terminal it may have left raw - dev servers
/// do that even on a clean quit.
pub fn child_end() {
    CHILD_ACTIVE.store(false, Ordering::SeqCst);
    restore();
}

/// Confine a spawned child to a kill-on-close job object (Windows).
///
/// The job handle is deliberately never closed by us: the OS closes it when
/// turnout exits, and that close is precisely what kills any survivor in the
/// tree. On Unix this is a no-op - the kernel already delivers the terminal's
/// SIGINT to the whole foreground process group.
pub fn confine(child: &std::process::Child) {
    imp::confine(child);
}

/// The escape codes every restore writes: show the cursor, reset colors.
const RESET_SEQUENCE: &str = "\x1b[?25h\x1b[0m";

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};

    use windows_sys::Win32::Foundation::{FALSE, HANDLE, TRUE};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    /// Console modes as they were at startup; the "+1" trick is not needed
    /// because a captured flag rides alongside each.
    static STDIN_MODE: AtomicU32 = AtomicU32::new(0);
    static STDOUT_MODE: AtomicU32 = AtomicU32::new(0);
    static STDIN_CAPTURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static STDOUT_CAPTURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    /// The job the current child lives in, for the second-Ctrl+C kill.
    static JOB: AtomicIsize = AtomicIsize::new(0);

    pub fn capture() {
        unsafe {
            let mut mode = 0u32;
            let stdin = GetStdHandle(STD_INPUT_HANDLE);
            if GetConsoleMode(stdin, &mut mode) != 0 {
                STDIN_MODE.store(mode, Ordering::SeqCst);
                STDIN_CAPTURED.store(true, Ordering::SeqCst);
            }
            let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
            if GetConsoleMode(stdout, &mut mode) != 0 {
                STDOUT_MODE.store(mode, Ordering::SeqCst);
                STDOUT_CAPTURED.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn restore() {
        unsafe {
            if STDIN_CAPTURED.load(Ordering::SeqCst) {
                SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), STDIN_MODE.load(Ordering::SeqCst));
            }
            if STDOUT_CAPTURED.load(Ordering::SeqCst) {
                SetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), STDOUT_MODE.load(Ordering::SeqCst));
                use std::io::Write;
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(super::RESET_SEQUENCE.as_bytes());
                let _ = stdout.flush();
            }
        }
    }

    pub fn install_handler() {
        unsafe {
            SetConsoleCtrlHandler(Some(handler), TRUE);
        }
    }

    /// Runs on a system thread, so the full API is available - unlike a POSIX
    /// signal handler.
    unsafe extern "system" fn handler(ctrl_type: u32) -> windows_sys::core::BOOL {
        if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
            // Console close and friends: the default handling is right.
            return FALSE;
        }
        super::INTERRUPTED.store(true, Ordering::SeqCst);
        if super::CHILD_ACTIVE.load(Ordering::SeqCst) {
            // The same event is already on its way to the child; our wait()
            // returns when it dies and the normal path restores the terminal.
            // A second Ctrl+C means the child is ignoring it - kill the tree.
            if super::INTERRUPTS.fetch_add(1, Ordering::SeqCst) >= 1 {
                let job = JOB.load(Ordering::SeqCst);
                if job != 0 {
                    unsafe { TerminateJobObject(job as HANDLE, 130) };
                }
            }
            return TRUE;
        }
        // No child: this interrupt is for turnout itself, mid-spinner or
        // mid-picker. Put the terminal back and leave.
        restore();
        std::process::exit(130);
    }

    pub fn confine(child: &std::process::Child) {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok != 0 {
                AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
            }
            JOB.store(job as isize, Ordering::SeqCst);
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::cell::UnsafeCell;
    use std::mem::MaybeUninit;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The termios captured at startup. Written once in `capture`, before the
    /// handler is installed, so the handler only ever reads settled memory.
    struct TermiosSlot(UnsafeCell<MaybeUninit<libc::termios>>);
    unsafe impl Sync for TermiosSlot {}
    static ORIGINAL: TermiosSlot = TermiosSlot(UnsafeCell::new(MaybeUninit::uninit()));
    static CAPTURED: AtomicBool = AtomicBool::new(false);

    pub fn capture() {
        unsafe {
            let slot = ORIGINAL.0.get();
            if libc::tcgetattr(libc::STDIN_FILENO, (*slot).as_mut_ptr()) == 0 {
                CAPTURED.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn restore() {
        if !CAPTURED.load(Ordering::SeqCst) {
            return;
        }
        unsafe {
            let slot = ORIGINAL.0.get();
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, (*slot).as_ptr());
            // write(2), not println: this also runs inside the signal handler,
            // where only async-signal-safe calls are allowed.
            let sequence = super::RESET_SEQUENCE;
            libc::write(libc::STDOUT_FILENO, sequence.as_ptr().cast(), sequence.len());
        }
    }

    pub fn install_handler() {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = handler as *const std::ffi::c_void as libc::sighandler_t;
            // SA_RESTART so the wait() on the child resumes instead of failing
            // with EINTR when the handler returns.
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        }
    }

    /// Async-signal-safe only: atomics, tcsetattr, write, _exit.
    extern "C" fn handler(_signal: libc::c_int) {
        super::INTERRUPTED.store(true, Ordering::SeqCst);
        if super::CHILD_ACTIVE.load(Ordering::SeqCst) {
            // The kernel delivered the same SIGINT to the whole foreground
            // process group, the child included; the normal return path from
            // wait() restores the terminal. Nothing to do here.
            super::INTERRUPTS.fetch_add(1, Ordering::SeqCst);
            return;
        }
        restore();
        unsafe { libc::_exit(130) };
    }

    /// SIGINT reaches the whole foreground process group by itself.
    pub fn confine(_child: &std::process::Child) {}
}
