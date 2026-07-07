//! Integration tests for the navigation service (S-013 / FR-NV-01..09,
//! FR-RC-05, NFR-DM-02, ADR-11), exercised end-to-end through the eight
//! [`Engine`] navigation methods against a real temp-directory fixture.
//!
//! Coverage by acceptance criterion:
//! - `search` is FTS5-ranked and kind-filtered; `context` is a deterministic
//!   bundle replacing multiple file reads (FR-NV-01/02, UAT-NV-01/02);
//! - `impact` returns both labeled directions; `callers`/`callees` honour the
//!   limit (FR-NV-05/06, UAT-NV-03/04);
//! - `node` returns metadata with code opt-in; `status` reports index health;
//!   an unknown symbol degrades gracefully (FR-NV-04/07/09, UAT-NV-05/06/08);
//! - navigation never triggers a reconcile and reads from the RO pool
//!   (FR-RC-05, NFR-DM-02, ADR-11) — the sprint risk-table fitness function;
//! - the very first navigation call auto-indexes an empty graph (FR-IX-07).

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeKind};
use logos_core::models::navigation::EdgeDirection;
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The standard fixture: a two-file crate with a known call chain
/// alpha → {run, beta}, beta → gamma, run in its own file.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use crate::util::run;

pub fn alpha() {
    run();
    beta();
}
pub fn beta() {
    gamma();
}
pub fn gamma() {}
",
    );
    write(tmp.path(), "src/util.rs", "pub fn run() {}\n");
    tmp
}

/// An indexed engine over the standard fixture.
fn indexed_engine(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

// ── FR-NV-01 / UAT-NV-01: FTS5-ranked, kind-filtered search ──────────────────

#[test]
fn search_ranks_known_symbols_first_filters_by_kind_and_honours_limit() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    // A known symbol name is ranked first.
    let result = engine.search("alpha", None, None);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.hits[0].name, "alpha", "exact name ranks first");
    assert_eq!(result.hits[0].kind, NodeKind::Function);
    assert_eq!(result.hits[0].file.as_deref(), Some("src/lib.rs"));
    assert!(result.hits[0].line.is_some(), "hits carry a location");

    // kind=function excludes the `util` module; kind=module finds it.
    let functions = engine.search("util", Some(NodeKind::Function), None);
    assert!(
        functions.hits.iter().all(|h| h.kind == NodeKind::Function),
        "{:?}",
        functions.hits
    );
    let modules = engine.search("util", Some(NodeKind::Module), None);
    assert!(
        modules.hits.iter().any(|h| h.name == "util"),
        "module filter finds the util module: {:?}",
        modules.hits
    );

    // The limit bounds the result set.
    let limited = engine.search("a", None, Some(1));
    assert!(limited.hits.len() <= 1);
}

// ── FR-IX-07: the first navigation call auto-indexes an empty graph ─────────

#[test]
fn first_navigation_call_auto_indexes_a_never_indexed_project() {
    let tmp = fixture();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // No explicit `index()` — search must transparently build the graph.
    let result = engine.search("alpha", None, None);
    assert!(
        result.hits.iter().any(|h| h.name == "alpha"),
        "auto-index served the very first navigation call: {result:?}"
    );
}

// ── FR-NV-02 / UAT-NV-02: deterministic, bounded context bundle ──────────────

#[test]
fn context_bundle_is_deterministic_bounded_and_replaces_file_reads() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let bundle = engine.context("alpha", None, false);
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    assert!(!bundle.nodes.is_empty(), "the task seeds a bundle");
    assert!(
        bundle
            .nodes
            .iter()
            .any(|n| n.symbol.name == "alpha" && n.seed),
        "the FTS seed is in the bundle: {:?}",
        bundle.nodes
    );
    // The ranking contract (FR-NV-02): in this single-seed fixture the seed's
    // match-score dominates, so the top-ranked node IS the seed.
    assert!(
        bundle.nodes[0].seed && bundle.nodes[0].symbol.name == "alpha",
        "the seed ranks first: {:?}",
        bundle.nodes
    );
    // alpha calls run (src/util.rs) — the 1-hop bundle spans both files, so
    // one call replaced at least two naïve reads (the token-saving thesis).
    assert!(
        bundle.est_reads_replaced >= 2,
        "bundle spans multiple files: {:?}",
        bundle.files
    );
    assert_eq!(bundle.est_reads_replaced as usize, bundle.files.len());
    assert!(
        bundle.nodes.iter().all(|n| n.code.is_none()),
        "code is opt-in"
    );

    // max_nodes caps the bundle.
    let capped = engine.context("alpha", Some(2), false);
    assert!(capped.nodes.len() <= 2);

    // Determinism: same graph state, byte-identical bundle (NFR-RA-06).
    let again = engine.context("alpha", None, false);
    assert_eq!(
        serde_json::to_string(&bundle).unwrap(),
        serde_json::to_string(&again).unwrap(),
        "two identical calls produce the identical bundle"
    );
}

