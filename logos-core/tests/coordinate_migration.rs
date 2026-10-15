//! S-704 ([FR-EX-35]): an existing store migrates to the repository coordinate
//! in place when an engine opens it — the graph, its snapshot history and its
//! gate baseline untouched.
//!
//! The store under test is real: `coordinate_migration/logos-1.17.0.db` is
//! what the logos 1.17.0 release binary built over `coordinate_migration/tree`
//! (`logos init`, a `rules.toml` with two layers and a boundary, `logos index`,
//! `logos scan`, `logos gate --save`, `logos scan`, then a declaration added,
//! indexed and removed again, which leaves one orphan symbol row). Every
//! symbol in it is coordinate-less and it records no coordinate. The tree's
//! `Cargo.toml` is stored as `Cargo.toml.in`, so this repository holds no
//! nested package manifest; [`project`] restores the name.
//!
//! [FR-EX-35]: ../../docs/specs/requirements/FR-EX-35.md

#![cfg(all(
    feature = "lang-rust",
    feature = "lang-java",
    feature = "lang-typescript",
    feature = "lang-markdown",
    feature = "lang-toml"
))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::graph_store::{PROJECT_ID_KEY, SYMBOL_COORDINATE_KEY};
use logos_core::model::LogosSymbol;
use logos_core::Engine;
use rusqlite::types::Value;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

use coordinate::FIXTURE_COORDINATE;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/coordinate_migration");

/// The declaration added, indexed and removed again while the fixture store
/// was built: its symbol row outlived its node.
const ORPHAN: &str = "logos . . . src/`service.rs`/legacy_total().";

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let target = match name.to_str() {
            Some("Cargo.toml.in") => to.join("Cargo.toml"),
            _ => to.join(&name),
        };
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The fixture tree at `root`, with its `rules.toml` and `[project] id`.
fn project(root: &Path, id: &str) {
    copy_dir(&Path::new(FIXTURE).join("tree"), root);
    fs::create_dir_all(root.join(".logos")).unwrap();
    fs::copy(Path::new(FIXTURE).join("rules.toml"), root.join(".logos/rules.toml")).unwrap();
    set_id(root, id);
}

/// Set `[project] id`, replacing the whole config.
fn set_id(root: &Path, id: &str) {
    fs::write(root.join(".logos/config.toml"), coordinate::project_table(id)).unwrap();
}

/// Put the 1.17.0 store in place as `root`'s `logos.db`, never opened since.
fn install_store(root: &Path) {
    fs::copy(Path::new(FIXTURE).join("logos-1.17.0.db"), db(root)).unwrap();
}

fn db(root: &Path) -> PathBuf {
    root.join(".logos/logos.db")
}

/// The one text column `sql` selects, sorted.
fn texts(root: &Path, sql: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db(root)).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    let mut out: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    out.sort();
    out
}

fn count(root: &Path, table: &str) -> i64 {
    let conn = rusqlite::Connection::open(db(root)).unwrap();
    conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}

