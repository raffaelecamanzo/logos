//! Tests for path entries in the ignore file ([S-770], [FR-SC-11]): one entry
//! naming a `path` glob, optionally narrowed by `rule` and `scanner`,
//! suppresses every finding under it, counts them, and is orphaned when it
//! names none; a glob covering the whole project, or an entry naming both a
//! fingerprint and a path, rejects the file with exit 2 at its line.
//!
//! [S-770]: ../../../../docs/planning/journal.md#s-770-a-suppression-can-cover-a-path
//! [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md

use std::path::Path;

use chrono::NaiveDate;

use super::suppress::{canonical_path, label, suppress, unsuppress, IgnoreFile, Labelled, Selector, Suppression, IGNORE_FILE};
use super::{FindingBinding, ScannerConfidence, SecurityFinding, Severity};
use crate::error::exit_code;

const FP_A: &str = "0a7c3e51d9b84f26a1c05e8d7b3f9246e1d08a5c7b2f4963d8e0a1b5c7f92e43";

const ROOT: &str = "/work/project";

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 19).unwrap()
}

/// A finding bound to the indexed `path`, by `scanner` under `rule`, with a
/// fingerprint derived from all three so each is distinct.
fn at(path: &str, scanner: &str, rule: &str) -> SecurityFinding {
    SecurityFinding {
        scanner_name: scanner.into(),
        scanner_version: "1".into(),
        head_sha: "0".repeat(40),
        fingerprint: format!("{:064x}", fxhash(&format!("{scanner}␟{rule}␟{path}"))),
        rule_id: rule.into(),
        severity: Severity::High,
        confidence: ScannerConfidence::Unstated,
        cwes: Vec::new(),
        message: "m".into(),
        redacted: false,
        binding: FindingBinding::Bound,
        report_path: Some(path.into()),
        path: Some(path.into()),
        start_line: Some(1),
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

/// A finding on a file the index does not hold (a lockfile): no bound path,
/// only the path its report named — absolute for osv-scanner, relative for
/// Trivy, as the stored reports have them.
fn unbound(report_path: &str, scanner: &str, rule: &str) -> SecurityFinding {
    let mut finding = at(report_path, scanner, rule);
    finding.binding = FindingBinding::Unmatched;
    finding.path = None;
    finding
}

fn fxhash(text: &str) -> u128 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325_u128, |h, b| (h ^ u128::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn labelled(findings: &[SecurityFinding], text: &str) -> Labelled {
    label(findings, &IgnoreFile::parse(text).unwrap(), Path::new(ROOT), today())
}

/// Which of `findings` came back suppressed, by report path.
fn suppressed(labelled: &Labelled) -> Vec<&str> {
    labelled
        .findings
        .iter()
        .filter(|f| f.is_suppressed())
        .map(|f| f.finding.report_path.as_deref().unwrap())
        .collect()
}

fn rejected(text: &str) -> (String, i32) {
    let err = IgnoreFile::parse(text).expect_err("the file must be rejected");
    (err.to_string(), exit_code(&err))
}

// ── label ───────────────────────────────────────────────────────────────────

#[test]
fn a_path_entry_suppresses_every_finding_under_it_and_counts_them() {
    let findings = [
        at("tests/fixtures/keys.txt", "gitleaks", "generic-api-key"),
        at("tests/fixtures/deep/er/token.json", "gitleaks", "generic-api-key"),
        at("tests/fixtures", "Semgrep OSS", "odd-but-exact"),
        at("tests/fixtures-extra/keys.txt", "gitleaks", "generic-api-key"),
        at("tests/fixture/keys.txt", "gitleaks", "generic-api-key"),
        at("src/tests/fixtures/keys.txt", "gitleaks", "generic-api-key"),
    ];
    let out = labelled(&findings, "[[ignore]]\npath = \"tests/fixtures\"\nreason = \"planted fixtures\"\n");
    assert_eq!(
        suppressed(&out),
        ["tests/fixtures/keys.txt", "tests/fixtures/deep/er/token.json", "tests/fixtures"],
        "the directory and everything under it, never a sibling that only shares its prefix"
    );
    let reasons: Vec<_> = out.findings.iter().filter_map(|f| f.suppression.as_ref()).collect();
    for reason in reasons {
        let Suppression::Suppressed { reason, by, .. } = reason else { panic!("{reason:?}") };
        assert_eq!(reason, "planted fixtures", "each stays listed with the entry's reason");
        assert_eq!(*by, Selector::Path { path: "tests/fixtures".into(), rule: None, scanner: None });
    }
    assert_eq!(out.entries[0].suppressed, 3);
    assert!(!out.entries[0].orphaned);
}

#[test]
fn a_glob_keeps_star_within_a_segment_and_lets_double_star_cross() {
    let findings = [
        at("Cargo.lock", "osv-scanner", "CVE-1"),
        at("crates/a/Cargo.lock", "osv-scanner", "CVE-1"),
        at("web/fixtures/a.txt", "gitleaks", "k"),
        at("web/ui/fixtures/b.txt", "gitleaks", "k"),
        at("web/fixture/c.txt", "gitleaks", "k"),
    ];
    assert_eq!(
        suppressed(&labelled(&findings, "[[ignore]]\npath = \"*.lock\"\nreason = \"r\"\n")),
        ["Cargo.lock"],
        "`*` stays within one segment"
    );
    assert_eq!(
        suppressed(&labelled(&findings, "[[ignore]]\npath = \"**/Cargo.lock\"\nreason = \"r\"\n")),
        ["Cargo.lock", "crates/a/Cargo.lock"]
    );
    assert_eq!(
        suppressed(&labelled(&findings, "[[ignore]]\npath = \"web/**/fixtures\"\nreason = \"r\"\n")),
        ["web/fixtures/a.txt", "web/ui/fixtures/b.txt"],
        "a glob naming a directory covers what is under it"
    );
}

#[test]
fn rule_and_scanner_narrow_a_path_entry() {
    let findings = [
        at("tests/fixtures/a.txt", "gitleaks", "generic-api-key"),
        at("tests/fixtures/b.txt", "gitleaks", "aws-access-token"),
        at("tests/fixtures/c.py", "Semgrep OSS", "generic-api-key"),
    ];
    let by_rule = labelled(&findings, "[[ignore]]\npath = \"tests/fixtures\"\nrule = \"generic-api-key\"\nreason = \"r\"\n");
    assert_eq!(suppressed(&by_rule), ["tests/fixtures/a.txt", "tests/fixtures/c.py"]);

    // The scanner as findings name it, either case; a rule is matched exactly.
    let by_scanner = labelled(&findings, "[[ignore]]\npath = \"tests/fixtures\"\nscanner = \"GITLEAKS\"\nreason = \"r\"\n");
    assert_eq!(suppressed(&by_scanner), ["tests/fixtures/a.txt", "tests/fixtures/b.txt"]);

    let both = labelled(
        &findings,
        "[[ignore]]\npath = \"tests/fixtures\"\nrule = \"generic-api-key\"\nscanner = \"gitleaks\"\nreason = \"r\"\n",
    );
    assert_eq!(suppressed(&both), ["tests/fixtures/a.txt"]);

    let near_miss = labelled(&findings, "[[ignore]]\npath = \"tests/fixtures\"\nrule = \"generic-api-ke\"\nreason = \"r\"\n");
    assert!(suppressed(&near_miss).is_empty(), "a rule one character short matches nothing");
    assert!(near_miss.entries[0].orphaned);
}

#[test]
fn a_glob_matching_nothing_is_orphaned_with_a_zero_count() {
    let findings = [at("src/main.rs", "Semgrep OSS", "r")];
    let out = labelled(&findings, "[[ignore]]\npath = \"tests/fixtures\"\nreason = \"r\"\n");
    assert!(out.entries[0].orphaned);
    assert_eq!(out.entries[0].suppressed, 0);
    assert!(out.findings[0].suppression.is_none());
}

#[test]
fn an_absolute_report_path_matches_relative_to_the_project_root() {
    let text = "[[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-2026-63127\"\nreason = \"r\"\n";
    let osv = |root: &str| unbound(&format!("{root}/Cargo.lock"), "osv-scanner", "CVE-2026-63127");
    let ignore = IgnoreFile::parse(text).unwrap();
    // The same entry suppresses the scan of any checkout, each read from its
    // own root — where a fingerprint entry matched the main checkout's only.
    for root in [ROOT, "/Users/someone/src/logos/.worktrees/sprint-9/I1-S1", "/ci/build"] {
        let out = label(&[osv(root)], &ignore, Path::new(root), today());
        assert!(out.findings[0].is_suppressed(), "{root}");
    }
    // Under a trailing-slash spelling of the root as well.
    let out = label(&[osv(ROOT)], &ignore, Path::new("/work/project/"), today());
    assert!(out.findings[0].is_suppressed());
    // A report path outside the project, or under a root that only shares
    // its prefix, is named by no path entry.
    for elsewhere in ["/elsewhere", "/work/project-two", "/work/projec"] {
        let out = label(&[osv(elsewhere)], &ignore, Path::new(ROOT), today());
        assert!(!out.findings[0].is_suppressed(), "{elsewhere}");
        assert!(out.entries[0].orphaned, "{elsewhere}");
    }
    // A relative report path (Trivy's spelling) needs no root at all.
    let trivy = unbound("Cargo.lock", "Trivy", "CVE-2026-63127");
    assert!(label(&[trivy], &ignore, Path::new(""), today()).findings[0].is_suppressed());
    // And an empty root never makes an absolute path relative.
    let out = label(&[osv("")], &ignore, Path::new(""), today());
    assert!(!out.findings[0].is_suppressed(), "`/Cargo.lock` is not the project's Cargo.lock");
}

#[test]
fn a_finding_without_a_location_is_never_under_a_path() {
    let mut nowhere = at("x", "gitleaks", "k");
    nowhere.path = None;
    nowhere.report_path = None;
    let out = label(&[nowhere], &IgnoreFile::parse("[[ignore]]\npath = \"x\"\nreason = \"r\"\n").unwrap(), Path::new(ROOT), today());
    assert!(out.findings[0].suppression.is_none());
    assert!(out.entries[0].orphaned);
}

#[test]
fn a_fingerprint_entry_labels_a_finding_both_entries_name_and_it_counts_once() {
    let finding = at("tests/fixtures/a.txt", "gitleaks", "k");
    let fingerprint = finding.fingerprint.clone();
    let text = format!(
        "[[ignore]]\npath = \"tests\"\nreason = \"whole tests tree\"\n\n[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"this one\"\n"
    );
    let out = labelled(&[finding], &text);
    let Some(Suppression::Suppressed { reason, .. }) = &out.findings[0].suppression else { panic!() };
    assert_eq!(reason, "this one", "the fingerprint entry sorts, and labels, first");
    let counts: Vec<_> = out.entries.iter().map(|e| (e.suppressed, e.orphaned)).collect();
    assert_eq!(counts, [(1, false), (0, false)], "the path entry names it, so it is no orphan, but counts nothing");
}

#[test]
fn an_expired_entry_gives_way_to_a_live_one_and_alone_leaves_the_finding_back() {
    let finding = at("tests/fixtures/a.txt", "gitleaks", "k");
    let fingerprint = finding.fingerprint.clone();
    let expired_fp = format!("[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"lapsed\"\nexpires = 2026-01-01\n");
    let live_path = "[[ignore]]\npath = \"tests/fixtures\"\nreason = \"live\"\n";
    let out = labelled(std::slice::from_ref(&finding), &format!("{expired_fp}\n{live_path}"));
    let Some(Suppression::Suppressed { reason, .. }) = &out.findings[0].suppression else { panic!() };
    assert_eq!(reason, "live");
    assert_eq!(out.entries.iter().map(|e| e.suppressed).collect::<Vec<_>>(), [0, 1]);

    let out = labelled(&[finding], "[[ignore]]\npath = \"tests/fixtures\"\nreason = \"lapsed\"\nexpires = 2026-01-01\n");
    assert!(matches!(&out.findings[0].suppression, Some(Suppression::Expired { reason, .. }) if reason == "lapsed"));
    assert!(!out.findings[0].is_suppressed());
    assert!(out.entries[0].expired && !out.entries[0].orphaned);
    assert_eq!(out.entries[0].suppressed, 0);
}

#[test]
fn a_path_suppression_serialises_its_selector() {
    let out = labelled(
        &[at("tests/fixtures/a.txt", "gitleaks", "k")],
        "[[ignore]]\npath = \"tests/fixtures\"\nscanner = \"gitleaks\"\nreason = \"r\"\n",
    );
    let json = serde_json::to_value(&out).unwrap();
    assert_eq!(
        json["findings"][0]["suppression"],
        serde_json::json!({"state": "suppressed", "reason": "r", "expires": null, "by": {"path": "tests/fixtures", "scanner": "gitleaks"}})
    );
    assert_eq!(
        json["entries"][0],
        serde_json::json!({
            "entry": {"path": "tests/fixtures", "scanner": "gitleaks", "reason": "r", "expires": null},
            "expired": false, "orphaned": false, "suppressed": 1, "should_name": null, "out_of_date": []
        })
    );
}

// ── parse: rejections, each exit 2 at its line ──────────────────────────────

#[test]
fn a_glob_covering_the_whole_project_rejects_the_file_at_its_line() {
    for glob in ["*", "**", ".", "./", "**/*", "*/**", "**/", "", "  ", "./**", "/"] {
        let text = format!("[[ignore]]\nreason = \"r\"\npath = \"{glob}\"\n");
        let (message, code) = rejected(&text);
        assert_eq!(code, 2, "{glob:?}");
        assert!(message.contains("line 3"), "{glob:?}: {message}");
        assert!(message.contains("covers the whole project"), "{glob:?}: {message}");
    }
    // Spelled otherwise, a glob that matches every name at the root covers
    // every finding just the same.
    for glob in ["?*", "**/?*", "{*}", "{**}", "{*,**}", "{*,x}", "*{,}", "[!/]*"] {
        let (message, code) = rejected(&format!("[[ignore]]\nreason = \"r\"\npath = \"{glob}\"\n"));
        assert_eq!(code, 2, "{glob:?}");
        assert!(message.contains("line 3") && message.contains("covers the whole project"), "{glob:?}: {message}");
    }
    // The near misses: one named segment is enough, and so is leaving out the
    // root's files or its dotfiles.
    for glob in ["*.lock", "**/fixtures", "src/*", "./tests/", "a", "?*/**", "[!.]*"] {
        let text = format!("[[ignore]]\npath = \"{glob}\"\nreason = \"r\"\n");
        assert!(IgnoreFile::parse(&text).is_ok(), "{glob:?}");
    }
}

#[test]
fn an_entry_naming_both_a_fingerprint_and_a_path_is_rejected_at_its_line() {
    let text = format!(
        "[[ignore]]\npath = \"tests\"\nreason = \"r\"\n\n[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"r\"\npath = \"tests/fixtures\"\n"
    );
    let (message, code) = rejected(&text);
    assert_eq!(code, 2);
    assert!(message.contains("line 8"), "{message}");
    assert!(message.contains("both a `fingerprint` and a `path`"), "{message}");
}

#[test]
fn a_narrowing_beside_a_fingerprint_or_left_blank_is_rejected_at_its_line() {
    for key in ["rule", "scanner"] {
        let (message, code) = rejected(&format!("[[ignore]]\nfingerprint = \"{FP_A}\"\n{key} = \"x\"\nreason = \"r\"\n"));
        assert_eq!(code, 2, "{key}");
        assert!(message.contains("line 3"), "{key}: {message}");
        assert!(message.contains("narrow a `path` entry"), "{key}: {message}");

        let (message, code) = rejected(&format!("[[ignore]]\npath = \"tests\"\nreason = \"r\"\n{key} = \" \"\n"));
        assert_eq!(code, 2, "{key}");
        assert!(message.contains("line 4"), "{key}: {message}");
        assert!(message.contains(&format!("`{key}` is empty")), "{key}: {message}");
    }
}

#[test]
fn an_escaping_or_broken_glob_is_rejected_at_its_line() {
    for (glob, needle) in [("../secrets", "escapes the project root"), ("/etc/passwd", "escapes the project root"), ("a/[b", "invalid glob")] {
        let (message, code) = rejected(&format!("[[ignore]]\nreason = \"r\"\npath = \"{glob}\"\n"));
        assert_eq!(code, 2, "{glob}");
        assert!(message.contains("line 3"), "{glob}: {message}");
        assert!(message.contains(needle), "{glob}: {message}");
    }
}

#[test]
fn an_entry_with_neither_selector_still_names_its_header_line() {
    let (message, code) = rejected("\n\n[[ignore]]\nreason = \"r\"\n");
    assert_eq!(code, 2);
    assert!(message.contains("line 3"), "{message}");
    assert!(message.contains("no `fingerprint`") && message.contains("no `path`"), "{message}");
    assert!(message.contains("no `rule` with its `scanner`"), "{message}");
}

#[test]
fn the_same_path_entry_twice_is_rejected_but_a_different_narrowing_is_not() {
    let twice = "[[ignore]]\npath = \"tests\"\nreason = \"one\"\n\n[[ignore]]\npath = \"./tests/\"\nreason = \"two\"\n";
    let (message, code) = rejected(twice);
    assert_eq!(code, 2);
    assert!(message.contains("line 6") && message.contains("already given at line 2"), "{message}");
    assert!(message.contains("duplicate path \"tests\""), "{message}");

    let narrowed = "[[ignore]]\npath = \"tests\"\nreason = \"one\"\n\n[[ignore]]\npath = \"tests\"\nrule = \"k\"\nreason = \"two\"\n";
    assert_eq!(IgnoreFile::parse(narrowed).unwrap().len(), 2);
}

// ── serialise: one canonical spelling ───────────────────────────────────────

#[test]
fn a_path_is_kept_in_one_canonical_spelling() {
    assert_eq!(canonical_path("./tests/fixtures/").unwrap(), "tests/fixtures");
    assert_eq!(canonical_path(" tests\\fixtures ").unwrap(), "tests/fixtures");
    assert_eq!(canonical_path("././a//").unwrap(), "a");
    assert_eq!(canonical_path("tests/fixtures/**").unwrap(), "tests/fixtures/**", "a glob is kept as written");
    assert_eq!(canonical_path("a/./b").unwrap(), "a/b", "a `.` segment names nothing");
    assert_eq!(canonical_path("a//b").unwrap(), "a/b");
}

#[test]
fn the_canonical_spelling_is_a_fixed_point_whatever_the_whitespace() {
    // Whitespace beside a `./` or a `/` once survived one pass and fell to the
    // next, so the file Logos wrote was not in its own canonical spelling.
    for raw in ["tests/fixtures /", "./ tests", " ./tests/ fixtures/ ", "a/ ./b", "a/. /b", "tests\\ fixtures\\"] {
        let once = canonical_path(raw).unwrap();
        assert_eq!(canonical_path(&once).unwrap(), once, "{raw:?}");
        assert!(!once.starts_with(' ') && !once.ends_with(' ') && !once.contains(" /") && !once.contains("/ "), "{raw:?} -> {once:?}");
    }
}

#[test]
fn a_path_logos_wrote_with_stray_whitespace_can_still_be_triaged() {
    let dir = tempfile::tempdir().unwrap();
    let spelled = Selector::Path { path: "tests/fixtures /".into(), rule: None, scanner: None };
    let added = suppress(dir.path(), spelled, "r", None, today()).unwrap();
    assert_eq!(added.entry.selector, Selector::Path { path: "tests/fixtures".into(), rule: None, scanner: None });
    // The next write reads the file as canonical, so neither is refused.
    suppress(dir.path(), Selector::Path { path: "docs".into(), rule: None, scanner: None }, "r", None, today()).unwrap();
    unsuppress(dir.path(), added.entry.selector).unwrap();
    assert_eq!(IgnoreFile::load(dir.path()).unwrap().len(), 1);
}

#[test]
fn path_entries_sort_after_fingerprints_and_a_canonical_file_round_trips_byte_for_byte() {
    let canonical = format!(
        "[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"one\"\n\n\
         [[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-1\"\nreason = \"two\"\nexpires = 2027-04-19\n\n\
         [[ignore]]\npath = \"Cargo.lock\"\nrule = \"CVE-2\"\nscanner = \"osv-scanner\"\nreason = \"three\"\n\n\
         [[ignore]]\npath = \"tests/fixtures\"\nreason = \"four\"\n"
    );
    let file = IgnoreFile::parse(&canonical).unwrap();
    assert_eq!(file.to_toml(), canonical);

    // Any order and the lenient spellings parse to the same canonical text.
    let shuffled = format!(
        "[[ignore]]\nreason = \"four\"\npath = \"./tests/fixtures/\"\n\n\
         [[ignore]]\nscanner = \"osv-scanner\"\nrule = \"CVE-2\"\npath = \"Cargo.lock\"\nreason = \"three\"\n\n\
         [[ignore]]\npath = 'Cargo.lock'\nexpires = \"2027-04-19\"\nrule = \"CVE-1\"\nreason = \"two\"\n\n\
         [[ignore]]\nreason = \"one\"\nfingerprint = \"{FP_A}\"\n"
    );
    assert_eq!(IgnoreFile::parse(&shuffled).unwrap().to_toml(), canonical);
}

// ── triage: the Suppress dialog's "everything under this path" ──────────────

fn root_with(text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".logos")).unwrap();
    std::fs::write(dir.path().join(IGNORE_FILE), text).unwrap();
    dir
}

fn sorted_fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/security-ignore/sorted.toml")
}

