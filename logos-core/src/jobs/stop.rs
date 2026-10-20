//! Stopping on SIGINT or SIGTERM in a process that runs [`foreground`] scans
//! and installs: the CLI's `security scan` and `security install`, and a
//! `serve --mcp` with no web runtime to catch the signal (sprint 107 review,
//! D1).
//!
//! The project lock ([`super::lock`]) is held by the Logos process, and each
//! scanner or installer runs in a process group of its own, out of reach of
//! the terminal's Ctrl-C. Without this, a stopped Logos freed the lock while
//! its scanner ran on, and a second scan was admitted beside it. With it, the
//! signal kills every [foreground](super::foreground) child's group first, and
//! the process then exits, which frees the lock. SIGKILL cannot be caught: a
//! Logos killed by it still leaves its scanners running.
//!
//! [`foreground`]: super::foreground

use std::io::Read;
use std::sync::atomic::{AtomicI32, Ordering};

/// The write end of the self-pipe the handler reports a signal on; `-1`
/// until [`stop_on_signal`] installs it.
static PIPE_WRITE: AtomicI32 = AtomicI32::new(-1);

/// How a process [stopped on a signal](stop_on_signal) exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopExit {
    /// 128 + the signal's number, as a shell reports a process the signal
    /// ended: a command that was interrupted.
    Interrupted,
    /// 0: a server for which a stop signal is how it is meant to end.
    Clean,
}

impl StopExit {
    fn code(self, signal: i32) -> i32 {
        match self {
            StopExit::Interrupted => 128 + signal,
            StopExit::Clean => 0,
        }
    }
}

/// On SIGINT or SIGTERM, [cancel every foreground run](super::cancel_foreground)
/// of this process, then exit as `exit` says. Installed once per process; a
/// later call does nothing. Returns whether the handlers are in place — when
/// they cannot be installed, a signal stops the process as it would without
/// them.
pub fn stop_on_signal(exit: StopExit) -> bool {
    #[cfg(unix)]
    {
        install(exit)
    }
    #[cfg(not(unix))]
    {
        let _ = exit;
        false
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
fn install(exit: StopExit) -> bool {
    use std::os::fd::FromRawFd;

    if PIPE_WRITE.load(Ordering::Acquire) >= 0 {
        return true;
    }
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe` writes two descriptors into the two-element array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return false;
    }
    for fd in fds {
        // SAFETY: `fd` is a descriptor `pipe` just opened; close-on-exec keeps
        // it out of every scanner and installer this process starts.
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    // SAFETY: the read end is ours alone from here on.
    let mut reader = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let watcher = std::thread::Builder::new().name("logos-stop-signal".to_string()).spawn(move || {
        let mut signal = [0u8; 1];
        if reader.read_exact(&mut signal).is_ok() {
            super::cancel_foreground();
            std::process::exit(exit.code(i32::from(signal[0])));
        }
    });
    if watcher.is_err() {
        return false;
    }
    PIPE_WRITE.store(fds[1], Ordering::Release);
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: a zeroed `sigaction` is a valid empty one; the handler only
        // calls `write`, which is async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return false;
            }
        }
    }
    true
}

/// The handler: report the signal on the self-pipe and nothing else.
#[cfg(unix)]
#[allow(unsafe_code)]
extern "C" fn on_signal(signal: libc::c_int) {
    let fd = PIPE_WRITE.load(Ordering::Acquire);
    if fd >= 0 {
        let byte = u8::try_from(signal).unwrap_or(u8::MAX);
        // SAFETY: `write` is async-signal-safe; `fd` is the pipe's write end,
        // open for the life of the process.
        unsafe {
            libc::write(fd, std::ptr::addr_of!(byte).cast(), 1);
        }
    }
}
