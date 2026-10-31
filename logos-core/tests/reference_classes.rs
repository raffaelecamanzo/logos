//! Reference resolution separates documentation, provably external and
//! internal-candidate references on every status surface (S-635, [FR-RS-49],
//! [CR-212]).
//!
//! [FR-RS-49]: ../../docs/specs/requirements/FR-RS-49.md
//! [CR-212]: ../../docs/requests/CR-212-reference-resolution-is-stated-over-the-references-that-could-be-internal.md

#![cfg(all(feature = "lang-rust", feature = "lang-markdown"))]

use std::fs;
use std::path::Path;

use logos_core::graph_store::NewUnresolvedRef;
use logos_core::model::{EdgeKind, RefForm};
use logos_core::models::{DocResolution, InternalResolution, ReferenceFigureAbsence, StatusInfo};
use logos_core::Engine;
use serde_json::{json, Value};
use tempfile::TempDir;

const CARGO_TOML: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n";

/// Bound internal calls (`util::run`, `make`), library-type calls
/// (`HashMap::new`, `serde_json::to_string`), a method only a library declares
/// (`iter`), a method the fixture declares too (`len`, on a value of unknown
/// type), and Markdown code spans.
const LIB_RS: &str = "\
mod util;

use std::collections::HashMap;

pub struct Thing;

impl Thing {
    pub fn new() -> Self {
        Thing
    }

    pub fn len(&self) -> usize {
        0
    }
}

pub fn make() -> Thing {
    Thing
}

