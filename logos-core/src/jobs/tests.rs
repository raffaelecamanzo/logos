//! The job runner on its own: single flight, cancel, replay and failure.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::*;

const KIND: JobKind = JobKind::SecurityScan;

/// The job's snapshot once it is no longer running.
fn ended(jobs: &AnalysisJobs, member: &Path) -> JobSnapshot {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = jobs.snapshot(KIND, member).expect("a job ran");
        if snapshot.state != JobState::Running {
            return snapshot;
        }
        assert!(Instant::now() < deadline, "the job is still running: {snapshot:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether `pid` names a process (a zombie included).
pub(crate) fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(libc::pid_t::try_from(pid).unwrap(), 0) == 0
    }
}

/// Wait until `pid` is gone; an orphan is reaped by init shortly after it dies.
pub(crate) fn gone(pid: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

#[test]
fn a_second_start_while_one_runs_joins_it_and_the_work_runs_once() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    let runs = Arc::new(AtomicUsize::new(0));
    let (release, hold) = mpsc::channel::<()>();
    let counted = Arc::clone(&runs);
    let first = jobs
        .start(KIND, member, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            hold.recv().ok();
            Ok(serde_json::json!("done"))
        })
        .unwrap();
    let counted = Arc::clone(&runs);
    let second = jobs
        .start(KIND, member, move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::json!("a second run"))
        })
        .unwrap();
    assert!(!first.joined);
    assert!(second.joined, "the second start joins: {second:?}");
    assert_eq!(second.job.id, first.job.id);

    release.send(()).unwrap();
    let done = ended(&jobs, member);
    assert_eq!((done.state, done.result), (JobState::Completed, Some(serde_json::json!("done"))));
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the joined start ran nothing");

    // Once it ended the slot is free: the next start is a new job.
    let third = jobs.start(KIND, member, |_| Ok(serde_json::Value::Null)).unwrap();
    assert!(!third.joined);
    assert_ne!(third.job.id, first.job.id);
}

