//! The `rules.toml [review]` policy (S-754, [FR-GV-25], [UAT-GV-15], [ADR-89]),
//! end-to-end through `Engine::gate`, `Engine::session_end` and
//! `Engine::review` over real git fixtures.
//!
//! Coverage by acceptance criterion:
//! - with `[review]` at defaults a branch adding a 6-deep function fails the
//!   gate, and `session_end`, naming it with value 6, threshold 4 and trust
//!   `resolved`; the review marks it gating;
//! - an Equality-only drift — a new above-mean function, the shape every
//!   feature produces — passes and is reported under `"report"`, fails at
//!   `drift_budget = 2`, and passes at a budget it does not exceed; the same
//!   drift fails the plain gate, which is what the policy exists to separate;
//! - the verdict over uncommitted work at the merge-base holds on a rerun,
//!   and once that work is committed, although every gate records a snapshot
//!   at `HEAD` over the working tree;
//! - without the table neither `gate` nor `review` carries any of the
//!   policy's fields; a delta that cannot be measured warns and never fails.
//!
//! The attribution and the re-bless record (S-755, [FR-GV-26], [FR-GV-16],
//! [UAT-GV-16]):
//! - `review` and, under the table, `gate`, `session_end` and the read-only
//!   verdict attribute the change per dimension: the contributions sum to the
//!   exact change with a residual under 0.5, the integer rounding stated, and
//!   each names its drivers — a new function, a near-clone group;
//! - `gate --save` past a numeric `drift_budget` without a reason is a usage
//!   error that moves no baseline; with one, the reason and the attribution
//!   are stored with the baseline, and a later anchor clears them;
//! - without the table a save stores the attribution and reports none, and
//!   needs no reason.
//!
//! The trend warning (S-756, [FR-GV-27], [UAT-GV-17]):
//! - two consecutive Equality declines warn on `scan`, `gate`, `session_end`
//!   and `review`, naming both values; one does not, and a gate re-measuring
//!   the same tree keeps the warning;
//! - `trend_window = 3` needs three; the warning moves no verdict or exit
//!   code, and without the table nothing warns.
//!
//! An invalid table's exit 2 is a CLI contract: `cli/tests/review_policy_cli.rs`,
//! which also pins the re-bless exit 2 through the binary.
//!
//! Every fixture is a throw-away repository with `GIT_CEILING_DIRECTORIES` set
//! and git maintenance off, so nothing escapes into an enclosing repository.
//!
//! [FR-GV-25]: ../../docs/specs/requirements/FR-GV-25.md
//! [UAT-GV-15]: ../../docs/specs/requirements/UAT-GV-15.md
//! [ADR-89]: ../../docs/specs/architecture/decisions/ADR-89.md
//! [FR-GV-26]: ../../docs/specs/requirements/FR-GV-26.md
//! [FR-GV-16]: ../../docs/specs/requirements/FR-GV-16.md
//! [UAT-GV-16]: ../../docs/specs/requirements/UAT-GV-16.md
//! [FR-GV-27]: ../../docs/specs/requirements/FR-GV-27.md
//! [UAT-GV-17]: ../../docs/specs/requirements/UAT-GV-17.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::config::ConfigError;
use logos_core::models::quality::{BranchReview, GateResult, ReviewPolicy, ReviewStatus, SignalAttribution};
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

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("    if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}    0\n}}\n", "    }\n".repeat(depth))
}

/// `name` with `n` sequential `if`s: cyclomatic complexity `n + 1`, nested one deep.
fn branches(name: &str, n: usize) -> String {
    let body: String = (0..n).map(|i| format!("    if x == {i} {{\n        return {i};\n    }}\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    x\n}}\n")
}

/// The 200 functions of `src/many.rs` on `main`: complexity 3 and 4,
/// alternating, so a new function of complexity 6 sits above the mean.
fn many() -> String {
    (0..200).map(|i| branches(&format!("f{i}"), 2 + i % 2)).collect()
}

/// A Rust repository whose `main` holds a 4-deep function and the 200
/// functions of [`many`], with the gate's baseline saved at `main`.
fn repo() -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod deep;\npub mod many;\n");
    write(root, "src/deep.rs", &nested("alpha_depth_four", 4));
    write(root, "src/many.rs", &many());
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let saved = engine.gate(None, true, true).expect("gate --save");
    assert!(saved.saved && saved.signal.is_some(), "{saved:?}");
    (tmp, engine)
}

/// Write `.logos/rules.toml`.
fn rules(root: &Path, text: &str) {
    write(root, ".logos/rules.toml", text);
}

fn gate(engine: &Engine) -> GateResult {
    engine.gate(None, false, true).expect("gate runs")
}

