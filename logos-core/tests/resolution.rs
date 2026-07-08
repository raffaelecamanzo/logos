//! Integration tests for the resolution engine (S-011 / FR-RS-01..05,
//! NFR-RA-05, NFR-RA-11, ADR-10), exercised end-to-end through
//! [`Engine::index`]/[`Engine::sync`] against real temp-directory fixtures and
//! the Logos dogfood tree.
//!
//! Coverage by acceptance criterion:
//! - imports and path heads resolve per the scope-hierarchy strategy
//!   (FR-RS-01, FR-RS-02 — Rust `use` aliases are the Rust face of
//!   path-alias honouring; UAT-RS-03's tsconfig case lands with the TS
//!   plugin, S-015);
//! - a deferred reference binds on a later sync; unresolvable refs persist,
//!   never invented (FR-RS-03, NFR-RA-05, UAT-RS-01);
//! - coverage/confidence is surfaced and measured on the dogfood (FR-RS-04,
//!   FR-RS-05, UAT-RS-02, NFR-RA-11);
//! - a self-referential import cycle terminates (sprint Testing &
//!   Verification);
//! - capture-before-delete survives the resolution rebinding pass (ADR-10
//!   end-to-end, the sprint risk-table fitness function).

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::resolve;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The id of the unique node with `name` and `kind`.
fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    rt.submit_read(|store| {
        let rows = store.search(name, Some(kind), 16)?;
        Ok(rows.into_iter().find(|r| r.name == name).map(|r| r.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

// ── FR-RS-01/02/03: scope-hierarchy binding on a fixture workspace ───────────

#[test]
fn index_binds_imports_aliases_and_calls_across_files() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use crate::util::run;
use crate::util as u_mod;

pub fn alpha() {
    run();
    helper();
    u_mod::run();
    crate::util::run();
}
pub fn helper() {}
",
    );
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let alpha = node_id(rt, "alpha", NodeKind::Function);
    let helper = node_id(rt, "helper", NodeKind::Function);
    let run = node_id(rt, "run", NodeKind::Function);
    let util_mod = node_id(rt, "util", NodeKind::Module);
    let lib_mod = node_id(rt, "crate", NodeKind::Module);

    let calls = edges_of(rt, EdgeKind::Calls);
    assert!(calls.contains(&(alpha, helper)), "module-scope bare call");
    assert!(
        calls.contains(&(alpha, run)),
        "alias + renamed-module + crate:: paths all bind to run"
    );

    // FR-RS-01 acceptance: the cross-module import binds to the target nodes
    // (the item import to the fn, the module import to the module node), with
    // the importing file's module node as the source.
    let imports = edges_of(rt, EdgeKind::Imports);
    assert!(imports.contains(&(lib_mod, run)), "use crate::util::run");
    assert!(
        imports.contains(&(lib_mod, util_mod)),
        "use crate::util as …"
    );

    // Everything in this fixture is resolvable: full, honest coverage.
    assert_eq!(result.resolution.refs_unresolved, 0);
    assert!((result.resolution.coverage - 1.0).abs() < f64::EPSILON);
    assert!(
        result.resolution.refs_total >= 6,
        "calls + imports recorded"
    );
}

// ── NFR-RA-05: unresolvable refs persist — never fabricated ──────────────────

#[test]
fn external_refs_persist_unresolved_with_an_honest_coverage_signal() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use anyhow::Result;

pub fn alpha() {
    std::mem::drop(());
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The external targets exist in no indexed crate: zero Calls/Imports
    // edges may appear (NFR-RA-05), the refs persist for retry (FR-RS-03).
    assert!(edges_of(rt, EdgeKind::Calls).is_empty());
    assert!(edges_of(rt, EdgeKind::Imports).is_empty());
    assert_eq!(result.resolution.refs_resolved, 0);
    assert!(result.resolution.refs_unresolved >= 2);
    // Graceful degradation (NFR-RA-11): the result still reports, with an
    // honest sub-1.0 confidence number — not a failure.
    assert!(result.resolution.coverage < 1.0);
    assert!(result.warnings.is_empty());
}

// ── UAT-RS-01: a deferred reference binds on a later sync ────────────────────

#[test]
fn deferred_reference_binds_once_its_target_is_indexed() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn alpha() {
    util::run();
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    assert_eq!(
        first.resolution.refs_resolved, 0,
        "util.rs does not exist yet — the ref must stay unresolved"
    );
    assert!(edges_of(rt, EdgeKind::Calls).is_empty());

    // The target file appears later; the sync's retry sweep binds the ref.
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");
    let second = engine.sync(&[PathBuf::from("src/util.rs")]);
    assert_eq!(second.files_added, 1);

    let alpha = node_id(rt, "alpha", NodeKind::Function);
    let run = node_id(rt, "run", NodeKind::Function);
    assert!(
        edges_of(rt, EdgeKind::Calls).contains(&(alpha, run)),
        "the deferred ref binds on the second pass (UAT-RS-01)"
    );
    assert!(second.resolution.refs_resolved >= 1);
}

// ── Sprint verification: an import cycle terminates ─────────────────────────

#[test]
fn self_referential_import_cycle_completes_without_hanging() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/a.rs",
        "\
pub use crate::b::thing as widget;
pub fn fa() {
    widget();
}
",
    );
    write(
        tmp.path(),
        "src/b.rs",
        "\
pub use crate::a::widget as thing;
pub fn fb() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    // The aliases chase each other; the depth limit terminates the chain and
    // the refs simply stay unresolved — completion IS the assertion.
    assert!(result.resolution.refs_unresolved >= 2);
    assert!(result.warnings.is_empty());
}

// ── ADR-10 end-to-end: capture-before-delete survives resolution ────────────

#[test]
fn cross_file_call_edge_rebinds_through_resolution_after_a_sync() {
    // The sprint risk-table fitness function: plant a *resolved* cross-file
    // call edge (bound by resolution itself, not by hand), sync the callee's
    // file, and verify the edge rebinds to the callee's NEW node id.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/caller.rs",
        "\
pub fn caller() {
    crate::callee::target();
}
",
    );
    write(tmp.path(), "src/callee.rs", "pub fn target() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let caller = node_id(rt, "caller", NodeKind::Function);
    let target_before = node_id(rt, "target", NodeKind::Function);
    assert!(
        edges_of(rt, EdgeKind::Calls).contains(&(caller, target_before)),
        "resolution bound the cross-file call at index time"
    );

    // Edit the callee's file (content change → re-extract → delete+reinsert
    // assigns target a fresh rowid).
    write(
        tmp.path(),
        "src/callee.rs",
        "// a comment shifts everything\npub fn target() {}\n",
    );
    let result = engine.sync(&[PathBuf::from("src/callee.rs")]);
    assert_eq!(result.files_modified, 1);

    let target_after = node_id(rt, "target", NodeKind::Function);
    assert_ne!(
        target_before, target_after,
        "the re-extract reassigned the node id (otherwise this test proves nothing)"
    );
    assert!(
        edges_of(rt, EdgeKind::Calls).contains(&(caller, target_after)),
        "the captured edge rebound to the new node id through resolution (ADR-10)"
    );
}