#[test]
fn another_member_runs_its_own_job() {
    let jobs = AnalysisJobs::new();
    let (release, hold) = mpsc::channel::<()>();
    let a = jobs
        .start(KIND, Path::new("/a"), move |_| {
            hold.recv().ok();
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    let b = jobs.start(KIND, Path::new("/b"), |_| Ok(serde_json::Value::Null)).unwrap();
    assert!(!b.joined, "a job for another member is not joined");
    assert_ne!(a.job.id, b.job.id);
    release.send(()).unwrap();
    assert_eq!(ended(&jobs, Path::new("/a")).state, JobState::Completed);
}

/// S-738: the coverage run is a kind of its own, spelled `coverage-run` on the
/// wire. A coverage run and a security scan of one member run side by side —
/// neither joins nor refuses the other — and cancelling one leaves the other
/// running.
#[test]
fn a_coverage_run_and_a_security_scan_of_one_member_run_side_by_side() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    let (release_scan, hold_scan) = mpsc::channel::<()>();
    let scan = jobs
        .start(JobKind::SecurityScan, member, move |_| {
            hold_scan.recv().ok();
            Ok(serde_json::json!("scanned"))
        })
        .unwrap();
    let coverage = jobs
        .start(JobKind::CoverageRun, member, |ctx| {
            while !ctx.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(Cancelled.into())
        })
        .unwrap();
    assert!(!coverage.joined, "a coverage run never joins the scan: {coverage:?}");
    assert_ne!(coverage.job.id, scan.job.id);
    assert_eq!(serde_json::to_value(coverage.job.kind).unwrap(), "coverage-run");
    assert_eq!(JobKind::CoverageRun.as_str(), "coverage-run");
    let running = |kind| jobs.snapshot(kind, member).unwrap().state == JobState::Running;
    assert!(running(JobKind::SecurityScan) && running(JobKind::CoverageRun), "both run at once");

    jobs.cancel(JobKind::CoverageRun, member);
    let deadline = Instant::now() + Duration::from_secs(20);
    while running(JobKind::CoverageRun) {
        assert!(Instant::now() < deadline, "the coverage run never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(jobs.snapshot(JobKind::CoverageRun, member).unwrap().state, JobState::Cancelled);
    assert!(running(JobKind::SecurityScan), "cancelling the coverage run left the scan running");
    release_scan.send(()).unwrap();
    assert_eq!(ended(&jobs, member).state, JobState::Completed);
}

/// S-738: a build daemon that detaches from the tool — its parent shell exits,
/// so it is re-parented away from the job — still sits in the tool's process
/// group, and the cancel kills it with the group.
#[test]
fn a_detached_daemon_in_the_tools_group_dies_with_the_cancel() {
    let tmp = TempDir::new().unwrap();
    let pids = tmp.path().join("pids");
    let member = tmp.path();
    let jobs = AnalysisJobs::new();
    let script = format!("(/bin/sleep 60 & echo $! > '{p}.tmp' && mv '{p}.tmp' '{p}'); /bin/sleep 60", p = pids.display());
    jobs.start(JobKind::CoverageRun, member, move |_| {
        let (mut child, _guard) = ChildGuard::spawn(Command::new("/bin/sh").args(["-c", &script]))?;
        child.wait()?;
        Err(Cancelled.into())
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let daemon = loop {
        if let Ok(pid) = std::fs::read_to_string(&pids) {
            break pid.trim().parse::<u32>().unwrap();
        }
        assert!(Instant::now() < deadline, "the daemon never started");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(alive(daemon), "the daemon runs before the cancel");
    jobs.cancel(JobKind::CoverageRun, member);
    assert!(gone(daemon), "the detached daemon {daemon} outlived the cancel");
}

/// The child runs a shell that leaves a grandchild behind; cancel kills the
/// whole group, so neither survives, and the work's `Cancelled` ends the job
/// cancelled.
#[test]
fn cancel_kills_every_child_group_and_ends_the_job_cancelled() {
    let tmp = TempDir::new().unwrap();
    let pids = tmp.path().join("pids");
    let member = tmp.path();
    let jobs = AnalysisJobs::new();
    let script = format!("sleep 60 & echo $$ $! > '{}'; wait", pids.display());
    jobs.start(KIND, member, move |ctx| {
        let (mut child, _guard) = ChildGuard::spawn(Command::new("/bin/sh").args(["-c", &script]))?;
        child.wait()?;
        if ctx.is_cancelled() {
            return Err(Cancelled.into());
        }
        Ok(serde_json::json!("finished by itself"))
    })
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    let recorded = loop {
        if let Some(line) = std::fs::read_to_string(&pids).ok().filter(|s| s.ends_with('\n')) {
            break line;
        }
        assert!(Instant::now() < deadline, "the child never started");
        std::thread::sleep(Duration::from_millis(10));
    };
    let [shell, sleeper]: [u32; 2] = recorded
        .split_whitespace()
        .map(|p| p.parse().unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    assert!(alive(sleeper), "the grandchild runs before the cancel");

    let cancelling = jobs.cancel(KIND, member).expect("the job is known");
    // Running until the work notices, which may already have happened.
    assert!(matches!(cancelling.state, JobState::Running | JobState::Cancelled), "{cancelling:?}");
    let done = ended(&jobs, member);
    assert_eq!((done.state, done.result, done.error), (JobState::Cancelled, None, None));
    assert!(gone(shell), "the child {shell} survived the cancel");
    assert!(gone(sleeper), "the grandchild {sleeper} survived the cancel");
}

#[test]
fn nothing_starts_after_a_cancel() {
    let tmp = TempDir::new().unwrap();
    let marker = tmp.path().join("started");
    let member = tmp.path();
    let jobs = AnalysisJobs::new();
    let (go, wait_for_cancel) = mpsc::channel::<()>();
    let target = marker.clone();
    jobs.start(KIND, member, move |_| {
        wait_for_cancel.recv().ok();
        assert!(cancelled(), "the job thread sees its cancel");
        let refused = ChildGuard::spawn(Command::new("/usr/bin/touch").arg(&target)).map(|_| ());
        let error = refused.expect_err("a spawn after the cancel is refused");
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        Err(Cancelled.into())
    })
    .unwrap();
    jobs.cancel(KIND, member);
    go.send(()).unwrap();
    assert_eq!(ended(&jobs, member).state, JobState::Cancelled);
    assert!(!marker.exists(), "a child started after the cancel");
}

#[test]
fn a_failing_or_panicking_job_fails_and_frees_its_slot() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    jobs.start(KIND, member, |_| Err(anyhow::anyhow!("the scanner config is invalid"))).unwrap();
    let failed = ended(&jobs, member);
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(failed.error.as_deref(), Some("the scanner config is invalid"));

    let panicking = jobs.start(KIND, member, |_| panic!("a bug in the work")).unwrap();
    assert!(!panicking.joined, "a failed job holds no slot");
    let panicked = ended(&jobs, member);
    assert_eq!((panicked.state, panicked.error.as_deref()), (JobState::Failed, Some("the job panicked")));
    assert!(!jobs.start(KIND, member, |_| Ok(serde_json::Value::Null)).unwrap().joined);
}

#[test]
fn events_replay_in_order_up_to_the_cap() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    jobs.start(KIND, member, |ctx| {
        for i in 0..EVENT_CAP + 5 {
            ctx.emit(serde_json::json!({ "step": i }));
        }
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    let done = ended(&jobs, member);
    assert_eq!(done.events.len(), EVENT_CAP);
    assert_eq!(done.events_dropped, 5);
    assert_eq!(done.events[0].seq, 5, "the oldest are dropped");
    assert_eq!(done.events[0].event, serde_json::json!({ "step": 5 }));
    assert_eq!(done.events.last().unwrap().seq, (EVENT_CAP + 4) as u64);
}

#[test]
fn outside_a_job_a_child_is_a_plain_spawn_and_nothing_is_cancelled() {
    assert!(!cancelled());
    let mut command = Command::new("/usr/bin/true");
    let (mut child, _guard) = ChildGuard::spawn(&mut command).unwrap();
    assert!(child.wait().unwrap().success());
    assert!(AnalysisJobs::new().cancel(KIND, Path::new("/never")).is_none());
}

/// A serve shutting down drops its runner: every child a running job holds is
/// killed with its group.
#[test]
fn dropping_the_runner_kills_what_its_jobs_run() {
    let tmp = TempDir::new().unwrap();
    let pids = tmp.path().join("pids");
    let jobs = AnalysisJobs::new();
    let script = format!("sleep 60 & echo $$ $! > '{}'; wait", pids.display());
    jobs.start(KIND, tmp.path(), move |_| {
        let (mut child, _guard) = ChildGuard::spawn(Command::new("/bin/sh").args(["-c", &script]))?;
        child.wait()?;
        Err(Cancelled.into())
    })
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let recorded = loop {
        if let Some(line) = std::fs::read_to_string(&pids).ok().filter(|s| s.ends_with('\n')) {
            break line;
        }
        assert!(Instant::now() < deadline, "the child never started");
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(jobs);
    for pid in recorded.split_whitespace().map(|p| p.parse::<u32>().unwrap()) {
        assert!(gone(pid), "process {pid} outlived the runner");
    }
}

/// Shutdown cancels every running job, of each kind and member, and returns
/// once each has ended — not before, and not at the end of its grace: the
/// children are gone and the cleanup the work does after the cancel has run.
#[test]
fn shutdown_cancels_every_running_job_and_waits_for_each_to_end() {
    let tmp = TempDir::new().unwrap();
    let jobs = AnalysisJobs::new();
    let mut started = Vec::new();
    for (kind, name) in [(JobKind::SecurityScan, "a"), (JobKind::ToolInstall, "b")] {
        let member = tmp.path().join(name);
        let pids = tmp.path().join(format!("{name}.pids"));
        let cleaned = tmp.path().join(format!("{name}.cleaned"));
        let script = format!("sleep 60 & echo $$ $! > '{}'; wait", pids.display());
        let marker = cleaned.clone();
        jobs.start(kind, &member, move |ctx| {
            let (mut child, _guard) = ChildGuard::spawn(Command::new("/bin/sh").args(["-c", &script]))?;
            child.wait()?;
            // Cleanup that takes a while, as removing a partial download may.
            std::thread::sleep(Duration::from_millis(200));
            std::fs::write(&marker, "")?;
            if ctx.is_cancelled() {
                return Err(Cancelled.into());
            }
            Ok(serde_json::json!("finished by itself"))
        })
        .unwrap();
        started.push((kind, member, pids, cleaned));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.iter().all(|(.., pids, _)| std::fs::read_to_string(pids).is_ok_and(|s| s.ends_with('\n'))) {
        assert!(Instant::now() < deadline, "the children never started");
        std::thread::sleep(Duration::from_millis(10));
    }

    let began = Instant::now();
    assert!(jobs.shutdown(Duration::from_secs(30)), "every job ended within the grace");
    // Woken as the last job ends, not at the end of the grace.
    let waited = began.elapsed();
    assert!(waited < Duration::from_secs(5), "shutdown returned once the jobs ended: {waited:?}");
    for (kind, member, pids, cleaned) in &started {
        let job = jobs.snapshot(*kind, member).expect("the job is known");
        assert_eq!(job.state, JobState::Cancelled, "ended by the time shutdown returns: {job:?}");
        assert!(cleaned.exists(), "{kind:?}: its cleanup ran before shutdown returned");
        for pid in std::fs::read_to_string(pids).unwrap().split_whitespace().map(|p| p.parse::<u32>().unwrap()) {
            assert!(gone(pid), "{kind:?}: process {pid} outlived the shutdown");
        }
    }
}

/// A job whose work never notices its cancel holds shutdown for the grace
/// alone, and shutdown says it did not end.
#[test]
fn shutdown_waits_no_longer_than_its_grace() {
    let jobs = AnalysisJobs::new();
    let (release, hold) = mpsc::channel::<()>();
    jobs.start(KIND, Path::new("/project"), move |_| {
        hold.recv().ok();
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    let began = Instant::now();
    assert!(!jobs.shutdown(Duration::from_millis(200)), "the job is still running");
    let waited = began.elapsed();
    assert!(waited >= Duration::from_millis(200), "it waited the grace: {waited:?}");
    assert!(waited < Duration::from_secs(5), "and no longer: {waited:?}");
    release.send(()).unwrap();
}

/// With nothing running, shutdown returns at once.
#[test]
fn shutdown_with_no_running_job_returns_at_once() {
    let jobs = AnalysisJobs::new();
    jobs.start(KIND, Path::new("/project"), |_| Ok(serde_json::Value::Null)).unwrap();
    let _ = ended(&jobs, Path::new("/project"));
    let began = Instant::now();
    assert!(jobs.shutdown(Duration::from_secs(30)));
    assert!(began.elapsed() < Duration::from_secs(5));
}

/// A child's registration ends with its guard: a cancel after the spawning
/// code is done with the child never reaches for its (possibly reused) id.
#[test]
fn a_dropped_guard_unregisters_its_child() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    jobs.start(KIND, member, |ctx| {
        let mut command = Command::new("/usr/bin/true");
        let (mut child, guard) = ChildGuard::spawn(&mut command)?;
        child.wait()?;
        let registered = |ctx: &JobContext| lock(&ctx.job.children.set).pids.len();
        assert_eq!(registered(ctx), 1, "the child is registered while its guard lives");
        drop(guard);
        assert_eq!(registered(ctx), 0, "a dropped guard leaves the child registered");
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    let done = ended(&jobs, member);
    assert_eq!((done.state, done.error), (JobState::Completed, None));
}

/// `jobs::spawn::run` has released the registration by the time it
/// returns (that it does so right after the group is killed is not observable).
#[test]
fn a_finished_scanner_run_leaves_nothing_registered() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    jobs.start(KIND, member, |ctx| {
        let launch = crate::jobs::spawn::Launch {
            cwd: Path::new("/"),
            env: &[],
            vars: &[],
            isolated: None,
            path: None,
            deadline: None,
        };
        let finished = crate::jobs::spawn::run(Path::new("/bin/echo"), &["hi".into()], &launch)?;
        assert_eq!(finished.stdout.trim(), "hi");
        assert!(lock(&ctx.job.children.set).pids.is_empty(), "a finished run left its child registered");
        Ok(serde_json::Value::Null)
    })
    .unwrap();
    let done = ended(&jobs, member);
    assert_eq!((done.state, done.error), (JobState::Completed, None));
}

/// A cancel after the job ended leaves it as it ended, result included.
#[test]
fn a_cancel_after_the_end_changes_nothing() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    jobs.start(KIND, member, |_| Ok(serde_json::json!("done"))).unwrap();
    let done = ended(&jobs, member);
    let after = jobs.cancel(KIND, member).expect("the job is known");
    assert_eq!((after.state, after.result.clone()), (JobState::Completed, Some(serde_json::json!("done"))));
    assert_eq!(after, jobs.snapshot(KIND, member).unwrap());
    assert_eq!(after.elapsed_ms, done.elapsed_ms, "an ended job's time is frozen");
}

/// Work that a cancel stopped with an error of its own — not [`Cancelled`] —
/// still ends the job cancelled, never failed.
#[test]
fn any_error_after_a_cancel_ends_the_job_cancelled() {
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    let (go, wait_for_cancel) = mpsc::channel::<()>();
    jobs.start(KIND, member, move |_| {
        wait_for_cancel.recv().ok();
        Err(anyhow::anyhow!("reading the report: interrupted"))
    })
    .unwrap();
    jobs.cancel(KIND, member);
    go.send(()).unwrap();
    let done = ended(&jobs, member);
    assert_eq!((done.state, done.error), (JobState::Cancelled, None));
}

/// A kind started exclusive of another (S-747: the scanner install and the
/// scan) is refused while that other kind runs for the member, with its job
/// returned and the work never run; another member, or the same kind, is not
/// affected; once the other ends, it starts.
#[test]
fn an_exclusive_start_is_refused_while_an_excluded_kind_runs() {
    let tmp = TempDir::new().unwrap();
    let jobs = AnalysisJobs::new();
    let member = Path::new("/project");
    let lock_file = tmp.path().join("project.lock");
    let (release, hold) = mpsc::channel::<()>();
    let scan = jobs
        .start(JobKind::SecurityScan, member, move |_| {
            hold.recv().ok();
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let refused = jobs
        .start_exclusive(JobKind::ToolInstall, member, &lock_file, &[JobKind::SecurityScan], move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    let Err(Refused::Running(blocking)) = refused else {
        panic!("an install beside a running scan is refused by that scan: {refused:?}");
    };
    assert_eq!((blocking.id, blocking.kind), (scan.job.id, JobKind::SecurityScan));
    assert!(jobs.snapshot(JobKind::ToolInstall, member).is_none(), "no install job is recorded");

    // Another member's install is independent of this member's scan.
    let other = jobs
        .start_exclusive(JobKind::ToolInstall, Path::new("/other"), &tmp.path().join("other.lock"), &[JobKind::SecurityScan], |_| {
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    assert!(other.is_ok(), "{other:?}");

    release.send(()).unwrap();
    assert_eq!(ended(&jobs, member).state, JobState::Completed);
    let started = jobs
        .start_exclusive(JobKind::ToolInstall, member, &lock_file, &[JobKind::SecurityScan], move |_| {
            Ok(serde_json::json!("installed"))
        })
        .unwrap()
        .expect("the scan ended, so the install starts");
    assert_eq!((started.joined, started.job.kind), (false, JobKind::ToolInstall));
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the refused work never ran");
}

/// S-758: an exclusive job holds the project lock for as long as it runs — its
/// work holds it on its own thread, a peek names the job, a second start of the
/// same kind still joins it — and releases it before it reads ended, so the
/// next start is never refused by a job that has finished.
#[test]
fn an_exclusive_job_holds_the_project_lock_while_it_runs_and_a_second_start_joins() {
    let tmp = TempDir::new().unwrap();
    let jobs = AnalysisJobs::new();
    let member = tmp.path();
    let lock_file = tmp.path().join("run.lock");
    let (seen, inside) = mpsc::channel::<bool>();
    let (release, hold) = mpsc::channel::<()>();
    let in_job = lock_file.clone();
    let first = jobs
        .start_exclusive(KIND, member, &lock_file, &[], move |_| {
            seen.send(lock::held_here(&in_job)).unwrap();
            hold.recv().ok();
            Ok(serde_json::Value::Null)
        })
        .unwrap()
        .expect("the project is free");
    assert!(inside.recv_timeout(Duration::from_secs(10)).unwrap(), "the job's work holds the lock on its thread");

    let busy = lock::peek(&lock_file).unwrap().expect("the running job holds the lock");
    let holder = busy.holder.expect("the job wrote itself into the lock");
    assert_eq!((holder.kind, holder.job, holder.pid), (KIND, Some(first.job.id), std::process::id()));

    let second = jobs
        .start_exclusive(KIND, member, &lock_file, &[], |_| Ok(serde_json::json!("a second scan")))
        .unwrap()
        .expect("a second start of the same kind is not refused by the lock its job holds");
    assert_eq!((second.joined, second.job.id), (true, first.job.id));

    release.send(()).unwrap();
    assert_eq!(ended(&jobs, member).result, Some(serde_json::Value::Null), "the first job's work ran, not the joined one's");
    assert_eq!(lock::peek(&lock_file).unwrap(), None, "the lock is free by the time the job reads ended");
}

/// S-758: while a scan or install started elsewhere holds the project lock, an
/// exclusive start is refused naming it; its work never runs and no job is
/// recorded.
#[test]
fn an_exclusive_start_is_refused_while_the_lock_is_held_elsewhere() {
    let tmp = TempDir::new().unwrap();
    let jobs = AnalysisJobs::new();
    let lock_file = tmp.path().join("run.lock");
    let cli = lock::Holder { kind: JobKind::ToolInstall, surface: Some("cli".into()), job: None, pid: 4242, since: 0 };
    let held = lock::ProjectLock::take(&lock_file, &cli).unwrap().unwrap();

    let ran = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ran);
    let refused = jobs
        .start_exclusive(KIND, tmp.path(), &lock_file, &[JobKind::ToolInstall], move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
            Ok(serde_json::Value::Null)
        })
        .unwrap();
    let Err(Refused::Elsewhere(busy)) = refused else {
        panic!("the held lock refuses the start: {refused:?}");
    };
    assert_eq!(busy.holder, Some(cli));
    assert!(jobs.snapshot(KIND, tmp.path()).is_none(), "no job is recorded");
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the refused work never ran");

    drop(held);
    let started = jobs
        .start_exclusive(KIND, tmp.path(), &lock_file, &[], |_| Ok(serde_json::Value::Null))
        .unwrap()
        .expect("free once the other holder let go");
    assert!(!started.joined);
}

/// FR-SC-27: a private file is created new with mode 0600, never written
/// through a symbolic link in its place, outlives a child killed at its
/// deadline only while it is held, and is gone once dropped.
#[cfg(unix)]
#[test]
fn a_private_file_is_the_owners_only_and_is_removed_whatever_ended_the_run() {
    use std::os::unix::fs::PermissionsExt;

    use super::spawn::{self, Launch, PrivateFile};

    let tmp = TempDir::new().unwrap();
    let outside = tmp.path().join("outside");
    std::fs::write(&outside, "untouched").unwrap();
    let path = tmp.path().join("secret.properties");
    std::os::unix::fs::symlink(&outside, &path).unwrap();

    let file = PrivateFile::create(&path, b"nvd.api.key=k\n").unwrap();
    let meta = std::fs::symlink_metadata(&path).unwrap();
    assert!(meta.is_file(), "the link was replaced by a file");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "nvd.api.key=k\n");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");

    let launch = Launch { cwd: tmp.path(), env: &[], vars: &[], isolated: None, path: None, deadline: Some(Duration::from_millis(100)) };
    let finished = spawn::run(Path::new("/bin/sleep"), &["5".into()], &launch).unwrap();
    assert_eq!(finished.status, None, "the child was killed at its deadline");
    assert!(path.exists(), "held across the run");
    drop(file);
    assert!(std::fs::symlink_metadata(&path).is_err(), "the file outlived its holder");
}

/// FR-SC-27, S-794 review: a private file created in a foreground run is
/// removed the moment the run is cancelled, while the thread still holds it —
/// a process stopping on a signal exits right after the cancel, unwinding
/// nothing — and a run already cancelled writes none.
#[cfg(unix)]
#[test]
fn cancelling_a_run_removes_the_private_files_it_holds_and_refuses_new_ones() {
    use super::spawn::PrivateFile;

    let tmp = TempDir::new().unwrap();
    let held = tmp.path().join("held.properties");
    let late = tmp.path().join("late.properties");
    foreground(|| {
        let file = PrivateFile::create(&held, b"nvd.api.key=k\n").unwrap();
        assert!(held.exists());
        // This run's cancel alone: `cancel_foreground` would reach every
        // foreground run of the test binary.
        CURRENT.with(|current| current.borrow().clone()).expect("a foreground run").cancel();
        assert!(std::fs::symlink_metadata(&held).is_err(), "the cancel left the file behind");
        let refused = PrivateFile::create(&late, b"nvd.api.key=k\n").err().expect("a cancelled run writes no file");
        assert_eq!(refused.kind(), io::ErrorKind::Interrupted);
        assert!(std::fs::symlink_metadata(&late).is_err());
        drop(file);
    });
    // Created outside any run, it is still removed on drop.
    let plain = PrivateFile::create(&late, b"x").unwrap();
    drop(plain);
    assert!(std::fs::symlink_metadata(&late).is_err());
}

/// HF-8: a stream past its bound keeps its head *and* its tail with one line
/// between them naming the bytes left out — where a tool says why it failed is
/// the end — and never more than the bound; one that fits is kept whole.
#[cfg(unix)]
#[test]
fn a_stream_past_its_bound_keeps_its_head_and_its_tail_and_one_that_fits_is_whole() {
    use super::spawn::{self, Launch, STDERR_CAP, STDOUT_CAP};

    let tmp = TempDir::new().unwrap();
    let launch = Launch { cwd: tmp.path(), env: &[], vars: &[], isolated: None, path: None, deadline: None };
    let script = |name: &str, body: &str| {
        let path = tmp.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        path
    };
    // Filler, bracketed below by a first line and a last one.
    let flood = |bytes: usize| format!("/usr/bin/head -c {bytes} /dev/zero | /usr/bin/tr '\\0' x");
    let long = script(
        "long",
        &format!(
            "printf 'FIRST-LINE\\n'; {out}; printf '\\nFAILED marker-test\\n'\n\
             printf 'FIRST-ERR\\n' >&2; {err} >&2; printf '\\nLAST-ERR\\n' >&2",
            out = flood(STDOUT_CAP * 3),
            err = flood(STDERR_CAP * 3),
        ),
    );
    let f = spawn::run(&long, &[], &launch).unwrap();
    for (name, text, cap, first, last) in [
        ("stdout", &f.stdout, STDOUT_CAP, "FIRST-LINE", "FAILED marker-test\n"),
        ("stderr", &f.stderr, STDERR_CAP, "FIRST-ERR", "LAST-ERR\n"),
    ] {
        assert!(text.len() <= cap, "{name}: {} bytes kept of a bound of {cap}", text.len());
        assert!(text.starts_with(first), "{name} keeps its head");
        assert!(text.ends_with(last), "{name} keeps its tail: {:?}", &text[text.len().saturating_sub(40)..]);
        assert_eq!(text.matches("bytes omitted").count(), 1, "{name}: exactly one omission note");
        assert!(text.len() > cap - 256, "{name}: the bound is used ({} of {cap})", text.len());
    }
    // The note's count is what was left out: with the note, and the newline
    // that sets it on a line of its own, the rest is what was kept.
    let total = "FIRST-LINE\n".len() + STDOUT_CAP * 3 + "\nFAILED marker-test\n".len();
    let note = f.stdout.lines().find(|l| l.contains("bytes omitted")).unwrap();
    let omitted: usize = note.trim_start_matches("[… ").split(' ').next().unwrap().parse().unwrap();
    let kept = f.stdout.len() - note.len() - 2;
    assert_eq!(omitted + kept, total, "{note}");

    let short = script("short", "printf 'line one\\nFAILED marker-test\\n'; printf 'warn\\n' >&2");
    let f = spawn::run(&short, &[], &launch).unwrap();
    assert_eq!((f.stdout.as_str(), f.stderr.as_str()), ("line one\nFAILED marker-test\n", "warn\n"));

    // Exactly the bound: still whole, no note; one byte past it: cut, noted, and within it.
    let exact = script("exact", &format!("{} >&2", flood(STDERR_CAP)));
    let f = spawn::run(&exact, &[], &launch).unwrap();
    assert_eq!(f.stderr.len(), STDERR_CAP);
    assert!(!f.stderr.contains("omitted"), "a stream that fits the bound is kept whole");
    let past = script("past", &format!("{} >&2", flood(STDERR_CAP + 1)));
    let f = spawn::run(&past, &[], &launch).unwrap();
    assert!(f.stderr.len() <= STDERR_CAP, "{} bytes kept", f.stderr.len());
    assert_eq!(f.stderr.matches("bytes omitted").count(), 1);

    // A head that ends on a line end gets no blank line before the note.
    let line = "y".repeat(63);
    let lines = script("lines", &format!("/usr/bin/yes {line} | /usr/bin/head -c {}", STDOUT_CAP * 2));
    let f = spawn::run(&lines, &[], &launch).unwrap();
    assert!(f.stdout.contains(&format!("{line}\n[… ")), "the note follows the head's last line");
    assert!(!f.stdout.contains("\n\n[… "), "no blank line before the note");

    // The notes in a capture are found, and a non-note is not.
    let text = f.stdout.clone() + "[… x bytes omitted …]\n[…  bytes omitted …]\n";
    assert_eq!(spawn::omission_notes(&text).len(), 1);
    let note = spawn::omission_notes(&f.stdout)[0].clone();
    assert!(f.stdout[note.clone()].starts_with("[… ") && f.stdout[note].ends_with(" bytes omitted …]\n"));
}