fn metadata(root: &Path, key: &str) -> Option<String> {
    let conn = rusqlite::Connection::open(db(root)).unwrap();
    conn.query_row("SELECT value FROM project_metadata WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

/// Every row of `table`, every column, in rowid order.
fn rows(root: &Path, table: &str) -> Vec<Vec<Value>> {
    let conn = rusqlite::Connection::open(db(root)).unwrap();
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
    let width = stmt.column_count();
    stmt.query_map([], |r| (0..width).map(|i| r.get::<_, Value>(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// What a migration must leave exactly as it was: the graph's shape and the
/// whole quality history.
#[derive(Debug, PartialEq)]
struct Kept {
    nodes: Vec<Vec<Value>>,
    edges: Vec<Vec<Value>>,
    snapshots: Vec<Vec<Value>>,
    offenders: Vec<Vec<Value>>,
    baseline: Vec<Vec<Value>>,
    violations: Vec<Vec<Value>>,
    check_runs: Vec<Vec<Value>>,
}

fn kept(root: &Path) -> Kept {
    Kept {
        nodes: rows(root, "nodes"),
        edges: rows(root, "edges"),
        snapshots: rows(root, "metric_snapshots"),
        offenders: rows(root, "metric_snapshot_offenders"),
        baseline: rows(root, "baseline"),
        violations: rows(root, "violations"),
        check_runs: rows(root, "check_run"),
    }
}

/// Every node's symbol, sorted.
fn node_symbols(root: &Path) -> Vec<String> {
    texts(root, "SELECT s.symbol FROM nodes n JOIN symbols s ON s.id = n.symbol_id")
}

/// `symbol` under `coordinate`, through the one coordinate helper (S-703).
fn under(symbol: &str, coordinate: &str) -> String {
    LogosSymbol::parse(symbol).unwrap().with_coordinate(Some(coordinate)).unwrap().to_string()
}

/// Every symbol-bearing value in the store, sorted.
fn every_symbol_value(root: &Path) -> Vec<String> {
    let mut all = texts(root, "SELECT symbol FROM symbols");
    all.extend(texts(root, "SELECT source_symbol FROM unresolved_refs"));
    all.extend(texts(root, "SELECT symbol FROM declared_types WHERE symbol IS NOT NULL"));
    all
}

fn carries(symbol: &str, coordinate: &str) -> bool {
    let symbol = LogosSymbol::parse(symbol).unwrap();
    symbol.as_str().starts_with("local ") || symbol.coordinate() == Some(coordinate)
}

/// The bytes of `root`'s store with its write-ahead log folded in.
fn settled_bytes(root: &Path) -> Vec<u8> {
    let conn = rusqlite::Connection::open(db(root)).unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
    drop(conn);
    fs::read(db(root)).unwrap()
}

#[test]
fn a_pre_coordinate_store_migrates_on_open_keeping_its_graph_history_and_baseline() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    let before = kept(root);
    assert_eq!(before.snapshots.len(), 3, "the fixture carries snapshot history");
    assert_eq!(before.baseline.len(), 1, "and a saved gate baseline");
    let symbols_before = node_symbols(root);
    assert!(texts(root, "SELECT symbol FROM symbols").contains(&ORPHAN.to_string()));
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY), None, "1.17.0 records no coordinate");
    assert!(
        every_symbol_value(root).iter().all(|s| !carries(s, FIXTURE_COORDINATE)),
        "and no symbol carries one"
    );
    let revision = |root: &Path| metadata(root, "graph_revision").unwrap().parse::<u64>().unwrap();
    let revision_before = revision(root);

    drop(Engine::start(root).unwrap());

    // Every cache keyed on the graph revision re-reads the moved symbols.
    assert_eq!(revision(root), revision_before + 1, "the migration advanced the graph revision");

    assert_eq!(kept(root), before, "nodes, edges, snapshots, offenders, baseline, violations, check runs");
    let expected: Vec<String> = {
        let mut moved: Vec<String> = symbols_before.iter().map(|s| under(s, FIXTURE_COORDINATE)).collect();
        moved.sort();
        moved
    };
    assert_eq!(node_symbols(root), expected, "every node's symbol moved, and only its package slot");
    assert!(node_symbols(root).iter().any(|s| s.starts_with("logos policy example.com/fixture . ")));
    let off: Vec<String> = every_symbol_value(root)
        .into_iter()
        .filter(|s| !carries(s, FIXTURE_COORDINATE))
        .collect();
    assert!(off.is_empty(), "symbols, ledger and declared types all moved: {off:#?}");
    assert_eq!(texts(root, "SELECT symbol FROM declared_types WHERE symbol IS NOT NULL").len(), 2);
    assert!(
        !texts(root, "SELECT symbol FROM symbols").iter().any(|s| s.ends_with("legacy_total().")),
        "the orphan symbol row is gone"
    );
    assert_eq!(count(root, "symbols"), count(root, "nodes"));
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some(FIXTURE_COORDINATE));
    assert_eq!(metadata(root, PROJECT_ID_KEY).as_deref(), Some(FIXTURE_COORDINATE));
    assert_eq!(
        texts(root, "SELECT ifnull(content_hash, 'NULL') FROM files"),
        vec!["NULL".to_string(); 9],
        "every file re-extracts on the next reconcile"
    );
}

#[test]
fn after_the_next_reconcile_the_store_equals_a_fresh_index_and_verifies() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    let engine = Engine::start(root).unwrap();
    engine.check_rules(None, true).unwrap();

    assert_eq!(count(root, "symbols"), count(root, "nodes"), "one symbol row per node");
    assert!(
        !texts(root, "SELECT ifnull(content_hash, 'NULL') FROM files").contains(&"NULL".to_string()),
        "the reconcile re-read every file"
    );
    let fresh = TempDir::new().unwrap();
    project(fresh.path(), FIXTURE_COORDINATE);
    Engine::start(fresh.path()).unwrap().index();
    assert_eq!(node_symbols(root), node_symbols(fresh.path()), "the migrated store is the fresh one");
    let report = engine.verify().unwrap();
    assert!(report.ok, "verify finds no drift after the migration: {report:#?}");
}

#[test]
fn a_second_open_is_a_no_op() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    drop(Engine::start(root).unwrap());
    let migrated = settled_bytes(root);

    drop(Engine::start(root).unwrap());
    assert!(settled_bytes(root) == migrated, "the second open wrote nothing");
}