#[test]
fn suppressing_a_path_writes_the_canonical_spelling_and_unsuppressing_restores_the_file_byte_for_byte() {
    let before = std::fs::read_to_string(sorted_fixture()).unwrap();
    let dir = root_with(&before);

    let selector = Selector::Path { path: "./tests/fixtures/".into(), rule: None, scanner: Some("gitleaks".into()) };
    let added = suppress(dir.path(), selector, " planted fixtures ", None, today()).unwrap();
    assert_eq!(added.entries, 4);
    assert_eq!(
        added.entry.selector,
        Selector::Path { path: "tests/fixtures".into(), rule: None, scanner: Some("gitleaks".into()) },
        "the entry is returned as stored"
    );
    let written = std::fs::read_to_string(dir.path().join(IGNORE_FILE)).unwrap();
    assert!(
        written.ends_with("\n\n[[ignore]]\npath = \"tests/fixtures\"\nscanner = \"gitleaks\"\nreason = \"planted fixtures\"\n"),
        "the path entry is written last, canonical: {written}"
    );
    assert!(written.starts_with(&before[..before.len() - 1]), "the entries already there are untouched");

    // Removed by the selector a finding's label carries.
    let removed = unsuppress(dir.path(), added.entry.selector.clone()).unwrap();
    assert_eq!(removed.entries, 3);
    let status = std::process::Command::new("cmp")
        .arg(sorted_fixture())
        .arg(dir.path().join(IGNORE_FILE))
        .status()
        .unwrap();
    assert!(status.success(), "cmp reported a difference after add-then-remove");
}

