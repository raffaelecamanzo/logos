//! Integration tests for the governance engine (S-020), exercised end-to-end
//! through the [`Engine`] façade against real temp-directory fixtures: real
//! extraction + resolution + annotation, real reconcile, real persistence.
//!
//! Coverage by acceptance criterion:
//! - `check_rules` enforces a layered contract and `passed` discriminates
//!   the exit-1 path; re-running yields identical violations (FR-GV-02/03,
//!   UAT-GV-01);
//! - the gate saves a baseline, tolerates a no-op re-run within epsilon, and
//!   fails on a deliberate regression naming the moved metric(s)
//!   (FR-GV-04/05, UAT-GV-03/04);
//! - every aggregate run reconciles first: an out-of-band edit is reflected
//!   by the next `scan`, with an accurate freshness count (FR-RC-01/02/03,
//!   NFR-RA-02, UAT-RC-01, UAT-SY-03);
//! - `--no-reconcile` skips reconciliation and says so (FR-RC-04,
//!   UAT-RC-03);
//! - a never-indexed tree degrades to a full index (FR-RC-02, UAT-RC-02);
//! - a partial reconcile failure stamps `INCOMPLETE`, never a silently-stale
//!   number (NFR-RA-11, ADR-11);
//! - every scan/gate appends a `metric_snapshots` row (FR-GV-09) and
//!   `evolution` windows them with deltas (FR-GV-06, UAT-GV-05);
//! - `dsm` returns a square layer-ordered matrix and `test_gaps` lists the
//!   untested function with the mandatory caveat (FR-GV-07/08, UAT-GV-05);
//! - an unchanged `rules.toml` hits the persisted rules cache (FR-GV-01).

#![cfg(feature = "lang-rust")]

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use logos_core::model::NodeKind;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The persisted snapshot count, via the public read seam.
fn snapshot_count(rt: &Runtime) -> usize {
    rt.submit_read(|store| store.metric_snapshots())
        .expect("read runs")
        .len()
}

/// A layered fixture with an upward (domain → presentation) call that also
/// crosses the declared boundary — the UAT-GV-01 shape.
fn layered_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[layers]]
name  = \"domain\"
paths = [\"src/domain_*.rs\"]
order = 1

[[layers]]
name  = \"presentation\"
paths = [\"src/ui_*.rs\"]
order = 2

[[boundaries]]
from   = \"domain\"
to     = \"presentation\"
reason = \"the domain must not reach upward into presentation\"
",
    );
    write(
        tmp.path(),
        "src/domain_core.rs",
        "\
use crate::ui_view::render;

pub fn compute() {
    render();
}
",
    );
    write(tmp.path(), "src/ui_view.rs", "pub fn render() {}\n");
    tmp
}

/// A clean two-function fixture with no rules contract.
fn clean_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
",
    );
    tmp
}

// ── FR-GV-02/03 / UAT-GV-01: check_rules enforces the contract ──────────────

#[test]
fn check_rules_flags_the_layered_violation_and_rerun_is_identical() {
    let tmp = layered_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "an error violation must fail the check (FR-GV-03 exit-1 path)"
    );
    assert!(report.checked_rules >= 2, "ordering + boundary are active");
    assert!(report.rules_present, "a loaded rules.toml reports as present (FR-GV-17 honesty)");
    let rules_hit: Vec<&str> = report.violations.iter().map(|v| v.rule.as_str()).collect();
    assert!(
        rules_hit.contains(&"layer-ordering"),
        "the upward dependency is reported (BR-11): {rules_hit:?}"
    );
    assert!(
        rules_hit.contains(&"boundary:domain->presentation"),
        "the boundary crossing is reported: {rules_hit:?}"
    );
    assert!(
        report.violations.iter().all(|v| v.severity == "error"),
        "checked-in policy is binding"
    );

    // FR-GV-02 idempotence: a re-run yields identical violations and the
    // persisted set matches (no duplicate derived artifacts accumulate).
    let again = engine.check_rules(None, true).expect("re-run");
    assert_eq!(again.violations, report.violations);
    let persisted = engine
        .runtime()
        .unwrap()
        .submit_read(|store| store.violations())
        .expect("read persisted violations");
    assert_eq!(persisted.len(), report.violations.len());

    // FR-GV-01: the unchanged contract is cached by hash in the store.
    let cache = engine
        .runtime()
        .unwrap()
        .submit_read(|store| store.rules_cache())
        .expect("read rules cache")
        .expect("rules cache row written on first parse");
    assert!(!cache.rules_hash.is_empty() && cache.parsed_json.contains("domain"));
}

#[test]
fn a_clean_project_passes_check_rules() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(report.passed, "no contract → nothing violated");
    assert_eq!(
        report.checked_rules, 0,
        "no active rules without rules.toml"
    );
    assert!(
        !report.rules_present,
        "an absent .logos/rules.toml reports as not-present — the honest signal the \
         web Gaps view (S-149) reads to show its onboarding empty state (NFR-CC-04)"
    );
}

// ── CR-052 / FR-GV-18 / NFR-RA-13: the structural-integrity guard ────────────

/// Force graph structural drift the FK cascade would normally forbid: an orphan
/// shingle (its `node_id` references a node that does not exist), inserted via a
/// raw connection with foreign keys disabled. A shingle is not part of the
/// metric graph, so the aggregate signal is left unchanged — exactly the
/// "corrupt graph, unmoved signal" case the gate must still hard-fail
/// ([FR-GV-05], [NFR-RA-13]).
fn inject_orphan_shingle(root: &Path) {
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open(&db).expect("open the store directly");
    conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute("INSERT INTO shingles (node_id, hash) VALUES (999999, 42)", [])
        .expect("orphan shingle inserts once foreign keys are disabled");
}

#[test]
fn doctor_reports_ok_on_a_healthy_graph_and_drift_on_a_forced_orphan() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    // Populate the graph (a clean reconcile — no drift, so the debug-build
    // structural assertion after sync holds).
    engine.check_rules(None, true).expect("populate the graph");

    let healthy = engine.doctor().expect("doctor runs");
    assert!(healthy.ok, "a freshly-indexed graph is structurally sound");
    assert_eq!(
        healthy.node_count, healthy.distinct_symbol_ids,
        "one node per symbol_id on a healthy graph (NFR-RA-13)"
    );
    assert!(healthy.faults.is_empty());

    inject_orphan_shingle(tmp.path());

    let drifted = engine.doctor().expect("doctor runs");
    assert!(!drifted.ok, "doctor detects the forced orphan (FR-GV-18)");
    assert_eq!(drifted.orphan_shingles, 1);
    assert!(
        drifted.faults.iter().any(|f| f.contains("orphan shingle")),
        "the fault is named: {:?}",
        drifted.faults
    );
}

#[test]
fn structural_drift_hard_fails_the_gate_and_check_rules_even_when_the_signal_holds() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Baseline + populate on a clean graph (reconcile ok — no drift yet).
    engine.session_start().expect("session_start saves the baseline");

    // Clean: every gate path passes and doctor is ok. `reconcile=false` reads
    // the just-reconciled graph without re-running sync.
    let clean_gate = engine.gate(None, false, false).expect("gate runs");
    assert!(clean_gate.passed, "clean graph passes: {}", clean_gate.message);
    assert!(clean_gate.structural_faults.is_empty());
    let baseline_signal = clean_gate.signal;
    assert!(
        engine.check_rules(None, false).expect("check_rules").passed,
        "clean graph passes check_rules"
    );
    assert!(engine.doctor().expect("doctor").ok);

    // Force drift that does NOT move the metric signal.
    inject_orphan_shingle(tmp.path());

    // The gate hard-fails INDEPENDENT of the (unchanged) signal (FR-GV-05).
    // `reconcile=false`: a reconcile re-runs sync, whose debug-build structural
    // assertion (NFR-RA-13) would panic on the drift before the gate could
    // report it — the structural fold-in itself runs regardless of reconcile,
    // so this exercises the same code session_end reaches in a release build.
    let gate = engine.gate(None, false, false).expect("gate runs");
    assert!(!gate.passed, "structural drift hard-fails the gate: {}", gate.message);
    assert_eq!(
        gate.signal, baseline_signal,
        "the metric signal is unchanged — the gate fails on structure alone"
    );
    assert!(
        !gate.structural_faults.is_empty() && gate.message.contains("structural drift"),
        "the gate names the structural fault: {} / {:?}",
        gate.message,
        gate.structural_faults
    );

    // check_rules exits 1 with an error-severity structural finding (FR-GV-02).
    let check = engine.check_rules(None, false).expect("check_rules runs");
    assert!(!check.passed, "structural drift fails check_rules");
    assert!(
        check.violations.iter().any(|v| {
            v.rule == "graph-structural-integrity" && v.severity == "error"
        }),
        "an error-severity structural violation is reported: {:?}",
        check.violations
    );

    // health folds the verdict in too (CR-052).
    let health = engine.health(false).expect("health runs");
    assert!(!health.ok && !health.structural_ok, "health surfaces the drift");
    assert!(!health.structural_faults.is_empty());
}