#[test]
fn context_include_code_attaches_declaration_source() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let bundle = engine.context("alpha", None, true);
    let seed = bundle
        .nodes
        .iter()
        .find(|n| n.symbol.name == "alpha")
        .expect("seed present");
    assert!(
        seed.code.as_deref().unwrap_or("").contains("fn alpha"),
        "opt-in code carries the declaration: {:?}",
        seed.code
    );
}

// ── FR-NV-03: explore groups neighbourhood source by file ────────────────────

#[test]
fn explore_groups_neighbourhood_source_by_file_and_caps_files() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.explore("alpha", None);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let anchor = result.anchor.as_ref().expect("anchor resolves");
    assert_eq!(anchor.name, "alpha");
    // alpha's file leads; the call into util.rs pulls a second group.
    assert_eq!(result.files[0].file, "src/lib.rs");
    assert!(
        result.total_files >= 2,
        "the neighbourhood spans lib.rs and util.rs: {result:?}"
    );
    // explore returns source (FR-NV-03).
    assert!(
        result.files[0].symbols.iter().any(|s| s
            .code
            .as_deref()
            .unwrap_or("")
            .contains("fn alpha")),
        "groups carry declaration source"
    );

    // max_files caps the groups but total_files stays honest.
    let capped = engine.explore("alpha", Some(1));
    assert_eq!(capped.files.len(), 1);
    assert!(capped.total_files >= 2);
}

// ── FR-NV-04 / UAT-NV-05: node metadata with code opt-in ─────────────────────

#[test]
fn node_returns_metadata_immediate_edges_and_code_only_on_request() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let info = engine.node("beta", false);
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    let detail = info.node.as_ref().expect("beta resolves");
    assert_eq!(detail.symbol.kind, NodeKind::Function);
    assert_eq!(detail.symbol.file.as_deref(), Some("src/lib.rs"));
    assert!(detail.symbol.line.is_some() && detail.end_line.is_some());
    assert!(detail.code.is_none(), "code is opt-in (FR-NV-04)");
    assert!(
        detail.annotations.is_empty(),
        "annotation columns are S-014"
    );

    // The immediate edge summary spans both directions.
    assert!(
        detail
            .edges
            .iter()
            .any(|e| { e.direction == EdgeDirection::In && e.other.name == "alpha" }),
        "inbound call from alpha: {:?}",
        detail.edges
    );
    assert!(
        detail
            .edges
            .iter()
            .any(|e| { e.direction == EdgeDirection::Out && e.other.name == "gamma" }),
        "outbound call to gamma: {:?}",
        detail.edges
    );

    let with_code = engine.node("beta", true);
    let detail = with_code.node.expect("beta resolves");
    assert!(
        detail.code.as_deref().unwrap_or("").contains("fn beta"),
        "opt-in code carries the declaration"
    );
}

// ── FR-NV-05 / UAT-NV-04: callers/callees correct, limit honoured ────────────

#[test]
fn callers_and_callees_return_direct_sets_and_honour_the_limit() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let callers = engine.callers("gamma", None);
    assert_eq!(
        callers
            .callers
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["beta"],
        "gamma's only direct caller is beta"
    );
    assert_eq!(callers.total, 1);

    let callees = engine.callees("alpha", None);
    let names: Vec<&str> = callees.callees.iter().map(|c| c.name.as_str()).collect();
    assert!(
        names.contains(&"run") && names.contains(&"beta"),
        "{names:?}"
    );
    assert_eq!(callees.total, 2);
    assert!(
        !names.contains(&"gamma"),
        "direct callees only — gamma is transitive"
    );

    // The limit bounds the page while `total` stays honest.
    let limited = engine.callees("alpha", Some(1));
    assert_eq!(limited.callees.len(), 1);
    assert_eq!(limited.total, 2, "pre-limit total is reported");
}

// ── FR-NV-06 / UAT-NV-03 / DL-03: impact, both directions, labeled ───────────

