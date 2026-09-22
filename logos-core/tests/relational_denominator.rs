//! A relational answer **states the resolution denominator it was computed
//! over** ([FR-NV-14], [S-442], [CR-143]).
//!
//! `logos callers navItemsFor` answered `total: 0` for a TypeScript symbol with
//! two live callers, and `impact` labelled the same empty set "breaks if
//! changed". The answer was a traversal of a resolved edge set that holds no
//! cross-file TypeScript call at all, and nothing on it said so: *nothing
//! depends on this* and *nothing could be resolved here* had one spelling.
//! These tests pin the typed field that separates them, on all six relational
//! answers:
//!
//! - it rides a **non-empty** Rust answer on every tool — a partial set read as
//!   complete is the same false clearance with a count in front of it
//!   ([CR-143] D3);
//! - it is the very row `status` reports for the anchor's language, so the
//!   readout and the answer cannot disagree ([CR-143] §3.5);
//! - an anchor the index does not hold, and a question that named nothing,
//!   read two different named states, and a degraded answer reads `n/a` rather
//!   than an empty row list ([NFR-CC-04]);
//! - it is identical across runs and across a reopened engine ([NFR-RA-06]).
//!
//! Like [S-441]'s fixtures, these use shapes whose reading does not move when
//! the resolver fixes land ([S-439], [S-440]): the TypeScript file's only call
//! stays inside it, so its `Calls` row is `same-file-only` before and after
//! them. The pre-fix reproduction over this repository's own graph —
//! `callers navItemsFor` at `total: 0` beside `same-file-only` — is a
//! measurement recorded in the sprint's implementation notes, not a fixture
//! that would pin the defect in place.
//!
//! [FR-NV-14]: ../../docs/specs/requirements/FR-NV-14.md
//! [S-439]: ../../docs/planning/journal.md#s-439-a-module-specifier-is-canonicalised-as-a-path-not-as-a-member-expression
//! [S-440]: ../../docs/planning/journal.md#s-440-an-imported-binding-resolves-a-cross-file-call
//! [S-441]: ../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
//! [S-442]: ../../docs/planning/journal.md#s-442-a-relational-answer-states-the-resolution-denominator-it-was-computed-over
//! [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md

#![cfg(all(feature = "lang-rust", feature = "lang-typescript"))]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::navigation::{
    AffectedResult, BranchOverlapResult, CalleesResult, CallersResult, DenominatorAbsence,
    ImpactIntersectionResult, ImpactResult, LanguageResolution, PrecedentResult,
    RelationResolution, ResolutionDenominator,
};
use logos_core::models::quality::CrossFileAbsence;
use logos_core::Engine;
use tempfile::TempDir;

// ── fixture ──────────────────────────────────────────────────────────────────

/// Run `git -C <cwd> <args…>` with a hermetic identity, asserting success —
/// `branch_overlap` reads git, so the fixture is a repository.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c",
            "user.email=dev@logos",
            "-c",
            "user.name=Logos Dev",
            "-c",
            "commit.gpgsign=false",
            // A global `core.hooksPath` would let a foreign hook veto the
            // fixture's commits on a developer machine; empty disables it.
            "-c",
            "core.hooksPath=",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn lib_rs(alpha_body: &str) -> String {
    format!(
        "use crate::util::run;\n\npub fn alpha() {{\n    run();\n    beta();{alpha_body}\n}}\n\n\
         pub fn beta() {{}}\n"
    )
}

/// [S-441]'s shapes, in a repository: a Rust crate calling and importing
/// across two files, and a TypeScript file whose only call stays inside it
/// but which imports from a sibling. Two branches each edit `alpha`, so
/// `branch_overlap` has a contended symbol to report.
///
/// [S-441]: ../../docs/planning/journal.md#s-441-resolution-coverage-is-reported-per-language-with-its-denominator
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, "src/lib.rs", &lib_rs(""));
    write(root, "src/util.rs", "pub fn run() {}\n");
    write(root, "web/labels.ts", "export const LABEL = \"nav\";\n");
    write(
        root,
        "web/nav.ts",
        "import { LABEL } from \"./labels\";\n\nexport function navItems(): number {\n  \
         return count() + LABEL.length;\n}\n\nfunction count(): number {\n  return 1;\n}\n",
    );
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);
    for (name, extra) in [("left", "\n    let _l = 1;"), ("right", "\n    let _r = 2;")] {
        git(root, &["checkout", "-q", "-b", name, "main"]);
        write(root, "src/lib.rs", &lib_rs(extra));
        git(root, &["commit", "-q", "-am", name]);
        git(root, &["checkout", "-q", "main"]);
    }
    // A branch whose only change is a file the index never held (it exists on
    // that branch alone, and the index reads the checked-out `main`).
    git(root, &["checkout", "-q", "-b", "notes", "main"]);
    write(root, "NOTES.txt", "not indexed\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "notes"]);
    git(root, &["checkout", "-q", "main"]);
    tmp
}