#[test]
fn changing_the_project_id_re_runs_the_migration_from_the_recorded_coordinate() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, "example.com/first");
    install_store(root);
    drop(Engine::start(root).unwrap());
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/first"));
    let first = node_symbols(root);
    let before = kept(root);

    set_id(root, "example.com/second");
    drop(Engine::start(root).unwrap());

    assert_eq!(kept(root), before, "the second migration moves nothing else either");
    let mut expected: Vec<String> = first.iter().map(|s| under(s, "example.com/second")).collect();
    expected.sort();
    assert_eq!(node_symbols(root), expected);
    let off: Vec<String> = every_symbol_value(root)
        .into_iter()
        .filter(|s| !carries(s, "example.com/second"))
        .collect();
    assert!(off.is_empty(), "nothing keeps the first coordinate: {off:#?}");
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/second"));
    assert_eq!(metadata(root, PROJECT_ID_KEY).as_deref(), Some("example.com/second"));
}

#[test]
fn a_store_whose_configuration_does_not_load_is_left_as_it_was_until_it_does() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    let symbols_before = texts(root, "SELECT symbol FROM symbols");
    fs::write(root.join(".logos/config.toml"), "[project]\nid = \"has a space\"\n").unwrap();

    drop(Engine::start(root).expect("the engine starts all the same"));
    assert_eq!(texts(root, "SELECT symbol FROM symbols"), symbols_before, "nothing moved");
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY), None);

    set_id(root, FIXTURE_COORDINATE);
    drop(Engine::start(root).unwrap());
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some(FIXTURE_COORDINATE), "the next open migrates");
}

#[test]
fn a_phase_reported_engine_start_migrates_the_store_too() {
    // `start_with_phase_report` mirrors `start_with_configs` step for step; a
    // step it missed would make the diagnostic path open a store differently.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    drop(Engine::start_with_phase_report(root).unwrap());
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some(FIXTURE_COORDINATE));
    assert!(node_symbols(root).iter().all(|s| carries(s, FIXTURE_COORDINATE)));
}

/// Run git in `cwd`, panicking on failure — fixtures only.
fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `(file, symbol)` for every node outside `except`, sorted.
fn file_symbols(root: &Path, except: &str) -> Vec<String> {
    texts(
        root,
        &format!(
            "SELECT f.path || ' ' || s.symbol FROM nodes n \
             JOIN symbols s ON s.id = n.symbol_id JOIN files f ON f.id = n.file_id \
             WHERE f.path <> '{except}'"
        ),
    )
}

