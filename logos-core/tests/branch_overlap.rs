//! Branch and merge symbol overlap (S-360 / [FR-NV-13], [FR-CL-04],
//! [NFR-CC-04], CR-114), exercised end-to-end through `Engine::branch_overlap`
//! against real git fixtures.
//!
//! Coverage by acceptance criterion:
//! - given N refs, the symbols more than one modifies are reported, naming the
//!   refs;
//! - symbols (and files) present in a ref but absent from a stated merge result
//!   are reported;
//! - replayed over the Sprint 63 five-branch roster shape, the two arms that
//!   never reached the roster are named;
//! - the payload states its limits, including that a symbol outside the indexed
//!   set cannot be reported.
//!
//! Every fixture is a throw-away temp repository. The refs are ordinary local
//! branches: nothing here fetches, and `branch_overlap` itself only ever reads
//! the object database ([NFR-SE-01]).
//!
//! [FR-NV-13]: ../../docs/specs/requirements/FR-NV-13.md
//! [FR-CL-04]: ../../docs/specs/requirements/FR-CL-04.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
//! [NFR-SE-01]: ../../docs/specs/requirements/NFR-SE-01.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::models::navigation::BranchOverlapResult;
use logos_core::Engine;
use tempfile::TempDir;

// ── git fixture helpers ──────────────────────────────────────────────────────

/// Run `git -C <cwd> <args…>` with a hermetic identity, asserting success.
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
            // A global `core.hooksPath` — which this project's own
            // `logos init --hooks` teaches users to set — makes a foreign
            // `pre-commit` veto the fixture's commits and fails the suite on
            // the developer's machine but not in CI. An empty value disables
            // hook lookup entirely.
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

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Stage everything and commit.
fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", message]);
}

/// The roster file, with `extra` appended to the one-line capability list.
///
/// One line is deliberate: it is the shape a hand-written roster actually has,
/// and the shape every contributing branch has to edit.
fn lib_rs(extra: &[&str]) -> String {
    let entries: Vec<String> = std::iter::once("\"rust\"".to_string())
        .chain(extra.iter().map(|name| format!("\"{name}\"")))
        .collect();
    format!(
        "pub fn language_roster() -> Vec<&'static str> {{\n    \
         vec![{}]\n}}\n\n\
         pub fn unrelated_helper() -> u32 {{\n    1\n}}\n\n\
         pub fn lonely_corner() -> u32 {{\n    2\n}}\n",
        entries.join(", ")
    )
}

/// A per-language capture arm, the file each Sprint 63 story added.
fn arm_rs(language: &str) -> String {
    format!("pub fn {language}_capture() -> u32 {{\n    0\n}}\n")
}

/// A repository whose `main` holds the roster and two unrelated functions.
fn base_repo() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, "src/lib.rs", &lib_rs(&[]));
    write(root, "src/arms.rs", &arm_rs("rust"));
    commit(root, "base");
    git(root, &["branch", "-M", "main"]);
    tmp
}

/// Branch off `main`, apply `changes`, commit, and return to `main`.
fn branch(root: &Path, name: &str, changes: &[(&str, String)]) {
    git(root, &["checkout", "-q", "-b", name, "main"]);
    for (rel, contents) in changes {
        write(root, rel, contents);
    }
    commit(root, name);
    git(root, &["checkout", "-q", "main"]);
}

/// An engine over the repository as it stands, indexed.
fn indexed_engine(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// The contended row for `name`, or `None`.
fn contended<'a>(
    result: &'a BranchOverlapResult,
    name: &str,
) -> Option<&'a logos_core::models::navigation::ContendedSymbol> {
    result.contended.iter().find(|row| row.symbol.name == name)
}

