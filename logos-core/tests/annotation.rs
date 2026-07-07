//! Integration tests for the annotation engine (S-014 / FR-AN-01..04,
//! ADR-10 Pass 3), exercised end-to-end through [`Engine::index`] /
//! [`Engine::sync`] against real temp-directory fixtures: real Rust source,
//! real extraction, real resolution, then annotation over the resolved graph.
//!
//! Coverage by acceptance criterion:
//! - a function unreachable from any public export is flagged dead; an
//!   exported function is live with no call-site (FR-AN-01, UAT-AN-01);
//! - two functions with identical AST shape but different names are flagged
//!   as a duplicate pair; a structurally distinct one is not (FR-AN-02,
//!   UAT-AN-02);
//! - a layered `rules.toml` with a forbidden dependency direction yields
//!   `layer` policy nodes and a derived `forbidden_dependency` edge on the
//!   violating call (FR-AN-03, UAT-AN-03);
//! - annotations land in native `nodes` columns and survive an incremental
//!   sync without accumulating policy artifacts (FR-AN-04, ADR-10).

use std::fs;
use std::path::Path;

use logos_core::graph_store::AnnotationNodeRow;
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The whole annotation snapshot, via the public read seam.
fn snapshot(rt: &Runtime) -> Vec<AnnotationNodeRow> {
    rt.submit_read(|store| store.annotation_nodes())
        .expect("read runs")
}

/// The unique non-derived node named `name` with `kind`.
fn named<'a>(snap: &'a [AnnotationNodeRow], name: &str, kind: NodeKind) -> &'a AnnotationNodeRow {
    snap.iter()
        .find(|n| !n.derived && n.name == name && n.kind == kind)
        .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
}

// ── FR-AN-01 / UAT-AN-01: dead-code with exported-is-live, end-to-end ────────

#[test]
fn index_flags_unreachable_private_function_dead_and_exported_live() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn api() {
    used_helper();
}
fn used_helper() {}
fn never_called() {}
pub fn unreferenced_export() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);

    assert_eq!(
        named(&snap, "never_called", NodeKind::Function).is_dead,
        Some(true),
        "a private fn unreachable from any export is dead (UAT-AN-01)"
    );
    assert_eq!(
        named(&snap, "used_helper", NodeKind::Function).is_dead,
        Some(false),
        "a private fn called from the public API is live"
    );
    assert_eq!(
        named(&snap, "unreferenced_export", NodeKind::Function).is_dead,
        Some(false),
        "an exported fn is live with no call-site in the indexed scope"
    );
    assert_eq!(result.annotation.dead, 1, "exactly one dead function");
}

// ── FR-AN-02 / UAT-AN-02: duplicate detection end-to-end ─────────────────────

#[test]
fn index_flags_renamed_identifier_twins_as_duplicates() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn first(input: u32) -> u32 {
    let doubled = input * 2;
    if doubled > 10 {
        return doubled;
    }
    doubled + 1
}

pub fn second(value: u32) -> u32 {
    let scaled = value * 2;
    if scaled > 10 {
        return scaled;
    }
    scaled + 1
}

pub fn distinct(items: &[u32]) -> u32 {
    let mut total = 0;
    for item in items {
        total += item;
    }
    total
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let snap = snapshot(engine.runtime().unwrap());

    assert_eq!(
        named(&snap, "first", NodeKind::Function).is_duplicate,
        Some(true),
        "identical structure under renamed identifiers is a duplicate (UAT-AN-02)"
    );
    assert_eq!(
        named(&snap, "second", NodeKind::Function).is_duplicate,
        Some(true),
        "both members of the pair are flagged"
    );
    assert_eq!(
        named(&snap, "distinct", NodeKind::Function).is_duplicate,
        Some(false),
        "a structurally distinct fn is not flagged"
    );
    assert_eq!(result.annotation.duplicates, 2);
}

// ── FR-AN-03 / UAT-AN-03: layer policy + forbidden edge, end-to-end ──────────

/// A two-layer fixture with one deliberate `domain -> presentation` violation:
/// the layered `rules.toml` plus a domain function calling upward into a UI
/// function.
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

#[test]
fn layered_rules_materialise_policy_nodes_and_flag_the_forbidden_call() {
    let tmp = layered_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);

    // Layer membership lands on the nodes' native columns (FR-AN-04).
    let compute = named(&snap, "compute", NodeKind::Function);
    let render = named(&snap, "render", NodeKind::Function);
    assert_eq!(compute.layer_membership.as_deref(), Some("domain"));
    assert_eq!(render.layer_membership.as_deref(), Some("presentation"));

    // The policy nodes exist, marked derived (UAT-AN-03).
    let layer_names: Vec<&str> = snap
        .iter()
        .filter(|n| n.kind == NodeKind::Layer)
        .map(|n| n.name.as_str())
        .collect();
    assert_eq!(layer_names, ["domain", "presentation"]);
    assert!(
        snap.iter()
            .filter(|n| matches!(n.kind, NodeKind::Layer | NodeKind::Boundary))
            .all(|n| n.derived),
        "policy nodes are derived"
    );
    assert_eq!(
        snap.iter().filter(|n| n.kind == NodeKind::Boundary).count(),
        1
    );

    // The violating call carries a derived forbidden_dependency flag.
    let forbidden: Vec<_> = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .collect();
    assert!(
        forbidden
            .iter()
            .any(|e| e.source == compute.id && e.target == render.id),
        "the compute -> render call is flagged forbidden (UAT-AN-03)"
    );
    assert!(result.annotation.forbidden_edges >= 1);
    assert_eq!(result.annotation.layer_nodes, 2);
    assert_eq!(result.annotation.boundary_nodes, 1);
}

