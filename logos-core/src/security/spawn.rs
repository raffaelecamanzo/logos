//! Spawning one scanner process: an argv vector, never a shell ([NFR-SE-08]),
//! with stdout and stderr captured to a bound ([ADR-80]).
//!
//! Both pipes are drained on their own threads, so a scanner that prints more
//! than the bound never blocks on a full pipe; only the bounded part is kept.
//! Stdout keeps its **head** (a version line comes first) and stderr its
//! **tail** (the cause of a failure comes last).
//!
//! On Unix the scanner runs in a process group of its own. When it exits, or
//! when the deadline passes, the whole group is killed: a launcher script's
//! child (a JVM, a Python interpreter, a build daemon) never outlives the run,
//! and never holds the pipes open past it. The pipes are read for at most
//! [`DRAIN_GRACE`] after that.
//!
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::ffi::OsString;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Bytes of stdout kept per spawn.
pub(crate) const STDOUT_CAP: usize = 64 * 1024;
/// Bytes of stderr kept per spawn.
pub(crate) const STDERR_CAP: usize = 16 * 1024;
/// How long the pipes are still read once the process group is gone.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// What a spawned scanner left behind.
#[derive(Debug)]
pub(crate) struct Finished {
    /// `None` when the deadline passed and the child was killed.
    pub status: Option<ExitStatus>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy)]
enum Keep {
    Head,
    Tail,
}

/// How to start a scanner: everything but the program and its arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Launch<'a> {
    /// The working directory: the scan root.
    pub cwd: &'a Path,
    /// Variables added to the inherited environment.
    pub env: &'a [(&'a str, &'a str)],
    /// The child's whole `PATH`; `None` removes it.
    pub path: Option<&'a OsString>,
    /// Kill the process group after this long.
    pub deadline: Option<Duration>,
}

/// Run `program` with `args` as `launch` says, stdin closed, and wait for it.
pub(crate) fn run(program: &Path, args: &[OsString], launch: &Launch<'_>) -> io::Result<Finished> {
    let mut child = spawn(program, args, launch)?;
    let stdout = child.stdout.take().map(|p| drain(p, STDOUT_CAP, Keep::Head));
    let stderr = child.stderr.take().map(|p| drain(p, STDERR_CAP, Keep::Tail));
    let status = wait(&mut child, launch.deadline);
    // Whatever the scanner left running goes with it.
    kill_group(&child);
    let status = status?;
    Ok(Finished {
        status,
        stdout: stdout.map(Drained::collect).unwrap_or_default(),
        stderr: stderr.map(Drained::collect).unwrap_or_default(),
    })
}

fn spawn(program: &Path, args: &[OsString], launch: &Launch<'_>) -> io::Result<Child> {
    let mut attempt = 0;
    loop {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(launch.cwd)
            .envs(launch.env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match launch.path {
            Some(path) => command.env("PATH", path),
            None => command.env_remove("PATH"),
        };
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            // ETXTBSY: another thread of this process forked while the binary
            // was still open for writing (a freshly written executable). It
            // clears once that child execs; retry a few times.
            Err(e) if is_text_busy(&e) && attempt < 5 => {
                attempt += 1;
                thread::sleep(Duration::from_millis(20 * attempt));
            }
            other => return other,
        }
    }
}

fn is_text_busy(e: &io::Error) -> bool {
    #[cfg(target_os = "linux")]
    {
        e.raw_os_error() == Some(libc::ETXTBSY)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = e;
        false
    }
}

fn wait(child: &mut Child, deadline: Option<Duration>) -> io::Result<Option<ExitStatus>> {
    let Some(limit) = deadline else {
        return child.wait().map(Some);
    };
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if start.elapsed() >= limit {
            // The rest of the group goes in `run`, once this one is reaped.
            // Already exited between the poll and the kill: reaped by `wait`.
            let _ = child.kill();
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// SIGKILL the child's process group (its id: `process_group(0)` made it the
/// leader). A group with no member left is a no-op (`ESRCH`).
#[cfg(unix)]
#[allow(unsafe_code)]
fn kill_group(child: &Child) {
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // SAFETY: `killpg` takes two integers and touches no memory of ours. The
    // group id is a child this process started in a group of its own; while
    // any member is alive the id is not reused.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_child: &Child) {}

/// A pipe being read on its own thread into a bounded buffer.
struct Drained {
    kept: Arc<Mutex<Vec<u8>>>,
    done: mpsc::Receiver<()>,
}

impl Drained {
    /// What was read, once the pipe closed or [`DRAIN_GRACE`] passed.
    fn collect(self) -> String {
        let _ = self.done.recv_timeout(DRAIN_GRACE);
        let kept = self.kept.lock().map(|k| k.clone()).unwrap_or_default();
        String::from_utf8_lossy(&kept).into_owned()
    }
}

/// Read `pipe` to its end on a thread, keeping at most `cap` bytes.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize, keep: Keep) -> Drained {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let (tx, done) = mpsc::channel();
    let shared = Arc::clone(&kept);
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let n = match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let Ok(mut kept) = shared.lock() else { break };
            match keep {
                Keep::Head => {
                    let room = cap.saturating_sub(kept.len());
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
                Keep::Tail => {
                    kept.extend_from_slice(&buf[..n]);
                    if kept.len() > cap {
                        let excess = kept.len() - cap;
                        kept.drain(..excess);
                    }
                }
            }
        }
        let _ = tx.send(());
    });
    Drained { kept, done }
}