#[test]
fn impact_returns_both_labeled_directions_within_the_depth_bound() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let impact = engine.impact("gamma", None);
    assert!(impact.warnings.is_empty(), "{:?}", impact.warnings);
    assert_eq!(impact.upstream_label, "breaks if changed", "DL-03 label");
    assert_eq!(impact.downstream_label, "depends on", "DL-03 label");

    // Upstream: beta directly (d1), alpha transitively (d2), nearest-first.
    let upstream: Vec<(&str, u32)> = impact
        .upstream
        .iter()
        .map(|e| (e.symbol.name.as_str(), e.distance))
        .collect();
    assert!(upstream.contains(&("beta", 1)), "{upstream:?}");
    assert!(upstream.contains(&("alpha", 2)), "{upstream:?}");
    // Downstream: gamma calls nothing.
    assert!(impact.downstream.is_empty(), "{:?}", impact.downstream);

    // The depth bound is respected: at depth 1 alpha (d2) drops out.
    let shallow = engine.impact("gamma", Some(1));
    let upstream: Vec<&str> = shallow
        .upstream
        .iter()
        .map(|e| e.symbol.name.as_str())
        .collect();
    assert!(upstream.contains(&"beta"), "{upstream:?}");
    assert!(!upstream.contains(&"alpha"), "depth bound: {upstream:?}");

    // And the forward direction: alpha depends on run, beta, gamma (d2).
    let forward = engine.impact("alpha", None);
    let downstream: Vec<&str> = forward
        .downstream
        .iter()
        .map(|e| e.symbol.name.as_str())
        .collect();
    for expected in ["run", "beta", "gamma"] {
        assert!(downstream.contains(&expected), "{downstream:?}");
    }
}

// ── FR-NV-07 / UAT-NV-06: status reports index health ────────────────────────

#[test]
fn status_reports_populated_counts_and_the_freshness_contract() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let status = engine.status();
    assert!(status.warnings.is_empty(), "{:?}", status.warnings);
    assert!(status.indexed);
    assert_eq!(status.file_count, 2, "both fixture files indexed");
    assert!(status.node_count > 0 && status.edge_count > 0);
    assert!(status.db_size_bytes > 0, "the store exists on disk");
    assert!(
        status.last_full_index_at.is_some(),
        "a full index ran this process (FR-NV-07, UAT-NV-06)"
    );
    assert!(
        status.last_sync_at.is_some(),
        "a write timestamp is observed"
    );
    assert!(status.refs_total > 0, "the reference ledger is populated");
    assert_eq!(
        status.refs_total,
        status.refs_resolved + status.refs_unresolved
    );
    assert!((0.0..=1.0).contains(&status.resolution_coverage));
    assert!(
        status.freshness.contains("never reconciles"),
        "the ADR-11 contract is stated: {}",
        status.freshness
    );
}

#[test]
fn status_reports_an_unindexed_graph_without_building_one() {
    let tmp = fixture();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let status = engine.status();
    assert!(!status.indexed, "no index has run");
    assert_eq!((status.file_count, status.node_count), (0, 0));
    assert!(
        status.last_full_index_at.is_none(),
        "no index ran, no timestamp"
    );
    assert!(
        status.freshness.contains("unindexed"),
        "{}",
        status.freshness
    );

    // The health probe itself must not have indexed anything (it reports,
    // it never builds) — a second look still sees an empty graph.
    let again = engine.status();
    assert_eq!(again.node_count, 0, "status never triggers the auto-index");
}

// ── FR-NV-09 / UAT-NV-08: graceful unknown-symbol handling ───────────────────

#[test]
fn unknown_symbols_yield_empty_results_with_suggestions_never_errors() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let callers = engine.callers("zzz_definitely_missing", None);
    assert!(callers.resolved.is_none());
    assert!(callers.callers.is_empty() && callers.total == 0);
    assert!(callers.warnings.is_empty(), "graceful, not an error");

    // A near-miss earns a "did you mean" (FTS prefix on `alph`) — on `node`,
    // on `callers`/`callees`, and on `search` alike (FR-NV-09 is tool-wide).
    let info = engine.node("alph", false);
    assert!(info.node.is_none());
    assert!(
        info.suggestions.iter().any(|s| s == "alpha"),
        "did-you-mean suggests the real symbol: {:?}",
        info.suggestions
    );
    let near_callers = engine.callers("alph", None);
    assert!(near_callers.resolved.is_none());
    assert!(
        near_callers.suggestions.iter().any(|s| s == "alpha"),
        "callers near-miss suggests too: {:?}",
        near_callers.suggestions
    );
    let near_search = engine.search("alph", None, None);
    assert!(near_search.hits.is_empty());
    assert!(
        near_search.suggestions.iter().any(|s| s == "alpha"),
        "search near-miss suggests too: {:?}",
        near_search.suggestions
    );

    // Every traversal tool shares the contract.
    assert!(engine.impact("zzz_missing", None).resolved.is_none());
    assert!(engine.explore("zzz_missing", None).anchor.is_none());
    assert!(engine.context("zzz_missing", None, false).nodes.is_empty());
    let search = engine.search("zzz_missing", None, None);
    assert!(search.hits.is_empty() && search.warnings.is_empty());
}

