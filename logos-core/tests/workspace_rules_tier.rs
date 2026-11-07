//! The workspace root as a **second rules tier** (S-506, [FR-CF-08], [ADR-73]),
//! driven end to end through the [`Engine`] façade over real temp-directory
//! workspaces: a `logos.workspace.toml` at the root, a root `.logos/rules.toml`
//! written at runtime, and members indexed for real.
//!
//! Coverage by acceptance criterion:
//! - a root `max_cc = 15` is enforced in a member with no `rules.toml`, which
//!   exits `0`/`1` and never `4`, and the finding reports origin `workspace`; a
//!   member declaring `max_cc = 25` is evaluated at 25 with origin `member` and
//!   still inherits the root's other keys;
//! - every member-only root table is refused with a stated reason and applied to
//!   no member;
//! - an inherited `[metric_thresholds]` key enters each member's thresholds hash,
//!   so a root edit re-baselines each member's next gate with the notice — and
//!   the index clusters near clones under the same inherited similarity;
//! - the [FR-GV-21] marker's evaluated set includes inherited rules, and `check`
//!   exits `4` only when neither tier declares a rule;
//! - a single-root project — and a workspace member whose root declares no rules
//!   file — carries no `workspace` origin in any read-model ([ADR-52]).
//!
//! Every `rules.toml` is written by the test at runtime: a fixture checked in
//! under a `.logos/` directory would be a second contract the repository's own
//! tooling could pick up.
//!
//! [FR-CF-08]: ../../docs/specs/requirements/FR-CF-08.md
//! [FR-GV-21]: ../../docs/specs/requirements/FR-GV-21.md
//! [ADR-52]: ../../docs/specs/architecture/decisions/ADR-52.md
//! [ADR-73]: ../../docs/specs/architecture/decisions/ADR-73.md

#![cfg(feature = "lang-rust")]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A function whose cyclomatic complexity is `conditions + 1` and whose body
/// spans `conditions + 3` lines: one `if` per line.
fn branchy(name: &str, conditions: u32) -> String {
    let mut body = format!("pub fn {name}(x: i32) -> i32 {{\n    let mut n = 0;\n");
    for i in 1..=conditions {
        writeln!(body, "    if x == {i} {{ n += {i}; }}").unwrap();
    }
    body.push_str("    n\n}\n");
    body
}

/// `ws/` holding the manifest and two members, `svc-a` and `svc-b`, each with
/// one function of cyclomatic complexity 20 spanning 22 lines. Neither member
/// has a `rules.toml` and the root has none either: each test writes the tiers
/// it is about.
struct Workspace {
    tmp: TempDir,
}

impl Workspace {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("ws");
        write(
            &ws,
            "logos.workspace.toml",
            "[workspace]\nname = \"w\"\nmembers = [\"svc-a\", \"svc-b\"]\n",
        );
        for member in ["svc-a", "svc-b"] {
            write(&ws.join(member), "src/lib.rs", &branchy("route", 19));
        }
        Workspace { tmp }
    }

    fn root(&self) -> PathBuf {
        self.tmp.path().join("ws")
    }

    fn member(&self, name: &str) -> PathBuf {
        self.root().join(name)
    }

    /// Write (or replace) the workspace tier's `rules.toml`.
    fn root_rules(&self, toml: &str) {
        write(&self.root(), ".logos/rules.toml", toml);
    }

    /// Write (or replace) a member's own `rules.toml`.
    fn member_rules(&self, name: &str, toml: &str) {
        write(&self.member(name), ".logos/rules.toml", toml);
    }

    fn engine(&self, name: &str) -> Engine {
        let engine = Engine::start(self.member(name)).expect("engine starts");
        let index = engine.pipeline().index();
        assert!(index.warnings.is_empty(), "{:?}", index.warnings);
        engine
    }
}

/// `check --json`, as a value.
fn check(engine: &Engine) -> (Value, i32) {
    let report = engine.quality().check_rules(None, true).expect("check runs");
    let code = report.exit_code(false);
    (serde_json::to_value(&report).expect("the report serialises"), code)
}

/// The violations of `report` for `rule`.
fn violations<'a>(report: &'a Value, rule: &str) -> Vec<&'a Value> {
    report["violations"]
        .as_array()
        .expect("violations is an array")
        .iter()
        .filter(|v| v["rule"] == rule)
        .collect()
}

