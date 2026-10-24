//! One scan or install at a time per project, whichever surface or process
//! starts it (S-758, [analysis-jobs]): an exclusive `flock` on the project's
//! lock file, taken by every security scan and every scanner install from the
//! CLI, MCP and the web UI — save a CLI install where no scan can run (no
//! `.logos` directory), which writes nothing there (`Engine::security_install`).
//!
//! A lock the kernel holds per open file, not per process, so two surfaces of
//! one `serve` exclude each other exactly as two processes do, and the kernel
//! drops it however its holder ends. The holder writes who it is into the file
//! — what runs, the surface that started it, the web job's id, its process and
//! when — so a refused start names the running one ([`Busy`]).
//!
//! A web job takes the lock when [`AnalysisJobs::start_exclusive`] starts it and
//! holds it until its work returns; the work's own scan or install, on the
//! job's thread, finds the lock [held here](held_here) and takes nothing more.
//! A second web start of the same kind joins the running job before any lock
//! is looked at.
//!
//! [analysis-jobs]: ../../../docs/specs/architecture/components/analysis-jobs.md
//! [`AnalysisJobs::start_exclusive`]: super::AnalysisJobs::start_exclusive

use std::cell::RefCell;
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{JobId, JobKind};

/// The most of a lock file a refused start reads: its holder is one short
/// line of JSON.
const HOLDER_CAP: u64 = 4 * 1024;

/// How many more times a lock held by someone that names no one is tried, and
/// how long apart. Such a hold is a [`peek`] or a holder that has not written
/// itself yet, each over in microseconds; a holder that never writes (another
/// program's `flock`) is refused once the retries are spent.
const UNNAMED_RETRIES: u32 = 20;
const UNNAMED_WAIT: Duration = Duration::from_millis(5);

thread_local! {
    /// The lock files this thread holds, innermost last.
    static HELD: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Who holds a project's lock, as its holder wrote it into the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    /// What runs.
    pub kind: JobKind,
    /// The surface that started it (`cli`, `mcp`, `web`), when known.
    pub surface: Option<String>,
    /// The web job it runs as; `None` for a CLI or MCP call.
    pub job: Option<JobId>,
    /// The Logos process that holds the lock.
    pub pid: u32,
    /// When it was taken, in Unix seconds.
    pub since: u64,
}

impl Holder {
    /// A holder in this process, on the surface this thread answers for.
    pub fn here(kind: JobKind, job: Option<JobId>) -> Self {
        Self {
            kind,
            surface: crate::observability::surface_here().map(|s| s.as_str().to_string()),
            job,
            pid: std::process::id(),
            since: unix_now(),
        }
    }

    fn what(&self) -> &'static str {
        match self.kind {
            JobKind::SecurityScan => "a security scan",
            // A scanner install, or a coverage tool install (S-748, S-750).
            JobKind::ToolInstall => "a tool install",
            // A coverage run takes no project lock (S-738); named for a
            // holder file that says otherwise.
            JobKind::CoverageRun => "a coverage run",
        }
    }

    fn whence(&self) -> String {
        match self.surface.as_deref() {
            Some("web") => " from the web UI".to_string(),
            Some("mcp") => " over MCP".to_string(),
            Some("cli") => " from the command line".to_string(),
            Some(other) => format!(" from {other}"),
            None => String::new(),
        }
    }
}

/// A start refused because another scan or install of the project holds its
/// lock. Its message names the running one, as far as its holder wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    /// `None` while the holder has not written itself yet, or wrote something
    /// unreadable.
    pub holder: Option<Holder>,
}

impl fmt::Display for Busy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(holder) = &self.holder else {
            return f.write_str(
                "another scan or install is already running on this project; \
                 wait for it to finish, then try again",
            );
        };
        write!(f, "{} is already running on this project: ", holder.what())?;
        if let Some(job) = holder.job {
            write!(f, "job {job}, ")?;
        }
        write!(
            f,
            "started{} by Logos process {} {} s ago; wait for it to finish or cancel it, then try again",
            holder.whence(),
            holder.pid,
            unix_now().saturating_sub(holder.since),
        )
    }
}

impl std::error::Error for Busy {}

/// A held project lock. Dropping it empties the file, then releases the lock.
#[derive(Debug)]
pub struct ProjectLock {
    file: fs::File,
    path: PathBuf,
}