/// The branch `deep` off `main`, adding a 6-deep function and committed.
fn add_deep_function(root: &Path, engine: &Engine) {
    git(root, &["checkout", "-q", "-b", "deep"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    engine.index();
}

/// The branch `feature` off `main`, adding one above-mean function and committed.
fn add_above_mean_function(root: &Path, engine: &Engine) {
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/many.rs", &format!("{}{}", many(), branches("g_above_mean", 5)));
    commit(root, "add an above-mean function");
    engine.index();
}

/// Each serialised key of `value`, in order.
fn keys(value: &serde_json::Value) -> Vec<String> {
    value.as_object().expect("an object").keys().cloned().collect()
}

/// [UAT-GV-15] step 1: the 6-deep function fails `gate` and `session_end`,
/// named with value 6, threshold 4 and trust `resolved`; the review answer
/// carries the policy and marks the entry gating.
#[test]
fn a_six_deep_function_fails_the_gate_named_with_its_value_threshold_and_trust() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_deep_function(root, &engine);

    for result in [gate(&engine), engine.session_end().expect("session_end runs")] {
        assert!(!result.passed, "{result:#?}");
        assert_eq!(result.exit_code(), 1);
        let names: Vec<(&str, &str, ReviewStatus)> = result
            .gating_entries
            .iter()
            .map(|e| (e.dimension.as_str(), e.name.as_str(), e.status))
            .collect();
        assert_eq!(names, [("nesting", "beta_depth_six", ReviewStatus::New)], "{result:#?}");
        let beta = &result.gating_entries[0];
        assert_eq!(
            (beta.after, beta.threshold, beta.trust.as_str()),
            (Some(6), Some(4), BranchReview::TRUST_RESOLVED)
        );
        assert!(
            result.message.contains("1 gating branch-delta entry against the merge-base")
                && result.message.contains("nesting `beta_depth_six` (src/deep.rs:")
                && result.message.contains("nesting_depth 6, threshold 4 [new, trust resolved]"),
            "{}",
            result.message
        );
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["gating_entries"][0]["gating"], true, "{json}");
    }

    let review = engine.review(None).expect("review runs");
    assert_eq!(
        review.policy,
        Some(ReviewPolicy {
            gate_dimensions: ["nesting", "conciseness", "cohesion", "focus", "uniqueness"].map(String::from).to_vec(),
            gating: 1,
        })
    );
    let listed: Vec<(&str, bool)> = review
        .offenders
        .iter()
        .filter(|e| e.dimension == "nesting")
        .map(|e| (e.name.as_str(), e.gating))
        .collect();
    assert_eq!(listed, [("beta_depth_six", true)]);
    // The 4-deep function on `main` is an offender too, and unchanged: it is
    // counted, never listed, and never gates.
    assert!(review.offender_counts.unchanged >= 1, "{:?}", review.offender_counts);

    // A policy that names no nesting gates nothing here.
    rules(root, "[review]\ngate_dimensions = [\"cohesion\"]\n");
    let ungated = gate(&engine);
    assert!(ungated.gating_entries.is_empty() && ungated.passed, "{ungated:#?}");
}

/// [UAT-GV-15] steps 2 and 3: an Equality-only drift — a new above-mean
/// function — passes and is reported under `"report"`, fails at
/// `drift_budget = 2`, and passes at a budget equal to it. The plain gate
/// fails the same drift.
#[test]
fn an_equality_only_drift_is_reported_and_fails_only_past_its_budget() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    add_above_mean_function(root, &engine);

    let plain = gate(&engine);
    assert!(
        !plain.passed && plain.message.starts_with("signal regressed"),
        "the drift fails the plain gate: {plain:#?}"
    );
    let regressed: Vec<&str> = plain.regressions.iter().map(|r| r.metric.as_str()).collect();
    assert_eq!(regressed, ["equality"], "the drift is Equality's alone: {plain:#?}");

    rules(root, "[review]\n");
    let reported = gate(&engine);
    assert!(reported.passed, "{reported:#?}");
    assert!(reported.gating_entries.is_empty());
    let drift = reported.drift.expect("the drift is reported");
    assert!(drift.drop > 2, "the fixture drifts past the budget below: {drift:?}");
    assert_eq!((drift.budget, drift.exceeded), (None, false));
    assert_eq!(i64::from(drift.baseline) - i64::from(drift.current), drift.drop);
    assert_eq!(
        reported.message,
        format!(
            "signal {} drifted {} below the baseline {} (drift_budget \"report\": reported, never failing)",
            drift.current, drift.drop, drift.baseline
        )
    );
    assert_eq!(reported.regressions.len(), 1, "per-metric movement is still detail");

    // The above-mean function is a new Equality entry, which never gates.
    let review = engine.review(None).expect("review runs");
    let equality = review
        .offenders
        .iter()
        .find(|e| e.dimension == "equality" && e.name == "g_above_mean")
        .expect("the new function is listed under Equality");
    assert_eq!((equality.status, equality.gating), (ReviewStatus::New, false));
    assert_eq!(review.policy.map(|p| p.gating), Some(0));

    rules(root, "[review]\ndrift_budget = 2\n");
    let budgeted = gate(&engine);
    assert!(!budgeted.passed, "{budgeted:#?}");
    assert_eq!(budgeted.exit_code(), 1);
    let past = budgeted.drift.expect("the drift is judged");
    assert_eq!((past.drop, past.budget, past.exceeded), (drift.drop, Some(2.0), true));
    assert!(budgeted.message.ends_with("past drift_budget 2"), "{}", budgeted.message);

    rules(root, &format!("[review]\ndrift_budget = {}\n", drift.drop));
    let within = gate(&engine);
    assert!(within.passed, "a drift of exactly the budget passes: {within:#?}");
    assert!(within.message.ends_with(&format!("within drift_budget {}", drift.drop)), "{}", within.message);
}

