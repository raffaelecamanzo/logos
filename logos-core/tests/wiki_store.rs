//! Integration tests for the source-wiki **store** (S-052, CR-008, ADR-24)
//! against a real `Engine`, a real git tree, and a real `index`:
//!
//! - rebuild-survival: a full `index` (which wipes `logos.db`) leaves every page
//!   readable, and `wiki.db`'s migration version is independent ([FR-WK-01]);
//! - read-time freshness on disk: editing an anchored file flips it stale at the
//!   next read with **no** `sync`, and `sync` never writes `wiki.db` ([FR-WK-03]);
//! - the orphan lifecycle: a single-anchor page whose file is renamed is pruned
//!   and logged; a two-anchor page survives flagged ([FR-WK-07]);
//! - the symbol-anchor resolver path against a real graph ([FR-WK-02]);
//! - gate-immunity: the gate is byte-identical with the wiki absent, written,
//!   stale, or deleted ([UAT-WK-02], [BR-29]).
//!
//! The store-contract unit tests (write rejection, freshness verdicts, prune
//! lifecycle with a fake resolver) live in `src/wiki/tests.rs`. The native
//! (extracted) tier's render unit tests live in `src/wiki/native/tests.rs`; the
//! end-to-end native render + memoization + built-at-revision fixtures (real
//! `Engine`, real `index`, real graph revision) live here.

use std::path::Path;
use std::process::Command;

use logos_core::Engine;
use tempfile::TempDir;

// ── git fixture helpers (mirroring tests/coverage_surface.rs conventions) ────

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A branchy function body (decision points → a non-trivial per-function CC), so
/// the gated structural signal is well-defined and an edit can change the tree.
fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

fn anchors(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

/// A minimal well-formed body — a heading plus enough prose — for tests whose
/// focus is not the page content itself, so a plain write clears the
/// [FR-WK-19] content-validity guard without the test needing to care.
const PAGE: &str = "# Test Page\n\nPlaceholder prose long enough to satisfy the write-path content-validity guard.";

/// `PRAGMA user_version` of a store file — used to show `wiki.db`'s migration
/// track is independent of `logos.db`'s ([FR-WK-01]).
fn user_version(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("open store");
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("read user_version")
}

// ── FR-WK-01: rebuild-survival + independent migration track ─────────────────

#[test]
fn pages_survive_a_full_index_rebuild_on_an_independent_migration_track() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    engine
        .wiki_write(
            "guide/a",
            "About a",
            "# About a\n\nThe a module does things, described at enough length to clear the guard.",
            &anchors(&["file:src/a.rs"]),
            "claude-opus",
        )
        .expect("write succeeds");

    let wiki_db = repo.join(".logos/wiki.db");
    let logos_db = repo.join(".logos/logos.db");
    // Independence is structural, not a numeric coincidence: wiki.db reports the
    // latest version on ITS OWN track, and logos.db carries its own (non-zero)
    // version on a separate track ([FR-WK-01]). We assert each store owns its
    // version rather than that the two integers merely differ.
    assert_eq!(
        user_version(&wiki_db),
        logos_core::wiki::latest_version(),
        "wiki.db is fully migrated on its own track"
    );
    let logos_v = user_version(&logos_db);
    assert!(
        logos_v >= 1,
        "logos.db carries its own migration version on a separate track"
    );

    // A full index rebuilds logos.db wholesale; wiki.db must be untouched — the
    // page survives and wiki.db's own version is unchanged by the rebuild.
    engine.index();
    let page = engine
        .wiki_read("guide/a")
        .expect("read ok")
        .expect("the page survived the index rebuild");
    assert_eq!(
        page.body,
        "# About a\n\nThe a module does things, described at enough length to clear the guard."
    );
    assert_eq!(
        user_version(&wiki_db),
        logos_core::wiki::latest_version(),
        "the rebuild did not touch wiki.db's version"
    );
}

#[test]
fn wiki_read_on_a_fresh_store_is_an_exit_zero_miss() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 1), "add a");
    let engine = Engine::start(repo).expect("engine starts");

    // No page was ever written → a miss, never an error (the empty-store posture).
    assert!(engine.wiki_read("nothing/here").expect("ok").is_none());
}

// ── FR-WK-03 / FR-WK-07: read-time freshness + orphan lifecycle on disk ──────

#[test]
fn editing_an_anchored_file_flips_it_stale_with_no_sync_and_sync_never_writes_wiki_db() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine
        .wiki_write(
            "guide/a",
            "About a",
            PAGE,
            &anchors(&["file:src/a.rs"]),
            "gen",
        )
        .expect("write");
    assert!(
        !engine.wiki_read("guide/a").unwrap().unwrap().stale,
        "fresh right after write"
    );

    // Edit the anchored file on disk — no sync, no index.
    std::fs::write(repo.join("src/a.rs"), branchy("a", 3)).unwrap();
    let page = engine.wiki_read("guide/a").unwrap().unwrap();
    assert!(
        page.stale,
        "editing the anchored file flips it stale at the next read, no sync needed (FR-WK-03)"
    );

    // sync must never write wiki.db (FR-WK-03): its bytes are unchanged across a sync.
    let wiki_db = repo.join(".logos/wiki.db");
    let before = std::fs::read(&wiki_db).unwrap();
    engine.sync(&[]);
    let after = std::fs::read(&wiki_db).unwrap();
    assert_eq!(before, after, "sync left wiki.db byte-identical");
}

#[test]
fn single_anchor_page_prunes_on_rename_and_two_anchor_page_survives_flagged() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine
        .wiki_write("page/a", "A", PAGE, &anchors(&["file:src/a.rs"]), "gen")
        .expect("single-anchor write");
    engine
        .wiki_write(
            "page/ab",
            "AB",
            PAGE,
            &anchors(&["file:src/a.rs", "file:src/b.rs"]),
            "gen",
        )
        .expect("two-anchor write");

    // Rename a.rs away (a rename is delete+add at the graph/tree level).
    sh_git(repo, &["mv", "src/a.rs", "src/c.rs"]);

    // The single-anchor page is pruned and logged.
    assert!(
        engine.wiki_read("page/a").unwrap().is_none(),
        "the single-anchor page is pruned once its only anchor is gone"
    );
    let log = engine.wiki_pruned_log().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].slug, "page/a");

    // The two-anchor page survives with a.rs flagged missing, b.rs still fresh.
    let ab = engine.wiki_read("page/ab").unwrap().expect("survives");
    assert!(ab.has_missing, "the renamed-away anchor is flagged missing");
    assert_eq!(ab.anchors[0].freshness.as_str(), "missing");
    assert_eq!(ab.anchors[1].freshness.as_str(), "fresh");
}

// ── FR-WK-02: the symbol-anchor resolver path against a real graph ───────────

