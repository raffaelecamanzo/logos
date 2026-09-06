//! Impact-set intersection across planned work items (S-358 / [FR-NV-11],
//! [FR-NV-06], [NFR-CC-04], CR-114), exercised end-to-end through
//! `Engine::impact_intersection` against a real temp-directory fixture.
//!
//! Coverage by acceptance criterion:
//! - every pair with intersecting transitive impact sets is reported, naming
//!   the shared symbols;
//! - items with disjoint impact sets are reported as safely parallel;
//! - replayed over the Sprint 63 S-341/S-343/S-345 set, the `http_client_crates`
//!   intersection that prose-reading missed is reported;
//! - the payload states its coverage limits — an unindexed surface cannot
//!   contribute an intersection, and an unresolved declaration says so.
//!
//! [FR-NV-11]: ../../docs/specs/requirements/FR-NV-11.md
//! [FR-NV-06]: ../../docs/specs/requirements/FR-NV-06.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::fs;
use std::path::Path;

use logos_core::models::navigation::{ImpactIntersectionResult, ItemIntersection, WorkItem};
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The Sprint 63 collision, rebuilt as code: three per-language capture arms
/// that all route through one shared `http_client_crates` helper, plus a wiki
/// pair that shares nothing with them.
///
/// This is the shape [S-341]/[S-343]/[S-345] actually had — the reason the
/// three could not merge — and the shape the intersection query must catch.
///
/// [S-341]: ../../docs/planning/journal.md#s-341-java-http-client-call-capture
/// [S-343]: ../../docs/planning/journal.md#s-343-typescript-and-tsx-http-client-call-capture
/// [S-345]: ../../docs/planning/journal.md#s-345-go-http-client-call-capture
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/http.rs",
        "pub fn http_client_crates() {}\n",
    );
    for (file, arm) in [
        ("src/java.rs", "java_http_client_call"),
        ("src/typescript.rs", "typescript_http_client_call"),
        ("src/go.rs", "go_http_client_call"),
    ] {
        write(
            tmp.path(),
            file,
            &format!(
                "use crate::http::http_client_crates;\n\n\
                 pub fn {arm}() {{\n    http_client_crates();\n}}\n"
            ),
        );
    }
    // A second, genuinely independent cluster: nothing here reaches the HTTP
    // helper, so work on it is safely parallel with all three arms.
    write(
        tmp.path(),
        "src/wiki.rs",
        "pub fn wiki_render() {\n    wiki_helper();\n}\npub fn wiki_helper() {}\n",
    );
    tmp
}

/// An indexed engine over the fixture.
fn indexed_engine(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// One work-item spec, spelled the way every surface spells it.
fn item(spec: &str) -> String {
    assert!(WorkItem::parse_spec(spec).is_some(), "{spec} must be well-formed");
    spec.to_string()
}

/// The intersection reported for `left`/`right`, in either order.
fn pair<'a>(
    result: &'a ImpactIntersectionResult,
    left: &str,
    right: &str,
) -> Option<&'a ItemIntersection> {
    result
        .intersecting
        .iter()
        .find(|i| (i.left == left && i.right == right) || (i.left == right && i.right == left))
}

/// The names of the symbols an intersection reports as shared.
fn shared_names(intersection: &ItemIntersection) -> Vec<&str> {
    intersection
        .shared
        .iter()
        .map(|s| s.symbol.name.as_str())
        .collect()
}

// ── FR-NV-11 AC 5 + AC 1: the Sprint 63 replay, naming the shared symbol ─────

