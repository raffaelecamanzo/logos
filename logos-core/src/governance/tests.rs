//! Pure unit tests for the governance engine (S-020): the rules evaluator
//! ([FR-GV-02], BR-11/BR-12/BR-15), the freshness line ([FR-RC-03/04],
//! [NFR-RA-11]), gate regression detail ([FR-GV-05]), test marking
//! ([FR-GV-08]), and evolution deltas ([FR-GV-06]) — no I/O. Store-backed
//! behaviour (reconcile, baseline persistence, snapshots, caches) is covered
//! by the integration suite in `logos-core/tests/governance.rs`.
//!
//! [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
//! [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
//! [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
//! [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
//! [FR-RC-03/04]: ../../../docs/specs/requirements/FR-RC-03.md
//! [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md

use super::*;
use crate::config::{
    Boundary, Constraints, ForbiddenImport, Layer, MaxDead, MaxDeadBaseline, RequireDocumented,
    RequireTested,
};
use crate::graph_store::{AnnotationNodeRow, FunctionMetricRow};
use crate::model::{LogosSymbol, NodeId};

// ── Fixture builders ────────────────────────────────────────────────────────

/// A `NodeRow` with id `id`, named `name`, defined in `file`.
fn node(id: i64, name: &str, file: Option<&str>) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("test symbol"),
        kind: NodeKind::Function,
        name: name.to_string(),
        file_path: file.map(str::to_string),
        start_line: Some(1),
        end_line: Some(2),
    }
}

/// A `calls` edge `source → target`.
fn edge(source: i64, target: i64) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind: EdgeKind::Calls,
    }
}

/// Compile a three-layer contract: domain(0) ← app(1) ← infra(2), with one
/// boundary forbidding domain → infra.
fn layered_rules() -> CompiledRules {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: vec![
            Layer {
                name: "domain".to_string(),
                paths: vec!["domain/**".to_string()],
                order: 0,
            },
            Layer {
                name: "app".to_string(),
                paths: vec!["app/**".to_string()],
                order: 1,
            },
            Layer {
                name: "infra".to_string(),
                paths: vec!["infra/**".to_string()],
                order: 2,
            },
        ],
        boundaries: vec![Boundary {
            from: "domain".to_string(),
            to: "infra".to_string(),
            reason: Some("the domain stays IO-free".to_string()),
        }],
        forbidden_imports: Vec::new(),
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    CompiledRules::compile(rules, "test".to_string()).expect("test globs compile")
}

/// An edge `source → target` of an arbitrary kind (the forbidden-imports linter
/// acts on `Imports`/`References`, not `Calls`).
fn kind_edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

/// Compile a contract carrying a single `[[forbidden_imports]]` ban
/// (`src/web/** -> src/db/**`) and nothing else ([FR-GV-12]).
fn forbidden_import_rules() -> CompiledRules {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: vec![ForbiddenImport {
            from: "src/web/**".to_string(),
            to: "src/db/**".to_string(),
            reason: Some("the web layer must not import the db directly".to_string()),
        }],
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    CompiledRules::compile(rules, "test".to_string()).expect("test globs compile")
}

/// An `EvalInput` over the given graph with the layered contract.
// A plain test helper wrapping the pure `evaluate` fn (no code execution).
// The redundancy budgets ([FR-GV-11]) read `function_metrics`; the constraint
// tests that don't exercise them pass an empty slice via this helper, while
// `run_eval_redundancy` supplies the annotation rows.
fn run_eval<'a>(
    compiled: &'a CompiledRules,
    nodes: &'a [NodeRow],
    edges: &'a [EdgeRow],
    functions: &'a [FunctionConstraintRow],
    cycles: u64,
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes,
        edges,
        functions,
        function_metrics: &[],
        annotations: &[],
        test_node_ids: &[],
        cycles,
        thresholds: crate::metrics::Thresholds::default(),
    })
}

/// A `dep` edge `source → target` of an arbitrary non-`Contains` kind — the
/// coupling budgets count every dependency kind, so `References` exercises the
/// "not just calls" basis ([BR-19]).
fn dep_edge(source: i64, target: i64, kind: EdgeKind) -> EdgeRow {
    EdgeRow {
        source: NodeId(source),
        target: NodeId(target),
        kind,
    }
}

/// A `FunctionMetricRow` carrying only the dead/duplicate verdicts the
/// redundancy budgets read.
fn metric_row(id: i64, is_dead: bool, is_duplicate: bool) -> FunctionMetricRow {
    FunctionMetricRow {
        id: NodeId(id),
        cyclomatic_complexity: None,
        is_dead: Some(is_dead),
        is_duplicate: Some(is_duplicate),
        line_count: None,
        max_nesting_depth: None,
        clone_group: None,
    }
}

/// Evaluate with `test_node_ids` supplied (the [CR-065] production-scoped
/// coupling path).
///
/// [CR-065]: ../../../docs/requests/CR-065-module-grain-coupling-metric.md
fn run_eval_coupling(
    compiled: &CompiledRules,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    test_node_ids: &[NodeId],
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes,
        edges,
        functions: &[],
        function_metrics: &[],
        annotations: &[],
        test_node_ids,
        cycles: 0,
        thresholds: crate::metrics::Thresholds::default(),
    })
}

/// Evaluate with `function_metrics` supplied (the redundancy-budget path).
fn run_eval_redundancy(
    compiled: &CompiledRules,
    function_metrics: &[FunctionMetricRow],
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes: &[],
        edges: &[],
        functions: &[],
        function_metrics,
        annotations: &[],
        test_node_ids: &[],
        cycles: 0,
        thresholds: crate::metrics::Thresholds::default(),
    })
}

// ── FR-GV-02 / BR-11 / UAT-GV-01: layer ordering ───────────────────────────

