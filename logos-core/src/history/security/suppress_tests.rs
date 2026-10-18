//! Tests for the ignore file and the labeller ([S-666], [FR-SC-11]).
//!
//! The labeller is pure, so every test injects `today` and builds its findings
//! in memory; the parser is exercised on text, and the round trip on the
//! committed sorted fixture.
//!
//! [S-666]: ../../../../docs/planning/journal.md#s-666-suppressions-live-in-a-committed-ignore-file-with-a-reason-and-an-expiry
//! [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md

use std::path::Path;

use chrono::NaiveDate;

use super::suppress::{label, parse_expires, suppress, unsuppress, IgnoreFile, Suppression, IGNORE_FILE};
use super::{FindingBinding, SecurityFinding, Severity};
use crate::config::ConfigError;
use crate::error::exit_code;

const FP_A: &str = "0a7c3e51d9b84f26a1c05e8d7b3f9246e1d08a5c7b2f4963d8e0a1b5c7f92e43";
const FP_B: &str = "6b1f9d04a2c85e37b9d1f06a4c8e2b75d3a9f0c1e6b84a27d5c3f8e90a1b7d62";
const FP_C: &str = "f3d5a8c1e07b4926d1a3f58c0e9b7d24a6c1e5f83b0d9472e8a5c3f1b6d07e94";

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

fn finding(fingerprint: &str) -> SecurityFinding {
    SecurityFinding {
        scanner_name: "Semgrep OSS".into(),
        scanner_version: "1.85.0".into(),
        head_sha: "0".repeat(40),
        fingerprint: fingerprint.into(),
        rule_id: "sql-injection".into(),
        severity: Severity::High,
        cwes: vec!["CWE-89".into()],
        message: "tainted query".into(),
        redacted: false,
        binding: FindingBinding::Bound,
        report_path: Some("app/db.py".into()),
        path: Some("app/db.py".into()),
        start_line: Some(12),
        start_column: None,
        end_line: None,
        end_column: None,
        freshness: "fresh",
    }
}

fn entry(fingerprint: &str, tail: &str) -> String {
    format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\n{tail}")
}

/// The rejection the parser raised: its message and the exit code the surfaces
/// map it to.
fn rejected(text: &str) -> (String, i32) {
    let err = IgnoreFile::parse(text).expect_err("the file must be rejected");
    (err.to_string(), exit_code(&err))
}

// ── parse ───────────────────────────────────────────────────────────────────

#[test]
fn an_entry_without_a_reason_is_rejected_with_exit_2_and_its_line() {
    // The second entry (header on line 5) has no reason.
    let text = format!(
        "{}\n{}",
        entry(FP_A, "reason = \"fixture\"\n"),
        entry(FP_B, "expires = 2026-12-31\n")
    );
    let (message, code) = rejected(&text);
    assert_eq!(code, 2);
    assert!(message.contains("line 5"), "{message}");
    assert!(message.contains("no `reason`"), "{message}");
}

#[test]
fn a_blank_reason_is_rejected_at_the_line_that_holds_it() {
    let (message, code) = rejected(&entry(FP_A, "reason = \"   \"\n"));
    assert_eq!(code, 2);
    assert!(message.contains("line 3"), "{message}");
    assert!(message.contains("`reason` is empty"), "{message}");
}

#[test]
fn a_malformed_date_is_rejected_at_its_line() {
    for (value, needle) in [
        ("\"next tuesday\"", "not a calendar date"),
        ("\"2026-02-30\"", "not a calendar date"),
        ("\"2026-1-5\"", "not a calendar date"),
        ("\"2026-12-31-01\"", "not a calendar date"),
        ("\"12026-12-31\"", "not a calendar date"),
        ("\"2026-012-31\"", "not a calendar date"),
        ("\"2026-12-031\"", "not a calendar date"),
        ("2026-12-31T10:00:00", "carries a time"),
        ("2026-12-31T10:00:00Z", "carries a time"),
        ("20261231", "not a date"),
        ("true", "not a date"),
    ] {
        let (message, code) = rejected(&entry(FP_A, &format!("reason = \"r\"\nexpires = {value}\n")));
        assert_eq!(code, 2, "{value}");
        assert!(message.contains("line 4"), "{value}: {message}");
        assert!(message.contains(needle), "{value}: {message}");
    }
}