#[test]
fn a_first_path_suppression_undone_leaves_no_file_and_any_spelling_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    suppress(dir.path(), Selector::Path { path: "tests/fixtures".into(), rule: None, scanner: None }, "r", None, today()).unwrap();
    let spelled = Selector::Path { path: "./tests/fixtures/".into(), rule: None, scanner: None };
    unsuppress(dir.path(), spelled).unwrap();
    assert!(!dir.path().join(IGNORE_FILE).exists());
}

#[test]
fn a_refused_path_suppression_writes_nothing() {
    let before = std::fs::read_to_string(sorted_fixture()).unwrap();
    let path = |p: &str, rule: Option<&str>| Selector::Path { path: p.into(), rule: rule.map(str::to_string), scanner: None };
    for (selector, why) in [
        (path("**", None), "covers the whole project"),
        (path("../x", None), "escapes the project root"),
        (path("tests", Some("")), "`rule` is empty"),
    ] {
        let dir = root_with(&before);
        let err = suppress(dir.path(), selector.clone(), "r", None, today()).unwrap_err();
        assert_eq!(exit_code(&err), 2, "{selector:?}");
        assert!(err.to_string().contains(why), "{selector:?}: {err}");
        assert_eq!(std::fs::read_to_string(dir.path().join(IGNORE_FILE)).unwrap(), before);
    }
    let dir = root_with(&before);
    suppress(dir.path(), path("tests", None), "r", None, today()).unwrap();
    let err = suppress(dir.path(), path("./tests", None), "again", None, today()).unwrap_err();
    assert!(err.to_string().contains("already suppressed"), "{err}");
}

