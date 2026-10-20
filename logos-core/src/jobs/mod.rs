//! Detached, single-flight, cancellable jobs ([analysis-jobs], [ADR-80] §6,
//! S-673): a long, child-process-backed analysis run off the request path, so
//! a surface that started it keeps answering while it runs.
//!
//! - [`ScanJobs::start`] runs a job's work on a thread of its own. One job per
//!   [`JobKind`] per member runs at a time: a second start while one runs
//!   **joins** it and gets its [`JobId`] back. Different kinds, and different
//!   members, are independent.
//! - The work reports through its [`JobContext`]; every event is kept (to a
//!   bound) and replayed in each [`JobSnapshot`], so a late reader catches up.
//! - [`ScanJobs::cancel`] kills the whole process group of every child the job
//!   has running, and no child starts after it. A child joins its job through
//!   [`ChildGuard::spawn`], which the spawning code calls in place of
//!   `Command::spawn`; the job is found on the spawning thread, so the code
//!   between the job and the spawn passes nothing down.
//! - [`ScanJobs::shutdown`] cancels every running job the same way and waits,
//!   to a bound, for each to end: what `serve` calls when it stops (S-759), so
//!   no child outlives it and each job's own cleanup has run.
//!
//! The runner knows nothing about what a job does: each kind supplies the
//! closure that runs its steps. The security scan is the first kind; [S-747]
//! adds the tool install, which may end in a scan. Both start through
//! [`ScanJobs::start_exclusive`], which refuses while a kind they must not
//! overlap runs, and takes the project's [`lock`] so that no scan or install
//! started anywhere else — over MCP, from the CLI, in another process — runs
//! beside them (S-758).
//!
//! [analysis-jobs]: ../../../docs/specs/architecture/components/analysis-jobs.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md
//! [S-747]: ../../../docs/planning/journal.md#s-747-the-security-page-installs-missing-scanners-and-runs-a-full-scan-in-one-action

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod lock;
mod stop;

pub use stop::{stop_on_signal, StopExit};
pub(crate) mod spawn;

use lock::{Busy, Holder, ProjectLock};

/// Events a job keeps for replay. Past it the oldest are dropped and counted
/// in [`JobSnapshot::events_dropped`].
pub const EVENT_CAP: usize = 256;

/// What a job does. One job of a kind runs per member at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobKind {
    /// Every installed, enabled scanner, then the ingest of each report.
    SecurityScan,
    /// Pinned scanner installs from the Security page, and the full scan that
    /// may follow them (S-747).
    ToolInstall,
}

impl JobKind {
    /// The wire token, as serialized.
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::SecurityScan => "security-scan",
            JobKind::ToolInstall => "tool-install",
        }
    }
}

/// A job's identity: unique within one [`ScanJobs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Where a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    /// The work returned a result.
    Completed,
    /// The work returned an error, or panicked.
    Failed,
    /// It was cancelled: every child it had running was killed.
    Cancelled,
}

/// One event a job reported, in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobEvent {
    /// Its position among the job's events, from 0, dropped ones included.
    pub seq: u64,
    /// Milliseconds since the job started.
    pub at_ms: u64,
    /// What the work reported.
    pub event: serde_json::Value,
}

/// A job as it stands.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    /// Milliseconds since it started, or how long it ran once it ended.
    pub elapsed_ms: u64,
    /// The kept events, oldest first.
    pub events: Vec<JobEvent>,
    /// Events dropped past [`EVENT_CAP`].
    pub events_dropped: u64,
    /// The work's result, once [`JobState::Completed`].
    pub result: Option<serde_json::Value>,
    /// Why it failed, once [`JobState::Failed`].
    pub error: Option<String>,
}

/// What [`ScanJobs::start`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Started {
    /// `true` when a job of this kind was already running for this member and
    /// this start joined it rather than starting another.
    pub joined: bool,
    pub job: JobSnapshot,
}

/// The error a job's work returns when it stopped because the job was
/// cancelled. A job whose work returns it ends [`JobState::Cancelled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the job was cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Why [`ScanJobs::start_exclusive`] started nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// A job of an excluded kind runs for the member in this runner.
    Running(JobSnapshot),
    /// A scan or install started elsewhere — over MCP, from the CLI, in
    /// another process — holds the project's lock.
    Elsewhere(Busy),
}