#[test]
fn sync_reannotates_without_accumulating_policy_artifacts() {
    let tmp = layered_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let first = engine.index();
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);

    // Touch the domain file (content change) and sync it: Pass 3 re-runs over
    // the whole graph (verdicts are graph-global), clearing and
    // re-materialising the policy artifacts instead of stacking them (ADR-10
    // idempotent derived edges).
    write(
        tmp.path(),
        "src/domain_core.rs",
        "\
use crate::ui_view::render;

pub fn compute() {
    render();
}

pub fn extra() {}
",
    );
    let synced = engine.sync(&["src/domain_core.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert_eq!(synced.files_modified, 1);

    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);
    assert_eq!(
        snap.iter().filter(|n| n.kind == NodeKind::Layer).count(),
        2,
        "layer nodes do not accumulate across syncs"
    );
    let forbidden = rt
        .submit_read(|s| s.all_edges())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
        .count() as u64;
    assert_eq!(
        forbidden, synced.annotation.forbidden_edges,
        "the stored forbidden flags are exactly this run's — cleared, then re-materialised"
    );
    assert!(
        synced.annotation.forbidden_edges >= 1,
        "the violation is re-flagged after the sync, not lost"
    );
    assert_eq!(
        first.annotation.forbidden_edges, synced.annotation.forbidden_edges,
        "an unrelated edit does not change the violation count"
    );
    // The new exported fn was annotated too — the pass covers the whole graph.
    assert_eq!(
        named(&snap, "extra", NodeKind::Function).is_dead,
        Some(false)
    );
}

#[test]
fn forbidden_flag_does_not_survive_rule_removal_after_target_file_sync() {
    // Review round 1 regression (S-014 must-fix #1): syncing the *target* file
    // of a violating call runs capture-before-delete over the edges pointing
    // into it. If the derived forbidden_dependency flag were captured, the
    // resolution pass would rebind it as a permanent derived=0 edge that
    // clear_derived could never remove — a governance flag outliving its rule.
    let tmp = layered_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let first = engine.index();
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    assert!(first.annotation.forbidden_edges >= 1, "violation flagged");

    // Sync the TARGET file (ui_view.rs holds `render`, the violated endpoint):
    // capture-before-delete runs over the inbound edges, including — before
    // the fix — the derived forbidden_dependency flag.
    write(
        tmp.path(),
        "src/ui_view.rs",
        "pub fn render() {}\n\npub fn helper() {}\n",
    );
    let synced = engine.sync(&["src/ui_view.rs".into()]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert!(
        synced.annotation.forbidden_edges >= 1,
        "the violation is re-flagged while the rules still forbid it"
    );

    // The architect deletes the architecture contract: every forbidden flag
    // must vanish on the next pass — no zombie edge resurrected from the
    // capture ledger.
    fs::remove_file(tmp.path().join(".logos/rules.toml")).unwrap();
    let after = engine.sync(&["src/ui_view.rs".into()]);
    assert!(after.warnings.is_empty(), "{:?}", after.warnings);
    assert_eq!(after.annotation.forbidden_edges, 0);

    let rt = engine.runtime().unwrap();
    let zombies = |rt: &logos_core::Runtime| {
        rt.submit_read(|s| s.all_edges())
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == EdgeKind::ForbiddenDependency)
            .count()
    };
    assert_eq!(
        zombies(rt),
        0,
        "no forbidden_dependency edge survives the rule removal (NFR-CC-04 honesty)"
    );

    // One more rule-less sync: had the flag been captured into the ledger, the
    // resolution retry sweep would re-bind it on EVERY later sync — and with
    // no rules there is no annotate step left to clean it up. The flag must
    // stay gone across rounds, not just on the round that removed the rules.
    write(
        tmp.path(),
        "src/domain_core.rs",
        "\
use crate::ui_view::render;

pub fn compute() {
    render();
}

pub fn extra_two() {}
",
    );
    let resurrection_round = engine.sync(&["src/domain_core.rs".into()]);
    assert!(
        resurrection_round.warnings.is_empty(),
        "{:?}",
        resurrection_round.warnings
    );
    assert_eq!(resurrection_round.annotation.forbidden_edges, 0);
    assert_eq!(
        zombies(rt),
        0,
        "the ledger never resurrects a derived flag on later syncs"
    );
}

// ── FR-AN-05 / UAT-AN-04 / CR-001: unified is_test annotation, end-to-end ────