/// Uncommitted work at the merge-base: every gate records a snapshot at
/// `HEAD`, over the working tree. Read as the before-state, it would make the
/// failing entry `unchanged` on the next run; under the policy the merge-base
/// tree is measured instead, so the verdict holds.
#[test]
fn the_verdict_over_uncommitted_work_at_the_merge_base_holds_on_a_rerun() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));

    for run in 1..=3 {
        let result = engine.session_end().expect("session_end runs");
        let names: Vec<&str> = result.gating_entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["beta_depth_six"], "run {run}: {result:#?}");
        assert!(!result.passed, "run {run}");
    }
    let review = engine.review(None).expect("review runs");
    assert_eq!(review.before_state, Some(BranchReview::SHADOW_INDEX));
}

/// The same work, failed while uncommitted at the merge-base and then
/// committed on a branch: the snapshot the failing gate recorded sits at the
/// merge-base commit and holds the offender, so read as the before-state it
/// would make the committed function `unchanged` and the gate pass. Under the
/// policy no recorded snapshot is read; the verdict holds.
#[test]
fn committing_work_a_gate_failed_does_not_make_it_pass() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    git(root, &["checkout", "-q", "-b", "deep"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    let uncommitted = engine.session_end().expect("session_end runs");
    assert!(!uncommitted.passed, "{uncommitted:#?}");

    commit(root, "add a 6-deep function");
    engine.index();
    let committed = engine.session_end().expect("session_end runs");
    let names: Vec<&str> = committed.gating_entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["beta_depth_six"], "{committed:#?}");
    assert!(!committed.passed);
    let review = engine.review(None).expect("review runs");
    assert_eq!(review.before_state, Some(BranchReview::SHADOW_INDEX), "no recorded snapshot under the policy");
}

/// `gate --save` records a baseline and judges nothing, under the policy as
/// without it: a branch with a gating entry saves and passes.
#[test]
fn a_save_under_the_policy_judges_nothing() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_deep_function(root, &engine);

    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.passed && saved.saved, "{saved:#?}");
    assert!(saved.gating_entries.is_empty() && saved.drift.is_none(), "{saved:#?}");
    assert!(saved.message.starts_with("baseline saved"), "{}", saved.message);
    assert!(!gate(&engine).passed, "the compare after it still judges the branch");
}

/// The explicit `--threshold` floor still fails a gate whose drift the
/// budget reports.
#[test]
fn the_threshold_floor_still_fails_under_the_policy() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_above_mean_function(root, &engine);

    let floored = engine.gate(Some(10_001), false, true).expect("gate runs");
    assert!(!floored.passed, "{floored:#?}");
    assert!(floored.drift.is_some_and(|d| !d.exceeded), "the drift alone passes: {floored:#?}");
    assert!(floored.message.contains("under the required threshold 10001"), "{}", floored.message);
}

/// A builder worth copying: the UAT-QM-12 near-clone shape.
const BUILDER: &str = "\
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
";

/// The builder copied with its identifiers renamed and one line added.
const BUILDER_COPY: &str = "\
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