/// An upward dependency (order i → order j, j > i) between two assigned
/// layers is reported as an error violation; the downward direction is fine.
#[test]
fn upward_dependency_between_assigned_layers_is_reported() {
    let compiled = layered_rules();
    let nodes = [
        node(1, "pure", Some("domain/core.rs")),
        node(2, "handler", Some("app/handler.rs")),
    ];
    // domain (order 0) calls app (order 1): upward — violates BR-11.
    let edges = [edge(1, 2)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(violations.len(), 1, "exactly one ordering violation");
    let v = &violations[0];
    assert_eq!(v.rule, "layer-ordering");
    assert_eq!(v.rule_type, "layer");
    assert_eq!(v.severity, "error", "checked-in policy is binding");
    assert_eq!(v.file, "domain/core.rs");
    assert!(
        v.message.contains("domain") && v.message.contains("app"),
        "message names both layers: {}",
        v.message
    );

    // The reverse direction (app → domain, order 1 → 0) is the sanctioned
    // downward dependency.
    let edges = [edge(2, 1)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert!(violations.is_empty(), "downward deps violate nothing");
}

/// BR-15 / UAT-GV-02: an edge touching a file assigned to no layer is exempt
/// from ordering — layers are opt-in.
#[test]
fn unassigned_files_are_exempt_from_layer_ordering() {
    let compiled = layered_rules();
    let nodes = [
        node(1, "pure", Some("domain/core.rs")),
        node(2, "helper", Some("scripts/tool.rs")), // matches no layer glob
    ];
    // Both directions cross a layered/unlayered pair: exempt either way.
    let edges = [edge(1, 2), edge(2, 1)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert!(
        violations.is_empty(),
        "unassigned files are exempt (BR-15), got {violations:?}"
    );
}

/// FR-GV-02 idempotence: the evaluator is a pure function — the same input
/// yields the identical violation list, and repeated edges between the same
/// file pair collapse to one violation.
#[test]
fn evaluation_is_deterministic_and_dedupes_per_file_pair() {
    let compiled = layered_rules();
    let nodes = [
        node(1, "a", Some("domain/core.rs")),
        node(2, "b", Some("domain/core.rs")),
        node(3, "h", Some("app/handler.rs")),
    ];
    // Two distinct upward edges between the SAME file pair.
    let edges = [edge(1, 3), edge(2, 3)];
    let (first, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    let (second, _) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(first.len(), 1, "one violation per offending file pair");
    assert_eq!(first, second, "re-running yields identical violations");
}

// ── FR-GV-02: boundaries ────────────────────────────────────────────────────

/// A dependency crossing a declared `[[boundaries]]` pair is flagged with
/// the boundary key and the declared reason.
#[test]
fn boundary_crossing_is_reported_with_reason() {
    let compiled = layered_rules();
    let nodes = [
        node(1, "pure", Some("domain/core.rs")),
        node(2, "db", Some("infra/db.rs")),
    ];
    // domain → infra is both upward (0 → 2) AND the declared boundary.
    let edges = [edge(1, 2)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);

    let boundary = violations
        .iter()
        .find(|v| v.rule_type == "boundary")
        .expect("boundary violation present");
    assert_eq!(boundary.rule, "boundary:domain->infra");
    assert_eq!(boundary.severity, "error");
    assert!(
        boundary.message.contains("IO-free"),
        "the declared reason is surfaced: {}",
        boundary.message
    );
    // The ordering violation rides alongside — two findings, one edge.
    assert!(violations.iter().any(|v| v.rule == "layer-ordering"));
}

// ── FR-GV-02: constraints (point queries) ──────────────────────────────────

/// Each set constraint is enforced; `checked_rules` counts exactly the
/// active rules (set constraints + ordering + boundaries).
#[test]
fn constraints_enforce_their_budgets() {
    let rules = Rules {
        constraints: Constraints {
            max_cycles: Some(1),
            max_cc: Some(10),
            max_fn_lines: Some(50),
            no_god_files: Some(2),
            ..Constraints::default()
        },
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: Vec::new(),
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    let compiled = CompiledRules::compile(rules, "test".to_string()).unwrap();

    let nodes = [
        node(1, "a", Some("src/big.rs")),
        node(2, "b", Some("src/big.rs")),
        node(3, "c", Some("src/big.rs")), // 3 symbols > no_god_files = 2
    ];
    let functions = [
        FunctionConstraintRow {
            id: NodeId(1),
            name: "complex".to_string(),
            file_path: Some("src/big.rs".to_string()),
            start_line: Some(1),
            cyclomatic_complexity: Some(11), // > max_cc = 10
            line_count: Some(60),            // > max_fn_lines = 50
        },
        FunctionConstraintRow {
            id: NodeId(2),
            name: "fine".to_string(),
            file_path: Some("src/big.rs".to_string()),
            start_line: Some(70),
            cyclomatic_complexity: Some(2),
            line_count: Some(5),
        },
        FunctionConstraintRow {
            id: NodeId(3),
            name: "unknown".to_string(),
            file_path: Some("src/big.rs".to_string()),
            start_line: Some(80),
            cyclomatic_complexity: None, // NULL = not computed → no verdict
            line_count: None,
        },
    ];
    // cycles = 2 > max_cycles = 1.
    let (violations, checked) = run_eval(&compiled, &nodes, &[], &functions, 2);

    let rules_hit: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(
        rules_hit,
        ["max_cycles", "max_cc", "max_fn_lines", "no_god_files"],
        "all four constraints fire, in canonical order"
    );
    assert_eq!(checked, 4, "four active constraints, no layers/boundaries");

    let cc = violations.iter().find(|v| v.rule == "max_cc").unwrap();
    assert_eq!(
        cc.node_id,
        Some(1),
        "per-function violations carry the node"
    );
    assert!(cc.message.contains("complex") && cc.message.contains("11"));
}

/// An omitted constraint is simply not enforced (`Constraints` fields are
/// opt-in budgets).
#[test]
fn omitted_constraints_are_not_enforced() {
    let compiled =
        CompiledRules::compile(Rules::default(), "test".to_string()).expect("empty contract");
    let functions = [FunctionConstraintRow {
        id: NodeId(1),
        name: "huge".to_string(),
        file_path: Some("src/a.rs".to_string()),
        start_line: Some(1),
        cyclomatic_complexity: Some(1000),
        line_count: Some(10_000),
    }];
    let (violations, checked) = run_eval(&compiled, &[], &[], &functions, 99);
    assert!(violations.is_empty(), "no budgets set → nothing to violate");
    assert_eq!(checked, 0, "no active rules");
}

// ── FR-GV-11 / BR-19: coupling budgets (per-symbol fan-in / fan-out) ────────

/// Compile a constraints-only contract (no layers/boundaries).
fn constraints_only(constraints: Constraints) -> CompiledRules {
    CompiledRules::compile(
        Rules {
            constraints,
            metric_thresholds: Default::default(),
            layers: Vec::new(),
            boundaries: Vec::new(),
            forbidden_imports: Vec::new(),
            require_tested: Vec::new(),
            require_documented: Vec::new(),
            history: Default::default(),
            coverage: Default::default(),
        },
        "test".to_string(),
    )
    .expect("constraints-only contract compiles")
}

/// An over-coupled hub's MODULE above `max_fan_in` is reported with its
/// distinct-neighbouring-module count ([CR-065]); a module-grain violation has
/// no single owning node, and the file-fallback module key (no
/// `NodeKind::Module` ancestor in these fixtures) names the file.
///
/// [CR-065]: ../../../docs/requests/CR-065-module-grain-coupling-metric.md
#[test]
fn fan_in_over_budget_is_reported_with_count() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(2),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "hub", Some("src/hub.rs")),
        node(2, "a", Some("src/a.rs")),
        node(3, "b", Some("src/b.rs")),
        node(4, "c", Some("src/c.rs")),
    ];
    // Three callers, each its own module (file), depend on the hub's module:
    // module fan-in 3 > max_fan_in = 2.
    let edges = [edge(2, 1), edge(3, 1), edge(4, 1)];
    let (violations, checked) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(checked, 1, "one active coupling budget");
    assert_eq!(violations.len(), 1, "only the hub's module is over budget");
    let v = &violations[0];
    assert_eq!(v.rule, "max_fan_in");
    assert_eq!(v.rule_type, "constraint");
    assert_eq!(v.severity, "error");
    assert_eq!(
        v.node_id, None,
        "a module-grain violation has no single owning node"
    );
    assert_eq!(v.file, "src/hub.rs");
    assert!(
        v.message.contains("src/hub.rs") && v.message.contains("fan-in 3"),
        "the count is surfaced: {}",
        v.message
    );

    // Determinism (NFR-RA-06): a re-run yields the identical list.
    let (again, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert_eq!(again, violations);
}

/// A module above `max_fan_out` is reported with its distinct-neighbouring-
/// module count.
#[test]
fn fan_out_over_budget_is_reported_with_count() {
    let compiled = constraints_only(Constraints {
        max_fan_out: Some(2),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "spider", Some("src/spider.rs")),
        node(2, "a", Some("src/a.rs")),
        node(3, "b", Some("src/b.rs")),
        node(4, "c", Some("src/c.rs")),
    ];
    // The spider's module depends on three distinct target modules: module
    // fan-out 3 > max_fan_out = 2.
    let edges = [edge(1, 2), edge(1, 3), edge(1, 4)];
    let (violations, checked) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(checked, 1);
    assert_eq!(violations.len(), 1);
    let v = &violations[0];
    assert_eq!(v.rule, "max_fan_out");
    assert_eq!(v.node_id, None);
    assert_eq!(v.file, "src/spider.rs");
    assert!(
        v.message.contains("src/spider.rs") && v.message.contains("fan-out 3"),
        "{}",
        v.message
    );
}

/// [CR-065] AC: a shared helper called from many symbols in ONE module counts
/// as one neighbour, not N — the module-grain distinction from the old
/// per-symbol count (which would have reported fan-in 3 here).
///
/// [CR-065]: ../../../docs/requests/CR-065-module-grain-coupling-metric.md
#[test]
fn coupling_counts_distinct_neighbouring_modules_not_symbols() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(1),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "helper", Some("src/helper.rs")),
        node(2, "caller_a", Some("src/caller.rs")),
        node(3, "caller_b", Some("src/caller.rs")),
        node(4, "caller_c", Some("src/caller.rs")),
    ];
    // Three symbols in the SAME module (file) all call the shared helper: one
    // neighbouring module, not three callers, so fan-in 1 does not exceed
    // max_fan_in = 1 (per-symbol fan-in 3 WOULD have tripped this budget).
    let edges = [edge(2, 1), edge(3, 1), edge(4, 1)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert!(
        violations.is_empty(),
        "one neighbouring module, not three callers: {violations:?}"
    );

    // A second, genuinely DISTINCT caller module pushes fan-in to 2 neighbours.
    let nodes_two_modules = [
        node(1, "helper", Some("src/helper.rs")),
        node(2, "caller_a", Some("src/caller.rs")),
        node(5, "other", Some("src/other.rs")),
    ];
    let edges_two_modules = [edge(2, 1), edge(5, 1)];
    let (violations_two, _) = run_eval(&compiled, &nodes_two_modules, &edges_two_modules, &[], 0);
    assert_eq!(
        violations_two.len(),
        1,
        "two distinct neighbouring modules > max_fan_in = 1"
    );
}

/// [CR-065] AC: `is_test` nodes and their incident edges are excluded BEFORE
/// the rollup — a test-only module is never reported, and neither a
/// test→production nor a production→test edge contributes to either side's
/// count.
///
/// [CR-065]: ../../../docs/requests/CR-065-module-grain-coupling-metric.md
#[test]
fn coupling_excludes_test_only_module_before_rollup() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(0),
        max_fan_out: Some(0),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "prod_fn", Some("src/prod.rs")),
        node(2, "mock_helper", Some("tests/mocks.rs")),
        node(3, "test_a", Some("tests/mocks.rs")),
        node(4, "test_b", Some("tests/mocks.rs")),
    ];
    // Two test symbols call a mock helper in the same test-only module, and
    // the mock both depends on and is depended on by production code — all of
    // it must vanish before the rollup.
    let edges = [edge(3, 2), edge(4, 2), edge(2, 1), edge(1, 2)];
    let test_ids = [NodeId(2), NodeId(3), NodeId(4)];
    let (violations, checked) = run_eval_coupling(&compiled, &nodes, &edges, &test_ids);
    assert_eq!(checked, 2, "both coupling budgets are active");
    assert!(
        violations.is_empty(),
        "a test-only module and its incident edges are excluded before the rollup: {violations:?}"
    );
}

/// BR-19 canonical edge set: every dependency kind counts EXCEPT `Contains`
/// (lexical containment), `Accesses` (member access, [CR-065]), and the
/// derived `ForbiddenDependency` edge (last run's governance output, BR-12).
#[test]
fn coupling_excludes_contains_accesses_and_forbidden_dependency_edges() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(0),
        ..Constraints::default()
    });
    let nodes = [node(1, "hub", Some("src/hub.rs")), node(2, "a", Some("src/a.rs"))];

    // Only excluded kinds point at the hub's module → effective fan-in 0, no
    // violation even with max_fan_in = 0.
    let excluded = [
        dep_edge(2, 1, EdgeKind::Contains),
        dep_edge(2, 1, EdgeKind::Accesses),
        dep_edge(2, 1, EdgeKind::ForbiddenDependency),
    ];
    let (violations, _) = run_eval(&compiled, &nodes, &excluded, &[], 0);
    assert!(
        violations.is_empty(),
        "Contains/Accesses/ForbiddenDependency are not coupling dependencies: {violations:?}"
    );

    // A single real dependency (any other kind) now exceeds 0.
    let real = [dep_edge(2, 1, EdgeKind::References)];
    let (violations, _) = run_eval(&compiled, &nodes, &real, &[], 0);
    assert_eq!(
        violations.len(),
        1,
        "a References edge counts toward fan-in"
    );
    assert_eq!(violations[0].rule, "max_fan_in");
}

/// A `NodeKind::Module` node, mirroring the synthetic per-file module every
/// real extraction emits (S-011) — unlike the plain `node()` fixture, walking
/// `Contains` ancestry from a node it contains resolves to the PRIMARY
/// `module:<symbol>` rollup key, not the `file:<path>` fallback.
fn module_node(id: i64, name: &str, file: Option<&str>) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("test symbol"),
        kind: NodeKind::Module,
        name: name.to_string(),
        file_path: file.map(str::to_string),
        start_line: Some(1),
        end_line: Some(1),
    }
}

/// [CR-065] regression guard: every other coupling fixture in this file has no
/// `Contains` ancestor, so `module_key()` always falls back to the `file:`
/// form — which happens to carry its own path. In real extraction EVERY file
/// gets a synthetic `NodeKind::Module` node (S-011), so the rollup vertex
/// almost always resolves to the PRIMARY `module:<symbol>` form instead. This
/// pins that the violation's `file` is still populated (from the underlying
/// Module row), not silently empty, on that far more common real-world path.
#[test]
fn coupling_violation_on_real_module_ancestor_has_file() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(0),
        ..Constraints::default()
    });
    let nodes = [
        module_node(1, "hub", Some("src/hub.rs")),
        node(2, "hub_fn", Some("src/hub.rs")),
        module_node(3, "caller", Some("src/caller.rs")),
        node(4, "caller_fn", Some("src/caller.rs")),
    ];
    let edges = [
        kind_edge(1, 2, EdgeKind::Contains), // the hub module contains hub_fn
        kind_edge(3, 4, EdgeKind::Contains), // the caller module contains caller_fn
        edge(4, 2),                          // caller_fn calls hub_fn: cross-module
    ];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(violations.len(), 1);
    let v = &violations[0];
    assert_eq!(v.rule, "max_fan_in");
    assert_eq!(
        v.file, "src/hub.rs",
        "a genuine module vertex resolves its file from the underlying Module \
         row, not the empty project-wide default: {v:?}"
    );
    assert!(v.message.contains("hub"), "{}", v.message);
}

