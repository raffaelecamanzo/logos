//! The `logos` binary's share of S-513
//! ([FR-EH-05](../../docs/specs/requirements/FR-EH-05.md),
//! [FR-CL-03](../../docs/specs/requirements/FR-CL-03.md)): a file whose facts
//! cannot be persisted fails alone, and only a run that persisted nothing it
//! reached exits 1.
//!
//! A file's persistence is failed on purpose through the debug-only fault seam
//! the runtime reads from `LOGOS_TEST_FAIL_PERSIST` at open — the binary under
//! test is the debug build, so the seam is compiled in. A release `cargo test`
//! skips this file rather than silently testing nothing.
#![cfg(debug_assertions)]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const FAULT_ENV: &str = "LOGOS_TEST_FAIL_PERSIST";

/// Run `logos --json <args>` against `project`, failing the persistence of the
/// files `fault` names (`None`: no fault).
fn logos(project: &Path, fault: Option<&str>, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_logos"));
    cmd.arg("--project").arg(project).arg("--json").args(args);
    match fault {
        Some(paths) => cmd.env(FAULT_ENV, paths),
        None => cmd.env_remove(FAULT_ENV),
    };
    cmd.output().expect("the logos binary runs")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("no signal termination")
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is JSON ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Three independent Rust files.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/a.rs", "pub fn alpha() {}\n");
    write(tmp.path(), "src/b.rs", "pub fn beta() {}\n");
    write(tmp.path(), "src/c.rs", "pub fn gamma() {}\n");
    tmp
}

fn warnings(v: &Value) -> Vec<String> {
    v["warnings"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[test]
fn one_failing_file_exits_zero_and_status_and_scan_report_it() {
    let tmp = fixture();
    let out = logos(tmp.path(), Some("src/b.rs"), &["index"]);
    assert_eq!(exit_code(&out), 0, "a degraded index exits 0: {}", String::from_utf8_lossy(&out.stderr));
    let index = json(&out);
    assert_eq!(index["files_indexed"], 2, "{index}");
    assert_eq!(index["files_failed"], serde_json::json!(["src/b.rs"]));
    assert_eq!(index["persist_failures"][0]["path"], "src/b.rs");
    assert!(
        index["persist_failures"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("injected persistence fault")),
        "{index}"
    );
    assert!(index.get("failed").is_none(), "a degraded run is not failed: {index}");
    assert!(
        warnings(&index).iter().any(|w| w.starts_with("src/b.rs: could not be persisted")),
        "{index}"
    );

    // A later process reads the durable record — no fault needed to see it.
    let status = json(&logos(tmp.path(), None, &["status"]));
    assert_eq!(status["persistence"]["failed_to_persist"], 1, "{status}");
    assert_eq!(status["persistence"]["stale_files"], serde_json::json!([]));

    // `scan` reconciles first; with the fault still in force the file fails again.
    let out = logos(tmp.path(), Some("src/b.rs"), &["scan"]);
    assert_eq!(exit_code(&out), 0, "scan is the report tier");
    let scan = json(&out);
    assert_eq!(scan["persistence"]["failed_to_persist"], 1, "{scan}");

    // Without the fault the next reconcile persists it and the readout is gone.
    let scan = json(&logos(tmp.path(), None, &["scan"]));
    assert!(scan.get("persistence").is_none(), "healed: {scan}");
    let status = json(&logos(tmp.path(), None, &["status"]));
    assert!(status.get("persistence").is_none(), "healed: {status}");
}

#[test]
fn a_run_that_persisted_nothing_it_reached_exits_one_on_index_and_sync() {
    let tmp = fixture();
    let out = logos(tmp.path(), Some("*"), &["index"]);
    assert_eq!(exit_code(&out), 1, "nothing persisted: exit 1");
    let index = json(&out);
    assert_eq!(index["failed"], true, "{index}");
    assert!(
        warnings(&index).iter().any(|w| w.starts_with("nothing was persisted: all 3 file(s)")),
        "the run says nothing was persisted: {index}"
    );

    // A clean index, then a sync whose one file fails.
    assert_eq!(exit_code(&logos(tmp.path(), None, &["index"])), 0);
    write(tmp.path(), "src/c.rs", "pub fn gamma_edited() {}\n");
    let out = logos(tmp.path(), Some("src/c.rs"), &["sync", "src/c.rs"]);
    assert_eq!(exit_code(&out), 1, "{}", String::from_utf8_lossy(&out.stdout));
    let sync = json(&out);
    assert_eq!(sync["failed"], true);
    assert!(warnings(&sync).iter().any(|w| w.starts_with("nothing was persisted")));
    let status = json(&logos(tmp.path(), None, &["status"]));
    assert_eq!(status["persistence"]["stale_files"], serde_json::json!(["src/c.rs"]), "{status}");

    // The same sync without the fault persists, clears the mark, and exits 0.
    let out = logos(tmp.path(), None, &["sync", "src/c.rs"]);
    assert_eq!(exit_code(&out), 0);
    assert!(json(&logos(tmp.path(), None, &["status"])).get("persistence").is_none());
}

#[test]
fn an_index_admitting_no_file_still_exits_zero_under_a_total_fault() {
    // FR-IX-13: nothing reached persistence, so nothing failed to persist.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "notes.txt", "no source\n");
    let out = logos(tmp.path(), Some("*"), &["index"]);
    assert_eq!(exit_code(&out), 0);
    let index = json(&out);
    assert_eq!(index["files_indexed"], 0);
    assert!(index.get("failed").is_none(), "{index}");
}
