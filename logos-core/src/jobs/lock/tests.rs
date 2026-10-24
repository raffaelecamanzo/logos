//! The project lock on its own (S-758): exclusion within a process and across
//! processes, the holder a refusal names, re-entry on the holding thread, and
//! the lock file's symlink rule.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::observability::{in_surface, Surface};

fn holder(kind: JobKind, surface: Option<&str>, job: Option<u64>) -> Holder {
    Holder { kind, surface: surface.map(str::to_string), job: job.map(JobId), pid: std::process::id(), since: unix_now() }
}

#[test]
fn a_second_take_is_busy_and_names_the_running_job() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let web = holder(JobKind::SecurityScan, Some("web"), Some(3));
    let _held = ProjectLock::take(&path, &web).unwrap().expect("the lock is free");

    let busy = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("mcp"), None))
        .unwrap()
        .expect_err("a second open of the lock in this process is refused");
    assert_eq!(busy.holder.as_ref(), Some(&web));
    let message = busy.to_string();
    let pid = std::process::id();
    assert!(
        message.starts_with(&format!(
            "a security scan is already running on this project: job 3, started from the web UI by Logos process {pid} "
        )),
        "{message}"
    );
    assert!(message.ends_with("s ago; wait for it to finish or cancel it, then try again"), "{message}");
}

#[test]
fn a_refusal_names_a_cli_or_mcp_holder_without_a_job() {
    let busy = |kind, surface| Busy { holder: Some(holder(kind, surface, None)) }.to_string();
    let pid = std::process::id();
    assert!(
        busy(JobKind::ToolInstall, Some("cli"))
            .starts_with(&format!("a tool install is already running on this project: started from the command line by Logos process {pid} ")),
        "{}",
        busy(JobKind::ToolInstall, Some("cli"))
    );
    assert!(
        busy(JobKind::SecurityScan, Some("mcp")).contains(&format!(": started over MCP by Logos process {pid} ")),
        "{}",
        busy(JobKind::SecurityScan, Some("mcp"))
    );
    assert!(busy(JobKind::SecurityScan, None).contains(&format!(": started by Logos process {pid} ")));
}

#[test]
fn a_dropped_lock_is_free_again_and_its_file_names_no_one() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let held = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None)).unwrap().unwrap();
    assert!(!fs::read_to_string(&path).unwrap().is_empty(), "the holder wrote itself");
    drop(held);
    assert_eq!(fs::read_to_string(&path).unwrap(), "", "a released lock names no one");
    assert_eq!(peek(&path).unwrap(), None);
    ProjectLock::take(&path, &holder(JobKind::ToolInstall, Some("cli"), None)).unwrap().expect("the lock is free again");
}

#[test]
fn peek_names_the_holder_without_taking_the_lock() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    assert_eq!(peek(&path).unwrap(), None, "no file, no holder");
    assert!(!path.exists(), "peek creates no lock file");

    let web = holder(JobKind::ToolInstall, Some("web"), Some(7));
    let held = ProjectLock::take(&path, &web).unwrap().unwrap();
    assert_eq!(peek(&path).unwrap().and_then(|busy| busy.holder), Some(web.clone()));
    assert_eq!(peek(&path).unwrap().and_then(|busy| busy.holder), Some(web), "peeking took nothing");
    drop(held);
    assert_eq!(peek(&path).unwrap(), None);
}

/// The holding thread runs a nested scan or install straight away; any other
/// thread is refused, naming the holder on the surface it was taken on.
#[test]
fn exclusively_reenters_on_the_holding_thread_and_refuses_every_other() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let (entered, inside) = mpsc::channel::<()>();
    let (release, hold) = mpsc::channel::<()>();
    let holding = {
        let path = path.clone();
        std::thread::spawn(move || {
            in_surface(Surface::Mcp, || {
                exclusively(&path, JobKind::ToolInstall, || {
                    assert!(held_here(&path));
                    let nested = exclusively(&path, JobKind::SecurityScan, || Ok("scanned"))?;
                    entered.send(()).unwrap();
                    hold.recv().unwrap();
                    Ok(nested)
                })
            })
        })
    };
    inside.recv_timeout(Duration::from_secs(10)).expect("the holder ran its nested scan");

    assert!(!held_here(&path), "only the holding thread holds it");
    let ran = std::cell::Cell::new(false);
    let refused = in_surface(Surface::Cli, || {
        exclusively(&path, JobKind::SecurityScan, || {
            ran.set(true);
            Ok(())
        })
    })
    .expect_err("another thread is refused");
    assert!(!ran.get(), "the refused work never ran");
    let busy = refused.downcast_ref::<Busy>().expect("the refusal is the lock's Busy");
    assert_eq!(busy.holder.as_ref().map(|h| (h.kind, h.surface.as_deref())), Some((JobKind::ToolInstall, Some("mcp"))));
    assert_eq!(crate::error::exit_code(&refused), 3);

    release.send(()).unwrap();
    assert_eq!(holding.join().unwrap().unwrap(), "scanned");
    assert!(!held_here(&path), "the mark ends with the hold");
    exclusively(&path, JobKind::SecurityScan, || Ok(())).expect("released once the holder returned");
}