#[test]
fn a_worktree_seeded_from_an_unmigrated_main_store_migrates_before_its_first_sync() {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    project(&main, FIXTURE_COORDINATE);
    fs::write(main.join(".gitignore"), ".logos/*.db*\n").unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "fixture"]);
    install_store(&main);

    let wt = tmp.path().join("wt");
    git(&main, &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "feature"]);
    let service = fs::read_to_string(wt.join("src/service.rs")).unwrap();
    fs::write(wt.join("src/service.rs"), format!("{service}\npub fn audit() -> u32 {{\n    1\n}}\n")).unwrap();

    drop(Engine::start(&wt).unwrap());

    assert_eq!(metadata(&wt, SYMBOL_COORDINATE_KEY).as_deref(), Some(FIXTURE_COORDINATE));
    let off: Vec<String> = every_symbol_value(&wt)
        .into_iter()
        .filter(|s| !carries(s, FIXTURE_COORDINATE))
        .collect();
    assert!(off.is_empty(), "the seed migrated, and the diff synced under its coordinate: {off:#?}");
    assert!(node_symbols(&wt).contains(&format!("logos . {FIXTURE_COORDINATE} . src/`service.rs`/audit().")));
    // Ordering: the migration clears every content hash, and the diff
    // reconcile re-hashes the file it syncs — so only a sync that ran after
    // the migration leaves that file's hash in place.
    assert_eq!(
        texts(&wt, "SELECT path || ':' || (content_hash IS NULL) FROM files WHERE path LIKE 'src/%'"),
        ["src/lib.rs:1", "src/model.rs:1", "src/service.rs:0"],
        "the worktree synced after it migrated"
    );
    assert_eq!(metadata(&main, SYMBOL_COORDINATE_KEY), None, "seeding migrated the copy, not main");

    drop(Engine::start(&main).unwrap());
    assert_eq!(metadata(&main, SYMBOL_COORDINATE_KEY).as_deref(), Some(FIXTURE_COORDINATE));
    let unchanged = file_symbols(&main, "src/service.rs");
    assert!(unchanged.len() > 20, "the comparison spans the graph: {}", unchanged.len());
    assert_eq!(file_symbols(&wt, "src/service.rs"), unchanged, "unchanged files' symbols equal main's");
}

