//! S-705 ([FR-EX-36]): a symbol argument written before symbols carried their
//! repository coordinate — an MCP or CLI argument, a bookmarked web `?seed=`, a
//! symbol recalled from an old answer — resolves under the store's coordinate,
//! and the answer says so in one line. A symbol naming another repository's
//! coordinate is never rewritten. A canonical argument answers as it always did.
//!
//! The fixture pins `[project] id`, so every symbol here is the same on every
//! machine.
//!
//! [FR-EX-36]: ../../docs/specs/requirements/FR-EX-36.md

#![cfg(all(feature = "lang-rust", feature = "lang-markdown"))]

use std::fs;
use std::path::Path;

use logos_core::models::navigation::GraphGranularity;
use logos_core::Engine;
use serde::Serialize;
use serde_json::Value;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// `run` is called by `alpha`, calls `gamma`, and a guide references it — so
/// every answer below has rows to compare, not just an anchor.
fn indexed() -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    coordinate::pin(tmp.path());
    write(
        tmp.path(),
        "src/lib.rs",
        "pub fn alpha() {\n    run();\n}\npub fn run() {\n    gamma();\n}\npub fn gamma() {}\n",
    );
    write(tmp.path(), "docs/guide.md", "# Guide\n\nCall `run` to start.\n");
    let engine = Engine::start(tmp.path()).unwrap();
    let result = engine.index();
    assert!(result.persist_failures.is_empty(), "{:?}", result.persist_failures);
    (tmp, engine)
}

const RUN_TAIL: &str = "src/`lib.rs`/run().";

/// `run` as a pre-coordinate argument: the package slot empty.
fn old_form() -> String {
    format!("logos . . . {RUN_TAIL}")
}

/// `run` as this store holds it.
fn canonical() -> String {
    coordinate::symbol(RUN_TAIL)
}

/// `run` under a coordinate this store does not hold.
fn foreign() -> String {
    format!("logos . example.com/other . {RUN_TAIL}")
}

/// The note an answer to the old-form argument carries.
fn note() -> String {
    format!(
        "{:?} carries no repository coordinate; resolved as {:?}, the form this store holds",
        old_form(),
        canonical()
    )
}

/// An answer as JSON with its `query` echo and `warnings` lifted out: the part
/// that must not depend on how the symbol was spelled.
fn answer_and_warnings(answer: &impl Serialize) -> (Value, Vec<String>) {
    let mut json = serde_json::to_value(answer).unwrap();
    let object = json.as_object_mut().expect("an answer is a JSON object");
    object.remove("query");
    let warnings = serde_json::from_value(object.remove("warnings").expect("every answer has warnings")).unwrap();
    (json, warnings)
}

/// The old-form argument answers exactly as the canonical one, plus the note
/// first in `warnings`; the canonical one carries no note.
fn assert_upgraded(tool: &str, old: &impl Serialize, canonical: &impl Serialize) {
    let (old_answer, old_warnings) = answer_and_warnings(old);
    let (canonical_answer, canonical_warnings) = answer_and_warnings(canonical);
    assert_eq!(old_answer, canonical_answer, "{tool}: the same answer");
    assert!(
        !canonical_warnings.iter().any(|w| w.contains("carries no repository coordinate")),
        "{tool}: a canonical argument carries no note: {canonical_warnings:?}"
    );
    let mut expected = vec![note()];
    expected.extend(canonical_warnings);
    assert_eq!(old_warnings, expected, "{tool}: the note, then the canonical warnings");
}

#[test]
fn node_callers_callees_and_impact_answer_an_old_form_symbol_with_a_note() {
    let (_tmp, engine) = indexed();
    let (old, canonical) = (old_form(), canonical());

    let node = engine.node(&canonical, false);
    assert_eq!(node.node.as_ref().map(|n| n.symbol.symbol.as_str()), Some(canonical.as_str()));
    assert_upgraded("node", &engine.node(&old, false), &node);

    let callers = engine.callers(&canonical, None);
    assert_eq!(callers.callers.len(), 1, "alpha calls run: {callers:?}");
    assert_upgraded("callers", &engine.callers(&old, None), &callers);

    let callees = engine.callees(&canonical, None);
    assert_eq!(callees.callees.len(), 1, "run calls gamma: {callees:?}");
    assert_upgraded("callees", &engine.callees(&old, None), &callees);

    let impact = engine.impact(&canonical, None);
    assert!(!impact.upstream.is_empty() && !impact.downstream.is_empty(), "{impact:?}");
    assert_upgraded("impact", &engine.impact(&old, None), &impact);
}