#[test]
fn a_date_toml_itself_refuses_is_still_exit_2() {
    let (message, code) = rejected(&entry(FP_A, "reason = \"r\"\nexpires = 2026-13-45\n"));
    assert_eq!(code, 2);
    assert!(message.contains("line 4"), "{message}");
}

#[test]
fn a_repeated_fingerprint_is_rejected_naming_both_lines() {
    let text = format!(
        "{}\n{}",
        entry(FP_A, "reason = \"one\"\n"),
        entry(FP_A, "reason = \"two\"\n")
    );
    let (message, code) = rejected(&text);
    assert_eq!(code, 2);
    assert!(message.contains("line 6"), "{message}");
    assert!(message.contains("already given at line 2"), "{message}");
}

#[test]
fn a_fingerprint_that_is_not_ours_is_rejected_not_silently_orphaned() {
    // One character off in length, an uppercase digit, a non-hex digit, empty.
    for bad in [&FP_A[1..], &FP_A.to_uppercase(), &FP_A.replace('0', "g"), ""] {
        let (message, code) = rejected(&entry(bad, "reason = \"r\"\n"));
        assert_eq!(code, 2, "{bad}");
        assert!(message.contains("line 2"), "{bad}: {message}");
        assert!(message.contains("not a finding fingerprint"), "{bad}: {message}");
    }
}

#[test]
fn an_entry_without_a_fingerprint_names_its_header_line() {
    let (message, code) = rejected("\n[[ignore]]\nreason = \"r\"\n");
    assert_eq!(code, 2);
    assert!(message.contains("line 2"), "{message}");
    assert!(message.contains("no `fingerprint`"), "{message}");
}

#[test]
fn an_unknown_key_or_table_and_broken_toml_are_exit_2() {
    for text in [
        "[[ignore]]\nfingerprint = \"x\"\nreson = \"typo\"\n".to_string(),
        "[suppress]\nfoo = 1\n".to_string(),
        "ignore = 3\n".to_string(),
        "[[ignore]\nfingerprint =\n".to_string(),
    ] {
        let (_, code) = rejected(&text);
        assert_eq!(code, 2, "{text}");
    }
}

#[test]
fn an_unknown_key_inside_an_otherwise_valid_entry_is_rejected() {
    // A misspelt optional key must not silently become "no expiry".
    let (message, code) = rejected(&entry(FP_A, "reason = \"r\"\nexpire = 2026-12-31\n"));
    assert_eq!(code, 2);
    assert!(message.contains("expire"), "{message}");
}

#[test]
fn the_rejection_is_the_config_error_every_surface_already_maps() {
    let err = IgnoreFile::parse(&entry(FP_A, "")).unwrap_err();
    let typed = err.downcast_ref::<ConfigError>().expect("a ConfigError");
    assert_eq!(typed.exit_code(), ConfigError::EXIT_CODE);
    assert!(err.to_string().contains(IGNORE_FILE), "{err}");
}

#[test]
fn a_lenient_file_parses_sorted_with_native_and_quoted_dates() {
    let text = format!(
        "{}\n{}\n{}",
        entry(FP_C, "reason = 'literal string'\nexpires = \"2027-06-30\"\n"),
        entry(FP_A, "expires = 2026-12-31\nreason = \"fixture\"\n"),
        entry(FP_B, "reason = \"no expiry\"\n"),
    );
    let file = IgnoreFile::parse(&text).unwrap();
    let got: Vec<_> = file
        .entries()
        .iter()
        .map(|e| (e.fingerprint.as_str(), e.reason.as_str(), e.expires))
        .collect();
    assert_eq!(
        got,
        [
            (FP_A, "fixture", Some(day(2026, 12, 31))),
            (FP_B, "no expiry", None),
            (FP_C, "literal string", Some(day(2027, 6, 30))),
        ]
    );
}

#[test]
fn an_empty_file_is_no_suppression_and_serialises_to_nothing() {
    for text in ["", "\n", "# only a comment\n"] {
        let file = IgnoreFile::parse(text).unwrap();
        assert!(file.is_empty());
        assert_eq!(file.to_toml(), "");
    }
}

