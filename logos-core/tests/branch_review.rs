//! The branch delta (S-677, [FR-GV-24], [UAT-GV-14], [ADR-82]), end-to-end
//! through `Engine::review` over real git fixtures.
//!
//! Coverage by acceptance criterion:
//! - a branch adding a 6-deep function reports it `new` with value 6 and
//!   threshold 4; the fix, once the base carries the function, reads
//!   `resolved`;
//! - a copied builder is a new near-clone group;
//! - with no snapshot at the merge-base the first run builds and caches the
//!   shadow before-state and the second reuses it, and a snapshot recorded at
//!   any other commit is never used;
//! - Python entries carry the same-file trust tag, and an unsynced edit
//!   `stale`; reruns are byte-identical, and `check`/`gate` are unchanged.
//!
//! Every fixture is a throw-away repository with `GIT_CEILING_DIRECTORIES` set
//! and git maintenance off, so nothing escapes into an enclosing repository.
//!
//! [FR-GV-24]: ../../docs/specs/requirements/FR-GV-24.md
//! [UAT-GV-14]: ../../docs/specs/requirements/UAT-GV-14.md
//! [ADR-82]: ../../docs/specs/architecture/decisions/ADR-82.md

#![cfg(all(feature = "lang-rust", feature = "lang-python"))]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::quality::{
    BeforeStateCache, BranchReview, CrossFileAbsence, ReviewStatus, ReviewedOffender,
};
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

fn rev(cwd: &Path, reference: &str) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", reference])
        .env("GIT_CEILING_DIRECTORIES", cwd.parent().unwrap())
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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

/// A Rust repository whose `main` holds a 4-deep function and the builder.
fn rust_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod deep;\npub mod build;\n");
    write(root, "src/deep.rs", &nested("alpha_depth_four", 4));
    write(root, "src/build.rs", BUILDER);
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    tmp
}

fn started(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    engine
}

fn review(engine: &Engine, base: &str) -> BranchReview {
    let review = engine.review(Some(base)).expect("review runs");
    assert!(review.measured, "measured: {:?}", review.not_measured_because);
    review
}

fn entry<'a>(review: &'a BranchReview, dimension: &str, name: &str) -> Option<&'a ReviewedOffender> {
    review
        .offenders
        .iter()
        .find(|e| e.dimension == dimension && e.name == name)
}

fn cache_files(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join(".logos/review-cache"))
        .map(|d| d.filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned())).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// The wire tag of a same-file-only language, read from the type's own
/// serialisation rather than spelled.
fn same_file_tag() -> String {
    let absence = CrossFileAbsence::SameFileOnly { same_file_edges: 1 };
    serde_json::to_value(absence).unwrap()["cause"].as_str().unwrap().to_string()
}

#[test]
fn a_new_deep_function_reads_new_against_the_recorded_snapshot_and_its_fix_resolved() {
    let tmp = rust_repo();
    let root = tmp.path();
    let engine = started(root);
    engine.scan(true).expect("scan records a snapshot at main");

    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    engine.index();
    // A later snapshot at the branch head: the record read is by the
    // merge-base's own snapshot, never the latest one.
    engine.scan(true).expect("scan records a snapshot at the branch head");

    let added = review(&engine, "main");
    assert_eq!(added.merge_base.as_deref(), Some(rev(root, "main").as_str()));
    assert_eq!(added.before_state, Some(BranchReview::RECORDED_SNAPSHOT));
    assert_eq!(added.before_state_cache, BeforeStateCache::NotUsed);
    let beta = entry(&added, "nesting", "beta_depth_six").expect("the deep function is listed");
    assert_eq!(
        (beta.status, beta.before, beta.after, beta.threshold),
        (ReviewStatus::New, None, Some(6), Some(4)),
        "{beta:?}"
    );
    assert_eq!(beta.trust, BranchReview::TRUST_RESOLVED);
    assert!(
        entry(&added, "nesting", "alpha_depth_four").is_none(),
        "the base's unchanged offender is counted, not listed"
    );
    assert!(added.offenders.iter().all(|e| e.status != ReviewStatus::Unchanged));
    assert!(added.offender_counts.unchanged >= 1, "{:?}", added.offender_counts);
    assert!(
        added.notes.iter().any(|n| n.contains("unchanged entry is counted")),
        "the answer states its own elision: {:?}",
        added.notes
    );
    assert_eq!(added.offender_counts.new, added.offenders.iter().filter(|e| e.status == ReviewStatus::New).count() as u64);

    // The branch lands; a fix branch off the new main flattens the function.
    git(root, &["checkout", "-q", "main"]);
    git(root, &["merge", "-q", "--ff-only", "feature"]);
    engine.index();
    engine.scan(true).expect("scan records a snapshot at the new main");
    git(root, &["checkout", "-q", "-b", "fix"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 2)));
    commit(root, "flatten it");
    engine.index();

    let fixed = review(&engine, "main");
    let beta = entry(&fixed, "nesting", "beta_depth_six").expect("the fixed function is listed");
    assert_eq!((beta.status, beta.before, beta.after), (ReviewStatus::Resolved, Some(6), None), "{beta:?}");
}