#[test]
fn a_symbol_anchor_resolves_against_the_graph_and_flips_stale_when_its_file_changes() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/lib.rs", &branchy("widget", 2), "add widget");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Resolve the canonical symbol of `widget` from the graph.
    let hits = engine.search("widget", None, Some(5));
    let symbol = hits
        .hits
        .iter()
        .find(|h| h.name == "widget")
        .map(|h| h.symbol.clone())
        .expect("widget is indexed");

    engine
        .wiki_write(
            "sym/widget",
            "Widget",
            PAGE,
            &anchors(&[&format!("symbol:{symbol}")]),
            "gen",
        )
        .expect("symbol-anchored write resolves against the graph");
    assert!(!engine.wiki_read("sym/widget").unwrap().unwrap().stale);

    // Editing the symbol's defining file flips the symbol anchor stale.
    std::fs::write(repo.join("src/lib.rs"), branchy("widget", 4)).unwrap();
    assert!(
        engine.wiki_read("sym/widget").unwrap().unwrap().stale,
        "a symbol anchor goes stale when its defining file changes (FR-WK-03)"
    );
}

#[test]
fn a_write_naming_an_unknown_symbol_is_rejected_with_the_store_byte_identical() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 1), "add a");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let err = engine
        .wiki_write(
            "p",
            "T",
            "b",
            &anchors(&["symbol:scip-no-such-symbol#"]),
            "gen",
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown anchor"), "got: {err}");
    assert!(
        engine.wiki_read("p").unwrap().is_none(),
        "the rejected write left no page"
    );
}

// ── UAT-WK-02 / BR-29: gate immunity across all wiki states ──────────────────

/// The gated verdict, stripped of provenance that legitimately tracks the commit
/// — what BR-29 pins as a pure function of tree + config (mirrors the coverage
/// and history gate-immunity tests).
fn gated_verdict(g: &logos_core::models::quality::GateResult) -> String {
    serde_json::to_string(&serde_json::json!({
        "passed": g.passed,
        "signal": g.signal,
        "baseline_signal": g.baseline_signal,
        "regressions": g.regressions,
        "test_function_count": g.test_function_count,
    }))
    .unwrap()
}

#[test]
fn gate_is_byte_identical_across_wiki_absent_written_stale_and_deleted() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-b", "main", "-q"]);
    commit(
        repo,
        "src/covered.rs",
        &branchy("covered", 4),
        "add covered",
    );
    commit(repo, "src/other.rs", &branchy("other", 2), "add other");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // (0) Baseline on the pristine indexed tree — wiki absent.
    engine.gate(None, true, true).expect("gate --save");
    let baseline = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert!(
        !repo.join(".logos/wiki.db").exists(),
        "no wiki store exists yet"
    );
    // UAT-WK-02 postcondition: logos.db's schema version is unchanged throughout,
    // no matter what the wiki tier does.
    let logos_db = repo.join(".logos/logos.db");
    let logos_schema_v = user_version(&logos_db);

    // (1) Write pages (anchored + zero-anchor) on the SAME tree → identical gate.
    engine
        .wiki_write(
            "guide/covered",
            "Covered",
            PAGE,
            &anchors(&["file:src/covered.rs"]),
            "gen",
        )
        .expect("anchored write");
    engine
        .wiki_write("guide/overview", "Overview", PAGE, &[], "gen")
        .expect("zero-anchor write");
    assert!(
        repo.join(".logos/wiki.db").exists(),
        "the wiki store now exists"
    );
    let after_written = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        baseline, after_written,
        "writing wiki pages never moves the gate (BR-29)"
    );
    assert_eq!(
        user_version(&logos_db),
        logos_schema_v,
        "writing wiki pages never changes logos.db's schema version (UAT-WK-02)"
    );

    // (2) Edit the anchored file → the page goes stale AND the tree changes. On
    // that edited tree, the gate WITH the (stale) wiki and the gate AFTER deleting
    // wiki.db must be identical — i.e. the wiki tier never enters the gate.
    std::fs::write(repo.join("src/covered.rs"), branchy("covered", 5)).unwrap();
    assert!(
        engine.wiki_read("guide/covered").unwrap().unwrap().stale,
        "editing the covered file made its wiki page stale"
    );
    let with_stale = gated_verdict(&engine.gate(None, false, true).expect("gate"));

    std::fs::remove_file(repo.join(".logos/wiki.db")).expect("delete wiki.db");
    let without_wiki = gated_verdict(&engine.gate(None, false, true).expect("gate"));
    assert_eq!(
        with_stale, without_wiki,
        "stale wiki vs no wiki on the same tree → identical gate (BR-29, UAT-WK-02)"
    );
    assert_eq!(
        user_version(&logos_db),
        logos_schema_v,
        "no wiki state (written/stale/deleted) ever changed logos.db's schema version (UAT-WK-02)"
    );
}

// ── FR-WK-10 / FR-SY-09 / ADR-32: the native (extracted) tier end-to-end ─────

#[test]
fn native_renders_three_sections_with_the_revision_label_over_a_real_graph() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "core/src/lib.rs", &branchy("run", 2), "add core");
    commit(repo, "web/src/main.rs", &branchy("serve", 1), "add web");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let native = engine.wiki_native().expect("native render");

    // The label carries the SAME revision the status read-model reports — the
    // single persisted graph revision both surfaces read ([FR-SY-09]).
    let revision = engine.status().graph_revision;
    assert_eq!(native.revision, revision);
    assert_eq!(
        native.label,
        format!("extracted — live from graph @revision {revision}"),
    );

    // All three native sections render from the graph.
    assert!(!native.structure.is_empty(), "codebase structure renders");
    assert!(!native.files.is_empty(), "files view renders");
    assert!(
        native.dependency_mermaid.starts_with("graph LR\n"),
        "dependency Mermaid renders"
    );
    let listed: Vec<&str> = native.files.iter().map(|e| e.path.as_str()).collect();
    assert!(listed.contains(&"core/src/lib.rs"));
    assert!(listed.contains(&"web/src/main.rs"));
}

#[test]
fn native_render_creates_no_wiki_db_and_is_byte_identical_at_a_fixed_revision() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // The native tier has no store: rendering it must NOT create wiki.db, and the
    // memoized cache hit returns a byte-identical value ([ADR-32], [NFR-RA-06]).
    let first = engine.wiki_native().expect("first render");
    let second = engine.wiki_native().expect("second render (cache hit)");
    assert_eq!(first, second, "byte-identical at a fixed revision");
    assert!(
        !repo.join(".logos/wiki.db").exists(),
        "rendering the native tier writes no wiki.db (ADR-32)"
    );
}

#[test]
fn native_re_renders_after_an_index_advances_the_revision() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    let before = engine.wiki_native().expect("render before");
    let rev_before = engine.status().graph_revision;
    assert_eq!(before.revision, rev_before);
    assert!(
        !before.files.iter().any(|e| e.path == "src/b.rs"),
        "src/b.rs is not in the graph yet"
    );

    // A new file + a fresh index advances the persisted revision; the next native
    // render is a cache miss that reflects the new graph ([FR-SY-09], [ADR-32]).
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");
    engine.index();
    let after = engine.wiki_native().expect("render after");
    let rev_after = engine.status().graph_revision;
    assert!(rev_after > rev_before, "the revision strictly advanced");
    assert_eq!(after.revision, rev_after);
    assert!(
        after.files.iter().any(|e| e.path == "src/b.rs"),
        "the re-render reflects the new file"
    );
}

