//! A stop signal with no surface left to shut its jobs down (S-738): what
//! `stop_on_signal` calls, [`logos_core::jobs::cancel_every_run`], kills the
//! children of every running job of every runner in the process, as well as
//! every foreground run's.
//!
//! Its own test binary, holding one test: the call reaches every run in the
//! process, so it must not share one with tests whose jobs it would cancel.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use logos_core::jobs::{AnalysisJobs, Cancelled, ChildGuard, JobKind, JobState};

/// Whether `pid` names a process (a zombie included).
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(libc::pid_t::try_from(pid).unwrap(), 0) == 0
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cancel_every_run_kills_every_running_jobs_children() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Two runners, as a `serve --mcp` and a test harness might hold.
    let runners = [AnalysisJobs::new(), AnalysisJobs::new()];
    let mut pid_files = Vec::new();
    for (n, (jobs, kind)) in runners.iter().zip([JobKind::CoverageRun, JobKind::SecurityScan]).enumerate() {
        let pids = tmp.path().join(format!("{n}.pids"));
        let script = format!("/bin/sleep 60 & echo $$ $! > '{p}.tmp' && mv '{p}.tmp' '{p}'; wait", p = pids.display());
        jobs.start(kind, tmp.path(), move |_| {
            let (mut child, _guard) = ChildGuard::spawn(Command::new("/bin/sh").args(["-c", &script]))?;
            child.wait()?;
            Err(Cancelled.into())
        })
        .unwrap();
        pid_files.push(pids);
    }
    let mut pids = Vec::new();
    for file in &pid_files {
        wait_for("a job's child never started", || file.exists());
        pids.extend(std::fs::read_to_string(file).unwrap().split_whitespace().map(|p| p.parse::<u32>().unwrap()));
    }
    assert!(pids.iter().all(|&pid| alive(pid)), "every child runs before the cancel");

    logos_core::jobs::cancel_every_run();

    for pid in pids {
        wait_for(&format!("process {pid} outlived cancel_every_run"), || !alive(pid));
    }
    for (jobs, kind) in runners.iter().zip([JobKind::CoverageRun, JobKind::SecurityScan]) {
        wait_for("a cancelled job never ended", || {
            jobs.snapshot(kind, Path::new(tmp.path())).is_some_and(|job| job.state == JobState::Cancelled)
        });
    }
}