/// Refs, as the surfaces hand them over — verbatim strings, never interpreted.
fn refs(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

// ── The Sprint 63 replay ─────────────────────────────────────────────────────

/// Sprint 63 iteration 3, rebuilt: five branches each adding one capture arm,
/// three of which also append to the shared roster. The merge result carries
/// every arm file and a roster missing two of them.
///
/// This is what actually happened ([S-342], [S-344], [S-346], [S-347],
/// [S-348]): `python` and `php` landed their arms and never touched the roster,
/// nothing conflicted, every test stayed green, and the two entries were
/// missing until someone read the list by hand.
///
/// The `merged` branch is a plain commit whose *tree* is that outcome. Its
/// parentage is irrelevant to this query — the comparison is `base..merge` on
/// content — and building it directly keeps the fixture honest about the one
/// thing that matters: what the merge result contains.
///
/// [S-342]: ../../docs/planning/journal.md#s-342-kotlin-http-client-call-capture
/// [S-344]: ../../docs/planning/journal.md#s-344-python-http-client-call-capture
/// [S-346]: ../../docs/planning/journal.md#s-346-c-http-client-call-capture
/// [S-347]: ../../docs/planning/journal.md#s-347-ruby-http-client-call-capture
/// [S-348]: ../../docs/planning/journal.md#s-348-php-http-client-call-capture
fn sprint_63_repo() -> TempDir {
    let tmp = base_repo();
    let root = tmp.path();
    // Three arms that also joined the roster…
    for (name, entry) in [
        ("kotlin", "kotlin"),
        ("csharp", "c-sharp"),
        ("ruby", "ruby"),
    ] {
        branch(
            root,
            name,
            &[
                ("src/lib.rs", lib_rs(&[entry])),
                (
                    Box::leak(format!("src/{name}_arm.rs").into_boxed_str()),
                    arm_rs(name),
                ),
            ],
        );
    }
    // …and two that did not.
    for name in ["python", "php"] {
        branch(
            root,
            name,
            &[(
                Box::leak(format!("src/{name}_arm.rs").into_boxed_str()),
                arm_rs(name),
            )],
        );
    }

    git(root, &["checkout", "-q", "-b", "merged", "main"]);
    write(root, "src/lib.rs", &lib_rs(&["kotlin", "c-sharp", "ruby"]));
    for name in ["kotlin", "csharp", "ruby", "python", "php"] {
        write(root, &format!("src/{name}_arm.rs"), &arm_rs(name));
    }
    commit(root, "merge iteration 3");
    tmp
}

/// [FR-NV-13] AC 1 and AC 3, on the case the requirement was written for.
///
/// Three of five refs modify one roster function; the query names all three,
/// and names the two that did not — `python` and `php`, the entries the merge
/// dropped. That second list is the whole point: a shared append point some
/// siblings reached and others did not is what a silent drop looks like from
/// the outside, and it is visible *before* anyone reads the roster by hand.
#[test]
fn sprint_63_replay_names_the_two_arms_that_never_reached_the_roster() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(
        &refs(&["kotlin", "python", "csharp", "ruby", "php"]),
        None,
        Some("merged"),
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(result.base.is_some(), "the five branches share `main`");

    let roster = contended(&result, "language_roster").expect("the roster function is contended");
    assert_eq!(roster.modified_by, ["kotlin", "csharp", "ruby"]);
    assert_eq!(
        roster.absent_from,
        ["python", "php"],
        "the two arms that landed without joining the roster are named"
    );

    // Every ref resolved and every arm file was seen, so the answer is not
    // reporting a hole as a clean bill of health.
    assert_eq!(result.refs.len(), 5);
    assert!(result.refs.iter().all(|r| r.commit.is_some()));
    assert!(result.refs.iter().all(|r| r.files_changed >= 1));

    // And the merge half is silent, which is exactly why nothing caught this in
    // Sprint 63: by symbol, the merge is complete — the roster function *was*
    // changed, by the three refs that bothered. Only the participation gap
    // above distinguishes a complete merge from a clean one.
    let merge = result.merge.as_ref().expect("a merge result was stated");
    assert_eq!(merge.lost_symbols_total, 0, "{:?}", merge.lost_symbols);
    assert_eq!(merge.lost_files_total, 0, "{:?}", merge.lost_files);
    assert!(merge.lost_files.is_empty(), "{:?}", merge.lost_files);
    // That silence is a stated limit, not an omission: the merge check compares
    // WHICH symbols changed, and the payload says so rather than leaving an
    // empty `lost_symbols` to read as a clean bill of health ([NFR-CC-04]).
    assert!(
        result
            .coverage
            .statement
            .contains("compares WHICH symbols changed, not their content"),
        "{}",
        result.coverage.statement
    );
}

/// Determinism and the truncation guard's ordering rule ([NFR-RA-06]): the
/// most-contended symbol leads, so a bounded list can never drop a five-way
/// collision in favour of a two-way one.
#[test]
fn contended_symbols_are_ordered_most_contended_first() {
    let tmp = sprint_63_repo();
    let root = tmp.path();
    // A second contention, narrower than the roster's: two of the five refs
    // also touch `unrelated_helper`.
    for name in ["python", "php"] {
        git(root, &["checkout", "-q", name]);
        let mut source = lib_rs(&[]);
        source = source.replace("pub fn unrelated_helper() -> u32 {\n    1\n}", "pub fn unrelated_helper() -> u32 {\n    11\n}");
        write(root, "src/lib.rs", &source);
        commit(root, "touch the helper");
    }
    git(root, &["checkout", "-q", "merged"]);
    let engine = indexed_engine(root);

    let result = engine.branch_overlap(
        &refs(&["kotlin", "python", "csharp", "ruby", "php"]),
        None,
        None,
    );
    let names: Vec<&str> = result
        .contended
        .iter()
        .map(|row| row.symbol.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["language_roster", "unrelated_helper"],
        "three-way contention precedes two-way: {result:?}"
    );
    assert_eq!(result.contended_total, 2);
    assert_eq!(result.contended_elided, 0);
}

// ── The silent-drop half ([FR-NV-13] AC 2) ───────────────────────────────────

/// A repository where one branch's work genuinely did not reach the merge.
///
/// `wanted` edits `lonely_corner` and adds a file of its own; `landed` edits
/// `unrelated_helper`. The `partial` branch carries only `landed`'s work — the
/// integration everyone believed was complete.
fn dropped_work_repo() -> TempDir {
    let tmp = base_repo();
    let root = tmp.path();
    branch(
        root,
        "wanted",
        &[
            (
                "src/lib.rs",
                lib_rs(&[]).replace("    2\n", "    22\n"),
            ),
            // A symbol in a file the merge result never touches at all — the
            // `merge_changed_the_file: false` case, so one answer carries both
            // kinds of loss and the ordering rule between them is observable.
            ("src/arms.rs", "pub fn rust_capture() -> u32 {\n    9\n}\n".to_string()),
            ("src/wanted_only.rs", arm_rs("wanted")),
        ],
    );
    branch(
        root,
        "landed",
        &[(
            "src/lib.rs",
            lib_rs(&[]).replace("    1\n", "    11\n"),
        )],
    );
    git(root, &["checkout", "-q", "-b", "partial", "landed"]);
    tmp
}

/// [FR-NV-13] AC 2: a symbol a ref modified that the merge result does not
/// change is reported, and so is a file the merge result never took.
#[test]
fn work_a_ref_did_and_the_merge_did_not_is_reported_lost() {
    let tmp = dropped_work_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["wanted", "landed"]), None, Some("partial"));
    let merge = result.merge.as_ref().expect("a merge result was stated");

    // Both kinds of loss, and the ordering rule between them: the stronger
    // signal — the merge took part of that file and not this symbol — leads, so
    // truncation can never drop it for a whole-file omission.
    let lost: Vec<(&str, bool)> = merge
        .lost_symbols
        .iter()
        .map(|row| (row.symbol.name.as_str(), row.merge_changed_the_file))
        .collect();
    // `src/arms.rs` holds a single function whose span is the whole file, so the
    // file's module node ties with it and both are reported — the documented
    // equal-span case, pinned here rather than papered over.
    assert_eq!(
        lost,
        [
            ("lonely_corner", true),
            ("arms", false),
            ("rust_capture", false)
        ],
        "{merge:?}"
    );
    assert!(merge
        .lost_symbols
        .iter()
        .all(|row| row.modified_by == ["wanted"]));
    assert_eq!(merge.lost_symbols_total, 3);
    assert_eq!(merge.lost_symbols_elided, 0);

    // `landed`'s own change is in the merge, so it is not reported lost.
    assert!(!lost.iter().any(|(name, _)| *name == "unrelated_helper"));

    // The coarse twin catches the whole file the merge never took — the only
    // report that can speak for content the index holds no symbol for.
    let lost_files: Vec<&str> = merge
        .lost_files
        .iter()
        .map(|row| row.path.as_str())
        .collect();
    assert_eq!(lost_files, ["src/arms.rs", "src/wanted_only.rs"]);
    assert!(merge
        .lost_files
        .iter()
        .all(|row| row.modified_by == ["wanted"]));
    assert_eq!(merge.lost_files_total, 2);
    assert_eq!(merge.lost_files_elided, 0);
}