// ── S-215 / FR-GV-20 / ADR-48: the always-on admission tripwire ─────────────

/// Force admission drift: a `files` row whose path is under the
/// default-ignored `.worktrees` scratch dir ([S-213]) — a path the *current*
/// `AdmissionAuthority` would never admit if it were freshly discovered. No
/// node references the row, so the metric signal is left unchanged — the
/// same "corrupt store, unmoved signal" shape [`inject_orphan_shingle`] proves
/// for the structural dimension.
///
/// [S-213]: ../../docs/planning/journal.md#s-213-add-worktrees-and-playwright-mcp-to-default-ignored_dirs
fn inject_unadmitted_file(root: &Path) -> &'static str {
    const PATH: &str = ".worktrees/sprint-99/src/leaked.rs";
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open(&db).expect("open the store directly");
    conn.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
    conn.execute("INSERT INTO files (path) VALUES (?1)", [PATH])
        .expect("unadmitted file row inserts");
    PATH
}

#[test]
fn doctor_reports_ok_on_a_healthy_graph_and_admission_drift_on_an_injected_unadmitted_file() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("populate the graph");

    let healthy = engine.doctor().expect("doctor runs");
    assert!(healthy.ok, "a freshly-indexed graph has no admission drift");
    assert_eq!(healthy.unadmitted_files, 0);
    assert!(healthy.unadmitted_sample.is_empty());

    let path = inject_unadmitted_file(tmp.path());

    let drifted = engine.doctor().expect("doctor runs");
    assert!(!drifted.ok, "doctor detects the admission drift (FR-GV-20)");
    assert_eq!(drifted.unadmitted_files, 1);
    assert_eq!(drifted.unadmitted_sample, vec![path.to_string()]);
    assert!(
        drifted.faults.iter().any(|f| f.contains(path) && f.contains("AdmissionAuthority")),
        "the fault names the offending file: {:?}",
        drifted.faults
    );
    // Untouched by the admission fold-in — the structural dimension is
    // independently still sound.
    assert_eq!(drifted.node_count, drifted.distinct_symbol_ids);
}

#[test]
fn admission_drift_hard_fails_the_gate_check_rules_and_health_with_the_signal_unchanged() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Baseline + populate on a clean graph (no drift yet).
    engine.session_start().expect("session_start saves the baseline");

    let clean_gate = engine.gate(None, false, false).expect("gate runs");
    assert!(clean_gate.passed, "clean graph passes: {}", clean_gate.message);
    assert!(clean_gate.structural_faults.is_empty());
    let baseline_signal = clean_gate.signal;
    assert!(engine.check_rules(None, false).expect("check_rules").passed);
    assert!(engine.doctor().expect("doctor").ok);

    let path = inject_unadmitted_file(tmp.path());

    // The gate hard-fails INDEPENDENT of the (unchanged) signal (FR-GV-20).
    // `reconcile=false`: this test isolates the S-215 tripwire from the S-214
    // sync/watcher consumer work (a separate story) — no reconcile runs here
    // to (re-)purge or re-admit the injected row.
    let gate = engine.gate(None, false, false).expect("gate runs");
    assert!(!gate.passed, "admission drift hard-fails the gate: {}", gate.message);
    assert_eq!(
        gate.signal, baseline_signal,
        "the metric signal is unchanged — the gate fails on admission alone"
    );
    assert!(
        gate.structural_faults.iter().any(|f| f.contains(path)),
        "the gate names the offending file: {:?}",
        gate.structural_faults
    );

    // check_rules exits 1 with a distinct error-severity admission finding.
    let check = engine.check_rules(None, false).expect("check_rules runs");
    assert!(!check.passed, "admission drift fails check_rules");
    assert!(
        check.violations.iter().any(|v| {
            v.rule == "graph-admission-drift" && v.severity == "error" && v.message.contains(path)
        }),
        "an error-severity admission violation is reported: {:?}",
        check.violations
    );

    // health folds the verdict in too.
    let health = engine.health(false).expect("health runs");
    assert!(!health.ok && !health.structural_ok, "health surfaces the admission drift");
    assert!(health.structural_faults.iter().any(|f| f.contains(path)));
}

#[test]
fn doctor_report_json_shape_is_additive_only() {
    // FR-GV-20 acceptance: the two new fields land WITHOUT renaming or
    // removing any pre-existing (CR-052) `DoctorReport` field — the CLI
    // `--json`, MCP, and web surfaces all serialize this struct verbatim.
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.check_rules(None, true).expect("populate the graph");

    let report = engine.doctor().expect("doctor runs");
    let value = serde_json::to_value(&report).expect("DoctorReport serializes");
    let obj = value.as_object().expect("DoctorReport is a JSON object");

    for key in [
        "ok",
        "node_count",
        "distinct_symbol_ids",
        "duplicate_symbol_nodes",
        "dangling_file_refs",
        "dangling_edge_endpoints",
        "orphan_shingles",
        "faults",
        "message",
    ] {
        assert!(
            obj.contains_key(key),
            "pre-existing DoctorReport field `{key}` must survive S-215 additively"
        );
    }
    assert!(obj.contains_key("unadmitted_files"));
    assert!(obj.contains_key("unadmitted_sample"));
}

// ── CR-052 / FR-GV-19 / NFR-RA-06: the deep shadow-reindex verify ───────────

#[test]
fn verify_reports_ok_on_a_clean_freshly_indexed_graph() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    // A full index leaves the live store population-equal to a fresh reindex.
    engine.index();

    let report = engine.verify().expect("verify runs");
    assert!(
        report.ok,
        "a freshly-indexed graph matches a fresh reindex: {}",
        report.message
    );
    assert_eq!(
        (report.node_delta, report.edge_delta, report.file_delta),
        (0, 0, 0),
        "zero count deltas on a clean graph (FR-GV-19)"
    );
    assert!(report.leaked_total == 0 && report.leaked_symbols.is_empty());
    assert!(report.orphaned_total == 0 && report.orphaned_symbols.is_empty());
    assert!(report.structural.ok, "the embedded structural check is ok");
    assert_eq!(
        report.live, report.reindex,
        "the live and reindex censuses match on a clean graph"
    );
}