/// The `EdgeDirection` wire form is the MCP-facing contract (S-017 consumes
/// it): lower-case `"in"`/`"out"`, locked here against serde-attr drift.
#[test]
fn edge_direction_serialises_to_lowercase_wire_names() {
    assert_eq!(serde_json::to_string(&EdgeDirection::In).unwrap(), "\"in\"");
    assert_eq!(
        serde_json::to_string(&EdgeDirection::Out).unwrap(),
        "\"out\""
    );
}

// ── FR-RC-05 / NFR-DM-02 / ADR-11: never reconcile, read-only ────────────────

#[test]
fn navigation_never_writes_and_never_reconciles_the_working_tree() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);
    let rt = engine.runtime().unwrap();

    // Edit the working tree WITHOUT syncing: a reconciling tool would see it.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use crate::util::run;

pub fn alpha() {
    run();
    beta();
}
pub fn beta() {
    gamma();
}
pub fn gamma() {}
pub fn delta_added_after_index() {}
",
    );

    let stamp_before = engine.sync_stamp();
    let counts_before = rt.submit_read(|s| s.counts()).unwrap();

    // Run all eight tools.
    let search = engine.search("delta_added_after_index", None, None);
    let _ = engine.context("alpha", None, false);
    let _ = engine.explore("alpha", None);
    let _ = engine.node("beta", false);
    let _ = engine.callers("gamma", None);
    let _ = engine.callees("alpha", None);
    let _ = engine.impact("beta", None);
    let status = engine.status();

    // Best-effort fresh: the un-synced edit is NOT visible — no per-call
    // reconcile happened (FR-RC-05); the status freshness line says so.
    assert!(
        search.hits.is_empty(),
        "an un-synced symbol must stay invisible to navigation: {search:?}"
    );
    assert!(status.freshness.contains("never reconciles"));

    // Read-only: the graph state and the sync clock are untouched.
    let counts_after = rt.submit_read(|s| s.counts()).unwrap();
    assert_eq!(counts_before, counts_after, "no write reached the store");
    assert_eq!(
        stamp_before,
        engine.sync_stamp(),
        "no navigation call advanced the sync clock"
    );

    // Background sync — not navigation — is the freshness mechanism: after an
    // explicit sync the new symbol is served.
    let synced = engine.sync(&[PathBuf::from("src/lib.rs")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    let found = engine.search("delta_added_after_index", None, None);
    assert!(
        found
            .hits
            .iter()
            .any(|h| h.name == "delta_added_after_index"),
        "sync (the background path) makes the edit visible: {found:?}"
    );
}

// ── ADR-14: the infallible surface degrades on a transient engine ────────────

#[test]
fn navigation_on_a_transient_engine_degrades_with_warnings_not_panics() {
    let engine = Engine::open("/tmp");

    assert!(!engine.search("x", None, None).warnings.is_empty());
    assert!(!engine.context("x", None, false).warnings.is_empty());
    assert!(!engine.explore("x", None).warnings.is_empty());
    assert!(!engine.node("x", false).warnings.is_empty());
    assert!(!engine.callers("x", None).warnings.is_empty());
    assert!(!engine.callees("x", None).warnings.is_empty());
    assert!(!engine.impact("x", None).warnings.is_empty());
    assert!(!engine.status().warnings.is_empty());
    // The S-016 closure query joins the same degradation contract.
    assert!(!engine
        .affected(&["x.rs".to_string()], false)
        .warnings
        .is_empty());
}

// ── Sprint Testing & Verification: the navigation dogfood ────────────────────

/// Copy every `.rs` file under `dir` (relative to `base`) into `dst_root`,
/// preserving structure — the established dogfood-fixture helper.
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

/// Index Logos's own `logos-core/src` tree with full resolution and run all
/// eight tools against the result — the sprint's dogfood verification. The
/// p95<100ms budget is *targeted* here (timings printed for the record) and
/// validated rigorously in S-024; asserting wall-clock in CI would flake.
#[test]
fn dogfood_runs_all_eight_tools_on_logos_own_source() {
    let tmp = TempDir::new().unwrap();
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    copy_rs_tree(&crate_root, &crate_root.join("src"), tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    // search: a real Logos symbol ranks first under its exact name.
    let t = std::time::Instant::now();
    let search = engine.search("build_view", Some(NodeKind::Function), None);
    let search_ms = t.elapsed().as_millis();
    assert!(
        search.hits.iter().any(|h| h.name == "build_view"),
        "{search:?}"
    );

    // context: one call on a task phrase yields a multi-file bundle.
    let t = std::time::Instant::now();
    let bundle = engine.context("hydrate graph view", None, false);
    let context_ms = t.elapsed().as_millis();
    assert!(!bundle.nodes.is_empty());
    assert!(
        bundle.est_reads_replaced >= 2,
        "the dogfood bundle replaces multiple reads: {:?}",
        bundle.files
    );

    // explore / node / callers / callees / impact on known hot symbols.
    // `raw_to_node` has several same-module callers in graph_store — the
    // binding class the scope-hierarchy resolver always resolves, so the
    // assertion stays stable regardless of cross-module resolution recall.
    let explore = engine.explore("build_view", None);
    assert!(explore.anchor.is_some());
    let node = engine.node("build_view", false);
    assert!(node.node.is_some());
    let t = std::time::Instant::now();
    let callers = engine.callers("raw_to_node", None);
    let callers_ms = t.elapsed().as_millis();
    assert!(
        !callers.callers.is_empty(),
        "raw_to_node has same-module callers: {callers:?}"
    );
    let callees = engine.callees("build_view", None);
    assert!(callees.total > 0, "build_view dispatches to builders");
    let impact = engine.impact("raw_to_node", None);
    assert!(!impact.upstream.is_empty(), "raw_to_node has dependents");

    // status: a populated, healthy index over the dogfood tree.
    let status = engine.status();
    assert!(status.indexed && status.node_count > 100);
    assert!(status.refs_total > 0);

    println!(
        "dogfood navigation: files={} nodes={} edges={} coverage={:.3} \
         | search={search_ms}ms context={context_ms}ms callers={callers_ms}ms \
         (p95<100ms targeted here, validated in S-024)",
        status.file_count, status.node_count, status.edge_count, status.resolution_coverage,
    );
}

// ── FR-CL-04 / UAT-CL-03 / DL-08: `affected` reverse-transitive closure ─────

/// A three-hop dependency chain plus a test-marked dependent, for closure
/// assertions: top.rs → mid.rs → core.rs, and core_test.rs → core.rs.
fn chain_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/core.rs", "pub fn base() {}\n");
    write(
        tmp.path(),
        "src/mid.rs",
        "use crate::core::base;\npub fn mid() {\n    base();\n}\n",
    );
    write(
        tmp.path(),
        "src/top.rs",
        "use crate::mid::mid;\npub fn top() {\n    mid();\n}\n",
    );
    write(
        tmp.path(),
        "src/core_test.rs",
        "use crate::core::base;\npub fn check_base() {\n    base();\n}\n",
    );
    tmp
}

#[test]
fn affected_returns_the_whole_reverse_transitive_closure_by_default() {
    let tmp = chain_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.affected(&["src/core.rs".to_string()], false);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.changed, ["src/core.rs"]);
    assert!(result.unknown.is_empty(), "{:?}", result.unknown);

    let files: Vec<&str> = result.affected.iter().map(|f| f.file.as_str()).collect();
    // Direct dependents (distance 1) and the transitive one (distance 2):
    // the closure is NOT depth-bounded (DL-08 — "whole closure by default").
    assert!(files.contains(&"src/mid.rs"), "{files:?}");
    assert!(files.contains(&"src/core_test.rs"), "{files:?}");
    assert!(
        files.contains(&"src/top.rs"),
        "transitive dependent: {files:?}"
    );
    // The changed file itself is echoed in `changed`, not listed as affected.
    assert!(!files.contains(&"src/core.rs"), "{files:?}");

    // Distances are minimal hops from the changed set, nearest-first.
    let dist = |name: &str| {
        result
            .affected
            .iter()
            .find(|f| f.file == name)
            .map(|f| f.distance)
            .unwrap()
    };
    assert_eq!(dist("src/mid.rs"), 1);
    assert_eq!(dist("src/top.rs"), 2);
    assert!(
        result
            .affected
            .windows(2)
            .all(|w| w[0].distance <= w[1].distance),
        "nearest-first ordering: {:?}",
        result.affected
    );
}

