//! S-761 ([FR-QM-17], [CR-238] D7): a "where to start" unit keeps its id across
//! a full re-index, and `node` and `impact` resolve a single-symbol unit's id.
//!
//! `improvements` once serialised the storage row id a unit was named by. A
//! re-index renumbers those rows, so the same unit read back as 49, then 71,
//! and the `/logos:review` skill could not find it again. Over a real indexed
//! fixture: the re-index here is made to renumber every storage id (asserted,
//! so the test cannot pass vacuously on a store that happened to keep them),
//! and every unit and label keeps its id.
//!
//! The fixture pins `[project] id`, so every symbol here is the same on every
//! machine.
//!
//! [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
//! [CR-238]: ../../docs/requests/CR-238-sprint-106-review-follow-ups.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::models::quality::{FixUnit, FixUnitKind, WhereToStart};
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A function nested `depth` deep.
fn nested_fn(name: &str, depth: usize) -> String {
    let mut body = String::new();
    for level in 0..depth {
        body.push_str(&format!("{}if x > {level} {{\n", "    ".repeat(level + 1)));
    }
    body.push_str(&format!("{}return x;\n", "    ".repeat(depth + 1)));
    for level in (0..depth).rev() {
        body.push_str(&format!("{}}}\n", "    ".repeat(level + 1)));
    }
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    0\n}}\n")
}

/// A body long enough to clear the default 50-token exact-duplicate floor.
fn duplicated_body(name: &str) -> String {
    format!(
        "pub fn {name}(items: &[i64], limit: i64) -> i64 {{\n    let mut total = 0;\n    \
         for item in items {{\n        if *item > limit {{\n            total += item * 2 - limit;\n        \
         }} else if *item < -limit {{\n            total -= item * 3 + limit;\n        }} else {{\n            \
         total += item;\n        }}\n    }}\n    if total > limit * 10 {{\n        total / 2\n    }} else {{\n        \
         total + limit\n    }}\n}}\n"
    )
}

/// Eight shallow functions, one nested six deep (a function unit and a nesting
/// outlier) and an exact-duplicate pair (a group unit), under a pinned
/// coordinate.
fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    coordinate::pin(tmp.path());
    let mut source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    source.push_str(&nested_fn("deep_one", 6));
    write(tmp.path(), "src/lib.rs", &source);
    write(
        tmp.path(),
        "src/dup.rs",
        &format!("{}{}", duplicated_body("copy_one"), duplicated_body("copy_two")),
    );
    tmp
}