/// [FR-CF-08] AC 1 — the inherited key is a real policy: enforced, a loaded
/// contract (exit 1, not 4), and attributed to the workspace.
#[test]
fn a_root_constraint_is_enforced_in_a_member_with_no_rules_file() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\n");
    let engine = ws.engine("svc-a");

    let (report, code) = check(&engine);
    assert_eq!(code, 1, "an inherited breach fails the member's check: {report:#}");
    assert_eq!(report["rules_present"], true, "{report:#}");
    assert_eq!(report["passed"], false, "{report:#}");
    let cc = violations(&report, "max_cc");
    assert_eq!(cc.len(), 1, "{report:#}");
    assert_eq!(cc[0]["origin"], "workspace", "the finding names its tier: {report:#}");
    assert_eq!(report["origins"]["constraints"]["max_cc"], "workspace", "{report:#}");
}

/// [FR-CF-08] AC 1 — the member key wins, per key: `max_cc = 25` is the member's
/// and holds, while `max_fn_lines` is still the root's and breaks.
#[test]
fn a_member_key_wins_and_the_other_root_keys_still_inherit() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\nmax_fn_lines = 10\n");
    ws.member_rules("svc-b", "[constraints]\nmax_cc = 25\n");
    let engine = ws.engine("svc-b");

    let (report, code) = check(&engine);
    assert_eq!(code, 1, "{report:#}");
    assert!(
        violations(&report, "max_cc").is_empty(),
        "CC 20 holds under the member's 25, not the root's 15: {report:#}"
    );
    let lines = violations(&report, "max_fn_lines");
    assert_eq!(lines.len(), 1, "{report:#}");
    assert_eq!(lines[0]["origin"], "workspace", "{report:#}");
    assert_eq!(report["origins"]["constraints"]["max_cc"], "member", "{report:#}");
    assert_eq!(report["origins"]["constraints"]["max_fn_lines"], "workspace", "{report:#}");

    // The member's own breach is attributed to the member.
    ws.member_rules("svc-b", "[constraints]\nmax_cc = 19\n");
    let (report, _) = check(&engine);
    let cc = violations(&report, "max_cc");
    assert_eq!(cc.len(), 1, "{report:#}");
    assert_eq!(cc[0]["origin"], "member", "{report:#}");
}

/// [FR-CF-08] AC 2 — every member-only table is refused at the root, with the
/// table and the reason stated, and applied to no member. A refused table is
/// not a declared rule, so a member inheriting nothing else still exits `4`.
#[test]
fn every_member_only_root_table_is_refused_and_applied_to_no_member() {
    let ws = Workspace::new();
    let engine = ws.engine("svc-a");
    let tables = [
        // Member-local gate policy that postdates the September requirement
        // (S-506 drift): refused on the same footing.
        ("[security]", "[security]\nmax_severity = \"critical\"\n"),
        ("[review]", "[review]\ndrift_budget = 2\n"),
        (
            "[[layers]]",
            "[[layers]]\nname = \"all\"\npaths = [\"src/**\"]\norder = 1\n",
        ),
        (
            "[[boundaries]]",
            "[[layers]]\nname = \"a\"\npaths = [\"src/**\"]\norder = 1\n\
             [[layers]]\nname = \"b\"\npaths = [\"lib/**\"]\norder = 2\n\
             [[boundaries]]\nfrom = \"a\"\nto = \"b\"\n",
        ),
        (
            "[[forbidden_imports]]",
            "[[forbidden_imports]]\nfrom = \"src/**\"\nto = \"src/**\"\n",
        ),
        ("[[require_tested]]", "[[require_tested]]\npaths = [\"src/**\"]\n"),
        ("[[require_documented]]", "[[require_documented]]\npaths = [\"src/**\"]\n"),
        ("[history]", "[history]\nwindow_months = 6\n"),
        ("[coverage]", "[coverage]\npath_strip_prefixes = [\"build/\"]\n"),
    ];
    for (table, toml) in tables {
        ws.root_rules(toml);
        let (report, code) = check(&engine);
        let warnings = report["warnings"].as_array().expect("warnings is an array");
        let refusal = warnings
            .iter()
            .filter_map(Value::as_str)
            .find(|w| w.contains(table))
            .unwrap_or_else(|| panic!("{table} is refused with a stated reason: {report:#}"));
        assert!(
            refusal.contains("refused") && refusal.contains("applied to no member"),
            "{table}: the refusal states what happened: {refusal}"
        );
        assert!(
            refusal.contains(&root_rules_path(&ws).display().to_string()),
            "{table}: the refusal names the root's file: {refusal}"
        );
        assert!(
            report["violations"].as_array().unwrap().is_empty(),
            "{table} is applied to no member: {report:#}"
        );
        assert_eq!(report["rules_present"], false, "{table} is not a declared rule: {report:#}");
        assert_eq!(code, 4, "{table}: neither tier declares a rule: {report:#}");
    }

    // A refused table beside an inheritable key: the key still inherits.
    ws.root_rules("[constraints]\nmax_cc = 15\n[[require_tested]]\npaths = [\"src/**\"]\n");
    let (report, code) = check(&engine);
    assert_eq!(code, 1, "{report:#}");
    assert_eq!(violations(&report, "max_cc").len(), 1, "{report:#}");
    assert!(violations(&report, "require_tested:src/**").is_empty(), "{report:#}");
}