/// [NFR-RA-06] AC: violations are ordered by stable MODULE KEY, not by
/// rollup-vertex insertion order (which follows first-seen node id). Node ids
/// here are assigned so insertion order (z's caller has the lowest id, so z's
/// module vertex is created first) is the OPPOSITE of key order, so this only
/// passes if the explicit key sort is doing the work.
#[test]
fn coupling_violations_are_ordered_by_module_key_not_insertion_order() {
    let compiled = constraints_only(Constraints {
        max_fan_in: Some(0),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "z_hub", Some("z.rs")), // module key "file:z.rs" — created FIRST
        node(2, "z_caller", Some("z_caller.rs")),
        node(3, "a_hub", Some("a.rs")), // module key "file:a.rs" — created SECOND
        node(4, "a_caller", Some("a_caller.rs")),
    ];
    let edges = [edge(2, 1), edge(4, 3)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(violations.len(), 2, "both modules are over max_fan_in = 0");
    assert_eq!(
        violations.iter().map(|v| v.file.as_str()).collect::<Vec<_>>(),
        ["a.rs", "z.rs"],
        "module-key order (NFR-RA-06), not first-seen insertion order"
    );
}

// ── FR-GV-11: redundancy budgets (project-wide is_dead / is_duplicate) ──────

/// `max_dead`/`max_duplicates` count `is_dead`/`is_duplicate` functions
/// project-wide; a project over either ceiling is reported, a clean one passes.
#[test]
fn redundancy_budgets_count_dead_and_duplicates_project_wide() {
    // 3 dead (#1,#2,#3), 2 duplicate (#3,#4).
    let metrics = [
        metric_row(1, true, false),
        metric_row(2, true, false),
        metric_row(3, true, true),
        metric_row(4, false, true),
    ];

    // max_dead = 2 fires (3 > 2); max_duplicates = 2 does NOT (2 is not > 2).
    let compiled = constraints_only(Constraints {
        max_dead: Some(MaxDead::Absolute(2)),
        max_duplicates: Some(2),
        ..Constraints::default()
    });
    let (violations, checked) = run_eval_redundancy(&compiled, &metrics);
    assert_eq!(checked, 2, "both redundancy budgets are active");
    let rules_hit: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(rules_hit, ["max_dead"], "only the dead ceiling is exceeded");
    let dead = &violations[0];
    assert_eq!(dead.node_id, None, "redundancy budgets are project-wide");
    assert_eq!(dead.file, "", "no single offending file");
    assert!(
        dead.message.contains("3 dead functions") && dead.message.contains("max_dead = 2"),
        "{}",
        dead.message
    );

    // A clean project (ceilings above the counts) passes.
    let lenient = constraints_only(Constraints {
        max_dead: Some(MaxDead::Absolute(5)),
        max_duplicates: Some(5),
        ..Constraints::default()
    });
    let (clean, checked) = run_eval_redundancy(&lenient, &metrics);
    assert!(clean.is_empty(), "counts under ceilings → no violation");
    assert_eq!(checked, 2);
}

/// FR-GV-11 / CR-043 / ADR-39: in delta-from-blessed-baseline mode the gate fails
/// only when the dead count rises ABOVE the blessed baseline — the known steady-
/// state passes, a newly-introduced dead function fails, and a re-run is
/// byte-identical ([NFR-RA-06]). The count basis is unchanged (`is_dead == Some(true)`),
/// so the mode is purely a comparison change orthogonal to the metrics gate.
#[test]
fn max_dead_delta_mode_passes_baseline_and_catches_new_dead() {
    // The blessed steady-state: exactly 3 dead functions.
    let steady = [
        metric_row(1, true, false),
        metric_row(2, true, false),
        metric_row(3, true, false),
    ];
    // One newly-introduced dead function on top of the steady-state (4 dead).
    let regressed = [
        metric_row(1, true, false),
        metric_row(2, true, false),
        metric_row(3, true, false),
        metric_row(4, true, false),
    ];

    // baseline = 3, delta = 0 → fail iff dead > 3.
    let compiled = constraints_only(Constraints {
        max_dead: Some(MaxDead::Baseline(MaxDeadBaseline {
            baseline: 3,
            delta: 0,
        })),
        ..Constraints::default()
    });

    // The blessed steady-state holds the baseline — no violation.
    let (held, checked) = run_eval_redundancy(&compiled, &steady);
    assert!(
        held.is_empty(),
        "the blessed steady-state (3) holds the baseline (3)"
    );
    assert_eq!(checked, 1, "the delta-mode budget counts as one active rule");

    // A re-run over the same inputs is byte-identical (NFR-RA-06).
    let (held_again, _) = run_eval_redundancy(&compiled, &steady);
    assert_eq!(held, held_again, "a clean re-run is byte-identical");

    // A newly-introduced dead function rises above the baseline — fail.
    let (caught, _) = run_eval_redundancy(&compiled, &regressed);
    let rules_hit: Vec<&str> = caught.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(rules_hit, ["max_dead"], "the new dead function trips the gate");
    let dead = &caught[0];
    assert_eq!(dead.node_id, None, "the budget is project-wide");
    assert_eq!(dead.rule_type, "constraint");
    assert!(
        dead.message
            .contains("exceed the blessed max_dead baseline 3"),
        "the delta-mode message names the blessed baseline: {}",
        dead.message
    );

    // With slack (baseline = 3, delta = 1) the regression of one is tolerated.
    let lenient = constraints_only(Constraints {
        max_dead: Some(MaxDead::Baseline(MaxDeadBaseline {
            baseline: 3,
            delta: 1,
        })),
        ..Constraints::default()
    });
    let (tolerated, _) = run_eval_redundancy(&lenient, &regressed);
    assert!(
        tolerated.is_empty(),
        "4 dead is within baseline 3 + delta 1"
    );
}

/// AC: omitting all four coupling/redundancy keys enforces none of them — even
/// over a graph that would violate any non-zero ceiling.
#[test]
fn coupling_and_redundancy_omitted_enforce_nothing() {
    let compiled = constraints_only(Constraints::default());
    let nodes = [node(1, "hub", Some("src/hub.rs")), node(2, "a", None)];
    let edges = [edge(2, 1), edge(1, 2)];
    let metrics = [metric_row(1, true, true)];
    let (violations, checked) = evaluate(&EvalInput {
        compiled: &compiled,
        nodes: &nodes,
        edges: &edges,
        functions: &[],
        function_metrics: &metrics,
        annotations: &[],
        test_node_ids: &[],
        cycles: 0,
        thresholds: crate::metrics::Thresholds::default(),
    });
    assert!(violations.is_empty(), "no budgets set → nothing enforced");
    assert_eq!(checked, 0, "no active rules");
}

// ── FR-GV-01: first-glob-wins layer assignment (DL-05) ─────────────────────

/// A file matching several layer globs is assigned by declaration order —
/// first match wins (resolved OQ-02).
#[test]
fn layer_assignment_is_first_glob_wins() {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: vec![
            Layer {
                name: "first".to_string(),
                paths: vec!["src/**".to_string()],
                order: 0,
            },
            Layer {
                name: "second".to_string(),
                paths: vec!["src/sub/**".to_string()],
                order: 1,
            },
        ],
        boundaries: Vec::new(),
        forbidden_imports: Vec::new(),
        require_tested: Vec::new(),
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    let compiled = CompiledRules::compile(rules, "test".to_string()).unwrap();
    let (name, order) = compiled.layer_of("src/sub/deep.rs").expect("assigned");
    assert_eq!((name, order), ("first", 0), "declaration order wins");
    assert!(compiled.layer_of("other/x.rs").is_none(), "unassigned");
}

// ── FR-RC-03/04 / NFR-RA-11: the freshness line ─────────────────────────────

#[test]
fn freshness_line_carries_count_head_and_unresolved() {
    let fresh = Freshness {
        reconciled: 3,
        head: Some("abc123".to_string()),
        unresolved: 7,
        ..Freshness::default()
    };
    assert_eq!(
        fresh.line(),
        "reconciled 3 files · HEAD abc123 · 7 unresolved refs"
    );
}

#[test]
fn freshness_line_reports_no_git_outside_a_repo() {
    let fresh = Freshness {
        reconciled: 0,
        head: None,
        unresolved: 0,
        ..Freshness::default()
    };
    assert!(fresh.line().contains("HEAD no-git"), "{}", fresh.line());
}

#[test]
fn freshness_line_marks_assumed_fresh_under_no_reconcile() {
    let fresh = Freshness {
        assumed: true,
        head: Some("abc123".to_string()),
        unresolved: 2,
        ..Freshness::default()
    };
    let line = fresh.line();
    assert!(line.starts_with("assumed-fresh (--no-reconcile)"), "{line}");
    assert!(!line.contains("reconciled"), "no reconcile count: {line}");
}

/// NFR-RA-11: per-file failures stamp INCOMPLETE prominently, name the
/// files, and truncate a long list.
#[test]
fn freshness_line_stamps_incomplete_on_partial_failure() {
    let fresh = Freshness {
        reconciled: 5,
        head: Some("abc123".to_string()),
        unresolved: 1,
        failed: (1..=7).map(|i| format!("src/bad{i}.rs")).collect(),
        ..Freshness::default()
    };
    let line = fresh.line();
    assert!(line.starts_with("INCOMPLETE — "), "prominent: {line}");
    assert!(line.contains("7 files failed to sync"), "{line}");
    assert!(line.contains("src/bad1.rs"), "names the files: {line}");
    assert!(line.contains("… 2 more"), "truncates past 5: {line}");
}

// ── FR-GV-05: per-metric regression detail ──────────────────────────────────

#[test]
fn metric_regressions_report_real_drops_and_ignore_noise() {
    let base = MetricSnapshotRow {
        id: 1,
        created_at: 0,
        commit_sha: None,
        node_count: 10,
        edge_count: 10,
        function_count: 5,
        test_function_count: 0,
        metric_version: crate::metrics::METRIC_SEMANTICS_VERSION,
        empty: false,
        modularity_raw: 0.4,
        modularity_normalized: 0.6,
        acyclicity_raw: 0.0,
        acyclicity_normalized: 1.0,
        depth_raw: 2.0,
        depth_normalized: 0.8,
        equality_raw: 0.0,
        equality_normalized: 1.0,
        redundancy_raw: 0.0,
        redundancy_normalized: 1.0,
        thresholds_hash: None,
        aggregate_signal: Some(8000),
    };
    let current = MetricSnapshot {
        modularity: crate::models::quality::MetricValue {
            raw: 0.1,
            normalized: 0.4, // real drop
        },
        acyclicity: crate::models::quality::MetricValue {
            raw: 0.0,
            normalized: 1.0 - 1e-12, // float residue, not a regression
        },
        depth: crate::models::quality::MetricValue {
            raw: 2.0,
            normalized: 0.8,
        },
        equality: crate::models::quality::MetricValue {
            raw: 0.0,
            normalized: 1.0,
        },
        redundancy: crate::models::quality::MetricValue {
            raw: 0.5,
            normalized: 0.5, // real drop
        },
        ..MetricSnapshot::default()
    };

    let regressions = metric_regressions(&base, &current);
    let names: Vec<&str> = regressions.iter().map(|r| r.metric.as_str()).collect();
    assert_eq!(
        names,
        ["modularity", "redundancy"],
        "which metric(s) moved, in canonical order"
    );
    assert!(regressions[0].delta < 0.0, "delta is signed (negative)");
}

// FR-GV-08 test marking is no longer derived here: `test_gaps` reads the
// persisted `is_test` annotation column ([FR-AN-05], CR-001), the single
// source of truth the annotation pass computes. The marking predicate
// (evidence ∨ path convention ∨ `[semantics].test_markers` affix) and its
// negatives are unit-tested at its new home in `crate::annotate`
// (`is_test_marked_*` tests); the end-to-end `test_gaps` ⇒ `is_test` parity is
// covered by the store-backed integration test in `tests/governance.rs`.

// ── FR-GV-06: evolution deltas ──────────────────────────────────────────────

#[test]
fn evolution_points_carry_signal_and_metric_deltas() {
    let mut first = MetricSnapshotRow {
        id: 1,
        created_at: 100,
        commit_sha: Some("aaa".to_string()),
        node_count: 10,
        edge_count: 10,
        function_count: 5,
        test_function_count: 0,
        metric_version: crate::metrics::METRIC_SEMANTICS_VERSION,
        empty: false,
        modularity_raw: 0.4,
        modularity_normalized: 0.6,
        acyclicity_raw: 0.0,
        acyclicity_normalized: 1.0,
        depth_raw: 2.0,
        depth_normalized: 0.8,
        equality_raw: 0.0,
        equality_normalized: 1.0,
        redundancy_raw: 0.0,
        redundancy_normalized: 1.0,
        thresholds_hash: None,
        aggregate_signal: Some(8000),
    };

    let head = evolution_point(&first, None);
    assert_eq!(head.snapshot_id, 1);
    assert_eq!(head.signal, Some(8000));
    assert_eq!(head.signal_delta, None, "no predecessor → no delta");
    assert!(head.metric_deltas.iter().all(|d| d.delta.is_none()));

    let mut second = first.clone();
    second.id = 2;
    second.aggregate_signal = Some(7900);
    second.modularity_normalized = 0.5;
    first.id = 1;

    let point = evolution_point(&second, Some(&first));
    assert_eq!(point.signal_delta, Some(-100), "signed signal movement");
    let modularity = &point.metric_deltas[0];
    assert_eq!(modularity.metric, "modularity");
    assert!(
        (modularity.delta.unwrap() - (-0.1)).abs() < 1e-9,
        "per-metric delta: {:?}",
        modularity.delta
    );
}

// ── FR-GV-07: dsm granularity parsing ───────────────────────────────────────

#[test]
fn dsm_granularity_parses_module_and_file_only() {
    assert_eq!("module".parse(), Ok(DsmGranularity::Module));
    assert_eq!("file".parse(), Ok(DsmGranularity::File));
    assert!("bogus".parse::<DsmGranularity>().is_err());
    assert_eq!(DsmGranularity::default(), DsmGranularity::Module);
}

/// The DSM row → layer mapping path: file keys map directly, module keys
/// only through their `file:` fallback form.
#[test]
fn dsm_key_paths_resolve_per_granularity() {
    assert_eq!(
        dsm_key_path("src/a.rs", DsmGranularity::File),
        Some("src/a.rs")
    );
    assert_eq!(dsm_key_path("<unbound>", DsmGranularity::File), None);
    assert_eq!(
        dsm_key_path("file:src/a.rs", DsmGranularity::Module),
        Some("src/a.rs")
    );
    assert_eq!(
        dsm_key_path("module:crate::m", DsmGranularity::Module),
        None
    );
}

// ── FR-GV-12 / CR-002: [[forbidden_imports]] glob-level import linter ────────

/// An `Imports` edge whose source file matches `from` and whose target matches
/// `to` is reported with the `reason`; only the import edge kind triggers it.
#[test]
fn forbidden_import_edge_is_reported_with_reason() {
    let compiled = forbidden_import_rules();
    let nodes = [
        node(1, "handler", Some("src/web/handler.rs")),
        node(2, "query", Some("src/db/query.rs")),
    ];
    let edges = [kind_edge(1, 2, EdgeKind::Imports)];
    let (violations, checked) = run_eval(&compiled, &nodes, &edges, &[], 0);

    assert_eq!(checked, 1, "one active forbidden_imports rule");
    assert_eq!(violations.len(), 1, "exactly one banned import");
    let v = &violations[0];
    assert_eq!(v.rule, "forbidden_import:src/web/**->src/db/**");
    assert_eq!(
        v.rule_type, "boundary",
        "a forbidden import reuses the boundary rule_type"
    );
    assert_eq!(
        v.severity, "error",
        "every CR-002 violation is blocking (BR-20)"
    );
    assert_eq!(v.file, "src/web/handler.rs", "the importing file is named");
    assert!(
        v.message.contains("src/web/handler.rs")
            && v.message.contains("src/db/query.rs")
            && v.message
                .contains("the web layer must not import the db directly"),
        "message names both files and the reason: {}",
        v.message
    );
}

/// A `References` edge (not only `Imports`) across the glob pair is also a
/// violation — the linter acts on both reference-kind edges ([FR-GV-12]).
#[test]
fn forbidden_import_matches_references_too() {
    let compiled = forbidden_import_rules();
    let nodes = [
        node(1, "handler", Some("src/web/handler.rs")),
        node(2, "Row", Some("src/db/row.rs")),
    ];
    let edges = [kind_edge(1, 2, EdgeKind::References)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert_eq!(violations.len(), 1, "a References edge is banned too");
    assert!(violations[0].rule.starts_with("forbidden_import:"));
}

/// An import that does not match both globs is not flagged, and a `Calls` edge
/// across the same files is ignored (the linter is import/reference-only).
#[test]
fn unmatched_import_and_calls_edge_are_not_flagged() {
    let compiled = forbidden_import_rules();
    let nodes = [
        node(1, "handler", Some("src/web/handler.rs")),
        node(2, "query", Some("src/db/query.rs")),
        node(3, "util", Some("src/util/mod.rs")),
    ];
    let edges = [
        kind_edge(1, 3, EdgeKind::Imports), // target off-glob
        kind_edge(1, 2, EdgeKind::Calls),   // not an import/reference
        kind_edge(3, 2, EdgeKind::Imports), // source off-glob
    ];
    let (violations, checked) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert_eq!(checked, 1, "the rule is still counted as active");
    assert!(
        violations.is_empty(),
        "nothing matches both globs on an import edge"
    );
}

/// An unbound endpoint (no file path — e.g. an external/unresolved target) is
/// not a resolved intra-workspace edge, so v1 does not flag it ([CR-002] CRA-01).
#[test]
fn forbidden_import_skips_unbound_endpoints() {
    let compiled = forbidden_import_rules();
    let nodes = [
        node(1, "handler", Some("src/web/handler.rs")),
        node(2, "external", None),
    ];
    let edges = [kind_edge(1, 2, EdgeKind::Imports)];
    let (violations, _) = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert!(
        violations.is_empty(),
        "an edge into an unbound (external) target is out of v1 scope"
    );
}

/// Re-running the evaluator over identical inputs yields byte-identical
/// violations, including with multiple banned edges ([NFR-RA-06]).
#[test]
fn forbidden_import_evaluation_is_deterministic() {
    let compiled = forbidden_import_rules();
    let nodes = [
        node(1, "a", Some("src/web/a.rs")),
        node(2, "b", Some("src/web/b.rs")),
        node(3, "q", Some("src/db/q.rs")),
        node(4, "r", Some("src/db/r.rs")),
    ];
    let edges = [
        kind_edge(1, 3, EdgeKind::Imports),
        kind_edge(2, 4, EdgeKind::References),
        kind_edge(1, 4, EdgeKind::Imports),
    ];
    let first = run_eval(&compiled, &nodes, &edges, &[], 0);
    let second = run_eval(&compiled, &nodes, &edges, &[], 0);
    assert_eq!(
        first.0, second.0,
        "violations are byte-identical across runs"
    );
    assert_eq!(first.0.len(), 3, "all three banned edges reported");
}

// ── FR-GV-13 / CR-002: [[require_tested]] coverage contract ──────────────────

/// An `AnnotationNodeRow` carrying just the fields `[[require_tested]]` reads:
/// `id`, `kind`, `name`, the `exported` flag, and the defining `file`.
fn annot(
    id: i64,
    name: &str,
    kind: NodeKind,
    exported: bool,
    file: Option<&str>,
) -> AnnotationNodeRow {
    AnnotationNodeRow {
        id: NodeId(id),
        kind,
        name: name.to_string(),
        exported,
        derived: false,
        fingerprint: None,
        test_evidence: false,
        file_id: None,
        file_path: file.map(str::to_string),
        is_dead: None,
        is_duplicate: None,
        is_test: false,
        layer_membership: None,
        clone_group: None,
    }
}

/// Compile a contract carrying a single `[[require_tested]]` contract over
/// `src/api/**` and nothing else ([FR-GV-13]).
fn require_tested_rules() -> CompiledRules {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: Vec::new(),
        require_tested: vec![RequireTested {
            paths: vec!["src/api/**".to_string()],
            reason: Some("public API must have a test path".to_string()),
        }],
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    CompiledRules::compile(rules, "test".to_string()).expect("test globs compile")
}

/// Evaluate with the `[[require_tested]]` inputs supplied (annotation rows for
/// the `exported` flag, `Calls` edges, and the `is_test` seed ids).
fn run_eval_require_tested(
    compiled: &CompiledRules,
    annotations: &[AnnotationNodeRow],
    edges: &[EdgeRow],
    test_node_ids: &[NodeId],
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes: &[],
        edges,
        functions: &[],
        function_metrics: &[],
        annotations,
        test_node_ids,
        cycles: 0,
        thresholds: crate::metrics::Thresholds::default(),
    })
}

