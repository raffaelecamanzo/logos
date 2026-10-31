//! A suppression's evidence over a **real index** ([S-818], [FR-SC-32]): the
//! graph-backed half of [`evidence_tests`](super::evidence_tests), which reads
//! its spans from a fixture map.
//!
//! The fixture is the SARIF project of [`tests`](super::tests): Semgrep's first
//! finding sits in `find` (`app/db.py`), and `show` (`app/views.py`) is a
//! symbol in another file for a reason to rest on. These tests cover what the
//! CLI and web flows (which sync after every edit) cannot: a file edited but not
//! yet synced, a project with no index, a name that matches two symbols, a
//! path-wide entry re-confirmed from a finding under it, and the findings filter.
//!
//! [S-818]: ../../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
//! [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md

use std::path::{Path, PathBuf};

use serde_json::Value;
use tempfile::TempDir;

use super::evidence::{EvidenceSource, GraphEvidence, SpanReading};
use super::tests::{fixture, repo_with};
use super::{Selector, IGNORE_FILE};
use crate::security::FindingsFilter;
use crate::Engine;

const VIEWS: &str = "app/views.py";
const REASON: &str = "the id is validated in show";

/// An indexed engine over the fixture, Semgrep's report ingested, and the
/// fingerprint of the finding in `find`.
fn world(extra: &[(&str, &str)]) -> (TempDir, PathBuf, Engine, String) {
    let (tmp, root) = repo_with(extra);
    // `sync` refuses a path outside the root as the OS spells it: use the canonical one.
    let root = root.canonicalize().unwrap();
    std::fs::create_dir_all(root.join(".logos")).unwrap();
    std::fs::write(root.join(".logos/config.toml"), "[project]\nid = \"evidence-fixture\"\n").unwrap();
    let engine = Engine::start(&root).expect("engine starts");
    assert!(engine.index().files_indexed > 0);
    engine.security_ingest(&fixture("semgrep.sarif")).unwrap();
    let findings = engine.security_findings(&FindingsFilter::default()).unwrap();
    let fingerprint = findings.findings.iter().find(|f| f.finding.start_line == Some(12)).unwrap().finding.fingerprint.clone();
    (tmp, root, engine, fingerprint)
}

fn suppression_of(engine: &Engine, fingerprint: &str) -> Value {
    let findings = engine.security_findings(&FindingsFilter::default()).unwrap();
    let listed = findings.findings.iter().find(|f| f.finding.fingerprint == fingerprint).unwrap();
    serde_json::to_value(&listed.suppression).unwrap()
}

fn edit_views(root: &Path, from: &str, to: &str) {
    let path = root.join(VIEWS);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from));
    std::fs::write(path, text.replace(from, to)).unwrap();
}

/// A file edited and not yet synced cannot be checked against the index: the
/// entry stops suppressing and says how to fix it, and holds again once the
/// text is as indexed. A file removed and not synced is gone.
#[test]
fn a_file_edited_but_not_synced_is_unverifiable_and_a_removed_one_is_gone() {
    let (_tmp, root, engine, fingerprint) = world(&[]);
    let written = engine
        .security_suppress_with_evidence(Selector::from(fingerprint.as_str()), REASON, None, &["show".to_string()])
        .unwrap();
    assert_eq!(written.entry.evidence.len(), 2, "show, and the enclosing find");
    assert_eq!(suppression_of(&engine, &fingerprint)["state"], "suppressed");

    edit_views(&root, "'id'", "'uid'");
    let behind = suppression_of(&engine, &fingerprint);
    assert_eq!(behind["state"], "out_of_date", "{behind}");
    let change = &behind["changed"][0];
    assert_eq!((change["name"].as_str(), change["change"].as_str()), (Some("show"), Some("unverifiable")), "{behind}");
    assert!(change["detail"].as_str().unwrap().contains("run `logos sync app/views.py`"), "{behind}");

    edit_views(&root, "'uid'", "'id'");
    assert_eq!(suppression_of(&engine, &fingerprint)["state"], "suppressed", "the text as indexed holds again");

    std::fs::remove_file(root.join(VIEWS)).unwrap();
    let gone = suppression_of(&engine, &fingerprint);
    assert_eq!((gone["state"].as_str(), gone["changed"][0]["change"].as_str()), (Some("out_of_date"), Some("gone")), "{gone}");
}

