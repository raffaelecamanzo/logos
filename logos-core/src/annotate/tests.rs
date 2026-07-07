//! Behaviour tests for the annotation engine (S-014), organised by acceptance
//! criterion. Language-independent: the graph is seeded directly through the
//! writer actor (no tree-sitter), so these exercise exactly the Pass-3
//! contract — dead-code ([UAT-AN-01]), duplicates ([UAT-AN-02]), and layer
//! policy materialisation ([UAT-AN-03]).
//!
//! [UAT-AN-01]: ../../../docs/specs/requirements/UAT-AN-01.md
//! [UAT-AN-02]: ../../../docs/specs/requirements/UAT-AN-02.md
//! [UAT-AN-03]: ../../../docs/specs/requirements/UAT-AN-03.md

use tempfile::TempDir;

use super::*;
use crate::config::{Boundary, ForbiddenImport, Layer, Rules};

/// A live runtime over a fresh on-disk store.
fn runtime() -> (Runtime, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let runtime = Runtime::open(dir.path().join("logos.db")).expect("runtime opens");
    (runtime, dir)
}

/// A live runtime whose shared worker pool has exactly `workers` threads — the
/// 1→N harness for the S-229 byte-identical-across-worker-counts equivalence and
/// stress tests (the parallel annotation compute runs on this pool, [NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn runtime_with_workers(workers: usize) -> (Runtime, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let config = crate::runtime::RuntimeConfig {
        worker_threads: workers,
        ..crate::runtime::RuntimeConfig::default()
    };
    let runtime =
        Runtime::open_with_config(dir.path().join("logos.db"), config).expect("runtime opens");
    (runtime, dir)
}

/// Seed one node with the given annotation inputs, returning its id. `n` keys
/// a distinct local symbol.
///
/// When the caller does not pin a `file_id`, the node is defaulted into a shared
/// Rust-pathed file (`src/seed.rs`) so the S-159 reachability-capability gate
/// computes a dead-code verdict (Rust declares the capability — see [`reach`]).
/// A caller that wants a non-capable language (or a specific path) passes an
/// explicit `file_id` from [`seed_file`]/[`seed_lang_file`].
fn seed_node(
    runtime: &Runtime,
    n: u32,
    name: &str,
    kind: NodeKind,
    exported: bool,
    fingerprint: Option<&str>,
    file_id: Option<i64>,
) -> NodeId {
    let name = name.to_string();
    let fingerprint = fingerprint.map(str::to_string);
    runtime
        .submit_write(move |w| {
            let sym = LogosSymbol::parse(&format!("local n{n}"))?;
            let symbol_id = w.upsert_symbol(&sym)?;
            // Default a file-less node into a Rust file so it is reachability-
            // capable (S-159); idempotent — reused across every defaulted node.
            let file_id = match file_id {
                Some(id) => Some(id),
                None => Some(
                    w.file_id("src/seed.rs")?
                        .map_or_else(|| w.insert_file("src/seed.rs", Some("rust"), None), Ok)?,
                ),
            };
            w.insert_node(&NewNode {
                exported,
                fingerprint: fingerprint.as_deref(),
                file_id,
                ..NewNode::plain(symbol_id, kind, &name)
            })
        })
        .expect("seed node commits")
}

/// Seed a file row, returning its id.
fn seed_file(runtime: &Runtime, path: &str) -> i64 {
    let path = path.to_string();
    runtime
        .submit_write(move |w| {
            w.file_id(&path)?
                .map_or_else(|| w.insert_file(&path, Some("rust"), None), Ok)
        })
        .expect("seed file commits")
}

/// Seed an extracted (non-derived) edge.
fn seed_edge(runtime: &Runtime, source: NodeId, target: NodeId, kind: EdgeKind) {
    runtime
        .submit_write(move |w| w.insert_edge(source, target, kind))
        .expect("seed edge commits");
}

/// Persist a function's winnowed shingle set ([FR-EX-09]) into the inverted
/// `shingles` index — the near-clone clustering input ([FR-AN-06]).
fn seed_shingles(runtime: &Runtime, id: NodeId, hashes: &[u64]) {
    let hashes = hashes.to_vec();
    runtime
        .submit_write(move |w| w.insert_shingles(id, &hashes))
        .expect("seed shingles commits");
}

/// A shingle set of `n` distinct hashes from `base` — a body comfortably above
/// the near-clone eligibility floor.
fn shingle_set(base: u64, n: u64) -> Vec<u64> {
    (base..base + n).collect()
}

/// The annotation snapshot, as the consumers (metrics, governance) read it.
fn snapshot(runtime: &Runtime) -> Vec<AnnotationNodeRow> {
    runtime
        .submit_read(|store| store.annotation_nodes())
        .expect("snapshot reads")
}

/// The row for `id`, panicking when absent.
fn row(snapshot: &[AnnotationNodeRow], id: NodeId) -> &AnnotationNodeRow {
    snapshot
        .iter()
        .find(|n| n.id == id)
        .expect("node present in snapshot")
}

/// The default entry-point list (`["main"]`).
fn entries() -> Vec<String> {
    vec!["main".to_string()]
}

/// The default `[semantics].test_markers` list (`["test", "tests", "spec"]`).
fn markers() -> Vec<String> {
    ["test", "tests", "spec"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// The reachability-capable extension set (S-159, [CR-043], [ADR-39]) the
/// production registry would build with only Rust declaring the capability —
/// `{"rs"}`. The annotation pass computes a dead-code verdict only for callables
/// whose file extension is in this set; everything else renders `is_dead = NULL`.
///
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
fn reach() -> std::collections::HashSet<String> {
    std::iter::once("rs".to_string()).collect()
}

/// Seed a file row carrying an explicit language + path, returning its id — the
/// hook for a non-Rust (reachability-incapable) fixture, e.g. a `.js` file.
fn seed_lang_file(runtime: &Runtime, path: &str, language: &str) -> i64 {
    let path = path.to_string();
    let language = language.to_string();
    runtime
        .submit_write(move |w| {
            w.file_id(&path)?
                .map_or_else(|| w.insert_file(&path, Some(&language), None), Ok)
        })
        .expect("seed lang file commits")
}

/// Every node's persisted verdict tuple, id-ordered — the comparison key for the
/// incremental-commit equivalence tests (S-024-HF).
#[allow(clippy::type_complexity)]
fn verdicts(
    runtime: &Runtime,
) -> Vec<(NodeId, Option<bool>, Option<bool>, bool, Option<String>, Option<NodeId>)> {
    let mut rows: Vec<_> = snapshot(runtime)
        .into_iter()
        .map(|n| {
            (
                n.id,
                n.is_dead,
                n.is_duplicate,
                n.is_test,
                n.layer_membership,
                n.clone_group,
            )
        })
        .collect();
    rows.sort_by_key(|t| t.0);
    rows
}

// ── S-024-HF: the incremental commit writes only changed verdicts ────────────

#[test]
fn incremental_annotate_is_byte_identical_to_a_full_re_annotate() {
    // A graph with a verdict spread: an exported (live) root, a dead helper, and
    // a duplicate pair.
    let (rt, _dir) = runtime();
    let root = seed_node(&rt, 0, "main", NodeKind::Function, true, None, None);
    let helper = seed_node(&rt, 1, "helper", NodeKind::Function, false, None, None);
    let dup_a = seed_node(&rt, 2, "dup_a", NodeKind::Function, false, Some("fp"), None);
    let dup_b = seed_node(&rt, 3, "dup_b", NodeKind::Function, false, Some("fp"), None);
    seed_edge(&rt, root, dup_a, EdgeKind::Calls);

    // A full annotate establishes the baseline.
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let full = verdicts(&rt);
    // The baseline must actually carry a spread of verdicts to preserve.
    assert_eq!(row(&snapshot(&rt), helper).is_dead, Some(true));
    assert_eq!(row(&snapshot(&rt), root).is_dead, Some(false));
    assert_eq!(row(&snapshot(&rt), dup_a).is_duplicate, Some(true));
    assert_eq!(row(&snapshot(&rt), dup_b).is_duplicate, Some(true));

    // An incremental re-annotate over the unchanged graph leaves every stored
    // verdict identical — the diff-write skips the no-op writes (S-024-HF).
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), true).unwrap();
    assert_eq!(
        full,
        verdicts(&rt),
        "an incremental re-annotate is byte-identical to a full one"
    );
}