/// The running and the last finished job of each kind, per member.
#[derive(Default)]
pub struct ScanJobs {
    slots: Mutex<HashMap<(JobKind, PathBuf), Arc<Job>>>,
    next_id: AtomicU64,
}

impl ScanJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a `kind` job for `member` running `work` on a thread of its own,
    /// or join the one already running. `work`'s `Ok` value is the job's
    /// result; an `Err` holding [`Cancelled`] ends it cancelled, any other
    /// error or a panic ends it failed. The slot is free again either way.
    ///
    /// # Errors
    /// The thread could not be created; no job is recorded then.
    pub fn start<F>(&self, kind: JobKind, member: &Path, work: F) -> io::Result<Started>
    where
        F: FnOnce(&JobContext) -> anyhow::Result<serde_json::Value> + Send + 'static,
    {
        Ok(self
            .start_with(kind, member, &[], None, work)?
            .expect("no kind excludes the start and no lock is taken"))
    }

    /// [`start`](Self::start), holding the project lock in `lock_file` for as
    /// long as the job runs ([`lock`], S-758), unless a job of one of
    /// `excluded` runs for `member` here ([`Refused::Running`]) or a scan or
    /// install started elsewhere holds the lock ([`Refused::Elsewhere`]):
    /// then nothing starts. A running job of `kind` itself is still joined,
    /// before the lock is looked at. The checks, the lock and the start happen
    /// under the runner's own lock, so two starts here never both run and a
    /// second start of the same kind always joins.
    ///
    /// # Errors
    /// As [`start`](Self::start), and the lock file's own I/O error.
    pub fn start_exclusive<F>(
        &self,
        kind: JobKind,
        member: &Path,
        lock_file: &Path,
        excluded: &[JobKind],
        work: F,
    ) -> io::Result<Result<Started, Refused>>
    where
        F: FnOnce(&JobContext) -> anyhow::Result<serde_json::Value> + Send + 'static,
    {
        self.start_with(kind, member, excluded, Some(lock_file), work)
    }

    fn start_with<F>(
        &self,
        kind: JobKind,
        member: &Path,
        excluded: &[JobKind],
        lock_file: Option<&Path>,
        work: F,
    ) -> io::Result<Result<Started, Refused>>
    where
        F: FnOnce(&JobContext) -> anyhow::Result<serde_json::Value> + Send + 'static,
    {
        let key = (kind, member.to_path_buf());
        let mut slots = lock(&self.slots);
        let blocking = excluded.iter().find_map(|other| {
            slots
                .get(&(*other, member.to_path_buf()))
                .filter(|job| job.is_running())
        });
        if let Some(blocking) = blocking {
            return Ok(Err(Refused::Running(blocking.snapshot())));
        }
        if let Some(running) = slots.get(&key).filter(|job| job.is_running()) {
            tracing::info!(target: "logos::jobs", kind = kind.as_str(), member = %member.display(), job = %running.id, "job joined");
            return Ok(Ok(Started { joined: true, job: running.snapshot() }));
        }
        let id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let held = match lock_file {
            None => None,
            Some(path) => match ProjectLock::take(path, &Holder::here(kind, Some(id)))? {
                Ok(held) => Some(held),
                Err(busy) => {
                    tracing::info!(target: "logos::jobs", kind = kind.as_str(), member = %member.display(), "job refused: {busy}");
                    return Ok(Err(Refused::Elsewhere(busy)));
                }
            },
        };
        let job = Arc::new(Job::new(id, kind));
        let context = JobContext { job: Arc::clone(&job) };
        let member_name = member.display().to_string();
        std::thread::Builder::new()
            .name(format!("logos-job-{}", kind.as_str()))
            .spawn(move || run(context, &member_name, held, work))?;
        tracing::info!(target: "logos::jobs", kind = kind.as_str(), member = %member.display(), job = %job.id, "job started");
        let snapshot = job.snapshot();
        slots.insert(key, job);
        Ok(Ok(Started { joined: false, job: snapshot }))
    }

    /// The running job of `kind` for `member`, else the last one that ran
    /// since this [`ScanJobs`] was made; `None` when none ever did.
    pub fn snapshot(&self, kind: JobKind, member: &Path) -> Option<JobSnapshot> {
        lock(&self.slots)
            .get(&(kind, member.to_path_buf()))
            .map(|job| job.snapshot())
    }

    /// Cancel the running job of `kind` for `member`: kill the process group
    /// of every child it has running and refuse any child it starts after.
    /// Returns the job as it stands — still `running` until its work notices
    /// and returns — or `None` when no job of that kind ever ran here. A job
    /// that has already ended is left as it is.
    pub fn cancel(&self, kind: JobKind, member: &Path) -> Option<JobSnapshot> {
        let job = lock(&self.slots).get(&(kind, member.to_path_buf())).cloned()?;
        if job.is_running() {
            tracing::info!(target: "logos::jobs", kind = kind.as_str(), member = %member.display(), job = %job.id, "job cancel");
            job.children.cancel();
        }
        Some(job.snapshot())
    }

    /// Cancel every running job, as [`cancel`](Self::cancel) does each one,
    /// then wait until each has ended or `grace` has passed. A cancelled job
    /// ends once its work notices — its children are already killed — so the
    /// wait is what lets the work clean up after itself (an install removes
    /// its partial download) before the process exits. Returns whether every
    /// job ended within `grace`.
    pub fn shutdown(&self, grace: Duration) -> bool {
        let running: Vec<Arc<Job>> = lock(&self.slots)
            .values()
            .filter(|job| job.is_running())
            .cloned()
            .collect();
        for job in &running {
            tracing::info!(target: "logos::jobs", kind = job.kind.as_str(), job = %job.id, "job cancel at shutdown");
            job.children.cancel();
        }
        let deadline = Instant::now() + grace;
        running.iter().all(|job| job.wait_until(deadline))
    }
}