#[test]
fn an_absent_file_loads_empty_and_a_present_one_loads_its_entries() {
    let dir = tempfile::tempdir().unwrap();
    assert!(IgnoreFile::load(dir.path()).unwrap().is_empty());
    assert!(!dir.path().join(".logos").exists(), "loading must create nothing");

    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    std::fs::write(dir.path().join(IGNORE_FILE), entry(FP_A, "reason = \"r\"\n")).unwrap();
    let file = IgnoreFile::load(dir.path()).unwrap();
    assert_eq!(file.len(), 1);

    // A malformed committed file is a loud, typed rejection (exit 2).
    std::fs::write(dir.path().join(IGNORE_FILE), entry(FP_A, "")).unwrap();
    let err = IgnoreFile::load(dir.path()).unwrap_err();
    assert_eq!(exit_code(&err), 2);

    // A TOML-level fault names the real file it was read from.
    std::fs::write(dir.path().join(IGNORE_FILE), "[[ignore]\n").unwrap();
    let err = IgnoreFile::load(dir.path()).unwrap_err();
    assert_eq!(exit_code(&err), 2);
    assert!(err.to_string().contains(&dir.path().display().to_string()), "{err}");
}

#[test]
fn a_file_that_exists_but_cannot_be_read_is_exit_2_not_an_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file belongs: present, unreadable as text.
    std::fs::create_dir_all(dir.path().join(IGNORE_FILE)).unwrap();
    let err = IgnoreFile::load(dir.path()).unwrap_err();
    assert_eq!(exit_code(&err), 2);
    assert!(err.to_string().contains(&dir.path().display().to_string()), "{err}");
}

// ── serialise ───────────────────────────────────────────────────────────────

fn sorted_fixture() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/security-ignore/sorted.toml");
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn the_committed_sorted_fixture_round_trips_byte_for_byte() {
    let text = sorted_fixture();
    let file = IgnoreFile::parse(&text).unwrap();
    assert_eq!(file.len(), 3);
    assert_eq!(file.to_toml().as_bytes(), text.as_bytes());

    // The same comparison the story names, as `cmp` over two files.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.toml");
    std::fs::write(&out, file.to_toml()).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/security-ignore/sorted.toml");
    let status = std::process::Command::new("cmp").arg(&fixture).arg(&out).status().unwrap();
    assert!(status.success(), "cmp reported a difference");
}

#[test]
fn the_fixture_pins_the_canonical_spelling() {
    // Round-tripping alone would pass for any self-consistent writer; this
    // pins the spelling the file promises.
    let text = sorted_fixture();
    assert!(text.starts_with("[[ignore]]\nfingerprint = \""), "{text}");
    assert!(text.contains("\nexpires = 2026-12-31\n\n[[ignore]]\n"), "a native date, blank-line separated");
    assert!(text.ends_with("upstream\"\nexpires = 2027-06-30\n"), "one trailing newline");
    let fingerprints: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("fingerprint = \"")).collect();
    let mut sorted = fingerprints.clone();
    sorted.sort_unstable();
    assert_eq!(fingerprints, sorted, "the fixture is sorted");
}

#[test]
fn serialising_sorts_whatever_order_the_file_was_written_in() {
    let shuffled = format!(
        "{}\n{}",
        entry(FP_C, "reason = \"c\"\n"),
        entry(FP_A, "reason = \"a\"\n")
    );
    let canonical = IgnoreFile::parse(&shuffled).unwrap().to_toml();
    let a = canonical.find(FP_A).unwrap();
    let c = canonical.find(FP_C).unwrap();
    assert!(a < c);
    // And the canonical text is a fixed point.
    assert_eq!(IgnoreFile::parse(&canonical).unwrap().to_toml(), canonical);
}

#[test]
fn a_reason_with_awkward_characters_survives_the_round_trip() {
    for reason in [
        "quote \" and backslash \\ and tab \t",
        "two\nlines\r\nand a bell \u{7} and a delete \u{7f}",
        "unicode — ünïcödé ✓",
        "trailing space ",
    ] {
        let text = format!("[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = {}\n", super::suppress::basic_string(reason));
        let file = IgnoreFile::parse(&text).unwrap();
        assert_eq!(file.entries()[0].reason, reason);
        assert_eq!(file.to_toml(), text, "{reason:?}");
    }
}

