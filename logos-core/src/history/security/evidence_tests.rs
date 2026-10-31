//! Tests for a suppression's evidence ([S-818], [FR-SC-32]).
//!
//! The labeller reads evidence through [`EvidenceSource`], so the pure tests
//! here inject a map of span hashes and prove each acceptance criterion with no
//! graph: a hash that matches holds, one that differs or is gone re-opens the
//! finding, an expired entry is never read, and an entry without evidence is
//! unchanged. The graph-backed half, over a real index, is in
//! [`engine_evidence_tests`](super::engine_evidence_tests): file edited but not synced, no index,
//! an ambiguous name, a path-wide entry re-confirmed from a finding it covers.
//!
//! [S-818]: ../../../../docs/planning/journal.md#s-818-a-suppression-records-its-evidence-and-re-opens-when-the-evidence-changes
//! [FR-SC-32]: ../../../../docs/specs/requirements/FR-SC-32.md

use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDate;

use super::evidence::{ChangeKind, Evidence, EvidenceSource, NoGraph, SpanReading};
use super::suppress::{label_with, reconfirm, suppress_with_evidence, IgnoreFile, Selector, Suppression, IGNORE_FILE};
use super::{FindingBinding, ScannerConfidence, SecurityFinding, Severity};

const FP_A: &str = "0a7c3e51d9b84f26a1c05e8d7b3f9246e1d08a5c7b2f4963d8e0a1b5c7f92e43";
const FP_B: &str = "6b1f9d04a2c85e37b9d1f06a4c8e2b75d3a9f0c1e6b84a27d5c3f8e90a1b7d62";
const ROOT: &str = "/work/project";
const CONFIG: &str = "internal/`cookie.go`/DefaultCookieConfig().";
const OTHER: &str = "internal/`cookie.go`/Other().";