/// [FR-CF-08] AC 3 — an inherited threshold is in each member's thresholds hash:
/// editing it at the root makes each member's next gate pass informationally
/// with the re-baseline notice, and the gate after that compares.
#[test]
fn editing_an_inherited_threshold_at_the_root_re_baselines_each_member() {
    let ws = Workspace::new();
    ws.root_rules("[metric_thresholds]\nnesting_depth = 4\n");
    let engines = [ws.engine("svc-a"), ws.engine("svc-b")];
    for engine in &engines {
        let saved = engine.quality().gate(None, true, true).expect("gate --save runs");
        assert!(saved.saved && saved.passed, "{}", saved.message);
    }

    ws.root_rules("[metric_thresholds]\nnesting_depth = 5\n");
    for engine in &engines {
        let first = engine.quality().gate(None, false, true).expect("gate runs");
        assert!(first.passed && first.saved, "{}", first.message);
        assert!(
            first.message.contains("baseline reset: metric thresholds changed"),
            "the root edit is announced in each member: {}",
            first.message
        );
        assert_eq!(first.exit_code(), 0);
        let second = engine.quality().gate(None, false, true).expect("gate runs");
        assert!(!second.message.contains("baseline reset"), "{}", second.message);
    }
}

/// [FR-CF-08] AC 4 / [FR-GV-21] — the recorded marker describes the effective
/// evaluated set: a member with no `rules.toml` and an inherited rule records a
/// present contract, with the inherited rules counted and attributed.
#[test]
fn the_marker_records_the_inherited_rules_with_their_origin() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\nmax_fn_lines = 10\n");
    ws.member_rules("svc-b", "[constraints]\nmax_cc = 25\n");
    for (member, inherited) in [("svc-a", 2), ("svc-b", 1)] {
        let engine = ws.engine(member);
        let (report, _) = check(&engine);
        let marker = engine
            .runtime()
            .expect("a started engine has a runtime")
            .submit_read(|store| store.check_run())
            .expect("the marker reads")
            .expect("check recorded a marker");
        assert_eq!(marker.rules_present, Some(true), "{member}: {marker:?}");
        assert_eq!(
            marker.checked_rules,
            report["checked_rules"].as_i64(),
            "{member}: the marker's denominator is the effective set"
        );
        assert_eq!(marker.workspace_rules, Some(inherited), "{member}: {marker:?}");
    }
}

/// [ADR-52] — no second tier, no `workspace` origin: a single-root project, and
/// a workspace member whose root declares no `rules.toml`, render exactly the
/// pre-S-506 shape (no `origins`, no violation `origin`) and record no
/// inherited count in the marker.
#[test]
fn without_a_root_rules_file_no_workspace_origin_appears_anywhere() {
    let solo = TempDir::new().unwrap();
    write(solo.path(), "src/lib.rs", &branchy("route", 19));
    write(solo.path(), ".logos/rules.toml", "[constraints]\nmax_cc = 15\n");
    let ws = Workspace::new();
    ws.member_rules("svc-a", "[constraints]\nmax_cc = 15\n");

    for root in [solo.path().to_path_buf(), ws.member("svc-a")] {
        let engine = Engine::start(&root).expect("engine starts");
        assert!(engine.pipeline().index().warnings.is_empty());
        let (report, code) = check(&engine);
        assert_eq!(code, 1, "{report:#}");
        let text = report.to_string();
        assert!(report.get("origins").is_none(), "{report:#}");
        assert!(!text.contains("\"origin\""), "{report:#}");
        assert!(!text.contains("workspace"), "{report:#}");
        let marker = engine
            .runtime()
            .unwrap()
            .submit_read(|store| store.check_run())
            .unwrap()
            .unwrap();
        assert_eq!(marker.workspace_rules, None, "{}: {marker:?}", root.display());
    }
}