/// The headline acceptance criterion. Three items scheduled in parallel, each
/// naming what it intended to change; the query reports all three pairs as
/// intersecting **on `http_client_crates`** — the symbol whose deletion by the
/// first cost the other two their merge.
///
/// Note what the items declare: only S-341 names `http_client_crates` itself.
/// The other two reach it transitively, which is exactly why reading the
/// stories did not surface the collision.
#[test]
fn sprint_63_replay_reports_the_missed_http_client_crates_intersection() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let items = [
        item("S-341=java_http_client_call,http_client_crates"),
        item("S-343=typescript_http_client_call"),
        item("S-345=go_http_client_call"),
    ];
    let result = engine.impact_intersection(&items, None);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    // All three pairs collide, and none is reported safely parallel.
    assert_eq!(
        result.intersecting.len(),
        3,
        "every pair of the three arms intersects: {:?}",
        result.intersecting
    );
    assert!(
        result.safe_parallel.is_empty(),
        "no pair of the three is safely parallel: {:?}",
        result.safe_parallel
    );

    for (left, right) in [("S-341", "S-343"), ("S-341", "S-345"), ("S-343", "S-345")] {
        let found = pair(&result, left, right)
            .unwrap_or_else(|| panic!("{left}/{right} must be reported: {result:?}"));
        assert!(
            shared_names(found).contains(&"http_client_crates"),
            "{left}/{right} must name the shared symbol: {:?}",
            shared_names(found)
        );
        assert_eq!(
            found.shared_total,
            found.shared.len() as u32,
            "a small overlap is reported whole"
        );
        assert_eq!(found.shared_elided, 0, "nothing elided from a small overlap");
    }

    // The pair order follows the caller's item order (NFR-RA-06 determinism).
    let ordered: Vec<(&str, &str)> = result
        .intersecting
        .iter()
        .map(|i| (i.left.as_str(), i.right.as_str()))
        .collect();
    assert_eq!(
        ordered,
        vec![("S-341", "S-343"), ("S-341", "S-345"), ("S-343", "S-345")]
    );
}

/// A directly-declared collision is distinguished from a merely-reachable one:
/// `declared_by` names the items that intended to change the shared symbol.
#[test]
fn shared_symbols_name_which_items_declared_them_directly() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let items = [
        item("S-341=java_http_client_call,http_client_crates"),
        item("S-343=typescript_http_client_call"),
    ];
    let result = engine.impact_intersection(&items, None);

    let found = pair(&result, "S-341", "S-343").expect("the pair collides");
    let shared = found
        .shared
        .iter()
        .find(|s| s.symbol.name == "http_client_crates")
        .expect("the helper is the shared symbol");
    assert_eq!(
        shared.declared_by,
        vec!["S-341".to_string()],
        "only S-341 named the helper directly; S-343 reaches it"
    );
    assert_eq!(
        shared.symbol.file.as_deref(),
        Some("src/http.rs"),
        "the shared symbol carries its location"
    );
}

// ── FR-NV-11 AC 2: disjoint impact sets are safely parallel ──────────────────

#[test]
fn items_with_disjoint_impact_sets_are_reported_safely_parallel() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let items = [
        item("S-341=java_http_client_call"),
        item("S-999=wiki_render"),
    ];
    let result = engine.impact_intersection(&items, None);
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    assert!(
        result.intersecting.is_empty(),
        "the wiki cluster shares nothing with the HTTP arm: {:?}",
        result.intersecting
    );
    assert_eq!(result.safe_parallel.len(), 1);
    assert_eq!(result.safe_parallel[0].left, "S-341");
    assert_eq!(result.safe_parallel[0].right, "S-999");

    // Each item still reports what it resolved and how far it reaches.
    let arm = &result.items[0];
    assert_eq!(arm.declared, vec!["java_http_client_call".to_string()]);
    assert_eq!(arm.resolved.len(), 1);
    assert!(arm.unresolved.is_empty());
    assert!(
        arm.impact_set_size >= 2,
        "the seed plus what it reaches: {}",
        arm.impact_set_size
    );
}

// ── FR-NV-06: the same depth bound `impact` applies ──────────────────────────

/// The depth bound is real: at depth 0 an item's impact set is its seeds alone,
/// so the transitive `http_client_crates` collision disappears while the
/// directly-declared one survives.
#[test]
fn the_depth_bound_narrows_the_impact_sets() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let items = [
        item("S-343=typescript_http_client_call"),
        item("S-345=go_http_client_call"),
    ];

    let deep = engine.impact_intersection(&items, None);
    assert_eq!(deep.depth, 3, "the default depth matches `impact`");
    assert!(pair(&deep, "S-343", "S-345").is_some(), "{deep:?}");

    let shallow = engine.impact_intersection(&items, Some(0));
    assert_eq!(shallow.depth, 0);
    assert!(
        shallow.intersecting.is_empty(),
        "at depth 0 only declared symbols overlap: {:?}",
        shallow.intersecting
    );
    assert_eq!(shallow.safe_parallel.len(), 1);
    for reported in &shallow.items {
        assert_eq!(
            reported.impact_set_size, 1,
            "depth 0 leaves the seed alone: {reported:?}"
        );
    }
}