pub fn alpha() {
    util::run();
    let map: HashMap<u8, u8> = HashMap::new();
    let text = serde_json::to_string(&map);
    let items = make();
    items.len();
    let numbers = [1, 2];
    numbers.iter();
    drop(text);
}
";
const UTIL_RS: &str = "pub fn run() {}\n";
const README: &str = "# Fixture\n\nCall `alpha`, which reaches `run`. Nothing declares `nowhere_at_all`.\n";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn indexed(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

const FIXTURE: &[(&str, &str)] = &[
    ("Cargo.toml", CARGO_TOML),
    ("src/lib.rs", LIB_RS),
    ("src/util.rs", UTIL_RS),
    ("README.md", README),
];

/// The reported ledger rows: `(kind/form, target, resolved)`.
fn ledger(engine: &Engine) -> Vec<(String, String, bool)> {
    engine
        .runtime()
        .unwrap()
        .submit_read(|store| {
            let mut rows: Vec<_> = store
                .unresolved_refs()?
                .into_iter()
                .filter(|r| r.form != RefForm::Symbol)
                .map(|r| (format!("{:?}/{:?}", r.kind, r.form), r.target, r.resolved))
                .collect();
            rows.sort();
            Ok(rows)
        })
        .expect("read runs")
}

/// The classes S-635 states, from one `status` read: the headline, the doc
/// figure, and each language's provably-external `(calls, imports)`.
type Figures = (InternalResolution, DocResolution, Vec<(String, Option<u64>, Option<u64>)>);

fn figures(status: &StatusInfo) -> Figures {
    (
        status.internal_resolution,
        status.doc_resolution,
        status
            .resolution_by_language
            .iter()
            .map(|row| (row.language.clone(), row.calls.provably_external, row.imports.provably_external))
            .collect(),
    )
}

fn rust_external(status: &StatusInfo) -> (Option<u64>, Option<u64>) {
    let row = status
        .resolution_by_language
        .iter()
        .find(|row| row.language == "rust")
        .expect("a rust row");
    (row.calls.provably_external, row.imports.provably_external)
}

#[test]
fn each_row_lands_in_exactly_one_class_and_the_classes_sum_to_the_population() {
    let (_tmp, engine) = indexed(FIXTURE);
    // The fixture's ledger, pinned, so every count below is a count of these
    // rows and of nothing else.
    let rows = ledger(&engine);
    let expected: Vec<(String, String, bool)> = [
        ("Calls/Method", "iter", false),
        ("Calls/Method", "len", false),
        ("Calls/Path", "HashMap::new", false),
        ("Calls/Path", "drop", false),
        ("Calls/Path", "make", true),
        ("Calls/Path", "serde_json::to_string", false),
        ("Calls/Path", "util::run", true),
        ("DocReference/Method", "alpha", true),
        ("DocReference/Method", "nowhere_at_all", false),
        ("DocReference/Method", "run", true),
        ("Imports/Path", "std::collections::HashMap", false),
    ]
    .into_iter()
    .map(|(kind, target, bound)| (kind.to_string(), target.to_string(), bound))
    .collect();
    assert_eq!(rows, expected);

    let status = engine.status();
    // Doc references: the three code spans, two bound.
    assert_eq!((status.doc_resolution.references, status.doc_resolution.resolved), (3, 2));
    // Provably external: `iter`, `drop` and `serde_json::to_string` name
    // nothing the fixture declares (i); `HashMap::new` names the fixture's own
    // `new`, and only the binder's `external-type` proves it external (ii);
    // the `std::collections::HashMap` import names nothing declared (i).
    // Internal candidates: the two bound calls and `len`, which the fixture
    // declares too.
    let headline = status.internal_resolution;
    assert_eq!(
        (
            headline.code_references,
            headline.provably_external,
            headline.internal_candidates,
            headline.resolved
        ),
        (8, 5, 3, 2)
    );
    assert_eq!(rust_external(&status), (Some(4), Some(1)));
    assert_eq!(headline.lower_bound, Some(2.0 / 3.0));
    assert_eq!(headline.absence, None);
    // The classes partition the reported population.
    assert_eq!(
        status.doc_resolution.references + headline.provably_external + headline.internal_candidates,
        status.refs_total
    );
    assert_eq!(status.doc_resolution.resolved + headline.resolved, status.refs_resolved);
}

#[test]
fn status_json_carries_the_headline_beside_the_unchanged_deprecated_fields() {
    let (_tmp, engine) = indexed(FIXTURE);
    let status: Value = serde_json::to_value(engine.status()).expect("serialises");
    assert_eq!(
        status["internal_resolution"],
        json!({
            "resolved": 2,
            "internal_candidates": 3,
            "provably_external": 5,
            "code_references": 8,
            "lower_bound": 2.0 / 3.0,
            "absence": null,
        })
    );
    assert_eq!(
        status["doc_resolution"],
        json!({ "references": 3, "resolved": 2, "ratio": 2.0 / 3.0, "absence": null })
    );
    // The deprecated figures: every reported row and its bound share, as the
    // ledger counts them — doc references and external code included.
    let rows = ledger(&engine);
    let bound = rows.iter().filter(|(_, _, resolved)| *resolved).count();
    assert_eq!(status["refs_total"], json!(rows.len()));
    assert_eq!(status["refs_resolved"], json!(bound));
    assert_eq!(status["refs_unresolved"], json!(rows.len() - bound));
    assert_eq!(status["resolution_coverage"], json!(bound as f64 / rows.len() as f64));
    // Per language: the provably-external count beside each class's figures.
    let rust = status["resolution_by_language"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["language"] == "rust")
        .unwrap();
    assert_eq!(
        (&rust["calls"]["provably_external"], &rust["imports"]["provably_external"]),
        (&json!(4), &json!(1))
    );
}

#[test]
fn a_graph_with_no_code_reference_reads_absent_not_complete() {
    let (_tmp, engine) = indexed(&[("README.md", README)]);
    let status = engine.status();
    assert_eq!(status.internal_resolution.code_references, 0);
    let headline = serde_json::to_value(status.internal_resolution).unwrap();
    assert_eq!(headline["lower_bound"], Value::Null);
    let none_recorded = serde_json::to_value(ReferenceFigureAbsence::NoReferencesRecorded).unwrap();
    assert_eq!(headline["absence"], none_recorded);
    assert_eq!(status.doc_resolution.references, 3, "the doc figure is stated on its own");
    assert!(status.doc_resolution.ratio.is_some());

    let (_empty, nothing) = indexed(&[("Cargo.toml", CARGO_TOML)]);
    let status = serde_json::to_value(nothing.status()).unwrap();
    for figure in ["internal_resolution", "doc_resolution"] {
        assert_eq!(status[figure]["absence"], none_recorded, "{figure}");
    }
}

/// A capture-before-delete row (`RefForm::Symbol`) a sync keeps: unbound, under
/// `src/util.rs`, naming nothing the fixture declares. Counted, it would read
/// provably external; it is a duplicate of a reference, not one (S-598).
fn plant_capture(engine: &Engine) {
    engine
        .runtime()
        .unwrap()
        .submit_write(|w| {
            w.insert_unresolved_ref(&NewUnresolvedRef {
                file_id: w.file_id("src/util.rs")?,
                source_symbol: "planted caller",
                target: "planted vanished target",
                alias: None,
                form: RefForm::Symbol,
                kind: EdgeKind::Calls,
                line: None,
                payload: None,
                receiver: None,
                peeled: None,
                arg_count: None,
                exported: None,
            })
        })
        .expect("plant the capture");
}

#[test]
fn two_reads_agree_and_a_synced_store_reads_as_a_cold_reindex() {
    let (tmp, engine) = indexed(FIXTURE);
    let first = figures(&engine.status());
    assert_eq!(first, figures(&engine.status()), "two reads of one graph agree");

    // A sync that moves every class: a new library call, a new internal call
    // and a new code span.
    let edited = LIB_RS.replace(
        "    drop(text);\n",
        "    drop(text);\n    util::run();\n    std::mem::take(&mut 0);\n",
    );
    write(tmp.path(), "src/lib.rs", &edited);
    let readme = format!("{README}\nSee `make`.\n");
    write(tmp.path(), "README.md", &readme);
    engine.sync(&["src/lib.rs".into(), "README.md".into()]);
    plant_capture(&engine);
    let synced = figures(&engine.status());
    assert_ne!(synced, first, "the sync moved the figures, or this test pins nothing");

    let (_cold_dir, cold) = indexed(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", &edited),
        ("src/util.rs", UTIL_RS),
        ("README.md", &readme),
    ]);
    assert_eq!(synced, figures(&cold.status()), "a synced store reads as a cold reindex");
}