/// The negative: refs that touch nothing in common report no contention at all,
/// and without a stated merge result the payload carries no merge block rather
/// than an empty one that reads like a clean bill of health.
#[test]
fn refs_with_no_shared_symbol_report_no_contention_and_no_merge_block() {
    let tmp = dropped_work_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["wanted", "landed"]), None, None);
    assert!(result.contended.is_empty(), "{:?}", result.contended);
    assert_eq!(result.contended_total, 0);
    assert!(result.merge.is_none(), "no merge was stated");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    // The pipeline demonstrably ran: `wanted` touched real symbols. Without a
    // positive companion, "no contention" would also be what a broken query
    // returns.
    assert!(result.refs[0].symbols_modified >= 1, "{:?}", result.refs);
    assert_eq!(result.refs[1].symbols_modified, 1);
}

/// A ref that deletes a file has modified every symbol the file held, so it
/// contends with a ref that edits one of them. Without the whole-file rule a
/// deletion has no new-side lines and would silently contend with nothing —
/// the most destructive collision of all, invisible.
#[test]
fn a_ref_that_deletes_a_file_contends_with_one_that_edits_it() {
    let tmp = base_repo();
    let root = tmp.path();
    git(root, &["checkout", "-q", "-b", "remover", "main"]);
    fs::remove_file(root.join("src/arms.rs")).unwrap();
    commit(root, "drop the rust arm");
    git(root, &["checkout", "-q", "main"]);
    branch(
        root,
        "editor",
        &[(
            "src/arms.rs",
            "pub fn rust_capture() -> u32 {\n    7\n}\n".to_string(),
        )],
    );
    let engine = indexed_engine(root);

    let result = engine.branch_overlap(&refs(&["remover", "editor"]), None, None);
    let row = contended(&result, "rust_capture").expect("the deleted symbol is contended");
    assert_eq!(row.modified_by, ["remover", "editor"]);
    assert!(row.absent_from.is_empty(), "both refs touch it");
}