#[test]
fn affected_tests_only_narrows_to_test_marked_files() {
    let tmp = chain_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.affected(&["src/core.rs".to_string()], true);
    assert!(result.tests_only);
    let files: Vec<&str> = result.affected.iter().map(|f| f.file.as_str()).collect();
    assert_eq!(
        files,
        ["src/core_test.rs"],
        "only the test-marked dependent survives the filter"
    );
    assert!(result.affected.iter().all(|f| f.is_test));
}

#[test]
fn affected_deduplicates_repeated_input_files() {
    let tmp = chain_fixture();
    let engine = indexed_engine(&tmp);

    // The same file in raw and "./"-prefixed form collapses to ONE seed: one
    // `changed` entry, no duplicated dependents (NFR-RA-06 determinism).
    let result = engine.affected(
        &[
            "src/core.rs".to_string(),
            "src/core.rs".to_string(),
            "./src/core.rs".to_string(),
        ],
        false,
    );
    assert_eq!(result.changed, ["src/core.rs"], "duplicates collapse");
    assert!(result.unknown.is_empty(), "{:?}", result.unknown);
    for file in ["src/mid.rs", "src/top.rs", "src/core_test.rs"] {
        assert_eq!(
            result.affected.iter().filter(|f| f.file == file).count(),
            1,
            "{file} appears exactly once: {:?}",
            result.affected
        );
    }

    // The library-level empty changed set (unreachable from the binary —
    // clap requires ≥ 1 file) yields the all-empty result, never an error.
    let empty = engine.affected(&[], false);
    assert!(empty.changed.is_empty());
    assert!(empty.affected.is_empty());
    assert!(empty.unknown.is_empty());
    assert!(empty.warnings.is_empty());
}