#[test]
fn a_copied_builder_is_a_new_near_clone_group() {
    let tmp = rust_repo();
    let root = tmp.path();
    let engine = started(root);
    engine.scan(true).expect("scan");
    git(root, &["checkout", "-q", "-b", "copy"]);
    write(root, "src/build.rs", &format!("{BUILDER}\n{BUILDER_COPY}"));
    commit(root, "copy the builder");
    engine.index();

    let copied = review(&engine, "main");
    let names: Vec<Vec<(&str, bool)>> = copied
        .introduced_clone_groups
        .iter()
        .map(|g| g.members.iter().map(|m| (m.name.as_str(), m.newly_duplicated)).collect())
        .collect();
    assert_eq!(names, vec![vec![("build_totals", true), ("build_sums", true)]], "{copied:#?}");
    let copy = entry(&copied, "uniqueness", "build_sums").expect("the copy is a uniqueness offender");
    assert_eq!((copy.status, copy.after), (ReviewStatus::New, Some(2)));
}

#[test]
fn without_a_merge_base_snapshot_the_shadow_is_built_once_cached_and_reused() {
    let tmp = rust_repo();
    let root = tmp.path();
    let engine = started(root);
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    engine.index();
    // A snapshot at the branch head: a commit that is not the merge-base, so
    // it must never stand in for the before-state.
    engine.scan(true).expect("scan records a snapshot at the branch head");
    let check_before = engine.check_rules(None, false).expect("check runs");
    let gate_before = engine.gate(None, false, false).expect("gate runs");
    let snapshots_before = engine.evolution(None).expect("evolution").snapshots.len();

    let first = review(&engine, "main");
    let merge_base = rev(root, "main");
    assert_eq!(first.before_state, Some(BranchReview::SHADOW_INDEX));
    assert_eq!(first.before_state_cache, BeforeStateCache::Built);
    assert_eq!(cache_files(root), vec![format!("{merge_base}.json")], "only the merge-base is cached");
    let cached = root.join(format!(".logos/review-cache/{merge_base}.json"));
    let (bytes, modified) = (fs::read(&cached).unwrap(), fs::metadata(&cached).unwrap().modified().unwrap());
    let beta = entry(&first, "nesting", "beta_depth_six").expect("listed");
    assert_eq!((beta.status, beta.after, beta.threshold), (ReviewStatus::New, Some(6), Some(4)));
    assert!(
        entry(&first, "nesting", "alpha_depth_four").is_none() && first.offender_counts.unchanged >= 1,
        "the shadow measured main's tree, where the 4-deep function already was: {first:#?}"
    );

    let second = review(&engine, "main");
    assert_eq!(second.before_state_cache, BeforeStateCache::Reused, "no second index");
    assert_eq!(fs::read(&cached).unwrap(), bytes, "the cache entry is not rewritten");
    assert_eq!(fs::metadata(&cached).unwrap().modified().unwrap(), modified);
    assert_eq!(cache_files(root), vec![format!("{merge_base}.json")]);
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap(),
        "a rerun is byte-identical"
    );

    let check_after = engine.check_rules(None, false).expect("check runs");
    let gate_after = engine.gate(None, false, false).expect("gate runs");
    assert_eq!(check_after.exit_code(false), check_before.exit_code(false));
    assert_eq!(check_after.passed, check_before.passed);
    assert_eq!(check_after.violations, check_before.violations);
    assert_eq!(gate_after.exit_code(), gate_before.exit_code());
    assert_eq!(
        engine.evolution(None).expect("evolution").snapshots.len(),
        snapshots_before + 1,
        "the review appended no snapshot; only the gate above did"
    );
}