#[test]
fn native_honors_graph_exclusions_a_gitignored_file_is_absent() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    // A gitignored file is never indexed → it must appear in no native section
    // (exclusion parity via the admitted set, [FR-WK-10]). It lives on disk but is
    // excluded by discovery, so it is written directly (a `git add` would refuse
    // an ignored path anyway).
    commit(repo, ".gitignore", "vendor/\n", "ignore vendor");
    let vendored = repo.join("vendor/skip.rs");
    std::fs::create_dir_all(vendored.parent().unwrap()).unwrap();
    std::fs::write(&vendored, branchy("skip", 1)).unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    let native = engine.wiki_native().expect("native render");

    assert!(
        !native.files.iter().any(|e| e.path.starts_with("vendor/")),
        "a gitignored file is absent from the files view"
    );
    assert!(
        !native.dependency_mermaid.contains("vendor"),
        "an excluded crate is absent from the dependency diagram"
    );
    // AC-2 is "any native section": the structure tree must be clear of the
    // excluded path too.
    assert!(
        !native.structure.iter().any(|n| n.path.starts_with("vendor")),
        "an excluded crate is absent from the codebase structure"
    );
    // And the admitted file is present — the tier is not simply empty.
    assert!(native.files.iter().any(|e| e.path == "src/a.rs"));
}

#[test]
fn native_before_the_first_index_is_an_empty_revision_zero_render_with_no_wiki_db() {
    // FR-WK-12 AC-3 / ADR-32 §4 first-availability: before any `index` the native
    // tier still renders — an honest empty, revision-0 state — and writes no
    // wiki.db; `init` is never blocked by wiki work.
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    // Deliberately NO engine.index() here.
    let native = engine.wiki_native().expect("native render before any index");
    assert_eq!(native.revision, 0, "no index yet → revision 0");
    assert_eq!(native.label, "extracted — live from graph @revision 0");
    assert!(native.structure.is_empty(), "no graph yet → empty structure");
    assert!(native.files.is_empty());
    assert_eq!(native.dependency_mermaid, "graph LR\n");
    assert!(
        !repo.join(".logos/wiki.db").exists(),
        "rendering the native tier before any index creates no wiki.db"
    );
}

// ── FR-WK-12: each agent page records its built-at graph revision at write ───

#[test]
fn an_agent_page_records_its_built_at_revision_and_a_rewrite_updates_it() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    engine
        .wiki_write("guide/a", "A", PAGE, &anchors(&["file:src/a.rs"]), "gen")
        .expect("write");
    let rev_at_write = engine.status().graph_revision;
    let page = engine.wiki_read("guide/a").unwrap().unwrap();
    assert_eq!(
        page.built_at_revision, rev_at_write,
        "the page records the graph revision it was built at (FR-WK-12)"
    );

    // A fresh index advances the revision; re-writing the page records the new
    // built-at revision (last-write-wins), so the freshness comparator the view
    // derives sees the up-to-date value.
    std::fs::write(repo.join("src/a.rs"), branchy("a", 3)).unwrap();
    engine.index();
    let rev_after = engine.status().graph_revision;
    assert!(rev_after > rev_at_write, "the revision advanced after re-index");
    engine
        .wiki_write("guide/a", "A", PAGE, &anchors(&["file:src/a.rs"]), "gen")
        .expect("re-write");
    let page = engine.wiki_read("guide/a").unwrap().unwrap();
    assert_eq!(
        page.built_at_revision, rev_after,
        "the re-write captures the advanced revision"
    );
}

// ── FR-WK-05: search over a real graph + survives a full index rebuild ────────

#[test]
fn search_finds_a_page_and_survives_a_full_index_rebuild() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine
        .wiki_write(
            "guide/a",
            "About a",
            "# About a\n\nthe quux subsystem orchestrates widgets across modules",
            &anchors(&["file:src/a.rs"]),
            "gen",
        )
        .expect("write");

    // A phrase unique to the page's body returns that page, staleness-flagged.
    let hits = engine.wiki_search("quux subsystem", false).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].slug, "guide/a");
    assert!(!hits[0].stale);

    // A full index rebuilds logos.db wholesale; the FTS index lives in wiki.db,
    // so search still finds the page afterwards (FR-WK-05 / FR-WK-01).
    engine.index();
    let hits = engine.wiki_search("quux subsystem", false).expect("search");
    assert_eq!(hits.len(), 1, "search survives the index rebuild");
    assert_eq!(hits[0].slug, "guide/a");

    // List mode enumerates every page with provenance, slug-ordered.
    let all = engine.wiki_search("", true).expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].generator, "gen");
}

// ── FR-WK-06: the regeneration work-list over a real graph (UAT-WK-03) ────────

#[test]
fn status_work_list_excludes_file_and_module_entities_over_a_real_graph() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    // guide/a covers src/a.rs by file anchor; overview is zero-anchor. src/b.rs
    // has no page, but File is no longer page-worthy ([CR-056]/[S-221]).
    engine
        .wiki_write("guide/a", "A", PAGE, &anchors(&["file:src/a.rs"]), "gen")
        .expect("write a");
    engine
        .wiki_write("overview", "Overview", PAGE, &[], "gen")
        .expect("write overview");

    let st = engine.wiki_status().expect("status");
    assert_eq!(st.page_count, 2);
    assert_eq!(st.stale_count, 0);
    assert!((st.freshness_fraction - 1.0).abs() < f64::EPSILON);

    // Neither the unanchored file nor the per-file synthetic module nodes are
    // page-worthy anymore ([FR-WK-06] as modified by [CR-056]/[S-221]) — a
    // generic per-entity page for either has no navigational entry point in the
    // wiki menu ([FR-WK-11]) or the native Files view ([FR-WK-10]).
    assert!(
        st.work_list
            .unanchored_entities
            .iter()
            .all(|e| e.kind != "file" && e.kind != "module"),
        "File and Module are never seeded as unanchored entities: {:?}",
        st.work_list.unanchored_entities
    );
    // No swe-skills convention files in this fixture → no typed doc nodes
    // fabricated (FR-WK-06, NFR-RA-05).
    assert!(
        st.work_list
            .unanchored_entities
            .iter()
            .all(|e| e.kind != "requirement" && e.kind != "adr" && e.kind != "story"),
        "no typed doc nodes without the doc graph"
    );
    assert!(
        st.work_list.unanchored_entities.is_empty(),
        "no page-worthy entity kind is currently seeded at all: {:?}",
        st.work_list.unanchored_entities
    );

    // Edit the anchored file on disk (no sync): guide/a goes stale and joins the
    // work-list at the next status (FR-WK-03 / FR-WK-06). This generic
    // existing-page mechanism is unaffected by the File/Module exclusion.
    std::fs::write(repo.join("src/a.rs"), branchy("a", 5)).unwrap();
    let st = engine.wiki_status().expect("status after edit");
    assert_eq!(st.stale_count, 1);
    assert_eq!(st.work_list.stale_pages[0].slug, "guide/a");
    assert!(st.work_list.stale_pages[0].stale);
}