/// The storage id of every node, by symbol.
fn storage_ids(root: &Path) -> Vec<(String, i64)> {
    let conn = Connection::open_with_flags(root.join(".logos/logos.db"), OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut stmt = conn
        .prepare("SELECT s.symbol, n.id FROM nodes n JOIN symbols s ON s.id = n.symbol_id ORDER BY s.symbol")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Scan and return the recorded list.
fn scanned(engine: &Engine) -> WhereToStart {
    let w = engine.scan(true).expect("scan runs").where_to_start;
    assert!(w.recorded);
    w
}

fn unit_of(w: &WhereToStart, kind: FixUnitKind) -> &FixUnit {
    w.units
        .iter()
        .find(|u| u.kind == kind)
        .unwrap_or_else(|| panic!("a {kind:?} unit is ranked: {:#?}", w.units))
}

/// [CR-238] AC 1 and AC 2: a full re-index that renumbers every storage id
/// leaves every unit's and label's id where it was — the function's, the
/// duplicate group's (its member set unchanged), and the label's — and the
/// whole recorded list byte for byte.
///
/// The re-index is made to renumber: a different tree — one filler file
/// alone — is indexed in between, so the second index of the original
/// allocates every row anew. Real
/// re-indexes renumber the same way (review D7 saw 49 become 71); the
/// assertion on the ids keeps that a fact of this test, not an assumption.
///
/// [CR-238]: ../../docs/requests/CR-238-sprint-106-review-follow-ups.md
#[test]
fn every_unit_keeps_its_id_across_a_full_reindex_that_renumbers_the_store() {
    let tmp = project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let before = scanned(&engine);
    let ids_before = storage_ids(tmp.path());

    // Renumber: index a different tree first, then the original again.
    let lib = fs::read_to_string(tmp.path().join("src/lib.rs")).unwrap();
    let dup = fs::read_to_string(tmp.path().join("src/dup.rs")).unwrap();
    fs::remove_file(tmp.path().join("src/lib.rs")).unwrap();
    fs::remove_file(tmp.path().join("src/dup.rs")).unwrap();
    write(tmp.path(), "src/aaa.rs", &(0..20).map(|i| format!("pub fn filler_{i}() {{}}\n")).collect::<String>());
    assert!(engine.index().warnings.is_empty());
    fs::remove_file(tmp.path().join("src/aaa.rs")).unwrap();
    write(tmp.path(), "src/lib.rs", &lib);
    write(tmp.path(), "src/dup.rs", &dup);
    assert!(engine.index().warnings.is_empty());
    let after = scanned(&engine);
    let ids_after = storage_ids(tmp.path());

    let symbols = |ids: &[(String, i64)]| ids.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>();
    assert_eq!(symbols(&ids_after), symbols(&ids_before), "the same tree, the same symbols");
    assert!(
        ids_before.iter().zip(&ids_after).all(|((_, a), (_, b))| a != b),
        "every storage id was renumbered: {ids_before:?} → {ids_after:?}"
    );

    let function = unit_of(&before, FixUnitKind::Function);
    assert_eq!(function.name, "deep_one");
    assert_eq!(function.node_id.as_deref(), Some("src/`lib.rs`/deep_one()."), "the coordinate-free symbol id");
    let group = unit_of(&before, FixUnitKind::DuplicateGroup);
    assert!(
        group.node_id.as_deref().is_some_and(|id| id.starts_with("duplicate_group:")),
        "a group's id is derived from its members: {group:?}"
    );
    assert_eq!(unit_of(&after, FixUnitKind::Function).node_id, function.node_id, "the function keeps its id");
    assert_eq!(unit_of(&after, FixUnitKind::DuplicateGroup).node_id, group.node_id, "the group keeps its id");
    let label = |w: &WhereToStart| {
        w.outliers.iter().find(|m| m.measure == "nesting").unwrap().labels[0].node_id.clone()
    };
    assert_eq!(label(&before), function.node_id, "the label carries the unit's id");
    assert_eq!(label(&after), label(&before), "and keeps it");

    // Byte for byte, but for the one storage-derived figure the list states: a
    // near-clone group's descriptor names its cluster number (`clone group
    // #N`), which the re-index renumbers too. The group's id does not read it.
    let clone = |w: &WhereToStart| unit_of(w, FixUnitKind::CloneGroup).clone();
    assert_eq!(clone(&after).node_id, clone(&before).node_id, "the near-clone group keeps its id");
    assert_ne!(clone(&after).detail, clone(&before).detail, "while its cluster number moved");
    let without_cluster_numbers = |w: &WhereToStart| {
        let mut w = w.clone();
        for u in w.units.iter_mut().filter(|u| u.kind == FixUnitKind::CloneGroup) {
            u.detail = u.detail.split(" · ").skip(1).collect::<Vec<_>>().join(" · ");
        }
        serde_json::to_string(&w).unwrap()
    };
    assert_eq!(
        without_cluster_numbers(&after),
        without_cluster_numbers(&before),
        "the whole recorded list, byte for byte"
    );
}

/// [CR-238] AC 3: `node` and `impact` resolve a single-symbol unit's id — the
/// symbol id without its repository coordinate — to the unit's own node, as
/// exactly as the full symbol, with no note; so does `impact_intersection`,
/// whose resolver is a separate one. A group's id names no single symbol, so
/// it resolves to none and the answer is the graceful empty one.
///
/// [CR-238]: ../../docs/requests/CR-238-sprint-106-review-follow-ups.md
#[test]
fn node_and_impact_resolve_a_single_symbol_units_id() {
    let tmp = project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let ranking = scanned(&engine);
    let function = unit_of(&ranking, FixUnitKind::Function);
    let id = function.node_id.clone().expect("a recorded unit carries its id");
    let full = coordinate::symbol(&id);

    let node = engine.node(&id, false);
    let detail = node.node.as_ref().unwrap_or_else(|| panic!("node resolves {id:?}: {node:?}"));
    assert_eq!(detail.symbol.symbol, full, "the unit's own node");
    assert_eq!(detail.symbol.name, function.name);
    assert!(node.alternatives.is_empty() && node.warnings.is_empty(), "exact, with no note: {node:?}");
    let by_symbol = engine.node(&full, false);
    assert_eq!(
        serde_json::to_value(&node.node).unwrap(),
        serde_json::to_value(&by_symbol.node).unwrap(),
        "the same answer as the full symbol"
    );

    let impact = engine.impact(&id, None);
    let resolved = impact.resolved.as_ref().unwrap_or_else(|| panic!("impact resolves {id:?}: {impact:?}"));
    assert_eq!(resolved.symbol, full);
    assert!(impact.alternatives.is_empty(), "{impact:?}");
    let by_symbol = engine.impact(&full, None);
    assert_eq!(
        serde_json::to_value(&impact.upstream).unwrap(),
        serde_json::to_value(&by_symbol.upstream).unwrap(),
        "the same upstream as the full symbol"
    );

    // `impact_intersection` resolves through its own resolver, which counts the
    // candidates a name matched: the id is exact, so it matches one.
    let intersection = engine.impact_intersection(&[format!("a={id}"), format!("b={id}")], None);
    let resolved: Vec<&str> = intersection.items[0].resolved.iter().map(|r| r.symbol.as_str()).collect();
    assert_eq!(resolved, [full.as_str()], "impact_intersection resolves the id: {intersection:?}");
    assert!(intersection.warnings.is_empty(), "exact, so no ambiguity warning: {:?}", intersection.warnings);

    let group = unit_of(&ranking, FixUnitKind::DuplicateGroup).node_id.clone().unwrap();
    assert!(engine.node(&group, false).node.is_none(), "a group's id names no single symbol");
    assert!(engine.impact(&group, None).resolved.is_none());
}