impl ProjectLock {
    /// Take the lock in `path`, creating the file, and write `holder` into it;
    /// [`Busy`], naming the holder, while anyone else holds it — another
    /// process, or another open of it in this one.
    ///
    /// # Errors
    /// The file cannot be opened, locked or written.
    pub fn take(path: &Path, holder: &Holder) -> io::Result<Result<Self, Busy>> {
        let mut file = open_lock_file(path)?;
        if let Some(busy) = try_hold(&mut file)? {
            return Ok(Err(busy));
        }
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&serde_json::to_vec(holder)?)?;
        Ok(Ok(Self { file, path: path.to_path_buf() }))
    }

    /// Run `f` with this thread marked as holding the lock, so the scan or
    /// install `f` runs takes it no second time.
    pub fn holding<T>(&self, f: impl FnOnce() -> T) -> T {
        HELD.with(|held| held.borrow_mut().push(self.path.clone()));
        let _release = Unmark;
        f()
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        // Emptied while still held, so a start refused later never reads a
        // holder that has gone.
        let _ = self.file.set_len(0);
        // Released explicitly rather than by the close: a child another thread
        // has forked but not yet exec'd shares this open file, and would keep
        // the lock held past the close until its exec.
        let _ = self.file.unlock();
    }
}

/// Pops the innermost [`HELD`] entry on drop, a panic included.
struct Unmark;

impl Drop for Unmark {
    fn drop(&mut self) {
        HELD.with(|held| held.borrow_mut().pop());
    }
}

/// Whether this thread holds the lock in `path`.
pub fn held_here(path: &Path) -> bool {
    HELD.with(|held| held.borrow().iter().any(|p| p == path))
}

/// Who holds the lock in `path`, without taking it: `None` when no one does
/// or the file does not exist. A held lock is [`Busy`] here, even to the
/// thread holding it.
///
/// # Errors
/// The file exists but cannot be opened or locked.
pub fn peek(path: &Path) -> io::Result<Option<Busy>> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let mut file = open_lock_file(path)?;
    let busy = try_hold(&mut file)?;
    if busy.is_none() {
        // Released at once, and explicitly, as `ProjectLock`'s drop does.
        file.unlock()?;
    }
    Ok(busy)
}

/// Lock `file`; the [`Busy`] naming its holder when someone else holds it. A
/// hold that names no one — a [`peek`], or a holder between its lock and its
/// write — is tried again, [`UNNAMED_RETRIES`] times, so that neither refuses
/// a start in place of a scan that is not running.
fn try_hold(file: &mut fs::File) -> io::Result<Option<Busy>> {
    let mut retries = UNNAMED_RETRIES;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(None),
            Err(fs::TryLockError::Error(e)) => return Err(e),
            Err(fs::TryLockError::WouldBlock) => {
                let busy = read_busy(file);
                if busy.holder.is_some() || retries == 0 {
                    return Ok(Some(busy));
                }
                retries -= 1;
                std::thread::sleep(UNNAMED_WAIT);
            }
        }
    }
}

/// Run `f` holding the lock in `path` as a `kind` started on this thread's
/// surface — or straight away when this thread already holds it (the scan an
/// install job runs once it is done).
///
/// # Errors
/// [`Busy`] while another scan or install holds the lock, nothing run; the
/// lock file's own I/O error; else `f`'s error.
pub fn exclusively<T>(path: &Path, kind: JobKind, f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    if held_here(path) {
        return f();
    }
    let lock = ProjectLock::take(path, &Holder::here(kind, None))?.map_err(anyhow::Error::new)?;
    lock.holding(f)
}

/// Open — creating it — the lock file at `path` for reading and writing: a
/// symlink in its place is removed, and none is followed. Shared with the
/// tool installer's staging locks.
pub(crate) fn open_lock_file(path: &Path) -> io::Result<fs::File> {
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::remove_file(path)?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// The holder a busy lock file names, read to [`HOLDER_CAP`].
fn read_busy(file: &mut fs::File) -> Busy {
    let mut text = String::new();
    let holder = file
        .seek(SeekFrom::Start(0))
        .ok()
        .and_then(|_| (&mut *file).take(HOLDER_CAP).read_to_string(&mut text).ok())
        .and_then(|_| serde_json::from_str(&text).ok());
    Busy { holder }
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(all(test, unix))]
mod tests;
