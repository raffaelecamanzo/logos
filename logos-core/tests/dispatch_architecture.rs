//! Architecture views exclude dispatch edges (S-713, CR-226 BR-76, FR-QM-20) —
//! end to end through the public [`Engine`] façade on the two-crate
//! dependency-inversion fixture, with the SHIPPED Rust plugin.
//!
//! Crate `a` calls its own trait through `&dyn Tr` and `&impl Tr`; crate `b`
//! implements it and depends on `a`. The calls reach `b` only by dispatch, so:
//!
//! - the module rollup the DSM reads has no `a → b` cell;
//! - the metric graph holds no `a ↔ b` cycle: Depth counts two module layers
//!   (`b → a`), not one condensed tangle;
//! - a layer rule and a boundary forbidding `a → b` fire nothing, and the
//!   annotation pass materialises no forbidden edge;
//! - the module graph canvas draws no `a → b` edge either, while `affected` —
//!   a runtime reach — still names `a` as reached by a change to `b`.
//!
//! A direct call from `a` to `b`'s implementation is a dependency, and the
//! module edge, the condensed layer and both violations come back.

use logos_core::governance::DsmGranularity;
use logos_core::model::EdgeKind;
use logos_core::models::navigation::GraphGranularity;
use logos_core::Engine;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

#[path = "support/dispatch_crates.rs"]
mod dispatch_crates;
use dispatch_crates::{two_crates_with, A_LIB, A_LIB_DIRECT};

/// Crate `a` is the lower layer, `b` the upper one, and `a → b` is a
/// forbidden boundary besides.
const RULES: &str = "\
[[layers]]
name  = \"core\"
paths = [\"a/**\"]
order = 1

[[layers]]
name  = \"plugin\"
paths = [\"b/**\"]
order = 2

[[boundaries]]
from   = \"core\"
to     = \"plugin\"
reason = \"the core names its trait, never an implementation\"
";

/// The indexed fixture with [`RULES`], crate `a`'s library being `a_lib`.
fn indexed(a_lib: &str) -> (TempDir, Engine) {
    let tmp = two_crates_with(a_lib, &[(".logos/rules.toml", RULES)]);
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    (tmp, engine)
}

/// The module-DSM cell from crate `from`'s module to crate `to`'s.
fn dsm_cell(engine: &Engine, from: &str, to: &str) -> u32 {
    let dsm = engine.dsm(Some(DsmGranularity::Module), false).expect("dsm runs");
    let row = |krate: &str| {
        let prefix = format!("{krate}/src/");
        let rows: Vec<usize> = (0..dsm.rows.len()).filter(|&i| dsm.rows[i].name.contains(&prefix)).collect();
        assert_eq!(rows.len(), 1, "one module row per crate: {:?}", dsm.rows);
        rows[0]
    };
    dsm.matrix[row(from)][row(to)]
}

/// The rules each violation of a `check_rules` run names, in report order.
fn violated(engine: &Engine) -> Vec<String> {
    let report = engine.check_rules(None, false).expect("check_rules runs");
    report.violations.into_iter().map(|v| v.rule).collect()
}

/// How many `forbidden_dependency` edges the annotation pass materialised.
fn forbidden_edges(engine: &Engine) -> usize {
    let edges = engine.runtime().unwrap().submit_read(|store| store.all_edges()).expect("read runs");
    edges.iter().filter(|e| e.kind == EdgeKind::ForbiddenDependency).count()
}

/// Whether the module graph canvas draws an edge from crate `from`'s module to
/// crate `to`'s.
fn canvas_edge(engine: &Engine, from: &str, to: &str) -> bool {
    let canvas = engine.graph_elements(None, None, None, None, Some(GraphGranularity::Module), false);
    let (from, to) = (format!("{from}/src/"), format!("{to}/src/"));
    canvas.edges.iter().any(|e| e.source.contains(&from) && e.target.contains(&to))
}

/// The files a change to `b/src/lib.rs` reaches.
fn affected_by_b(engine: &Engine) -> Vec<String> {
    let result = engine.affected(&["b/src/lib.rs".to_string()], false);
    assert!(result.unknown.is_empty(), "{:?}", result.unknown);
    result.affected.into_iter().map(|f| f.file).collect()
}

#[test]
fn a_call_through_a_trait_is_no_dependency_on_its_implementation() {
    let (_tmp, engine) = indexed(A_LIB);

    assert_eq!(dsm_cell(&engine, "a", "b"), 0, "no a → b module edge");
    assert!(dsm_cell(&engine, "b", "a") > 0, "b depends on a's trait");

    let metrics = engine.scan(false).expect("scan runs").metrics;
    assert_eq!(metrics.depth.raw, 2.0, "b → a is two module layers, no cross-crate SCC");

    assert_eq!(violated(&engine), Vec::<String>::new(), "a layer rule forbidding a → b fires nothing");
    assert_eq!(forbidden_edges(&engine), 0, "no forbidden edge is materialised");

    assert!(!canvas_edge(&engine, "a", "b"), "the module canvas agrees with the DSM");
    assert!(canvas_edge(&engine, "b", "a"));
    assert_eq!(affected_by_b(&engine), ["a/src/lib.rs"], "a change to b still reaches its trait's callers");
}

#[test]
fn a_direct_call_to_the_implementation_restores_the_dependency() {
    let (_tmp, engine) = indexed(A_LIB_DIRECT);

    assert!(dsm_cell(&engine, "a", "b") > 0, "the direct call is an a → b module edge");

    let metrics = engine.scan(false).expect("scan runs").metrics;
    assert_eq!(metrics.depth.raw, 1.0, "a ↔ b condenses to one module layer");

    assert_eq!(violated(&engine), ["layer-ordering", "boundary:core->plugin"]);
    assert_eq!(forbidden_edges(&engine), 1, "the direct edge is flagged forbidden");
    assert!(canvas_edge(&engine, "a", "b"), "the canvas draws the direct dependency");
}