/// With no index at all nothing can be verified, and a read creates nothing
/// ([ADR-28]): the symbol reads unverifiable naming `logos index`, no
/// `.logos/logos.db` appears, and recording evidence is refused with exit 2.
///
/// [ADR-28]: ../../../../docs/specs/architecture/decisions/ADR-28.md
#[test]
fn a_project_with_no_index_reads_unverifiable_and_a_read_creates_nothing() {
    let tmp = TempDir::new().unwrap();
    let graph = GraphEvidence::at(tmp.path());
    let SpanReading::Unverifiable(why) = graph.read("pkg/`a.py`/f().") else { panic!("no index cannot verify") };
    assert!(why.contains("logos index"), "{why}");
    let err = graph.pin("f").expect_err("nothing to pin against");
    assert_eq!(crate::error::exit_code(&err), 2);
    assert!(!tmp.path().join(".logos").exists(), "a read creates no store");
}

/// A bare name that matches two symbols is refused, listing both, and nothing
/// is written: a verdict is never about a symbol picked by luck.
#[test]
fn a_name_that_matches_two_symbols_is_refused_listing_both_and_writes_nothing() {
    let (_tmp, root, engine, fingerprint) =
        world(&[("app/a.py", "def helper():\n    return 1\n"), ("app/b.py", "def helper():\n    return 2\n")]);
    let err = engine
        .security_suppress_with_evidence(Selector::from(fingerprint.as_str()), REASON, None, &["helper".to_string()])
        .expect_err("ambiguous");
    let message = format!("{err:#}");
    assert_eq!(crate::error::exit_code(&err), 2);
    assert!(message.contains("names 2 symbols") && message.contains("a.py") && message.contains("b.py"), "{message}");
    assert!(!root.join(IGNORE_FILE).exists(), "a refusal writes nothing");
}

/// Naming the finding's own enclosing symbol records it once, and a path-wide
/// entry — re-confirmed from the fingerprint of a finding under it, not its own
/// key — is re-recorded with the original reason. The findings filter reads an
/// out-of-date finding as active.
#[test]
fn a_path_wide_entry_is_re_confirmed_from_a_finding_under_it_and_out_of_date_reads_active() {
    let (_tmp, root, engine, fingerprint) = world(&[]);
    let named_twice = engine
        .security_suppress_with_evidence(Selector::from(fingerprint.as_str()), "kept", None, &["find".to_string(), "show".to_string()])
        .unwrap();
    assert_eq!(named_twice.entry.evidence.len(), 2, "the enclosing symbol, named too, is recorded once");
    engine.security_unsuppress(Selector::from(fingerprint.as_str())).unwrap();

    let path = Selector::from_parts(None, Some("app".into()), None, None).unwrap();
    engine.security_suppress_with_evidence(path.clone(), REASON, None, &["show".to_string()]).unwrap();
    let held = engine.security_findings(&FindingsFilter::default()).unwrap();
    assert!(held.findings.iter().all(|f| f.suppression.is_some()), "the path entry covers both findings");
    let entry = std::fs::read_to_string(root.join(IGNORE_FILE)).unwrap();
    assert!(entry.contains("path = \"app\"") && entry.contains("show"), "{entry}");
    assert!(!entry.contains("find"), "no enclosing symbol for a path entry: {entry}");

    // The literal flips and the index catches up.
    edit_views(&root, "'id'", "'uid'");
    let synced = engine.sync(&[root.join(VIEWS)]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    let out_of_date = suppression_of(&engine, &fingerprint);
    assert_eq!(out_of_date["state"], "out_of_date", "{out_of_date}");
    let filtered = |token: &str| {
        let filter = FindingsFilter::parse(None, None, None, None, Some(token), None, Some("all")).unwrap();
        let listed = engine.security_findings(&filter).unwrap();
        listed.findings.iter().any(|f| f.finding.fingerprint == fingerprint)
    };
    assert!(filtered("active"), "an out-of-date verdict is an active finding");
    assert!(!filtered("suppressed"), "and no longer a suppressed one");

    // Re-confirmed by the finding's fingerprint: the path entry is found through it.
    let written = engine.security_reconfirm(&fingerprint).unwrap();
    assert_eq!(written.entry.selector, path);
    assert_eq!(written.entry.reason, REASON);
    assert_eq!(suppression_of(&engine, &fingerprint)["state"], "suppressed");
    let err = engine.security_reconfirm(&"c".repeat(64)).expect_err("no such entry or finding");
    assert!(format!("{err:#}").contains("no entry or stored finding"), "{err:#}");
}