/// Indexing real Rust marks test code through every detection path —
/// extraction evidence (an inline `#[cfg(test)] mod tests` fn), a `tests/`
/// path convention, and a `[semantics].test_markers` affix — while leaving
/// production code untouched ([FR-AN-05], [UAT-AN-04], [CR-001]).
#[test]
fn index_marks_test_code_via_evidence_path_and_marker_not_production() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn parse(input: &str) -> usize {
    input.len()
}

// Matches the `spec` marker by name affix, with no test attribute and on a
// production path — exercises the pure marker disjunct.
pub fn spec_builder() -> u8 {
    0
}

#[cfg(test)]
mod tests {
    fn inline_helper() -> bool {
        true
    }
}
",
    );
    write(
        tmp.path(),
        "tests/integration.rs",
        "\
fn integration_helper() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);

    // 1. Extraction evidence — the inline #[cfg(test)] module function.
    assert!(
        named(&snap, "inline_helper", NodeKind::Function).is_test,
        "an inline #[cfg(test)] mod fn is is_test via extraction evidence (FR-EX-06)"
    );
    // 2. Path convention — anything under tests/.
    assert!(
        named(&snap, "integration_helper", NodeKind::Function).is_test,
        "a tests/-directory fn is is_test via path convention"
    );
    // 3. Marker affix — `spec_builder` matches the `spec` marker.
    assert!(
        named(&snap, "spec_builder", NodeKind::Function).is_test,
        "a test_markers-affixed fn is is_test"
    );
    // Production code carries none of the three signals.
    assert!(
        !named(&snap, "parse", NodeKind::Function).is_test,
        "production code is never is_test (positive evidence only, ADR-18)"
    );

    // An is_test fn is a live root: the unreferenced inline test helper is
    // never is_dead (FR-AN-01/CR-001).
    assert_eq!(
        named(&snap, "inline_helper", NodeKind::Function).is_dead,
        Some(false),
        "an is_test=true fn is a dead-code live root"
    );
}

// ── FR-AN-06 / UAT-QM-12: near-clone clustering, end-to-end over real source ─

/// The UAT-QM-12 fixture, indexed end-to-end: two functions identical modulo
/// identifier renames plus **one edited line** (near clones, *not* exact
/// AST-shape duplicates), and one unrelated function. After a real
/// index — extraction persists the winnowed shingles, annotation clusters
/// them — the two near clones share one `clone_group` (the minimum member id)
/// while the unrelated function is in none; and because the edit gives them
/// distinct AST-shape fingerprints, neither is an exact duplicate, so the
/// exact-duplicate verdict ([FR-AN-02]) the Redundancy metric reads is
/// untouched ([UAT-QM-12], [FR-AN-06]).
#[test]
fn index_clusters_near_clones_without_touching_exact_duplicates() {
    let tmp = TempDir::new().unwrap();
    // `original` and `renamed` share their whole body shape except `renamed`
    // adds one statement (`scaled += 1;`) — a one-line edit that perturbs only a
    // bounded number of shingles (Jaccard stays ≥ 0.85) yet changes the exact
    // AST shape, so the two are near clones but not AST-shape duplicates.
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn original(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    let mut count = 0;
    for value in values {
        if *value > limit {
            let scaled = value * 2;
            total += scaled;
            count += 1;
        } else {
            total += value;
        }
    }
    if count > 0 {
        total = total / count;
    }
    total
}

pub fn renamed(items: &[u32], threshold: u32) -> u32 {
    let mut sum = 0;
    let mut seen = 0;
    for item in items {
        if *item > threshold {
            let scaled = item * 2;
            sum += scaled;
            sum += 1;
            seen += 1;
        } else {
            sum += item;
        }
    }
    if seen > 0 {
        sum = sum / seen;
    }
    sum
}

pub fn unrelated(name: &str) -> String {
    let mut greeting = String::from(\"hello, \");
    greeting.push_str(name);
    greeting.push('!');
    greeting
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let rt = engine.runtime().unwrap();
    let snap = snapshot(rt);

    let original = named(&snap, "original", NodeKind::Function);
    let renamed = named(&snap, "renamed", NodeKind::Function);
    let unrelated = named(&snap, "unrelated", NodeKind::Function);

    // The two near clones land in one group, identified by the minimum member id.
    let group = original
        .clone_group
        .expect("`original` is in a near-clone group (UAT-QM-12)");
    assert_eq!(
        renamed.clone_group,
        Some(group),
        "the renamed near clone shares `original`'s clone group (FR-AN-06)"
    );
    assert_eq!(
        group,
        original.id.min(renamed.id),
        "the group id is the minimum member node id (stable identifier)"
    );
    assert_eq!(
        unrelated.clone_group, None,
        "an unrelated function is in no near-clone group (UAT-QM-12)"
    );

    // The one-line edit makes them distinct AST shapes, so exact-duplicate
    // detection — the Redundancy input — is untouched (FR-AN-02 intact).
    assert_eq!(
        original.is_duplicate,
        Some(false),
        "near clones with a structural edit are not exact AST duplicates (FR-AN-02)"
    );
    assert_eq!(renamed.is_duplicate, Some(false));
}