// ── FR-WK-06 / CR-027 / UAT-WK-05: structured agent-tier section seeding ──────

#[test]
fn structured_sections_seed_after_index_and_derive_revision_staleness() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    // Pin the [FR-WK-17] dampening threshold to its minimum (1): this test
    // exercises the undamped revision-pending baseline (a single re-index makes
    // a built page revision-stale) — dampening cadence itself is covered by the
    // dedicated FR-WK-17 fixtures.
    std::fs::create_dir_all(repo.join(".logos")).unwrap();
    std::fs::write(
        repo.join(".logos/config.toml"),
        "[wiki]\nrevision_stale_threshold = 1\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");

    // Before the first `index` the structured work-list is empty — `init` is
    // never blocked by wiki work (FR-WK-12 first-build-after-index).
    let st = engine.wiki_status().expect("status before index");
    assert!(
        st.work_list.structured_sections.is_empty(),
        "no structured work before the first index"
    );

    engine.index();
    let rev1 = engine.status().graph_revision;
    assert!(rev1 >= 1, "an index establishes a graph revision");

    // After the first `index` the five Overview children seed as `absent`, and
    // the native (extracted) tier is never listed. The synthesized
    // overview/architecture page is retired ([CR-062]) — presented, not seeded.
    let st = engine.wiki_status().expect("status after index");
    let overview_slugs: Vec<&str> = st
        .work_list
        .structured_sections
        .iter()
        .filter(|s| s.anchor.is_empty() && s.state == logos_core::wiki::SectionState::Absent)
        .map(|s| s.slug.as_str())
        .collect();
    for slug in [
        "overview/project-overview",
        "overview/getting-started",
        "overview/key-concepts",
        "overview/how-it-works",
        "overview/known-issues",
    ] {
        assert!(
            overview_slugs.contains(&slug),
            "the Overview child {slug} seeds absent: {overview_slugs:?}"
        );
    }
    assert!(
        !overview_slugs.contains(&"overview/architecture"),
        "the synthesized overview/architecture page is retired ([CR-062]): {overview_slugs:?}"
    );
    assert!(
        st.work_list.structured_sections.iter().all(|s| {
            !matches!(
                s.section,
                "codebase structure" | "configuration" | "files view" | "dependency mermaid"
            )
        }),
        "the deterministic native tier is never in the work-list"
    );

    // Author the five Overview pages at their canonical slugs (zero-anchor, the
    // way the embedded skill does) → they clear from the work-list.
    for slug in [
        "overview/project-overview",
        "overview/getting-started",
        "overview/key-concepts",
        "overview/how-it-works",
        "overview/known-issues",
    ] {
        engine
            .wiki_write(slug, "Overview section", PAGE, &[], "gen")
            .expect("write overview section");
    }
    let st = engine.wiki_status().expect("status after authoring overview");
    assert!(
        st.work_list
            .structured_sections
            .iter()
            .all(|s| !s.slug.starts_with("overview/")),
        "the authored Overview children clear from the work-list"
    );

    // Edit a file and re-index → the revision advances and the pages built at the
    // old revision read `revision-stale` — derived purely from the revision, on a
    // pure-read `wiki status` (no regeneration, no write).
    std::fs::write(repo.join("src/a.rs"), branchy("a", 4)).unwrap();
    engine.index();
    let rev2 = engine.status().graph_revision;
    assert!(rev2 > rev1, "the re-index advanced the revision");

    let st = engine.wiki_status().expect("status after re-index");
    let stale_overview: Vec<&str> = st
        .work_list
        .structured_sections
        .iter()
        .filter(|s| {
            s.slug.starts_with("overview/")
                && s.state == logos_core::wiki::SectionState::RevisionStale
        })
        .map(|s| s.slug.as_str())
        .collect();
    assert_eq!(
        stale_overview.len(),
        5,
        "all five Overview pages built at the old revision read revision-stale: {stale_overview:?}"
    );

    // [CR-044] dual-axis freshness regression: these five zero-anchor prose pages
    // were built at rev1 and the graph is now at rev2, so they are revision-stale.
    // Having no anchors they can never be anchor-stale — yet they must NOT count as
    // fresh (the bug that let the landing read "all fresh" while the banner read
    // STALE). `wiki status` now reports them honestly on the revision axis, and the
    // returned `current_revision` is the very revision the counts were computed at.
    assert_eq!(st.page_count, 5);
    assert_eq!(
        st.revision_stale_count, 5,
        "every zero-anchor page built at the old revision is revision-stale"
    );
    assert_eq!(
        st.fresh_count, 0,
        "a revision-pending page is not fresh, even with no anchors to be stale"
    );
    assert!(
        (st.freshness_fraction - 0.0).abs() < f64::EPSILON,
        "dual-axis freshness fraction reflects the revision-stale pages"
    );
    assert_eq!(
        st.current_revision, rev2,
        "the counts are computed against the revision wiki status returns"
    );
}

// ── FR-WK-06 / CR-034: doc-grounded, consolidated category seeding ────────────

