//! Resolution coverage is reported **per language, with its denominator**
//! ([FR-RS-09], [S-441], [CR-142] D3).
//!
//! `logos status` carried one global `refs_resolved / refs_total` ratio, and a
//! language binding **zero** cross-file references averaged into it unseen for
//! 74 sprints. These tests pin the readout that replaces that blind spot:
//!
//! - the cross-file figure and its absence are exclusive, and a zero is never
//!   serialised as a figure ([NFR-CC-04], [NFR-RA-05]);
//! - over a real index, a language whose references resolve only inside their
//!   own file reads a **different named state** from one that resolves across
//!   files, and from one with nothing to resolve;
//! - every language in the index is a row, in a deterministic order
//!   ([NFR-RA-06]);
//! - the readout persists nothing ([ADR-28]).
//!
//! The fixtures deliberately use shapes whose reading does not depend on the
//! resolver fixes that follow this story ([S-439], [S-440]): a same-file
//! TypeScript call is `same-file-only` before and after them. The pre-fix
//! reproduction of [CR-142] §3.1 is a measurement over the two real corpora,
//! recorded in the sprint's implementation notes, not a fixture that would pin
//! the defect in place.
//!
//! [FR-RS-09]: ../../docs/specs/requirements/FR-RS-09.md
//! [S-439]: ../../docs/planning/journal.md#s-439-a-module-specifier-is-canonicalised-as-a-path-not-as-a-member-expression
//! [S-440]: ../../docs/planning/journal.md#s-440-an-imported-binding-resolves-a-cross-file-call
//! [S-441]: ../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
//! [CR-142]: ../../docs/requests/CR-142-cross-file-call-resolution-is-rust-only.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [ADR-28]: ../../docs/specs/architecture/decisions/ADR-28.md

#![cfg(all(
    feature = "lang-rust",
    feature = "lang-typescript",
    feature = "lang-toml"
))]

use std::fs;
use std::path::Path;

use logos_core::models::navigation::{LanguageResolution, RelationResolution};
use logos_core::models::quality::CrossFileAbsence;
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A Rust crate calling and importing across two files, a TypeScript file
/// whose only call stays inside it but which imports from a sibling (a
/// reference, never called, so the `Calls` reading holds before and after
/// [S-440]), and a TOML manifest with nothing to resolve.
///
/// [S-440]: ../../docs/planning/journal.md#s-440-an-imported-binding-resolves-a-cross-file-call
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    write(
        root,
        "src/lib.rs",
        "use crate::util::run;\n\npub fn alpha() {\n    run();\n    beta();\n}\n\npub fn beta() {}\n",
    );
    write(root, "src/util.rs", "pub fn run() {}\n");
    write(root, "web/labels.ts", "export const LABEL = \"nav\";\n");
    write(
        root,
        "web/nav.ts",
        "import { LABEL } from \"./labels\";\n\nexport function navItems(): number {\n  return count() + LABEL.length;\n}\n\nfunction count(): number {\n  return 1;\n}\n",
    );
    write(
        root,
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    );
    tmp
}

fn indexed(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

fn row<'a>(rows: &'a [LanguageResolution], language: &str) -> &'a LanguageResolution {
    rows.iter()
        .find(|r| r.language == language)
        .unwrap_or_else(|| panic!("{language} is a row: {rows:#?}"))
}

// ── the classifier ───────────────────────────────────────────────────────────