// ── Coverage limits ([FR-NV-13] AC 4, [NFR-CC-04]) ───────────────────────────

/// The payload states its limits, and each claim in the statement is backed by
/// counted evidence rather than left as a disclaimer.
#[test]
fn the_payload_states_its_limits_including_the_unindexed_symbol_bound() {
    let tmp = base_repo();
    let root = tmp.path();
    branch(
        root,
        "one",
        &[
            ("src/lib.rs", lib_rs(&["one"])),
            ("assets/notes.txt", "a file the graph holds no symbol for\n".to_string()),
        ],
    );
    branch(root, "two", &[("src/lib.rs", lib_rs(&["two"]))]);
    let engine = indexed_engine(root);

    let result = engine.branch_overlap(&refs(&["one", "two"]), None, None);
    let coverage = &result.coverage;
    assert!(
        coverage
            .statement
            .contains("A symbol outside the indexed set cannot be reported"),
        "the AC's own limit must be stated verbatim: {}",
        coverage.statement
    );
    assert!(coverage.statement.contains("smell rather than a proof"));
    assert!(
        coverage.indexed_snapshot.is_some(),
        "the snapshot the spans came from is named"
    );
    assert!(
        coverage
            .files_without_indexed_symbols
            .contains(&"assets/notes.txt".to_string()),
        "a changed file the index holds nothing for is named: {coverage:?}"
    );
    // Both refs differ from the indexed working tree (`main`), so the spans used
    // to attribute their hunks are disclosed as possibly moved. Asserted as an
    // exact set: a `contains` would still hold if the narrowing to the changed
    // files were dropped and the list grew to everything differing from the
    // snapshot.
    assert_eq!(
        coverage.files_with_drifted_spans,
        ["assets/notes.txt", "src/lib.rs"],
        "{coverage:?}"
    );
    assert!(coverage.unresolved_refs.is_empty());
    assert!(coverage.refs_not_diffed.is_empty());
    assert_eq!(
        coverage.unattributed_hunks, 0,
        "every hunk in this fixture lands inside a symbol"
    );
    assert_eq!(
        result.base_origin,
        "the common ancestor (git merge-base) of the supplied refs"
    );
}