/// A default gating dimension with no per-unit threshold gates too: a copied
/// builder is a new near-clone group, and each member is named with "no
/// threshold".
#[test]
fn a_new_near_clone_group_gates_though_uniqueness_has_no_threshold() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    git(root, &["checkout", "-q", "-b", "copy"]);
    write(root, "src/lib.rs", "pub mod deep;\npub mod many;\npub mod build;\n");
    write(root, "src/build.rs", &format!("{BUILDER}\n{BUILDER_COPY}"));
    commit(root, "a builder and its copy");
    engine.index();

    let result = gate(&engine);
    assert!(!result.passed, "{result:#?}");
    let mut copies: Vec<(&str, Option<i64>, Option<i64>)> = result
        .gating_entries
        .iter()
        .filter(|e| e.dimension == "uniqueness")
        .map(|e| (e.name.as_str(), e.after, e.threshold))
        .collect();
    copies.sort_unstable();
    assert_eq!(copies, [("build_sums", Some(2), None), ("build_totals", Some(2), None)], "{result:#?}");
    assert!(
        result.message.contains("uniqueness `build_sums` (src/build.rs:")
            && result.message.contains("clone_group_size 2, no threshold [new, trust resolved]"),
        "{}",
        result.message
    );
}

/// Without the table the gate carries exactly the fields it carried before
/// CR-237, judges by `baseline − ε`, and the review marks nothing.
#[test]
fn without_a_review_table_gate_and_review_carry_none_of_its_fields() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[constraints]\nmax_cc = 50\n");
    add_deep_function(root, &engine);

    let result = gate(&engine);
    assert!(result.gating_entries.is_empty() && result.drift.is_none(), "{result:#?}");
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(
        keys(&json),
        [
            "passed", "saved", "signal", "baseline_signal", "test_function_count", "threshold", "epsilon",
            "regressions", "structural_faults", "freshness", "message", "warnings",
        ]
        .map(String::from),
        "{json}"
    );
    assert!(
        result.message.starts_with("signal regressed") || result.message.contains("holds the baseline"),
        "BR-10 judges: {}",
        result.message
    );

    let review = engine.review(None).expect("review runs");
    assert!(review.policy.is_none());
    let text = serde_json::to_string(&review).unwrap();
    assert!(!text.contains("\"policy\"") && !text.contains("\"gating\""), "{text}");
    assert!(review.offenders.iter().any(|e| e.name == "beta_depth_six" && !e.gating));
}

/// A branch delta that cannot be measured — no default branch to compare
/// against — warns and judges nothing; the aggregate is still a drift budget.
#[test]
fn an_unmeasured_delta_warns_and_never_fails_the_gate() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_deep_function(root, &engine);
    git(root, &["branch", "-q", "-m", "main", "trunk"]);

    let result = gate(&engine);
    assert!(result.passed, "{result:#?}");
    assert!(result.gating_entries.is_empty());
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.starts_with("[review]: the branch delta was not measured") && w.contains("no default branch resolves")),
        "{:?}",
        result.warnings
    );
    assert!(result.drift.is_some(), "the drift budget still judges the aggregate");
}

// ── S-755: attribution and the re-bless record ─────────────────────────────

/// The attribution's own arithmetic: the contributions and the residual make
/// the exact change, the residual is under half a point, and the rounding is
/// the rest of the observed change.
fn assert_sums(a: &SignalAttribution) {
    let points: f64 = a.contributions.iter().map(|c| c.points).sum();
    let (exact, residual, rounding) = (a.exact_change.unwrap(), a.residual.unwrap(), a.rounding.unwrap());
    assert!(residual.abs() < 0.5, "residual {residual}: {a:#?}");
    assert!((points + residual - exact).abs() < 1e-9, "Σ {points} + residual {residual} ≠ {exact}: {a:#?}");
    assert!(rounding.abs() < 1.0, "the integer rounding is under a point: {a:#?}");
    assert!(
        (exact + rounding - a.change.unwrap() as f64).abs() < 0.011,
        "exact {exact} + rounding {rounding} make the observed change: {a:#?}"
    );
}

/// [UAT-GV-16] step 1 on a fixture: a new above-mean function lowers Equality,
/// and `review`, `gate` and `session_end` attribute the fall to it alike.
#[test]
fn the_change_is_attributed_to_its_dimension_and_its_driving_symbol() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_above_mean_function(root, &engine);

    let review = engine.review(None).expect("review runs").attribution.expect("measured, so attributed");
    assert_sums(&review);
    assert!(review.change.unwrap() < -2, "{review:#?}");
    let equality = review.contributions.first().expect("a contribution");
    assert_eq!(equality.dimension, "equality", "the largest share: {review:#?}");
    assert!(equality.points < 0.0 && equality.before > equality.after, "{equality:#?}");
    let drivers: Vec<(&str, ReviewStatus, Option<i64>)> =
        equality.drivers.iter().map(|d| (d.name.as_str(), d.status, d.after)).collect();
    assert_eq!(drivers, [("g_above_mean", ReviewStatus::New, Some(6))], "{equality:#?}");
    assert_eq!(equality.driver_count, 1);

    // The gate attributes the change from its baseline, saved at `main`, and
    // reads the same measurement on both sides.
    for gate in [gate(&engine), engine.session_end().expect("session_end runs")] {
        assert_eq!(gate.attribution.as_ref(), Some(&review), "{gate:#?}");
    }
    // The read-only verdict the dashboard shows, over the snapshot just taken.
    let latest = engine.latest_gate().expect("the read-only verdict");
    assert_eq!(latest.attribution.as_ref(), Some(&review), "{latest:#?}");
}