/// A ledger target that is not UTF-8 — a corrupt store, an outside writer —
/// is still counted, and never fails the `status` around the figures: before
/// the lossy read, one such row reported the whole graph unindexed.
#[test]
fn a_target_that_is_not_utf8_is_counted_and_fails_no_status() {
    let (tmp, engine) = indexed(FIXTURE);
    let before = engine.status().internal_resolution;
    let conn = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).unwrap();
    let corrupted = conn
        .execute(
            "UPDATE unresolved_refs SET target = CAST(x'C328' AS TEXT) WHERE target = 'iter'",
            [],
        )
        .unwrap();
    assert_eq!(corrupted, 1, "the fixture's `iter` row, or this pins nothing");
    drop(conn);

    let status = engine.status();
    assert!(status.indexed, "{:?}", status.warnings);
    assert_eq!(status.internal_resolution, before, "the row keeps its class");
}

/// An invocation call site whose `source_symbol` is not UTF-8 is still a code
/// reference, and fails no `status` (S-637): the workspace reads each unbound
/// call site's declaration in the same scan, and a declaration it cannot
/// decode names no call site rather than failing the read the headline rides on.
#[test]
fn a_call_site_whose_declaration_is_not_utf8_is_counted_and_fails_no_status() {
    let (tmp, engine) = indexed(FIXTURE);
    let before = engine.status().internal_resolution;
    let conn = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).unwrap();
    conn.execute(
        "INSERT INTO unresolved_refs (source_symbol, target, form, kind, payload) \
         VALUES (CAST(x'FF' AS TEXT), 'GET /users', ?1, ?2, 'http-client-call')",
        rusqlite::params![RefForm::Path.as_i32(), EdgeKind::ArtifactBinding.as_i32()],
    )
    .unwrap();
    drop(conn);

    let status = engine.status();
    assert!(status.indexed, "{:?}", status.warnings);
    let after = status.internal_resolution;
    assert_eq!(
        (after.code_references, after.internal_candidates, after.resolved),
        (before.code_references + 1, before.internal_candidates + 1, before.resolved),
        "the call site is one more unbound internal candidate"
    );
}
