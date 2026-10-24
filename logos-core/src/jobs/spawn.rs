//! Spawning one child process — a scanner, the tool installer's `curl`,
//! `tar`, `unzip` and `uv`, or a coverage tool — as an argv vector, never a shell ([NFR-SE-08]),
//! with stdout and stderr captured to a bound ([ADR-80]), and the rules for
//! where such a program may come from.
//!
//! Both pipes are drained on their own threads, so a process that prints more
//! than the bound never blocks on a full pipe; only the bounded part is kept.
//! Each stream keeps its **head** (a version line comes first) *and* its
//! **tail** (the cause of a failure — a test runner's `failures:` list, a
//! compiler's last error — comes last), with one line between them stating how
//! many bytes were left out. A stream that fits its bound is kept whole.
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
//! A secret a child reads — Dependency-Check's NVD API key, for one — is
//! handed over in a [`PrivateFile`], never in argv: a 0600 file that a cancel
//! or the end of the run removes (S-794).
//!
//! This module is shared by the security orchestrator, the tool installer and
//! the coverage runner (S-735), and depends on none of them: it lives beside
//! [`ChildGuard`] so that none depends on another to start a process (S-747).
//!
//! [NFR-SE-08]: ../../../docs/specs/requirements/NFR-SE-08.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::ChildGuard;

/// Bytes of stdout kept per spawn, the omission note included.
pub(crate) const STDOUT_CAP: usize = 64 * 1024;
/// Bytes of stderr kept per spawn, the omission note included.
pub(crate) const STDERR_CAP: usize = 16 * 1024;
/// Room set aside in a stream's bound for the omission note.
const NOTE_RESERVE: usize = 64;
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
    let stdout = child.stdout.take().map(|p| drain(p, STDOUT_CAP));
    let stderr = child.stderr.take().map(|p| drain(p, STDERR_CAP));
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

/// The first quarter of a stream's bound is its head, the rest its tail: the
/// end is where a tool says why it failed.
pub(crate) const fn head_cap(cap: usize) -> usize {
    cap / 4
}

/// The most of a stream's tail kept once something was left out: what the head
/// leaves of `cap`, less the room for the omission note. A stream that fits
/// `cap` is kept whole, so this applies only past it.
pub(crate) const fn tail_cap(cap: usize) -> usize {
    cap - head_cap(cap) - NOTE_RESERVE
}

/// The omission note is `[… N bytes omitted …]` and a newline.
const NOTE_OPEN: &str = "[… ";
const NOTE_CLOSE: &str = " bytes omitted …]\n";

/// Where each omission note in `text` is, as byte ranges including the note's
/// newline. A cut left in `text` by the capture lies at each: what comes
/// before one is a stream's head, what follows it that stream's tail. A note
/// a tool printed itself is found too, which only makes a caller treat one
/// more place as a cut.
pub(crate) fn omission_notes(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(close) = text[from..].find(NOTE_CLOSE).map(|i| from + i) {
        let end = close + NOTE_CLOSE.len();
        let digits = |open: &usize| {
            let count = &text[open + NOTE_OPEN.len()..close];
            !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit())
        };
        if let Some(open) = text[from..close].rfind(NOTE_OPEN).map(|i| from + i).filter(digits) {
            found.push(open..end);
        }
        from = end;
    }
    found
}