#[test]
fn a_contract_with_rules_classifies_its_violations_against_the_shadow() {
    let tmp = rust_repo();
    let root = tmp.path();
    write(root, ".logos/rules.toml", "[constraints]\nmax_nesting_depth = 4\n");
    let engine = started(root);
    engine.scan(true).expect("a snapshot at main, which a contract with rules cannot use");
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    engine.index();

    let delta = review(&engine, "main");
    assert_eq!(delta.before_state, Some(BranchReview::SHADOW_INDEX));
    let deep: Vec<_> = delta.violations.iter().filter(|v| v.rule == "max_nesting_depth").collect();
    assert_eq!(deep.len(), 1, "{:#?}", delta.violations);
    assert_eq!(
        (deep[0].status, deep[0].before, deep[0].after, deep[0].threshold),
        (ReviewStatus::New, None, Some(6), Some(4)),
        "{deep:?}"
    );
    assert!(deep[0].subject.ends_with("beta_depth_six()."), "{deep:?}");
    assert_eq!(delta.violation_counts.new, 1);

    // The cached before-state was measured under this contract; another
    // contract re-measures rather than reading it back.
    assert_eq!(review(&engine, "main").before_state_cache, BeforeStateCache::Reused);
    write(root, ".logos/rules.toml", "[constraints]\nmax_nesting_depth = 5\n");
    let retuned = review(&engine, "main");
    assert_eq!(retuned.before_state_cache, BeforeStateCache::Built, "a contract edit re-measures");
    let deep: Vec<_> = retuned.violations.iter().filter(|v| v.rule == "max_nesting_depth").collect();
    assert_eq!((deep[0].after, deep[0].threshold), (Some(6), Some(5)), "{deep:?}");
    assert_eq!(deep[0].trust, BranchReview::TRUST_RESOLVED);

    // An edit without a sync: the violation's file no longer matches the graph.
    write(root, "src/deep.rs", &format!("{}{}// edited\n", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    let edited = review(&engine, "main");
    let deep: Vec<_> = edited.violations.iter().filter(|v| v.rule == "max_nesting_depth").collect();
    assert_eq!(deep[0].trust, BranchReview::TRUST_STALE, "{deep:?}");
}

#[test]
fn python_entries_carry_the_same_file_tag_and_an_unsynced_edit_reads_stale() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    let deep = |name: &str, depth: usize| -> String {
        let mut body = format!("def {name}(x):\n");
        for i in 0..depth {
            body.push_str(&format!("{}if x > {i}:\n", "    ".repeat(i + 1)));
        }
        body.push_str(&format!("{}return helper(x)\n    return 0\n\n", "    ".repeat(depth + 1)));
        body
    };
    write(root, "pkg/__init__.py", "");
    write(root, "pkg/core.py", &format!("def helper(x):\n    return x\n\n{}", deep("shallow", 2)));
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "pkg/core.py", &format!("def helper(x):\n    return x\n\n{}{}", deep("shallow", 2), deep("deep_one", 6)));
    commit(root, "add a deep function");
    let engine = started(root);

    let first = review(&engine, "main");
    assert_eq!(first.before_state, Some(BranchReview::SHADOW_INDEX));
    assert_eq!(first.before_state_cache, BeforeStateCache::Built, "no snapshot: the first run builds");
    let deep_one = entry(&first, "nesting", "deep_one").expect("listed");
    assert_eq!((deep_one.status, deep_one.after), (ReviewStatus::New, Some(6)));
    assert_eq!(deep_one.trust, same_file_tag(), "{deep_one:?}");
    assert!(first.offenders.iter().all(|e| e.trust == same_file_tag()), "{:#?}", first.offenders);
    let second = review(&engine, "main");
    assert_eq!(second.before_state_cache, BeforeStateCache::Reused);
    assert_eq!(serde_json::to_vec(&first).unwrap(), serde_json::to_vec(&second).unwrap());

    // Edit without syncing: the graph no longer matches the tree.
    write(root, "pkg/core.py", &format!("def helper(x):\n    return x + 1\n\n{}{}", deep("shallow", 2), deep("deep_one", 6)));
    let edited = review(&engine, "main");
    assert_eq!(edited.stale_files, vec!["pkg/core.py".to_string()]);
    let deep_one = entry(&edited, "nesting", "deep_one").expect("listed");
    assert_eq!(deep_one.trust, BranchReview::TRUST_STALE);

    // A file deleted without a sync is out of step with the graph too.
    fs::remove_file(root.join("pkg/__init__.py")).unwrap();
    let deleted = review(&engine, "main");
    assert_eq!(deleted.stale_files, vec!["pkg/__init__.py".to_string(), "pkg/core.py".to_string()]);
}