#[test]
fn verify_detects_an_orphan_file_leak_and_never_mutates_the_live_store() {
    // A two-file project; index it, then delete one file on disk WITHOUT a sync,
    // so the live store retains its nodes — the Channel-B orphan leak FR-GV-19
    // exists to catch. A fresh shadow reindex sees only the surviving file.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/keep.rs", "pub fn keep() {}\n");
    write(
        tmp.path(),
        "src/gone.rs",
        "pub fn gone() {}\npub fn also_gone() {}\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // Live counts before the leak — the read-only-census immutability baseline.
    let before = engine.health(false).expect("health reads live counts");
    fs::remove_file(tmp.path().join("src/gone.rs")).unwrap();

    let report = engine.verify().expect("verify runs");
    assert!(
        !report.ok,
        "the orphan-file leak is drift: {}",
        report.message
    );
    assert!(
        report.node_delta > 0,
        "the live graph has surplus nodes vs a fresh reindex: {}",
        report.node_delta
    );
    assert!(
        report.file_delta >= 1,
        "the live store retains the deleted file: {}",
        report.file_delta
    );
    assert!(
        report.leaked_total >= 1 && !report.leaked_symbols.is_empty(),
        "the deleted file's symbols are leaked (live-only): {:?}",
        report.leaked_symbols
    );
    assert!(
        report.leaked_symbols.iter().any(|s| s.contains("gone")),
        "the leaked sample names the removed file's symbols: {:?}",
        report.leaked_symbols
    );
    assert_eq!(
        report.orphaned_total, 0,
        "a pure leak has no reindex-only (orphaned) symbols"
    );

    // The live store is opened READ-ONLY for the census — `verify` must not purge
    // the leaked nodes or otherwise mutate the graph (FR-GV-19). Counts are read
    // with reconcile=false so `health` itself never syncs the leak away.
    let after = engine.health(false).expect("health reads live counts");
    assert_eq!(before.nodes, after.nodes, "verify left the node count unchanged");
    assert_eq!(before.files, after.files, "verify left the file count unchanged");
    assert_eq!(before.edges, after.edges, "verify left the edge count unchanged");

    // Repeatable: the shadow store was torn down cleanly (no leftover lock or
    // corruption), so a second run yields the same verdict.
    let again = engine.verify().expect("verify runs again");
    assert!(
        !again.ok && again.node_delta == report.node_delta,
        "verify is repeatable after shadow teardown"
    );
}

#[test]
fn verify_detects_a_new_unindexed_file_as_orphaned_reindex_only_symbols() {
    // The mirror of the leak: a file ADDED on disk without a sync is unknown to
    // the live store but seen by a fresh reindex → the live graph UNDER-counts.
    // This pins the diff DIRECTION (leaked = live-only, orphaned = reindex-only
    // must never be swapped) and the `node_delta < 0` branch (FR-GV-19).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/first.rs", "pub fn first() {}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // Add a new file on disk; no sync, so the live store never learns of it.
    write(tmp.path(), "src/added.rs", "pub fn added_symbol() {}\n");

    let report = engine.verify().expect("verify runs");
    assert!(
        !report.ok,
        "an unindexed new file is drift: {}",
        report.message
    );
    assert!(
        report.node_delta < 0,
        "the live graph under-counts vs a fresh reindex: {}",
        report.node_delta
    );
    assert!(
        report.orphaned_total >= 1 && !report.orphaned_symbols.is_empty(),
        "the new file's symbols are orphaned (reindex-only): {:?}",
        report.orphaned_symbols
    );
    assert!(
        report
            .orphaned_symbols
            .iter()
            .any(|s| s.contains("added_symbol")),
        "the orphaned sample names the new file's symbol: {:?}",
        report.orphaned_symbols
    );
    assert_eq!(
        report.leaked_total, 0,
        "nothing leaked — the drift is purely reindex-only"
    );
}

#[test]
fn verify_fails_on_embedded_structural_drift_even_when_counts_match() {
    // A structural fault that leaves counts and the symbol set untouched — an
    // orphan shingle is not a node/edge/file, so a fresh reindex censuses
    // identically. The census diff is therefore clean, yet the EMBEDDED fast
    // structural check (FR-GV-18) must still fail `verify` (FR-GV-19), exercising
    // the `structural.is_ok()` conjunct of `ok` and the structural message branch.
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    inject_orphan_shingle(tmp.path());

    let report = engine.verify().expect("verify runs");
    assert!(
        !report.ok,
        "embedded structural drift fails verify: {}",
        report.message
    );
    assert_eq!(
        (report.node_delta, report.edge_delta, report.file_delta),
        (0, 0, 0),
        "the census diff is clean — verify fails on structure alone"
    );
    assert!(
        report.leaked_total == 0 && report.orphaned_total == 0,
        "no count/symbol drift — only the structural fault"
    );
    assert!(
        !report.structural.ok && report.structural.orphan_shingles == 1,
        "the embedded structural check names the orphan shingle: {:?}",
        report.structural.faults
    );
    assert!(
        report.message.contains("structural"),
        "the verify message names the structural fault: {}",
        report.message
    );
}

// ── FR-GV-04/05 / UAT-GV-03/04: the gate ────────────────────────────────────

#[test]
fn gate_saves_a_baseline_then_fails_on_a_deliberate_regression() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // No baseline yet → informational pass (FR-GV-05).
    let informational = engine.gate(None, false, true).expect("gate runs");
    assert!(informational.passed, "no baseline → informational pass");
    assert!(
        informational.message.contains("no baseline"),
        "{}",
        informational.message
    );

    // Save the baseline (FR-GV-04).
    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.passed && saved.saved);
    assert!(saved.signal.is_some(), "a non-empty fixture has a signal");

    // UAT-GV-04: an unchanged repo re-gates within epsilon — identical
    // signal, no regression, pass.
    let unchanged = engine.gate(None, false, true).expect("gate runs");
    assert!(
        unchanged.passed,
        "identical code must hold the baseline: {}",
        unchanged.message
    );
    assert_eq!(unchanged.baseline_signal, saved.signal);

    // Deliberate regression: introduce a dependency cycle + dead code, which
    // degrades acyclicity (1 cycle) and redundancy.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
fn tangle_a() {
    tangle_b();
}
fn tangle_b() {
    tangle_a();
}
",
    );
    let regressed = engine.gate(None, false, true).expect("gate runs");
    assert!(
        !regressed.passed,
        "a deliberate regression must fail the gate (UAT-GV-03): {}",
        regressed.message
    );
    assert!(
        regressed
            .regressions
            .iter()
            .any(|r| r.metric == "acyclicity"),
        "the report names which metric moved (FR-GV-05): {:?}",
        regressed.regressions
    );
    assert!(
        regressed.signal < regressed.baseline_signal,
        "the aggregate moved down"
    );

    // FR-GV-04 "re-running overwrites it": saving again anchors the gate to
    // the NEW (lower) baseline — the next bare gate passes against it.
    let resaved = engine.gate(None, true, true).expect("gate --save again");
    assert_eq!(
        resaved.signal, regressed.signal,
        "the re-save records the degraded signal"
    );
    let against_new = engine.gate(None, false, true).expect("gate runs");
    assert!(
        against_new.passed,
        "the baseline was overwritten, not duplicated: {}",
        against_new.message
    );
    assert_eq!(
        against_new.baseline_signal, resaved.signal,
        "the comparison anchor is the overwritten baseline"
    );
}

#[test]
fn gate_threshold_floor_fails_a_low_signal() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.gate(Some(10_000), false, true).expect("gate runs");
    assert!(
        !result.passed,
        "no real project scores a perfect 10000: {}",
        result.message
    );
    assert!(result.message.contains("threshold"), "{}", result.message);
}

// ── FR-GV-04/05: the session spelling ───────────────────────────────────────

#[test]
fn session_start_saves_the_baseline_session_end_compares() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let session = engine.session_start().expect("session_start runs");
    assert!(!session.session_id.is_empty());
    assert!(session.started_at > 0);
    assert!(session.signal.is_some());

    // An unchanged session passes its end gate.
    let end = engine.session_end().expect("session_end runs");
    assert!(end.passed, "{}", end.message);
    assert_eq!(end.baseline_signal, session.signal);
}

// ── FR-GV-10 / UAT-GV-06: versioned baseline auto-re-baseline ────────────────

/// A baseline recorded under the previous metrics-semantics version is
/// incomparable to a production-scope run: the first gate after the upgrade
/// auto-re-baselines with a notice and passes informationally — never a
/// regression failure against an incomparable anchor — and the second gate
/// compares normally against the fresh baseline (FR-GV-10, UAT-GV-06).
#[test]
fn first_gate_after_a_semantics_change_auto_re_baselines_then_compares() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    // Save a baseline, then rewrite the snapshot it points at to the OLD (v1)
    // metrics-semantics version — exactly the on-disk state an upgraded binary
    // inherits from a pre-CR-001 `.logos` database (the migration-7 DEFAULT 1).
    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.saved && saved.passed);
    downgrade_all_snapshots_to_v1(tmp.path());

    // First post-upgrade gate: auto-re-baseline, informational pass, the notice.
    let first = engine.gate(None, false, true).expect("gate runs");
    assert!(
        first.passed,
        "an incomparable baseline must never fail the gate (FR-GV-10): {}",
        first.message
    );
    assert!(
        first.saved,
        "the first gate re-baselines against the fresh snapshot"
    );
    assert!(
        first
            .message
            .contains("baseline reset: metric semantics changed"),
        "the re-baseline is announced (UAT-GV-06): {}",
        first.message
    );

    // Second gate: the fresh baseline is the current version, so it compares
    // normally — no notice, a real signal-vs-baseline message.
    let second = engine.gate(None, false, true).expect("gate runs");
    assert!(second.passed, "{}", second.message);
    assert!(
        !second.saved,
        "the second gate compares, it does not re-baseline again"
    );
    assert!(
        !second.message.contains("baseline reset"),
        "no second reset — the versions now match: {}",
        second.message
    );
    assert!(
        second.message.contains("holds the baseline"),
        "the second gate is a normal comparison: {}",
        second.message
    );
    assert_eq!(
        second.baseline_signal, first.signal,
        "the second gate compares against the snapshot the first gate re-based to"
    );
}

/// Rewrite every persisted `metric_snapshots` row to the v1 (test-inclusive)
/// semantics version through a side connection to the same `.logos/logos.db` —
/// simulating a baseline inherited from a pre-upgrade database without a public
/// downgrade seam.
fn downgrade_all_snapshots_to_v1(root: &Path) {
    let db = root.join(".logos/logos.db");
    let conn = rusqlite::Connection::open(&db).expect("open the store directly");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn.execute("UPDATE metric_snapshots SET metric_version = 1", [])
        .expect("downgrade the recorded semantics version");
}