/// A copied builder is a near-clone group: Uniqueness's fall names the group,
/// with both members.
#[test]
fn a_new_near_clone_group_is_named_with_its_members() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    git(root, &["checkout", "-q", "-b", "copy"]);
    write(root, "src/lib.rs", "pub mod deep;\npub mod many;\npub mod build;\n");
    write(root, "src/build.rs", &format!("{BUILDER}\n{BUILDER_COPY}"));
    commit(root, "a builder and its copy");
    engine.index();

    let review = engine.review(None).expect("review runs");
    // `main`'s saved baseline is a snapshot at the merge-base, and no rule is
    // declared: the before-state's terms are read from that row.
    assert_eq!(review.before_state, Some(BranchReview::RECORDED_SNAPSHOT));
    let a = review.attribution.expect("attributed");
    assert_sums(&a);
    let uniqueness = a.contributions.iter().find(|c| c.dimension == "uniqueness").expect("listed");
    assert!(uniqueness.points < 0.0, "{a:#?}");
    assert_eq!(uniqueness.driver_count, 1, "{uniqueness:#?}");
    let group = &uniqueness.drivers[0];
    assert_eq!((group.status, group.after, group.measure.as_deref()), (ReviewStatus::New, Some(2), Some("group_size")));
    let members: Vec<&str> = group.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["build_totals", "build_sums"], "by line in src/build.rs: {group:#?}");

    // Merged into `main`, the copy then removed on a branch: Uniqueness rises,
    // and the group that went is named, resolved.
    git(root, &["checkout", "-q", "main"]);
    git(root, &["merge", "-q", "--ff-only", "copy"]);
    git(root, &["checkout", "-q", "-b", "dedupe"]);
    write(root, "src/build.rs", BUILDER);
    commit(root, "the copy goes");
    engine.index();
    let a = engine.review(None).expect("review runs").attribution.expect("attributed");
    assert_sums(&a);
    let uniqueness = a.contributions.iter().find(|c| c.dimension == "uniqueness").expect("listed");
    assert!(uniqueness.points > 0.0, "{a:#?}");
    let gone: Vec<(ReviewStatus, Option<i64>, Vec<&str>)> = uniqueness
        .drivers
        .iter()
        .map(|g| (g.status, g.before, g.members.iter().map(|m| m.name.as_str()).collect()))
        .collect();
    assert_eq!(gone, [(ReviewStatus::Resolved, Some(2), vec!["build_totals", "build_sums"])], "{uniqueness:#?}");
}