#[test]
fn a_surface_selector_is_exactly_one_of_fingerprint_and_path() {
    let some = |s: &str| Some(s.to_string());
    let path = Selector::from_parts(None, some("./tests/"), some("k"), None).unwrap();
    assert_eq!(path, Selector::Path { path: "tests".into(), rule: Some("k".into()), scanner: None });
    assert_eq!(Selector::from_parts(some(FP_A), None, None, None).unwrap(), Selector::from(FP_A));
    for (fingerprint, path, rule, why) in [
        (some(FP_A), some("tests"), None, "never both"),
        (None, None, None, "no `path`"),
        (some(FP_A), None, some("k"), "narrow a `path` entry"),
        (some("nope"), None, None, "not a finding fingerprint"),
        (None, some("*"), None, "covers the whole project"),
    ] {
        let err = Selector::from_parts(fingerprint, path, rule, None).unwrap_err();
        assert_eq!(exit_code(&err), 2, "{why}");
        assert!(err.to_string().contains(why), "{why}: {err}");
    }
}

// ── the repository's own ignore file ([S-777]) ──────────────────────────────
//
// [S-777]: ../../../../docs/planning/journal.md#s-777-unreachable-rmcp-advisories-are-suppressed-with-a-reason-and-an-expiry

/// The four rmcp advisories as the stored reports carry them: osv-scanner
/// 1.9.2 with the absolute `Cargo.lock` of the checkout it ran in, Trivy
/// 0.76.0 with a relative one.
const RMCP_ADVISORIES: [&str; 4] = ["CVE-2026-63127", "CVE-2026-63128", "CVE-2026-64684", "GHSA-c9xm-49cp-xcr9"];