/// AC1 + AC3: an exported, test-unreachable function under a `paths` glob is
/// reported with its `reason`, and the violation surfaces the
/// static-reachability caveat ([FR-GV-13], [FR-GV-08]).
#[test]
fn exported_unreached_function_under_paths_is_reported_with_reason_and_caveat() {
    let compiled = require_tested_rules();
    let annotations = [annot(
        1,
        "create_user",
        NodeKind::Function,
        true,
        Some("src/api/users.rs"),
    )];
    let (violations, checked) = run_eval_require_tested(&compiled, &annotations, &[], &[]);

    assert_eq!(checked, 1, "one active require_tested contract");
    assert_eq!(violations.len(), 1, "the unreached exported fn is flagged");
    let v = &violations[0];
    assert_eq!(v.rule, "require_tested:src/api/**");
    assert_eq!(
        v.rule_type, "constraint",
        "a require_tested gap reuses the constraint rule_type (CR-002 no migration)"
    );
    assert_eq!(
        v.severity, "error",
        "every CR-002 violation is blocking (BR-20)"
    );
    assert_eq!(v.file, "src/api/users.rs", "the offending file is named");
    assert_eq!(v.node_id, Some(1), "the violation points at the symbol");
    assert!(
        v.message.contains("create_user") && v.message.contains("public API must have a test path"),
        "message names the symbol and the reason: {}",
        v.message
    );
    // AC3 ([FR-GV-08], BR-16): the static-reachability caveat is surfaced inline.
    assert!(
        v.message.contains("not execution coverage") && v.message.contains("dynamic-dispatch"),
        "the static-reachability caveat is surfaced: {}",
        v.message
    );
}

/// AC2 (pass side): an exported function transitively reached from a test node
/// over `calls` passes ([FR-GV-13] reusing the [FR-GV-08] BFS).
#[test]
fn exported_function_reached_from_a_test_passes() {
    let compiled = require_tested_rules();
    // Node 10 is the test seed; it calls node 1 (exported, under src/api).
    let annotations = [
        annot(
            1,
            "create_user",
            NodeKind::Function,
            true,
            Some("src/api/users.rs"),
        ),
        annot(
            10,
            "test_create_user",
            NodeKind::Function,
            true,
            Some("tests/users.rs"),
        ),
    ];
    let edges = [edge(10, 1)];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &edges, &[NodeId(10)]);
    assert!(
        violations.is_empty(),
        "a transitively test-reached exported fn passes: {violations:?}"
    );
}