/// Dropping the runner cancels every running job, killing each child's
/// process group, but does not wait for the jobs to end: `serve` calls
/// [`ScanJobs::shutdown`] for that. Dropping covers a runner torn down in
/// process without it, such as every test. Neither covers a process killed by
/// SIGKILL, or by a signal it does not handle (`serve` and the CLI's scans and
/// installs handle SIGINT and SIGTERM): no code runs then, and a child in a
/// process group of its own runs to its own end — beside a scan the freed
/// project lock then admits; the next install discards the staging such a
/// process left ([`crate::tools`]).
impl Drop for ScanJobs {
    fn drop(&mut self) {
        for job in lock(&self.slots).values() {
            job.children.cancel();
        }
    }
}

/// A job's own handle on itself, given to its work.
pub struct JobContext {
    job: Arc<Job>,
}

impl JobContext {
    pub fn id(&self) -> JobId {
        self.job.id
    }

    /// Report one event; it is kept for replay, to [`EVENT_CAP`].
    pub fn emit(&self, event: serde_json::Value) {
        let at_ms = millis(self.job.started);
        let mut inner = lock(&self.job.inner);
        let seq = inner.next_seq;
        inner.next_seq += 1;
        if inner.events.len() == EVENT_CAP {
            inner.events.pop_front();
            inner.events_dropped += 1;
        }
        inner.events.push_back(JobEvent { seq, at_ms, event });
    }

    /// Whether the job was cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.job.children.is_cancelled()
    }
}

/// Whether the job or [`foreground`] run on this thread was cancelled; `false`
/// on a thread that runs neither. Work deep inside a job checks it between steps, so a
/// cancel also stops what has not started yet.
pub fn cancelled() -> bool {
    CURRENT.with(|current| current.borrow().as_ref().is_some_and(|c| c.is_cancelled()))
}