/// `logos.db`'s baseline record, read beside the engine: `(reason, attribution)`.
fn baseline_record(root: &Path) -> (Option<String>, Option<String>) {
    let conn = rusqlite::Connection::open(root.join(".logos/logos.db")).unwrap();
    conn.query_row("SELECT reason, attribution FROM baseline", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
}

/// The stored attribution, parsed.
fn parsed(stored: Option<String>) -> serde_json::Value {
    serde_json::from_str(&stored.expect("an attribution is stored")).expect("the stored attribution is JSON")
}

/// [UAT-GV-16] steps 2 and 3: a re-bless past `drift_budget` without a reason
/// is a usage error and moves no baseline; with one, the reason and the
/// attribution are stored with the baseline and the next compare names the
/// reason. A later anchor clears both.
#[test]
fn a_rebless_past_the_budget_requires_a_reason_and_records_it() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\ndrift_budget = 2\n");
    add_above_mean_function(root, &engine);
    let before = gate(&engine);
    let drift = before.drift.expect("judged");
    assert!(drift.exceeded, "the fixture drifts past the budget: {drift:?}");

    for reason in [None, Some("  ")] {
        let err = engine.gate_with_reason(None, true, true, reason).expect_err("refused");
        let config = err.downcast_ref::<ConfigError>().unwrap_or_else(|| panic!("a usage error: {err:#}"));
        assert_eq!(config.exit_code(), 2);
        let text = err.to_string();
        assert!(
            text.contains("gate --reason") && text.contains("past drift_budget 2") && text.contains("--reason"),
            "{text}"
        );
    }
    let unmoved = gate(&engine);
    assert_eq!(unmoved.baseline_signal, Some(drift.baseline), "no baseline moved: {unmoved:#?}");
    assert_eq!(baseline_record(root), (None, None));

    let why = "an above-mean dispatcher is intrinsic to the feature";
    let saved = engine.gate_with_reason(None, true, true, Some(why)).expect("gate --save --reason");
    assert!(saved.saved && saved.passed, "{saved:#?}");
    assert_eq!(saved.baseline_reason.as_deref(), Some(why));
    let blessed = saved.attribution.clone().expect("the blessed change, under the table");
    assert_sums(&blessed);
    assert_eq!(blessed.change, Some(-drift.drop));
    let (reason, stored) = baseline_record(root);
    assert_eq!(reason.as_deref(), Some(why));
    assert_eq!(stored, Some(serde_json::to_string(&blessed).unwrap()), "the record is the reported attribution");

    let next = gate(&engine);
    assert!(next.passed && next.baseline_reason.as_deref() == Some(why), "{next:#?}");
    let web = engine.latest_gate().expect("the read-only verdict");
    assert_eq!(web.baseline_reason.as_deref(), Some(why), "the dashboard names it too: {web:#?}");
    assert_eq!(next.attribution.as_ref().and_then(|a| a.change), Some(0), "nothing moved since the bless");

    // A save within the budget needs no reason, and replaces the record.
    let plain = engine.gate(None, true, true).expect("a save with nothing to bless");
    assert!(plain.saved && plain.baseline_reason.is_none(), "{plain:#?}");
    let (reason, stored) = baseline_record(root);
    assert_eq!(reason, None, "a reason never outlives its anchor");
    assert_eq!(parsed(stored)["change"], 0);
    // `session_start` moves no existing baseline (S-763), so the record stays.
    engine.gate_with_reason(None, true, true, Some(why)).expect("record a reason again");
    let kept = baseline_record(root);
    engine.session_start().expect("session_start runs");
    assert_eq!(kept.0.as_deref(), Some(why));
    assert_eq!(baseline_record(root), kept, "the reason outlives no anchor: the anchor stayed");
}

/// Without the table a save needs no reason whatever it blesses, reports no
/// attribution, and still records it — with the reason when one is given.
#[test]
fn without_the_table_a_save_records_the_attribution_and_reports_none() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    add_above_mean_function(root, &engine);
    assert!(!gate(&engine).passed, "the drift fails the plain gate");

    let saved = engine.gate(None, true, true).expect("gate --save without a reason");
    assert!(saved.saved && saved.attribution.is_none() && saved.baseline_reason.is_none(), "{saved:#?}");
    let json = serde_json::to_value(&saved).unwrap();
    assert!(json.get("attribution").is_none() && json.get("baseline_reason").is_none(), "{json}");
    let (reason, stored) = baseline_record(root);
    assert_eq!(reason, None);
    let stored = parsed(stored);
    assert!(stored["change"].as_i64().unwrap() < 0, "{stored}");
    assert_eq!(stored["contributions"][0]["dimension"], "equality", "{stored}");
    assert_eq!(stored["contributions"][0]["drivers"][0]["name"], "g_above_mean", "{stored}");

    let compare = gate(&engine);
    assert!(compare.attribution.is_none(), "a compare without the table attributes nothing: {compare:#?}");
    let web = engine.latest_gate().expect("the read-only verdict");
    assert!(web.attribution.is_none(), "nor does the dashboard's verdict: {web:#?}");
}

/// A save across a threshold change has no comparable baseline to attribute
/// against, so it needs no reason however far the signal moved, and records
/// none.
#[test]
fn a_save_across_a_threshold_change_attributes_nothing_and_needs_no_reason() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\ndrift_budget = 0\n[metric_thresholds]\nduplicate_min_tokens = 5\n");
    add_above_mean_function(root, &engine);

    let saved = engine.gate(None, true, true).expect("no reason is needed across a threshold change");
    assert!(saved.saved && saved.attribution.is_none(), "{saved:#?}");
    assert_eq!(baseline_record(root), (None, None));
}

/// The read-only verdict reads the table without the parse cache, and an
/// unreadable contract attributes nothing there: the verdict still stands.
#[test]
fn an_unreadable_contract_leaves_the_read_only_verdict_standing() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_above_mean_function(root, &engine);
    assert!(gate(&engine).attribution.is_some(), "the gate attributes under the table");
    rules(root, "[review]\nnot_a_key = 1\n");
    let web = engine.latest_gate().expect("the verdict stands over an invalid contract");
    assert!(web.attribution.is_none() && web.baseline_signal.is_some(), "{web:#?}");
}

