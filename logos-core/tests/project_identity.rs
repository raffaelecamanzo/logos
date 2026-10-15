//! S-702 ([FR-CF-10]): the project's identity coordinate is recorded in the
//! store by a full index and reported by `status`; since S-703 every symbol
//! carries it, and a changed coordinate moves nothing else.
//!
//! The fixture pins `[project] id`, so no figure here depends on the machine's
//! directory names.
//!
//! [FR-CF-10]: ../../docs/specs/requirements/FR-CF-10.md

use std::fs;
use std::path::Path;

use logos_core::Engine;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Two Rust files with a cross-file call, so the store holds nodes, edges and
/// a bound reference.
fn fixture(root: &Path, project_id: &str) {
    write(root, ".logos/config.toml", &format!("[project]\nid = \"{project_id}\"\n"));
    write(root, "src/lib.rs", "mod util;\npub fn run() { util::helper(); }\n");
    write(root, "src/util.rs", "pub fn helper() {}\npub struct Thing;\n");
}

/// Every node and edge in the store, keyed by symbol strings rather than row
/// ids, sorted — the rows a coordinate in the symbol rewrites.
fn node_and_edge_rows(root: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(root.join(".logos/logos.db")).unwrap();
    let mut rows = Vec::new();
    for sql in [
        "SELECT 'node ' || s.symbol || ' ' || n.kind || ' ' || n.name || ' ' || \
         COALESCE(f.path, '-') || ' ' || COALESCE(n.start_line, -1) || '-' || \
         COALESCE(n.end_line, -1) \
         FROM nodes n JOIN symbols s ON s.id = n.symbol_id LEFT JOIN files f ON f.id = n.file_id",
        "SELECT 'edge ' || ss.symbol || ' -> ' || ts.symbol || ' ' || e.kind FROM edges e \
         JOIN nodes sn ON sn.id = e.source JOIN symbols ss ON ss.id = sn.symbol_id \
         JOIN nodes tn ON tn.id = e.target JOIN symbols ts ON ts.id = tn.symbol_id",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let found = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.extend(found.map(Result::unwrap));
    }
    rows.sort();
    rows
}

#[test]
fn a_full_index_records_the_coordinate_and_status_reports_it() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path(), "example.org/team/app");
    let engine = Engine::start(tmp.path()).unwrap();
    assert_eq!(engine.status().project_id, None, "nothing recorded before an index");
    assert!(
        !serde_json::to_string(&engine.status()).unwrap().contains("project_id"),
        "an unrecorded coordinate is elided, so a never-indexed status is unchanged"
    );
    engine.index();
    assert_eq!(engine.status().project_id.as_deref(), Some("example.org/team/app"));
}

/// **A changed coordinate re-renders every symbol and moves nothing else**
/// (S-703, which reverses S-702's "no symbol changes"). Re-indexing the same
/// tree under a different `[project] id` stores the same node and edge rows
/// with the coordinate in their symbols replaced, and records the coordinate
/// now in force under both keys.
#[test]
fn a_changed_coordinate_re_renders_every_symbol_and_moves_nothing_else() {
    let tmp = TempDir::new().unwrap();
    fixture(tmp.path(), "example.org/team/app");
    {
        let engine = Engine::start(tmp.path()).unwrap();
        engine.index();
    }
    let before = node_and_edge_rows(tmp.path());
    assert!(before.iter().any(|r| r.starts_with("edge ")), "the fixture has edges: {before:#?}");
    assert!(
        before.iter().all(|r| r.contains("logos . example.org/team/app . ")),
        "every symbol carries the coordinate: {before:#?}"
    );

    write(tmp.path(), ".logos/config.toml", "[project]\nid = \"example.org/team/renamed\"\n");
    let engine = Engine::start(tmp.path()).unwrap();
    engine.index();
    assert_eq!(
        engine.status().project_id.as_deref(),
        Some("example.org/team/renamed"),
        "the next full index records the coordinate now in force"
    );
    drop(engine);
    let renamed: Vec<String> = before
        .iter()
        .map(|r| r.replace("logos . example.org/team/app . ", "logos . example.org/team/renamed . "))
        .collect();
    assert_eq!(node_and_edge_rows(tmp.path()), renamed, "the same rows under the new coordinate");
}