fn indexed(tmp: &TempDir) -> Engine {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    engine
}

/// The row `status` reports for `language` — the one each answer must carry.
fn status_row(engine: &Engine, language: &str) -> LanguageResolution {
    engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == language)
        .unwrap_or_else(|| panic!("status reports a {language} row"))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// Every relational answer over one engine, labelled, each reduced to its
/// denominator and whether its set was non-empty — the six tools the
/// requirement names, in one place, so no test can quietly check five.
fn six_answers(
    engine: &Engine,
    symbol: &str,
    file: &str,
    items: &[&str],
) -> Vec<(&'static str, bool, ResolutionDenominator)> {
    let callers: CallersResult = engine.callers(symbol, None);
    let callees: CalleesResult = engine.callees(symbol, None);
    let impact: ImpactResult = engine.impact(symbol, None);
    let affected: AffectedResult = engine.affected(&strings(&[file]), false);
    let intersection: ImpactIntersectionResult =
        engine.impact_intersection(&strings(items), None);
    let overlap: BranchOverlapResult =
        engine.branch_overlap(&strings(&["left", "right"]), None, None);
    vec![
        ("callers", callers.total > 0, callers.resolution_denominator),
        ("callees", callees.total > 0, callees.resolution_denominator),
        ("impact", !impact.upstream.is_empty(), impact.resolution_denominator),
        ("affected", !affected.affected.is_empty(), affected.resolution_denominator),
        (
            "impact_intersection",
            !intersection.intersecting.is_empty(),
            intersection.resolution_denominator,
        ),
        ("branch_overlap", overlap.contended_total > 0, overlap.resolution_denominator),
    ]
}

// ── AC: present on a NON-EMPTY Rust answer, on all six tools ────────────────

/// **The AC-level pin** ([FR-NV-14] AC 2): every relational tool, answering a
/// non-empty set over Rust, carries the Rust row it traversed — with a
/// cross-file figure, since that is what makes the non-empty count readable as
/// complete or not. A field raised only on empty answers fails here.
///
/// [FR-NV-14]: ../../docs/specs/requirements/FR-NV-14.md
#[test]
fn a_non_empty_rust_answer_carries_its_denominator_on_every_relational_tool() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let rust = status_row(&engine, "rust");
    assert!(
        rust.calls.cross_file_edges.is_some(),
        "the fixture's Rust row has a cross-file Calls figure: {rust:#?}"
    );

    // `run` is called across a file by `alpha`; `alpha` calls it; changing
    // `util.rs` reaches `lib.rs`; A and B collide on `run`; both branches edit
    // `alpha`. Each set is non-empty, and each answer is anchored in Rust.
    let answers = six_answers(&engine, "run", "src/util.rs", &["A=alpha", "B=run"]);
    let callees = engine.callees("alpha", None);
    assert!(callees.total > 0, "alpha calls run and beta");
    assert_eq!(answers.len(), 6, "the six relational tools of FR-NV-14");
    for (tool, non_empty, denominator) in answers {
        if tool != "callees" {
            assert!(non_empty, "{tool} returns a non-empty set over the fixture");
        }
        assert_eq!(
            denominator,
            ResolutionDenominator {
                languages: vec![rust.clone()],
                absence: None,
            },
            "{tool} states the Rust row it was computed over"
        );
    }
    assert_eq!(
        callees.resolution_denominator.languages,
        vec![rust],
        "callees, on its own non-empty answer"
    );
}