#[test]
fn basic_strings_are_spelt_exactly_as_the_canonical_form_promises() {
    use super::suppress::basic_string;
    assert_eq!(basic_string("plain"), "\"plain\"");
    assert_eq!(basic_string("q\"b\\"), "\"q\\\"b\\\\\"");
    assert_eq!(basic_string("\n\r\t"), "\"\\n\\r\\t\"");
    assert_eq!(basic_string("\u{8}\u{c}"), "\"\\b\\f\"");
    assert_eq!(basic_string("\u{1}\u{1f}\u{7f}"), "\"\\u0001\\u001F\\u007F\"");
    assert_eq!(basic_string("ünï ✓"), "\"ünï ✓\"");
}

// ── label ───────────────────────────────────────────────────────────────────

fn file_with_expiry(expires: &str) -> IgnoreFile {
    IgnoreFile::parse(&entry(FP_A, &format!("reason = \"accepted\"\nexpires = {expires}\n"))).unwrap()
}

#[test]
fn a_matching_entry_suppresses_the_finding_and_the_reason_rides_with_it() {
    let file = file_with_expiry("2026-12-31");
    let labelled = label(&[finding(FP_A), finding(FP_B)], &file, day(2026, 10, 16));

    assert!(labelled.findings[0].is_suppressed());
    assert_eq!(
        labelled.findings[0].suppression,
        Some(Suppression::Suppressed { reason: "accepted".into(), expires: Some(day(2026, 12, 31)) })
    );
    assert!(!labelled.findings[1].is_suppressed());
    assert_eq!(labelled.findings[1].suppression, None);
    // Suppressed findings are still listed, in input order, unchanged.
    assert_eq!(labelled.findings.len(), 2);
    assert_eq!(labelled.findings[0].finding, finding(FP_A));
    assert_eq!(labelled.findings[1].finding, finding(FP_B));
}

#[test]
fn an_entry_stops_suppressing_the_day_after_its_expiry() {
    let file = file_with_expiry("2026-12-31");
    let findings = [finding(FP_A)];

    let before = label(&findings, &file, day(2026, 12, 30));
    assert!(before.findings[0].is_suppressed());
    assert!(!before.entries[0].expired);

    // Through the expiry date inclusive.
    let on = label(&findings, &file, day(2026, 12, 31));
    assert!(on.findings[0].is_suppressed());
    assert!(!on.entries[0].expired);

    let after = label(&findings, &file, day(2027, 1, 1));
    assert!(!after.findings[0].is_suppressed(), "an expired entry no longer suppresses");
    assert_eq!(
        after.findings[0].suppression,
        Some(Suppression::Expired { reason: "accepted".into(), expires: Some(day(2026, 12, 31)) }),
        "the finding returns, and says its entry expired"
    );
    assert!(after.entries[0].expired);
    assert!(!after.entries[0].orphaned, "an expired entry that still matches is not an orphan");
}

#[test]
fn an_entry_without_an_expiry_never_expires() {
    let file = IgnoreFile::parse(&entry(FP_A, "reason = \"forever\"\n")).unwrap();
    let labelled = label(&[finding(FP_A)], &file, day(2999, 1, 1));
    assert!(labelled.findings[0].is_suppressed());
    assert!(!labelled.entries[0].expired);
}

#[test]
fn an_entry_that_matches_no_finding_is_orphaned() {
    let text = format!(
        "{}\n{}",
        entry(FP_A, "reason = \"live\"\n"),
        entry(FP_B, "reason = \"fixed long ago\"\n")
    );
    let file = IgnoreFile::parse(&text).unwrap();
    let labelled = label(&[finding(FP_A), finding(FP_C)], &file, day(2026, 10, 16));

    let status: Vec<_> = labelled.entries.iter().map(|e| (e.entry.fingerprint.as_str(), e.orphaned, e.expired)).collect();
    assert_eq!(status, [(FP_A, false, false), (FP_B, true, false)]);
    // An unnamed finding is neither suppressed nor labelled.
    assert_eq!(labelled.findings[1].suppression, None);
}

