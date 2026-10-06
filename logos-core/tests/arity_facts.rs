//! A callable's parameter range, a call's argument count and a Rust `impl`
//! function's takes-`self` fact are persisted (S-591, CR-190, CR-200,
//! FR-EX-32, NFR-RA-06) — exercised end to end through the public [`Engine`]
//! façade against temp-directory fixtures.
//!
//! The facts ride beside the node (`nodes.param_min`, `param_max`,
//! `takes_self`) and on the ledger row (`unresolved_refs.arg_count`, part of the
//! row's identity), all added by migration 33. Nothing binds on them yet, so
//! every edge is the one the facts' absence produced. A one-file edit re-derives
//! them through sync to exactly what a fresh index of the edited tree records.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite here: a fixture tree checked into this repository would be
//! indexed into its own graph.

#![cfg(all(feature = "lang-rust", feature = "lang-kotlin", feature = "lang-python"))]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ParamRange};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn tree(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().unwrap();
    for (rel, text) in files {
        write(tmp.path(), rel, text);
    }
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// A node's label: `file:name@line`.
fn labels(rt: &Runtime) -> HashMap<NodeId, String> {
    rt.submit_read(|store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .map(|n| {
                let file = n.file_path.unwrap_or_default();
                (n.id, format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
            })
            .collect())
    })
    .expect("read runs")
}

/// Every persisted arity fact as `label → (range, takes self)`.
fn arities(rt: &Runtime) -> BTreeMap<String, (Option<ParamRange>, Option<bool>)> {
    let label = labels(rt);
    rt.submit_read(|store| store.node_arities())
        .expect("read runs")
        .into_iter()
        .map(|(id, range, takes_self)| (label[&id].clone(), (range, takes_self)))
        .collect()
}

/// Every `Calls` ledger row as `(source label, target, argument count,
/// resolved)`, sorted.
fn call_rows(rt: &Runtime) -> Vec<(String, String, Option<u32>, bool)> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| {
                    let file = n.file_path.unwrap_or_default();
                    (n.symbol.as_str().to_string(), format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0)))
                })
                .collect())
        })
        .expect("read runs");
    let mut rows: Vec<(String, String, Option<u32>, bool)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls)
        .filter_map(|r| Some((by_symbol.get(&r.source_symbol)?.clone(), r.target, r.arg_count, r.resolved)))
        .collect();
    rows.sort();
    rows
}

/// Every bound edge as `(source label, target label, kind)`, sorted.
fn edges(rt: &Runtime) -> Vec<(String, String, i32)> {
    let label = labels(rt);
    let mut out: Vec<(String, String, i32)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .map(|e| (label[&e.source].clone(), label[&e.target].clone(), e.kind.as_i32()))
        .collect();
    out.sort();
    out
}

fn range(min: u32, max: Option<u32>) -> Option<ParamRange> {
    Some(ParamRange { min, max })
}

const LIB_RS: &str = "pub struct A;
impl A {
    pub fn by_ref(&self, x: i32) {}
    pub fn new() -> Self { A }
}
pub fn free(a: i32, b: i32) {}
pub fn caller(a: A) {
    free(1, 2);
    free(1, 2, 3);
    a.by_ref(1);
    A::new();
}
";

const C_KT: &str = "class C {
    fun d(a: Int, b: Int = 2) {}
    fun caller() { d(1); d(1, 2); k(1) { it } }
}
";

const M_PY: &str = "class M:
    def m(self, a, *rest):
        self.m(1, *rest)
";

const FILES: [(&str, &str); 3] = [("src/lib.rs", LIB_RS), ("src/C.kt", C_KT), ("pkg/m.py", M_PY)];