/// Transitive reach through a non-test intermediary covers the whole chain —
/// the BFS is not limited to direct test callees ([FR-GV-08]).
#[test]
fn transitive_reach_through_an_intermediary_covers_the_target() {
    let compiled = require_tested_rules();
    // test(10) → service(1) → repo(2): both exported under src/api, both reached.
    let annotations = [
        annot(
            1,
            "service",
            NodeKind::Function,
            true,
            Some("src/api/svc.rs"),
        ),
        annot(2, "repo", NodeKind::Method, true, Some("src/api/repo.rs")),
        annot(
            10,
            "test_svc",
            NodeKind::Function,
            true,
            Some("tests/svc.rs"),
        ),
    ];
    let edges = [edge(10, 1), edge(1, 2)];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &edges, &[NodeId(10)]);
    assert!(
        violations.is_empty(),
        "transitive BFS covers the whole chain: {violations:?}"
    );
}

/// AC2 (exempt side): a non-exported function under the glob is exempt — the
/// contract enforces a public-API test path, not total coverage ([FR-GV-13]).
#[test]
fn non_exported_function_under_paths_is_exempt() {
    let compiled = require_tested_rules();
    let annotations = [annot(
        1,
        "internal_helper",
        NodeKind::Function,
        false,
        Some("src/api/users.rs"),
    )];
    let (violations, checked) = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert_eq!(checked, 1, "the contract is still counted as active");
    assert!(
        violations.is_empty(),
        "a non-exported symbol is exempt: {violations:?}"
    );
}

/// An exported function whose file is off the `paths` glob is not covered by
/// the contract, even when unreached ([FR-GV-13]).
#[test]
fn exported_function_off_glob_is_exempt() {
    let compiled = require_tested_rules();
    let annotations = [annot(
        1,
        "main",
        NodeKind::Function,
        true,
        Some("src/bin/main.rs"),
    )];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert!(
        violations.is_empty(),
        "a symbol off the paths glob is exempt: {violations:?}"
    );
}

/// A non-callable node (a struct) under the glob is not subject to the
/// contract, which targets Functions/Methods only ([FR-GV-13]).
#[test]
fn non_callable_node_under_paths_is_exempt() {
    let compiled = require_tested_rules();
    let annotations = [annot(
        1,
        "User",
        NodeKind::Struct,
        true,
        Some("src/api/users.rs"),
    )];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert!(
        violations.is_empty(),
        "a Type node is not a Function/Method, so it is exempt: {violations:?}"
    );
}

/// NFR-RA-06: re-running the evaluator yields byte-identical violations, and
/// the per-symbol findings come back in node-id order.
#[test]
fn require_tested_evaluation_is_deterministic() {
    let compiled = require_tested_rules();
    let annotations = [
        annot(1, "a", NodeKind::Function, true, Some("src/api/a.rs")),
        annot(2, "b", NodeKind::Method, true, Some("src/api/b.rs")),
        annot(3, "c", NodeKind::Function, true, Some("src/api/c.rs")),
    ];
    let first = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    let second = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert_eq!(
        first.0, second.0,
        "violations are byte-identical across runs (NFR-RA-06)"
    );
    assert_eq!(
        first.0.len(),
        3,
        "all three unreached exported symbols reported"
    );
    let ids: Vec<_> = first.0.iter().map(|v| v.node_id).collect();
    assert_eq!(
        ids,
        vec![Some(1), Some(2), Some(3)],
        "findings are emitted in node-id order"
    );
}

/// Omitting `[[require_tested]]` enforces nothing and counts no active rule.
#[test]
fn require_tested_omitted_enforces_nothing() {
    let compiled = constraints_only(Constraints::default());
    let annotations = [annot(
        1,
        "x",
        NodeKind::Function,
        true,
        Some("src/api/x.rs"),
    )];
    let (violations, checked) = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert!(violations.is_empty(), "no contract → nothing enforced");
    assert_eq!(checked, 0, "no active rules");
}

/// Compile a `[[require_tested]]`-only contract from explicit entries — for
/// the `reason: None` and multi-contract cases the single-contract
/// `require_tested_rules` helper cannot express.
fn require_tested_rules_from(entries: Vec<RequireTested>) -> CompiledRules {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: Vec::new(),
        require_tested: entries,
        require_documented: Vec::new(),
        history: Default::default(),
        coverage: Default::default(),
    };
    CompiledRules::compile(rules, "test".to_string()).expect("test globs compile")
}

/// A contract with no `reason` produces a clean message — no trailing ` — `
/// separator — while still naming the symbol and carrying the caveat.
#[test]
fn require_tested_violation_without_a_reason_has_no_trailing_separator() {
    let compiled = require_tested_rules_from(vec![RequireTested {
        paths: vec!["src/api/**".to_string()],
        reason: None,
    }]);
    let annotations = [annot(
        1,
        "orphan",
        NodeKind::Function,
        true,
        Some("src/api/x.rs"),
    )];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &[], &[]);

    assert_eq!(violations.len(), 1, "the uncovered exported fn is flagged");
    let v = &violations[0];
    assert!(
        !v.message.contains(" — "),
        "no reason → no trailing separator: {}",
        v.message
    );
    assert!(
        v.message.contains("orphan") && v.message.contains("not execution coverage"),
        "the symbol and the caveat are still present: {}",
        v.message
    );
}

/// An exported function with no bound file path carries no path to glob-match,
/// so it is silently exempt (the analogue of `forbidden_import_skips_unbound_endpoints`).
#[test]
fn exported_function_with_no_file_path_is_exempt() {
    let compiled = require_tested_rules();
    let annotations = [annot(1, "unbound", NodeKind::Function, true, None)];
    let (violations, _) = run_eval_require_tested(&compiled, &annotations, &[], &[]);
    assert!(
        violations.is_empty(),
        "an unbound (no file_path) symbol cannot be glob-matched: {violations:?}"
    );
}

/// Two `[[require_tested]]` contracts over disjoint globs are evaluated
/// independently — each produces its own violation with a distinct rule key,
/// and both count toward `checked` ([NFR-RA-06] declaration order).
#[test]
fn multiple_require_tested_contracts_are_evaluated_independently() {
    let compiled = require_tested_rules_from(vec![
        RequireTested {
            paths: vec!["src/api/**".to_string()],
            reason: Some("api needs tests".to_string()),
        },
        RequireTested {
            paths: vec!["src/cli/**".to_string()],
            reason: Some("cli needs tests".to_string()),
        },
    ]);
    let annotations = [
        annot(1, "api_fn", NodeKind::Function, true, Some("src/api/a.rs")),
        annot(2, "cli_fn", NodeKind::Function, true, Some("src/cli/b.rs")),
    ];
    let (violations, checked) = run_eval_require_tested(&compiled, &annotations, &[], &[]);

    assert_eq!(checked, 2, "both contracts are active");
    assert_eq!(violations.len(), 2, "each contract flags its own symbol");
    // Outer loop is declaration order: api contract first, then cli.
    assert_eq!(violations[0].rule, "require_tested:src/api/**");
    assert!(violations[0].message.contains("api_fn"));
    assert_eq!(violations[1].rule, "require_tested:src/cli/**");
    assert!(violations[1].message.contains("cli_fn"));
}

// ── FR-GV-14 / FR-GV-15 / CR-003: [[require_documented]] contract ────────────

/// A `NodeRow` of kind `kind` named `name` in `file` — for the documentation
/// source nodes (`DocSection`/`DocFile`) the documented-set core reads.
fn node_of(id: i64, name: &str, kind: NodeKind, file: Option<&str>) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("test symbol"),
        kind,
        name: name.to_string(),
        file_path: file.map(str::to_string),
        start_line: Some(1),
        end_line: Some(2),
    }
}

/// A `DocReference` edge `source → target` — the doc-graph edge the
/// documented-set core counts when its source is a `DocSection`.
fn doc_ref_edge(source: i64, target: i64) -> EdgeRow {
    kind_edge(source, target, EdgeKind::DocReference)
}

/// Compile a contract carrying a single `[[require_documented]]` contract over
/// `src/api/**` and nothing else ([FR-GV-15]).
fn require_documented_rules() -> CompiledRules {
    require_documented_rules_from(vec![RequireDocumented {
        paths: vec!["src/api/**".to_string()],
        reason: Some("public API must be documented".to_string()),
    }])
}

/// Compile a `[[require_documented]]`-only contract from explicit entries.
fn require_documented_rules_from(entries: Vec<RequireDocumented>) -> CompiledRules {
    let rules = Rules {
        constraints: Constraints::default(),
        metric_thresholds: Default::default(),
        layers: Vec::new(),
        boundaries: Vec::new(),
        forbidden_imports: Vec::new(),
        require_tested: Vec::new(),
        require_documented: entries,
        history: Default::default(),
        coverage: Default::default(),
    };
    CompiledRules::compile(rules, "test".to_string()).expect("test globs compile")
}

/// Evaluate with the `[[require_documented]]` inputs supplied: `nodes` (for the
/// `DocSection` sources), annotation rows (for the `exported` flag), and
/// `DocReference` edges.
fn run_eval_require_documented(
    compiled: &CompiledRules,
    nodes: &[NodeRow],
    annotations: &[AnnotationNodeRow],
    edges: &[EdgeRow],
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes,
        edges,
        functions: &[],
        function_metrics: &[],
        annotations,
        test_node_ids: &[],
        cycles: 0,
        thresholds: crate::metrics::Thresholds::default(),
    })
}

/// AC ([FR-GV-15]): an exported, undocumented function under a `paths` glob is
/// reported with its `reason`, reusing the `constraint` rule_type, and the
/// violation surfaces the reference-presence caveat.
#[test]
fn exported_undocumented_function_under_paths_is_reported_with_reason_and_caveat() {
    let compiled = require_documented_rules();
    let annotations = [annot(
        1,
        "create_user",
        NodeKind::Function,
        true,
        Some("src/api/users.rs"),
    )];
    let (violations, checked) = run_eval_require_documented(&compiled, &[], &annotations, &[]);

    assert_eq!(checked, 1, "one active require_documented contract");
    assert_eq!(
        violations.len(),
        1,
        "the undocumented exported fn is flagged"
    );
    let v = &violations[0];
    assert_eq!(v.rule, "require_documented:src/api/**");
    assert_eq!(
        v.rule_type, "constraint",
        "a require_documented gap reuses the constraint rule_type (CR-003 no migration)"
    );
    assert_eq!(v.severity, "error", "every doc-governance violation blocks");
    assert_eq!(v.file, "src/api/users.rs", "the offending file is named");
    assert_eq!(v.node_id, Some(1), "the violation points at the symbol");
    assert!(
        v.message.contains("create_user") && v.message.contains("public API must be documented"),
        "message names the symbol and the reason: {}",
        v.message
    );
    assert!(
        v.message
            .contains("reference presence, not documentation quality"),
        "the honesty caveat is surfaced inline: {}",
        v.message
    );
}

/// AC (pass side, [FR-GV-15]): an exported function a `DocSection` references
/// over a `DocReference` edge passes — reusing the same `documented_set` core
/// `doc_gaps` builds ([FR-GV-14]).
#[test]
fn exported_function_referenced_by_a_docsection_passes() {
    let compiled = require_documented_rules();
    // Node 100 is a DocSection; it references node 1 (exported, under src/api).
    let nodes = [node_of(
        100,
        "Users API",
        NodeKind::DocSection,
        Some("docs/api.md"),
    )];
    let annotations = [annot(
        1,
        "create_user",
        NodeKind::Function,
        true,
        Some("src/api/users.rs"),
    )];
    let edges = [doc_ref_edge(100, 1)];
    let (violations, _) = run_eval_require_documented(&compiled, &nodes, &annotations, &edges);
    assert!(
        violations.is_empty(),
        "a DocSection-referenced exported fn passes: {violations:?}"
    );
}