#[test]
fn the_committed_ignore_file_suppresses_the_rmcp_advisories_from_any_checkout() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let committed = IgnoreFile::load(repo).unwrap();
    assert_eq!(committed.to_toml(), std::fs::read_to_string(repo.join(IGNORE_FILE)).unwrap(), "canonical, so triage can still edit it");
    // Only the four rmcp entries are this test's subject: the file may gain
    // other entries without it noticing.
    let mut ignore = IgnoreFile::default();
    for advisory in RMCP_ADVISORIES {
        let selector = Selector::Path { path: "Cargo.lock".into(), rule: Some(advisory.into()), scanner: None };
        let entry = committed.entries().iter().find(|e| e.selector == selector).unwrap_or_else(|| panic!("no entry for {advisory}"));
        assert!(entry.reason.contains("rmcp 1.7.0") && entry.reason.contains(advisory), "{advisory}: {}", entry.reason);
        assert_eq!(entry.expires, NaiveDate::from_ymd_opt(2027, 4, 19), "{advisory}");
        ignore.insert(entry.clone()).unwrap();
    }

    for root in ["/Users/someone/source/logos", "/Users/someone/source/logos/.worktrees/sprint-107/I5-S3", "/home/runner/work/logos"] {
        let mut findings: Vec<SecurityFinding> = Vec::new();
        for advisory in RMCP_ADVISORIES {
            findings.push(unbound(&format!("{root}/Cargo.lock"), "osv-scanner", advisory));
            findings.push(unbound("Cargo.lock", "Trivy", advisory));
        }
        // The near misses: another advisory in the same lockfile, and the
        // same advisory in another lockfile.
        findings.push(unbound(&format!("{root}/Cargo.lock"), "osv-scanner", "CVE-2026-99999"));
        findings.push(unbound(&format!("{root}/web/ui/package-lock.json"), "osv-scanner", "CVE-2026-63127"));

        let out = label(&findings, &ignore, Path::new(root), today());
        let kept: Vec<_> = out.findings.iter().map(|f| f.is_suppressed()).collect();
        assert_eq!(kept, [vec![true; 8], vec![false, false]].concat(), "{root}");
        assert!(out.entries.iter().all(|e| e.suppressed == 2 && !e.orphaned), "{root}: {:?}", out.entries);
    }
}