#[test]
fn the_cross_file_figure_and_its_absence_are_exclusive() {
    // Every combination over a small grid: exactly one of the pair is `Some`,
    // and a present figure is never `0`.
    for references in 0..3 {
        for bound in 0..=references {
            for same in 0..3 {
                for cross in 0..3 {
                    let r = RelationResolution::measured(references, bound, same, cross);
                    assert_ne!(
                        r.cross_file_edges.is_some(),
                        r.cross_file_absence.is_some(),
                        "exactly one of figure and absence: {r:?}"
                    );
                    assert_ne!(
                        r.cross_file_edges,
                        Some(0),
                        "a zero is never a figure: {r:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn each_named_state_is_decided_by_what_establishes_it() {
    assert_eq!(
        CrossFileAbsence::classify(10, 4, 3, 2),
        (Some(2), None),
        "a cross-file edge settles it: the figure, no absence"
    );
    assert_eq!(
        CrossFileAbsence::classify(10, 4, 3, 0),
        (
            None,
            Some(CrossFileAbsence::SameFileOnly { same_file_edges: 3 })
        ),
        "resolved edges that all stay in their file"
    );
    assert_eq!(
        CrossFileAbsence::classify(10, 1, 0, 0),
        (
            None,
            Some(CrossFileAbsence::NoResolvedEdges {
                references: 10,
                bound: 1
            })
        ),
        "references and no resolved edge — the ledger's own bound count is carried, not claimed 0"
    );
    assert_eq!(
        CrossFileAbsence::classify(0, 0, 0, 0),
        (None, Some(CrossFileAbsence::NoReferencesRecorded)),
        "nothing to resolve is its own state"
    );
    // Edges without ledger rows (a population the two halves do not share)
    // are still judged by the edges.
    assert_eq!(
        CrossFileAbsence::classify(0, 0, 2, 0),
        (
            None,
            Some(CrossFileAbsence::SameFileOnly { same_file_edges: 2 })
        ),
    );
}

#[test]
fn a_zero_cross_file_count_never_serialises_as_a_figure() {
    let same_only = serde_json::to_value(RelationResolution::measured(2103, 245, 245, 0)).unwrap();
    assert_eq!(
        same_only,
        serde_json::json!({
            "references": 2103,
            "bound": 245,
            "same_file_edges": 245,
            "cross_file_edges": null,
            "cross_file_absence": { "cause": "same-file-only", "same_file_edges": 245 },
        }),
        "the CR-142 TSX row: a named state carrying what establishes it, never `0`"
    );
    let none = serde_json::to_value(RelationResolution::measured(441, 0, 0, 0)).unwrap();
    assert_eq!(
        none["cross_file_absence"],
        serde_json::json!({ "cause": "no-resolved-edges", "references": 441, "bound": 0 })
    );
    let nothing = serde_json::to_value(RelationResolution::measured(0, 0, 0, 0)).unwrap();
    assert_eq!(
        nothing["cross_file_absence"],
        serde_json::json!({ "cause": "no-references-recorded" })
    );
    let measured =
        serde_json::to_value(RelationResolution::measured(61335, 14496, 13232, 1285)).unwrap();
    assert_eq!(
        (
            &measured["cross_file_edges"],
            &measured["cross_file_absence"]
        ),
        (&serde_json::json!(1285), &serde_json::Value::Null),
        "the CR-142 Rust row: a figure and no absence"
    );
}

// ── the readout over a real index ────────────────────────────────────────────

#[test]
fn a_same_file_only_language_reads_differently_from_one_resolving_across_files() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let rows = engine.status().resolution_by_language;

    let rust = row(&rows, "rust");
    let rust_cross = rust
        .calls
        .cross_file_edges
        .expect("`alpha → run` crosses a file boundary");
    assert!(rust_cross >= 1, "{rust:?}");
    assert!(
        rust.calls.same_file_edges >= 1,
        "`alpha → beta` stays in lib.rs: {rust:?}"
    );
    assert!(
        rust.calls.references >= rust.calls.bound && rust.calls.bound >= 1,
        "the numerator never exceeds its denominator: {rust:?}"
    );

    // The Imports class through the same composition, not only its store
    // counts: `use crate::util::run` is one reference, bound, across files.
    // Rust is the control S-439 must leave byte-identical, so this is exact.
    assert_eq!(
        rust.imports,
        RelationResolution::measured(1, 1, 0, 1),
        "the Rust import reaches the readout as a cross-file figure: {rust:?}"
    );

    let ts = row(&rows, "typescript");
    // The TypeScript import is recorded under its own language. Its binding is
    // what S-439 changes, so only the denominator is pinned here.
    assert_eq!(
        ts.imports.references, 1,
        "`import {{ LABEL }} from \"./labels\"` is one Imports reference: {ts:?}"
    );
    assert_eq!(
        ts.calls.cross_file_absence,
        Some(CrossFileAbsence::SameFileOnly {
            same_file_edges: ts.calls.same_file_edges
        }),
        "a language binding only inside its own files is a named state: {ts:?}"
    );
    assert!(
        ts.calls.same_file_edges >= 1,
        "`navItems → count` bound: {ts:?}"
    );

    let toml = row(&rows, "toml");
    assert_eq!(
        (
            toml.calls.cross_file_absence,
            toml.imports.cross_file_absence
        ),
        (
            Some(CrossFileAbsence::NoReferencesRecorded),
            Some(CrossFileAbsence::NoReferencesRecorded)
        ),
        "a language with nothing to resolve is present, and says so: {toml:?}"
    );
}

#[test]
fn every_indexed_language_is_a_row_in_name_order() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let status = engine.status();
    let rows: Vec<(String, u64)> = status
        .resolution_by_language
        .iter()
        .map(|r| (r.language.clone(), r.files))
        .collect();

    // The fixture's own composition, stated rather than derived: two Rust
    // files, two TypeScript files, one manifest.
    assert_eq!(
        rows,
        [
            ("rust".to_string(), 2),
            ("toml".to_string(), 1),
            ("typescript".to_string(), 2),
        ],
        "one row per indexed language, in name order, with its exact file count"
    );

    // …and the files table is the authority on "present in the index".
    let conn = Connection::open_with_flags(
        tmp.path().join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT language, COUNT(*) FROM files WHERE language IS NOT NULL \
             GROUP BY language ORDER BY language",
        )
        .unwrap();
    let indexed: Vec<(String, u64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        rows, indexed,
        "never an absent row, never a miscounted one: the readout agrees with `files`"
    );
}

#[test]
fn the_readout_is_deterministic_and_persists_nothing() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let db = tmp.path().join(".logos/logos.db");
    let snapshot = || {
        let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let one = |sql: &str| -> String {
            conn.query_row(sql, [], |r| r.get::<_, String>(0))
                .unwrap_or_else(|e| panic!("{sql}: {e}"))
        };
        (
            one("SELECT COUNT(*) || ':' || COALESCE(SUM(resolved), 0) FROM unresolved_refs"),
            one("SELECT COUNT(*) || ':' || COALESCE(SUM(kind), 0) FROM edges"),
            one("SELECT CAST(COUNT(*) AS TEXT) FROM nodes"),
            one("SELECT COALESCE(group_concat(key || '=' || value, ';'), '') FROM (SELECT * FROM project_metadata ORDER BY key)"),
        )
    };
    let before = snapshot();
    let revision = engine.status().graph_revision;

    let first = engine.status().resolution_by_language;
    for _ in 0..3 {
        assert_eq!(
            engine.status().resolution_by_language,
            first,
            "the same graph reads the same rows (NFR-RA-06)"
        );
    }

    assert_eq!(
        snapshot(),
        before,
        "the readout wrote nothing to the graph store"
    );
    assert_eq!(
        engine.status().graph_revision,
        revision,
        "no status call advanced the graph revision"
    );
}