/// Only `DocSection`-sourced references document a symbol ([FR-GV-14]/[FR-GV-15]):
/// a reference from a file-level preamble is sourced from the enclosing
/// `DocFile`, so it does not satisfy the contract.
#[test]
fn reference_sourced_from_a_docfile_does_not_document() {
    let compiled = require_documented_rules();
    // Node 100 is a DocFile (a preamble source), not a DocSection.
    let nodes = [node_of(
        100,
        "api.md",
        NodeKind::DocFile,
        Some("docs/api.md"),
    )];
    let annotations = [annot(
        1,
        "create_user",
        NodeKind::Function,
        true,
        Some("src/api/users.rs"),
    )];
    let edges = [doc_ref_edge(100, 1)];
    let (violations, _) = run_eval_require_documented(&compiled, &nodes, &annotations, &edges);
    assert_eq!(
        violations.len(),
        1,
        "a DocFile-sourced reference does not document a symbol: {violations:?}"
    );
    assert!(violations[0].message.contains("create_user"));
}

/// AC (exempt side, [FR-GV-15]): a non-exported function under the glob is
/// exempt — the contract enforces a public-API documentation gate, not total
/// documentation.
#[test]
fn non_exported_function_under_paths_is_exempt_from_documentation() {
    let compiled = require_documented_rules();
    let annotations = [annot(
        1,
        "internal_helper",
        NodeKind::Function,
        false,
        Some("src/api/users.rs"),
    )];
    let (violations, checked) = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert_eq!(checked, 1, "the contract is still counted as active");
    assert!(
        violations.is_empty(),
        "a non-exported symbol is exempt: {violations:?}"
    );
}

/// A derived (policy-materialised) node under the glob is exempt even when its
/// annotation row reads `exported` — the contract skips `derived` rows
/// defensively ([FR-GV-15]; a Function/Method is never derived in practice,
/// only `Layer`/`Boundary` are, but the guard mirrors `[[require_tested]]`).
#[test]
fn derived_node_under_paths_is_exempt_from_documentation() {
    let compiled = require_documented_rules();
    let mut ann = annot(1, "phantom", NodeKind::Function, true, Some("src/api/x.rs"));
    ann.derived = true;
    let (violations, _) = run_eval_require_documented(&compiled, &[], &[ann], &[]);
    assert!(
        violations.is_empty(),
        "a derived row is exempt regardless of its exported flag: {violations:?}"
    );
}

/// An exported function whose file is off the `paths` glob is not covered by the
/// contract, even when undocumented ([FR-GV-15]).
#[test]
fn exported_function_off_glob_is_exempt_from_documentation() {
    let compiled = require_documented_rules();
    let annotations = [annot(
        1,
        "main",
        NodeKind::Function,
        true,
        Some("src/bin/main.rs"),
    )];
    let (violations, _) = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert!(
        violations.is_empty(),
        "a symbol off the paths glob is exempt: {violations:?}"
    );
}

/// A non-callable node (a struct) under the glob is not subject to the contract,
/// which targets Functions/Methods only ([FR-GV-15]).
#[test]
fn non_callable_node_under_paths_is_exempt_from_documentation() {
    let compiled = require_documented_rules();
    let annotations = [annot(
        1,
        "User",
        NodeKind::Struct,
        true,
        Some("src/api/users.rs"),
    )];
    let (violations, _) = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert!(
        violations.is_empty(),
        "a Type node is not a Function/Method, so it is exempt: {violations:?}"
    );
}

/// NFR-RA-06: re-running the evaluator yields byte-identical violations, and the
/// per-symbol findings come back in node-id order.
#[test]
fn require_documented_evaluation_is_deterministic() {
    let compiled = require_documented_rules();
    let annotations = [
        annot(1, "a", NodeKind::Function, true, Some("src/api/a.rs")),
        annot(2, "b", NodeKind::Method, true, Some("src/api/b.rs")),
        annot(3, "c", NodeKind::Function, true, Some("src/api/c.rs")),
    ];
    let first = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    let second = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert_eq!(
        first.0, second.0,
        "violations are byte-identical across runs (NFR-RA-06)"
    );
    assert_eq!(
        first.0.len(),
        3,
        "all three undocumented exported symbols reported"
    );
    let ids: Vec<_> = first.0.iter().map(|v| v.node_id).collect();
    assert_eq!(
        ids,
        vec![Some(1), Some(2), Some(3)],
        "findings are emitted in node-id order"
    );
}

/// Omitting `[[require_documented]]` enforces nothing and counts no active rule
/// ([FR-GV-15] "an absent contract enforces nothing").
#[test]
fn require_documented_omitted_enforces_nothing() {
    let compiled = constraints_only(Constraints::default());
    let annotations = [annot(
        1,
        "x",
        NodeKind::Function,
        true,
        Some("src/api/x.rs"),
    )];
    let (violations, checked) = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert!(violations.is_empty(), "no contract → nothing enforced");
    assert_eq!(checked, 0, "no active rules");
}

/// A contract with no `reason` produces a clean message — no trailing ` — `
/// separator — while still naming the symbol and carrying the caveat.
#[test]
fn require_documented_violation_without_a_reason_has_no_trailing_separator() {
    let compiled = require_documented_rules_from(vec![RequireDocumented {
        paths: vec!["src/api/**".to_string()],
        reason: None,
    }]);
    let annotations = [annot(
        1,
        "orphan",
        NodeKind::Function,
        true,
        Some("src/api/x.rs"),
    )];
    let (violations, _) = run_eval_require_documented(&compiled, &[], &annotations, &[]);

    assert_eq!(
        violations.len(),
        1,
        "the undocumented exported fn is flagged"
    );
    let v = &violations[0];
    assert!(
        !v.message.contains(" — "),
        "no reason → no trailing separator: {}",
        v.message
    );
    assert!(
        v.message.contains("orphan") && v.message.contains("reference presence"),
        "the symbol and the caveat are still present: {}",
        v.message
    );
}

/// An exported function with no bound file path carries no path to glob-match,
/// so it is silently exempt.
#[test]
fn exported_function_with_no_file_path_is_exempt_from_documentation() {
    let compiled = require_documented_rules();
    let annotations = [annot(1, "unbound", NodeKind::Function, true, None)];
    let (violations, _) = run_eval_require_documented(&compiled, &[], &annotations, &[]);
    assert!(
        violations.is_empty(),
        "an unbound (no file_path) symbol cannot be glob-matched: {violations:?}"
    );
}

/// Two `[[require_documented]]` contracts over disjoint globs are evaluated
/// independently — each produces its own violation with a distinct rule key, and
/// both count toward `checked` ([NFR-RA-06] declaration order).
#[test]
fn multiple_require_documented_contracts_are_evaluated_independently() {
    let compiled = require_documented_rules_from(vec![
        RequireDocumented {
            paths: vec!["src/api/**".to_string()],
            reason: Some("api needs docs".to_string()),
        },
        RequireDocumented {
            paths: vec!["src/cli/**".to_string()],
            reason: Some("cli needs docs".to_string()),
        },
    ]);
    let annotations = [
        annot(1, "api_fn", NodeKind::Function, true, Some("src/api/a.rs")),
        annot(2, "cli_fn", NodeKind::Function, true, Some("src/cli/b.rs")),
    ];
    let (violations, checked) = run_eval_require_documented(&compiled, &[], &annotations, &[]);

    assert_eq!(checked, 2, "both contracts are active");
    assert_eq!(violations.len(), 2, "each contract flags its own symbol");
    assert_eq!(violations[0].rule, "require_documented:src/api/**");
    assert!(violations[0].message.contains("api_fn"));
    assert_eq!(violations[1].rule, "require_documented:src/cli/**");
    assert!(violations[1].message.contains("cli_fn"));
}

// ── CR-005 / FR-GV-11 ext. / UAT-GV-08: the four structural budgets ──────────

/// A `NodeRow` of an arbitrary kind (the god-container budget needs `Class`
/// containers and `Method` members, not just `Function`s).
fn kinded_node(id: i64, name: &str, file: Option<&str>, kind: NodeKind) -> NodeRow {
    NodeRow {
        id: NodeId(id),
        symbol: LogosSymbol::parse(&format!("local {id}")).expect("test symbol"),
        kind,
        name: name.to_string(),
        file_path: file.map(str::to_string),
        start_line: Some(1),
        end_line: Some(2),
    }
}

/// A `FunctionMetricRow` carrying the CR-005 structural facts (CC, LOC, nesting,
/// clone group) the new budgets read; dead/duplicate are irrelevant here.
fn struct_row(
    id: i64,
    cc: Option<i64>,
    loc: Option<i64>,
    nest: Option<i64>,
    clone: Option<i64>,
) -> FunctionMetricRow {
    FunctionMetricRow {
        id: NodeId(id),
        cyclomatic_complexity: cc,
        is_dead: None,
        is_duplicate: None,
        line_count: loc,
        max_nesting_depth: nest,
        clone_group: clone.map(NodeId),
    }
}

/// Evaluate the CR-005 budgets over a full structural input bundle.
fn run_eval_budgets(
    compiled: &CompiledRules,
    nodes: &[NodeRow],
    edges: &[EdgeRow],
    function_metrics: &[FunctionMetricRow],
    test_node_ids: &[NodeId],
    thresholds: crate::metrics::Thresholds,
) -> (Vec<Violation>, u32) {
    evaluate(&EvalInput {
        compiled,
        nodes,
        edges,
        functions: &[],
        function_metrics,
        annotations: &[],
        test_node_ids,
        cycles: 0,
        thresholds,
    })
}

/// A `Class` container with `methods` production members (Contains edges) — a
/// god-by-method-count container when `methods >= T_m`.
fn god_container_fixture(class_id: i64, methods: i64) -> (Vec<NodeRow>, Vec<EdgeRow>) {
    let mut nodes = vec![kinded_node(
        class_id,
        "BigClass",
        Some("src/big.rs"),
        NodeKind::Class,
    )];
    let mut edges = Vec::new();
    for m in 0..methods {
        let mid = class_id + 1 + m;
        nodes.push(kinded_node(mid, "m", Some("src/big.rs"), NodeKind::Method));
        edges.push(kind_edge(class_id, mid, EdgeKind::Contains));
    }
    (nodes, edges)
}

#[test]
fn effective_thresholds_falls_back_to_defaults_for_omitted_keys() {
    use crate::config::MetricThresholds;
    let d = crate::metrics::Thresholds::default();

    // An empty table is all-defaults.
    let none = effective_thresholds(&Rules::default());
    assert_eq!(none, d, "an omitted table keeps every documented default");

    // A partial table overrides only its keys; the rest stay at the defaults.
    let rules = Rules {
        metric_thresholds: MetricThresholds {
            nesting_depth: Some(5),
            god_methods: Some(30),
            ..MetricThresholds::default()
        },
        ..Rules::default()
    };
    let t = effective_thresholds(&rules);
    assert_eq!(t.nest, 5, "the overridden T_nest applies");
    assert_eq!(t.god_methods, 30, "the overridden T_m applies");
    assert_eq!(
        t.brain_cc, d.brain_cc,
        "an omitted key falls back to default"
    );
    assert_eq!(t.brain_loc, d.brain_loc);
    assert_eq!(t.brain_nest, d.brain_nest);
    assert_eq!(t.god_span, d.god_span);
}