#[test]
fn affected_reports_unknown_files_and_degrades_gracefully() {
    let tmp = chain_fixture();
    let engine = indexed_engine(&tmp);

    // A path not in the graph is reported, not an error (ADR-14 posture);
    // "./"-prefixed input normalises to the stored project-relative form.
    let result = engine.affected(
        &["./src/core.rs".to_string(), "no/such/file.rs".to_string()],
        false,
    );
    assert_eq!(result.unknown, ["no/such/file.rs"]);
    assert!(
        result.affected.iter().any(|f| f.file == "src/mid.rs"),
        "known seeds still produce their closure: {:?}",
        result.affected
    );

    // An entirely-unknown changed set yields an empty closure, never an error.
    let none = engine.affected(&["ghost.rs".to_string()], false);
    assert!(none.affected.is_empty());
    assert_eq!(none.unknown, ["ghost.rs"]);
}

// ── S-037 doc-aware navigation + traceability (FR-DG-05, FR-NV-01/02/04/10) ──

/// A fixture pairing a unique code symbol with documentation: `run` is named in
/// a doc body's inline code (so a `DocReference` binds, S-035); `idle` is code
/// no doc mentions; the doc bodies carry phrases that appear in NO heading or
/// symbol, so they are findable only once the body is FTS-indexed (FR-DG-05).
fn doc_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/util.rs",
        "pub fn run() {}\npub fn idle() {}\n",
    );
    write(
        tmp.path(),
        "docs/guide.md",
        "\
# Guide

## Running Tasks

The `run` function performs the quasiquibble step that no other text mentions.

### Inner Detail

This nested heading body alone mentions floopglorp.

## Lonely Notes

This section references no code and uniquely mentions zibblewump.
",
    );
    tmp
}

/// FR-DG-05 / FR-NV-01: a phrase living ONLY in a doc body is found by `search`,
/// and `kind=doc_section` narrows to documentation while a code-kind filter
/// excludes it.
#[test]
fn search_finds_a_phrase_only_in_a_doc_body_and_honours_kind() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    // The body-only phrase resolves to its DocSection (the heading does not
    // contain it, nor does any symbol name).
    let hits = engine.search("quasiquibble", None, None);
    assert!(hits.warnings.is_empty(), "{:?}", hits.warnings);
    assert!(
        hits.hits
            .iter()
            .any(|h| h.kind == NodeKind::DocSection && h.name == "Running Tasks"),
        "a body-only phrase finds its DocSection (FR-DG-05): {:?}",
        hits.hits
    );

    // A nested section's body is indexed under ITS OWN DocSection, not the
    // parent's — proof the body excludes sub-section prose.
    let nested = engine.search("floopglorp", None, None);
    assert!(
        nested
            .hits
            .iter()
            .any(|h| h.kind == NodeKind::DocSection && h.name == "Inner Detail"),
        "the nested body indexes under its own section: {:?}",
        nested.hits
    );

    // kind=doc_section keeps it; kind=function excludes documentation entirely.
    let docs_only = engine.search("quasiquibble", Some(NodeKind::DocSection), None);
    assert!(
        !docs_only.hits.is_empty()
            && docs_only
                .hits
                .iter()
                .all(|h| h.kind == NodeKind::DocSection),
        "kind=doc_section narrows to docs: {:?}",
        docs_only.hits
    );
    let funcs_only = engine.search("quasiquibble", Some(NodeKind::Function), None);
    assert!(
        funcs_only.hits.is_empty(),
        "a code-kind filter excludes the doc body match: {:?}",
        funcs_only.hits
    );
}