/// A baseline snapshot whose offender values were never recorded (written
/// before FR-QM-19) still has its change attributed, names no driver, and
/// says why.
#[test]
fn a_baseline_without_recorded_offender_values_names_no_driver_and_says_why() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_above_mean_function(root, &engine);
    let conn = rusqlite::Connection::open(root.join(".logos/logos.db")).unwrap();
    conn.execute(
        "UPDATE metric_snapshots SET offender_values_recorded = NULL \
         WHERE id = (SELECT snapshot_id FROM baseline)",
        [],
    )
    .unwrap();
    drop(conn);

    let a = gate(&engine).attribution.expect("attributed");
    assert_sums(&a);
    assert!(a.contributions.iter().any(|c| c.points != 0.0), "{a:#?}");
    assert!(a.contributions.iter().all(|c| c.drivers.is_empty() && c.driver_count == 0), "{a:#?}");
    assert!(
        a.drivers_not_named_because.as_deref().is_some_and(|w| w.contains("predates recorded offender values")),
        "{a:#?}"
    );
}

// ── S-756, FR-GV-27: the trend warning ──────────────────────────────────────

/// `src/many.rs` with `extra` above-mean functions appended, of complexity 6,
/// 7, …: each one lowers Equality.
fn many_plus(extra: usize) -> String {
    let added: String = (0..extra).map(|i| branches(&format!("g_above_mean_{i}"), 5 + i)).collect();
    format!("{}{added}", many())
}

/// The trend warnings among `warnings`.
fn trend(warnings: &[String]) -> Vec<String> {
    warnings.iter().filter(|w| w.contains("review.trend_window")).cloned().collect()
}

/// `scan` after `extra` above-mean functions: its trend warnings and its
/// Equality, which must have fallen from `before` when one is given.
fn scan_with(root: &Path, engine: &Engine, extra: usize, before: Option<f64>) -> (Vec<String>, f64) {
    write(root, "src/many.rs", &many_plus(extra));
    let scan = engine.scan(true).expect("scan runs");
    let equality = scan.metrics.equality.normalized;
    if let Some(before) = before {
        assert!(equality < before, "the fixture lowers Equality: {before} → {equality}");
    }
    (trend(&scan.warnings), equality)
}

/// [UAT-GV-17] and [FR-GV-27] acceptance 1 and 2: no warning after the first
/// Equality decline; after the second, `scan`, `gate`, `session_end` and
/// `review` each carry one naming Equality with both declined values. The
/// gate re-measures the same tree, which neither breaks the run nor extends
/// it. Every verdict and exit code is the one a window the run does not fill
/// gives over the same tree.
///
/// [UAT-GV-17]: ../../docs/specs/requirements/UAT-GV-17.md
/// [FR-GV-27]: ../../docs/specs/requirements/FR-GV-27.md
#[test]
fn two_equality_declines_warn_on_scan_gate_and_review_and_move_no_exit_code() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    let (none, start) = scan_with(root, &engine, 0, None);
    assert!(none.is_empty(), "{none:?}");
    let (one, first) = scan_with(root, &engine, 1, Some(start));
    assert!(one.is_empty(), "one decline does not warn: {one:?}");
    let (two, second) = scan_with(root, &engine, 2, Some(first));
    let expected = format!(
        "equality declined at each of the last 2 recorded snapshots that moved it: {start} → {first} → {second} \
         (review.trend_window = 2); a trend warning, never a failure"
    );
    assert_eq!(two, std::slice::from_ref(&expected));

    let warned = gate(&engine);
    assert_eq!(trend(&warned.warnings), std::slice::from_ref(&expected), "{warned:#?}");
    let ended = engine.session_end().expect("session_end runs");
    assert_eq!(trend(&ended.warnings), std::slice::from_ref(&expected));
    let reviewed = engine.review(None).expect("review runs");
    assert_eq!(reviewed.warnings, std::slice::from_ref(&expected));
    // The trend reads the recorded series, so a review that cannot measure
    // still carries it.
    let unmeasured = engine.review(Some("no-such-ref")).expect("review runs");
    assert!(!unmeasured.measured, "{unmeasured:#?}");
    assert_eq!(unmeasured.warnings, std::slice::from_ref(&expected));

    rules(root, "[review]\ntrend_window = 99\n");
    let quiet = gate(&engine);
    assert!(trend(&quiet.warnings).is_empty(), "{quiet:#?}");
    assert_eq!(
        (quiet.passed, quiet.exit_code(), &quiet.message, quiet.gating_entries.len()),
        (warned.passed, warned.exit_code(), &warned.message, warned.gating_entries.len()),
        "the warning moves no verdict"
    );
    assert_eq!(quiet.warnings.len() + 1, warned.warnings.len());
    let unwarned = engine.review(None).expect("review runs");
    assert!(unwarned.warnings.is_empty());
    assert_eq!((unwarned.measured, unwarned.policy), (reviewed.measured, reviewed.policy));

    // `gate --save` carries it too, and saves all the same. Last: a save
    // moves the baseline the verdicts above compare against.
    rules(root, "[review]\n");
    let saved = engine.gate(None, true, true).expect("gate --save runs");
    assert!(saved.saved && saved.passed, "{saved:#?}");
    assert_eq!(trend(&saved.warnings), [expected]);
}