/// Two functions that are near clones under the default similarity (0.85) and
/// not under 1.0 — the `branch_review.rs` builder pair.
const NEAR_CLONES: &str = "\
pub fn build_totals(values: &[u32], limit: u32) -> u32 {
    let mut total = 0;
    let mut count = 0;
    for value in values {
        if *value > limit {
            let scaled = value * 2;
            total += scaled;
            count += 1;
        } else {
            total += value;
        }
    }
    if count > 0 {
        total = total / count;
    }
    total
}

pub fn build_sums(items: &[u32], threshold: u32) -> u32 {
    let mut sum = 0;
    let mut seen = 0;
    for item in items {
        if *item > threshold {
            let scaled = item * 2;
            sum += scaled;
            sum += 1;
            seen += 1;
        } else {
            sum += item;
        }
    }
    if seen > 0 {
        sum = sum / seen;
    }
    sum
}
";

/// How many nodes the last index put in a clone group, read from the store.
fn clone_grouped(root: &Path) -> i64 {
    let db = rusqlite::Connection::open(root.join(".logos/logos.db")).expect("the store opens");
    db.query_row("SELECT count(*) FROM nodes WHERE clone_group IS NOT NULL", [], |r| r.get(0))
        .expect("clone_group reads")
}

/// [FR-CF-08] with [CR-013]'s one-threshold-set rule: the index clusters near
/// clones under the **effective** similarity, so an inherited
/// `clone_similarity` reaches the clone groups the snapshot scores, not only
/// the hash it records. A single-root control with the defaults proves the
/// pair does cluster.
///
/// [CR-013]: ../../docs/requests/CR-013-tunable-near-clone-thresholds.md
#[test]
fn the_index_clusters_clones_under_an_inherited_similarity() {
    let solo = TempDir::new().unwrap();
    write(solo.path(), "src/lib.rs", NEAR_CLONES);
    let control = Engine::start(solo.path()).expect("engine starts");
    assert!(control.pipeline().index().warnings.is_empty());
    drop(control);
    assert!(clone_grouped(solo.path()) >= 2, "the pair is a clone group under the defaults");

    let ws = Workspace::new();
    write(&ws.member("svc-a"), "src/lib.rs", NEAR_CLONES);
    ws.root_rules("[metric_thresholds]\nclone_similarity = 1.0\n");
    let engine = ws.engine("svc-a");
    drop(engine);
    assert_eq!(clone_grouped(&ws.member("svc-a")), 0, "the root's similarity is the one the index used");
}

/// The workspace tier's file as the engine names it: the canonical root's
/// `.logos/rules.toml`.
fn root_rules_path(ws: &Workspace) -> PathBuf {
    ws.root().canonicalize().unwrap().join(".logos").join("rules.toml")
}

/// The recorded check-run marker of `engine`.
fn marker(engine: &Engine) -> logos_core::graph_store::CheckRunRow {
    engine
        .runtime()
        .expect("a started engine has a runtime")
        .submit_read(|store| store.check_run())
        .expect("the marker reads")
        .expect("a run recorded a marker")
}

/// [FR-CF-08] on `scan`, the other run that replaces the violation set: the
/// refusal is stated, the inherited finding is attributed, and the marker it
/// writes carries the inherited count — and a root holding only a refused
/// table records no contract.
#[test]
fn scan_states_refusals_and_records_the_inherited_count() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\n[history]\nwindow_months = 6\n");
    let engine = ws.engine("svc-a");

    let scan = engine.quality().scan(true).expect("scan runs");
    assert!(
        scan.warnings.iter().any(|w| w.contains("[history] is member-only")),
        "{:?}",
        scan.warnings
    );
    let cc: Vec<_> = scan.violations.iter().filter(|v| v.rule == "max_cc").collect();
    assert_eq!(cc.len(), 1, "{:?}", scan.violations);
    assert_eq!(cc[0].origin, Some(logos_core::config::RuleOrigin::Workspace));
    let recorded = marker(&engine);
    assert_eq!(recorded.operation.as_deref(), Some("scan"));
    assert_eq!(recorded.rules_present, Some(true), "{recorded:?}");
    assert_eq!(recorded.workspace_rules, Some(1), "{recorded:?}");

    ws.root_rules("[history]\nwindow_months = 6\n");
    engine.quality().scan(true).expect("scan runs");
    let recorded = marker(&engine);
    assert_eq!(recorded.rules_present, Some(false), "a refused table is no rule: {recorded:?}");
    assert_eq!(recorded.workspace_rules, Some(0), "{recorded:?}");
}