#[test]
fn an_entry_matching_only_a_later_finding_is_not_an_orphan() {
    let file = IgnoreFile::parse(&entry(FP_B, "reason = \"r\"\n")).unwrap();
    for findings in [[finding(FP_A), finding(FP_B)], [finding(FP_B), finding(FP_A)]] {
        let labelled = label(&findings, &file, day(2026, 10, 16));
        assert!(!labelled.entries[0].orphaned);
    }
}

#[test]
fn an_entry_can_be_both_expired_and_orphaned() {
    let file = file_with_expiry("2026-01-01");
    let labelled = label(&[], &file, day(2026, 10, 16));
    assert!(labelled.entries[0].expired);
    assert!(labelled.entries[0].orphaned);
}

#[test]
fn one_entry_suppresses_every_finding_that_carries_its_fingerprint() {
    // Two scanners reporting the same fingerprint are one accepted risk.
    let mut other = finding(FP_A);
    other.scanner_name = "Bandit".into();
    let labelled = label(&[finding(FP_A), other], &file_with_expiry("2026-12-31"), day(2026, 10, 16));
    assert!(labelled.findings.iter().all(|f| f.is_suppressed()));
    assert!(!labelled.entries[0].orphaned);
}

#[test]
fn no_file_labels_nothing_and_reports_no_entries() {
    let labelled = label(&[finding(FP_A)], &IgnoreFile::default(), day(2026, 10, 16));
    assert_eq!(labelled.findings[0].suppression, None);
    assert!(labelled.entries.is_empty());
}

#[test]
fn the_label_serialises_with_a_state_tag_and_iso_dates() {
    let labelled = label(&[finding(FP_A)], &file_with_expiry("2026-12-31"), day(2027, 2, 1));
    let json = serde_json::to_value(&labelled).unwrap();
    assert_eq!(
        json["findings"][0]["suppression"],
        serde_json::json!({"state": "expired", "reason": "accepted", "expires": "2026-12-31"})
    );
    assert_eq!(json["entries"][0]["entry"]["expires"], "2026-12-31");
    assert_eq!(json["entries"][0]["expired"], true);
    assert_eq!(json["entries"][0]["orphaned"], false);
}

#[test]
fn a_suppressed_label_serialises_with_its_tag_and_a_null_expiry() {
    let file = IgnoreFile::parse(&entry(FP_A, "reason = \"forever\"\n")).unwrap();
    let json = serde_json::to_value(label(&[finding(FP_A)], &file, day(2026, 10, 16))).unwrap();
    assert_eq!(
        json["findings"][0]["suppression"],
        serde_json::json!({"state": "suppressed", "reason": "forever", "expires": null})
    );
    assert_eq!(json["entries"][0]["entry"]["expires"], serde_json::Value::Null);

    let unlabelled = serde_json::to_value(label(&[finding(FP_B)], &file, day(2026, 10, 16))).unwrap();
    assert_eq!(unlabelled["findings"][0]["suppression"], serde_json::Value::Null);
}

#[test]
fn findings_come_back_in_input_order_not_fingerprint_order() {
    let file = IgnoreFile::parse(&entry(FP_B, "reason = \"r\"\n")).unwrap();
    let labelled = label(&[finding(FP_C), finding(FP_A), finding(FP_B)], &file, day(2026, 10, 16));
    let order: Vec<_> = labelled
        .findings
        .iter()
        .map(|f| (f.finding.fingerprint.as_str(), f.is_suppressed()))
        .collect();
    assert_eq!(order, [(FP_C, false), (FP_A, false), (FP_B, true)]);
}

// ── triage: add and remove one entry ([S-672], [FR-SC-12]) ───────────────────
//
// [S-672]: ../../../../docs/planning/journal.md#s-672-findings-are-triaged-from-the-security-page
// [FR-SC-12]: ../../../../docs/specs/requirements/FR-SC-12.md

/// Sorts between the fixture's first and second entries, so the added entry
/// lands mid-file and the round trip proves the removal restores the order.
const FP_MID: &str = "3e8a0c5f1d7b9264a0c3e5f7b1d9a2846c0e2a4f6b8d1c3e5a7f9b2d4c6e8a01";

/// A project root holding `text` as its ignore file.
fn root_with(text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    std::fs::write(dir.path().join(IGNORE_FILE), text).unwrap();
    dir
}

fn on_disk(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(IGNORE_FILE)).ok()
}