// ── FR-NV-11 AC 3 / NFR-CC-04: the payload states its coverage limits ────────

#[test]
fn the_payload_states_its_coverage_limits_and_names_unresolved_declarations() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let items = [
        item("S-341=java_http_client_call"),
        item("S-500=a_symbol_no_index_knows"),
    ];
    let result = engine.impact_intersection(&items, None);

    // The standing statement: a disjoint verdict is bounded by the index.
    let statement = &result.coverage.statement;
    assert!(
        statement.contains("indexed code graph"),
        "the limit names the graph it computed over: {statement}"
    );
    assert!(
        statement.contains("not proof of independence"),
        "the limit refuses to read `safe_parallel` as ground truth: {statement}"
    );

    // The unknown declaration is named, attributed, and not silently dropped.
    assert_eq!(result.coverage.unresolved.len(), 1);
    let unresolved = &result.coverage.unresolved[0];
    assert_eq!(unresolved.item, "S-500");
    assert_eq!(unresolved.symbol, "a_symbol_no_index_knows");
    assert_eq!(
        result.items[1].unresolved,
        vec!["a_symbol_no_index_knows".to_string()]
    );

    // An item with nothing resolved is disjoint by construction — the payload
    // says so rather than letting `safe_parallel` imply independence.
    assert_eq!(
        result.coverage.items_without_resolved_symbols,
        vec!["S-500".to_string()]
    );
    assert_eq!(result.safe_parallel.len(), 1, "{:?}", result.safe_parallel);
    assert!(result.intersecting.is_empty());
}

/// A single item cannot intersect with anything: an honest empty answer plus a
/// warning saying why, never an error (the ADR-14 infallible-surface contract).
#[test]
fn fewer_than_two_items_is_an_honest_empty_answer_with_a_warning() {
    let tmp = fixture();
    let engine = indexed_engine(&tmp);

    let result = engine.impact_intersection(&[item("S-341=java_http_client_call")], None);
    assert!(result.intersecting.is_empty());
    assert!(result.safe_parallel.is_empty());
    assert_eq!(result.items.len(), 1, "the item is still reported");
    assert!(
        result.warnings.iter().any(|w| w.contains("at least two")),
        "{:?}",
        result.warnings
    );

    let empty = engine.impact_intersection(&[], None);
    assert!(empty.items.is_empty());
    assert!(
        empty.warnings.iter().any(|w| w.contains("at least two")),
        "{:?}",
        empty.warnings
    );
}

// ── The shared `<id>=<symbol>,...` parser every surface uses ─────────────────

#[test]
fn work_item_specs_parse_once_for_every_surface() {
    // The plain form.
    let parsed = WorkItem::parse_spec("S-341=java_http_client_call,http_client_crates")
        .expect("a well-formed spec parses");
    assert_eq!(parsed.id, "S-341");
    assert_eq!(parsed.symbols, vec!["java_http_client_call", "http_client_crates"]);

    // Only the FIRST `=` splits, so a symbol containing `=` survives.
    let generic = WorkItem::parse_spec("item=a=b").expect("splits once");
    assert_eq!(generic.id, "item");
    assert_eq!(generic.symbols, vec!["a=b"]);

    // Blank entries and surrounding whitespace are dropped, not carried.
    let messy = WorkItem::parse_spec(" S-1 = alpha , , beta ").expect("parses");
    assert_eq!(messy.id, "S-1");
    assert_eq!(messy.symbols, vec!["alpha", "beta"]);

    // Malformed specs are refused rather than guessed at.
    assert!(WorkItem::parse_spec("no-equals-sign").is_none());
    assert!(WorkItem::parse_spec("=orphaned").is_none());

    // A repeated id accumulates — the escape hatch for a symbol with a comma —
    // and a malformed spec becomes a warning, never a silent drop.
    let (items, warnings) = WorkItem::from_specs(&["A=f", "B=g", "A=h", "broken"]);
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0].id, "A");
    assert_eq!(items[0].symbols, vec!["f", "h"]);
    assert_eq!(items[1].id, "B");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("broken"), "{warnings:?}");
}
