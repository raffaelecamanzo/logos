//! The ratchet end to end through the [`Engine`] façade (S-507, [FR-GV-23]):
//! real extraction, real persistence, a real `.logos/rules-baseline.json`.
//!
//! - `check --bless` writes a sorted, deterministic blessed set, byte-identical
//!   when re-blessed (the AC's bless-twice `cmp`);
//! - `check`, `gate` and `scan` never modify it — bytes and mtime;
//! - blessed debt passes `check` (`passed == Some(true)`, the exit-0 path);
//!   worsened and new debt fail it; reduced debt is reported fixed;
//! - with no blessed file every ratchet offender is an error and the report
//!   names the bless command.
//!
//! The pure classification table (F at 22/23, G at 16, F at 14) is pinned in
//! `governance/tests.rs`; this file proves the file-system half.
//!
//! [FR-GV-23]: ../../docs/specs/requirements/FR-GV-23.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use logos_core::models::{RatchetStatus, RulesReport};
use logos_core::Engine;
use tempfile::TempDir;

const RULES: &str = "[constraints]\nmax_cc = { max = 2, ratchet = true }\n";

/// A function with `branches` sequential `if`s — cyclomatic complexity
/// `1 + branches` — named `name`.
fn branchy(name: &str, branches: usize) -> String {
    let mut body = format!("pub fn {name}(x: i32) -> i32 {{\n    let mut y = 0;\n");
    for i in 0..branches {
        body.push_str(&format!("    if x > {i} {{\n        y += 1;\n    }}\n"));
    }
    body.push_str("    y\n}\n");
    body
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A project whose only source file is `src/lib.rs` = `source`, under the
/// ratchet contract.
fn project(source: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), ".logos/rules.toml", RULES);
    write(tmp.path(), "src/lib.rs", source);
    tmp
}

fn blessed_file(root: &Path) -> PathBuf {
    root.join(".logos").join("rules-baseline.json")
}

/// The `max_cc` offenders of a report as `(fn name in the message, severity,
/// status)`, in report order.
fn offenders(report: &RulesReport) -> Vec<(String, String, RatchetStatus)> {
    report
        .violations
        .iter()
        .filter(|v| v.rule == "max_cc")
        .map(|v| {
            let name = v.message.split('`').nth(1).unwrap_or_default().to_string();
            let status = v.ratchet.as_ref().expect("a ratchet offender is marked").status;
            (name, v.severity.clone(), status)
        })
        .collect()
}

fn stamp(path: &Path) -> (Vec<u8>, SystemTime) {
    (
        fs::read(path).expect("the blessed set exists"),
        fs::metadata(path).unwrap().modified().unwrap(),
    )
}

#[test]
fn with_no_blessed_file_every_ratchet_offender_is_an_error_naming_bless() {
    let tmp = project(&branchy("f", 3));
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check runs");
    assert_eq!(report.passed, Some(false), "{:?}", report.violations);
    assert_eq!(
        offenders(&report),
        [("f".to_string(), "error".to_string(), RatchetStatus::New)]
    );
    assert!(report.violations[0].message.contains("logos check --bless"));
    let ratchet = report.ratchet.as_ref().expect("a ratchet summary");
    assert!(!ratchet.blessed_file_present);
    assert_eq!(ratchet.bless_command, "logos check --bless");
    assert!(
        report.notes.iter().any(|n| n.contains("logos check --bless")),
        "the report names the bless command: {:?}",
        report.notes
    );
    assert!(!blessed_file(tmp.path()).exists(), "check never writes the blessed set");
}

#[test]
fn bless_writes_a_deterministic_set_and_check_gate_scan_never_touch_it() {
    let tmp = project(&format!("{}{}", branchy("f", 3), branchy("g", 4)));
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let path = blessed_file(tmp.path());

    let blessed = engine.bless_rules(None, true).expect("bless runs");
    assert_eq!(blessed.passed, Some(true), "blessed debt passes: {:?}", blessed.violations);
    assert_eq!(blessed.ratchet.as_ref().unwrap().written, Some(2));
    let first = fs::read_to_string(&path).expect("bless wrote the file");
    let f_at = first.find("`f`").or_else(|| first.find("f().")).unwrap_or(usize::MAX);
    let g_at = first.find("`g`").or_else(|| first.find("g().")).unwrap_or(usize::MAX);
    assert!(f_at < g_at, "entries are sorted by symbol id:\n{first}");
    assert!(first.ends_with("}\n"), "canonical trailing newline");

    // The AC's bless-twice `cmp`: a fresh engine re-blessing the same tree
    // writes byte-identical content.
    fs::remove_file(&path).unwrap();
    let engine = Engine::start(tmp.path()).expect("engine restarts");
    engine.bless_rules(None, true).expect("re-bless runs");
    assert_eq!(fs::read_to_string(&path).unwrap(), first, "byte-identical re-bless");

    // check, gate (compare and save) and scan read it at most.
    let before = stamp(&path);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let checked = engine.check_rules(None, true).expect("check runs");
    engine.gate(None, true, true).expect("gate --save runs");
    engine.gate(None, false, true).expect("gate runs");
    let scanned = engine.scan(true).expect("scan runs");
    assert_eq!(stamp(&path), before, "check/gate/scan left the blessed set untouched");

    assert_eq!(checked.passed, Some(true), "{:?}", checked.violations);
    assert_eq!(
        offenders(&checked),
        [
            ("f".to_string(), "warning".to_string(), RatchetStatus::Blessed),
            ("g".to_string(), "warning".to_string(), RatchetStatus::Blessed),
        ]
    );
    let scanned_cc: Vec<(&str, RatchetStatus)> = scanned
        .violations
        .iter()
        .filter(|v| v.rule == "max_cc")
        .map(|v| (v.severity.as_str(), v.ratchet.as_ref().expect("marked").status))
        .collect();
    assert_eq!(
        scanned_cc,
        [("warning", RatchetStatus::Blessed), ("warning", RatchetStatus::Blessed)],
        "scan classifies both offenders against the same blessed set: {:?}",
        scanned.violations
    );
}