// ── FR-RS-04: coverage() reads the same numbers from the ledger ──────────────

#[test]
fn coverage_read_matches_the_run_stats() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use std::collections::HashMap;
pub fn alpha() {
    helper();
}
pub fn helper() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    let cov = rt
        .submit_read(|store| resolve::coverage(store))
        .expect("coverage reads");
    assert_eq!(cov.refs_total, result.resolution.refs_total);
    assert_eq!(cov.refs_resolved, result.resolution.refs_resolved);
    assert_eq!(cov.refs_unresolved, result.resolution.refs_unresolved);
    assert_eq!(cov.edges_created, 0, "a pure read creates nothing");
}

// ── FR-RS-05 / UAT-RS-02: the dogfood accuracy measurement ───────────────────

/// Recursively copy every `.rs` file under `dir` into `dst_root/src`,
/// preserving the path relative to `base`.
fn copy_rs_tree(base: &Path, dir: &Path, dst_root: &Path) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            copy_rs_tree(base, &path, dst_root);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path.strip_prefix(base).unwrap();
            let dst = dst_root.join(rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(&path, &dst).unwrap();
        }
    }
}

#[test]
fn dogfood_measures_resolution_accuracy_on_logos_own_source() {
    // FR-RS-05: a *documented measurement* on the Rust dogfood graph —
    // surfaced, not release-gating. The printed numbers are the measurement;
    // the assertions below are a recall floor (known-true bindings exist), a
    // precision guard (a known-ambiguous name must NOT bind), and the honest
    // confidence signal (NFR-RA-11, UAT-RS-02).
    let tmp = TempDir::new().unwrap();
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    copy_rs_tree(&crate_root, &crate_root.join("src"), tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();
    let stats = result.resolution;

    println!(
        "dogfood resolution: refs_total={} resolved={} unresolved={} coverage={:.3} edges_created={}",
        stats.refs_total,
        stats.refs_resolved,
        stats.refs_unresolved,
        stats.coverage,
        stats.edges_created,
    );

    // Decompose the unresolved tail (the documented FR-RS-05 measurement).
    // Method-form refs are receiver calls — overwhelmingly on std/external
    // types (`.iter()`, `.collect()`, …) — and multi-segment paths with a
    // foreign head are external crates: both are unresolvable *by design*
    // (their definitions are not indexed) and persist for retry. The
    // remainder — workspace-rooted or bare *path* refs — is the
    // intra-workspace recall gap, the number AR-05 actually cares about.
    use logos_core::model::RefForm;
    let unresolved = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| !r.resolved)
                .map(|r| (r.form, r.target))
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    let workspace_heads = ["crate", "self", "super"];
    let (mut methods, mut external_paths, mut internal_misses) = (0usize, 0usize, 0usize);
    for (form, target) in &unresolved {
        let head = target.split("::").next().unwrap_or_default();
        if *form == RefForm::Method {
            methods += 1;
        } else if target.contains("::") && !workspace_heads.contains(&head) {
            external_paths += 1;
        } else {
            internal_misses += 1;
        }
    }
    let adjusted =
        (stats.refs_resolved as f64) / (stats.refs_resolved as f64 + internal_misses as f64);
    println!(
        "dogfood resolution: unresolved breakdown — methods={methods} \
         external_paths={external_paths} internal_misses={internal_misses} \
         adjusted_internal_coverage={adjusted:.3}",
    );

    // The ledger is substantial and the bound-ratio is surfaced (UAT-RS-02).
    assert!(stats.refs_total > 500, "the dogfood has a real ledger");
    assert!(
        stats.refs_resolved > 100,
        "a meaningful share of intra-workspace refs binds"
    );
    assert!(
        stats.coverage > 0.0 && stats.coverage < 1.0,
        "honest coverage: std/external refs keep it below 1.0"
    );

    // Recall floor — known-true intra-crate bindings (hand-verified against
    // the source):
    let calls = edges_of(rt, EdgeKind::Calls);
    let pairs = [
        // pipeline/mod.rs: load_files() hashes each file via hash_source().
        ("load_files", "hash_source"),
        // extract/mod.rs: extract() delegates to extract_one().
        ("extract", "extract_one"),
        // extract/mod.rs: collect_refs() normalises via split_path_text().
        ("collect_refs", "split_path_text"),
    ];
    for (from, to) in pairs {
        let from_id = node_id(rt, from, NodeKind::Function);
        let to_id = node_id(rt, to, NodeKind::Function);
        assert!(
            calls.contains(&(from_id, to_id)),
            "known-true call {from} -> {to} must be bound"
        );
    }

    // Precision guard — `as_i32` has three same-named definitions (NodeKind,
    // EdgeKind, RefForm): every receiver-method call `kind.as_i32()` is
    // ambiguous and must stay unresolved under the balanced default. Since
    // CR-068 Part B these `impl`-associated fns are kinded `Method`, not
    // `Function` — the receiver-call exclusion is unchanged, so the guard holds.
    let as_i32_candidates = rt
        .submit_read(|store| {
            Ok(store
                .search("as_i32", Some(NodeKind::Method), 16)?
                .into_iter()
                .filter(|r| r.name == "as_i32")
                .map(|r| r.id)
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    assert!(
        as_i32_candidates.len() >= 2,
        "the ambiguity premise holds (multiple as_i32 definitions)"
    );
    for target in &as_i32_candidates {
        assert!(
            !calls.iter().any(|(_, t)| t == target),
            "an ambiguous method name must never bind (NFR-RA-05/AR-05)"
        );
    }
}

// ── NFR-CC-04: a resolved ref flips back when its target vanishes ────────────

#[test]
fn resolved_ref_flips_back_when_target_file_is_deleted() {
    // The "ledger never lies" guarantee end-to-end: a bound ref whose target
    // file is later removed must report resolved = false again after the
    // sync's retry sweep — stale resolved flags would poison the coverage
    // signal (FR-RS-04, NFR-CC-04).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "pub fn caller() { crate::util::run(); }\n",
    );
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let resolved_count = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.resolved)
                .count())
        })
        .expect("read runs");
    assert!(
        resolved_count >= 1,
        "the cross-file call must be resolved after the index"
    );

    // Remove the target file; the sync deletes its nodes and re-sweeps.
    fs::remove_file(tmp.path().join("src/util.rs")).unwrap();
    let result = engine.sync(&[PathBuf::from("src/util.rs")]);
    assert_eq!(result.files_removed, 1);

    let stale = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.resolved && r.target.contains("run"))
                .count())
        })
        .expect("read runs");
    assert_eq!(
        stale, 0,
        "a ref whose target was deleted must flip back to unresolved"
    );
    assert!(
        result.resolution.refs_unresolved >= 1,
        "the surfaced stats reflect the flip-back honestly"
    );
}

// ── NFR-RA-06: resolution output is deterministic across repeated runs ───────

#[test]
fn resolution_output_is_deterministic_across_repeated_runs() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "use crate::util::run;\npub fn alpha() {\n    run();\n    helper();\n}\npub fn helper() {}\n",
    );
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    let edges_first = edges_of(rt, EdgeKind::Calls);
    let second = engine.index();
    let edges_second = edges_of(rt, EdgeKind::Calls);

    assert_eq!(first.resolution.refs_total, second.resolution.refs_total);
    assert_eq!(
        first.resolution.refs_resolved,
        second.resolution.refs_resolved
    );
    assert!((first.resolution.coverage - second.resolution.coverage).abs() < f64::EPSILON);
    assert_eq!(
        edges_first.len(),
        edges_second.len(),
        "the bound edge set is reproducible"
    );
}