// ── FR-GV-10 / BR-25 / UAT-QM-13 step 3: thresholds-hash auto-re-baseline ────

/// A clean fixture that carries a `rules.toml` (so it travels with a hashable
/// content) and two functions — enough for a real, non-empty signal.
fn thresholds_project(rules_toml: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), ".logos/rules.toml", rules_toml);
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
",
    );
    tmp
}

/// CR-005 / BR-25: editing a metric threshold in `rules.toml` changes the
/// effective-thresholds hash, so the next `gate` finds its baseline incomparable
/// and takes the SAME announced re-baseline path a semantics-version change does
/// — an informational pass with the notice — and the gate after that compares
/// normally against the fresh baseline (UAT-QM-13 step 3). Threshold tuning is
/// always possible and never silently moves a gated comparison.
#[test]
fn editing_a_threshold_re_baselines_once_then_compares() {
    // Start with the documented defaults (an empty table is all-defaults).
    let tmp = thresholds_project("[metric_thresholds]\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    // Save a v3 baseline under the default thresholds.
    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.saved && saved.passed);

    // A bare gate with the SAME thresholds compares normally — no reset.
    let unchanged = engine.gate(None, false, true).expect("gate runs");
    assert!(
        unchanged.passed && !unchanged.saved,
        "{}",
        unchanged.message
    );
    assert!(
        !unchanged.message.contains("baseline reset"),
        "an unchanged threshold set must not re-baseline: {}",
        unchanged.message
    );

    // Edit one threshold (T_nest 4 → 5, the UAT-QM-13 test datum). The file
    // content hash changes, so the rules cache reparses and the snapshot scores
    // under — and persists — a different effective-thresholds hash.
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[metric_thresholds]\nnesting_depth = 5\n",
    );

    // First gate after the edit: auto-re-baseline, informational pass, the
    // CR-005 notice — never a regression failure against the incomparable anchor.
    let first = engine.gate(None, false, true).expect("gate runs");
    assert!(
        first.passed,
        "an incomparable (re-tuned) baseline must never fail the gate: {}",
        first.message
    );
    assert!(
        first.saved,
        "the first gate re-baselines against the fresh snapshot"
    );
    assert!(
        first
            .message
            .contains("baseline reset: metric thresholds changed"),
        "the threshold re-baseline is announced (BR-25): {}",
        first.message
    );

    // Second gate: the fresh baseline carries the new hash, so it compares
    // normally — no second reset.
    let second = engine.gate(None, false, true).expect("gate runs");
    assert!(second.passed, "{}", second.message);
    assert!(
        !second.saved,
        "the second gate compares, it does not re-baseline again"
    );
    assert!(
        !second.message.contains("baseline reset"),
        "no second reset — the hashes now match: {}",
        second.message
    );
    assert_eq!(
        second.baseline_signal, first.signal,
        "the second gate compares against the snapshot the first gate re-based to"
    );
}

/// CR-013 / FR-GV-10 / BR-25: a near-clone parameter is now a hashed effective
/// threshold, so editing `clone_similarity` re-baselines the gate exactly like a
/// structural threshold — the first gate after the edit is an informational pass
/// with the announced notice, the next compares normally. With the defaults the
/// gate compares without any reset (the default-set hash is byte-identical to the
/// pre-CR-013 build, so no spurious re-baseline).
#[test]
fn editing_a_clone_threshold_re_baselines_once_then_compares() {
    // Start with the documented defaults (an empty table is all-defaults).
    let tmp = thresholds_project("[metric_thresholds]\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    // Save a baseline, then confirm a bare gate under the SAME (default) clone
    // params compares normally — defaults never re-baseline.
    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.saved && saved.passed);
    let unchanged = engine.gate(None, false, true).expect("gate runs");
    assert!(
        unchanged.passed && !unchanged.saved && !unchanged.message.contains("baseline reset"),
        "the default clone params must not re-baseline: {}",
        unchanged.message
    );

    // Edit `clone_similarity` away from its 0.85 default. The effective-thresholds
    // hash moves, so the gate finds its baseline incomparable.
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[metric_thresholds]\nclone_similarity = 0.9\n",
    );

    // First gate after the edit: announced auto-re-baseline, informational pass.
    let first = engine.gate(None, false, true).expect("gate runs");
    assert!(
        first.passed && first.saved,
        "an incomparable (re-tuned) clone threshold re-baselines, never fails: {}",
        first.message
    );
    assert!(
        first
            .message
            .contains("baseline reset: metric thresholds changed"),
        "the clone-threshold re-baseline is announced (BR-25): {}",
        first.message
    );

    // Second gate: the fresh baseline carries the new hash, so it compares.
    let second = engine.gate(None, false, true).expect("gate runs");
    assert!(
        second.passed && !second.saved && !second.message.contains("baseline reset"),
        "no second reset — the hashes now match: {}",
        second.message
    );
}

/// CR-013 / FR-GV-10: the SAME re-baseline path holds for the *other* near-clone
/// key — editing `clone_min_tokens` through the engine moves the effective hash,
/// triggers the announced reset once, and the next gate compares normally. The
/// acceptance criterion is "editing **either** key"; this pins the engine-level
/// behaviour for `clone_min_tokens` (the `clone_similarity` twin is above).
#[test]
fn editing_clone_min_tokens_re_baselines_once_then_compares() {
    let tmp = thresholds_project("[metric_thresholds]\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.saved && saved.passed);

    // Edit `clone_min_tokens` away from its default of 50.
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[metric_thresholds]\nclone_min_tokens = 80\n",
    );

    let first = engine.gate(None, false, true).expect("gate runs");
    assert!(
        first.passed && first.saved,
        "a re-tuned clone_min_tokens re-baselines, never fails: {}",
        first.message
    );
    assert!(
        first
            .message
            .contains("baseline reset: metric thresholds changed"),
        "the clone_min_tokens re-baseline is announced (BR-25): {}",
        first.message
    );

    let second = engine.gate(None, false, true).expect("gate runs");
    assert!(
        second.passed && !second.saved && !second.message.contains("baseline reset"),
        "no second reset — the hashes now match: {}",
        second.message
    );
}

// ── CR-005 §3.2 / FR-QM-09..13: worst-offender detail in scan output ─────────

/// `scan` carries the per-dimension worst-offender lists, deterministically:
/// two scans of the same tree yield byte-identical serialized offenders
/// ([NFR-RA-06]). A clean fixture has no offenders, so the lists are empty — the
/// honest, stable shape the review surface exposes.
#[test]
fn scan_emits_deterministic_worst_offenders() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let first = engine.scan(true).expect("scan runs");
    let second = engine.scan(true).expect("scan runs");

    let to_json =
        |s: &logos_core::models::quality::WorstOffenders| serde_json::to_string(s).unwrap();
    assert_eq!(
        to_json(&first.worst_offenders),
        to_json(&second.worst_offenders),
        "worst-offender lists are byte-identical across runs (NFR-RA-06)"
    );
    // A clean two-function fixture trips no structural dimension.
    let w = &first.worst_offenders;
    assert!(
        w.nesting.is_empty()
            && w.conciseness.is_empty()
            && w.cohesion.is_empty()
            && w.focus.is_empty()
            && w.uniqueness.is_empty(),
        "a clean fixture has no offenders"
    );
}

/// A fixture with one production function nested five control-flow levels deep —
/// the structural-budget / worst-offender source.
fn deeply_nested_project(rules_toml: Option<&str>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    if let Some(rules) = rules_toml {
        write(tmp.path(), ".logos/rules.toml", rules);
    }
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn deeply_nested(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            if x > 2 {
                if x > 3 {
                    if x > 4 {
                        return x;
                    }
                }
            }
        }
    }
    0
}
",
    );
    tmp
}

/// CR-005 / [UAT-GV-08]: the structural budgets fail `check` end-to-end when
/// exceeded and pass when relaxed. A depth-5 function violates
/// `max_nesting_depth = 4` (an error violation that drives the CLI exit-1 path),
/// and bumping the budget clears it.
#[test]
fn check_rules_enforces_a_structural_budget_and_relaxing_passes() {
    let tmp = deeply_nested_project(Some("[constraints]\nmax_nesting_depth = 4\n"));
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "a depth-5 function violates max_nesting_depth=4: {:?}",
        report.violations
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.rule == "max_nesting_depth" && v.severity == "error"),
        "the budget violation is reported as an error: {:?}",
        report.violations
    );

    // Relax the budget (rewrite rules.toml) → the same fixture passes.
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[constraints]\nmax_nesting_depth = 10\n",
    );
    let relaxed = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        relaxed.passed,
        "a relaxed budget admits the fixture: {:?}",
        relaxed.violations
    );
}