#[test]
fn incremental_annotate_persists_a_cross_node_verdict_flip() {
    // The hard case the whole-graph compute must still catch: a change makes an
    // UNTOUCHED node's verdict flip, and the incremental commit must persist it
    // (the diff is verdict-value-based, not file-based).
    let (rt, _dir) = runtime();
    let root = seed_node(&rt, 0, "main", NodeKind::Function, true, None, None);
    let helper = seed_node(&rt, 1, "helper", NodeKind::Function, false, None, None);

    // Baseline: helper is unreferenced ⇒ dead.
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), true).unwrap();
    assert_eq!(row(&snapshot(&rt), helper).is_dead, Some(true));

    // Add a call from the live root to helper — helper is now reachable. An
    // incremental annotate must recompute whole-graph and write helper's flip to
    // live even though helper's own inputs did not change.
    seed_edge(&rt, root, helper, EdgeKind::Calls);
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), true).unwrap();
    assert_eq!(
        row(&snapshot(&rt), helper).is_dead,
        Some(false),
        "a cross-node reachability flip is persisted by the incremental commit"
    );
}

/// Seed a node carrying extraction-time test-marker evidence ([FR-EX-06]) —
/// the persisted Pass-1 input to the unified `is_test` annotation.
fn seed_test_evidence_node(runtime: &Runtime, n: u32, name: &str, file_id: Option<i64>) -> NodeId {
    let name = name.to_string();
    runtime
        .submit_write(move |w| {
            let sym = LogosSymbol::parse(&format!("local n{n}"))?;
            let symbol_id = w.upsert_symbol(&sym)?;
            w.insert_node(&NewNode {
                file_id,
                test_evidence: true,
                ..NewNode::plain(symbol_id, NodeKind::Function, &name)
            })
        })
        .expect("seed node commits")
}

/// A two-layer `rules.toml` read-model with one forbidden boundary
/// (`presentation` may not be depended on by `domain`… i.e. `domain -> presentation`
/// is forbidden).
fn layered_rules() -> Rules {
    Rules {
        constraints: Default::default(),
        metric_thresholds: Default::default(),
        layers: vec![
            Layer {
                name: "domain".to_string(),
                paths: vec!["src/domain/**".to_string()],
                order: 1,
            },
            Layer {
                name: "presentation".to_string(),
                paths: vec!["src/ui/**".to_string()],
                order: 2,
            },
        ],
        boundaries: vec![Boundary {
            from: "domain".to_string(),
            to: "presentation".to_string(),
            reason: Some("the domain must not reach upward".to_string()),
        }],
        forbidden_imports: Vec::new(),
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    }
}

/// A `rules.toml` read-model carrying a single `[[forbidden_imports]]` ban
/// (`src/web/** -> src/db/**`) and nothing else ([FR-GV-12]).
fn forbidden_import_rules() -> Rules {
    Rules {
        constraints: Default::default(),
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: vec![ForbiddenImport {
            from: "src/web/**".to_string(),
            to: "src/db/**".to_string(),
            reason: Some("the web layer must not import the db".to_string()),
        }],
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    }
}

// ── FR-AN-01 / UAT-AN-01: dead-code with exported-is-live ────────────────────

#[test]
fn unreferenced_private_function_is_dead_and_exported_one_is_live() {
    let (rt, _dir) = runtime();
    // The UAT-AN-01 fixture: (a) an unreferenced private fn, (b) an
    // unreferenced exported fn.
    let private = seed_node(
        &rt,
        1,
        "internal_helper",
        NodeKind::Function,
        false,
        None,
        None,
    );
    let public = seed_node(&rt, 2, "open_api", NodeKind::Function, true, None, None);

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, private).is_dead,
        Some(true),
        "an unreferenced private fn is dead (FR-AN-01)"
    );
    assert_eq!(
        row(&snap, public).is_dead,
        Some(false),
        "an exported fn is live even with no call-site (exported-is-live)"
    );
    assert_eq!(stats.dead, 1);
    assert_eq!(stats.nodes_annotated, 2);
}

/// FR-DG-06: documentation is outside the dead-code scope. A `DocSection` never
/// receives a dead-code verdict (it is not callable — tri-state honesty,
/// NFR-CC-04), and a doc→code `DocReference` confers no liveness, so an
/// unreferenced private function a doc happens to mention is still dead. Together
/// these prove a documentation node can neither appear in the dead-code report
/// nor perturb which code is reported dead.
#[test]
fn documentation_is_outside_the_dead_code_scope() {
    let (rt, _dir) = runtime();
    // An unreferenced private function — dead on its own merits.
    let dead = seed_node(
        &rt,
        1,
        "unused_helper",
        NodeKind::Function,
        false,
        None,
        None,
    );
    // A doc section that references the function (a resolved doc→code edge).
    let doc = seed_node(&rt, 2, "Usage", NodeKind::DocSection, false, None, None);
    seed_edge(&rt, doc, dead, EdgeKind::DocReference);

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, doc).is_dead,
        None,
        "a DocSection is not callable — it never gets a dead-code verdict (FR-DG-06)"
    );
    assert_eq!(
        row(&snap, dead).is_dead,
        Some(true),
        "a doc→code reference confers no liveness; the function stays dead (FR-DG-06)"
    );
    assert_eq!(stats.dead, 1, "exactly the one code function is dead");
}