/// A finding's location relative to the project, as the checked-in security
/// baseline records it ([FR-SC-25]): the root taken off an absolute path under
/// it, a relative one normalised, and none for a path outside it — a sibling
/// directory whose name merely starts with the root's included.
///
/// [FR-SC-25]: ../../../../docs/specs/requirements/FR-SC-25.md
#[test]
fn project_location_is_root_relative_and_none_outside_the_project() {
    use super::project_location;
    for root in [ROOT, "/work/project/"] {
        let root = Path::new(root);
        assert_eq!(project_location(root, "/work/project/Cargo.lock").as_deref(), Some("Cargo.lock"));
        assert_eq!(project_location(root, "/work/project/web/ui/package-lock.json").as_deref(), Some("web/ui/package-lock.json"));
        assert_eq!(project_location(root, "./app/db.py").as_deref(), Some("app/db.py"));
        assert_eq!(project_location(root, "app\\db.py").as_deref(), Some("app/db.py"));
        assert_eq!(project_location(root, "/work/project-old/Cargo.lock"), None, "near miss: a sibling sharing the prefix");
        assert_eq!(project_location(root, "/elsewhere/Cargo.lock"), None);
    }
}

// ── a rule suppressed project-wide ([S-814]) ────────────────────────────────
//
// [S-814]: ../../../../docs/planning/journal.md#s-814-a-rule-can-be-suppressed-project-wide-with-a-reason

const CHECKOV_RULE_WIDE: &str =
    "[[ignore]]\nscanner = \"checkov\"\nrule = \"CKV_K8S_21\"\nreason = \"the namespace is set by the deploy tool\"\n";

#[test]
fn a_rule_wide_entry_suppresses_that_rule_of_that_scanner_in_every_file() {
    let findings = [
        at("k8s/deploy.yaml", "Checkov", "CKV_K8S_21"),
        at("charts/api/templates/pod.yaml", "Checkov", "CKV_K8S_21"),
        at("Dockerfile", "Checkov", "CKV_K8S_21"),
        at("k8s/deploy.yaml", "Checkov", "CKV_K8S_22"),
        at("k8s/deploy.yaml", "Semgrep OSS", "CKV_K8S_21"),
        at("k8s/deploy.yaml", "Checkov", "ckv_k8s_21"),
        at("k8s/deploy.yaml", "Checkov", "CKV_K8S_210"),
        at("k8s/deploy.yaml", "Checkov", "XCKV_K8S_21"),
        at("k8s/deploy.yaml", "Checkov OSS", "CKV_K8S_21"),
    ];
    let out = labelled(&findings, CHECKOV_RULE_WIDE);
    let held: Vec<_> = out.findings.iter().map(|f| f.is_suppressed()).collect();
    assert_eq!(
        held,
        [true, true, true, false, false, false, false, false, false],
        "the rule matches exactly, the scanner in either case and whole"
    );
    for labelled in out.findings.iter().filter(|f| f.is_suppressed()) {
        let Some(Suppression::Suppressed { reason, expires, by, .. }) = &labelled.suppression else { panic!("{labelled:?}") };
        assert_eq!(reason, "the namespace is set by the deploy tool", "each stays listed with the entry's reason");
        assert_eq!(*expires, None);
        assert_eq!(*by, Selector::Rule { rule: "CKV_K8S_21".into(), scanner: "checkov".into() });
    }
    assert_eq!(out.entries[0].suppressed, 3);
    assert!(!out.entries[0].orphaned && !out.entries[0].expired);
}

#[test]
fn a_rule_wide_entry_names_a_finding_with_no_location() {
    let mut nowhere = at("x", "checkov", "CKV_K8S_21");
    nowhere.path = None;
    nowhere.report_path = None;
    let out = labelled(&[nowhere], CHECKOV_RULE_WIDE);
    assert!(out.findings[0].is_suppressed(), "no file is needed to be named by a rule");
    assert_eq!(out.entries[0].suppressed, 1);
}

#[test]
fn a_rule_wide_entry_that_matches_no_finding_is_orphaned() {
    let out = labelled(&[at("k8s/deploy.yaml", "Checkov", "CKV_K8S_22")], CHECKOV_RULE_WIDE);
    assert!(out.entries[0].orphaned);
    assert_eq!(out.entries[0].suppressed, 0);
    assert!(out.findings[0].suppression.is_none());
}

#[test]
fn a_rule_wide_entry_honours_its_expiry_through_the_day_it_names() {
    let findings = [at("k8s/deploy.yaml", "Checkov", "CKV_K8S_21")];
    let text = format!("{CHECKOV_RULE_WIDE}expires = 2026-10-19\n");
    let ignore = IgnoreFile::parse(&text).unwrap();
    let on = |day: u32| label(&findings, &ignore, Path::new(ROOT), NaiveDate::from_ymd_opt(2026, 10, day).unwrap());

    let last_day = on(19);
    assert!(last_day.findings[0].is_suppressed(), "the entry suppresses through its expiry, inclusive");
    assert_eq!(last_day.entries[0].suppressed, 1);

    let after = on(20);
    assert!(!after.findings[0].is_suppressed());
    let Some(Suppression::Expired { reason, expires, by }) = &after.findings[0].suppression else {
        panic!("{:?}", after.findings[0].suppression)
    };
    assert_eq!((reason.as_str(), *expires), ("the namespace is set by the deploy tool", NaiveDate::from_ymd_opt(2026, 10, 19)));
    assert_eq!(*by, Selector::Rule { rule: "CKV_K8S_21".into(), scanner: "checkov".into() });
    assert!(after.entries[0].expired && !after.entries[0].orphaned);
    assert_eq!(after.entries[0].suppressed, 0);
}