/// CR-005 §3.2: the worst-offender plumbing surfaces a real offender end-to-end —
/// a depth-5 function appears in `scan`'s nesting list with its descriptor. Guards
/// against a silently-broken `scan → worst_offenders` path (which the clean-fixture
/// determinism test alone would not catch).
#[test]
fn scan_populates_worst_offenders_for_a_deeply_nested_fixture() {
    let tmp = deeply_nested_project(None);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let result = engine.scan(true).expect("scan runs");
    assert!(
        !result.worst_offenders.nesting.is_empty(),
        "a depth-5 function surfaces as a nesting offender"
    );
    let top = &result.worst_offenders.nesting[0];
    assert_eq!(top.name, "deeply_nested");
    assert!(
        top.detail.contains("nesting depth"),
        "the offender carries a deterministic descriptor: {}",
        top.detail
    );
}

// ── FR-RC-01/02/03 / NFR-RA-02 / UAT-RC-01 / UAT-SY-03: reconcile ──────────

#[test]
fn scan_reflects_an_out_of_band_edit_via_reconcile() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let before = engine.scan(true).expect("scan runs");
    assert!(
        before.freshness.contains("reconciled 0 files"),
        "nothing changed since the index: {}",
        before.freshness
    );

    // The out-of-band edit, with NO watcher running (ADR-11's fitness
    // function): introduce a cycle that must move the signal.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
fn tangle_a() {
    tangle_b();
}
fn tangle_b() {
    tangle_a();
}
",
    );
    let after = engine.scan(true).expect("scan runs");
    assert!(
        after.freshness.contains("reconciled 1 files"),
        "O(changed): exactly the edited file re-entered (UAT-RC-01): {}",
        after.freshness
    );
    assert!(
        after.signal < before.signal,
        "the edit is reflected in the signal (NFR-RA-02): {:?} -> {:?}",
        before.signal,
        after.signal
    );
    assert!(
        (after.metrics.acyclicity.raw - 1.0).abs() < f64::EPSILON,
        "the new cycle is scored"
    );
}

/// ADR-11: `rescan` replays the last scan's parameters — assumed-fresh after
/// a `--no-reconcile` scan, reconciling after a default scan.
#[test]
fn rescan_replays_the_last_scan_parameters() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    engine.scan(false).expect("scan --no-reconcile");
    let replayed = engine.rescan().expect("rescan runs");
    assert!(
        replayed.freshness.contains("assumed-fresh"),
        "rescan replays the no-reconcile parameter: {}",
        replayed.freshness
    );

    engine.scan(true).expect("scan");
    let replayed = engine.rescan().expect("rescan runs");
    assert!(
        replayed.freshness.contains("reconciled"),
        "rescan replays the reconciling parameter: {}",
        replayed.freshness
    );
}

#[test]
fn no_reconcile_skips_the_edit_and_marks_assumed_fresh() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    let before = engine.scan(true).expect("scan runs");

    // Out-of-band edit, then an assumed-fresh scan (UAT-RC-03).
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    helper();
}
fn helper() {}
fn tangle_a() {
    tangle_b();
}
fn tangle_b() {
    tangle_a();
}
",
    );
    let stale = engine.scan(false).expect("scan --no-reconcile runs");
    assert!(
        stale.freshness.contains("assumed-fresh (--no-reconcile)"),
        "the freshness line says so (FR-RC-04): {}",
        stale.freshness
    );
    assert_eq!(
        stale.signal, before.signal,
        "the edit is NOT reflected without reconcile"
    );

    // The next reconciling scan picks the edit up.
    let fresh = engine.scan(true).expect("scan runs");
    assert!(fresh.signal < before.signal, "reconcile is the backstop");
}

#[test]
fn a_never_indexed_tree_degrades_to_a_full_index() {
    // UAT-RC-02: init-but-no-index — the first aggregate run builds the
    // whole graph rather than failing.
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let result = engine.scan(true).expect("scan runs");
    assert!(
        result.freshness.contains("reconciled 1 files"),
        "the full-index degrade reconciled the whole (1-file) tree: {}",
        result.freshness
    );
    assert!(result.signal.is_some(), "the fresh repo scores");
}

// ── NFR-RA-11 / ADR-11: partial reconcile failure degrades with INCOMPLETE ──

#[test]
fn a_partial_reconcile_failure_stamps_incomplete() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    // An unreadable (non-UTF-8) source file the reconcile discovers but
    // cannot enter into the graph.
    fs::write(tmp.path().join("src/bad.rs"), [0xFF, 0xFE, 0x00, 0x9F]).unwrap();

    let result = engine.scan(true).expect("scan still emits a signal");
    assert!(
        result.freshness.starts_with("INCOMPLETE"),
        "the degradation is prominent, never silent (NFR-RA-11): {}",
        result.freshness
    );
    assert!(
        result.freshness.contains("src/bad.rs"),
        "the unsynced file is named: {}",
        result.freshness
    );
    assert!(
        result.signal.is_some(),
        "the signal is still emitted (availability over abort, ADR-11)"
    );
    assert!(
        result.warnings.iter().any(|w| w.contains("src/bad.rs")),
        "the warning rides the read-model: {:?}",
        result.warnings
    );
}

// ── FR-GV-09 / FR-GV-06 / UAT-GV-05: snapshots and evolution ────────────────

#[test]
fn every_scan_and_gate_appends_a_snapshot_and_evolution_windows_them() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();

    engine.scan(true).expect("scan 1");
    engine.scan(true).expect("scan 2");
    engine.gate(None, true, true).expect("gate --save");
    engine.gate(None, false, true).expect("gate");
    assert_eq!(
        snapshot_count(rt),
        4,
        "every scan/gate writes a snapshot (FR-GV-09)"
    );

    let evolution = engine.evolution(None).expect("evolution runs");
    assert_eq!(evolution.limit, 30, "default window (FR-GV-06)");
    assert_eq!(evolution.snapshots.len(), 4);
    assert!(
        evolution.snapshots[0].signal_delta.is_none(),
        "the first point has no predecessor"
    );
    assert!(
        evolution.snapshots[1].signal_delta.is_some(),
        "later points carry deltas"
    );
    assert_eq!(evolution.snapshots[1].metric_deltas.len(), 5);

    // The window cut respects --limit and keeps the most recent points.
    let windowed = engine.evolution(Some(2)).expect("evolution --limit 2");
    assert_eq!(windowed.snapshots.len(), 2);
    assert_eq!(
        windowed.snapshots[1].snapshot_id, evolution.snapshots[3].snapshot_id,
        "the newest snapshot is retained"
    );
    assert!(
        windowed.snapshots[0].signal_delta.is_some(),
        "deltas are computed over the full series before the cut"
    );
}

// ── FR-GV-07 / UAT-GV-05: the DSM ───────────────────────────────────────────

#[test]
fn dsm_returns_a_square_layer_ordered_matrix() {
    let tmp = layered_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine
        .dsm(Some(logos_core::governance::DsmGranularity::File), true)
        .expect("dsm runs");
    assert_eq!(report.granularity, "file");
    let n = report.rows.len();
    assert!(n >= 2, "both fixture files appear");
    assert!(
        report.matrix.len() == n && report.matrix.iter().all(|row| row.len() == n),
        "the matrix is square"
    );

    // Rows are ordered by layer order then name: domain (1) before
    // presentation (2).
    let domain = report
        .rows
        .iter()
        .position(|r| r.name == "src/domain_core.rs")
        .expect("domain row");
    let ui = report
        .rows
        .iter()
        .position(|r| r.name == "src/ui_view.rs")
        .expect("ui row");
    assert!(domain < ui, "layer order drives row order");
    assert_eq!(report.rows[domain].layer.as_deref(), Some("domain"));

    // The domain → presentation dependency lands in cell (domain, ui) — an
    // upward dep ABOVE the diagonal in this ordering (the back-edge the
    // matrix makes visible, FR-GV-07).
    assert!(
        report.matrix[domain][ui] >= 1,
        "the cross-file dependency is counted"
    );

    // Default granularity is the module rollup.
    let default = engine.dsm(None, false).expect("dsm runs");
    assert_eq!(default.granularity, "module");
}

// ── FR-GV-08 / UAT-GV-05: test gaps ─────────────────────────────────────────