#[test]
fn liveness_propagates_transitively_over_calls() {
    let (rt, _dir) = runtime();
    // exported `api` → `step` → `core`; `orphan` hangs off nothing.
    let api = seed_node(&rt, 1, "api", NodeKind::Function, true, None, None);
    let step = seed_node(&rt, 2, "step", NodeKind::Function, false, None, None);
    let core = seed_node(&rt, 3, "core", NodeKind::Function, false, None, None);
    let orphan = seed_node(&rt, 4, "orphan", NodeKind::Function, false, None, None);
    seed_edge(&rt, api, step, EdgeKind::Calls);
    seed_edge(&rt, step, core, EdgeKind::Calls);

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(row(&snap, step).is_dead, Some(false), "called from the API");
    assert_eq!(row(&snap, core).is_dead, Some(false), "transitively live");
    assert_eq!(row(&snap, orphan).is_dead, Some(true), "unreachable");
}

#[test]
fn entry_points_root_the_reachability_walk() {
    let (rt, _dir) = runtime();
    // `main` is private (a binary root, not an export); its callee must live.
    let main = seed_node(&rt, 1, "main", NodeKind::Function, false, None, None);
    let callee = seed_node(&rt, 2, "do_work", NodeKind::Function, false, None, None);
    seed_edge(&rt, main, callee, EdgeKind::Calls);

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(row(&snap, main).is_dead, Some(false), "entry point is live");
    assert_eq!(row(&snap, callee).is_dead, Some(false), "called from main");
}

#[test]
fn route_nodes_root_their_handlers() {
    let (rt, _dir) = runtime();
    // A framework route dispatching to a private handler (S-012 promotion).
    let route = seed_node(&rt, 1, "GET /users", NodeKind::Route, false, None, None);
    let handler = seed_node(&rt, 2, "list_users", NodeKind::Function, false, None, None);
    seed_edge(&rt, route, handler, EdgeKind::RoutesTo);

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, handler).is_dead,
        Some(false),
        "a route's handler is live (route entry points, FR-AN-01)"
    );
}

#[test]
fn non_callable_kinds_keep_the_null_verdicts() {
    let (rt, _dir) = runtime();
    let strukt = seed_node(&rt, 1, "Widget", NodeKind::Struct, false, None, None);

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    let s = row(&snap, strukt);
    assert_eq!(
        s.is_dead, None,
        "dead-code verdicts apply to callables only — NULL, not a fake false"
    );
    assert_eq!(
        s.is_duplicate, None,
        "no fingerprint → no duplicate verdict"
    );
}

// ── S-159 / CR-043 / ADR-39: capability-gated reachability ───────────────────

/// A callable whose language does **not** declare the reachability capability
/// renders `is_dead = NULL` ("not computed", NFR-CC-04) rather than a fabricated
/// `true`, and is excluded from the `max_dead` count — while a Rust callable in
/// the same graph still earns an honest verdict (FR-AN-01, CR-043, ADR-39).
#[test]
fn non_capable_language_callable_renders_null_not_dead() {
    let (rt, _dir) = runtime();
    // A Rust file (reachability-capable) and a JavaScript file (not capable).
    let rs_file = seed_lang_file(&rt, "src/lib.rs", "rust");
    let js_file = seed_lang_file(&rt, "src/app.js", "javascript");
    // Both are unreferenced private callables — by reachability alone each would
    // be is_dead = true. Only Rust's language declares the capability.
    let rust_dead = seed_node(
        &rt,
        1,
        "rust_helper",
        NodeKind::Function,
        false,
        None,
        Some(rs_file),
    );
    let js_unbindable = seed_node(
        &rt,
        2,
        "jsHandler",
        NodeKind::Function,
        false,
        None,
        Some(js_file),
    );

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, rust_dead).is_dead,
        Some(true),
        "a Rust callable is reachability-capable — it still gets an honest verdict"
    );
    assert_eq!(
        row(&snap, js_unbindable).is_dead,
        None,
        "a JS callable renders NULL — never a fabricated dead (CR-043 / ADR-39)"
    );
    assert_eq!(
        stats.dead, 1,
        "only the Rust dead fn counts — the NULL JS node is excluded from max_dead"
    );
    assert_eq!(stats.nodes_annotated, 2);
}

/// The degraded state when **no** loaded grammar declares the capability (an
/// empty capable set): the pass still annotates normally (NFR-MA-01 — a
/// capability-less plugin indexes fine) and every callable renders NULL rather
/// than a fabricated verdict, so nothing is counted toward `max_dead`.
#[test]
fn an_empty_capability_set_renders_every_callable_null() {
    let (rt, _dir) = runtime();
    let private = seed_node(
        &rt,
        1,
        "internal_helper",
        NodeKind::Function,
        false,
        None,
        None,
    );
    let public = seed_node(&rt, 2, "open_api", NodeKind::Function, true, None, None);

    let empty = std::collections::HashSet::new();
    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &empty, false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, private).is_dead,
        None,
        "no capability → NULL, never a fabricated dead"
    );
    assert_eq!(
        row(&snap, public).is_dead,
        None,
        "no capability → NULL even for an exported callable"
    );
    assert_eq!(stats.dead, 0, "nothing counts toward max_dead");
    assert_eq!(
        stats.nodes_annotated, 2,
        "the pass still annotates every node (NFR-MA-01)"
    );
}

/// NFR-RA-06: with the capability gate in play, a clean re-run is byte-identical
/// — the Rust verdict and the JS NULL both reproduce exactly across runs.
#[test]
fn capability_gated_dead_code_is_byte_identical_across_runs() {
    let (rt, _dir) = runtime();
    let js_file = seed_lang_file(&rt, "src/app.js", "javascript");
    let rust_dead = seed_node(&rt, 1, "rust_helper", NodeKind::Function, false, None, None);
    let js_node = seed_node(
        &rt,
        2,
        "jsHandler",
        NodeKind::Function,
        false,
        None,
        Some(js_file),
    );

    let first = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let v1 = verdicts(&rt);
    let second = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let v2 = verdicts(&rt);

    assert_eq!(v1, v2, "a clean re-run is byte-identical (NFR-RA-06)");
    assert_eq!(first.dead, second.dead);
    assert_eq!(
        row(&snapshot(&rt), js_node).is_dead,
        None,
        "the JS callable stays NULL on every run"
    );
    assert_eq!(
        row(&snapshot(&rt), rust_dead).is_dead,
        Some(true),
        "the Rust callable stays dead on every run"
    );
}