/// An explicit `check --rules FILE` names the whole contract to check
/// against: the workspace tier is not applied to it, and no origin appears.
#[test]
fn an_explicit_rules_file_is_evaluated_without_the_tier() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\n");
    let engine = ws.engine("svc-a");
    let elsewhere = TempDir::new().unwrap();
    let explicit = elsewhere.path().join("contract.toml");
    fs::write(&explicit, "[constraints]\nmax_fn_lines = 100\n").unwrap();

    let report = engine.quality().check_rules(Some(&explicit), true).expect("check runs");
    assert_eq!(report.exit_code(false), 0, "{report:?}");
    assert!(report.violations.iter().all(|v| v.rule != "max_cc"), "{:?}", report.violations);
    assert!(report.origins.is_none(), "{:?}", report.origins);
}

/// [ADR-73] §5 with §2: a ratchet budget inherits like any key, its findings
/// keep their origin through classification, and the blessed set is the
/// member's own — `bless` writes it under the member, never at the root.
///
/// [ADR-73]: ../../docs/specs/architecture/decisions/ADR-73.md
#[test]
fn an_inherited_ratchet_is_blessed_into_the_members_own_set() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = { max = 15, ratchet = true }\n");
    let engine = ws.engine("svc-a");

    let (report, code) = check(&engine);
    assert_eq!(code, 1, "no blessed set yet, so the offender is an error: {report:#}");
    let cc = violations(&report, "max_cc");
    assert_eq!(cc.len(), 1, "{report:#}");
    assert_eq!(cc[0]["origin"], "workspace", "{report:#}");
    assert!(cc[0].get("ratchet").is_some(), "{report:#}");

    engine.quality().bless_rules(None, true).expect("bless runs");
    assert!(ws.member("svc-a").join(".logos/rules-baseline.json").is_file());
    assert!(!ws.root().join(".logos/rules-baseline.json").exists(), "the root holds no blessed set");

    let (report, code) = check(&engine);
    assert_eq!(code, 0, "blessed debt is a warning: {report:#}");
    assert_eq!(violations(&report, "max_cc")[0]["origin"], "workspace", "{report:#}");
}

/// A broken workspace tier fails `check` loud (a `ConfigError`, exit 2) and
/// names the root's file — never a silent fall back to the member alone.
#[test]
fn a_broken_root_contract_fails_check_naming_the_root_file() {
    let ws = Workspace::new();
    ws.root_rules("[constraints]\nmax_cc = 15\n");
    let engine = ws.engine("svc-a");
    ws.root_rules("[constraints]\nmax_cc = 15\nunknown_key = 1\n");

    let err = engine.quality().check_rules(None, false).expect_err("a broken tier is refused");
    let config = err
        .downcast_ref::<logos_core::config::ConfigError>()
        .unwrap_or_else(|| panic!("a ConfigError: {err:#}"));
    assert!(
        matches!(config, logos_core::config::ConfigError::Parse { path, .. } if *path == root_rules_path(&ws)),
        "{config}"
    );
}

/// The persisted parse cache holds the member's own parse, never the effective
/// one: a fresh engine after a root edit evaluates the edited root key.
#[test]
fn a_root_edit_reaches_a_fresh_engine_through_the_persisted_cache() {
    let ws = Workspace::new();
    ws.member_rules("svc-b", "[constraints]\nmax_cc = 25\n");
    ws.root_rules("[constraints]\nmax_fn_lines = 10\n");
    let (report, _) = check(&ws.engine("svc-b"));
    assert_eq!(violations(&report, "max_fn_lines").len(), 1, "{report:#}");

    ws.root_rules("[constraints]\nmax_fn_lines = 50\n");
    let fresh = Engine::start(ws.member("svc-b")).expect("engine starts");
    let (report, code) = check(&fresh);
    assert_eq!(code, 0, "{report:#}");
    assert!(violations(&report, "max_fn_lines").is_empty(), "the edited root key holds: {report:#}");
    assert_eq!(report["origins"]["constraints"]["max_fn_lines"], "workspace", "{report:#}");
    assert_eq!(report["origins"]["constraints"]["max_cc"], "member", "{report:#}");
}