/// What a bounded stream has kept so far: its head, its most recent bytes and
/// how many bytes passed in all.
struct Captured {
    cap: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl Captured {
    fn new(cap: usize) -> Self {
        Self { cap, head: Vec::new(), tail: Vec::new(), total: 0 }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let room = head_cap(self.cap).saturating_sub(self.head.len());
        let (into_head, rest) = bytes.split_at(bytes.len().min(room));
        self.head.extend_from_slice(into_head);
        let kept = self.cap - head_cap(self.cap);
        self.tail.extend_from_slice(rest);
        if self.tail.len() > kept {
            let excess = self.tail.len() - kept;
            self.tail.drain(..excess);
        }
    }

    /// The stream whole when it fits the bound; else the head, a note naming
    /// the bytes left out between it and the tail, and the tail, which the
    /// note's room has been taken from.
    fn text(&self) -> String {
        let tail = if self.total > self.cap {
            &self.tail[self.tail.len().saturating_sub(tail_cap(self.cap))..]
        } else {
            &self.tail[..]
        };
        let omitted = self.total - self.head.len() - tail.len();
        let mut out = self.head.clone();
        if omitted > 0 {
            if out.last().is_some_and(|b| *b != b'\n') {
                out.push(b'\n');
            }
            out.extend_from_slice(format!("{NOTE_OPEN}{omitted}{NOTE_CLOSE}").as_bytes());
        }
        out.extend_from_slice(tail);
        String::from_utf8_lossy(&out).into_owned()
    }
}

/// A pipe being read on its own thread into a bounded buffer.
struct Drained {
    kept: Arc<Mutex<Captured>>,
    done: mpsc::Receiver<()>,
}

impl Drained {
    /// What was read, once the pipe closed or [`DRAIN_GRACE`] passed.
    fn collect(self) -> String {
        let _ = self.done.recv_timeout(DRAIN_GRACE);
        self.kept.lock().map(|k| k.text()).unwrap_or_default()
    }
}

/// Read `pipe` to its end on a thread, keeping at most `cap` bytes: its head
/// and its tail.
fn drain(mut pipe: impl Read + Send + 'static, cap: usize) -> Drained {
    let kept = Arc::new(Mutex::new(Captured::new(cap)));
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
            kept.push(&buf[..n]);
        }
        let _ = tx.send(());
    });
    Drained { kept, done }
}

// ── a secret handed to a child in a file ─────────────────────────────────────

/// A file holding a secret a child process reads — Dependency-Check's NVD API
/// key, for one ([FR-SC-27]) — so that the secret never reaches the child's
/// argv.
///
/// It is created new, readable and writable by its owner only (mode 0600 on
/// Unix) from the moment it exists: whatever was at its path first — a copy an
/// earlier, crashed run left, or a symbolic link in its place — is removed,
/// never written through. It is removed again when this is dropped, so it
/// outlives no run, whatever ended it: the child exiting, failing, being
/// killed at its deadline, or the caller returning early. Created on a [job's
/// thread](crate::jobs) or in a [foreground run](super::foreground), it is
/// registered with that run, and cancelling the run removes it at once: a
/// process stopping on a signal exits without unwinding the thread that holds
/// it ([`super::stop`]).
///
/// [FR-SC-27]: ../../../docs/specs/requirements/FR-SC-27.md
pub(crate) struct PrivateFile {
    path: PathBuf,
    registration: Option<Arc<super::Children>>,
}

impl PrivateFile {
    /// Write `bytes` to a new file at `path`.
    ///
    /// # Errors
    /// [`io::ErrorKind::Interrupted`] when this thread's run was already
    /// cancelled — nothing is written then; else what was at `path` cannot be
    /// removed, or the file cannot be created or written — in which case
    /// nothing is left at `path`.
    pub(crate) fn create(path: &Path, bytes: &[u8]) -> io::Result<Self> {
        let registration = super::CURRENT.with(|current| current.borrow().clone());
        let Some(children) = registration.as_ref() else {
            return Self::write(path, bytes, None);
        };
        // Held while the file is written and registered: a cancel either
        // came first, and nothing is written, or finds the file registered.
        let mut set = super::lock(&children.set);
        if set.cancelled {
            return Err(super::cancelled_error());
        }
        let file = Self::write(path, bytes, registration.clone())?;
        set.files.insert(path.to_path_buf());
        Ok(file)
    }

    fn write(path: &Path, bytes: &[u8], registration: Option<Arc<super::Children>>) -> io::Result<Self> {
        use std::io::Write;
        remove_file_if_present(path)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Owned before the first byte is written: a failed write removes it.
        // Unregistered until it is written, so its drop takes no lock.
        let mut file = Self { path: path.to_path_buf(), registration: None };
        options.open(path)?.write_all(bytes)?;
        file.registration = registration;
        Ok(file)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PrivateFile {
    fn drop(&mut self) {
        if let Err(e) = remove_file_if_present(&self.path) {
            tracing::warn!(target: "logos::jobs", file = %self.path.display(), "could not remove a private file: {e}");
        }
        if let Some(children) = self.registration.take() {
            super::lock(&children.set).files.remove(&self.path);
        }
    }
}

/// Remove the file at `path`; one that is not there is no error. Never
/// follows a symbolic link: the link itself is removed.
pub(crate) fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
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

/// The first executable `name` in `search_dirs`, in order — the
/// [usable](usable_path_entries) `PATH` entries.
pub(crate) fn find_on_path(search_dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    search_dirs.iter().map(|dir| dir.join(name)).find(|candidate| is_executable(candidate))
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