/// FR-NV-04: `node` works on a documentation node — it returns the section's
/// metadata and its immediate doc→code edge.
#[test]
fn node_resolves_a_documentation_node_with_its_doc_to_code_edge() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    let info = engine.node("Running Tasks", false);
    assert!(info.warnings.is_empty(), "{:?}", info.warnings);
    let detail = info.node.expect("the DocSection resolves");
    assert_eq!(detail.symbol.kind, NodeKind::DocSection);
    assert!(
        detail
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::DocReference && e.other.name == "run"),
        "the doc node carries its doc→code reference: {:?}",
        detail.edges
    );
}

/// FR-NV-02: `context` on a code symbol can pull in the documentation that
/// explains it — the doc→code edge is a 1-hop expansion neighbour.
#[test]
fn context_can_include_the_docs_explaining_seeded_code() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    let bundle = engine.context("run", None, false);
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);
    assert!(
        bundle
            .nodes
            .iter()
            .any(|n| n.symbol.kind == NodeKind::DocSection && n.symbol.name == "Running Tasks"),
        "the documenting section is in the bundle: {:?}",
        bundle.nodes
    );

    // Determinism with docs in the bundle (NFR-RA-06): the doc-aware expansion
    // path (a store query feeding the same rank/truncate) stays byte-identical
    // across identical calls — guards against a non-deterministic doc collection.
    let again = engine.context("run", None, false);
    assert_eq!(
        serde_json::to_string(&bundle).unwrap(),
        serde_json::to_string(&again).unwrap(),
        "the doc-aware bundle is identical across two identical calls"
    );

    // Negative path: a symbol no doc references pulls in no DocSection — the
    // bundle stays clean (no spurious doc nodes, no warnings).
    let idle = engine.context("idle", None, false);
    assert!(idle.warnings.is_empty(), "{:?}", idle.warnings);
    assert!(
        idle.nodes
            .iter()
            .all(|n| n.symbol.kind != NodeKind::DocSection),
        "no doc references idle → no DocSection in its context: {:?}",
        idle.nodes
    );
}

/// A docs-rich fixture whose realistic prose lives ONLY in documentation bodies,
/// never in a code symbol name (S-078, CR-014). `settle_invoice` is the lone code
/// symbol; the spec's "Billing Reconciliation" section documents it (an inline
/// `settle_invoice` binds a `DocReference`, S-035) and its body — plus two sibling
/// sections — carries the multi-word prose a user would actually type as a task.
/// No task token ("nightly", "billing", "reconciliation", "workflow") matches the
/// `settle_invoice` symbol, so the code anchor is reachable ONLY by anchoring a
/// doc node and expanding doc→code.
fn prose_docs_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/billing.rs",
        "pub fn settle_invoice() {}\npub fn idle() {}\n",
    );
    write(
        tmp.path(),
        "docs/spec.md",
        "\
# Billing Spec

## Billing Reconciliation

The `settle_invoice` step runs the nightly billing reconciliation workflow.

## Ledger Notes

This section describes the nightly billing reconciliation workflow ledger.

## Audit Trail

The nightly billing reconciliation workflow records an audit trail entry.
",
    );
    tmp
}

/// FR-NV-02 / CR-014 regression: a realistic multi-word PROSE task whose tokens
/// live only in documentation bodies must still anchor a non-empty, deterministic
/// bundle. Before S-078 the FTS seeds were all doc nodes, none resolved against
/// the code-only hydrated view, the candidate set emptied, and `context` returned
/// an EMPTY bundle (the CR-014 empty-bundle defect). The fix anchors the doc seed
/// directly and expands doc→code to the implementing symbol.
#[test]
fn context_prose_task_anchors_a_bundle_via_documentation() {
    let tmp = prose_docs_fixture();
    let engine = indexed_engine(&tmp);

    let bundle = engine.context("nightly billing reconciliation workflow", None, false);
    assert!(bundle.warnings.is_empty(), "{:?}", bundle.warnings);

    // The defect: this exact call returned an empty bundle before S-078.
    assert!(
        !bundle.nodes.is_empty(),
        "a prose task matching only doc bodies still seeds a bundle (CR-014): {:?}",
        bundle.nodes
    );

    // The matching documentation node anchors the bundle as a seed — the prose
    // tokens hit its heading/body, and a doc node is no longer silently dropped.
    assert!(
        bundle.nodes.iter().any(|n| n.symbol.kind == NodeKind::DocSection
            && n.symbol.name == "Billing Reconciliation"
            && n.seed),
        "the documentation node anchors the bundle as a seed: {:?}",
        bundle.nodes
    );

    // Doc→code expansion: the anchored section reaches the code it documents, so
    // the implementing symbol joins the bundle even though no task token names it.
    assert!(
        bundle
            .nodes
            .iter()
            .any(|n| n.symbol.name == "settle_invoice"),
        "the doc anchor expands along doc→code to its implementing symbol: {:?}",
        bundle.nodes
    );

    // The acceptance-criteria invariants still hold: bounded, code opt-in OFF.
    let capped = engine.context("nightly billing reconciliation workflow", Some(3), false);
    assert!(capped.nodes.len() <= 3, "max_nodes bounds the bundle");
    assert!(
        bundle.nodes.iter().all(|n| n.code.is_none()),
        "code stays opt-in for a prose task: {:?}",
        bundle.nodes
    );

    // Determinism (NFR-RA-06): the doc-anchored bundle is byte-identical across
    // two identical calls — the kind-balanced seed window and the doc→code
    // expansion both order deterministically.
    let again = engine.context("nightly billing reconciliation workflow", None, false);
    assert_eq!(
        serde_json::to_string(&bundle).unwrap(),
        serde_json::to_string(&again).unwrap(),
        "the prose-anchored bundle is identical across two identical calls"
    );
}

