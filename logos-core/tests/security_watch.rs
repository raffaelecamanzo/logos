//! The watcher auto-ingests a SARIF report dropped into `.logos/security/`, and
//! starts no scanner doing it ([FR-SC-06], S-667).
//!
//! A real OS watcher over a git fixture outside every work tree. The engine's
//! scanner search path holds a stub `semgrep` that records every start, so
//! "no scanner was spawned" is an observed fact, not an inference from source:
//! the structural half of the guarantee is `watch::tests::
//! the_watch_module_spawns_no_subprocess`.
//!
//! [FR-SC-06]: ../../docs/specs/requirements/FR-SC-06.md

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use logos_core::config::record_scan_consent;
use logos_core::security::{FindingsFilter, ScanEnvironment};
use logos_core::Engine;
use tempfile::TempDir;

const DEADLINE: Duration = Duration::from_secs(20);

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0", "-c", "user.email=t@logos", "-c", "user.name=t"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}");
}

#[test]
fn a_sarif_report_dropped_in_the_security_directory_is_ingested_and_no_scanner_starts() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let (root, bin) = (base.join("project"), base.join("bin"));
    fs::create_dir_all(root.join("app")).unwrap();
    fs::create_dir_all(root.join(".logos")).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(root.join("app/db.py"), "import sqlite3\n").unwrap();
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-watch-fixture\"\n\n[watcher]\ndebounce_ms = 200\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "seed"]);

    // A scanner that records any start: the watcher must never cause one.
    let record = base.join("semgrep.started");
    let stub = bin.join("semgrep");
    fs::write(&stub, format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n", record.display())).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    // Consent is recorded, so a path from the watcher to the scan surface would
    // really start the stub rather than stop at the consent check.
    let user = base.join("user");
    fs::create_dir_all(&user).unwrap();
    record_scan_consent(Some(&user), &root, &["semgrep"]).expect("consent is recorded");
    let env = ScanEnvironment {
        user_dir: Some(user),
        search_path: Some(bin.into_os_string()),
    };
    let engine = Arc::new(Engine::start(&root).expect("engine starts").with_scan_environment(env));
    engine.index();
    let handle = engine.watch().expect("the watcher starts");

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif/semgrep.sarif");
    fs::create_dir_all(root.join(".logos/security")).unwrap();
    fs::copy(&fixture, root.join(".logos/security/ci-semgrep.sarif")).unwrap();

    let started = Instant::now();
    let scanned = loop {
        let listed = engine.security_findings(&FindingsFilter::default()).expect("findings read");
        if !listed.scanned.is_empty() {
            break listed;
        }
        assert!(started.elapsed() < DEADLINE, "the report was not ingested within {DEADLINE:?}: {:?}", handle.stats());
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(scanned.scanned[0].driver, "Semgrep OSS");
    assert_eq!(scanned.total, 2);
    assert_eq!(scanned.findings.iter().filter(|f| f.finding.path.as_deref() == Some("app/db.py")).count(), 2, "bound against the index");
    let stats = handle.stats();
    assert!(stats.security_reports_accepted >= 1 && stats.security_ingests_run >= 1, "{stats:?}");
    assert_eq!(stats.syncs_run, 0, "a report under .logos/ never drives a source sync: {stats:?}");
    drop(handle);
    assert!(!record.exists(), "the watcher started a scanner: {:?}", fs::read_to_string(&record));
}

/// The watcher's entry reads only a regular file: `.logos/security/` can hold
/// what a repository committed, and a symbolic link (here to a real report
/// outside the project) or a FIFO is refused at once — a FIFO read would
/// otherwise block the one sync worker until something writes to it.
#[test]
fn the_watcher_s_ingest_reads_only_a_regular_file() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let root = base.join("project");
    fs::create_dir_all(root.join("app")).unwrap();
    fs::create_dir_all(root.join(".logos/security")).unwrap();
    fs::write(root.join("app/db.py"), "import sqlite3\n").unwrap();
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"security-watch-fixture\"\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", "app", ".logos/config.toml"]);
    git(&root, &["commit", "-q", "-m", "seed"]);
    let engine = Arc::new(Engine::start(&root).expect("engine starts"));
    engine.index();

    let outside = base.join("semgrep.sarif");
    fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarif/semgrep.sarif"), &outside).unwrap();
    let link = root.join(".logos/security/link.sarif");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let err = engine.security_ingest_auto(&link).expect_err("a symbolic link is refused");
    assert!(format!("{err:#}").contains("not a regular file"), "{err:#}");

    let fifo = root.join(".logos/security/fifo.sarif");
    let made = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo runs");
    assert!(made.success());
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = Arc::clone(&engine);
    let path = fifo.clone();
    std::thread::spawn(move || {
        let _ = tx.send(worker.security_ingest_auto(&path).is_err());
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true), "a FIFO is refused, never read");

    let listed = engine.security_findings(&FindingsFilter::default()).expect("findings read");
    assert!(listed.scanned.is_empty(), "nothing was ingested: {:?}", listed.scanned);
}