#[test]
fn test_gaps_lists_the_untested_function_with_the_caveat() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn covered() {}
pub fn uncovered() {}
",
    );
    write(
        tmp.path(),
        "tests/api.rs",
        "\
pub fn test_covered() {
    covered();
}
fn covered() {}
",
    );
    // NOTE: the tests/ file carries its own local `covered` because the
    // fixture's cross-crate test→lib resolution is not the point here — the
    // BFS over whatever `calls` edges resolve is.
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.test_gaps(None, true).expect("test_gaps runs");
    assert_eq!(report.limit, 50, "default cap (FR-GV-08)");
    assert!(
        report.caveat.contains("static reachability"),
        "the honesty caveat is always emitted (BR-16): {}",
        report.caveat
    );
    assert!(
        report.untested.iter().any(|g| g.name == "uncovered"),
        "the untested fn is listed: {:?}",
        report.untested
    );
    assert!(
        !report.untested.iter().any(|g| g.name == "test_covered"),
        "test nodes are never gaps"
    );
    assert!(report.coverage_ratio.is_some(), "functions exist → a ratio");
    assert_eq!(
        report.total_functions,
        report.covered_functions + report.untested.len() as u64,
        "the arithmetic is consistent (untruncated case)"
    );
}

#[test]
fn test_gaps_and_is_test_annotation_classify_the_identical_function_set() {
    // CR-001 CRA-01: `test_gaps` reads the persisted `is_test` column, so it
    // can never disagree with the annotation about what a test is. No `main`
    // entry point, to keep the test/non-test partition unconfounded.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn produced() {
    helper();
}
fn helper() {}

#[cfg(test)]
mod tests {
    fn covers_it() {}
}
",
    );
    write(tmp.path(), "tests/api.rs", "fn exercise() {}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let rt = engine.runtime().unwrap();
    let fn_nodes = || {
        rt.submit_read(|store| store.annotation_nodes())
            .unwrap()
            .into_iter()
            .filter(|n| !n.derived && matches!(n.kind, NodeKind::Function | NodeKind::Method))
    };
    // The annotation's notion of "test": is_test=true function nodes.
    let annot_tests: HashSet<String> = fn_nodes().filter(|n| n.is_test).map(|n| n.name).collect();
    assert!(
        annot_tests.contains("covers_it") && annot_tests.contains("exercise"),
        "the evidence (#[cfg(test)]) and path (tests/) fns are is_test: {annot_tests:?}"
    );

    // No is_test function is ever reported as a gap.
    let report = engine.test_gaps(None, true).expect("test_gaps runs");
    for gap in &report.untested {
        assert!(
            !annot_tests.contains(&gap.name),
            "a test fn must never be a gap (CRA-01 parity): {}",
            gap.name
        );
    }
    // `total_functions` counts only the non-test functions, so the set
    // `test_gaps` excludes is exactly the annotation's is_test set (CRA-01).
    let all_fns = fn_nodes().count();
    assert_eq!(
        report.total_functions as usize + annot_tests.len(),
        all_fns,
        "test_gaps and the annotation partition the functions identically (CRA-01)"
    );
}

// ── health ──────────────────────────────────────────────────────────────────

#[test]
fn health_reports_store_integrity_and_counts() {
    let tmp = clean_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();

    let health = engine.health(true).expect("health runs");
    assert!(health.ok, "{}", health.message);
    assert!(health.fts_ok);
    assert!(
        health.structural_ok && health.structural_faults.is_empty(),
        "a clean graph is structurally sound (CR-052, NFR-RA-13)"
    );
    assert_eq!(health.schema_version, 16, "migration 16 applied");
    assert!(health.db_size_bytes > 0);
    assert!(health.db_path.ends_with("logos.db"));
    assert!(health.files >= 1 && health.nodes >= 2);
    assert!(
        health.freshness.contains("unresolved refs"),
        "health is an aggregate run with a freshness line (BR-03): {}",
        health.freshness
    );
}

// ── ADR-14: structural vs usage failures at the façade ──────────────────────

#[test]
fn an_invalid_rules_contract_fails_loud_as_a_config_error() {
    let tmp = clean_project();
    write(tmp.path(), ".logos/rules.toml", "not = valid = toml [");
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let err = engine
        .check_rules(None, true)
        .expect_err("an invalid contract is a structural failure");
    assert!(
        err.downcast_ref::<logos_core::config::ConfigError>()
            .is_some(),
        "the ConfigError survives for the CLI's exit-2 mapping: {err:#}"
    );
}

#[test]
fn quality_methods_on_a_transient_engine_fail_loud() {
    let engine = Engine::open("/tmp");
    assert!(
        engine.scan(true).is_err(),
        "no runtime → structural failure, never a fabricated signal"
    );
    assert!(engine.evolution(None).is_err());
}

// ── FR-GV-11 / CR-002: coupling & redundancy constraint budgets ─────────────

/// A four-file fixture with a genuinely over-coupled hub — called from three
/// SIBLING MODULES (`a.rs`/`b.rs`/`c.rs`), not three symbols in one file —
/// parameterised by the `rules.toml` content so the same graph can be scored
/// with and without the budgets (the orthogonality check). Module grain
/// ([CR-065]) means three callers sharing ONE module would collapse to a
/// single neighbour, so each caller lives in its own file to produce a real
/// cross-module fan-in of 3.
///
/// [CR-065]: ../../docs/requests/CR-065-module-grain-coupling-metric.md
fn hub_project(rules: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    if !rules.is_empty() {
        write(tmp.path(), ".logos/rules.toml", rules);
    }
    write(tmp.path(), "src/hub.rs", "pub fn hub() {}\n");
    write(
        tmp.path(),
        "src/a.rs",
        "use crate::hub::hub;\n\npub fn a() {\n    hub();\n}\n",
    );
    write(
        tmp.path(),
        "src/b.rs",
        "use crate::hub::hub;\n\npub fn b() {\n    hub();\n}\n",
    );
    write(
        tmp.path(),
        "src/c.rs",
        "use crate::hub::hub;\n\npub fn c() {\n    hub();\n}\n",
    );
    tmp
}

/// FR-GV-11 / BR-19 / [CR-065]: a module whose inbound dependency-edge count
/// (distinct neighbouring modules) exceeds `max_fan_in` is reported with its
/// count; the check fails and re-runs are byte-identical (NFR-RA-06).
///
/// [CR-065]: ../../docs/requests/CR-065-module-grain-coupling-metric.md
#[test]
fn coupling_budget_flags_an_over_coupled_hub_and_rerun_is_identical() {
    let tmp = hub_project("[constraints]\nmax_fan_in = 2\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "the over-coupled hub fails the check (exit 1)"
    );
    let hub = report
        .violations
        .iter()
        .find(|v| v.rule == "max_fan_in")
        .expect("a max_fan_in violation is reported");
    assert_eq!(hub.severity, "error");
    assert!(
        hub.message.contains("hub") && hub.message.contains("fan-in"),
        "the hub and its edge count are surfaced: {}",
        hub.message
    );
    // Real extraction gives every file a synthetic Module node (S-011), so the
    // rollup vertex resolves via the PRIMARY `module:<symbol>` key, not the
    // `file:<path>` fallback — this pins that `file` is still populated from
    // the underlying Module row on that real-world path, not silently empty.
    assert_eq!(hub.file, "src/hub.rs", "the offending module's file is surfaced");
    assert!(
        report.violations.iter().all(|v| v.rule == "max_fan_in"),
        "only the coupling budget is active: {:?}",
        report
            .violations
            .iter()
            .map(|v| v.rule.as_str())
            .collect::<Vec<_>>()
    );

    // NFR-RA-06: a re-run yields the identical violation list.
    let again = engine.check_rules(None, true).expect("re-run");
    assert_eq!(again.violations, report.violations);
}

/// FR-GV-11: a project whose `is_dead` function count exceeds `max_dead` is
/// reported project-wide; a generous ceiling passes.
#[test]
fn redundancy_budget_flags_dead_functions_over_the_ceiling() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[constraints]\nmax_dead = 1\n",
    );
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn main() {}
fn orphan_one() {}
fn orphan_two() {}
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.check_rules(None, true).expect("check_rules runs");

    assert!(!report.passed, "two dead functions over max_dead = 1 fails");
    let dead = report
        .violations
        .iter()
        .find(|v| v.rule == "max_dead")
        .expect("a max_dead violation is reported");
    assert_eq!(dead.node_id, None, "redundancy budgets are project-wide");
    assert!(
        dead.message.contains("dead functions") && dead.message.contains("max_dead = 1"),
        "{}",
        dead.message
    );
}