/// The facts are persisted beside each node and on each ledger row: a receiver
/// is not counted, a default raises only the maximum, a variadic makes it
/// unbounded, and a Rust impl function records whether it takes `self`.
#[test]
fn indexing_persists_each_callables_range_and_each_calls_argument_count() {
    let tmp = tree(&FILES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let expected: BTreeMap<String, (Option<ParamRange>, Option<bool>)> = [
        ("pkg/m.py:m@2", (range(1, None), None)),
        ("src/C.kt:caller@3", (range(0, Some(0)), None)),
        ("src/C.kt:d@2", (range(1, Some(2)), None)),
        ("src/lib.rs:by_ref@3", (range(1, Some(1)), Some(true))),
        ("src/lib.rs:caller@7", (range(1, Some(1)), None)),
        ("src/lib.rs:free@6", (range(2, Some(2)), None)),
        ("src/lib.rs:new@4", (range(0, Some(0)), Some(false))),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    assert_eq!(arities(rt), expected);

    let counts: Vec<(String, String, Option<u32>)> =
        call_rows(rt).into_iter().map(|(source, target, count, _)| (source, target, count)).collect();
    for row in [
        ("src/lib.rs:caller@7", "free", Some(2)),
        ("src/lib.rs:caller@7", "free", Some(3)),
        ("src/lib.rs:caller@7", "A::by_ref", Some(1)),
        ("src/lib.rs:caller@7", "A::new", Some(0)),
        ("src/C.kt:caller@3", "d", Some(1)),
        ("src/C.kt:caller@3", "d", Some(2)),
        ("src/C.kt:caller@3", "k", Some(2)),
        ("pkg/m.py:m@2", "m", None),
    ] {
        let row = (row.0.to_string(), row.1.to_string(), row.2);
        assert!(counts.contains(&row), "{row:?} recorded in {counts:?}");
    }
}

/// Nothing binds on the facts yet: `free(1, 2)` and `free(1, 2, 3)` are two
/// ledger rows and both bind the one `free`, so the caller keeps exactly the
/// one `Calls` edge it had.
#[test]
fn two_counts_of_one_call_are_two_rows_and_one_edge() {
    let tmp = tree(&FILES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let free_rows: Vec<(Option<u32>, bool)> = call_rows(rt)
        .into_iter()
        .filter(|(source, target, _, _)| source == "src/lib.rs:caller@7" && target == "free")
        .map(|(_, _, count, resolved)| (count, resolved))
        .collect();
    assert_eq!(free_rows, vec![(Some(2), true), (Some(3), true)], "both counts bind, arity unread");
    let free_edges = edges(rt)
        .into_iter()
        .filter(|(s, t, k)| s == "src/lib.rs:caller@7" && t == "src/lib.rs:free@6" && *k == EdgeKind::Calls.as_i32())
        .count();
    assert_eq!(free_edges, 1, "one edge, as before the count split the row");
}

/// A one-file edit that drops a receiver and changes a call's count re-derives
/// the facts through sync to exactly what a fresh index of the edited tree
/// records — nodes' facts, ledger rows and edges alike.
#[test]
fn a_synced_edit_matches_a_fresh_reindex() {
    let tmp = tree(&FILES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let edited = LIB_RS
        .replace("pub fn by_ref(&self, x: i32) {}", "pub fn by_ref(x: i32, y: i32) {}")
        .replace("    free(1, 2, 3);\n", "    free(1);\n");
    write(tmp.path(), "src/lib.rs", &edited);
    let changed: Vec<PathBuf> = vec!["src/lib.rs".into()];
    engine.sync(&changed);
    assert_eq!(
        arities(rt)["src/lib.rs:by_ref@3"],
        (range(2, Some(2)), Some(false)),
        "the dropped receiver is re-derived on sync"
    );

    let files = [("src/lib.rs", edited.as_str()), ("src/C.kt", C_KT), ("pkg/m.py", M_PY)];
    let fresh_tmp = tree(&files);
    let fresh = index(fresh_tmp.path());
    let fresh_rt = fresh.runtime().unwrap();
    assert_eq!(arities(rt), arities(fresh_rt), "sync ≡ reindex: node facts");
    assert_eq!(call_rows(rt), call_rows(fresh_rt), "sync ≡ reindex: ledger rows");
    assert_eq!(edges(rt), edges(fresh_rt), "sync ≡ reindex: edges");
}
