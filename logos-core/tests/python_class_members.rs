//! Python class members on sync (S-543, [FR-EX-20], [NFR-RA-06]): a class's
//! Methods, Fields and `self.x` Accesses are recorded when its file is
//! extracted, so an edit synced into an indexed tree must leave the graph a
//! fresh index of the edited tree builds.
//!
//! [FR-EX-20]: ../../docs/specs/requirements/FR-EX-20.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
#![cfg(feature = "lang-python")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::EdgeKind;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        let path = tmp.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound `Accesses` edge as `(method name, field name)`, sorted.
fn accesses(rt: &Runtime) -> Vec<(String, String)> {
    let mut pairs = rt
        .submit_read(|store| {
            let names: std::collections::HashMap<_, _> =
                store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect();
            Ok(store
                .all_edges()?
                .into_iter()
                .filter(|e| e.kind == EdgeKind::Accesses)
                .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
                .collect::<Vec<_>>())
        })
        .expect("read runs");
    pairs.sort();
    pairs
}

fn pair(method: &str, field: &str) -> (String, String) {
    (method.to_string(), field.to_string())
}

/// Before the edit `self.b` is read but never written, so it binds nothing;
/// the edit writes it in `__init__` and adds a method writing `self.a` again,
/// which declares no second `a`.
const BEFORE: &str = "\
class Box:
    def __init__(self):
        self.a = 1

    def total(self):
        return self.a + self.b
";
const AFTER: &str = "\
class Box:
    def __init__(self):
        self.a = 1
        self.b = 2

    def total(self):
        return self.a + self.b

    def reset(self):
        self.a = 0
";

#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    let tmp_a = tree(&[("app/box.py", BEFORE)]);
    let engine_a = index(tmp_a.path());
    assert_eq!(accesses(engine_a.runtime().unwrap()), [pair("__init__", "a"), pair("total", "a")]);
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    fs::write(tmp_a.path().join("app/box.py"), AFTER).unwrap();
    engine_a.sync(&[root_a.join("app/box.py")] as &[PathBuf]);
    let rt_a = engine_a.runtime().unwrap();

    let tmp_b = tree(&[("app/box.py", AFTER)]);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    assert_eq!(
        accesses(rt_a),
        [
            pair("__init__", "a"),
            pair("__init__", "b"),
            pair("reset", "a"),
            pair("total", "a"),
            pair("total", "b"),
        ]
    );
}