#[test]
fn a_rule_wide_entry_labels_after_a_fingerprint_and_a_path_entry_name_the_finding() {
    let finding = at("k8s/deploy.yaml", "Checkov", "CKV_K8S_21");
    let fingerprint = finding.fingerprint.clone();
    let without_fingerprint = format!("{CHECKOV_RULE_WIDE}\n[[ignore]]\npath = \"k8s\"\nreason = \"the k8s tree\"\n");
    let text = format!("{without_fingerprint}\n[[ignore]]\nfingerprint = \"{fingerprint}\"\nreason = \"this one\"\n");
    let out = labelled(std::slice::from_ref(&finding), &text);
    let Some(Suppression::Suppressed { reason, .. }) = &out.findings[0].suppression else { panic!() };
    assert_eq!(reason, "this one", "fingerprint, then path, then rule-wide");
    assert_eq!(out.entries.iter().map(|e| (e.suppressed, e.orphaned)).collect::<Vec<_>>(), [(1, false), (0, false), (0, false)]);

    let out = labelled(&[finding], &without_fingerprint);
    let Some(Suppression::Suppressed { reason, .. }) = &out.findings[0].suppression else { panic!() };
    assert_eq!(reason, "the k8s tree", "a path entry labels before a rule-wide one");
}

#[test]
fn a_rule_wide_suppression_serialises_its_selector_without_a_path_or_fingerprint() {
    let out = labelled(&[at("k8s/deploy.yaml", "Checkov", "CKV_K8S_21")], CHECKOV_RULE_WIDE);
    let json = serde_json::to_value(&out).unwrap();
    assert_eq!(
        json["findings"][0]["suppression"],
        serde_json::json!({
            "state": "suppressed", "reason": "the namespace is set by the deploy tool", "expires": null,
            "by": {"rule": "CKV_K8S_21", "scanner": "checkov"}
        })
    );
    assert_eq!(
        json["entries"][0],
        serde_json::json!({
            "entry": {"rule": "CKV_K8S_21", "scanner": "checkov", "reason": "the namespace is set by the deploy tool", "expires": null},
            "expired": false, "orphaned": false, "suppressed": 1, "should_name": null, "out_of_date": []
        })
    );
}

#[test]
fn an_entry_with_only_a_rule_or_only_a_scanner_is_rejected_naming_the_missing_key() {
    for (keys, line, missing, present) in [
        ("rule = \"CKV_K8S_21\"\nreason = \"r\"\n", 2, "`scanner`", "`rule`"),
        ("reason = \"r\"\nrule = \"CKV_K8S_21\"\n", 3, "`scanner`", "`rule`"),
        ("scanner = \"checkov\"\nreason = \"r\"\n", 2, "`rule`", "`scanner`"),
        ("reason = \"r\"\nscanner = \"checkov\"\n", 3, "`rule`", "`scanner`"),
    ] {
        let (message, code) = rejected(&format!("[[ignore]]\n{keys}"));
        assert_eq!(code, 2, "{keys}");
        assert!(message.contains(&format!("line {line}")), "{keys}: {message}");
        assert!(message.contains(&format!("has a {present} but no {missing}")), "{keys}: {message}");
    }
}

#[test]
fn a_rule_wide_entry_with_a_blank_key_is_rejected_at_its_line() {
    for (key, other) in [("rule", "scanner = \"checkov\""), ("scanner", "rule = \"CKV_K8S_21\"")] {
        let (message, code) = rejected(&format!("[[ignore]]\n{other}\n{key} = \" \"\nreason = \"r\"\n"));
        assert_eq!(code, 2, "{key}");
        assert!(message.contains("line 3") && message.contains(&format!("`{key}` is empty")), "{key}: {message}");
        assert!(!message.contains("leave it out"), "a rule-wide entry cannot leave the key out: {message}");
    }
}

#[test]
fn a_rule_wide_entry_leaves_the_other_rejections_in_force() {
    // A whole-project glob is refused whatever rule and scanner sit beside it.
    for glob in ["*", "**", "."] {
        let text = format!("[[ignore]]\npath = \"{glob}\"\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\nreason = \"r\"\n");
        let (message, code) = rejected(&text);
        assert_eq!(code, 2, "{glob}");
        assert!(message.contains("line 2") && message.contains("covers the whole project"), "{glob}: {message}");
    }
    // A fingerprint takes no narrowing, rule and scanner together included.
    let (message, code) = rejected(&format!(
        "[[ignore]]\nfingerprint = \"{FP_A}\"\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\nreason = \"r\"\n"
    ));
    assert_eq!(code, 2);
    assert!(message.contains("line 3") && message.contains("narrow a `path` entry"), "{message}");
    // Nor does the rule-wide entry take a reason-less spelling or a repeat.
    let (message, _) = rejected("[[ignore]]\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\n");
    assert!(message.contains("no `reason`"), "{message}");
    let twice = format!("{CHECKOV_RULE_WIDE}\n[[ignore]]\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\nreason = \"again\"\n");
    let (message, code) = rejected(&twice);
    assert_eq!(code, 2);
    assert!(message.contains("line 7") && message.contains("already given at line 3"), "{message}");
    assert!(message.contains("duplicate rule \"CKV_K8S_21\" scanner \"checkov\""), "{message}");
}