/// [CR-034]: over a real graph, documentation page-worthiness is **consolidated**
/// — the work-list seeds one entry per documentation category whose `docs/`
/// source files exist (each naming its glob), drops the per-node and Story/CR
/// page-worthiness, and never fabricates a category whose source files are
/// absent. A pure local-FS scan; gate-immune, no reindex of `wiki.db`.
#[test]
fn consolidated_doc_categories_seed_from_docs_over_a_real_graph() {
    use logos_core::wiki::{DocCategory, SectionState};

    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    // Three categories have source files; the other four (components, integrations,
    // NFRs, UATs) do not — they must NOT be seeded (reported by absence).
    commit(
        repo,
        "docs/specs/architecture/decisions/ADR-01.md",
        "# ADR-01: A decision\n",
        "adr",
    );
    commit(
        repo,
        "docs/specs/requirements/FR-X-01.md",
        "# FR-X-01: A requirement\n",
        "fr",
    );
    commit(
        repo,
        "docs/specs/frontend-design.md",
        "# Frontend design\n",
        "fe",
    );

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let st = engine.wiki_status().expect("status after index");

    // Exactly the three present categories seed, in fixed menu order, each
    // zero-anchor + absent + carrying a doc-grounding directive naming its glob.
    let consolidated: Vec<(&str, &str)> = st
        .work_list
        .structured_sections
        .iter()
        .filter(|s| DocCategory::ALL.iter().any(|c| c.label() == s.section))
        .map(|s| (s.section, s.slug.as_str()))
        .collect();
    assert_eq!(
        consolidated,
        vec![
            ("adrs", "architecture/adrs"),
            ("functional-requirements", "specs/functional-requirements"),
            ("frontend-design", "specs/frontend-design"),
        ],
        "one consolidated entry per present category, in menu order: {consolidated:?}"
    );
    let adrs = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "architecture/adrs")
        .expect("ADRs seeded");
    assert!(adrs.anchor.is_empty(), "consolidated pages are zero-anchor");
    assert_eq!(adrs.state, SectionState::Absent);
    assert_eq!(
        adrs.grounding.as_ref().expect("doc-grounded").sources,
        vec!["docs/specs/architecture/decisions/*.md".to_string()],
    );

    // The absent categories are never fabricated.
    for absent in [
        "architecture/components",
        "architecture/integrations",
        "specs/non-functional-requirements",
        "specs/user-acceptance-tests",
    ] {
        assert!(
            st.work_list
                .structured_sections
                .iter()
                .all(|s| s.slug != absent),
            "a category with no source files is not seeded: {absent}"
        );
    }

    // Documentation is consolidated, never per-node: no typed Requirement/Adr/
    // Story unanchored entities (the production resolver no longer yields them).
    assert!(
        st.work_list
            .unanchored_entities
            .iter()
            .all(|e| e.kind != "requirement" && e.kind != "adr" && e.kind != "story"),
        "per-node doc pages are dropped — documentation is consolidated"
    );

    // The generation queue surfaces the consolidated items with their grounding.
    let queue = engine.wiki_generate().expect("generate");
    let adrs_item = queue
        .items
        .iter()
        .find(|i| i.slug == "architecture/adrs")
        .expect("ADRs queued");
    assert_eq!(
        adrs_item.category,
        logos_core::wiki::GenerationCategory::ConsolidatedDoc
    );
    assert!(
        adrs_item.grounding.is_some(),
        "the queued consolidated item carries its doc-grounding directive"
    );
}

/// [FR-WK-21]/[ADR-57]: the `Engine::wiki_srs_mode` façade accessor reports Case 1
/// (SRS present) only when `docs/specs/architecture.md` AND ≥1 `FR-*`/`NFR-*`/
/// `UAT-*` file are on disk, Case 2 otherwise — a pure local-FS read through the
/// façade, exercised end-to-end so the accessor's `&self.root` plumbing and its
/// agreement with the free-function gate are guarded (not just the free function).
#[test]
fn engine_wiki_srs_mode_gate_reports_case_1_and_case_2_over_a_real_tree() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    let engine = Engine::start(repo).expect("engine starts");

    // Case 2: no docs/specs/ artifacts at all.
    assert!(!engine.wiki_srs_mode(), "empty tree → Case 2 (inference mode)");

    // Case 2: a requirement present, but architecture.md still absent.
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(repo.join("docs/specs/requirements/FR-X-01.md"), "# FR-X-01\n").unwrap();
    assert!(!engine.wiki_srs_mode(), "requirement but no architecture.md → Case 2");

    // Case 1: architecture.md added alongside the requirement — both load-bearing
    // artifacts present.
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n").unwrap();
    assert!(engine.wiki_srs_mode(), "architecture.md + a requirement → Case 1 (SRS present)");

    // Case 2 again: architecture.md present, but only a Design-tier doc (no FR/NFR/UAT).
    std::fs::remove_file(repo.join("docs/specs/requirements/FR-X-01.md")).unwrap();
    std::fs::create_dir_all(repo.join("docs/specs/architecture/decisions")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture/decisions/ADR-01.md"), "# ADR-01\n").unwrap();
    assert!(
        !engine.wiki_srs_mode(),
        "architecture.md + ADRs but no FR/NFR/UAT → Case 2"
    );
}

// ── FR-WK-22: reconciliation sweep (end-to-end through the façade) ───────────

/// [FR-WK-22]/[ADR-57]: `Engine::wiki_reconcile` purges a stored page whose slug
/// is outside the active-mode valid set (the Overview slugs ∪ present
/// consolidated-category slugs) while sparing a valid Overview page and a valid
/// present-category page, logs the purge, and is idempotent on a second call —
/// exercised against a real `Engine`/`self.root` (and its `wiki::open` +
/// `runtime.submit_read` wiring for `wiki_write`), not just the free functions
/// [`logos_core::wiki`]'s unit tests already cover.
#[test]
fn engine_wiki_reconcile_purges_orphan_and_is_idempotent() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    // The ADRs category has a source file on disk, so its consolidated-page
    // slug is valid; Components has none, so a page at its slug is an orphan.
    std::fs::create_dir_all(repo.join("docs/specs/architecture/decisions")).unwrap();
    std::fs::write(
        repo.join("docs/specs/architecture/decisions/ADR-01.md"),
        "# ADR-01\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    engine
        .wiki_write(
            "overview/project-overview",
            "Project Overview",
            PAGE,
            &[],
            "gen",
        )
        .expect("write the valid Overview page");
    engine
        .wiki_write("architecture/adrs", "ADRs", PAGE, &[], "gen")
        .expect("write the valid, present-category page");
    engine
        .wiki_write("architecture/components", "Components", PAGE, &[], "gen")
        .expect("write the orphan — Components has no source files under root");

    let removed = engine.wiki_reconcile().expect("reconcile succeeds");
    assert_eq!(
        removed,
        vec!["architecture/components".to_string()],
        "only the orphaned, non-present-category page is purged"
    );
    assert!(
        engine
            .wiki_read("overview/project-overview")
            .unwrap()
            .is_some(),
        "the valid Overview page survives"
    );
    assert!(
        engine.wiki_read("architecture/adrs").unwrap().is_some(),
        "the valid, present-category page survives"
    );
    assert!(
        engine
            .wiki_read("architecture/components")
            .unwrap()
            .is_none(),
        "the orphaned page is gone"
    );
    let log = engine.wiki_pruned_log().unwrap();
    assert_eq!(log.len(), 1, "the purge is logged");
    assert_eq!(log[0].slug, "architecture/components");

    // Idempotent: a second sweep against the reconciled store purges nothing.
    let removed_again = engine.wiki_reconcile().expect("reconcile succeeds again");
    assert!(
        removed_again.is_empty(),
        "a reconciled store has nothing left to purge"
    );
    assert_eq!(
        engine.wiki_pruned_log().unwrap().len(),
        1,
        "no duplicate pruning logged on the idempotent re-run"
    );
}

// ── FR-WK-17: revision-stale regeneration cadence dampening (end-to-end) ─────