/// Run `work` with this thread's children registered as a **foreground** run:
/// a scan or install the CLI or the MCP surface runs in the calling thread,
/// not as a job. Each child [`ChildGuard::spawn`] starts during it joins the
/// run, so [`cancel_foreground`] — what a stop signal calls before the process
/// ends, or a stopping `serve` — kills its whole process group, as a job's
/// cancel does, and [`cancelled`] then reads `true`. On a thread that already
/// runs a job or a foreground run, `work` runs within that one.
pub fn foreground<R>(work: impl FnOnce() -> R) -> R {
    if CURRENT.with(|current| current.borrow().is_some()) {
        return work();
    }
    let children = Arc::new(Children::default());
    lock(&FOREGROUND).push(Arc::clone(&children));
    CURRENT.with(|current| *current.borrow_mut() = Some(Arc::clone(&children)));
    /// Ends the run however `work` ends, a panic included.
    struct Ended(Arc<Children>);
    impl Drop for Ended {
        fn drop(&mut self) {
            CURRENT.with(|current| *current.borrow_mut() = None);
            lock(&FOREGROUND).retain(|run| !Arc::ptr_eq(run, &self.0));
        }
    }
    let _ended = Ended(children);
    work()
}

/// Cancel every [`foreground`] run in this process: kill the process group of
/// each child it has running and refuse any child it starts after. A process
/// killed by SIGKILL runs nothing, so its children run to their own end.
pub fn cancel_foreground() {
    for run in lock(&FOREGROUND).iter() {
        run.cancel();
    }
}

/// A child process started in a process group of its own and registered with
/// the job or [`foreground`] run on this thread, so that [`ScanJobs::cancel`]
/// or [`cancel_foreground`] kills its whole group. On a thread that runs
/// neither it is a plain spawn in a group of its own. Keep it alive until the
/// child's group is reaped; dropping it unregisters the child.
pub struct ChildGuard {
    registration: Option<(Arc<Children>, u32)>,
}

impl ChildGuard {
    /// Spawn `command` as the leader of a new process group (Unix) and register
    /// it with this thread's job.
    ///
    /// # Errors
    /// [`io::ErrorKind::Interrupted`] when this thread's job was already
    /// cancelled — nothing is started then — or the spawn's own error. A
    /// cancel that lands between the spawn and the registration kills the
    /// child's group at once.
    pub fn spawn(command: &mut Command) -> io::Result<(Child, ChildGuard)> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let Some(children) = CURRENT.with(|current| current.borrow().clone()) else {
            return Ok((command.spawn()?, ChildGuard { registration: None }));
        };
        if children.is_cancelled() {
            return Err(cancelled_error());
        }
        let child = command.spawn()?;
        let registered = {
            let mut set = lock(&children.set);
            if set.cancelled {
                false
            } else {
                set.pids.insert(child.id());
                true
            }
        };
        if !registered {
            kill_group(child.id());
        }
        let registration = registered.then(|| (children, child.id()));
        Ok((child, ChildGuard { registration }))
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some((children, pid)) = &self.registration {
            lock(&children.set).pids.remove(pid);
        }
    }
}

// ── internals ────────────────────────────────────────────────────────────────

thread_local! {
    /// The children of the job or [`foreground`] run on this thread, if one is.
    static CURRENT: RefCell<Option<Arc<Children>>> = const { RefCell::new(None) };
}

/// The children of every [`foreground`] run in this process.
static FOREGROUND: Mutex<Vec<Arc<Children>>> = Mutex::new(Vec::new());

struct Job {
    id: JobId,
    kind: JobKind,
    started: Instant,
    inner: Mutex<JobInner>,
    /// Notified when the job ends.
    ended: Condvar,
    children: Arc<Children>,
}

struct JobInner {
    state: JobState,
    events: VecDeque<JobEvent>,
    next_seq: u64,
    events_dropped: u64,
    result: Option<serde_json::Value>,
    error: Option<String>,
    /// Set when the job ends: what [`JobSnapshot::elapsed_ms`] then reads.
    ran_ms: Option<u64>,
}

impl Job {
    fn new(id: JobId, kind: JobKind) -> Self {
        Self {
            id,
            kind,
            started: Instant::now(),
            inner: Mutex::new(JobInner {
                state: JobState::Running,
                events: VecDeque::new(),
                next_seq: 0,
                events_dropped: 0,
                result: None,
                error: None,
                ran_ms: None,
            }),
            ended: Condvar::new(),
            children: Arc::new(Children::default()),
        }
    }

    fn is_running(&self) -> bool {
        lock(&self.inner).state == JobState::Running
    }