/// The lock is the kernel's, so it excludes another process too, and is free
/// the moment that process dies, however it dies. A holder that wrote nothing
/// is still refused, with a message that says it cannot name it.
#[test]
fn another_process_holding_the_lock_refuses_until_it_dies() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    fs::write(&path, "").unwrap();
    let mut other = Command::new("perl")
        .args(["-e", "use Fcntl ':flock'; open(my $f, '+<', $ARGV[0]) or die; flock($f, LOCK_EX) or die; $| = 1; print \"held\\n\"; sleep 60;"])
        .arg(&path)
        .stdout(Stdio::piped())
        .spawn()
        .expect("perl runs");
    let mut line = String::new();
    BufReader::new(other.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "held\n");

    let busy = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None))
        .unwrap()
        .expect_err("another process holds it");
    assert_eq!(busy.holder, None);
    assert_eq!(
        busy.to_string(),
        "another scan or install is already running on this project; wait for it to finish, then try again"
    );

    other.kill().unwrap();
    other.wait().unwrap();
    ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None))
        .unwrap()
        .expect("the dead process's lock was released by the kernel");
}

#[test]
fn a_symlink_in_place_of_the_lock_file_is_removed_and_never_written_through() {
    let tmp = TempDir::new().unwrap();
    let target = tmp.path().join("elsewhere");
    fs::write(&target, "kept").unwrap();
    let path = tmp.path().join("run.lock");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let _held = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None)).unwrap().unwrap();
    assert!(!fs::symlink_metadata(&path).unwrap().file_type().is_symlink(), "the link was replaced by a file");
    assert_eq!(fs::read_to_string(&target).unwrap(), "kept", "nothing was written through the link");
}

/// A child forked while the lock is held shares its open file until it execs
/// — another thread spawning a process does exactly that. Dropping the lock
/// releases it all the same; it does not wait for that child.
#[test]
#[allow(unsafe_code)]
fn a_dropped_lock_is_free_while_a_child_forked_before_still_shares_it() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let held = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None)).unwrap().unwrap();
    // SAFETY: the child calls only async-signal-safe functions (`sleep`,
    // `_exit`) and never returns into Rust code.
    let child = unsafe { libc::fork() };
    assert!(child >= 0, "fork failed");
    if child == 0 {
        unsafe {
            libc::sleep(30);
            libc::_exit(0);
        }
    }
    drop(held);
    let again = ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None)).unwrap();
    // SAFETY: `child` is this process's own child, killed and reaped once.
    unsafe {
        libc::kill(child, libc::SIGKILL);
        libc::waitpid(child, std::ptr::null_mut(), 0);
    }
    again.expect("the lock is free though the forked child still shares its file");
}

/// A hold that names no one and ends within moments — a peek, or a holder
/// between its lock and its write — never refuses a start: the start waits
/// it out. A peek that finds the lock free holds it only for that moment.
#[test]
fn a_brief_unnamed_hold_is_waited_out_and_a_peek_refuses_no_start() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let brief = open_lock_file(&path).unwrap();
    brief.try_lock().unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        brief.unlock().unwrap();
    });
    ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("mcp"), None))
        .unwrap()
        .expect("a start waits out a hold that names no one");
    releaser.join().unwrap();

    let (stop, stopped) = mpsc::channel::<()>();
    let peeking = {
        let path = path.clone();
        std::thread::spawn(move || {
            while stopped.try_recv().is_err() {
                peek(&path).unwrap();
            }
        })
    };
    let refused = (0..2000)
        .filter(|_| ProjectLock::take(&path, &holder(JobKind::SecurityScan, Some("cli"), None)).unwrap().is_err())
        .count();
    stop.send(()).unwrap();
    peeking.join().unwrap();
    assert_eq!(refused, 0, "a peek refused a start while nothing ran");
}

/// A scan that panics while it holds the lock leaves its thread unmarked and
/// the lock free: a pooled thread (tokio's blocking pool keeps a thread whose
/// task panicked) must not run the next scan as though it held the lock.
#[test]
fn a_panic_inside_a_hold_leaves_the_thread_unmarked_and_the_lock_free() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("run.lock");
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        exclusively(&path, JobKind::SecurityScan, || -> anyhow::Result<()> { panic!("the scan panicked") })
    }));
    assert!(panicked.is_err());
    assert!(!held_here(&path), "a panicked hold left this thread marked as holding the lock");
    assert_eq!(peek(&path).unwrap(), None, "and the lock was released");
}