/// The field is on the **wire**, not only on the type: a machine consumer
/// reads JSON, and a `#[serde(skip)]` would pass every Rust-level assertion
/// above.
#[test]
fn the_denominator_is_a_typed_field_on_the_serialised_answer() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let wire = serde_json::to_value(engine.callers("run", None)).expect("serialises");
    let denominator = &wire["resolution_denominator"];
    assert_eq!(denominator["absence"], serde_json::Value::Null);
    assert_eq!(denominator["languages"][0]["language"], "rust");
    assert!(
        denominator["languages"][0]["calls"]["cross_file_edges"].as_u64() > Some(0),
        "a machine-readable figure, not prose: {denominator:#}"
    );
    assert!(
        wire["warnings"].as_array().is_some_and(Vec::is_empty),
        "the denominator is not smuggled through the warnings channel"
    );
}

// ── AC: the empty answer is qualified, by the row status reports ────────────

/// The answer carries **the row `status` reports** for its anchor's language —
/// one number, computed once and consumed twice ([CR-143] §3.5). An empty
/// TypeScript answer therefore reads `same-file-only` beside its zero, which is
/// the distinction `total: 0` alone could not make.
///
/// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
#[test]
fn an_empty_typescript_answer_states_the_row_status_reports() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let typescript = status_row(&engine, "typescript");
    assert!(
        matches!(
            typescript.calls.cross_file_absence,
            Some(CrossFileAbsence::SameFileOnly { .. })
        ),
        "the fixture's TypeScript call stays in its file: {typescript:#?}"
    );

    let callers = engine.callers("navItems", None);
    assert_eq!(callers.total, 0, "nothing calls navItems");
    assert_eq!(callers.resolution_denominator.languages, vec![typescript.clone()]);

    let impact = engine.impact("navItems", None);
    assert!(impact.upstream.is_empty());
    assert_eq!(
        impact.resolution_denominator.languages,
        vec![typescript.clone()],
        "an empty `breaks if changed` set is never unqualified"
    );

    let affected = engine.affected(&strings(&["web/nav.ts"]), true);
    assert!(affected.affected.is_empty());
    assert_eq!(
        affected.resolution_denominator.languages,
        vec![typescript],
        "`--tests-only` over TypeScript is not an unqualified empty set"
    );
}

/// Anchors in two languages carry both rows, in name order — the anchor
/// decides the row, not the first match.
#[test]
fn anchors_in_two_languages_carry_both_rows_in_name_order() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let answer = engine.impact_intersection(&strings(&["A=alpha", "B=navItems"]), None);
    let languages: Vec<&str> = answer
        .resolution_denominator
        .languages
        .iter()
        .map(|row| row.language.as_str())
        .collect();
    assert_eq!(languages, ["rust", "typescript"]);
    assert_eq!(answer.resolution_denominator.absence, None);

    let affected = engine.affected(&strings(&["web/nav.ts", "src/util.rs"]), false);
    assert_eq!(
        affected.resolution_denominator.languages,
        vec![status_row(&engine, "rust"), status_row(&engine, "typescript")]
    );
}

// ── the named states ─────────────────────────────────────────────────────────

/// An anchor the index does not hold reads `unindexed` on every tool that can
/// be asked about one — never an empty row list with no reason.
#[test]
fn an_anchor_the_index_does_not_hold_reads_unindexed() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let unindexed = ResolutionDenominator {
        languages: Vec::new(),
        absence: Some(DenominatorAbsence::Unindexed),
    };
    let precedent: PrecedentResult = engine.precedent("no_such_symbol", None);
    for (tool, denominator) in [
        ("callers", engine.callers("no_such_symbol", None).resolution_denominator),
        ("callees", engine.callees("no_such_symbol", None).resolution_denominator),
        ("impact", engine.impact("no_such_symbol", None).resolution_denominator),
        (
            "affected",
            engine
                .affected(&strings(&["src/absent.rs"]), false)
                .resolution_denominator,
        ),
        (
            "impact_intersection",
            engine
                .impact_intersection(&strings(&["A=no_such", "B=nor_this"]), None)
                .resolution_denominator,
        ),
        ("precedent", precedent.resolution_denominator),
        (
            "branch_overlap",
            engine
                .branch_overlap(&strings(&["notes", "main"]), None, None)
                .resolution_denominator,
        ),
    ] {
        assert_eq!(denominator, unindexed, "{tool}");
    }
}