/// CR-002 invariant: the rules gate is orthogonal to the metrics gate — adding
/// the coupling/redundancy budgets (which make `check_rules` fail) leaves the
/// `gate` signal byte-identical over the same source.
#[test]
fn budgets_are_orthogonal_to_the_metrics_gate() {
    let without = hub_project("");
    let signal_without = Engine::start(without.path())
        .expect("engine starts")
        .gate(None, false, true)
        .expect("gate runs")
        .signal;

    let with = hub_project("[constraints]\nmax_fan_in = 2\nmax_dead = 0\n");
    let engine = Engine::start(with.path()).expect("engine starts");
    // The budgets fire here — but the metric signal must not move.
    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(!report.passed, "the budgets are active and flag the hub");
    let signal_with = engine.gate(None, false, true).expect("gate runs").signal;

    assert_eq!(
        signal_without, signal_with,
        "rules gate ⊥ metrics gate: the signal is unchanged by adding budgets"
    );
}

// ── FR-GV-12 / CR-002 / UAT-GV-01: [[forbidden_imports]] end-to-end ──────────

/// A `from`-glob source file (`src/web_*.rs`) importing a `to`-glob target
/// (`src/db_*.rs`) — the shape FR-GV-12 bans. Mirrors `layered_project`'s flat
/// `use crate::<stem>::<fn>` layout so resolution binds the import edge.
fn forbidden_import_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[forbidden_imports]]
from   = \"src/web_*.rs\"
to     = \"src/db_*.rs\"
reason = \"the web layer must not import the db directly\"
",
    );
    write(
        tmp.path(),
        "src/web_handler.rs",
        "\
use crate::db_query::run;

pub fn handle() {
    run();
}
",
    );
    write(tmp.path(), "src/db_query.rs", "pub fn run() {}\n");
    tmp
}

#[test]
fn check_rules_flags_a_forbidden_import_and_materialises_one_edge() {
    use logos_core::model::EdgeKind;

    let tmp = forbidden_import_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "a forbidden import fails the check (FR-GV-03)"
    );
    assert!(
        report.checked_rules >= 1,
        "the forbidden_imports rule is active"
    );

    let fi: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule.starts_with("forbidden_import:"))
        .collect();
    assert_eq!(
        fi.len(),
        1,
        "exactly one forbidden import: {:?}",
        report.violations
    );
    assert_eq!(fi[0].rule, "forbidden_import:src/web_*.rs->src/db_*.rs");
    assert_eq!(fi[0].severity, "error");
    assert!(
        fi[0].message.contains("src/web_handler.rs")
            && fi[0].message.contains("src/db_query.rs")
            && fi[0]
                .message
                .contains("the web layer must not import the db directly"),
        "the violation names both files and the reason: {}",
        fi[0].message
    );

    // The matched edge is materialised as a derived forbidden_dependency edge.
    let count_forbidden = |engine: &Engine| -> usize {
        engine
            .runtime()
            .unwrap()
            .submit_read(|s| s.all_edges())
            .expect("read edges")
            .into_iter()
            .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
            .count()
    };
    assert_eq!(
        count_forbidden(&engine),
        1,
        "one derived edge for the match"
    );

    // FR-GV-12 / NFR-RA-06: a re-run yields identical violations and no
    // duplicate derived edges.
    let again = engine.check_rules(None, true).expect("re-run");
    assert_eq!(
        again.violations, report.violations,
        "violations are byte-identical"
    );
    assert_eq!(
        count_forbidden(&engine),
        1,
        "derived edges do not accumulate"
    );
}

#[test]
fn an_unmatched_import_is_not_flagged() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[forbidden_imports]]
from = \"src/web_*.rs\"
to   = \"src/db_*.rs\"
",
    );
    // web -> util: the source matches `from`, but the target is off-glob.
    write(
        tmp.path(),
        "src/web_handler.rs",
        "use crate::util_helpers::run;\n\npub fn handle() { run(); }\n",
    );
    write(tmp.path(), "src/util_helpers.rs", "pub fn run() {}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        report
            .violations
            .iter()
            .all(|v| !v.rule.starts_with("forbidden_import:")),
        "an import that misses the `to` glob is not flagged: {:?}",
        report.violations
    );
    assert!(
        report.passed,
        "no other rule is active, so the check passes"
    );
}

#[test]
fn an_invalid_forbidden_import_glob_fails_to_load() {
    // NFR-SE-04 / NFR-UX-02: an invalid glob fails at load (mapped to exit 2 by
    // the surfaces); the engine evaluation never proceeds past the bad contract.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[[forbidden_imports]]\nfrom = \"src/{web\"\nto = \"src/db_*.rs\"\n",
    );
    write(tmp.path(), "src/web_handler.rs", "pub fn handle() {}\n");
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let err = engine
        .check_rules(None, true)
        .expect_err("an invalid glob must fail the load");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("src/{web") || msg.to_lowercase().contains("glob"),
        "the error names the offending glob: {msg}"
    );
}

// ── FR-GV-13 / FR-AN-05 / FR-GV-08 / CR-002: [[require_tested]] end-to-end ────

/// A `[[require_tested]]` project over `src/api/**`: one exported fn covered by
/// an in-file `#[cfg(test)]` test, one exported fn no test reaches, and a
/// non-exported helper. Resolution binds the test→fn `calls` edge intra-file,
/// so the BFS seeded from the persisted `is_test` column (FR-AN-05) reaches the
/// covered fn (the same seam `test_gaps` uses, FR-GV-08).
fn require_tested_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[require_tested]]
paths  = [\"src/api/**\"]
reason = \"every public API symbol needs a test path\"
",
    );
    write(
        tmp.path(),
        "src/api/users.rs",
        "\
pub fn create_user() {}

pub fn delete_user() {}

fn internal_helper() {}

#[cfg(test)]
mod tests {
    use super::*;

    fn covers_create() {
        create_user();
    }
}
",
    );
    tmp
}

#[test]
fn check_rules_flags_an_uncovered_exported_symbol_with_its_reason_and_caveat() {
    let tmp = require_tested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "an uncovered exported symbol fails the check (FR-GV-03)"
    );
    assert!(
        report.checked_rules >= 1,
        "the require_tested contract is active"
    );

    let rt: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule.starts_with("require_tested:"))
        .collect();
    assert_eq!(
        rt.len(),
        1,
        "exactly the uncovered exported fn is flagged: {:?}",
        report.violations
    );
    let v = rt[0];
    assert_eq!(v.rule, "require_tested:src/api/**");
    assert_eq!(
        v.rule_type, "constraint",
        "a require_tested gap reuses the constraint rule_type (CR-002 no migration)"
    );
    assert_eq!(v.severity, "error");
    assert!(
        v.message.contains("delete_user"),
        "AC1: names the uncovered exported symbol, not the covered one: {}",
        v.message
    );
    assert!(
        !v.message.contains("internal_helper"),
        "AC2: a non-exported symbol is exempt, never flagged: {}",
        v.message
    );
    assert!(
        v.message
            .contains("every public API symbol needs a test path"),
        "AC1: the declared reason is surfaced: {}",
        v.message
    );
    // AC3 (FR-GV-08, BR-16): the static-reachability caveat rides on the report.
    assert!(
        v.message.contains("not execution coverage"),
        "AC3: the static-reachability caveat is surfaced: {}",
        v.message
    );

    // The covered exported fn (reached transitively from the in-file test) and
    // the non-exported helper are both absent from the violation set.
    assert!(
        report
            .violations
            .iter()
            .all(|v| !v.message.contains("create_user")),
        "AC2: the test-reached exported fn passes: {:?}",
        report.violations
    );
}

#[test]
fn require_tested_check_rules_is_byte_identical_across_runs() {
    // NFR-RA-06: two consecutive `check_rules` yield identical violations.
    let tmp = require_tested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let first = engine.check_rules(None, true).expect("first run");
    let second = engine.check_rules(None, true).expect("second run");
    assert_eq!(
        first.violations, second.violations,
        "require_tested violations are byte-identical across runs"
    );
}

// ── FR-GV-14 / FR-GV-15 / FR-DG-04 / CR-003: doc_gaps + [[require_documented]] ──
// These exercise the documentation graph end-to-end, so they need the markdown
// grammar (default-on, but gated for `--no-default-features` runs).