/// The gate keys on the file extension, so a callable with **no file path** —
/// or a file with **no extension** — is "not declared" and renders NULL, never
/// a fabricated verdict ([CR-043], [ADR-39], [NFR-CC-04]). This pins the two
/// otherwise-untested branches of `reachability_capable` directly: the file-less
/// node is seeded raw, deliberately bypassing `seed_node`'s capable-by-default
/// Rust file.
#[test]
fn callable_with_no_or_extensionless_path_renders_null() {
    let (rt, _dir) = runtime();
    // (a) a callable with NO file at all — seeded raw so it keeps `file_id =
    //     None` (the helper would otherwise default it into a Rust file).
    let no_file = rt
        .submit_write(|w| {
            let sym = LogosSymbol::parse("local nofile")?;
            let symbol_id = w.upsert_symbol(&sym)?;
            w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, "no_file_fn"))
        })
        .expect("seed commits");
    // (b) a callable in an extension-less file (a `Makefile`-style path).
    let noext_file = seed_lang_file(&rt, "scripts/run", "shell");
    let noext = seed_node(&rt, 1, "ext_less", NodeKind::Function, false, None, Some(noext_file));

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, no_file).is_dead,
        None,
        "a callable with no file path is 'not declared' → NULL"
    );
    assert_eq!(
        row(&snap, noext).is_dead,
        None,
        "a callable in an extension-less file is 'not declared' → NULL"
    );
    assert_eq!(stats.dead, 0, "neither NULL callable counts toward max_dead");
}

/// A capable-language callable that is **live** renders `Some(false)`, not NULL —
/// the gate must not collapse capable verdicts to NULL. Paired with a
/// non-capable callable rendering NULL in the same run, this pins that the gate
/// discriminates on capability *and* on liveness, not capability alone
/// ([FR-AN-01], [CR-043], [ADR-39]).
#[test]
fn capable_live_callable_is_some_false_beside_a_null_non_capable() {
    let (rt, _dir) = runtime();
    let js_file = seed_lang_file(&rt, "src/app.js", "javascript");
    // An exported Rust callable — live by exported-is-live.
    let rust_live = seed_node(&rt, 1, "public_api", NodeKind::Function, true, None, None);
    let js_node = seed_node(
        &rt,
        2,
        "jsHandler",
        NodeKind::Function,
        false,
        None,
        Some(js_file),
    );

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, rust_live).is_dead,
        Some(false),
        "a live capable callable is Some(false), never collapsed to NULL"
    );
    assert_eq!(
        row(&snap, js_node).is_dead,
        None,
        "the non-capable callable is NULL in the same run"
    );
    assert_eq!(stats.dead, 0, "the live Rust fn and the NULL JS fn both count 0");
}

// ── FR-AN-02 / UAT-AN-02: duplicates by AST-shape fingerprint ────────────────

#[test]
fn matching_fingerprints_flag_both_duplicates_and_spare_the_distinct_one() {
    let (rt, _dir) = runtime();
    // The UAT-AN-02 fixture: two structurally identical fns (same shape
    // fingerprint, different names) + one distinct fn.
    let twin_a = seed_node(
        &rt,
        1,
        "first",
        NodeKind::Function,
        false,
        Some("fp-same"),
        None,
    );
    let twin_b = seed_node(
        &rt,
        2,
        "second",
        NodeKind::Function,
        false,
        Some("fp-same"),
        None,
    );
    let other = seed_node(
        &rt,
        3,
        "different",
        NodeKind::Function,
        false,
        Some("fp-other"),
        None,
    );

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(row(&snap, twin_a).is_duplicate, Some(true));
    assert_eq!(row(&snap, twin_b).is_duplicate, Some(true));
    assert_eq!(
        row(&snap, other).is_duplicate,
        Some(false),
        "a structurally distinct fn is not flagged (FR-AN-02)"
    );
    assert_eq!(stats.duplicates, 2);
}

// ── FR-AN-06 / UAT-QM-12: near-clone clustering into persisted groups ────────

#[test]
fn near_clones_group_and_an_unrelated_function_does_not() {
    let (rt, _dir) = runtime();
    // The UAT-QM-12 fixture: two near clones (renamed identifiers + one edited
    // line ≈ a near-identical shingle set with a one-shingle perturbation each)
    // and one unrelated function (a disjoint set).
    let clone_a = seed_node(&rt, 1, "compute", NodeKind::Function, false, None, None);
    let clone_b = seed_node(&rt, 2, "tally", NodeKind::Function, false, None, None);
    let unrelated = seed_node(&rt, 3, "greet", NodeKind::Function, false, None, None);

    let mut a = shingle_set(100, 20);
    a.push(500); // the edited line in `compute`
    let mut b = shingle_set(100, 20);
    b.push(501); // the edited line in `tally` — 20 shared, Jaccard ≈ 0.91 ≥ 0.85
    seed_shingles(&rt, clone_a, &a);
    seed_shingles(&rt, clone_b, &b);
    seed_shingles(&rt, unrelated, &shingle_set(900, 20)); // disjoint

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    let group = row(&snap, clone_a).clone_group;
    assert_eq!(
        group,
        Some(clone_a),
        "the group id is the minimum member node id (FR-AN-06)"
    );
    assert_eq!(
        row(&snap, clone_b).clone_group,
        Some(clone_a),
        "both near clones share one group (UAT-QM-12)"
    );
    assert_eq!(
        row(&snap, unrelated).clone_group,
        None,
        "an unrelated function is in no near-clone group (UAT-QM-12)"
    );
    assert_eq!(stats.clones, 2, "two functions are cloned");
    assert_eq!(stats.clone_groups, 1, "one near-clone group formed");
}