/// A question that named nothing has no anchor to be missing, so it reads
/// `n/a` rather than `unindexed` — R1: the condition establishes no cause. And
/// a degraded answer reads `n/a` too, rather than an empty list: a transient
/// engine has no read pool, so every query takes the [ADR-14] path.
///
/// [ADR-14]: ../../docs/specs/architecture/decisions/ADR-14.md
#[test]
fn nothing_asked_and_a_degraded_answer_read_n_a() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let not_available = ResolutionDenominator::not_available();
    assert_eq!(engine.affected(&[], false).resolution_denominator, not_available);
    assert_eq!(engine.impact_intersection(&[], None).resolution_denominator, not_available);
    assert_eq!(engine.branch_overlap(&[], None, None).resolution_denominator, not_available);
    assert_eq!(
        engine
            .branch_overlap(&strings(&["main", "main"]), None, None)
            .resolution_denominator,
        not_available,
        "refs that changed no file anchor nothing"
    );

    let transient = Engine::open(tmp.path());
    let degraded = transient.callers("run", None);
    assert!(!degraded.warnings.is_empty(), "the query degraded");
    assert_eq!(degraded.resolution_denominator, not_available);
    assert_eq!(
        serde_json::to_value(&degraded.resolution_denominator).unwrap(),
        serde_json::json!({ "languages": [], "absence": { "cause": "n/a" } }),
        "the wire spelling is the lexicon's `n/a`"
    );
}

/// Anchors that resolved into no language-tagged file read
/// `no-language-recorded`, never `unindexed`, on both ways an anchor can lack
/// a language: a held file whose `files.language` is `NULL`, and a node whose
/// file row is gone (`nodes.file_id` is `ON DELETE SET NULL`). R1: the anchor
/// **is** in the index, so saying otherwise would name a false cause.
#[test]
fn an_anchor_with_no_recorded_language_reads_no_language_recorded() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let db = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).expect("store opens");
    let no_language = Some(DenominatorAbsence::NoLanguageRecorded { anchors: 1 });

    db.execute_batch("UPDATE files SET language = NULL WHERE path = 'web/labels.ts';")
        .expect("the language clears");
    assert_eq!(
        engine
            .affected(&strings(&["web/labels.ts"]), false)
            .resolution_denominator
            .absence,
        no_language,
        "a held file that records no language"
    );

    db.execute_batch("PRAGMA foreign_keys = ON; DELETE FROM files WHERE path = 'src/util.rs';")
        .expect("the file row deletes, orphaning its nodes");
    let callers = engine.callers("run", None);
    assert!(callers.resolved.is_some(), "`run` still resolves, file-less");
    assert_eq!(
        callers.resolution_denominator.absence, no_language,
        "a node bound to no file"
    );
}

/// A denominator read that fails **beside a successful answer** degrades the
/// denominator alone: the answer keeps its result, the denominator reads
/// `n/a` — never a cause the failure did not establish — and `warnings` says
/// why, root cause included ([ADR-14]). The read is broken by renaming the
/// column it selects, which the traversal `callers` runs never touches.
///
/// [ADR-14]: ../../docs/specs/architecture/decisions/ADR-14.md
#[test]
fn a_failed_denominator_read_degrades_the_denominator_and_not_the_answer() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    let db = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).expect("store opens");
    db.execute_batch("ALTER TABLE files RENAME COLUMN language TO language_moved;")
        .expect("the column renames");

    let answer = engine.callers("run", None);
    assert!(answer.total > 0, "the traversal itself still answers: {answer:?}");
    assert_eq!(answer.resolution_denominator, ResolutionDenominator::not_available());
    let warning = answer
        .warnings
        .iter()
        .find(|w| w.starts_with("the resolution denominator could not be read: "))
        .unwrap_or_else(|| panic!("the failure is stated: {:?}", answer.warnings));
    assert!(
        warning.contains("no such column"),
        "the warning carries the root cause, not only the outer context: {warning}"
    );
}