const TODAY: (i32, u32, u32) = (2026, 10, 17);

fn today() -> NaiveDate {
    day(TODAY.0, TODAY.1, TODAY.2)
}

#[test]
fn adding_then_removing_an_entry_restores_the_file_byte_for_byte() {
    let dir = root_with(&sorted_fixture());

    let added = suppress(dir.path(), FP_MID, "  reviewed: not reachable  ", Some(day(2027, 1, 31)), today()).unwrap();
    assert_eq!(added.file, IGNORE_FILE);
    assert_eq!(added.entries, 4);
    assert_eq!(added.entry.reason, "reviewed: not reachable", "the reason is stored trimmed");
    let written = on_disk(dir.path()).unwrap();
    let fingerprints: Vec<&str> = written.lines().filter_map(|l| l.strip_prefix("fingerprint = \"")).collect();
    assert_eq!(fingerprints.len(), 4);
    assert!(fingerprints[1].starts_with(FP_MID), "the new entry sorts second: {written}");
    assert!(written.contains("reason = \"reviewed: not reachable\"\nexpires = 2027-01-31\n"), "{written}");

    let removed = unsuppress(dir.path(), FP_MID).unwrap();
    assert_eq!(removed.entries, 3);
    assert_eq!(removed.entry, added.entry);

    // The comparison the story names, as `cmp` over the two files.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/security-ignore/sorted.toml");
    let status = std::process::Command::new("cmp")
        .arg(&fixture)
        .arg(dir.path().join(IGNORE_FILE))
        .status()
        .unwrap();
    assert!(status.success(), "cmp reported a difference after add-then-remove");
}

#[test]
fn the_first_suppression_creates_the_file_and_removing_the_last_deletes_it() {
    let dir = tempfile::tempdir().unwrap();
    let added = suppress(dir.path(), FP_A, "fixture", None, today()).unwrap();
    assert_eq!(added.entries, 1);
    assert_eq!(on_disk(dir.path()).unwrap(), entry(FP_A, "reason = \"fixture\"\n"));

    unsuppress(dir.path(), FP_A).unwrap();
    assert!(!dir.path().join(IGNORE_FILE).exists(), "an absent file is restored as absent");
    assert!(dir.path().join(".logos").is_dir(), "only the file goes, never the directory");
}

#[test]
fn a_refused_suppression_writes_nothing_and_says_why() {
    let text = sorted_fixture();
    let expired_yesterday = today().pred_opt().unwrap();
    let cases: [(&str, &str, Option<NaiveDate>, &str); 5] = [
        (FP_MID, "", None, "`reason` is empty"),
        (FP_MID, " \t\n", None, "`reason` is empty"),
        ("not-a-fingerprint", "r", None, "is not a finding fingerprint"),
        (FP_A, "again", None, "already suppressed"),
        (FP_MID, "r", Some(expired_yesterday), "already past"),
    ];
    for (fingerprint, reason, expires, why) in cases {
        let dir = root_with(&text);
        let err = suppress(dir.path(), fingerprint, reason, expires, today()).unwrap_err();
        assert_eq!(exit_code(&err), 2, "{fingerprint} {reason:?}: {err}");
        assert!(err.downcast_ref::<ConfigError>().is_some(), "a validation fault: {err}");
        assert!(err.to_string().contains(why), "{fingerprint} {reason:?}: {err}");
        assert_eq!(on_disk(dir.path()).unwrap(), text, "a refused write leaves the file as it was");
    }
}

#[test]
fn an_expiry_of_today_is_accepted_because_it_still_suppresses_today() {
    // The near miss of the past-expiry refusal: `expires` is inclusive.
    let dir = root_with(&sorted_fixture());
    let added = suppress(dir.path(), FP_MID, "r", Some(today()), today()).unwrap();
    assert!(!added.entry.is_expired(today()));
}

#[test]
fn removing_an_entry_the_file_does_not_hold_is_refused_and_writes_nothing() {
    let text = sorted_fixture();
    let dir = root_with(&text);
    let err = unsuppress(dir.path(), FP_MID).unwrap_err();
    assert_eq!(exit_code(&err), 2);
    assert!(err.to_string().contains("no entry suppresses"), "{err}");
    assert_eq!(on_disk(dir.path()).unwrap(), text);

    let absent = tempfile::tempdir().unwrap();
    assert!(unsuppress(absent.path(), FP_MID).is_err());
    assert!(!absent.path().join(".logos").exists(), "a refused removal creates nothing");
}