/// The full [FR-WK-17] loop against a real `Engine` and a configured
/// `[wiki].revision_stale_threshold`: a regenerated Overview page does **not**
/// re-queue across repeated single-revision `index` advances below the
/// threshold — `wiki status` and `wiki generate` both stay silent on it, while
/// `revision_stale_count` reports it honestly throughout — and re-queues once
/// the accumulated drift crosses the threshold. Also asserts the offline
/// invariant ([NFR-SE-01]): neither call ever writes `wiki.db`.
#[test]
fn revision_stale_dampening_bounds_requeue_cadence_end_to_end_and_stays_offline() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    std::fs::create_dir_all(repo.join(".logos")).unwrap();
    std::fs::write(
        repo.join(".logos/config.toml"),
        "[wiki]\nrevision_stale_threshold = 3\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    let built_at = engine.status().graph_revision;

    engine
        .wiki_write(
            "overview/project-overview",
            "Project Overview",
            PAGE,
            &[],
            "gen",
        )
        .expect("write overview");

    let wiki_db = repo.join(".logos/wiki.db");
    let before = std::fs::read(&wiki_db).unwrap();

    // Two single-file-edit + reindex cycles: the revision advances by 1 each
    // time, a cumulative delta of 2 — below the configured threshold of 3.
    // Neither the work-list nor the `wiki generate` queue re-lists the page,
    // even though `wiki status` truthfully counts it as revision-stale
    // throughout ([NFR-CC-04]: reporting is never masked by dampening).
    for i in 0..2u32 {
        std::fs::write(repo.join("src/a.rs"), branchy("a", 3 + i as usize)).unwrap();
        engine.index();
        let current = engine.status().graph_revision;
        assert!(current > built_at, "the graph revision advanced");
        let delta = current - built_at;
        assert!(delta < 3, "this loop only covers deltas below the threshold: {delta}");

        let st = engine.wiki_status().expect("status");
        assert_eq!(
            st.revision_stale_count, 1,
            "revision_stale_count is honest at delta {delta}"
        );
        assert!(
            st.work_list
                .structured_sections
                .iter()
                .all(|s| s.slug != "overview/project-overview"),
            "delta {delta} is below the threshold of 3 — the page must not re-queue"
        );

        let queue = engine.wiki_generate().expect("generate");
        assert!(
            queue.items.iter().all(|i| i.slug != "overview/project-overview"),
            "the generation queue mirrors the dampened work-list at delta {delta}"
        );
    }

    // `wiki_status`/`wiki_generate` performed no writes across the whole loop:
    // wiki.db is byte-identical, and both are pure local-config + local-store
    // reads — no LLM call, no network call ([NFR-SE-01]).
    let after = std::fs::read(&wiki_db).unwrap();
    assert_eq!(
        before, after,
        "wiki_status/wiki_generate never write wiki.db (NFR-SE-01)"
    );

    // One more re-index crosses the configured threshold — the page re-queues,
    // revision-stale, in both the work-list and the generation queue.
    std::fs::write(repo.join("src/a.rs"), branchy("a", 9)).unwrap();
    engine.index();
    let current = engine.status().graph_revision;
    assert!(
        current - built_at >= 3,
        "the accumulated re-indexes crossed the configured threshold of 3"
    );

    let st = engine.wiki_status().expect("status");
    let section = st
        .work_list
        .structured_sections
        .iter()
        .find(|s| s.slug == "overview/project-overview")
        .expect("the threshold is crossed — the page re-queues");
    assert_eq!(section.state, logos_core::wiki::SectionState::RevisionStale);

    let queue = engine.wiki_generate().expect("generate");
    let item = queue
        .items
        .iter()
        .find(|i| i.slug == "overview/project-overview")
        .expect("the generation queue re-lists the page once the threshold is crossed");
    assert_eq!(item.reason, "revision-stale");
}

// ── FR-WK-20: deterministic presented tier (`wiki materialize`, end-to-end) ──