    fn snapshot(&self) -> JobSnapshot {
        let inner = lock(&self.inner);
        JobSnapshot {
            id: self.id,
            kind: self.kind,
            state: inner.state,
            elapsed_ms: inner.ran_ms.unwrap_or_else(|| millis(self.started)),
            events: inner.events.iter().cloned().collect(),
            events_dropped: inner.events_dropped,
            result: inner.result.clone(),
            error: inner.error.clone(),
        }
    }

    fn finish(&self, state: JobState, result: Option<serde_json::Value>, error: Option<String>) {
        let mut inner = lock(&self.inner);
        inner.state = state;
        inner.result = result;
        inner.error = error;
        inner.ran_ms = Some(millis(self.started));
        self.ended.notify_all();
    }

    /// Wait until the job has ended or `deadline` has passed; whether it ended.
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut inner = lock(&self.inner);
        while inner.state == JobState::Running {
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                return false;
            };
            inner = self
                .ended
                .wait_timeout(inner, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }
}

/// The process groups a job has running, and whether it was cancelled.
#[derive(Default)]
struct Children {
    set: Mutex<ChildSet>,
}

#[derive(Default)]
struct ChildSet {
    cancelled: bool,
    /// Each a group leader's pid, which is the group's id.
    pids: BTreeSet<u32>,
}

impl Children {
    fn is_cancelled(&self) -> bool {
        lock(&self.set).cancelled
    }

    /// Mark the job cancelled and kill every registered group. The lock is
    /// held throughout, so no child registers between the mark and the kills.
    fn cancel(&self) {
        let mut set = lock(&self.set);
        set.cancelled = true;
        for pid in &set.pids {
            kill_group(*pid);
        }
    }
}

/// The job thread's body: run `work` with this thread's job set and holding
/// `held`, then record how it ended. The lock is released before the job
/// reads ended, so whoever waited for the end finds the project free.
fn run<F>(context: JobContext, member: &str, held: Option<ProjectLock>, work: F)
where
    F: FnOnce(&JobContext) -> anyhow::Result<serde_json::Value>,
{
    CURRENT.with(|current| *current.borrow_mut() = Some(Arc::clone(&context.job.children)));
    let outcome = catch_unwind(AssertUnwindSafe(|| match &held {
        Some(held) => held.holding(|| work(&context)),
        None => work(&context),
    }));
    CURRENT.with(|current| *current.borrow_mut() = None);
    drop(held);
    let job = &context.job;
    let (state, result, error) = match outcome {
        Ok(Ok(value)) => (JobState::Completed, Some(value), None),
        Ok(Err(e)) if e.downcast_ref::<Cancelled>().is_some() || job.children.is_cancelled() => {
            (JobState::Cancelled, None, None)
        }
        Ok(Err(e)) => (JobState::Failed, None, Some(format!("{e:#}"))),
        Err(_) => (JobState::Failed, None, Some("the job panicked".to_string())),
    };
    job.finish(state, result, error);
    tracing::info!(
        target: "logos::jobs",
        kind = job.kind.as_str(),
        member,
        job = %job.id,
        state = ?state,
        duration_ms = millis(job.started),
        "job ended",
    );
}

fn cancelled_error() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, Cancelled)
}

/// SIGKILL the process group led by `pid` — a child [`ChildGuard::spawn`]
/// started as the leader of a group of its own. A group with no member left is
/// a no-op (`ESRCH`). The one kill site: a cancel here, and [`spawn::run`]
/// once a scanner's leader has exited.
#[cfg(unix)]
#[allow(unsafe_code)]
pub(crate) fn kill_group(pid: u32) {
    let Ok(group) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: `killpg` takes two integers and touches no memory of ours. The
    // group is one this process started; while any member is alive its id is
    // not reused. The id could name another group only after every member is
    // gone: `spawn::run` kills the group right after reaping its leader
    // and drops the child's registration at once, so a cancel's kill can land
    // on a stale id only in that same instant.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_group(_pid: u32) {}

fn millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A poisoned lock still holds consistent job state: every critical section
/// here is a few assignments that cannot panic halfway.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(all(test, unix))]
mod tests;