/// The one constructor that classifies, over synthetic rows: exactly one of
/// the rows and their absence, and each absence decided by what establishes
/// it.
#[test]
fn measured_selects_the_anchor_rows_or_names_why_there_are_none() {
    let row = |language: &str| LanguageResolution {
        language: language.to_string(),
        files: 1,
        calls: RelationResolution::measured(2, 1, 1, 0),
        imports: RelationResolution::measured(0, 0, 0, 0),
    };
    let rows = || vec![row("go"), row("rust"), row("tsx")];
    let some = |l: &str| Some(l.to_string());

    assert_eq!(
        ResolutionDenominator::measured(rows(), &[]).absence,
        Some(DenominatorAbsence::Unindexed),
        "no anchor resolved"
    );
    assert_eq!(
        ResolutionDenominator::measured(rows(), &[None, None]).absence,
        Some(DenominatorAbsence::NoLanguageRecorded { anchors: 2 }),
        "anchors with no recorded language"
    );
    assert_eq!(
        ResolutionDenominator::measured(rows(), &[some("python")]).absence,
        Some(DenominatorAbsence::NoLanguageRecorded { anchors: 1 }),
        "a language with no row selects nothing"
    );
    let mixed = ResolutionDenominator::measured(rows(), &[some("tsx"), None, some("go"), some("tsx")]);
    assert_eq!(
        mixed,
        ResolutionDenominator {
            languages: vec![row("go"), row("tsx")],
            absence: None,
        },
        "each anchor language once, in name order; a language-less anchor adds nothing"
    );

    for anchors in [vec![], vec![None], vec![some("rust")], vec![some("cobol")]] {
        let d = ResolutionDenominator::measured(rows(), &anchors);
        assert_ne!(
            d.languages.is_empty(),
            d.absence.is_none(),
            "exactly one of rows and absence: {d:?}"
        );
    }
    assert_eq!(
        ResolutionDenominator::default(),
        ResolutionDenominator::not_available(),
        "a defaulted answer states n/a, never an empty list with no reason"
    );
}

// ── NFR-RA-06: deterministic across runs ─────────────────────────────────────

/// The denominator is identical across repeated queries and across a reopened
/// engine over the same graph ([NFR-RA-06]) — compared as serialised JSON, the
/// form a consumer diffs.
///
/// [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#[test]
fn the_denominator_is_deterministic_across_runs() {
    let tmp = fixture();
    let wire = |engine: &Engine| -> Vec<String> {
        six_answers(engine, "run", "src/util.rs", &["A=alpha", "B=navItems"])
            .into_iter()
            .map(|(tool, _, d)| format!("{tool}:{}", serde_json::to_string(&d).unwrap()))
            .collect()
    };
    let first = {
        let engine = indexed(&tmp);
        let once = wire(&engine);
        assert_eq!(once, wire(&engine), "the same engine, asked twice");
        once
    };
    let reopened = Engine::start(tmp.path()).expect("engine restarts");
    assert_eq!(first, wire(&reopened), "a reopened engine over the same graph");
}

// ── precedent (FR-NV-12 AC 4, CR-143 §3.7) ───────────────────────────────────

/// `precedent`'s empty reason no longer states a coverage gap as a property of
/// the user's code: over a target whose language binds no cross-file call, the
/// reason names that language's state and points at the denominator — on
/// **each** of the two structural codes [CR-143] §3.7 measured, pinned
/// separately so neither arm can lose the clause behind the other.
///
/// `web/labels.ts` defines a constant that calls, implements and registers
/// nothing, so it has no anchor at all; `web/nav.ts` has anchors that nothing
/// else shares.
///
/// [CR-143]: ../../docs/requests/CR-143-a-relational-answer-states-its-resolution-denominator.md
#[test]
fn an_empty_precedent_over_an_unresolved_language_names_the_denominator() {
    let tmp = fixture();
    let engine = indexed(&tmp);
    for (target, code) in [
        ("web/labels.ts", "no_structural_anchors"),
        ("web/nav.ts", "anchors_are_unshared"),
    ] {
        let answer = engine.precedent(target, None);
        let reason = answer
            .empty_reason
            .unwrap_or_else(|| panic!("nothing is analogous to {target} in the fixture"));
        assert_eq!(reason.code.as_str(), code, "{target}: {reason:?}");
        assert!(
            reason.detail.contains("typescript binds no Calls edge across a file boundary")
                && reason.detail.contains("same-file-only")
                && reason.detail.contains("resolution_denominator"),
            "{target}: {:?}",
            reason.detail
        );
        assert_eq!(
            answer.resolution_denominator.languages,
            vec![status_row(&engine, "typescript")],
            "{target}"
        );
    }
}