/// The bounds are real, and their arithmetic is reported.
///
/// Every other assertion on `contended_total`/`contended_elided` in this file is
/// against a two-row answer, where the counters hold trivially. A regression
/// that computed the total *after* truncating — reporting 100 of 100 with
/// nothing elided — would pass all of them.
#[test]
fn a_contended_set_beyond_the_cap_is_truncated_with_the_elision_counted() {
    let tmp = base_repo();
    let root = tmp.path();
    let wide = |suffix: &str| {
        (0..120)
            .map(|n| format!("pub fn f{n:03}() -> u32 {{\n    {n}{suffix}\n}}\n"))
            .collect::<String>()
    };
    write(root, "src/wide.rs", &wide(""));
    commit(root, "wide");
    for name in ["left", "right"] {
        branch(root, name, &[("src/wide.rs", wide("1"))]);
    }
    // A third ref touches only the first function, making it the sole 3-way
    // collision — so the ordering rule is what decides which rows survive.
    let mut only_first = wide("");
    only_first = only_first.replacen("    0\n", "    7\n", 1);
    branch(root, "third", &[("src/wide.rs", only_first)]);
    let engine = indexed_engine(root);

    let result = engine.branch_overlap(&refs(&["left", "right", "third"]), None, None);
    // 120, not 121: the file's module node encloses every function, so the
    // innermost rule keeps the functions and drops it — the nesting case the
    // unit test pins, observed here at scale.
    assert_eq!(result.contended_total, 120);
    assert_eq!(result.contended.len(), 100, "capped at MAX_SYMBOLS_LISTED");
    assert_eq!(result.contended_elided, 20);
    assert_eq!(
        result.contended[0].modified_by,
        ["left", "right", "third"],
        "the only three-way collision leads, so truncation cannot drop it: {:?}",
        result.contended[0].symbol
    );
    assert!(
        result.contended[1..]
            .iter()
            .all(|row| row.modified_by == ["left", "right"]),
        "every other row is two-way"
    );
}

/// The three `base_origin` outcomes the happy path does not produce. These are
/// the words the payload uses to explain its comparison point, and a caller
/// acting on the wrong one re-runs the same broken command.
#[test]
fn every_way_the_base_can_fail_says_which_one_happened() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    // (1) A stated base that does not resolve names itself — and must NOT
    // advise passing `--base`, which the caller already did.
    let result = engine.branch_overlap(&refs(&["kotlin", "ruby"]), Some("no-such-base"), None);
    assert_eq!(
        result.base_origin,
        "no-such-base was stated as the base but does not resolve"
    );
    assert!(result.base.is_none());
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("no-such-base") && w.contains("stated as the base")),
        "{:?}",
        result.warnings
    );
    assert!(
        !result.warnings.iter().any(|w| w.contains("pass --base")),
        "the caller passed --base; that advice is a loop: {:?}",
        result.warnings
    );

    // (2) Unrelated histories have no comparison point at all.
    let root = tmp.path();
    git(root, &["checkout", "-q", "--orphan", "island"]);
    git(root, &["rm", "-rq", "--cached", "."]);
    write(root, "src/island.rs", "pub fn island() {}\n");
    commit(root, "island");
    git(root, &["checkout", "-q", "merged"]);
    let result = engine.branch_overlap(&refs(&["kotlin", "island"]), None, None);
    assert_eq!(result.base_origin, "the supplied refs have no common ancestor");
    assert!(
        result.warnings.iter().any(|w| w.contains("no common ancestor")),
        "{:?}",
        result.warnings
    );

    // (3) An empty optional is an ABSENT optional, on every surface — so this
    // falls back to the merge-base rather than collapsing the whole answer.
    let result = engine.branch_overlap(&refs(&["kotlin", "ruby"]), Some(""), Some("  "));
    assert_eq!(
        result.base_origin,
        "the common ancestor (git merge-base) of the supplied refs"
    );
    assert!(result.base.is_some());
    assert!(result.merge.is_none(), "a blank --merge states no merge");
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}

/// The two questions that cannot be asked from the CLI (clap requires `--ref`)
/// but reach the engine through the web route and MCP.
#[test]
fn no_refs_and_an_unresolvable_merge_are_honest_warnings() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&[], None, None);
    assert!(result.refs.is_empty());
    assert!(result.merge.is_none());
    assert!(
        result.warnings.iter().any(|w| w.contains("no refs supplied")),
        "{:?}",
        result.warnings
    );

    // A merge result that is not a commit leaves `merge: null`, which a
    // consumer could read as "no losses" — so it must never be silent.
    let result = engine.branch_overlap(&refs(&["kotlin", "ruby"]), None, Some("no-such-merge"));
    assert!(result.merge.is_none());
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("no-such-merge") && w.contains("does not resolve")),
        "{:?}",
        result.warnings
    );
}

