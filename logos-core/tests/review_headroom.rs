//! `review --headroom` (S-798, [FR-GV-29], [UAT-GV-19]), end-to-end through
//! `Engine::review_headroom` over real git fixtures.
//!
//! Coverage by acceptance criterion:
//! - a module at `max_fan_out − 2` on the base reads headroom 2: `--near 2`
//!   lists it, `--near 1` does not, and an uncommitted edge changes nothing;
//! - `--subjects <file>` lists the file's module, the file and its functions
//!   under their budgets, a symbol name resolves to its function, and a path
//!   the base lacks and a name that matches nothing are `unresolved` with why;
//! - under a ratchet budget a blessed subject reads against its blessed value
//!   (read at the base commit, never the working tree), an unblessed one past
//!   `max` has no room and says why, one under `max` reads `limit`;
//! - an unfiltered answer lists a rule's 10 tightest subjects with the count
//!   it evaluated; `not_projected` is `--branches`'s; the drift budget has a
//!   margin or says why not;
//! - the answer is byte-stable, and a headroom run and a `--branches` run
//!   share the cached base measurement.
//!
//! The exit code and the CLI/MCP parity are surface contracts:
//! `cli/tests/review_cli.rs` and `mcp/tests/review_parity.rs`.
//!
//! Every fixture is a throw-away repository with `GIT_CEILING_DIRECTORIES` set
//! and git maintenance off, so nothing escapes into an enclosing repository.
//!
//! [FR-GV-29]: ../../docs/specs/requirements/FR-GV-29.md
//! [UAT-GV-19]: ../../docs/specs/requirements/UAT-GV-19.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::quality::{HeadroomReport, HeadroomRule, HeadroomSubject};
use logos_core::Engine;
use tempfile::TempDir;

/// Run `git -C <cwd> <args…>` hermetically, asserting success.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// `src/engine.rs` calling one function in each of `modules`.
fn engine_calling(modules: &[&str]) -> String {
    let calls: String = modules.iter().map(|m| format!("    crate::{m}::work_{m}();\n")).collect();
    format!("pub fn run() {{\n{calls}}}\n")
}