#[test]
fn max_nesting_depth_budget_flags_deep_production_functions() {
    let compiled = constraints_only(Constraints {
        max_nesting_depth: Some(4),
        ..Constraints::default()
    });
    let nodes = [
        node(1, "deep", Some("src/a.rs")),
        node(2, "shallow", Some("src/b.rs")),
    ];
    // id 1 nests 5 (> 4 → violates); id 2 nests 4 (not > 4 → ok).
    let metrics = [
        struct_row(1, None, None, Some(5), None),
        struct_row(2, None, None, Some(4), None),
    ];
    let (violations, checked) =
        run_eval_budgets(&compiled, &nodes, &[], &metrics, &[], Default::default());

    assert_eq!(checked, 1, "the budget is the one active rule");
    assert_eq!(
        violations.len(),
        1,
        "only the over-budget function is flagged"
    );
    let v = &violations[0];
    assert_eq!(v.rule, "max_nesting_depth");
    assert_eq!(v.rule_type, "constraint");
    assert_eq!(v.severity, "error");
    assert_eq!(v.node_id, Some(1));
    assert_eq!(v.file, "src/a.rs");
    assert!(v.message.contains("nests 5"), "{}", v.message);
}

#[test]
fn max_brain_methods_budget_counts_production_brain_methods() {
    // Default brain thresholds: CC≥15 ∧ LOC≥100 ∧ nesting≥3.
    let strict = constraints_only(Constraints {
        max_brain_methods: Some(0),
        ..Constraints::default()
    });
    let metrics = [
        struct_row(1, Some(15), Some(100), Some(3), None), // a brain method
        struct_row(2, Some(15), Some(100), Some(2), None), // nesting < 3 → not brain
    ];
    let (violations, checked) =
        run_eval_budgets(&strict, &[], &[], &metrics, &[], Default::default());
    assert_eq!(checked, 1);
    assert_eq!(
        violations.len(),
        1,
        "exactly one brain method over the budget"
    );
    assert_eq!(violations[0].rule, "max_brain_methods");
    assert!(
        violations[0].message.contains("1 brain methods"),
        "{}",
        violations[0].message
    );

    // Relaxed budget: the same one brain method is under the ceiling.
    let lenient = constraints_only(Constraints {
        max_brain_methods: Some(5),
        ..Constraints::default()
    });
    let (clean, _) = run_eval_budgets(&lenient, &[], &[], &metrics, &[], Default::default());
    assert!(clean.is_empty(), "count under the ceiling → no violation");
}

#[test]
fn max_clone_ratio_budget_flags_a_near_clone_ratio_over_the_budget() {
    let strict = constraints_only(Constraints {
        max_clone_ratio: Some(0.0),
        ..Constraints::default()
    });
    // 2 production functions, 1 in a clone group → ratio 0.5 > 0.0.
    let metrics = [
        struct_row(1, None, None, None, Some(1)),
        struct_row(2, None, None, None, None),
    ];
    let (violations, checked) =
        run_eval_budgets(&strict, &[], &[], &metrics, &[], Default::default());
    assert_eq!(checked, 1);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].rule, "max_clone_ratio");
    assert!(
        violations[0].message.contains("ratio 0.5000"),
        "{}",
        violations[0].message
    );

    // Relaxed budget admits the ratio.
    let lenient = constraints_only(Constraints {
        max_clone_ratio: Some(1.0),
        ..Constraints::default()
    });
    let (clean, _) = run_eval_budgets(&lenient, &[], &[], &metrics, &[], Default::default());
    assert!(
        clean.is_empty(),
        "ratio at/under the ceiling → no violation"
    );
}

#[test]
fn no_god_containers_budget_flags_a_god_container() {
    let compiled = constraints_only(Constraints {
        no_god_containers: Some(true),
        ..Constraints::default()
    });
    // 20 methods ≥ default T_m (20) → god.
    let (nodes, edges) = god_container_fixture(1, 20);
    let (violations, checked) =
        run_eval_budgets(&compiled, &nodes, &edges, &[], &[], Default::default());
    assert_eq!(checked, 1);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].rule, "no_god_containers");
    assert_eq!(violations[0].node_id, Some(1));
    assert!(
        violations[0].message.contains("god container"),
        "{}",
        violations[0].message
    );

    // A small container is not god.
    let (small_nodes, small_edges) = god_container_fixture(1, 3);
    let (clean, _) = run_eval_budgets(
        &compiled,
        &small_nodes,
        &small_edges,
        &[],
        &[],
        Default::default(),
    );
    assert!(
        clean.is_empty(),
        "a 3-method container is not a god container"
    );

    // God by SPAN, not method count: a method-less class spanning ≥ default
    // T_span (500) is still a god container (the second predicate branch).
    let span_god = [NodeRow {
        end_line: Some(600),
        ..kinded_node(1, "Spanner", Some("src/s.rs"), NodeKind::Class)
    }];
    let (span_v, _) = run_eval_budgets(&compiled, &span_god, &[], &[], &[], Default::default());
    assert_eq!(span_v.len(), 1, "a 600-line container is god by span");
    assert_eq!(span_v[0].rule, "no_god_containers");
    assert!(
        span_v[0].message.contains("span 600"),
        "{}",
        span_v[0].message
    );

    // `false` / omitted is not enforced.
    let off = constraints_only(Constraints {
        no_god_containers: Some(false),
        ..Constraints::default()
    });
    let (none, checked_off) = run_eval_budgets(&off, &nodes, &edges, &[], &[], Default::default());
    assert!(
        none.is_empty() && checked_off == 0,
        "no_god_containers=false is inert"
    );
}

/// FR-QM-08: `no_god_containers` is production-scoped — a class whose 20 methods
/// are all `is_test` has zero production methods and is not a god container, so
/// adding test methods to a class never trips the budget.
#[test]
fn no_god_containers_excludes_test_scoped_methods() {
    let compiled = constraints_only(Constraints {
        no_god_containers: Some(true),
        ..Constraints::default()
    });
    // Class 1 + 20 methods (2..21), every method in the is_test set.
    let (nodes, edges) = god_container_fixture(1, 20);
    let test_ids: Vec<NodeId> = (2..=21).map(NodeId).collect();
    let (violations, _) = run_eval_budgets(
        &compiled,
        &nodes,
        &edges,
        &[],
        &test_ids,
        Default::default(),
    );
    assert!(
        violations.is_empty(),
        "a class of only test methods has 0 production methods → not god: {violations:?}"
    );
}

#[test]
fn cr005_budgets_are_production_scoped() {
    // A deep + brain + cloned function that is is_test must not trip any budget.
    let compiled = constraints_only(Constraints {
        max_nesting_depth: Some(4),
        max_brain_methods: Some(0),
        max_clone_ratio: Some(0.0),
        ..Constraints::default()
    });
    let nodes = [node(1, "test_fn", Some("tests/a.rs"))];
    let metrics = [struct_row(1, Some(99), Some(999), Some(9), Some(1))];
    let test_ids = [NodeId(1)];
    let (violations, _) = run_eval_budgets(
        &compiled,
        &nodes,
        &[],
        &metrics,
        &test_ids,
        Default::default(),
    );
    assert!(
        violations.is_empty(),
        "test-scoped functions are excluded from the new budgets (FR-QM-08): {violations:?}"
    );
}

/// UAT-GV-08: a fixture violating each new budget once — a 5-nested function, a
/// brain method, a clone pair, and a god container — under
/// `max_nesting_depth=4`, `max_brain_methods=0`, `max_clone_ratio=0.0`,
/// `no_god_containers=true` reports exactly four error violations in a
/// deterministic order; relaxing all four passes; the violation set is
/// byte-identical on re-run ([NFR-RA-06]).
#[test]
fn cr005_budgets_report_four_violations_then_relaxed_passes() {
    let strict = constraints_only(Constraints {
        max_nesting_depth: Some(4),
        max_brain_methods: Some(0),
        max_clone_ratio: Some(0.0),
        no_god_containers: Some(true),
        ..Constraints::default()
    });

    // A god container (Class 1 + 20 methods 2..21) plus four production functions
    // 30..33 (ids above the container's method ids to avoid collision): a deep
    // one, a brain method, and a clone pair.
    let (mut nodes, edges) = god_container_fixture(1, 20);
    nodes.push(node(30, "deep", Some("src/a.rs")));
    nodes.push(node(31, "brainy", Some("src/b.rs")));
    nodes.push(node(32, "clone_a", Some("src/c.rs")));
    nodes.push(node(33, "clone_b", Some("src/d.rs")));
    let metrics = [
        struct_row(30, Some(5), Some(10), Some(5), None), // deep (5 > 4)
        struct_row(31, Some(15), Some(100), Some(3), None), // brain
        struct_row(32, Some(1), Some(5), Some(1), Some(32)), // clone
        struct_row(33, Some(1), Some(5), Some(1), Some(32)), // clone
    ];

    let (violations, checked) =
        run_eval_budgets(&strict, &nodes, &edges, &metrics, &[], Default::default());

    assert_eq!(checked, 4, "all four budgets are active");
    assert_eq!(violations.len(), 4, "each budget fires exactly once");
    assert!(violations.iter().all(|v| v.severity == "error"));
    // Deterministic ordering: the four budgets in their fixed evaluation order.
    let rules: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(
        rules,
        [
            "max_nesting_depth",
            "max_brain_methods",
            "max_clone_ratio",
            "no_god_containers"
        ],
        "violation ordering is deterministic"
    );

    // Re-run → byte-identical violations (NFR-RA-06).
    let (again, _) = run_eval_budgets(&strict, &nodes, &edges, &metrics, &[], Default::default());
    assert_eq!(violations, again, "the violation set is reproducible");

    // Relax all four → the same fixture passes.
    let relaxed = constraints_only(Constraints {
        max_nesting_depth: Some(100),
        max_brain_methods: Some(100),
        max_clone_ratio: Some(1.0),
        no_god_containers: Some(false),
        ..Constraints::default()
    });
    let (clean, _) = run_eval_budgets(&relaxed, &nodes, &edges, &metrics, &[], Default::default());
    assert!(
        clean.is_empty(),
        "relaxed budgets admit the fixture: {clean:?}"
    );
}

// ── CR-023 / S-091: evaluate() orchestrator concatenation order ──────────────