#[test]
fn rule_wide_entries_sort_last_and_a_canonical_file_round_trips_byte_for_byte() {
    let canonical = format!(
        "[[ignore]]\nfingerprint = \"{FP_A}\"\nreason = \"one\"\n\n\
         [[ignore]]\npath = \"tests/fixtures\"\nreason = \"two\"\n\n\
         [[ignore]]\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\nreason = \"three\"\nexpires = 2027-04-19\n\n\
         [[ignore]]\nrule = \"CKV_K8S_22\"\nscanner = \"checkov\"\nreason = \"four\"\n"
    );
    assert_eq!(IgnoreFile::parse(&canonical).unwrap().to_toml(), canonical);

    // Any key order and the lenient spellings parse to the same canonical text.
    let shuffled = format!(
        "[[ignore]]\nreason = \"four\"\nscanner = \"checkov\"\nrule = \"CKV_K8S_22\"\n\n\
         [[ignore]]\nscanner = 'checkov'\nexpires = \"2027-04-19\"\nreason = \"three\"\nrule = \"CKV_K8S_21\"\n\n\
         [[ignore]]\nreason = \"two\"\npath = \"./tests/fixtures/\"\n\n\
         [[ignore]]\nreason = \"one\"\nfingerprint = \"{FP_A}\"\n"
    );
    assert_eq!(IgnoreFile::parse(&shuffled).unwrap().to_toml(), canonical);
}

#[test]
fn suppressing_a_rule_writes_the_canonical_spelling_and_unsuppressing_restores_the_file_byte_for_byte() {
    let before = std::fs::read_to_string(sorted_fixture()).unwrap();
    let dir = root_with(&before);
    let selector = Selector::Rule { rule: "CKV_K8S_21".into(), scanner: "checkov".into() };
    let added = suppress(dir.path(), selector.clone(), " the deploy tool sets it ", NaiveDate::from_ymd_opt(2027, 1, 1), today()).unwrap();
    assert_eq!((added.entries, &added.entry.selector), (4, &selector));
    let written = std::fs::read_to_string(dir.path().join(IGNORE_FILE)).unwrap();
    assert!(
        written.ends_with(
            "\n\n[[ignore]]\nrule = \"CKV_K8S_21\"\nscanner = \"checkov\"\nreason = \"the deploy tool sets it\"\nexpires = 2027-01-01\n"
        ),
        "the rule-wide entry is written last, canonical: {written}"
    );
    // A second write reads the file as canonical, and the repeat is refused.
    let err = suppress(dir.path(), selector.clone(), "again", None, today()).unwrap_err();
    assert!(err.to_string().contains("already suppressed"), "{err}");

    unsuppress(dir.path(), selector).unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join(IGNORE_FILE)).unwrap(), before, "add-then-remove restores the file");
}

#[test]
fn a_refused_rule_wide_suppression_writes_nothing() {
    let before = std::fs::read_to_string(sorted_fixture()).unwrap();
    for (selector, why) in [
        (Selector::Rule { rule: " ".into(), scanner: "checkov".into() }, "`rule` is empty"),
        (Selector::Rule { rule: "CKV_K8S_21".into(), scanner: String::new() }, "`scanner` is empty"),
    ] {
        let dir = root_with(&before);
        let err = suppress(dir.path(), selector.clone(), "r", None, today()).unwrap_err();
        assert_eq!(exit_code(&err), 2, "{selector:?}");
        assert!(err.to_string().contains(why), "{selector:?}: {err}");
        assert_eq!(std::fs::read_to_string(dir.path().join(IGNORE_FILE)).unwrap(), before);
    }
}

#[test]
fn a_surface_selector_with_a_rule_and_a_scanner_alone_is_rule_wide() {
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        Selector::from_parts(None, None, some("CKV_K8S_21"), some("checkov")).unwrap(),
        Selector::Rule { rule: "CKV_K8S_21".into(), scanner: "checkov".into() }
    );
    for (rule, scanner, missing) in
        [(some("CKV_K8S_21"), None, "has a `rule` but no `scanner`"), (None, some("checkov"), "has a `scanner` but no `rule`")]
    {
        let err = Selector::from_parts(None, None, rule, scanner).unwrap_err();
        assert_eq!(exit_code(&err), 2, "{missing}");
        assert!(err.to_string().contains(missing), "{missing}: {err}");
    }
    let err = Selector::from_parts(None, None, some(" "), some("checkov")).unwrap_err();
    assert!(err.to_string().contains("`rule` is empty"), "{err}");
    // Beside a path it is still a narrowing, and beside a fingerprint still refused.
    assert!(matches!(Selector::from_parts(None, some("tests"), some("k"), some("s")).unwrap(), Selector::Path { .. }));
    let err = Selector::from_parts(some(FP_A), None, some("k"), some("s")).unwrap_err();
    assert!(err.to_string().contains("narrow a `path` entry"), "{err}");
}

#[test]
fn the_same_rule_of_two_scanners_is_two_entries_each_removed_on_its_own() {
    let both = format!("{CHECKOV_RULE_WIDE}\n[[ignore]]\nrule = \"CKV_K8S_21\"\nscanner = \"semgrep\"\nreason = \"semgrep's own\"\n");
    let ignore = IgnoreFile::parse(&both).expect("one rule of two scanners is not a repeat");
    assert_eq!(ignore.len(), 2);
    let out = label(
        &[at("k8s/a.yaml", "checkov", "CKV_K8S_21"), at("k8s/a.yaml", "semgrep", "CKV_K8S_21")],
        &ignore,
        Path::new(ROOT),
        today(),
    );
    assert_eq!(out.entries.iter().map(|e| (e.suppressed, e.orphaned)).collect::<Vec<_>>(), [(1, false), (1, false)]);

    let wide = |scanner: &str| Selector::Rule { rule: "CKV_K8S_21".into(), scanner: scanner.into() };
    let dir = tempfile::tempdir().unwrap();
    suppress(dir.path(), wide("checkov"), "one", None, today()).unwrap();
    suppress(dir.path(), wide("semgrep"), "two", None, today()).expect("the second scanner is not 'already suppressed'");
    let removed = unsuppress(dir.path(), wide("semgrep")).unwrap();
    assert_eq!((removed.entry.selector, removed.entries), (wide("semgrep"), 1));
    let left = IgnoreFile::load(dir.path()).unwrap();
    assert_eq!(left.entries().iter().map(|e| e.selector.clone()).collect::<Vec<_>>(), [wide("checkov")]);
}