/// Every other tool resolving a symbol argument through the same resolvers
/// upgrades it the same way: explore, precedent, the documentation lookups.
#[test]
fn every_resolver_upgrades_an_old_form_symbol() {
    let (_tmp, engine) = indexed();
    let (old, canonical) = (old_form(), canonical());

    assert_upgraded("explore", &engine.explore(&old, None), &engine.explore(&canonical, None));
    assert_upgraded("precedent", &engine.precedent(&old, None), &engine.precedent(&canonical, None));
    let docs = engine.referencing_docs(&canonical);
    assert!(docs.resolved.is_some(), "{docs:?}");
    assert_upgraded("referencing_docs", &engine.referencing_docs(&old), &docs);
    assert_upgraded("implements", &engine.implements(&old), &engine.implements(&canonical));
}

/// `impact_intersection` resolves each declared symbol through the counting
/// resolver: the old-form item collides exactly as the canonical one does, and
/// the note is said once for the declaration that needed it.
#[test]
fn impact_intersection_upgrades_a_declared_old_form_symbol() {
    let (_tmp, engine) = indexed();
    let gamma = coordinate::symbol("src/`lib.rs`/gamma().");
    let canonical = engine.impact_intersection(&[format!("a={}", canonical()), format!("b={gamma}")], None);
    let old = engine.impact_intersection(&[format!("a={}", old_form()), format!("b={gamma}")], None);

    let (mut old_json, mut canonical_json) =
        (serde_json::to_value(&old).unwrap(), serde_json::to_value(&canonical).unwrap());
    for json in [&mut old_json, &mut canonical_json] {
        let object = json.as_object_mut().unwrap();
        object.remove("warnings");
        // Each item echoes the symbols it declared, as given.
        for item in object["items"].as_array_mut().unwrap() {
            item.as_object_mut().unwrap().remove("declared");
        }
    }
    assert_eq!(old_json, canonical_json, "the same intersection");
    assert!(!canonical.intersecting.is_empty() && !canonical.intersecting[0].shared.is_empty(), "{canonical:?}");
    assert_eq!(old.warnings, vec![note()]);
    assert!(canonical.warnings.is_empty(), "{:?}", canonical.warnings);
}

/// Another repository's coordinate is never rewritten: it resolves as unknown,
/// with no note, on every tool.
#[test]
fn a_symbol_carrying_another_repositorys_coordinate_resolves_as_unknown() {
    let (_tmp, engine) = indexed();
    let foreign = foreign();

    let node = engine.node(&foreign, false);
    assert!(node.node.is_none(), "{node:?}");
    let callers = engine.callers(&foreign, None);
    assert!(callers.resolved.is_none() && callers.callers.is_empty(), "{callers:?}");
    let callees = engine.callees(&foreign, None);
    assert!(callees.resolved.is_none() && callees.callees.is_empty(), "{callees:?}");
    let impact = engine.impact(&foreign, None);
    assert!(impact.resolved.is_none() && impact.upstream.is_empty(), "{impact:?}");
    let elements = engine.graph_elements(Some(&foreign), None, None, None, None, false);
    assert!(elements.nodes.is_empty(), "{elements:?}");
    assert_eq!(elements.warnings, vec![format!("unknown seed symbol: {foreign}")]);

    for warnings in [&node.warnings, &callers.warnings, &callees.warnings, &impact.warnings] {
        assert!(
            !warnings.iter().any(|w| w.contains("carries no repository coordinate")),
            "{warnings:?}"
        );
    }
}

/// A bookmarked `?seed=` with an old-form symbol renders the canonical seed's
/// graph: the same elements, the canonical seed echoed (the id the client marks
/// as the seed), and the note.
#[test]
fn a_web_seed_with_an_old_form_symbol_renders_the_canonical_graph() {
    let (_tmp, engine) = indexed();
    let canonical = engine.graph_elements(Some(&canonical()), None, None, None, None, false);
    let old = engine.graph_elements(Some(&old_form()), None, None, None, None, false);

    assert!(canonical.nodes.len() >= 3, "a seeded neighbourhood: {canonical:?}");
    assert!(canonical.warnings.is_empty(), "{:?}", canonical.warnings);
    assert_eq!(old.seed, canonical.seed, "the canonical seed is echoed");
    assert_eq!(old.warnings, vec![note()]);

    let (mut old_json, mut canonical_json) =
        (serde_json::to_value(&old).unwrap(), serde_json::to_value(&canonical).unwrap());
    old_json.as_object_mut().unwrap().remove("warnings");
    canonical_json.as_object_mut().unwrap().remove("warnings");
    assert_eq!(old_json, canonical_json, "the same elements");
}

/// A rollup tier's seed is a cluster key — a path, not a symbol — and is looked
/// up as given, with no note.
#[test]
fn a_rollup_seed_is_never_upgraded() {
    let (_tmp, engine) = indexed();
    let file = engine.graph_elements(Some("src/lib.rs"), None, None, None, Some(GraphGranularity::File), false);
    assert_eq!(file.seed.as_deref(), Some("src/lib.rs"));
    assert!(!file.nodes.is_empty(), "{file:?}");
    assert!(file.warnings.is_empty(), "{:?}", file.warnings);
}