#[test]
fn a_hand_spelled_file_is_never_rewritten_so_its_comments_survive() {
    // Valid, but not the canonical spelling: a rewrite would drop the comment
    // and reorder the entries, so the triage write refuses instead.
    let hand = format!(
        "# accepted risk, reviewed weekly\n{}\n{}",
        entry(FP_B, "reason = \"r\"\n"),
        entry(FP_A, "reason = \"r\"\n")
    );
    for text in [hand.as_str(), "# nothing suppressed yet\n"] {
        let dir = root_with(text);
        let err = suppress(dir.path(), FP_MID, "r", None, today()).unwrap_err();
        assert_eq!(exit_code(&err), 2);
        assert!(err.to_string().contains("not in the canonical spelling"), "{err}");
        assert_eq!(on_disk(dir.path()).unwrap(), text, "the hand-spelled file is untouched");
    }
}

#[test]
fn a_malformed_file_is_refused_before_any_write() {
    let dir = root_with(&entry(FP_A, ""));
    let err = suppress(dir.path(), FP_MID, "r", None, today()).unwrap_err();
    assert_eq!(exit_code(&err), 2);
    assert_eq!(on_disk(dir.path()).unwrap(), entry(FP_A, ""));
}

#[test]
fn an_expiry_sent_as_text_is_read_strictly_as_a_calendar_date() {
    assert_eq!(parse_expires("2026-12-31").unwrap(), day(2026, 12, 31));
    assert_eq!(parse_expires(" 2026-12-31 ").unwrap(), day(2026, 12, 31), "a date input's padding is tolerated");
    for text in ["2026-1-31", "2026-02-30", "2026-12-31T00:00", "31/12/2026", ""] {
        let err = parse_expires(text).expect_err(text);
        assert_eq!(exit_code(&err), 2, "{text}");
        assert!(err.to_string().contains("not a calendar date"), "{text}: {err}");
    }
}

#[cfg(unix)]
#[test]
fn an_ignore_file_that_cannot_be_read_is_refused_and_left_as_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let text = sorted_fixture();
    let dir = root_with(&text);
    let path = dir.path().join(IGNORE_FILE);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&path).is_ok() {
        // Running as root: permissions do not stop the read, so nothing to prove.
        return;
    }
    let result = suppress(dir.path(), FP_MID, "r", None, today());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let err = result.expect_err("an unreadable file must be refused, never treated as absent");
    assert!(matches!(err.downcast_ref::<ConfigError>(), Some(ConfigError::Io { .. })), "{err}");
    assert_eq!(on_disk(dir.path()).unwrap(), text, "the committed entries survive");
}

#[cfg(unix)]
#[test]
fn a_failed_write_is_reported_and_the_file_is_left_as_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let text = sorted_fixture();
    let dir = root_with(&text);
    let logos = dir.path().join(".logos");
    std::fs::set_permissions(&logos, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(logos.join("probe"), "").is_ok() {
        // Running as root: a read-only directory does not stop the write.
        std::fs::set_permissions(&logos, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let result = suppress(dir.path(), FP_MID, "r", None, today());
    std::fs::set_permissions(&logos, std::fs::Permissions::from_mode(0o755)).unwrap();

    let err = result.expect_err("a write that did not happen must not report success");
    assert!(matches!(err.downcast_ref::<ConfigError>(), Some(ConfigError::Write { .. })), "{err}");
    assert_eq!(on_disk(dir.path()).unwrap(), text);
}

#[test]
fn concurrent_suppressions_keep_every_entry() {
    // Each write reads, edits and publishes the whole file: without the
    // serialisation, a later publish drops the entries written meanwhile.
    let dir = tempfile::tempdir().unwrap();
    std::thread::scope(|scope| {
        for i in 0..32u32 {
            let root = dir.path();
            scope.spawn(move || suppress(root, &format!("{i:064x}"), "r", None, today()).unwrap());
        }
    });
    let file = IgnoreFile::load(dir.path()).unwrap();
    assert_eq!(file.len(), 32, "every concurrent suppression is in the file");
}