/// [FR-GV-27] acceptance 3: under `trend_window = 3` two declines do not warn
/// and a third does, naming all four values.
///
/// [FR-GV-27]: ../../docs/specs/requirements/FR-GV-27.md
#[test]
fn a_trend_window_of_three_needs_three_declines() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\ntrend_window = 3\n");
    let (_, start) = scan_with(root, &engine, 0, None);
    let (one, first) = scan_with(root, &engine, 1, Some(start));
    let (two, second) = scan_with(root, &engine, 2, Some(first));
    assert!(one.is_empty() && two.is_empty(), "{one:?} {two:?}");
    let (three, third) = scan_with(root, &engine, 3, Some(second));
    assert_eq!(three.len(), 1, "{three:?}");
    assert!(
        three[0].starts_with(&format!(
            "equality declined at each of the last 3 recorded snapshots that moved it: \
             {start} → {first} → {second} → {third}"
        )),
        "{three:?}"
    );
}

/// Without the table the same two declines raise no warning anywhere, and the
/// review answer carries no `warnings` key: the trend is part of the opt-in.
#[test]
fn without_the_table_two_declines_warn_nowhere() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    let (_, start) = scan_with(root, &engine, 0, None);
    let (_, first) = scan_with(root, &engine, 1, Some(start));
    let (two, _) = scan_with(root, &engine, 2, Some(first));
    assert!(two.is_empty(), "{two:?}");
    assert!(trend(&gate(&engine).warnings).is_empty());
    let review = serde_json::to_value(engine.review(None).expect("review runs")).unwrap();
    assert!(!keys(&review).contains(&"warnings".to_string()), "{review}");
}

/// A review that cannot measure answers `measured: false` over an invalid
/// table, as it did before the trend: only the trend reads the contract on
/// that path, and it reads it fail-soft. A measured review still fails loud.
#[test]
fn an_unmeasured_review_over_an_invalid_table_answers_as_before() {
    let (tmp, engine) = repo();
    rules(tmp.path(), "[review]\ntrend_window = 0\n");
    let review = engine.review(Some("no-such-ref")).expect("an unmeasured review is an answer");
    assert!(!review.measured && review.warnings.is_empty(), "{review:#?}");
    let err = engine.review(None).expect_err("a measured review loads the contract");
    assert!(format!("{err:#}").contains("review.trend_window"), "{err:#}");
}

/// FR-GV-25 on the dashboard (S-754 × S-755): under the table the read-only
/// verdict judges the aggregate by `drift_budget`, as `gate` does — a drift
/// `"report"` reports never fails, and a numeric budget fails past it — and
/// says that per-symbol entries are the persisting gate's.
#[test]
fn the_read_only_verdict_judges_the_drift_budget_as_the_gate_does() {
    let (tmp, engine) = repo();
    let root = tmp.path();
    rules(root, "[review]\n");
    add_above_mean_function(root, &engine);

    let persisting = gate(&engine);
    let web = engine.latest_gate().expect("the read-only verdict");
    let drift = persisting.drift.expect("a drift under the table");
    assert!(drift.drop > 0 && persisting.passed, "{persisting:#?}");
    assert_eq!((web.passed, web.message.as_str(), web.drift.as_ref()), (true, persisting.message.as_str(), Some(&drift)), "{web:#?}");
    assert!(web.notes.iter().any(|n| n.contains("per-symbol `[review]` entries")), "{web:#?}");

    rules(root, "[review]\ndrift_budget = 0\n");
    let persisting = gate(&engine);
    let web = engine.latest_gate().expect("the read-only verdict");
    assert!(!persisting.passed, "{persisting:#?}");
    assert_eq!((web.passed, web.message.as_str()), (false, persisting.message.as_str()), "{web:#?}");
}