fn hash(of: &str) -> String {
    blake3::hash(of.as_bytes()).to_hex().to_string()
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn finding(fingerprint: &str) -> SecurityFinding {
    SecurityFinding {
        scanner_name: "Semgrep OSS".into(),
        scanner_version: "1.85.0".into(),
        head_sha: "0".repeat(40),
        fingerprint: fingerprint.into(),
        rule_id: "cookie-missing-secure".into(),
        severity: Severity::Medium,
        confidence: ScannerConfidence::Unstated,
        cwes: Vec::new(),
        message: "cookie without Secure".into(),
        redacted: false,
        binding: FindingBinding::Bound,
        report_path: Some("internal/handler.go".into()),
        path: Some("internal/handler.go".into()),
        start_line: Some(12),
        start_column: None,
        end_line: None,
        end_column: None,
        commits: Vec::new(),
        legacy_fingerprints: Vec::new(),
        anchor: None,
        freshness: "fresh",
        reporters: Vec::new(),
    }
}

/// A source answering from a fixed map; a symbol it does not hold is gone.
struct Spans(HashMap<&'static str, SpanReading>);

impl Spans {
    fn of(items: &[(&'static str, SpanReading)]) -> Self {
        Self(items.iter().cloned().collect())
    }
}

impl EvidenceSource for Spans {
    fn read(&self, symbol: &str) -> SpanReading {
        self.0.get(symbol).cloned().unwrap_or(SpanReading::Gone)
    }
}

fn recorded(symbol: &str, source_text: &str) -> Evidence {
    Evidence { symbol: symbol.into(), hash: hash(source_text) }
}

fn file_with(evidence: &[Evidence], tail: &str) -> IgnoreFile {
    let mut text = format!("[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"the cookie is Secure by default\"\n{tail}");
    if !evidence.is_empty() {
        text.push_str("evidence = [\n");
        for item in evidence {
            text.push_str(&format!("    {{ symbol = \"{}\", hash = \"{}\" }},\n", item.symbol, item.hash));
        }
        text.push_str("]\n");
    }
    IgnoreFile::parse(&text).unwrap()
}

fn labelled(file: &IgnoreFile, source: &dyn EvidenceSource, today: NaiveDate) -> Option<Suppression> {
    label_with(&[finding(FP_A)], file, Path::new(ROOT), today, source).findings.remove(0).suppression
}

const SECURE: &str = "Secure: true";
const INSECURE: &str = "Secure: false";

/// FR-SC-32 acceptance 1: an entry whose evidence is `DefaultCookieConfig`
/// suppresses while that symbol's span is unchanged, and says what it rests on.
#[test]
fn an_entry_suppresses_while_every_evidence_span_matches() {
    let file = file_with(&[recorded(CONFIG, SECURE)], "");
    let source = Spans::of(&[(CONFIG, SpanReading::Hash(hash(SECURE)))]);
    let Some(Suppression::Suppressed { reason, evidence, .. }) = labelled(&file, &source, day(2026, 10, 26)) else {
        panic!("the unchanged evidence suppresses");
    };
    assert_eq!(reason, "the cookie is Secure by default");
    assert_eq!(evidence.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["DefaultCookieConfig"]);
}

/// FR-SC-32 acceptance 2: `Secure: true` to `Secure: false` re-opens it as out
/// of date, naming the symbol, and the finding no longer reads suppressed — so
/// every count, the gate and a cap see it.
#[test]
fn a_changed_literal_re_opens_the_finding_naming_the_symbol() {
    let file = file_with(&[recorded(CONFIG, SECURE)], "");
    let source = Spans::of(&[(CONFIG, SpanReading::Hash(hash(INSECURE)))]);
    let out = label_with(&[finding(FP_A)], &file, Path::new(ROOT), day(2026, 10, 26), &source);
    let item = &out.findings[0];
    let Some(Suppression::OutOfDate { reason, changed, .. }) = &item.suppression else {
        panic!("a changed span is out of date: {:?}", item.suppression);
    };
    assert_eq!(reason, "the cookie is Secure by default", "the reason it was given for stays visible");
    assert_eq!((changed[0].name.as_str(), changed[0].change), ("DefaultCookieConfig", ChangeKind::Changed));
    assert_eq!(changed[0].describe(), "`DefaultCookieConfig` changed");
    assert!(!item.is_suppressed(), "an out-of-date verdict suppresses nothing");
    assert!(
        super::suppress::counts(&item.finding, item.suppression.as_ref(), super::CodeScope::Production),
        "it counts in the headline, the gate and every cap"
    );
    assert_eq!(out.entries[0].suppressed, 0);
    assert_eq!(out.entries[0].out_of_date.len(), 1, "the entry says which symbol changed");
}

/// FR-SC-32 acceptance 3: deleting an evidence symbol re-opens it the same way.
#[test]
fn a_deleted_evidence_symbol_re_opens_the_finding() {
    let file = file_with(&[recorded(CONFIG, SECURE)], "");
    let Some(Suppression::OutOfDate { changed, .. }) = labelled(&file, &Spans::of(&[]), day(2026, 10, 26)) else {
        panic!("a deleted symbol is out of date");
    };
    assert_eq!((changed[0].name.as_str(), changed[0].change), ("DefaultCookieConfig", ChangeKind::Gone));
    assert_eq!(changed[0].describe(), "`DefaultCookieConfig` no longer exists");
}

/// One symbol out of several is enough, and only it is named.
#[test]
fn one_changed_symbol_of_several_re_opens_it_and_only_it_is_named() {
    let file = file_with(&[recorded(CONFIG, SECURE), recorded(OTHER, "x")], "");
    let source = Spans::of(&[(CONFIG, SpanReading::Hash(hash(SECURE))), (OTHER, SpanReading::Hash(hash("y")))]);
    let Some(Suppression::OutOfDate { changed, .. }) = labelled(&file, &source, day(2026, 10, 26)) else { panic!() };
    assert_eq!(changed.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Other"]);
}

/// Evidence that cannot be checked never holds: no graph, or an index behind.
#[test]
fn evidence_that_cannot_be_checked_does_not_suppress() {
    let file = file_with(&[recorded(CONFIG, SECURE)], "");
    let Some(Suppression::OutOfDate { changed, .. }) = labelled(&file, &NoGraph, day(2026, 10, 26)) else { panic!() };
    assert_eq!(changed[0].change, ChangeKind::Unverifiable);
    assert!(changed[0].describe().starts_with("`DefaultCookieConfig` could not be checked: "), "{}", changed[0].describe());

    let behind = Spans::of(&[(CONFIG, SpanReading::Unverifiable("the index is behind internal/cookie.go: run `logos sync`".into()))]);
    let Some(Suppression::OutOfDate { changed, .. }) = labelled(&file, &behind, day(2026, 10, 26)) else { panic!() };
    assert!(changed[0].describe().contains("run `logos sync`"), "{}", changed[0].describe());
}

/// FR-SC-32 acceptance 5: an entry without evidence suppresses exactly as before,
/// is never checked against the graph, and records nothing.
#[test]
fn an_entry_without_evidence_suppresses_as_before_and_reads_no_evidence() {
    let file = file_with(&[], "");
    let Some(Suppression::Suppressed { evidence, .. }) = labelled(&file, &NoGraph, day(2026, 10, 26)) else {
        panic!("no evidence is never checked, so it holds");
    };
    assert!(evidence.is_empty(), "no symbols is what surfaces read as \"no evidence recorded\"");
    let json = serde_json::to_value(labelled(&file, &NoGraph, day(2026, 10, 26))).unwrap();
    assert!(json.get("evidence").is_none(), "an entry without evidence serialises exactly as before: {json}");
}

/// An expired entry says expired, not out of date, and its evidence is not read.
#[test]
fn an_expired_entry_is_not_checked_for_evidence() {
    let file = file_with(&[recorded(CONFIG, SECURE)], "expires = 2026-10-01\n");
    assert!(matches!(labelled(&file, &NoGraph, day(2026, 10, 26)), Some(Suppression::Expired { .. })));
    assert!(label_with(&[], &file, Path::new(ROOT), day(2026, 10, 26), &NoGraph).entries[0].out_of_date.is_empty());
}

/// Two entries naming one finding are two verdicts: a second that still holds
/// suppresses it though the first went out of date.
#[test]
fn a_second_entry_that_holds_still_suppresses() {
    let text = format!(
        "[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"a\"\nevidence = [\n    {{ symbol = \"{CONFIG}\", hash = \"{}\" }},\n]\n\n[[ignore]]\npath = \"internal\"\nreason = \"b\"\n",
        hash(SECURE)
    );
    let file = IgnoreFile::parse(&text).unwrap();
    let source = Spans::of(&[(CONFIG, SpanReading::Hash(hash(INSECURE)))]);
    let Some(Suppression::Suppressed { reason, .. }) = labelled(&file, &source, day(2026, 10, 26)) else { panic!() };
    assert_eq!(reason, "b");
}

// ── the file ────────────────────────────────────────────────────────────────

#[test]
fn evidence_round_trips_byte_for_byte_in_its_canonical_spelling() {
    let text = format!(
        "[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"r\"\nexpires = 2999-12-31\nevidence = [\n    {{ symbol = \"{CONFIG}\", hash = \"{}\" }},\n    {{ symbol = \"{OTHER}\", hash = \"{}\" }},\n]\n",
        hash("a"),
        hash("b")
    );
    let file = IgnoreFile::parse(&text).unwrap();
    assert_eq!(file.entries()[0].evidence.len(), 2);
    assert_eq!(file.to_toml(), text);
}

#[test]
fn a_malformed_evidence_item_is_rejected_at_its_line() {
    let at = |tail: &str| {
        let text = format!("[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"r\"\n{tail}");
        let err = IgnoreFile::parse(&text).expect_err("rejected");
        (err.to_string(), crate::error::exit_code(&err))
    };
    let (message, code) = at("evidence = [\n    { symbol = \"s\" },\n]\n");
    assert_eq!(code, 2);
    assert!(message.contains("line 5") && message.contains("`symbol` and a `hash`"), "{message}");
    let (message, _) = at("evidence = [\n    { symbol = \"s\", hash = \"nope\" },\n]\n");
    assert!(message.contains("not a span hash"), "{message}");
    let (message, _) = at(&format!("evidence = [{{ symbol = \"s\", hash = \"{0}\" }}, {{ symbol = \"s\", hash = \"{0}\" }}]\n", hash("x")));
    assert!(message.contains("names `s` twice"), "{message}");
    let (message, _) = at(&format!("evidence = [{{ symbol = \"s\", hash = \"{}\", extra = 1 }}]\n", hash("x")));
    assert!(message.contains("extra"), "an unknown key is refused: {message}");
}

// ── write and reconfirm ─────────────────────────────────────────────────────

fn project() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join(".logos")).unwrap();
    tmp
}

/// FR-SC-32 acceptance 4: `reconfirm` re-records the hashes, keeps the reason
/// and the expiry, and leaves the rest of the file as it was.
#[test]
fn reconfirm_re_records_the_hashes_and_keeps_the_reason() {
    let root = project();
    let ignore = root.path().join(IGNORE_FILE);
    suppress_with_evidence(root.path(), FP_B, "kept", None, day(2026, 10, 26), Vec::new()).unwrap();
    suppress_with_evidence(
        root.path(),
        FP_A,
        "the cookie is Secure by default",
        Some(day(2999, 12, 31)),
        day(2026, 10, 26),
        vec![recorded(CONFIG, SECURE)],
    )
    .unwrap();
    let before = std::fs::read_to_string(&ignore).unwrap();

    let write = reconfirm(root.path(), &Selector::from(FP_A), day(2026, 10, 26), |old| {
        assert_eq!(old[0].symbol, CONFIG, "the entry's own symbols are asked for");
        Ok(vec![recorded(CONFIG, INSECURE)])
    })
    .unwrap();
    assert_eq!(write.entry.reason, "the cookie is Secure by default");
    assert_eq!(write.entry.expires, Some(day(2999, 12, 31)));
    let after = std::fs::read_to_string(&ignore).unwrap();
    assert_eq!(after.replace(&hash(INSECURE), &hash(SECURE)), before, "only the one hash moved");
    assert!(!after.contains(&hash(SECURE)));
}

#[test]
fn reconfirm_refuses_what_it_cannot_confirm_and_writes_nothing() {
    let root = project();
    let ignore = root.path().join(IGNORE_FILE);
    suppress_with_evidence(root.path(), FP_B, "no evidence", None, day(2026, 10, 26), Vec::new()).unwrap();
    suppress_with_evidence(root.path(), FP_A, "with", None, day(2026, 10, 26), vec![recorded(CONFIG, SECURE)]).unwrap();
    let before = std::fs::read(&ignore).unwrap();
    let refused = |selector: &str, current: Result<Vec<Evidence>, &str>| {
        let err = reconfirm(root.path(), &Selector::from(selector), day(2026, 10, 26), |_| current.map_err(|m| anyhow::anyhow!("{m}")))
            .expect_err("refused");
        (err.to_string(), crate::error::exit_code(&err))
    };
    let (message, code) = refused(FP_B, Ok(Vec::new()));
    assert_eq!(code, 2);
    assert!(message.contains("records no evidence"), "{message}");
    let (message, _) = refused(&"c".repeat(64), Ok(Vec::new()));
    assert!(message.contains("no entry suppresses"), "{message}");
    let (message, _) = refused(FP_A, Err("`DefaultCookieConfig` no longer exists"));
    assert!(message.contains("no longer exists"), "{message}");
    assert_eq!(std::fs::read(&ignore).unwrap(), before, "nothing was written");
}

/// A re-confirm that names its symbols records exactly those, so an entry that
/// recorded none gains evidence and one whose symbol was deleted can swap it.
#[test]
fn reconfirm_can_name_the_symbols_the_verdict_now_rests_on() {
    let root = project();
    suppress_with_evidence(root.path(), FP_B, "none yet", None, day(2026, 10, 26), Vec::new()).unwrap();
    let write = reconfirm(root.path(), &Selector::from(FP_B), day(2026, 10, 26), |old| {
        assert!(old.is_empty());
        Ok(vec![recorded(OTHER, "y"), recorded(CONFIG, SECURE), recorded(CONFIG, SECURE)])
    })
    .unwrap();
    assert_eq!(
        write.entry.evidence.iter().map(|e| e.symbol.as_str()).collect::<Vec<_>>(),
        [CONFIG, OTHER],
        "sorted, and a symbol named twice is recorded once"
    );
}

fn hash_of(symbol: &str) -> String {
    format!("    {{ symbol = \"{symbol}\", hash = \"{}\" }},\n", hash(symbol))
}

/// Evidence is part of the canonical spelling: unsorted items parse (the parser
/// is lenient) but are not what the writer produces, so a triage write refuses
/// the file rather than reorder it; a symbol twice is rejected wherever it sits.
#[test]
fn evidence_out_of_order_is_not_canonical_and_a_repeat_is_rejected_wherever_it_sits() {
    let entry = |items: &str| format!("[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"r\"\nevidence = [\n{items}]\n");
    let unsorted = entry(&format!("{}{}", hash_of("b"), hash_of("a")));
    let file = IgnoreFile::parse(&unsorted).unwrap();
    assert_ne!(file.to_toml(), unsorted, "the writer sorts by symbol");
    let root = project();
    std::fs::write(root.path().join(IGNORE_FILE), &unsorted).unwrap();
    let err = reconfirm(root.path(), &Selector::from(FP_A), day(2026, 10, 26), |old| Ok(old.to_vec())).expect_err("refused");
    assert!(err.to_string().contains("canonical spelling"), "{err}");
    assert_eq!(std::fs::read_to_string(root.path().join(IGNORE_FILE)).unwrap(), unsorted, "nothing was written");

    let err = IgnoreFile::parse(&entry(&format!("{}{}{}", hash_of("a"), hash_of("b"), hash_of("a")))).expect_err("a, b, a");
    assert!(err.to_string().contains("names `a` twice"), "{err}");
}

/// A writer refuses evidence the file would reject, and writes nothing.
#[test]
fn suppress_with_evidence_refuses_a_repeat_a_bad_hash_and_a_padded_symbol() {
    let root = project();
    let ok = recorded("a", "x");
    for bad in [
        vec![ok.clone(), ok.clone()],
        vec![Evidence { symbol: "a".into(), hash: "nope".into() }],
        vec![Evidence { symbol: " a".into(), hash: hash("x") }],
    ] {
        let err = suppress_with_evidence(root.path(), FP_A, "r", None, day(2026, 10, 26), bad).expect_err("refused");
        assert_eq!(crate::error::exit_code(&err), 2);
        assert!(!root.path().join(IGNORE_FILE).exists(), "a refusal writes nothing");
    }
}

/// An expired entry suppresses nothing, so re-confirming it would change
/// nothing while reading as if it had: it is refused, naming the date.
#[test]
fn reconfirm_refuses_an_expired_entry() {
    let root = project();
    suppress_with_evidence(root.path(), FP_A, "r", Some(day(2026, 10, 30)), day(2026, 10, 26), vec![recorded(CONFIG, SECURE)]).unwrap();
    let before = std::fs::read(root.path().join(IGNORE_FILE)).unwrap();
    let err = reconfirm(root.path(), &Selector::from(FP_A), day(2026, 11, 1), |old| Ok(old.to_vec())).expect_err("expired");
    assert!(err.to_string().contains("expired on 2026-10-30"), "{err}");
    assert_eq!(std::fs::read(root.path().join(IGNORE_FILE)).unwrap(), before);
}