/// [FR-WK-20]/[ADR-57]/[CR-062]: in SRS mode `Engine::wiki_materialize` presents
/// each present Design/Specs category and the single-file Architecture page from
/// the authored `docs/specs/**` sources into `wiki.db` — `generator =
/// logos:doc-present`, a sorted section-per-source-document body, one `file:`
/// anchor per document — serves the presented provenance marker, trusts the prose
/// guard for a doc that embeds an agent-noise token, runs the reconciliation sweep
/// so an orphan is purged while every presented page survives, and is
/// byte-identical on re-run.
#[test]
fn engine_wiki_materialize_presents_design_specs_pages_and_sweeps_orphans() {
    use logos_core::wiki::{PRESENTED_CONTENT_MARKER, PRESENTED_GENERATOR};

    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    // Case-1 fixture: architecture.md + two ADRs + two FRs. FR-WK-02 embeds a
    // `<tool_call` token the prose guard would reject for a normal write.
    std::fs::create_dir_all(repo.join("docs/specs/architecture/decisions")).unwrap();
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(
        repo.join("docs/specs/architecture.md"),
        "# Architecture\n\nThe system design.\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("docs/specs/architecture/decisions/ADR-02.md"),
        "# ADR-02\n\nSecond decision.\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("docs/specs/architecture/decisions/ADR-01.md"),
        "# ADR-01\n\nFirst decision.\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("docs/specs/requirements/FR-WK-01.md"),
        "# FR-WK-01\n\nA requirement.\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("docs/specs/requirements/FR-WK-02.md"),
        "# FR-WK-02\n\n<tool_call>{\"name\":\"read\"}</tool_call>\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Pre-seed an orphan the sweep must purge (Components has no source files).
    engine
        .wiki_write("architecture/components", "Components", PAGE, &[], "gen")
        .expect("seed an orphan page");

    let summary = engine.wiki_materialize().expect("materialize succeeds");
    assert!(summary.srs_mode, "architecture.md + a requirement → Case 1");
    // Write order: Architecture, then the present categories in DocCategory::ALL
    // order (Adrs, then FunctionalRequirements — the others have no source files).
    assert_eq!(
        summary.materialized,
        vec![
            "overview/architecture".to_string(),
            "architecture/adrs".to_string(),
            "specs/functional-requirements".to_string(),
        ],
    );
    assert_eq!(
        summary.pruned,
        vec!["architecture/components".to_string()],
        "the sweep purges the orphan and spares every presented page"
    );

    // The presented FR page: doc-present provenance, sorted sections, file anchors,
    // and the guard-trusted noise token presented verbatim.
    let fr = engine
        .wiki_read("specs/functional-requirements")
        .unwrap()
        .expect("FR page present");
    assert_eq!(fr.generator, PRESENTED_GENERATOR);
    assert_eq!(
        fr.marker, PRESENTED_CONTENT_MARKER,
        "served provenance is the presented label, never the generated marker"
    );
    assert_eq!(
        fr.anchors.iter().map(|a| a.entity_id.clone()).collect::<Vec<_>>(),
        vec![
            "docs/specs/requirements/FR-WK-01.md".to_string(),
            "docs/specs/requirements/FR-WK-02.md".to_string(),
        ],
        "one file anchor per source document, in sorted order"
    );
    // Sections are headed by the file stem (no `.md`), the stable-anchor source
    // ([FR-WK-25]).
    let one = fr.body.find("## FR-WK-01\n").expect("FR-01 section");
    let two = fr.body.find("## FR-WK-02\n").expect("FR-02 section");
    assert!(one < two, "sections are sorted by source path");
    assert!(
        fr.body.contains("<tool_call>"),
        "the agent-noise token is presented verbatim, not rejected"
    );

    // The Architecture page survives the sweep at its reused slug.
    let arch = engine
        .wiki_read("overview/architecture")
        .unwrap()
        .expect("Architecture page present");
    assert_eq!(arch.title, "Architecture");
    assert_eq!(arch.generator, PRESENTED_GENERATOR);
    assert!(arch.body.contains("The system design."));

    // Byte-identical re-run, and the sweep is now idempotent.
    let before = fr.body.clone();
    let again = engine.wiki_materialize().expect("re-materialize succeeds");
    assert!(
        again.pruned.is_empty(),
        "a reconciled store has nothing left to purge on the byte-identical re-run"
    );
    let fr_again = engine.wiki_read("specs/functional-requirements").unwrap().unwrap();
    assert_eq!(fr_again.body, before, "re-materialize is byte-identical ([FR-WK-20])");
}

/// [FR-WK-26]/[CR-064] (S-269): with the SRS hub present, `wiki_materialize`
/// presents `docs/specs/software-spec.md` at `specs/srs` (`generator =
/// logos:doc-present`, one `file:` anchor, presented body), the reconciliation
/// sweep never purges it (it survives even when pre-seeded as a would-be orphan),
/// a `software-spec.md`/`§N` reference from another presented page resolves onto
/// it, and the stray top-level analyst inputs are not presented. Byte-identical on
/// re-run.
#[test]
fn engine_wiki_materialize_presents_the_srs_hub_and_never_purges_it() {
    use logos_core::wiki::{PRESENTED_CONTENT_MARKER, PRESENTED_GENERATOR};

    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    // Case-1 fixture: architecture.md + a requirement (gates SRS mode) + the SRS
    // hub, whose body links to itself by relative path so the manifest rewrite is
    // exercised end-to-end.
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n\nThe design.\n")
        .unwrap();
    std::fs::write(repo.join("docs/specs/requirements/FR-WK-01.md"), "# FR-WK-01\n\nA req.\n")
        .unwrap();
    std::fs::write(
        repo.join("docs/specs/software-spec.md"),
        "# Software Requirements Specification\n\nThe hub. See [§3.24](software-spec.md#3-24).\n",
    )
    .unwrap();
    // Stray top-level analyst intermediates — present on disk, never presented.
    std::fs::write(repo.join("docs/specs/analyst-frs.md"), "# Analyst FRS\n\nSTRAY_ANALYST\n")
        .unwrap();
    std::fs::write(repo.join("docs/specs/writer-nfr.md"), "# Writer NFR\n\nSTRAY_WRITER\n").unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Pre-seed `specs/srs` as a would-be orphan (a stale prior write) to prove the
    // sweep spares it via the valid-slug set, not merely because materialize rewrote it.
    engine.wiki_write("specs/srs", "Stale", PAGE, &[], "gen").expect("seed a stale SRS page");

    let summary = engine.wiki_materialize().expect("materialize succeeds");
    assert!(summary.srs_mode, "architecture.md + a requirement → Case 1");
    assert!(
        summary.materialized.contains(&"specs/srs".to_string()),
        "the SRS hub is materialized: {:?}",
        summary.materialized
    );
    // Write order: Architecture, then the SRS hub, then the present categories.
    assert_eq!(
        summary.materialized,
        vec![
            "overview/architecture".to_string(),
            "specs/srs".to_string(),
            "specs/functional-requirements".to_string(),
        ],
    );
    assert!(
        !summary.pruned.contains(&"specs/srs".to_string()),
        "the reconciliation sweep never purges the SRS hub: {:?}",
        summary.pruned
    );

    // The presented SRS page: doc-present provenance, single file anchor, the hub's
    // body — and only the hub, never a stray analyst intermediate.
    let srs = engine.wiki_read("specs/srs").unwrap().expect("SRS page present");
    assert_eq!(srs.title, "Software Requirements Specification");
    assert_eq!(srs.generator, PRESENTED_GENERATOR);
    assert_eq!(srs.marker, PRESENTED_CONTENT_MARKER);
    assert_eq!(
        srs.anchors.iter().map(|a| a.entity_id.clone()).collect::<Vec<_>>(),
        vec!["docs/specs/software-spec.md".to_string()],
        "one file anchor — the hub, never the strays"
    );
    assert!(srs.body.contains("The hub."));
    assert!(!srs.body.contains("STRAY_ANALYST"), "analyst-frs.md is not presented");
    assert!(!srs.body.contains("STRAY_WRITER"), "writer-nfr.md is not presented");
    // The self-reference `software-spec.md#3-24` is rewritten to the in-page stem
    // anchor by the manifest transform (same-page → `#anchor`), not left dangling —
    // but only when the markdown grammar is present. Without `lang-markdown`,
    // `rewrite_refs` is the identity (the same graceful degradation the doc extractor
    // takes), so the authored ref is presented verbatim.
    #[cfg(feature = "lang-markdown")]
    assert!(
        srs.body.contains("[§3.24](#toc-software-spec)"),
        "the manifest rewrites the hub's own ref to its stem anchor: {}",
        srs.body
    );
    #[cfg(not(feature = "lang-markdown"))]
    assert!(
        srs.body.contains("[§3.24](software-spec.md#3-24)"),
        "without lang-markdown the transform is the identity — ref presented verbatim: {}",
        srs.body
    );

    // Byte-identical re-run over the unchanged tree, and the sweep is idempotent.
    let before = srs.body.clone();
    let again = engine.wiki_materialize().expect("re-materialize succeeds");
    assert!(again.pruned.is_empty(), "a reconciled store has nothing left to purge");
    let srs_again = engine.wiki_read("specs/srs").unwrap().unwrap();
    assert_eq!(srs_again.body, before, "re-materialize is byte-identical ([FR-WK-20])");
}

/// [FR-WK-21]/[CR-062]: in Case 2 (no authored SRS) `wiki_materialize` is a
/// no-op — it presents no page and reports `srs_mode = false`, so the Case-2
/// agent-inference path is untouched and an absent glob never crashes.
#[test]
fn engine_wiki_materialize_is_a_noop_in_case_2() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    let summary = engine.wiki_materialize().expect("materialize is a safe no-op in Case 2");
    assert!(!summary.srs_mode, "no docs/specs → Case 2");
    assert!(summary.materialized.is_empty(), "no page is presented in Case 2");
    assert!(summary.pruned.is_empty(), "no sweep runs in Case 2");
    // AC5: Case 2 makes NO wiki.db write — the no-op returns before the store is
    // even opened, so the file must not exist afterward.
    assert!(
        !repo.join(".logos/wiki.db").exists(),
        "Case-2 materialize never opens or writes wiki.db"
    );
}

// ── FR-WK-23: User Guide tier (per-file presentation, end-to-end) ───────────