#[test]
fn a_base_that_does_not_resolve_classifies_nothing_and_says_why() {
    let tmp = rust_repo();
    let root = tmp.path();
    let engine = started(root);
    let missing = engine.review(Some("no-such-branch")).expect("review answers");
    assert!(!missing.measured);
    assert_eq!(missing.base_origin, BranchReview::STATED);
    assert!(
        missing.not_measured_because.as_deref().unwrap().contains("no-such-branch"),
        "{missing:?}"
    );
    assert!(missing.offenders.is_empty() && missing.violations.is_empty());
    assert!(cache_files(root).is_empty(), "nothing was measured or cached");

    let defaulted = engine.review(None).expect("review answers");
    assert_eq!(defaulted.base_origin, BranchReview::DEFAULT_BRANCH);
    assert_eq!(defaulted.base, "main", "no origin, so main is the default branch");
    assert!(defaulted.measured);
}

#[test]
fn the_merge_base_is_measured_under_the_live_contract_not_the_tree_s() {
    // The builder and its copy are near clones under the default similarity.
    // The contract that says otherwise is not in the tree (`.logos/` is
    // ignored), so the merge-base export cannot carry it: unless the shadow is
    // re-materialised under the live contract, the base reads a clone group
    // the working tree does not, and the pair reads `resolved`.
    //
    // The contract is also edited after the last index, so the live graph was
    // annotated under the old one: unless the after side is re-materialised
    // too, the pair reads `new` in a file that never changed.
    let tmp = rust_repo();
    let root = tmp.path();
    write(root, "src/build.rs", &format!("{BUILDER}\n{BUILDER_COPY}"));
    commit(root, "the copy is on main");
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "an unrelated change");
    let engine = started(root);
    write(root, ".logos/rules.toml", "[metric_thresholds]\nclone_similarity = 1.0\n");

    let delta = review(&engine, "main");
    assert_eq!(delta.before_state, Some(BranchReview::SHADOW_INDEX), "main has no snapshot");
    let uniqueness: Vec<_> = delta.offenders.iter().filter(|e| e.dimension == "uniqueness").collect();
    assert!(uniqueness.is_empty(), "no clone group on either side under the live contract: {uniqueness:#?}");
    assert!(delta.introduced_clone_groups.is_empty());
}

#[test]
fn a_merge_base_tree_that_cannot_be_measured_classifies_nothing_and_says_why() {
    let tmp = rust_repo();
    let root = tmp.path();
    write(root, "src/gone.rs", "pub fn gone() -> u32 {\n    7\n}\n");
    commit(root, "a file only main holds");
    git(root, &["checkout", "-q", "-b", "feature"]);
    git(root, &["rm", "-q", "src/gone.rs"]);
    commit(root, "drop it");
    // Its blob is now reachable from the merge-base alone; lose it.
    let blob = rev(root, "main:src/gone.rs");
    fs::remove_file(root.join(".git/objects").join(&blob[..2]).join(&blob[2..])).expect("a loose blob");
    let engine = started(root);

    let review = engine.review(Some("main")).expect("an unmeasurable base is an answer, not an error");
    assert!(!review.measured, "{review:?}");
    assert!(
        review.not_measured_because.as_deref().unwrap().starts_with("no comparable before-state"),
        "{review:?}"
    );
    assert_eq!(review.merge_base.as_deref(), Some(rev(root, "main").as_str()));
    assert!(review.offenders.is_empty() && review.before_state.is_none());
    assert!(cache_files(root).is_empty(), "nothing was cached");
}