/// A 1.17.0 config names no `[project] id`, so most upgrades take the derived
/// coordinate: the normalised `origin` remote. A changed remote moves the store
/// on the next open.
#[test]
fn a_store_without_a_project_id_migrates_to_its_origin_and_follows_a_changed_remote() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("orders");
    project(&root, FIXTURE_COORDINATE);
    fs::write(root.join(".logos/config.toml"), "").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["remote", "add", "origin", "https://example.com/acme/fixture.git"]);
    install_store(&root);

    drop(Engine::start(&root).unwrap());
    assert_eq!(metadata(&root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/acme/fixture"));
    assert!(every_symbol_value(&root).iter().all(|s| carries(s, "example.com/acme/fixture")));

    git(&root, &["remote", "set-url", "origin", "https://example.com/acme/renamed.git"]);
    drop(Engine::start(&root).unwrap());
    assert_eq!(metadata(&root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/acme/renamed"));
    assert!(every_symbol_value(&root).iter().all(|s| carries(s, "example.com/acme/renamed")));
}

/// Where git cannot answer for the repository (absent from PATH, or refusing
/// its ownership) the derived coordinate falls back to the directory name. A
/// store that records a coordinate is not moved to that fallback: otherwise
/// opens from two environments would rewrite the whole store back and forth.
/// Moving `.git` aside puts the root in the same position.
#[test]
fn a_store_keeps_its_coordinate_when_git_cannot_answer_for_the_root() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("orders");
    project(&root, FIXTURE_COORDINATE);
    fs::write(root.join(".logos/config.toml"), "").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["remote", "add", "origin", "https://example.com/acme/fixture.git"]);
    install_store(&root);
    drop(Engine::start(&root).unwrap());
    assert_eq!(metadata(&root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/acme/fixture"));
    let migrated = settled_bytes(&root);

    fs::rename(root.join(".git"), tmp.path().join("git-aside")).unwrap();
    drop(Engine::start(&root).unwrap());
    assert!(settled_bytes(&root) == migrated, "nothing moved to the directory-name fallback");

    fs::rename(tmp.path().join("git-aside"), root.join(".git")).unwrap();
    drop(Engine::start(&root).unwrap());
    assert!(settled_bytes(&root) == migrated, "and nothing moves back when git answers again");
}

/// A clone whose `origin` is a local checkout resolves what that checkout
/// resolves. While the checkout is away (an unmounted volume, a renamed
/// sibling) git still answers for the clone, but only the path's last
/// component is left to name it: an unconfirmed fallback, which a store that
/// records a coordinate is not moved to — or every open would rewrite the
/// whole store, and the next one, once the checkout is back, rewrite it back.
#[test]
fn a_store_keeps_its_coordinate_while_its_local_origin_is_away() {
    let tmp = TempDir::new().unwrap();
    let upstream = tmp.path().join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "-q", "-b", "main"]);
    git(&upstream, &["remote", "add", "origin", "https://example.com/acme/fixture.git"]);
    let root = tmp.path().join("orders");
    project(&root, FIXTURE_COORDINATE);
    fs::write(root.join(".logos/config.toml"), "").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["remote", "add", "origin", upstream.to_str().unwrap()]);
    install_store(&root);
    drop(Engine::start(&root).unwrap());
    assert_eq!(metadata(&root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/acme/fixture"));
    let migrated = settled_bytes(&root);

    fs::rename(&upstream, tmp.path().join("upstream-away")).unwrap();
    drop(Engine::start(&root).unwrap());
    assert!(settled_bytes(&root) == migrated, "nothing moved to the gone source's name");

    fs::rename(tmp.path().join("upstream-away"), &upstream).unwrap();
    drop(Engine::start(&root).unwrap());
    assert!(settled_bytes(&root) == migrated, "and nothing moves back once it returns");
}

/// A migration that fails — here two live nodes whose symbols would become
/// one — fails the open of nothing: the engine starts, and the store is left
/// exactly as it was, under its recorded coordinate.
#[test]
fn a_migration_that_fails_leaves_the_store_as_it_was_and_the_engine_starts() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    project(root, FIXTURE_COORDINATE);
    install_store(root);
    {
        let conn = rusqlite::Connection::open(db(root)).unwrap();
        let twin = under("logos . . . src/`lib.rs`/run().", FIXTURE_COORDINATE);
        conn.execute("INSERT INTO symbols (symbol) VALUES (?1)", [&twin]).unwrap();
        conn.execute(
            "INSERT INTO nodes (symbol_id, kind, name) VALUES (last_insert_rowid(), 3, 'run')",
            [],
        )
        .unwrap();
    }
    let symbols_before = texts(root, "SELECT symbol FROM symbols");
    let before = kept(root);

    let engine = Engine::start(root).expect("a failed migration does not fail the start");
    drop(engine);

    assert_eq!(texts(root, "SELECT symbol FROM symbols"), symbols_before, "nothing moved");
    assert_eq!(kept(root), before);
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY), None, "and nothing was recorded");
}

/// An index that admits nothing records the coordinate over an empty store.
/// After `[project] id` changes, the next open moves the record — with no row
/// to rewrite — so the first file a sync adds carries the new coordinate.
#[test]
fn a_recorded_coordinate_over_an_empty_store_follows_a_changed_id() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".logos")).unwrap();
    set_id(root, "example.com/first");
    Engine::start(root).unwrap().index();
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/first"));
    assert_eq!(count(root, "symbols"), 0, "the index admitted nothing");

    set_id(root, "example.com/second");
    let engine = Engine::start(root).unwrap();
    assert_eq!(metadata(root, SYMBOL_COORDINATE_KEY).as_deref(), Some("example.com/second"));

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    assert!(node_symbols(root).contains(&"logos . example.com/second . src/`lib.rs`/run().".to_string()));
}