/// A documentation project over `src/api/**`: one exported fn a doc section
/// references (`create_user`), two exported fns no doc references
/// (`delete_user`, `purge_user`), and a non-exported helper. The doc→code
/// inline-code token binds a `DocReference` from the `Users` `DocSection`
/// (FR-DG-04), so the documented set seeded for `doc_gaps`/`[[require_documented]]`
/// contains exactly `create_user`.
#[cfg(feature = "lang-markdown")]
fn documented_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[require_documented]]
paths  = [\"src/api/**\"]
reason = \"every public API symbol needs documentation\"
",
    );
    write(
        tmp.path(),
        "src/api/users.rs",
        "\
pub fn create_user() {}

pub fn delete_user() {}

pub fn purge_user() {}

fn internal_helper() {}
",
    );
    // The `Users` section references `create_user` by inline-code token; the
    // workspace-unique symbol binds a doc→code DocReference (FR-DG-04).
    write(
        tmp.path(),
        "docs/api.md",
        "\
# API

## Users

Call `create_user` to add a user.
",
    );
    tmp
}

#[cfg(feature = "lang-markdown")]
#[test]
fn doc_gaps_lists_the_undocumented_exported_functions_with_the_caveat() {
    let tmp = documented_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.doc_gaps(None, true).expect("doc_gaps runs");
    assert_eq!(report.limit, 50, "default cap (FR-GV-14)");
    assert!(
        report.caveat.contains("reference presence"),
        "the honesty caveat is always emitted: {}",
        report.caveat
    );
    // Exactly the two undocumented exported fns are listed, in deterministic
    // (file, name) order — and the documented and non-exported ones are not.
    let names: Vec<&str> = report
        .undocumented
        .iter()
        .map(|g| g.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["delete_user", "purge_user"],
        "exactly the undocumented exported fns, sorted by file then name (FR-GV-14)"
    );
    assert_eq!(
        report.total_functions, 3,
        "the scope is the three exported functions (non-exported helper excluded)"
    );
    assert_eq!(
        report.documented_functions, 1,
        "only create_user is referenced by a doc section"
    );
    assert_eq!(
        report.total_functions,
        report.documented_functions + report.undocumented.len() as u64,
        "the arithmetic is consistent (untruncated case)"
    );
    assert!(
        !report.truncated,
        "two gaps fit under the default cap of 50 — not truncated"
    );
    assert!(
        report.documentation_ratio.is_some(),
        "functions exist → a ratio"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn doc_gaps_honours_the_limit_and_reports_truncation() {
    let tmp = documented_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.doc_gaps(Some(1), true).expect("doc_gaps runs");
    assert_eq!(report.limit, 1);
    assert!(report.truncated, "two gaps exist but only one is listed");
    assert_eq!(
        report.undocumented.len(),
        1,
        "the cap is honoured (FR-GV-14)"
    );
    assert_eq!(
        report.undocumented[0].name, "delete_user",
        "the deterministic order lists the first gap under the cap"
    );
    assert_eq!(
        report.total_functions, 3,
        "totals reflect the full scope, not the truncated listing"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn doc_gaps_is_byte_identical_across_runs() {
    // NFR-RA-06: the report is reproducible across repeated runs.
    let tmp = documented_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let first = engine.doc_gaps(None, true).expect("first run");
    let second = engine.doc_gaps(None, true).expect("second run");
    let names = |r: &logos_core::models::quality::DocGapsReport| {
        r.undocumented
            .iter()
            .map(|g| (g.name.clone(), g.file.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&first), names(&second), "doc_gaps is reproducible");
    assert_eq!(first.total_functions, second.total_functions);
    assert_eq!(first.documented_functions, second.documented_functions);
}

#[cfg(feature = "lang-markdown")]
#[test]
fn check_rules_flags_an_undocumented_exported_symbol_with_its_reason() {
    let tmp = documented_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.passed,
        "an undocumented exported symbol fails the check (FR-GV-03)"
    );

    let rd: Vec<_> = report
        .violations
        .iter()
        .filter(|v| v.rule.starts_with("require_documented:"))
        .collect();
    assert_eq!(
        rd.len(),
        2,
        "exactly the two undocumented exported fns are flagged: {:?}",
        report.violations
    );
    for v in &rd {
        assert_eq!(v.rule, "require_documented:src/api/**");
        assert_eq!(
            v.rule_type, "constraint",
            "a require_documented gap reuses the constraint rule_type (CR-003 no migration)"
        );
        assert_eq!(v.severity, "error");
        assert!(
            v.message
                .contains("every public API symbol needs documentation"),
            "the declared reason is surfaced: {}",
            v.message
        );
        assert!(
            v.message
                .contains("reference presence, not documentation quality"),
            "the honesty caveat rides on the report: {}",
            v.message
        );
    }
    let flagged: Vec<&str> = rd.iter().map(|v| v.message.as_str()).collect();
    assert!(
        flagged.iter().any(|m| m.contains("delete_user"))
            && flagged.iter().any(|m| m.contains("purge_user")),
        "both undocumented exported fns are named: {flagged:?}"
    );
    assert!(
        report
            .violations
            .iter()
            .all(|v| !v.message.contains("create_user") && !v.message.contains("internal_helper")),
        "the documented fn and the non-exported helper are exempt: {:?}",
        report.violations
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn require_documented_check_rules_is_byte_identical_across_runs() {
    // NFR-RA-06: two consecutive `check_rules` yield identical violations.
    let tmp = documented_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let first = engine.check_rules(None, true).expect("first run");
    let second = engine.check_rules(None, true).expect("second run");
    assert_eq!(
        first.violations, second.violations,
        "require_documented violations are byte-identical across runs"
    );
}

#[cfg(feature = "lang-markdown")]
#[test]
fn absent_require_documented_contract_enforces_nothing() {
    // FR-GV-15: like every contract, opt-in — an absent [[require_documented]]
    // enforces nothing. Same sources, no rules.toml contract.
    let tmp = documented_project();
    fs::remove_file(tmp.path().join(".logos/rules.toml")).unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        report
            .violations
            .iter()
            .all(|v| !v.rule.starts_with("require_documented:")),
        "no contract → no require_documented violations: {:?}",
        report.violations
    );
    // doc_gaps stays a read-only report regardless of any contract.
    let gaps = engine.doc_gaps(None, true).expect("doc_gaps runs");
    assert_eq!(
        gaps.undocumented.len(),
        2,
        "doc_gaps reports gaps with or without a contract (FR-GV-14)"
    );
}

/// FR-QM-02 / FR-GV-11 / CR-022 / ADR-30: the `max_cycles` rule consumes the
/// SAME `acyclicity.raw` the metric scores (single source), so the metric and
/// the rule can never disagree — and under metric-semantics v4 both exclude
/// self-recursion. A fixture with one genuine 2-node mutual recursion plus a
/// self-recursive function scores exactly one cycle in the metric, and the rule
/// reports that same single count; a self-recursion-only fixture scores zero in
/// both, so the `max_cycles = 0` budget passes.
#[test]
fn max_cycles_rule_agrees_with_acyclicity_metric_excluding_self_recursion() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), ".logos/rules.toml", "[constraints]\nmax_cycles = 0\n");
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn ping() {
    pong();
}
pub fn pong() {
    ping();
}
pub fn walk() {
    walk();
}
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // The metric: exactly one cycle — the `ping`/`pong` multi-node SCC. `walk`'s
    // self-loop is a singleton SCC and excluded under v4 (CR-022 / ADR-30).
    let scan = engine.scan(true).expect("scan runs");
    let metric_cycles = scan.metrics.acyclicity.raw;
    assert_eq!(
        metric_cycles, 1.0,
        "one multi-node SCC counts; the self-recursion does not (v4)"
    );

    // The rule reads the same `acyclicity.raw`, so it reports the same count.
    let report = engine.check_rules(None, true).expect("check_rules runs");
    let violation = report
        .violations
        .iter()
        .find(|v| v.rule == "max_cycles")
        .expect("max_cycles = 0 is violated by the one genuine cycle");
    assert!(
        violation
            .message
            .starts_with(&format!("{} dependency cycles", metric_cycles as u64)),
        "the rule's cycle count matches acyclicity.raw (single source): {}",
        violation.message
    );

    // Negative control: drop the mutual recursion, keep only self-recursion →
    // the metric scores zero and the rule agrees by passing the same budget.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn walk() {
    walk();
}
pub fn step() {
    walk();
}
",
    );
    let scan = engine.scan(true).expect("scan runs");
    assert_eq!(
        scan.metrics.acyclicity.raw, 0.0,
        "self-recursion alone is not a cycle (v4)"
    );
    let report = engine.check_rules(None, true).expect("check_rules runs");
    assert!(
        !report.violations.iter().any(|v| v.rule == "max_cycles"),
        "max_cycles = 0 passes when the only recursion is a self-loop: {:?}",
        report.violations
    );
}