/// Set one column of the snapshot recorded at `commit`, out of band.
fn tamper(root: &Path, commit: &str, assignment: &str) {
    let db = rusqlite::Connection::open(root.join(".logos/logos.db")).expect("the store opens");
    let changed = db
        .execute(&format!("UPDATE metric_snapshots SET {assignment} WHERE commit_sha = ?1"), [commit])
        .expect("the snapshot updates");
    assert_eq!(changed, 1, "one snapshot at {commit}");
}

#[test]
fn a_merge_base_snapshot_scored_otherwise_is_not_the_record() {
    let tmp = rust_repo();
    let root = tmp.path();
    let engine = started(root);
    engine.scan(true).expect("scan records a snapshot at main");
    let main = rev(root, "main");
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    engine.index();
    assert_eq!(review(&engine, "main").before_state, Some(BranchReview::RECORDED_SNAPSHOT));

    // Scored under other metric semantics, or before values were recorded:
    // the shadow measures the merge-base instead.
    for (spoil, restore) in [
        ("metric_version = metric_version - 1", "metric_version = metric_version + 1"),
        ("offender_values_recorded = NULL", "offender_values_recorded = 1"),
    ] {
        tamper(root, &main, spoil);
        let shadowed = review(&engine, "main");
        assert_eq!(shadowed.before_state, Some(BranchReview::SHADOW_INDEX), "after {spoil}");
        let beta = entry(&shadowed, "nesting", "beta_depth_six").expect("listed");
        assert_eq!(beta.status, ReviewStatus::New, "after {spoil}");
        tamper(root, &main, restore);
        assert_eq!(review(&engine, "main").before_state, Some(BranchReview::RECORDED_SNAPSHOT), "after {restore}");
    }

    // Scored under other thresholds — a contract with no rule, so only the
    // threshold hash tells them apart.
    write(root, ".logos/rules.toml", "[metric_thresholds]\nnesting_depth = 5\n");
    let retuned = review(&engine, "main");
    assert_eq!(retuned.before_state, Some(BranchReview::SHADOW_INDEX));
    let beta = entry(&retuned, "nesting", "beta_depth_six").expect("listed");
    assert_eq!((beta.status, beta.threshold), (ReviewStatus::New, Some(5)));
}

#[test]
fn a_base_with_no_common_history_is_never_measured() {
    let tmp = rust_repo();
    let root = tmp.path();
    git(root, &["checkout", "-q", "--orphan", "elsewhere"]);
    commit(root, "an unrelated root");
    let engine = started(root);
    let unrelated = engine.review(Some("main")).expect("review answers");
    assert!(!unrelated.measured, "{unrelated:?}");
    assert!(unrelated.merge_base.is_none());
    assert!(unrelated.not_measured_because.as_deref().unwrap().contains("no common ancestor"));
    assert!(cache_files(root).is_empty(), "no other commit was measured");
}