/// The decomposed `evaluate()` must concatenate its per-section `check_*`
/// helpers in the fixed canonical order — constraints, coupling, redundancy,
/// CR-005 structural budgets, layer ordering, forbidden imports, require-tested,
/// require-documented — and sum every section's `checked` contribution. This
/// fires exactly one violation per section in a single pass and asserts both the
/// cross-section ordering and the `checked` total, locking the byte-identical
/// guarantee the refactor preserves ([NFR-RA-06], CR-023 §6).
#[test]
fn evaluate_concatenates_sections_in_canonical_order() {
    let rules = Rules {
        constraints: Constraints {
            max_cycles: Some(0),
            max_fan_out: Some(1),
            max_dead: Some(MaxDead::Absolute(0)),
            max_nesting_depth: Some(0),
            ..Constraints::default()
        },
        metric_thresholds: Default::default(),
        layers: vec![
            Layer {
                name: "domain".to_string(),
                paths: vec!["domain/**".to_string()],
                order: 0,
            },
            Layer {
                name: "app".to_string(),
                paths: vec!["app/**".to_string()],
                order: 1,
            },
        ],
        boundaries: Vec::new(),
        forbidden_imports: vec![ForbiddenImport {
            from: "src/web/**".to_string(),
            to: "src/db/**".to_string(),
            reason: None,
        }],
        require_tested: vec![RequireTested {
            paths: vec!["lib/**".to_string()],
            reason: None,
        }],
        require_documented: vec![RequireDocumented {
            paths: vec!["lib/**".to_string()],
            reason: None,
        }],
        history: Default::default(),
        coverage: Default::default(),
    };
    let compiled = CompiledRules::compile(rules, "test".to_string()).unwrap();

    let nodes = [
        node(1, "domain_fn", Some("domain/core.rs")),
        node(2, "app_fn", Some("app/h.rs")),
        node(3, "web_fn", Some("src/web/x.rs")),
        node(4, "db_fn", Some("src/db/y.rs")),
        node(5, "exported_api", Some("lib/api.rs")),
    ];
    // node 1 fan-out = 2 (> max_fan_out 1) → one coupling violation; node 3
    // fan-out = 1 (not flagged). edge 1→2 is the upward layer-ordering edge;
    // edge 3→4 (Imports) is the forbidden import.
    let edges = [
        edge(1, 2),
        edge(1, 4),
        kind_edge(3, 4, EdgeKind::Imports),
    ];
    let function_metrics = [
        metric_row(10, true, false), // is_dead → max_dead
        FunctionMetricRow {
            id: NodeId(1),
            cyclomatic_complexity: None,
            is_dead: Some(false),
            is_duplicate: Some(false),
            line_count: None,
            max_nesting_depth: Some(1), // > max_nesting_depth 0 → structural
            clone_group: None,
        },
    ];
    let annotations = [annot(5, "exported_api", NodeKind::Function, true, Some("lib/api.rs"))];

    let (violations, checked) = evaluate(&EvalInput {
        compiled: &compiled,
        nodes: &nodes,
        edges: &edges,
        functions: &[],
        function_metrics: &function_metrics,
        annotations: &annotations,
        test_node_ids: &[],
        cycles: 1, // > max_cycles 0 → constraints
        thresholds: crate::metrics::Thresholds::default(),
    });

    let rules_hit: Vec<&str> = violations.iter().map(|v| v.rule.as_str()).collect();
    assert_eq!(
        rules_hit,
        [
            "max_cycles",                          // constraints
            "max_fan_out",                         // coupling
            "max_dead",                            // redundancy
            "max_nesting_depth",                   // structural
            "layer-ordering",                      // layer ordering
            "forbidden_import:src/web/**->src/db/**", // forbidden imports
            "require_tested:lib/**",               // require-tested
            "require_documented:lib/**",           // require-documented
        ],
        "sections concatenate in canonical order"
    );
    // 1 each: max_cycles, max_fan_out, max_dead, max_nesting_depth, layer
    // ordering, the one forbidden_import, the one require_tested, the one
    // require_documented.
    assert_eq!(checked, 8, "every active rule counted exactly once");
}

// ── FR-GV-17: blast-radius ranking of the test-gap set ───────────────────────

/// One untested gap in `file`, named `name`.
fn gap(name: &str, file: &str) -> TestGap {
    TestGap {
        name: name.to_string(),
        file: file.to_string(),
        line: Some(1),
    }
}

/// A hotspot ranking map: each `(file, score)` pair is the file's hotspot score
/// the façade supplies as the blast-radius weight ([FR-GV-17]).
fn ranks(pairs: &[(&str, i64)]) -> HashMap<String, i64> {
    pairs.iter().map(|(f, s)| ((*f).to_string(), *s)).collect()
}

#[test]
fn blast_radius_is_fan_in_times_the_files_hotspot_score() {
    let r = ranks(&[("hot.rs", 30), ("cool.rs", 2)]);
    // 4 callers into a function in the hottest file dominates 9 callers into a
    // cool file: 4×30 = 120 > 9×2 = 18 (FR-GV-17 — fan-in × containing-file
    // hotspot rank, not fan-in alone).
    assert_eq!(blast_radius(4, "hot.rs", Some(&r)), 120);
    assert_eq!(blast_radius(9, "cool.rs", Some(&r)), 18);
}

#[test]
fn blast_radius_of_an_unranked_file_is_zero_never_fabricated() {
    let r = ranks(&[("hot.rs", 30)]);
    // A file with no hotspot signal contributes weight 0 — honest absence, not a
    // fabricated rank (NFR-CC-04). Same when no ranking was supplied at all.
    assert_eq!(blast_radius(7, "unranked.rs", Some(&r)), 0);
    assert_eq!(blast_radius(7, "hot.rs", None), 0);
}

#[test]
fn blast_radius_saturates_instead_of_overflowing() {
    let r = ranks(&[("f.rs", i64::MAX)]);
    // A pathological fan-in × max weight saturates rather than wrapping.
    assert_eq!(blast_radius(u64::MAX, "f.rs", Some(&r)), i64::MAX);
}

#[test]
fn ranked_order_is_blast_radius_descending_hand_computed() {
    // Hand-computed expectation (FR-GV-17 AC1): blast = fan-in × file hotspot.
    //   a (fan 4, hot=30)  → 120   ← most urgent
    //   b (fan 9, cool=2)  →  18
    //   c (fan 1, hot=30)  →  30
    //   d (fan 0, hot=30)  →   0   ← no callers, no blast
    let r = ranks(&[("hot.rs", 30), ("cool.rs", 2)]);
    let scored = vec![
        ScoredGap { blast: blast_radius(9, "cool.rs", Some(&r)), gap: gap("b", "cool.rs") },
        ScoredGap { blast: blast_radius(0, "hot.rs", Some(&r)), gap: gap("d", "hot.rs") },
        ScoredGap { blast: blast_radius(4, "hot.rs", Some(&r)), gap: gap("a", "hot.rs") },
        ScoredGap { blast: blast_radius(1, "hot.rs", Some(&r)), gap: gap("c", "hot.rs") },
    ];
    let ordered: Vec<String> = order_untested(scored, true).into_iter().map(|g| g.name).collect();
    assert_eq!(ordered, ["a", "c", "b", "d"], "120 > 30 > 18 > 0");
}

#[test]
fn ranked_order_ties_break_on_file_then_name() {
    // Two gaps with identical blast radius fall back to file then name, so the
    // order stays deterministic (NFR-RA-06).
    let r = ranks(&[("a.rs", 10), ("b.rs", 10)]);
    let scored = vec![
        ScoredGap { blast: blast_radius(1, "b.rs", Some(&r)), gap: gap("zeta", "b.rs") },
        ScoredGap { blast: blast_radius(1, "a.rs", Some(&r)), gap: gap("beta", "a.rs") },
        ScoredGap { blast: blast_radius(1, "a.rs", Some(&r)), gap: gap("alpha", "a.rs") },
    ];
    let ordered: Vec<String> = order_untested(scored, true).into_iter().map(|g| g.name).collect();
    assert_eq!(ordered, ["alpha", "beta", "zeta"], "tie → file asc, then name asc");
}

#[test]
fn degraded_order_is_the_fr_gv_08_file_name_order() {
    // `ranked = false` (no history/hotspot store): the FR-GV-08 file/name order
    // is authoritative and byte-identical to the pre-CR-038 behaviour — every
    // blast score is ignored (here deliberately set misleadingly high).
    let scored = vec![
        ScoredGap { blast: 999, gap: gap("zeta", "b.rs") },
        ScoredGap { blast: 1, gap: gap("beta", "a.rs") },
        ScoredGap { blast: 500, gap: gap("alpha", "a.rs") },
    ];
    let ordered: Vec<String> = order_untested(scored, false).into_iter().map(|g| g.name).collect();
    assert_eq!(ordered, ["alpha", "beta", "zeta"], "file asc then name asc, blast ignored");
}

// ── S-215 / FR-GV-20 / ADR-48: the admission-tripwire doctor-report merge ───

#[test]
fn doctor_report_is_ok_when_both_structural_and_admission_censuses_are_clean() {
    let structural = StructuralReport {
        node_count: 3,
        distinct_symbol_ids: 3,
        ..StructuralReport::default()
    };
    let admission = AdmissionCensus::default();

    let report = doctor_report(structural, admission);

    assert!(report.ok);
    assert!(report.faults.is_empty());
    assert_eq!(report.unadmitted_files, 0);
    assert!(report.unadmitted_sample.is_empty());
    assert!(report.message.contains("sound"), "clean message: {}", report.message);
}

#[test]
fn doctor_report_fails_and_names_the_files_on_admission_drift_alone() {
    // A perfectly sound structural census (NFR-RA-13 holds) but a non-zero
    // admission census — the exact "corrupt-by-admission, structurally-sound"
    // shape FR-GV-20 exists to catch (doctor was previously blind to this).
    let structural = StructuralReport {
        node_count: 5,
        distinct_symbol_ids: 5,
        ..StructuralReport::default()
    };
    let admission = AdmissionCensus {
        unadmitted_files: 2,
        unadmitted_sample: vec![".worktrees/a.rs".to_string(), ".worktrees/b.rs".to_string()],
    };

    let report = doctor_report(structural, admission);

    assert!(!report.ok, "a non-zero admission census fails doctor even though structural holds");
    assert_eq!(report.unadmitted_files, 2);
    assert_eq!(
        report.unadmitted_sample,
        vec![".worktrees/a.rs".to_string(), ".worktrees/b.rs".to_string()]
    );
    assert!(
        report.faults.iter().any(|f| f.contains(".worktrees/a.rs") && f.contains("admission")),
        "the fault names the offending files: {:?}",
        report.faults
    );
    // The pre-existing structural counters are still carried through untouched.
    assert_eq!(report.node_count, 5);
    assert_eq!(report.distinct_symbol_ids, 5);
    assert!(report.faults.iter().all(|f| !f.contains("orphan") && !f.contains("dangling")));
}

#[test]
fn admission_census_faults_is_empty_and_ok_on_a_clean_census() {
    let admission = AdmissionCensus::default();
    assert!(admission.is_ok());
    assert!(admission.faults().is_empty());
}

// ── CR-071 / S-279 / FR-IX-11: the unindexed-doc-symlink warning is diagnostic ─

#[test]
fn doctor_report_builder_leaves_doc_symlink_warnings_empty() {
    // The builder is shared with `verify` (as its embedded `structural`); the
    // FR-IX-11 doc-symlink diagnostic is populated only by the `doctor` entry
    // point, so the builder must leave it empty and never fold it into `faults`.
    let structural = StructuralReport {
        node_count: 2,
        distinct_symbol_ids: 2,
        ..StructuralReport::default()
    };
    let report = doctor_report(structural, AdmissionCensus::default());
    assert!(report.doc_symlink_warnings.is_empty(), "builder does not populate the warning");
    assert!(report.ok, "clean census is ok");
}

#[test]
fn doc_symlink_warning_is_diagnostic_only_and_never_flips_ok() {
    // FR-IX-11: the unindexed-doc-symlink warning does not change the verdict —
    // a structurally-sound, admission-clean graph stays `ok` even with a warning
    // attached (as `doctor` attaches it), because it is not a fault.
    let structural = StructuralReport {
        node_count: 1,
        distinct_symbol_ids: 1,
        ..StructuralReport::default()
    };
    let mut report = doctor_report(structural, AdmissionCensus::default());
    report.doc_symlink_warnings =
        vec!["documentation directory-symlink docs/specs exists but is unindexed: …".to_string()];
    assert!(report.ok, "a doc-symlink warning is diagnostic only — `ok` is unaffected");
    assert!(report.faults.is_empty(), "the warning is not a fault");
}