/// FR-NV-10: "which code implements `<doc>`" lists the linked code symbols, and
/// returns an empty (non-error) result when the doc node links no code.
#[test]
fn implements_lists_code_for_a_doc_and_is_empty_when_unlinked() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    let impls = engine.implements("Running Tasks");
    assert!(impls.warnings.is_empty(), "{:?}", impls.warnings);
    assert!(impls.resolved.is_some(), "the doc node resolves");
    assert_eq!(
        impls
            .implementors
            .iter()
            .map(|l| l.symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["run"],
        "the doc→code edge yields the implementing symbol: {:?}",
        impls.implementors
    );
    assert!(impls
        .implementors
        .iter()
        .all(|l| l.via == EdgeKind::DocReference));

    // A doc section that references no code → empty, not an error (FR-NV-10).
    let lonely = engine.implements("Lonely Notes");
    assert!(lonely.resolved.is_some(), "the section still resolves");
    assert!(
        lonely.implementors.is_empty() && lonely.warnings.is_empty(),
        "no code edge → empty result, never an error: {:?}",
        lonely
    );

    // An unknown doc node → empty result plus suggestions, never an error.
    let unknown = engine.implements("No Such Heading At All");
    assert!(unknown.resolved.is_none());
    assert!(unknown.implementors.is_empty() && unknown.warnings.is_empty());
}

/// FR-NV-10: "which docs reference `<symbol>`" lists the linking sections, and
/// returns an empty (non-error) result for a symbol no doc references.
#[test]
fn referencing_docs_lists_docs_for_a_symbol_and_is_empty_when_undocumented() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    let docs = engine.referencing_docs("run");
    assert!(docs.warnings.is_empty(), "{:?}", docs.warnings);
    assert!(docs.resolved.is_some(), "the code symbol resolves");
    assert_eq!(
        docs.docs
            .iter()
            .map(|l| l.symbol.name.as_str())
            .collect::<Vec<_>>(),
        ["Running Tasks"],
        "the inbound doc→code edge yields the documenting section: {:?}",
        docs.docs
    );
    assert!(docs
        .docs
        .iter()
        .all(|l| l.symbol.kind == NodeKind::DocSection));

    // A code symbol no doc mentions → empty, not an error (FR-NV-10).
    let undocumented = engine.referencing_docs("idle");
    assert!(undocumented.resolved.is_some(), "idle still resolves");
    assert!(
        undocumented.docs.is_empty() && undocumented.warnings.is_empty(),
        "no doc references idle → empty result, never an error: {:?}",
        undocumented
    );

    // An unknown symbol → empty result, never an error.
    let unknown = engine.referencing_docs("nonexistent_symbol_zzz");
    assert!(unknown.resolved.is_none());
    assert!(unknown.docs.is_empty() && unknown.warnings.is_empty());
}

/// FR-NV-10: `impact` is doc-aware — it reports the docs referencing the
/// queried symbol alongside the code directions.
#[test]
fn impact_is_doc_aware_and_lists_referencing_docs() {
    let tmp = doc_fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.impact("run", None);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.docs_label, "documented by");
    assert!(
        result
            .docs
            .iter()
            .any(|l| l.symbol.name == "Running Tasks" && l.symbol.kind == NodeKind::DocSection),
        "impact surfaces the documenting section (FR-NV-10): {:?}",
        result.docs
    );

    // A symbol no doc references carries an empty docs dimension, not an error.
    let idle = engine.impact("idle", None);
    assert!(idle.docs.is_empty() && idle.warnings.is_empty());
}