#[test]
fn a_deeper_offender_reads_worsened_a_shallower_one_improved_and_both_are_counted() {
    let tmp = rust_repo();
    let root = tmp.path();
    let shape = |alpha, gamma, delta| {
        format!("{}{}{}", nested("alpha_depth_four", alpha), nested("gamma_depth_six", gamma), nested("delta_depth_four", delta))
    };
    write(root, "src/deep.rs", &shape(4, 6, 4));
    commit(root, "two 4-deep functions and a 6-deep one");
    let engine = started(root);
    engine.scan(true).expect("scan records a snapshot at main");
    git(root, &["checkout", "-q", "-b", "reshape"]);
    // Two deeper and one shallower, all still over 4.
    write(root, "src/deep.rs", &shape(5, 5, 5));
    commit(root, "reshape");
    engine.index();

    let delta = review(&engine, "main");
    let alpha = entry(&delta, "nesting", "alpha_depth_four").expect("listed");
    assert_eq!((alpha.status, alpha.before, alpha.after), (ReviewStatus::Worsened, Some(4), Some(5)));
    let gamma = entry(&delta, "nesting", "gamma_depth_six").expect("listed");
    assert_eq!((gamma.status, gamma.before, gamma.after), (ReviewStatus::Improved, Some(6), Some(5)));
    for status in [ReviewStatus::Worsened, ReviewStatus::Improved] {
        let listed = delta.offenders.iter().filter(|e| e.status == status).count() as u64;
        let counted = match status {
            ReviewStatus::Worsened => delta.offender_counts.worsened,
            _ => delta.offender_counts.improved,
        };
        assert_eq!(counted, listed, "{status:?}: {:?}", delta.offender_counts);
    }
}

#[test]
fn a_before_state_that_cannot_be_cached_still_answers() {
    let tmp = rust_repo();
    let root = tmp.path();
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/deep.rs", &format!("{}{}", nested("alpha_depth_four", 4), nested("beta_depth_six", 6)));
    commit(root, "add a 6-deep function");
    let engine = started(root);
    // A file where the cache directory goes: nothing can be cached.
    write(root, ".logos/review-cache", "not a directory\n");

    let first = review(&engine, "main");
    assert_eq!(first.before_state_cache, BeforeStateCache::Built);
    assert_eq!(entry(&first, "nesting", "beta_depth_six").map(|e| e.status), Some(ReviewStatus::New));
    let second = review(&engine, "main");
    assert_eq!(second.before_state_cache, BeforeStateCache::Built, "nothing was kept, so it measures again");
    assert_eq!(serde_json::to_vec(&first).unwrap(), serde_json::to_vec(&second).unwrap());
}

/// A Rust repository whose four modules chain `a → b → c` (two cross-module
/// edges): too thin a module graph for Acyclicity and Depth to apply.
fn thin_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    write(root, "src/a/mod.rs", "pub fn alpha(x: i32) -> i32 {\n    crate::b::beta(x)\n}\n");
    write(root, "src/b/mod.rs", "pub fn beta(x: i32) -> i32 {\n    crate::c::gamma(x)\n}\n");
    write(root, "src/c/mod.rs", "pub fn gamma(x: i32) -> i32 {\n    x + 1\n}\n");
    write(root, "src/d/mod.rs", "pub fn delta(x: i32) -> i32 {\n    x * 2\n}\n");
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    tmp
}

/// [FR-QM-18] and S-523: a graph dimension that is not applicable is outside
/// the signal and the gate, so a branch that lengthens a module chain on a thin
/// graph lists no Depth (or Acyclicity) entry — under a recorded and a shadow
/// before-state alike — and the answer says why.
///
/// [FR-QM-18]: ../../docs/specs/requirements/FR-QM-18.md
#[test]
fn a_thin_graph_s_longer_module_chain_is_not_classified() {
    for recorded in [true, false] {
        let tmp = thin_repo();
        let root = tmp.path();
        let engine = started(root);
        if recorded {
            engine.scan(true).expect("scan records a snapshot at main");
        }
        git(root, &["checkout", "-q", "-b", "longer"]);
        write(root, "src/c/mod.rs", "pub fn gamma(x: i32) -> i32 {\n    crate::d::delta(x) + 1\n}\n");
        commit(root, "c calls d");
        engine.index();

        let delta = review(&engine, "main");
        let graph: Vec<_> = delta
            .offenders
            .iter()
            .filter(|e| e.dimension == "depth" || e.dimension == "acyclicity")
            .collect();
        assert!(graph.is_empty(), "recorded={recorded}: {graph:#?}");
        assert!(
            delta.notes.iter().any(|n| n.contains("acyclicity and depth offenders are not classified")),
            "recorded={recorded}: {:?}",
            delta.notes
        );
    }
}