/// The two coverage counters that only ever read as their degenerate value
/// elsewhere: a hunk past everything the index knows, and a ref that IS the
/// indexed snapshot and therefore drifts from nothing.
///
/// This is also the AC's "a symbol a ref adds that never reached the index is
/// invisible" clause, demonstrated by a counter rather than asserted as prose.
#[test]
fn a_change_the_index_cannot_see_is_counted_not_dropped() {
    let tmp = base_repo();
    let root = tmp.path();
    // `grown` appends a whole new function below everything `main` holds.
    branch(
        root,
        "grown",
        &[(
            "src/lib.rs",
            format!("{}\npub fn brand_new() -> u32 {{\n    3\n}}\n", lib_rs(&[])),
        )],
    );
    branch(root, "quiet", &[("src/arms.rs", arm_rs("quiet"))]);
    let engine = indexed_engine(root);

    let result = engine.branch_overlap(&refs(&["grown", "quiet"]), None, None);
    assert!(
        result.coverage.unattributed_hunks >= 1,
        "the appended function is past every indexed span: {:?}",
        result.coverage
    );
    assert!(
        !result
            .contended
            .iter()
            .any(|row| row.symbol.name == "brand_new"),
        "a symbol outside the indexed set cannot be reported — the AC's own limit"
    );

    // The indexed working tree IS `main`, and `main` is neither ref, so both
    // drift. Compare with a ref that is the snapshot: it contributes nothing.
    let result = engine.branch_overlap(&refs(&["main", "quiet"]), None, None);
    assert!(
        !result
            .coverage
            .files_with_drifted_spans
            .contains(&"src/lib.rs".to_string()),
        "`main` is the snapshot, so nothing it changed can have drifted: {:?}",
        result.coverage
    );
}

/// A stated base wins over the computed merge-base: saying "compare against
/// this" is how a caller pins the comparison point, and second-guessing it
/// would make the answer depend on repository shape.
#[test]
fn a_stated_base_overrides_the_merge_base() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["kotlin", "ruby"]), Some("merged"), None);
    assert_eq!(result.base_origin, "stated by the caller as merged");
    assert!(result.base.is_some());
    // Against the merge result the two refs read as *removing* the entries the
    // others added, so they still contend on the roster — the comparison point
    // changes the story, which is why it is reported.
    assert!(contended(&result, "language_roster").is_some(), "{result:?}");
}

// ── Infallibility at the surface ([ADR-14]) ──────────────────────────────────

/// An unresolvable ref is a warning on an otherwise honest payload, never an
/// error — and it is listed in `coverage.unresolved_refs`, because a ref that
/// contributed nothing must not make the remaining refs look independent.
#[test]
fn an_unresolvable_ref_is_a_warning_not_an_error() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["kotlin", "no-such-branch", "ruby"]), None, None);
    assert_eq!(result.coverage.unresolved_refs, ["no-such-branch"]);
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("no-such-branch") && w.contains("does not resolve")),
        "{:?}",
        result.warnings
    );
    assert!(result.refs[1].commit.is_none());
    // The two live refs are still compared, and the dead one is not silently
    // counted as "absent from" anything it could never have modified.
    let roster = contended(&result, "language_roster").expect("kotlin and ruby still contend");
    assert_eq!(roster.modified_by, ["kotlin", "ruby"]);
    assert!(roster.absent_from.is_empty());
}

/// Fewer than two refs and no merge result cannot answer anything; the payload
/// says so instead of returning an empty result that reads as "no collisions".
#[test]
fn fewer_than_two_refs_without_a_merge_is_an_honest_warning() {
    let tmp = sprint_63_repo();
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["kotlin"]), None, None);
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("at least two refs")),
        "{:?}",
        result.warnings
    );
    assert!(result.contended.is_empty());

    // One ref *plus* a merge result is a complete question, so it must not warn.
    let result = engine.branch_overlap(&refs(&["kotlin"]), None, Some("merged"));
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(result.merge.is_some());
}

/// A project that is not a git repository degrades to an empty answer carrying
/// its warnings and its coverage statement — never an error, and never a bare
/// "no collisions" that a caller could mistake for a clean verdict.
#[test]
fn a_project_without_git_degrades_with_warnings_and_still_states_its_limits() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", &lib_rs(&[]));
    let engine = indexed_engine(tmp.path());

    let result = engine.branch_overlap(&refs(&["main", "topic"]), None, None);
    assert_eq!(result.coverage.unresolved_refs, ["main", "topic"]);
    assert!(result.base.is_none());
    assert!(result.contended.is_empty());
    assert!(
        result.coverage.statement.contains("indexed set"),
        "a degraded answer is the one that most needs its limits stated"
    );
    assert!(result.warnings.len() >= 2, "{:?}", result.warnings);
}
