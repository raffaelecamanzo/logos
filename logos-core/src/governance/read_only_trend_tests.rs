//! The read-only verdict's trend ([S-785], [CR-243]) over a real store: it
//! reads the snapshots up to the one the verdict displays, by id, so a `scan`
//! committing after that read never enters it — in either direction. Store-
//! backed, so kept apart from the pure unit tests in `tests.rs`.
//!
//! [S-785]: ../../../docs/planning/journal.md#s-785-the-health-page-shows-the-trend-warning-beside-the-gate-verdict
//! [CR-243]: ../../../docs/requests/CR-243-governance-signals-reach-every-surface.md

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `name` with `n` sequential `if`s: cyclomatic complexity `n + 1`.
fn branches(name: &str, n: usize) -> String {
    let body: String = (0..n).map(|i| format!("    if x == {i} {{\n        return {i};\n    }}\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    x\n}}\n")
}

/// Scan after writing 200 functions of complexity 3 and 4 plus `extra`
/// above-mean ones of complexity 6, 7, …: each one lowers Equality.
fn scan_with(engine: &Engine, root: &Path, extra: usize) {
    let many: String = (0..200).map(|i| branches(&format!("f{i}"), 2 + i % 2)).collect();
    let added: String = (0..extra).map(|i| branches(&format!("g_above_mean_{i}"), 5 + i)).collect();
    write(root, "src/many.rs", &format!("{many}{added}"));
    engine.scan(true).expect("scan runs");
}

/// The dimensions of `result`'s trend.
fn trending(result: &GateResult) -> Vec<&'static str> {
    result.trend.iter().map(|t| t.dimension).collect()
}

/// A snapshot recorded after the displayed one never enters its trend: the
/// second decline does not make the earlier verdict warn, and a rise does
/// not clear the warning of the verdict that recorded the second decline.
#[test]
fn a_snapshot_recorded_after_the_displayed_one_never_enters_its_trend() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(root, "src/lib.rs", "pub mod many;\n");
    write(root, ".logos/rules.toml", "[review]\n");
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    scan_with(&engine, root, 0);
    scan_with(&engine, root, 1);
    let one_decline = latest_metrics(&engine).unwrap().expect("a snapshot");

    scan_with(&engine, root, 2);
    assert_eq!(trending(&latest_gate(&engine).unwrap()), ["equality"], "two declines warn");
    let earlier = gate_from_snapshot(&engine, Some(&one_decline)).unwrap();
    assert!(earlier.trend.is_empty() && earlier.warnings.is_empty(), "{earlier:#?}");
    let two_declines = latest_metrics(&engine).unwrap().expect("a snapshot");

    scan_with(&engine, root, 0);
    assert!(latest_gate(&engine).unwrap().trend.is_empty(), "the rise ends the run");
    let displayed = gate_from_snapshot(&engine, Some(&two_declines)).unwrap();
    assert_eq!(trending(&displayed), ["equality"], "{displayed:#?}");
    assert_eq!(displayed.trend[0].values.last(), Some(&two_declines.metrics.equality.normalized));
}
