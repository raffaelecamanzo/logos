//! Spawning one child process — a scanner, or the tool installer's `curl`,
//! `tar`, `unzip` and `uv` — as an argv vector, never a shell ([NFR-SE-08]),
//! with stdout and stderr captured to a bound ([ADR-80]), and the rules for
//! where such a program may come from.
//!
//! Both pipes are drained on their own threads, so a process that prints more
//! than the bound never blocks on a full pipe; only the bounded part is kept.
//! Stdout keeps its **head** (a version line comes first) and stderr its
//! **tail** (the cause of a failure comes last).
//!
//! On Unix the process runs in a process group of its own. When it exits, or
//! when the deadline passes, the whole group is killed: a launcher script's
//! child (a JVM, a Python interpreter, a build daemon) never outlives the run,
//! and never holds the pipes open past it. The pipes are read for at most
//! [`DRAIN_GRACE`] after that.
//!
//! The process is started through [`ChildGuard`], so a spawn on a [job's
//! thread](crate::jobs) joins that job: cancelling the job kills the group at
//! once, and once it is cancelled no further process starts — [`run`] returns
//! an [`io::ErrorKind::Interrupted`] error instead (S-673).
//!
//! This module is shared by the security orchestrator and the tool installer,
//! and depends on neither: it lives beside [`ChildGuard`] so that neither of
//! the two depends on the other to start a process (S-747).
//!
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::ChildGuard;

/// Bytes of stdout kept per spawn.
pub(crate) const STDOUT_CAP: usize = 64 * 1024;
/// Bytes of stderr kept per spawn.
pub(crate) const STDERR_CAP: usize = 16 * 1024;
/// How long the pipes are still read once the process group is gone.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// What a spawned process left behind.
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

/// How to start a process: everything but the program and its arguments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Launch<'a> {
    /// The working directory: the scan root, or the install's staging directory.
    pub cwd: &'a Path,
    /// Variables added to the inherited environment.
    pub env: &'a [(&'a str, &'a str)],
    /// Variables with an owned value, added after `env`: a managed runtime's
    /// paths (Dependency-Check's Java runtime, S-746).
    pub vars: &'a [(&'a str, OsString)],
    /// When set, the inherited environment is cleared and only these
    /// variables are set, then `env`, `vars` and `PATH` (the managed `uv`: no
    /// `UV_*` or `PYTHON*` variable of the user's reaches it).
    pub isolated: Option<&'a [(&'a str, OsString)]>,
    /// The child's whole `PATH`; `None` removes it.
    pub path: Option<&'a OsString>,
    /// Kill the process group after this long.
    pub deadline: Option<Duration>,
}

/// Run `program` with `args` as `launch` says, stdin closed, and wait for it.
pub(crate) fn run(program: &Path, args: &[OsString], launch: &Launch<'_>) -> io::Result<Finished> {
    // Held until the group is gone, so a cancel can still reach it.
    let (mut child, guard) = spawn(program, args, launch)?;
    let stdout = child.stdout.take().map(|p| drain(p, STDOUT_CAP, Keep::Head));
    let stderr = child.stderr.take().map(|p| drain(p, STDERR_CAP, Keep::Tail));
    let status = wait(&mut child, launch.deadline);
    // Whatever the process left running goes with it; then the group is no
    // longer the job's to kill, while the pipes are still drained below.
    super::kill_group(child.id());
    drop(guard);
    let status = status?;
    Ok(Finished {
        status,
        stdout: stdout.map(Drained::collect).unwrap_or_default(),
        stderr: stderr.map(Drained::collect).unwrap_or_default(),
    })
}

fn spawn(program: &Path, args: &[OsString], launch: &Launch<'_>) -> io::Result<(Child, ChildGuard)> {
    let mut attempt = 0;
    loop {
        let mut command = Command::new(program);
        if let Some(vars) = launch.isolated {
            command.env_clear().envs(vars.iter().map(|(k, v)| (k, v)));
        }
        command
            .args(args)
            .current_dir(launch.cwd)
            .envs(launch.env.iter().copied())
            .envs(launch.vars.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        match launch.path {
            Some(path) => command.env("PATH", path),
            None => command.env_remove("PATH"),
        };
        // A process group of its own (Unix), registered with this thread's job.
        match ChildGuard::spawn(&mut command) {
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

// ── where a program may come from ────────────────────────────────────────────

/// The `PATH` entries a scanner — or the installer's `curl` and `tar` — may
/// come from, and the only ones its process sees: absolute, and not inside the project root (compared both as written
/// and canonicalised). An empty or relative entry would resolve against the
/// working directory, which is the repository.
pub(crate) fn usable_path_entries(
    root: &Path,
    search_path: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let Some(search_path) = search_path else {
        return Vec::new();
    };
    let canonical_root = root.canonicalize().ok();
    std::env::split_paths(search_path)
        .filter(|dir| dir.is_absolute() && !inside_root(root, canonical_root.as_deref(), dir))
        .collect()
}

/// Whether `dir` is inside the project `root`, compared both as written and
/// canonicalised (`canonical_root` is `root` canonicalised, when it can be).
pub(crate) fn inside_root(root: &Path, canonical_root: Option<&Path>, dir: &Path) -> bool {
    dir.starts_with(root)
        || canonical_root.is_some_and(|r| dir.canonicalize().is_ok_and(|d| d.starts_with(r)))
}

/// A regular file the current user may execute.
pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