/// A function named `name` of cyclomatic complexity `cc` (one `if` each above 1).
fn complex(name: &str, cc: usize) -> String {
    let body: String = (1..cc).map(|i| format!("    if x == {i} {{ return {i}; }}\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    0\n}}\n")
}

/// `main`, where the module `engine` calls into `a` and `b` — a fan-out of 2 —
/// and `c` and `d` exist uncalled, under the `[constraints]` in `rules`. The
/// engine is not indexed: the headroom measures a committed tree only.
fn repo(rules: &str) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/*\n!.logos/rules-baseline.json\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    for m in ["a", "b", "c", "d"] {
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n"));
    }
    write(root, "src/engine.rs", &engine_calling(&["a", "b"]));
    write(root, ".logos/rules.toml", rules);
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    tmp
}

fn headroom(root: &Path, subjects: &[&str], near: Option<u64>) -> HeadroomReport {
    let engine = Engine::start(root).expect("engine starts");
    let subjects: Vec<String> = subjects.iter().map(|s| s.to_string()).collect();
    engine.review_headroom(Some("main"), &subjects, near).expect("the headroom runs")
}

fn rule<'a>(report: &'a HeadroomReport, name: &str) -> &'a HeadroomRule {
    report
        .rules
        .iter()
        .find(|r| r.rule == name)
        .unwrap_or_else(|| panic!("{name} is listed: {report:#?}"))
}

fn subject<'a>(rule: &'a HeadroomRule, file: &str) -> Option<&'a HeadroomSubject> {
    rule.subjects.iter().find(|s| s.file == file)
}

#[test]
fn a_module_two_below_its_limit_reads_headroom_2_and_near_filters_it() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\n");
    let root = tmp.path();

    let all = headroom(root, &[], None);
    assert!(all.measured, "{all:#?}");
    assert_eq!(all.base, "main");
    assert_eq!(all.base_origin, "stated");
    assert!(all.base_commit.as_deref().is_some_and(|c| c.len() == 40), "the answer names the commit: {all:#?}");
    let fan_out = rule(&all, "max_fan_out");
    assert_eq!((fan_out.limit, fan_out.evaluated, fan_out.listed), (4, 1, 1), "{fan_out:#?}");
    let engine = subject(fan_out, "src/engine.rs").expect("the engine module is listed");
    assert_eq!(engine.subject, "module:src/`engine.rs`/", "the module key, without a coordinate");
    assert_eq!((engine.kind, engine.allowance, engine.current, engine.headroom), ("limit", 4, 2, 2));
    assert_eq!(engine.trust, "resolved");
    assert_eq!(engine.reason, None);

    let two = headroom(root, &[], Some(2));
    assert_eq!(rule(&two, "max_fan_out").listed, 1, "--near 2 lists it: {two:#?}");
    let one = headroom(root, &[], Some(1));
    assert_eq!(rule(&one, "max_fan_out").listed, 0, "--near 1 does not: {one:#?}");
    assert_eq!(rule(&one, "max_fan_out").evaluated, 1, "the denominator is the rule's, not the filter's");
}

#[test]
fn an_uncommitted_edge_changes_nothing_and_a_rerun_is_byte_identical() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\n");
    let root = tmp.path();
    let first = serde_json::to_string(&headroom(root, &["src/engine.rs"], None)).unwrap();

    write(root, "src/engine.rs", &engine_calling(&["a", "b", "c"]));
    let edited = serde_json::to_string(&headroom(root, &["src/engine.rs"], None)).unwrap();
    assert_eq!(edited, first, "the working tree is not measured");
    let fan_out_row = headroom(root, &["src/engine.rs"], None);
    assert_eq!(subject(rule(&fan_out_row, "max_fan_out"), "src/engine.rs").unwrap().current, 2);
}

#[test]
fn a_file_resolves_to_its_module_itself_and_its_functions_and_a_name_to_its_function() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\nmax_fan_in = 9\nno_god_files = 50\nmax_cc = 15\nmax_fn_lines = 80\n");
    let root = tmp.path();
    write(root, "src/engine.rs", &format!("{}{}", engine_calling(&["a", "b"]), complex("tangled", 6)));
    commit(root, "a branchy function");

    let report = headroom(root, &["src/engine.rs"], None);
    assert!(report.measured && report.unresolved.is_empty(), "{report:#?}");
    for name in ["max_fan_out", "no_god_files", "max_cc", "max_fn_lines"] {
        let row = rule(&report, name);
        assert!(row.listed >= 1 && row.subjects.iter().all(|s| s.file == "src/engine.rs"), "{name}: {row:#?}");
    }
    let fan_in = rule(&report, "max_fan_in");
    assert_eq!(fan_in.listed, 0, "nothing calls the engine, so its module has no fan-in to list: {fan_in:#?}");
    let cc = rule(&report, "max_cc");
    assert_eq!(cc.listed, 2, "each of the file's functions: {cc:#?}");
    let tangled = cc.subjects.iter().find(|s| s.subject.contains("tangled")).expect("tangled is listed");
    assert_eq!((tangled.current, tangled.allowance, tangled.headroom), (6, 15, 9));
    assert_eq!(rule(&report, "no_god_files").subjects[0].subject, "src/engine.rs");

    let named = headroom(root, &["tangled"], None);
    assert!(named.unresolved.is_empty(), "{named:#?}");
    let cc = rule(&named, "max_cc");
    assert_eq!(cc.listed, 1, "the one function of that name: {cc:#?}");
    assert!(cc.subjects[0].subject.contains("tangled"));
    assert_eq!(rule(&named, "max_fan_out").listed, 1, "its module is listed too: {named:#?}");
    assert_eq!(rule(&named, "no_god_files").listed, 1, "and its file");
    assert_eq!(rule(&named, "max_fn_lines").subjects.len(), 1);
}

#[test]
fn an_entry_that_resolves_to_nothing_is_unresolved_with_why() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\nmax_cc = 15\n");
    let root = tmp.path();
    let report = headroom(root, &["src/missing.rs", "no_such_function", "src/engine.rs"], None);
    let why: Vec<(&str, &str)> = report.unresolved.iter().map(|u| (u.entry.as_str(), u.reason.as_str())).collect();
    assert_eq!(why.len(), 2, "{why:?}");
    assert_eq!(why[0], ("src/missing.rs", "not in the base commit"));
    assert_eq!(why[1].0, "no_such_function");
    assert!(why[1].1.contains("no function or module of that name"), "{why:?}");
    assert_eq!(rule(&report, "max_fan_out").listed, 1, "the resolved entry is still answered: {report:#?}");

    let near_miss = headroom(root, &["ru", "runn"], None);
    assert_eq!(near_miss.unresolved.len(), 2, "a name matches whole, not by substring: {near_miss:#?}");
    let exact = headroom(root, &["run"], None);
    assert!(exact.unresolved.is_empty(), "{exact:#?}");

    let in_base_without_a_row = headroom(root, &[".gitignore"], None);
    assert!(
        in_base_without_a_row.unresolved[0].reason.contains("no declared budget records a value"),
        "{in_base_without_a_row:#?}"
    );
}

/// `max_cc = { max = 3, ratchet = true }`: `blessed` (cc 5) was blessed at 6,
/// `unblessed` (cc 4) is past `max` and not in the set, `fine` (cc 2) is under
/// it.
fn ratchet_repo() -> TempDir {
    let tmp = repo("[constraints]\nmax_cc = { max = 3, ratchet = true }\n");
    let root = tmp.path();
    write(
        root,
        "src/hot.rs",
        &format!("{}{}{}", complex("blessed", 5), complex("unblessed", 4), complex("fine", 2)),
    );
    commit(root, "functions of three complexities");
    tmp
}

/// The symbol id under which `max_cc` names `function` on `main`.
fn cc_subject(root: &Path, function: &str) -> String {
    let report = headroom(root, &["src/hot.rs"], None);
    rule(&report, "max_cc")
        .subjects
        .iter()
        .find(|s| s.subject.ends_with(&format!("/{function}().")))
        .unwrap_or_else(|| panic!("{function} is listed: {report:#?}"))
        .subject
        .clone()
}

fn bless(root: &Path, symbol: &str, value: u64) {
    write(
        root,
        ".logos/rules-baseline.json",
        &format!("{{\"version\":1,\"entries\":[{{\"rule\":\"max_cc\",\"symbol\":{symbol:?},\"value\":{value}}}]}}\n"),
    );
}

#[test]
fn a_ratchet_subject_reads_against_its_blessed_value_read_at_the_base_commit() {
    let tmp = ratchet_repo();
    let root = tmp.path();

    let bare = headroom(root, &["src/hot.rs"], None);
    assert!(
        bare.notes.iter().all(|n| !n.contains("blessed above its max")),
        "no blessed set is on the base commit: {bare:#?}"
    );
    let symbol = cc_subject(root, "blessed");
    bless(root, &symbol, 6);
    git(root, &["add", "-f", ".logos/rules-baseline.json"]);
    git(root, &["commit", "-q", "-m", "bless one function"]);

    let report = headroom(root, &["src/hot.rs"], None);
    let cc = rule(&report, "max_cc");
    let by = |name: &str| {
        let tail = format!("/{name}().");
        cc.subjects.iter().find(|s| s.subject.ends_with(&tail)).unwrap_or_else(|| panic!("{name}: {cc:#?}"))
    };
    let blessed = by("blessed");
    assert_eq!((blessed.kind, blessed.allowance, blessed.current, blessed.headroom), ("ratchet", 6, 5, 1));
    assert_eq!(blessed.reason, None);
    let unblessed = by("unblessed");
    assert_eq!((unblessed.kind, unblessed.allowance, unblessed.current, unblessed.headroom), ("ratchet", 4, 4, 0));
    assert!(unblessed.reason.as_deref().is_some_and(|r| r.contains("not blessed")), "{unblessed:#?}");
    let fine = by("fine");
    assert_eq!((fine.kind, fine.allowance, fine.current, fine.headroom), ("limit", 3, 2, 1));
    assert!(report.notes.iter().any(|n| n.contains("blessed above its max")), "{report:#?}");

    // A bless made after the base commit gives no room: the set is read there.
    bless(root, &symbol, 99);
    let uncommitted = headroom(root, &["src/hot.rs"], None);
    assert_eq!(uncommitted, report, "the working tree's blessed set is not read");

    // Nor is the checked-out branch's: the set read is the base commit's.
    git(root, &["checkout", "-q", "-b", "rebless"]);
    git(root, &["add", "-f", ".logos/rules-baseline.json"]);
    git(root, &["commit", "-q", "-m", "bless it at 99 on another branch"]);
    let from_head = headroom(root, &["src/hot.rs"], None);
    assert_eq!(from_head, report, "HEAD's blessed set is not read either");
    git(root, &["checkout", "-q", "main"]);
}

#[test]
fn a_ratchet_symbol_set_is_one_row_with_no_room() {
    let tmp = repo("[constraints]\nmax_dead = { max = 0, ratchet = true }\nmax_cycles = 3\n");
    let root = tmp.path();
    write(root, "src/a.rs", "pub fn work_a() {}\n\nfn unused() {}\n");
    commit(root, "a dead function");
    let report = headroom(root, &[], None);
    let dead = rule(&report, "max_dead");
    assert_eq!((dead.evaluated, dead.listed), (1, 1), "{dead:#?}");
    let row = &dead.subjects[0];
    assert_eq!((row.subject.as_str(), row.kind, row.current, row.headroom), ("", "ratchet", 1, 0), "{row:#?}");
    let cycles = &rule(&report, "max_cycles").subjects[0];
    assert_eq!((cycles.kind, cycles.allowance, cycles.current, cycles.headroom), ("limit", 3, 0, 3), "{cycles:#?}");

    let scoped = headroom(root, &["src/a.rs"], None);
    assert_eq!(rule(&scoped, "max_dead").listed, 0, "a project-wide count contains no entry: {scoped:#?}");
}

#[test]
fn without_a_filter_a_rule_lists_its_ten_tightest_subjects_and_its_denominator() {
    let tmp = repo("[constraints]\nmax_cc = 20\n");
    let root = tmp.path();
    let functions: String = (2..=13).map(|cc| complex(&format!("f{cc}"), cc)).collect();
    write(root, "src/many.rs", &functions);
    commit(root, "twelve functions");

    let report = headroom(root, &[], None);
    let cc = rule(&report, "max_cc");
    assert_eq!((cc.evaluated, cc.listed), (17, 10), "twelve new functions, `run` and the four `work_*`: {cc:#?}");
    let room: Vec<i64> = cc.subjects.iter().map(|s| s.headroom).collect();
    let mut sorted = room.clone();
    sorted.sort();
    assert_eq!(room, sorted, "tightest first");
    assert_eq!(room[0], 20 - 13, "{room:?}");

    let near = headroom(root, &[], Some(10));
    let listed = rule(&near, "max_cc");
    assert_eq!(listed.listed, 4, "--near lifts the cap and keeps room ≤ 10 (cc 10..=13): {listed:#?}");
    let scoped = headroom(root, &["src/many.rs"], None);
    assert_eq!(rule(&scoped, "max_cc").listed, 12, "--subjects lifts the cap: {scoped:#?}");
    let both = headroom(root, &["src/many.rs"], Some(10));
    assert_eq!(rule(&both, "max_cc").listed, 4, "the two filters intersect");
}

#[test]
fn a_subject_past_its_limit_on_the_base_reads_negative_room_and_sorts_first() {
    let tmp = repo("[constraints]\nmax_cc = 3\n");
    let root = tmp.path();
    write(root, "src/hot.rs", &format!("{}{}", complex("over", 5), complex("under", 2)));
    commit(root, "one function past the limit");
    let cc = rule(&headroom(root, &[], None), "max_cc").clone();
    assert_eq!((cc.subjects[0].current, cc.subjects[0].headroom), (5, -2), "{cc:#?}");
}

#[test]
fn not_projected_is_what_branches_lists_and_the_drift_has_a_margin_or_a_reason() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\nmax_clone_ratio = 0.5\n[review]\ndrift_budget = 5\n");
    let root = tmp.path();
    let engine = Engine::start(root).expect("engine starts");
    let headroom = engine.review_headroom(Some("main"), &[], None).unwrap();
    git(root, &["checkout", "-q", "-b", "b1"]);
    write(root, "src/engine.rs", &engine_calling(&["a", "b", "c"]));
    commit(root, "one more edge");
    git(root, &["checkout", "-q", "main"]);
    let projection = engine.review_branches(Some("main"), &["b1".to_string()]).unwrap();
    assert_eq!(headroom.not_projected, projection.not_projected, "{headroom:#?}");
    let rules: Vec<&str> = headroom.not_projected.iter().map(|l| l.rule.as_str()).collect();
    assert_eq!(rules, ["max_clone_ratio"]);

    let drift = headroom.drift.as_ref().expect("drift is answered");
    assert!(drift.applicable, "{drift:#?}");
    assert_eq!((drift.budget, drift.drop, drift.headroom), (Some(5.0), Some(0), Some(5.0)), "{drift:#?}");
    let reference = drift.reference.as_ref().expect("the reference is named");
    assert_eq!(reference.kind, "base");
    assert!(reference.reason.as_deref().is_some_and(|r| r.contains("no gate baseline")), "{reference:#?}");

    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 4\n[review]\ndrift_budget = \"report\"\n");
    let report = Engine::start(root).unwrap().review_headroom(Some("main"), &[], None).unwrap();
    let drift = report.drift.expect("drift is answered");
    assert!(!drift.applicable && drift.headroom.is_none(), "{drift:#?}");
    assert!(drift.reason.as_deref().is_some_and(|r| r.contains("never fails")), "{drift:#?}");
    assert_eq!(report.not_projected[0].rule, "drift_budget");

    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 4\n");
    let report = Engine::start(root).unwrap().review_headroom(Some("main"), &[], None).unwrap();
    let drift = report.drift.expect("drift is answered");
    assert!(!drift.applicable, "{drift:#?}");
    assert!(drift.reason.as_deref().is_some_and(|r| r.contains("no [review] drift_budget")), "{drift:#?}");
    assert!(report.not_projected.is_empty());
}

#[test]
fn an_unresolvable_base_is_not_measured_and_no_budget_is_an_answer_not_a_failure() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\n");
    let root = tmp.path();
    let engine = Engine::start(root).expect("engine starts");
    let missing = engine.review_headroom(Some("no-such-ref"), &[], None).unwrap();
    assert!(!missing.measured && missing.rules.is_empty() && missing.drift.is_none(), "{missing:#?}");
    assert!(missing.not_measured_because.as_deref().is_some_and(|w| w.contains("does not resolve")), "{missing:#?}");
    assert_eq!(missing.base_commit, None);

    write(root, ".logos/rules.toml", "[constraints]\nmax_clone_ratio = 0.5\n");
    let none = Engine::start(root).unwrap().review_headroom(Some("main"), &[], None).unwrap();
    assert!(none.measured && none.rules.is_empty(), "{none:#?}");
    assert!(none.notes.iter().any(|n| n.contains("no limit to list")), "{none:#?}");
    assert_eq!(none.not_projected[0].rule, "max_clone_ratio");
    assert!(none.base_commit.is_some());
}

#[test]
fn headroom_and_branches_share_one_cached_base_measurement() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\n");
    let root = tmp.path();
    git(root, &["checkout", "-q", "-b", "b1"]);
    write(root, "src/engine.rs", &engine_calling(&["a", "b", "c"]));
    commit(root, "one more edge");
    git(root, &["checkout", "-q", "main"]);

    let cache = root.join(".logos/review-cache");
    let entries = || {
        let mut names: Vec<String> =
            fs::read_dir(&cache).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        names
    };
    let first = serde_json::to_string(&headroom(root, &[], None)).unwrap();
    let built = entries();
    assert_eq!(built.len(), 1, "the base commit, once: {built:?}");
    assert!(built[0].ends_with(".projection.json"), "{built:?}");
    let base_entry = cache.join(&built[0]);
    let mtime = fs::metadata(&base_entry).unwrap().modified().unwrap();

    let engine = Engine::start(root).unwrap();
    let projection = engine.review_branches(Some("main"), &["b1".to_string()]).unwrap();
    assert!(projection.measured, "{projection:#?}");
    assert_eq!(
        fs::metadata(&base_entry).unwrap().modified().unwrap(),
        mtime,
        "--branches read the base entry the headroom run left"
    );
    assert_eq!(entries().len(), 2, "plus the branch tip");
    assert_eq!(serde_json::to_string(&headroom(root, &[], None)).unwrap(), first, "a rerun is byte-identical");
}

#[test]
fn entries_are_normalised_deduplicated_and_every_one_that_selects_nothing_says_why() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\nmax_cc = 15\nno_god_files = 50\n");
    let root = tmp.path();
    let report = headroom(root, &["./src/engine.rs", "src/engine.rs", " src/engine.rs ", "", "src/", "src", ".gitignore"], None);
    let why: Vec<(&str, &str)> = report.unresolved.iter().map(|u| (u.entry.as_str(), u.reason.as_str())).collect();
    assert_eq!(why.len(), 4, "{why:?}");
    assert_eq!(why[0], ("", "an empty entry names nothing"));
    assert!(why[1].1.contains("a directory"), "src/ is in the base commit: {why:?}");
    assert!(why[2].1.contains("a directory"), "so is src: {why:?}");
    assert_eq!(why[3].0, ".gitignore");
    assert!(why[3].1.contains("no declared budget records a value"), "{why:?}");
    let fan_out = rule(&report, "max_fan_out");
    assert_eq!(fan_out.listed, 1, "the three spellings of one file are one subject: {fan_out:#?}");
    for name in ["max_cc", "no_god_files"] {
        assert!(rule(&report, name).subjects.iter().all(|s| s.file == "src/engine.rs"), "{name}");
    }
    assert_eq!(
        rule(&report, "no_god_files").listed,
        1,
        "a file that is not code is not given a row at 0 under no_god_files: {report:#?}"
    );

    let none = headroom(root, &["nope", "src/missing.rs"], None);
    assert!(none.rules.iter().all(|r| r.listed == 0), "nothing selected, nothing listed: {none:#?}");
    assert_eq!(none.unresolved.len(), 2);
}

#[test]
fn a_module_is_named_by_its_file_and_a_file_lists_its_coupling_and_nesting_rows() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\nmax_fan_in = 9\nmax_nesting_depth = 5\n");
    let root = tmp.path();
    write(root, "src/top.rs", "pub fn go() {\n    crate::engine::run();\n}\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\npub mod top;\n");
    write(
        root,
        "src/a.rs",
        "pub fn work_a(x: i32) -> i32 {\n    if x > 0 {\n        if x > 1 {\n            return x;\n        }\n    }\n    0\n}\n",
    );
    commit(root, "a caller of the engine and a nested function");

    let by_file = headroom(root, &["src/engine.rs"], None);
    let fan_in = rule(&by_file, "max_fan_in");
    assert_eq!(fan_in.listed, 1, "the engine module has one caller: {fan_in:#?}");
    assert_eq!((fan_in.subjects[0].current, fan_in.subjects[0].headroom), (1, 8));
    assert_eq!(rule(&by_file, "max_fan_out").listed, 1);

    let nested = headroom(root, &["src/a.rs"], None);
    let depth = rule(&nested, "max_nesting_depth");
    assert_eq!(depth.listed, 1, "{depth:#?}");
    assert_eq!((depth.subjects[0].current, depth.subjects[0].headroom), (2, 3));

    let by_function = headroom(root, &["work_a"], None);
    let depth = rule(&by_function, "max_nesting_depth");
    assert_eq!((depth.listed, depth.subjects[0].current), (1, 2), "a name selects the function's nesting row: {depth:#?}");

    let by_module = headroom(root, &["engine.rs"], None);
    assert!(by_module.unresolved.is_empty(), "{by_module:#?}");
    for name in ["max_fan_in", "max_fan_out"] {
        let row = rule(&by_module, name);
        assert_eq!((row.listed, row.subjects[0].file.as_str()), (1, "src/engine.rs"), "{name}: {row:#?}");
    }
    assert_eq!(rule(&by_module, "max_nesting_depth").listed, 0, "a module's name names no function");
}

#[test]
fn no_budget_declared_still_reports_every_entry_it_could_not_select() {
    let tmp = repo("[constraints]\nmax_clone_ratio = 0.5\n");
    let report = headroom(tmp.path(), &["src/engine.rs", "nope"], None);
    assert!(report.measured && report.rules.is_empty(), "{report:#?}");
    let entries: Vec<&str> = report.unresolved.iter().map(|u| u.entry.as_str()).collect();
    assert_eq!(entries, ["src/engine.rs", "nope"]);
    assert!(report.unresolved.iter().all(|u| u.reason == "no per-subject [constraints] budget is declared"));
}

#[test]
fn a_drift_budget_alone_still_gets_its_margin() {
    let tmp = repo("[review]\ndrift_budget = 5\n");
    let report = headroom(tmp.path(), &[], None);
    assert!(report.measured && report.rules.is_empty(), "{report:#?}");
    let drift = report.drift.expect("the drift is answered");
    assert!(drift.applicable, "{drift:#?}");
    assert_eq!((drift.budget, drift.headroom), (Some(5.0), Some(5.0)), "{drift:#?}");
}

#[test]
fn a_gate_baseline_saved_in_this_checkout_is_the_drifts_named_reference() {
    let tmp = repo("[constraints]\nmax_fan_out = 4\n[review]\ndrift_budget = 5\n");
    let root = tmp.path();
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine.gate(None, true, false).expect("a baseline is saved");
    let report = engine.review_headroom(Some("main"), &[], None).unwrap();
    let drift = report.drift.as_ref().expect("the drift is answered");
    let reference = drift.reference.as_ref().expect("the reference is named");
    assert_eq!(reference.kind, "baseline", "a comparable baseline is the reference: {reference:#?}");
    assert!(reference.snapshot_id.is_some() && reference.reason.is_none(), "{reference:#?}");
    let (drop, headroom) = (drift.drop.expect("drop"), drift.headroom.expect("headroom"));
    assert_eq!(headroom, 5.0 - drop as f64, "the margin is the budget less the drop: {drift:#?}");
    assert!(report.notes.iter().any(|n| n.contains("blessed gate baseline")), "{report:#?}");
}

#[test]
fn a_blessed_set_the_base_commit_cannot_use_is_not_measured_and_an_empty_one_says_so() {
    let tmp = ratchet_repo();
    let root = tmp.path();
    let engine = |root: &Path| Engine::start(root).expect("engine starts");
    for (text, why) in [
        ("{ not json", "unusable"),
        ("{\"version\":2,\"entries\":[]}\n", "format version 2"),
        (
            "{\"version\":1,\"entries\":[{\"rule\":\"max_cc\",\"symbol\":\"f\",\"value\":1},{\"rule\":\"max_cc\",\"symbol\":\"f\",\"value\":2}]}\n",
            "blessed twice",
        ),
    ] {
        write(root, ".logos/rules-baseline.json", text);
        commit(root, "a blessed set that cannot be used");
        let report = engine(root).review_headroom(Some("main"), &[], None).unwrap();
        assert!(!report.measured && report.rules.is_empty(), "{text}: {report:#?}");
        let reason = report.not_measured_because.as_deref().unwrap_or_default();
        assert!(reason.contains("is unusable") && reason.contains(why), "{text}: {reason}");
        assert!(report.base_commit.is_some(), "the commit is still named");
    }

    write(root, ".logos/rules-baseline.json", "{\"version\":1,\"entries\":[]}\n");
    commit(root, "an empty blessed set");
    let empty = engine(root).review_headroom(Some("main"), &[], None).unwrap();
    assert!(empty.measured, "{empty:#?}");
    assert!(empty.notes.iter().any(|n| n.contains("blesses nothing")), "{empty:#?}");
    let plain = repo("[constraints]\nmax_cc = 15\n");
    let report = headroom(plain.path(), &[], None);
    assert!(report.notes.iter().all(|n| !n.contains("blessed") && !n.contains("blesses")), "no ratchet, no blessed note: {report:#?}");
}

/// A module under `max_fan_out = { max = 1, ratchet = true }` blessed at 2 — by
/// a key that carries the repository coordinate, as `check --bless` writes it —
/// reads against its blessed value.
#[test]
fn a_ratchet_module_matches_a_blessed_key_that_carries_a_coordinate() {
    let tmp = repo("[constraints]\nmax_fan_out = { max = 1, ratchet = true }\n");
    let root = tmp.path();
    let bare = headroom(root, &["src/engine.rs"], None);
    let row = &rule(&bare, "max_fan_out").subjects[0];
    assert_eq!((row.kind, row.current, row.headroom), ("ratchet", 2, 0), "past max and unblessed: {row:#?}");
    write(
        root,
        ".logos/rules-baseline.json",
        "{\"version\":1,\"entries\":[{\"rule\":\"max_fan_out\",\"symbol\":\"module:logos . acme/project . src/`engine.rs`/\",\"value\":3}]}\n",
    );
    commit(root, "bless the engine module at 3");
    let blessed = headroom(root, &["src/engine.rs"], None);
    let row = &rule(&blessed, "max_fan_out").subjects[0];
    assert_eq!((row.kind, row.allowance, row.headroom), ("ratchet", 3, 1), "{row:#?}");
}