/// CR-013 / FR-AN-06: the annotation pass clusters under the *configured*
/// near-clone parameters, not the module defaults — the end-to-end check that
/// `run` threads `MetricThresholds::effective()` into `clone::cluster`. Two
/// identical short bodies sit below the default token floor (so they do not
/// group with `Rules::default()`); lowering `clone_min_tokens` makes them
/// eligible and they group — a regression in the config→cluster wiring would
/// flip exactly this assertion.
#[test]
fn annotation_pass_uses_the_configured_clone_thresholds() {
    use crate::config::MetricThresholds;

    // Five identical shingles — below the default eligibility floor of 11.
    let short = shingle_set(100, 5);

    // Default config: the short pair is too small to be clone-eligible.
    let (rt_default, _d1) = runtime();
    let a0 = seed_node(&rt_default, 1, "compute", NodeKind::Function, false, None, None);
    let b0 = seed_node(&rt_default, 2, "tally", NodeKind::Function, false, None, None);
    seed_shingles(&rt_default, a0, &short);
    seed_shingles(&rt_default, b0, &short);
    let default_stats = run(&rt_default, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    assert_eq!(
        default_stats.clone_groups, 0,
        "5 shingles < the default floor of 11 → no group under Rules::default()"
    );
    assert_eq!(row(&snapshot(&rt_default), a0).clone_group, None);

    // A lowered `clone_min_tokens` (→ a 1-shingle floor) makes the same pair
    // eligible — proving the configured value reached the clustering pass.
    let (rt_tuned, _d2) = runtime();
    let a1 = seed_node(&rt_tuned, 1, "compute", NodeKind::Function, false, None, None);
    let b1 = seed_node(&rt_tuned, 2, "tally", NodeKind::Function, false, None, None);
    seed_shingles(&rt_tuned, a1, &short);
    seed_shingles(&rt_tuned, b1, &short);
    let tuned_rules = Rules {
        metric_thresholds: MetricThresholds {
            clone_min_tokens: Some(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let tuned_stats = run(&rt_tuned, &tuned_rules, &entries(), &markers(), &reach(), false).unwrap();
    assert_eq!(
        tuned_stats.clone_groups, 1,
        "clone_min_tokens = 1 makes the short pair eligible → one group"
    );
    assert_eq!(
        row(&snapshot(&rt_tuned), a1).clone_group,
        Some(a1),
        "the configured floor reached clone::cluster via MetricThresholds::effective()"
    );
}

/// UAT-QM-12 / NFR-RA-06: clustering is byte-identical across runs and a re-pass
/// is idempotent — clone-group membership is recomputed, never accumulated.
#[test]
fn near_clone_clustering_is_deterministic_and_idempotent() {
    let (rt, _dir) = runtime();
    let a = seed_node(&rt, 1, "alpha", NodeKind::Function, false, None, None);
    let b = seed_node(&rt, 2, "beta", NodeKind::Function, false, None, None);
    let set = shingle_set(100, 20);
    seed_shingles(&rt, a, &set);
    seed_shingles(&rt, b, &set);

    let first = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let groups_first: Vec<(NodeId, Option<NodeId>)> = snapshot(&rt)
        .iter()
        .map(|n| (n.id, n.clone_group))
        .collect();

    let second = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let groups_second: Vec<(NodeId, Option<NodeId>)> = snapshot(&rt)
        .iter()
        .map(|n| (n.id, n.clone_group))
        .collect();

    assert_eq!(
        groups_first, groups_second,
        "a second pass yields byte-identical clone-group membership (NFR-RA-06)"
    );
    assert_eq!(first.clones, second.clones);
    assert_eq!(first.clone_groups, second.clone_groups);
    assert_eq!(groups_first, vec![(a, Some(a)), (b, Some(a))]);
}

/// FR-AN-02 untouched: the near-clone pass must not perturb the exact-duplicate
/// verdict or its Redundancy input. Two functions that are near clones by
/// shingle similarity but carry **distinct** AST-shape fingerprints group as
/// near-clones yet stay `is_duplicate = false`; and exact duplicates (shared
/// fingerprint) keep their `is_duplicate = true` regardless of shingles.
#[test]
fn near_clone_pass_leaves_exact_duplicate_verdicts_untouched() {
    let (rt, _dir) = runtime();
    // Near clones (shared shingles) but structurally distinct fingerprints.
    let near_a = seed_node(
        &rt,
        1,
        "near_a",
        NodeKind::Function,
        false,
        Some("fp-a"),
        None,
    );
    let near_b = seed_node(
        &rt,
        2,
        "near_b",
        NodeKind::Function,
        false,
        Some("fp-b"),
        None,
    );
    // Exact duplicates (shared fingerprint) with no shingles at all.
    let dup_a = seed_node(
        &rt,
        3,
        "dup_a",
        NodeKind::Function,
        false,
        Some("fp-dup"),
        None,
    );
    let dup_b = seed_node(
        &rt,
        4,
        "dup_b",
        NodeKind::Function,
        false,
        Some("fp-dup"),
        None,
    );
    let set = shingle_set(100, 20);
    seed_shingles(&rt, near_a, &set);
    seed_shingles(&rt, near_b, &set);

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    // The near clones group …
    assert_eq!(row(&snap, near_a).clone_group, Some(near_a));
    assert_eq!(row(&snap, near_b).clone_group, Some(near_a));
    // … yet are NOT exact duplicates (distinct fingerprints, FR-AN-02 intact).
    assert_eq!(row(&snap, near_a).is_duplicate, Some(false));
    assert_eq!(row(&snap, near_b).is_duplicate, Some(false));
    // The exact duplicates keep their verdict and never join a clone group
    // (they carry no shingles) — the two detections are fully independent.
    assert_eq!(row(&snap, dup_a).is_duplicate, Some(true));
    assert_eq!(row(&snap, dup_b).is_duplicate, Some(true));
    assert_eq!(row(&snap, dup_a).clone_group, None);
    assert_eq!(row(&snap, dup_b).clone_group, None);
    assert_eq!(stats.duplicates, 2, "exact-duplicate count is unchanged");
    assert_eq!(stats.clones, 2, "near-clone count is independent");
}

// ── FR-AN-03 / UAT-AN-03: layer membership + policy materialisation ──────────

#[test]
fn layered_rules_materialise_policy_nodes_and_flag_the_violation() {
    let (rt, _dir) = runtime();
    let domain_file = seed_file(&rt, "src/domain/model.rs");
    let ui_file = seed_file(&rt, "src/ui/view.rs");
    // The deliberate violation: a domain fn calling upward into presentation.
    let domain_fn = seed_node(
        &rt,
        1,
        "compute",
        NodeKind::Function,
        true,
        None,
        Some(domain_file),
    );
    let ui_fn = seed_node(
        &rt,
        2,
        "render",
        NodeKind::Function,
        true,
        None,
        Some(ui_file),
    );
    // And an allowed-direction dependency: presentation consuming the domain.
    let ui_caller = seed_node(
        &rt,
        3,
        "page",
        NodeKind::Function,
        true,
        None,
        Some(ui_file),
    );
    seed_edge(&rt, domain_fn, ui_fn, EdgeKind::Calls); // forbidden: domain -> presentation
    seed_edge(&rt, ui_caller, domain_fn, EdgeKind::Calls); // allowed: presentation -> domain

    let stats = run(&rt, &layered_rules(), &entries(), &markers(), &reach(), false).unwrap();

    // Layer membership lands on the nodes (FR-AN-04 native columns).
    let snap = snapshot(&rt);
    assert_eq!(
        row(&snap, domain_fn).layer_membership.as_deref(),
        Some("domain")
    );
    assert_eq!(
        row(&snap, ui_fn).layer_membership.as_deref(),
        Some("presentation")
    );

    // The policy nodes exist, marked derived (UAT-AN-03).
    let layers: Vec<_> = snap.iter().filter(|n| n.kind == NodeKind::Layer).collect();
    assert_eq!(layers.len(), 2, "one layer node per [[layers]] declaration");
    assert!(
        layers.iter().all(|n| n.derived),
        "policy nodes are derived=1"
    );
    let boundaries: Vec<_> = snap
        .iter()
        .filter(|n| n.kind == NodeKind::Boundary)
        .collect();
    assert_eq!(boundaries.len(), 1);
    assert_eq!(boundaries[0].name, "domain->presentation");

    // Exactly the violating edge is flagged forbidden_dependency (derived).
    let forbidden: Vec<_> = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .collect();
    assert_eq!(forbidden.len(), 1, "one forbidden edge for one violation");
    assert_eq!(forbidden[0].source, domain_fn);
    assert_eq!(forbidden[0].target, ui_fn);
    assert_eq!(stats.forbidden_edges, 1);
    assert_eq!(stats.layer_nodes, 2);
    assert_eq!(stats.boundary_nodes, 1);
}

#[test]
fn no_rules_means_no_policy_artifacts_and_null_layers() {
    let (rt, _dir) = runtime();
    let file = seed_file(&rt, "src/lib.rs");
    let f = seed_node(&rt, 1, "f", NodeKind::Function, true, None, Some(file));

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert_eq!(row(&snap, f).layer_membership, None);
    assert!(
        snap.iter().all(|n| !n.derived),
        "no policy nodes materialise"
    );
    assert_eq!(stats.layer_nodes, 0);
    assert_eq!(stats.boundary_nodes, 0);
    assert_eq!(stats.forbidden_edges, 0);
}

#[test]
fn rerunning_the_pass_is_idempotent() {
    let (rt, _dir) = runtime();
    let domain_file = seed_file(&rt, "src/domain/model.rs");
    let ui_file = seed_file(&rt, "src/ui/view.rs");
    let domain_fn = seed_node(
        &rt,
        1,
        "compute",
        NodeKind::Function,
        true,
        None,
        Some(domain_file),
    );
    let ui_fn = seed_node(
        &rt,
        2,
        "render",
        NodeKind::Function,
        true,
        None,
        Some(ui_file),
    );
    seed_edge(&rt, domain_fn, ui_fn, EdgeKind::Calls);

    let first = run(&rt, &layered_rules(), &entries(), &markers(), &reach(), false).unwrap();
    let second = run(&rt, &layered_rules(), &entries(), &markers(), &reach(), false).unwrap();

    // Same counts both runs: cleared-then-rematerialised, never accumulated.
    assert_eq!(first.layer_nodes, second.layer_nodes);
    assert_eq!(first.boundary_nodes, second.boundary_nodes);
    assert_eq!(first.forbidden_edges, second.forbidden_edges);

    let snap = snapshot(&rt);
    assert_eq!(
        snap.iter().filter(|n| n.kind == NodeKind::Layer).count(),
        2,
        "policy nodes do not accumulate across runs (idempotent, FR-AN-03)"
    );
    let forbidden = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .count();
    assert_eq!(forbidden, 1, "forbidden edges do not accumulate");
}

#[test]
fn rules_flip_reannotates_honestly() {
    let (rt, _dir) = runtime();
    let domain_file = seed_file(&rt, "src/domain/model.rs");
    let ui_file = seed_file(&rt, "src/ui/view.rs");
    let domain_fn = seed_node(
        &rt,
        1,
        "compute",
        NodeKind::Function,
        true,
        None,
        Some(domain_file),
    );
    let ui_fn = seed_node(
        &rt,
        2,
        "render",
        NodeKind::Function,
        true,
        None,
        Some(ui_file),
    );
    seed_edge(&rt, domain_fn, ui_fn, EdgeKind::Calls);

    run(&rt, &layered_rules(), &entries(), &markers(), &reach(), false).unwrap();
    // The architect deletes every rule: the policy graph must vanish and the
    // memberships clear — annotations never outlive the contract they came from.
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert!(snap.iter().all(|n| !n.derived), "policy nodes cleared");
    assert_eq!(
        row(&snap, domain_fn).layer_membership,
        None,
        "membership cleared"
    );
    let forbidden = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .count();
    assert_eq!(forbidden, 0, "forbidden flags cleared with the rules");
}

#[test]
fn policy_symbols_survive_arbitrary_layer_names() {
    // A layer name full of SCIP-hostile characters must still assemble into a
    // valid canonical symbol (escape_name backtick-wraps it).
    let sym = policy_symbol("layer", &["my layer (v2)!"]).expect("escaped symbol parses");
    assert!(sym.as_str().starts_with("logos policy rules . layer/"));

    let plain = policy_symbol("boundary", &["domain", "presentation"]).unwrap();
    assert_eq!(
        plain.as_str(),
        "logos policy rules . boundary/domain/presentation/"
    );
}

// ── FR-GV-12 / CR-002: [[forbidden_imports]] edge materialisation ────────────

/// An `Imports` edge matching a `[[forbidden_imports]]` glob pair is
/// materialised as a derived `forbidden_dependency` edge — the same machinery
/// as boundaries — while a non-matching import and a `Calls` edge are not.
#[test]
fn forbidden_import_materialises_one_derived_edge() {
    let (rt, _dir) = runtime();
    let web_file = seed_file(&rt, "src/web/handler.rs");
    let db_file = seed_file(&rt, "src/db/query.rs");
    let util_file = seed_file(&rt, "src/util/mod.rs");
    let web_fn = seed_node(
        &rt,
        1,
        "handler",
        NodeKind::Function,
        true,
        None,
        Some(web_file),
    );
    let db_fn = seed_node(
        &rt,
        2,
        "query",
        NodeKind::Function,
        true,
        None,
        Some(db_file),
    );
    let util_fn = seed_node(
        &rt,
        3,
        "util",
        NodeKind::Function,
        true,
        None,
        Some(util_file),
    );

    // Banned: web imports db. Not banned: web imports util (off-glob target),
    // and a web->db `Calls` edge (the linter is import/reference-only).
    seed_edge(&rt, web_fn, db_fn, EdgeKind::Imports);
    seed_edge(&rt, web_fn, util_fn, EdgeKind::Imports);
    seed_edge(&rt, web_fn, db_fn, EdgeKind::Calls);

    let stats = run(&rt, &forbidden_import_rules(), &entries(), &markers(), &reach(), false).unwrap();

    let forbidden: Vec<_> = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .collect();
    assert_eq!(
        forbidden.len(),
        1,
        "one derived edge for the one banned import"
    );
    assert_eq!(forbidden[0].source, web_fn);
    assert_eq!(forbidden[0].target, db_fn);
    assert_eq!(stats.forbidden_edges, 1);
    assert_eq!(stats.layer_nodes, 0, "no layers declared");
    assert_eq!(stats.boundary_nodes, 0, "no boundaries declared");
}

/// Re-running the pass never accumulates derived `forbidden_dependency` edges
/// for forbidden imports either (cleared + rebuilt each run, [NFR-RA-06]);
/// dropping the contract clears the derived edge.
#[test]
fn forbidden_import_materialisation_is_idempotent() {
    let (rt, _dir) = runtime();
    let web_file = seed_file(&rt, "src/web/handler.rs");
    let db_file = seed_file(&rt, "src/db/query.rs");
    let web_fn = seed_node(
        &rt,
        1,
        "handler",
        NodeKind::Function,
        true,
        None,
        Some(web_file),
    );
    let db_fn = seed_node(
        &rt,
        2,
        "query",
        NodeKind::Function,
        true,
        None,
        Some(db_file),
    );
    seed_edge(&rt, web_fn, db_fn, EdgeKind::Imports);

    let first = run(&rt, &forbidden_import_rules(), &entries(), &markers(), &reach(), false).unwrap();
    let second = run(&rt, &forbidden_import_rules(), &entries(), &markers(), &reach(), false).unwrap();
    assert_eq!(first.forbidden_edges, second.forbidden_edges);

    let count = |rt: &Runtime| {
        rt.submit_read(|s| s.all_edges())
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
            .count()
    };
    assert_eq!(count(&rt), 1, "no duplicate derived edges on re-run");

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    assert_eq!(count(&rt), 0, "forbidden-import edge cleared with the rule");
}

// ── FR-AN-05 / UAT-AN-04 / CR-001: unified is_test annotation ────────────────

/// The unified `is_test` verdict is the disjunction of the three positive
/// signals (evidence ∨ path convention ∨ marker), and a production function
/// that merely *calls* a test helper carries none of them ([FR-AN-05],
/// [UAT-AN-04], [ADR-18]).
#[test]
fn is_test_set_by_evidence_path_or_marker_but_not_production_caller() {
    let (rt, _dir) = runtime();
    let prod_file = seed_file(&rt, "src/lib.rs");
    let tests_file = seed_file(&rt, "tests/api.rs");

    // 1. Extraction evidence (a Rust `#[cfg(test)] mod tests` fn, S-027) in a
    //    production-pathed file — only the persisted evidence marks it.
    let evid = seed_test_evidence_node(&rt, 1, "inline_case", Some(prod_file));
    // 2. A function in a `tests/` directory — the path convention.
    let path_fn = seed_node(
        &rt,
        2,
        "helper",
        NodeKind::Function,
        false,
        None,
        Some(tests_file),
    );
    // 3. A `[semantics].test_markers` affix (`test_*`) in a production file.
    let marker_fn = seed_node(
        &rt,
        3,
        "test_parse",
        NodeKind::Function,
        false,
        None,
        Some(prod_file),
    );
    // 4. A production function that CALLS a test helper — no positive evidence,
    //    so never is_test (never call-graph inference, ADR-18).
    let prod_caller = seed_node(
        &rt,
        4,
        "parse",
        NodeKind::Function,
        true,
        None,
        Some(prod_file),
    );
    seed_edge(&rt, prod_caller, marker_fn, EdgeKind::Calls);

    let stats = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert!(
        row(&snap, evid).is_test,
        "extraction evidence ⇒ is_test (FR-EX-06)"
    );
    assert!(
        row(&snap, path_fn).is_test,
        "a tests/ path convention ⇒ is_test"
    );
    assert!(
        row(&snap, marker_fn).is_test,
        "a test_markers affix ⇒ is_test"
    );
    assert!(
        !row(&snap, prod_caller).is_test,
        "a production caller of a test helper is never is_test (positive evidence only, ADR-18)"
    );
    assert_eq!(stats.tests, 3, "exactly the three marked nodes are counted");
}

/// An `is_test = true` function is a dead-code live root: an unreferenced,
/// non-exported test helper is never `is_dead`, while a plain production
/// orphan still is ([FR-AN-01], [CR-001]).
#[test]
fn unreferenced_test_is_a_live_root_never_dead() {
    let (rt, _dir) = runtime();
    let tests_file = seed_file(&rt, "tests/api.rs");
    let prod_file = seed_file(&rt, "src/lib.rs");
    // Exported-is-live would NOT save this (it is private + unreferenced) —
    // only is_test live-rooting does.
    let orphan_test = seed_node(
        &rt,
        1,
        "checks_invariant",
        NodeKind::Function,
        false,
        None,
        Some(tests_file),
    );
    let orphan_prod = seed_node(
        &rt,
        2,
        "unused",
        NodeKind::Function,
        false,
        None,
        Some(prod_file),
    );

    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();

    let snap = snapshot(&rt);
    assert!(row(&snap, orphan_test).is_test);
    assert_eq!(
        row(&snap, orphan_test).is_dead,
        Some(false),
        "an is_test=true fn is a live root — never is_dead (FR-AN-01/CR-001)"
    );
    assert_eq!(
        row(&snap, orphan_prod).is_dead,
        Some(true),
        "the plain production orphan is still dead (control)"
    );
}

/// Direct coverage of the `is_test_marked` predicate ([FR-AN-05]): each
/// positive disjunct fires independently, marker matching is affix-not-
/// substring, and a plain production function matches none. Replaces the
/// former `test_gaps` marking unit test now that the predicate lives here.
#[test]
fn is_test_marked_covers_evidence_path_and_affix_with_negatives() {
    fn r(name: &str, path: Option<&str>, evidence: bool) -> AnnotationNodeRow {
        AnnotationNodeRow {
            id: NodeId(1),
            kind: NodeKind::Function,
            name: name.to_string(),
            exported: false,
            derived: false,
            fingerprint: None,
            test_evidence: evidence,
            file_id: path.map(|_| 1),
            file_path: path.map(str::to_string),
            is_dead: None,
            is_duplicate: None,
            is_test: false,
            layer_membership: None,
            clone_group: None,
        }
    }
    let m = markers();
    // 1. Extraction evidence alone (production path, plain name).
    assert!(is_test_marked(&r("inline", Some("src/lib.rs"), true), &m));
    // 2. Path conventions (tests/ segment, *_test.* filename).
    assert!(is_test_marked(
        &r("anything", Some("tests/api.rs"), false),
        &m
    ));
    assert!(is_test_marked(
        &r("x", Some("src/parser_test.rs"), false),
        &m
    ));
    // 3. Marker name-affix and exact match on a production path.
    assert!(is_test_marked(
        &r("test_parse", Some("src/lib.rs"), false),
        &m
    ));
    assert!(is_test_marked(
        &r("parse_spec", Some("src/lib.rs"), false),
        &m
    ));
    assert!(is_test_marked(&r("test", Some("src/lib.rs"), false), &m));
    // Negatives: plain production, and a name merely CONTAINING a marker.
    assert!(!is_test_marked(&r("parse", Some("src/lib.rs"), false), &m));
    assert!(!is_test_marked(
        &r("attestation", Some("src/lib.rs"), false),
        &m
    ));
}

/// `is_test` is recomputed each run from the persisted inputs, so it is
/// idempotent and deterministic ([NFR-RA-06]).
#[test]
fn is_test_recomputation_is_idempotent() {
    let (rt, _dir) = runtime();
    let tests_file = seed_file(&rt, "tests/api.rs");
    let t = seed_node(
        &rt,
        1,
        "case",
        NodeKind::Function,
        false,
        None,
        Some(tests_file),
    );
    let first = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let second = run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    assert_eq!(first.tests, second.tests, "test count stable across runs");
    assert!(row(&snapshot(&rt), t).is_test);
}

// ── S-229: the parallel annotation compute is byte-identical across workers ──

/// Seed a graph exercising every verdict class, with a near-clone structure big
/// enough to drive the parallel keyspace-sharded clustering (S-229): a live
/// exported root calling a helper, a dead function, an exact-duplicate pair, a
/// test-named function, two 12-member near-clone groups (identical 40-shingle
/// sets), 8 solo functions with distinct sets, and a "hub" shingle every clustered
/// function shares (a large posting → many candidate pairs across shards). Seed
/// order is fixed, so a fresh store always assigns identical rowids — the verdict
/// tuples (which include the `NodeId`) are comparable across runtimes.
fn seed_annotate_fixture(rt: &Runtime) {
    const HUB: u64 = 9_000_000;
    // Verdict spread: an exported (live) root that calls `helper` (so `helper` is
    // reachable), a never-called `dead` function, a fingerprint-duplicate pair,
    // and a `test_`-prefixed function (is_test via a marker affix).
    let root = seed_node(rt, 0, "main", NodeKind::Function, true, None, None);
    let helper = seed_node(rt, 1, "helper", NodeKind::Function, false, None, None);
    seed_edge(rt, root, helper, EdgeKind::Calls);
    seed_node(rt, 2, "dead", NodeKind::Function, false, None, None);
    seed_node(rt, 3, "dup_a", NodeKind::Function, false, Some("fp"), None);
    seed_node(rt, 4, "dup_b", NodeKind::Function, false, Some("fp"), None);
    seed_node(rt, 5, "test_case", NodeKind::Function, false, None, None);

    // Two near-clone groups + solo functions, each also carrying the hub shingle.
    let mut n = 10u32;
    let group_a = shingle_set(100, 40);
    let group_b = shingle_set(200, 40);
    for _ in 0..12 {
        let id = seed_node(rt, n, &format!("clone_a_{n}"), NodeKind::Function, false, None, None);
        let mut hs = group_a.clone();
        hs.push(HUB);
        seed_shingles(rt, id, &hs);
        n += 1;
    }
    for _ in 0..12 {
        let id = seed_node(rt, n, &format!("clone_b_{n}"), NodeKind::Function, false, None, None);
        let mut hs = group_b.clone();
        hs.push(HUB);
        seed_shingles(rt, id, &hs);
        n += 1;
    }
    for _ in 0..8 {
        let id = seed_node(rt, n, &format!("solo_{n}"), NodeKind::Function, false, None, None);
        let base = 1_000 + u64::from(n) * 100;
        let mut hs = shingle_set(base, 40);
        hs.push(HUB);
        seed_shingles(rt, id, &hs);
        n += 1;
    }
}

/// S-229 / [NFR-RA-06]: the full annotation pass — near-clone clustering **and**
/// the per-node verdict loop, both now on the shared worker pool — produces
/// byte-identical verdicts across worker counts. The same fixture is annotated on
/// runtimes with 1, 2, 4, and 8 worker threads (so the clustering shards the pair
/// keyspace differently each time); the complete id-ordered verdict tuple must
/// equal the single-worker baseline exactly.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn annotation_is_byte_identical_across_worker_counts() {
    // Layered rules so the parallel verdict loop's `layer` computation is
    // exercised with a *non-null* value (every fixture node lives under `src/…`),
    // not merely the `None` default — the loop derives `layer` inside the
    // `par_iter().map(…)` closure, so it belongs in the cross-worker comparison.
    let rules = Rules {
        layers: vec![Layer { name: "core".to_string(), paths: vec!["src/**".to_string()], order: 1 }],
        ..Rules::default()
    };

    let (baseline_rt, _b) = runtime_with_workers(1);
    seed_annotate_fixture(&baseline_rt);
    run(&baseline_rt, &rules, &entries(), &markers(), &reach(), false).unwrap();
    let baseline = verdicts(&baseline_rt);

    // The baseline must carry the full verdict spread, or the equivalence check
    // would be vacuous. Pin the near-clone structure *exactly* (the fixture is
    // designed for two 12-member groups → 24 clustered), so a silent
    // fixture-shape regression is caught, not just a worker-count divergence.
    assert!(baseline.iter().any(|v| v.1 == Some(true)), "a dead verdict present");
    assert!(baseline.iter().any(|v| v.2 == Some(true)), "a duplicate verdict present");
    assert!(baseline.iter().any(|v| v.3), "a test verdict present");
    assert!(baseline.iter().any(|v| v.4.is_some()), "a non-null layer present");
    let clustered = baseline.iter().filter(|v| v.5.is_some()).count();
    assert_eq!(clustered, 24, "the fixture clusters exactly the 24 near-clone functions");
    let groups: std::collections::BTreeSet<NodeId> = baseline.iter().filter_map(|v| v.5).collect();
    assert_eq!(groups.len(), 2, "the fixture forms exactly two near-clone groups");

    for workers in [2, 4, 8] {
        let (rt, _d) = runtime_with_workers(workers);
        seed_annotate_fixture(&rt);
        run(&rt, &rules, &entries(), &markers(), &reach(), false).unwrap();
        assert_eq!(
            baseline,
            verdicts(&rt),
            "annotation verdicts differ at {workers} workers (NFR-RA-06)"
        );
    }
}

/// S-229: repeated full re-annotates under a multi-worker runtime are stable — 30
/// runs over the same graph all reproduce the first verdict set, so the parallel
/// clustering + verdict loop carry no data race or run-to-run nondeterminism (the
/// `--threads > 1` stress).
#[test]
fn annotation_is_stable_under_repeated_multiworker_runs() {
    let (rt, _d) = runtime_with_workers(8);
    seed_annotate_fixture(&rt);
    run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
    let first = verdicts(&rt);
    for run_ix in 0..30 {
        run(&rt, &Rules::default(), &entries(), &markers(), &reach(), false).unwrap();
        assert_eq!(first, verdicts(&rt), "verdicts drifted on repeat run {run_ix}");
    }
}