#[test]
fn worsened_and_new_offenders_fail_and_a_reduced_one_is_reported_fixed() {
    let tmp = project(&branchy("f", 3));
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.bless_rules(None, true).expect("bless F");

    // F gains a branch (worse than blessed) and G appears (new).
    write(
        tmp.path(),
        "src/lib.rs",
        &format!("{}{}", branchy("f", 4), branchy("g", 2)),
    );
    let report = engine.check_rules(None, true).expect("check runs");
    assert_eq!(report.passed, Some(false));
    assert_eq!(
        offenders(&report),
        [
            ("f".to_string(), "error".to_string(), RatchetStatus::Worsened),
            ("g".to_string(), "error".to_string(), RatchetStatus::New),
        ]
    );

    // F falls under max = 2 and G is gone: nothing violates, F is fixed.
    write(tmp.path(), "src/lib.rs", &branchy("f", 1));
    let report = engine.check_rules(None, true).expect("check runs");
    assert_eq!(report.passed, Some(true), "{:?}", report.violations);
    assert!(offenders(&report).is_empty());
    let fixed = &report.ratchet.as_ref().unwrap().fixed;
    assert_eq!(fixed.len(), 1, "{fixed:?}");
    assert_eq!(fixed[0].rule, "max_cc");
    assert_eq!(fixed[0].blessed, 4, "F was blessed at CC 4");
}

/// The blessed set is checked in, so it must hold in every checkout of the
/// tree: blessed under one project coordinate, it still classifies a checkout
/// under another (a fork, a renamed repository) as blessed debt.
#[test]
fn a_blessed_set_holds_in_a_checkout_under_another_coordinate() {
    let upstream = project(&branchy("f", 3));
    write(upstream.path(), ".logos/config.toml", "[project]\nid = \"github.com/acme/app\"\n");
    Engine::start(upstream.path())
        .expect("engine starts")
        .bless_rules(None, true)
        .expect("bless runs");

    let fork = project(&branchy("f", 3));
    write(fork.path(), ".logos/config.toml", "[project]\nid = \"github.com/someone/app\"\n");
    fs::copy(blessed_file(upstream.path()), blessed_file(fork.path())).unwrap();

    let report = Engine::start(fork.path())
        .expect("engine starts")
        .check_rules(None, true)
        .expect("check runs");
    assert_eq!(report.passed, Some(true), "{:?}", report.violations);
    assert_eq!(
        offenders(&report),
        [("f".to_string(), "warning".to_string(), RatchetStatus::Blessed)]
    );
    assert!(report.ratchet.as_ref().unwrap().fixed.is_empty());
}

/// `check --bless --rules FILE` re-blesses only the rules that contract
/// ratchets: the default contract's blessed debt in the same file survives,
/// so the default `check` still passes afterwards.
#[test]
fn blessing_under_an_alternate_contract_keeps_the_default_contracts_debt() {
    let tmp = project(&branchy("f", 3));
    let alt = tmp.path().join("alt-rules.toml");
    fs::write(&alt, "[constraints]\nmax_fn_lines = { max = 3, ratchet = true }\n").unwrap();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    engine.bless_rules(None, true).expect("bless the default contract");
    engine.bless_rules(Some(&alt), true).expect("bless the alternate contract");
    let text = fs::read_to_string(blessed_file(tmp.path())).unwrap();
    assert!(
        text.contains("\"max_cc\"") && text.contains("\"max_fn_lines\""),
        "both contracts' debt is blessed:\n{text}"
    );

    let report = engine.check_rules(None, true).expect("check runs");
    assert_eq!(report.passed, Some(true), "{:?}", report.violations);
}

/// A contract that never opted into ratchet never reads the blessed set, so a
/// malformed one cannot break its `check` or `scan`.
#[test]
fn a_contract_with_no_ratchet_budget_never_reads_the_blessed_set() {
    let tmp = project(&branchy("f", 3));
    write(tmp.path(), ".logos/rules.toml", "[constraints]\nmax_cc = 2\n");
    write(tmp.path(), ".logos/rules-baseline.json", "{ not json");
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let report = engine.check_rules(None, true).expect("check ignores the blessed set");
    assert_eq!(report.passed, Some(false), "the scalar budget still fails F");
    assert!(report.ratchet.is_none(), "no ratchet summary without a ratchet budget");
    engine.scan(true).expect("scan ignores the blessed set");

    // The near miss: the same malformed file under a ratchet contract is read,
    // and refused loud.
    write(tmp.path(), ".logos/rules.toml", RULES);
    let err = engine.check_rules(None, true).expect_err("a ratchet contract reads it");
    assert!(err.to_string().contains("rules-baseline.json"), "{err}");
}

/// Bless with no ratchet budget has no debt to record: it creates no file and
/// says why.
#[test]
fn bless_with_no_ratchet_budget_writes_nothing_and_says_so() {
    let tmp = project(&branchy("f", 3));
    write(tmp.path(), ".logos/rules.toml", "[constraints]\nmax_cc = 2\n");
    let report = Engine::start(tmp.path())
        .expect("engine starts")
        .bless_rules(None, true)
        .expect("bless runs");
    assert!(!blessed_file(tmp.path()).exists(), "no file the contract never reads");
    assert!(
        report.notes.iter().any(|n| n.contains("nothing to bless")),
        "{:?}",
        report.notes
    );
}