/// [FR-WK-23]/[CR-062]: in SRS mode, with `docs/howto/` present, `Engine::
/// wiki_materialize` also presents one `guide/<name>` page per `docs/howto/*.md`
/// file (`README.md` → `guide/overview`) — `generator = logos:doc-present`,
/// rendered **verbatim** (no consolidated-mode section wrapper), anchored to its
/// source file — and the reconciliation sweep purges a stale `guide/*` page whose
/// source file is gone while sparing every guide `materialize` just wrote.
/// Byte-identical on re-run.
#[test]
fn engine_wiki_materialize_presents_user_guide_pages_and_purges_stale_guide() {
    use logos_core::wiki::{PRESENTED_CONTENT_MARKER, PRESENTED_GENERATOR};

    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");

    // Case-1 fixture: architecture.md + one FR, so the User Guide gate ([FR-WK-21])
    // is satisfied alongside it.
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n\nThe design.\n")
        .unwrap();
    std::fs::write(
        repo.join("docs/specs/requirements/FR-WK-01.md"),
        "# FR-WK-01\n\nA requirement.\n",
    )
    .unwrap();

    // The User Guide fixture — a README landing plus one guide.
    std::fs::create_dir_all(repo.join("docs/howto")).unwrap();
    std::fs::write(repo.join("docs/howto/README.md"), "# User Guide\n\nStart here.\n").unwrap();
    std::fs::write(
        repo.join("docs/howto/installation.md"),
        "# Installation\n\nRun the installer.\n",
    )
    .unwrap();

    let engine = Engine::start(repo).expect("engine starts");
    engine.index();

    // Pre-seed a stale guide page whose source file does not exist — the sweep
    // must purge it as an ordinary orphan.
    engine
        .wiki_write(
            "guide/retired",
            "Retired",
            "# Retired\n\nA stale guide page whose source file no longer exists.\n",
            &[],
            "gen",
        )
        .expect("seed a stale guide orphan");

    let summary = engine.wiki_materialize().expect("materialize succeeds");
    assert!(summary.srs_mode, "architecture.md + a requirement → Case 1");
    assert!(
        summary.materialized.contains(&"guide/overview".to_string()),
        "README.md presents at the guide/overview landing: {:?}",
        summary.materialized
    );
    assert!(
        summary.materialized.contains(&"guide/installation".to_string()),
        "installation.md presents at guide/installation: {:?}",
        summary.materialized
    );
    assert_eq!(
        summary.pruned,
        vec!["guide/retired".to_string()],
        "the sweep purges the stale guide orphan and spares every presented guide page"
    );

    let overview = engine.wiki_read("guide/overview").unwrap().expect("landing page present");
    assert_eq!(overview.title, "Overview");
    assert_eq!(overview.generator, PRESENTED_GENERATOR);
    assert_eq!(overview.marker, PRESENTED_CONTENT_MARKER);
    assert_eq!(
        overview.body, "# User Guide\n\nStart here.\n",
        "rendered verbatim — no consolidated-mode header/section wrapper"
    );
    assert_eq!(
        overview.anchors.iter().map(|a| a.entity_id.clone()).collect::<Vec<_>>(),
        vec!["docs/howto/README.md".to_string()]
    );

    let installation = engine.wiki_read("guide/installation").unwrap().expect("guide page present");
    assert_eq!(installation.title, "Installation");
    assert_eq!(installation.body, "# Installation\n\nRun the installer.\n");

    // Byte-identical re-run; the sweep is now idempotent for the guide set.
    let before = installation.body.clone();
    let again = engine.wiki_materialize().expect("re-materialize succeeds");
    assert!(
        again.pruned.is_empty(),
        "a reconciled store has nothing left to purge on the byte-identical re-run"
    );
    let installation_again = engine.wiki_read("guide/installation").unwrap().unwrap();
    assert_eq!(installation_again.body, before, "re-materialize is byte-identical ([FR-WK-23])");
}

/// [FR-WK-23]: `Engine::wiki_guide_pages` reports the empty set when
/// `docs/howto/` has no `*.md` file, and one `(slug, title)` pair per file —
/// `README.md` first at `guide/overview` — once guides are added **and** the
/// project is in SRS mode ([FR-WK-21]), mirroring the order `wiki_materialize`
/// writes them in.
#[test]
fn engine_wiki_guide_pages_reports_the_dynamic_per_file_set() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    // SRS-mode fixture: architecture.md + a requirement, so the User Guide gate
    // ([FR-WK-23], "docs/howto/ presence in SRS mode") is satisfied.
    std::fs::create_dir_all(repo.join("docs/specs/requirements")).unwrap();
    std::fs::write(repo.join("docs/specs/architecture.md"), "# Architecture\n").unwrap();
    std::fs::write(repo.join("docs/specs/requirements/FR-WK-01.md"), "# FR-WK-01\n").unwrap();
    let engine = Engine::start(repo).expect("engine starts");

    assert!(engine.wiki_guide_pages().is_empty(), "no docs/howto/ → no User Guide pages");

    std::fs::create_dir_all(repo.join("docs/howto")).unwrap();
    std::fs::write(repo.join("docs/howto/usage.md"), "# Usage\n").unwrap();
    std::fs::write(repo.join("docs/howto/README.md"), "# User Guide\n").unwrap();

    assert_eq!(
        engine.wiki_guide_pages(),
        vec![
            ("guide/overview".to_string(), "Overview".to_string()),
            ("guide/usage".to_string(), "Usage".to_string()),
        ],
        "README.md lands first at guide/overview, regardless of on-disk write order"
    );
}

/// [FR-WK-23]: the User Guide tier is gated on `docs/howto/` presence **in SRS
/// mode** — in Case 2 (no `docs/specs/architecture.md` + requirement) both
/// `Engine::wiki_guide_pages()` and `Engine::wiki_materialize()` report/write
/// nothing even though `docs/howto/` has files, since `wiki_materialize` never
/// writes a guide page in Case 2 (review finding: a Case-2 menu listing would be
/// a permanent, un-fulfillable "not yet generated" placeholder).
#[test]
fn engine_wiki_guide_pages_is_empty_in_case_2_even_with_docs_howto_present() {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    // No docs/specs/architecture.md → Case 2, despite docs/howto/ being present.
    std::fs::create_dir_all(repo.join("docs/howto")).unwrap();
    std::fs::write(repo.join("docs/howto/README.md"), "# User Guide\n\nStart here.\n").unwrap();
    let engine = Engine::start(repo).expect("engine starts");

    assert!(!engine.wiki_srs_mode(), "no docs/specs/architecture.md → Case 2");
    assert!(
        engine.wiki_guide_pages().is_empty(),
        "Case 2 → no User Guide pages, even though docs/howto/ has files"
    );

    let summary = engine.wiki_materialize().expect("materialize is a safe no-op in Case 2");
    assert!(!summary.srs_mode);
    assert!(summary.materialized.is_empty(), "Case 2 writes no guide page either");
    assert!(
        engine.wiki_read("guide/overview").unwrap().is_none(),
        "no guide/overview page is ever written in Case 2"
    );
}
