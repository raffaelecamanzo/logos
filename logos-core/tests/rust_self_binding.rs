//! A Rust `self.m()` / `Self::m()` call binds through the enclosing impl's self
//! type (S-493, CR-159, FR-RS-11, FR-RS-06, NFR-RA-05, NFR-RA-06) — exercised
//! end to end through the public [`Engine`] façade against temp-directory
//! fixtures.
//!
//! Each impl method records its self type beside its node (`nodes.self_type`,
//! from the plugin query's `@symbol.self_type` capture); a `self.m()` inside it
//! is recorded as the Path-form `Self::m`, the row a written `Self::m()`
//! records; the binder binds it to the one `m` recorded for the caller's self
//! type in the caller's crate. Zero or two candidates stay unbound. Calls on any
//! other receiver, and calls inside a trait's default method, are recorded and
//! resolved exactly as before.
//!
//! Fixtures are written inline into temp directories, like every sibling binding
//! suite here: a `.rs` fixture tree checked into this repository would be indexed
//! into its own graph.

#![cfg(feature = "lang-rust")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, RefForm};
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

/// A node's label: `file:name@line` — the line tells two same-named methods of
/// one file apart.
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

/// Every bound `Calls` edge as `(source label, target label)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    let label = labels(rt);
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// The `Calls` ledger rows sourced in `file`: `(source label, target, form,
/// resolved)`, sorted.
fn call_rows(rt: &Runtime, file: &str) -> Vec<(String, String, RefForm, bool)> {
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| {
                    let file = n.file_path.unwrap_or_default();
                    let label = format!("{file}:{}@{}", n.name, n.start_line.unwrap_or(0));
                    (n.symbol.as_str().to_string(), label)
                })
                .collect())
        })
        .expect("read runs");
    let prefix = format!("{file}:");
    let mut rows: Vec<(String, String, RefForm, bool)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Calls && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.resolved))
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    rows
}

/// Every recorded self type as `label → self type`.
fn self_types(rt: &Runtime) -> BTreeMap<String, String> {
    let label = labels(rt);
    rt.submit_read(|store| store.node_self_types())
        .expect("read runs")
        .into_iter()
        .map(|(id, ty)| (label[&id].clone(), ty))
        .collect()
}

/// `is_dead` of every callable named `name`, by label.
fn dead(rt: &Runtime, name: &str) -> BTreeMap<String, Option<bool>> {
    let label = labels(rt);
    rt.submit_read(|store| store.annotation_nodes())
        .expect("read runs")
        .into_iter()
        .filter(|n| !n.derived && n.name == name)
        .map(|n| (label[&n.id].clone(), n.is_dead))
        .collect()
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

/// Two impl blocks in one file — `impl<M> A<M>` and `impl B` — each define and
/// self-call `helper`; `C` reaches its own `util` by a written `Self::util`,
/// beside a `D` that also defines `util`.
const TWO_IMPLS: &str = "\
pub struct A<M>(M);
pub struct B;
pub struct C;
pub struct D;

impl<M> A<M> {
    pub fn run(&self) {
        self.helper();
    }
    fn helper(&self) {}
}

impl B {
    pub fn run(&self) {
        self.helper();
    }
    fn helper(&self) {}
}

impl C {
    pub fn go(&self) {
        Self::util(self);
    }
    fn util(&self) {}
}

impl D {
    fn util(&self) {}
}

pub fn free() {}
";

#[test]
fn each_self_call_binds_to_its_own_impls_helper_never_the_siblings() {
    let tmp = tree(&[("src/lib.rs", TWO_IMPLS)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    // The self type rides beside each impl method; a free function has none.
    assert_eq!(
        self_types(rt),
        BTreeMap::from([
            ("src/lib.rs:run@7".to_string(), "A".to_string()),
            ("src/lib.rs:helper@10".to_string(), "A".to_string()),
            ("src/lib.rs:run@14".to_string(), "B".to_string()),
            ("src/lib.rs:helper@17".to_string(), "B".to_string()),
            ("src/lib.rs:go@21".to_string(), "C".to_string()),
            ("src/lib.rs:util@24".to_string(), "C".to_string()),
            ("src/lib.rs:util@28".to_string(), "D".to_string()),
        ]),
        "generics stripped (`impl<M> A<M>` → A); `free` records none"
    );

    // `self.helper()` is recorded as `Self::helper`, the row `Self::util()` is.
    assert_eq!(
        call_rows(rt, "src/lib.rs"),
        vec![
            ("src/lib.rs:go@21".to_string(), "Self::util".to_string(), RefForm::Path, true),
            ("src/lib.rs:run@14".to_string(), "Self::helper".to_string(), RefForm::Path, true),
            ("src/lib.rs:run@7".to_string(), "Self::helper".to_string(), RefForm::Path, true),
        ]
    );
    assert_eq!(
        call_edges(rt),
        vec![
            edge("src/lib.rs:go@21", "src/lib.rs:util@24"),
            edge("src/lib.rs:run@14", "src/lib.rs:helper@17"),
            edge("src/lib.rs:run@7", "src/lib.rs:helper@10"),
        ],
        "each call binds its own impl's method, never the sibling type's"
    );
    assert_eq!(
        dead(rt, "helper"),
        BTreeMap::from([
            ("src/lib.rs:helper@10".to_string(), Some(false)),
            ("src/lib.rs:helper@17".to_string(), Some(false)),
        ]),
        "neither helper is dead"
    );
    assert_eq!(
        dead(rt, "util"),
        BTreeMap::from([
            ("src/lib.rs:util@24".to_string(), Some(false)),
            ("src/lib.rs:util@28".to_string(), Some(true)),
        ]),
        "C's util is reached through `Self::`, D's is not"
    );
}

/// One crate whose `S` has a `twin` in two files and no `nowhere` at all; a
/// sibling crate declares a same-named `S` with the `elsewhere` the first crate
/// lacks, and both crates define `shared` on their own `S`.
const AMBIGUITY_AND_CRATES: [(&str, &str); 4] = [
        (
            "amb/src/lib.rs",
            "\
mod x;
mod y;
pub struct S;
impl S {
    pub fn go(&self) {
        self.twin();
        self.nowhere();
        self.elsewhere();
        self.shared();
    }
    fn shared(&self) {}
}
",
        ),
        ("amb/src/x.rs", "impl super::S {\n    fn twin(&self) {}\n}\n"),
        ("amb/src/y.rs", "impl crate::S {\n    fn twin(&self) {}\n}\n"),
        (
            "other/src/lib.rs",
            "\
pub struct S;
impl S {
    pub fn elsewhere(&self) {}
    pub fn shared(&self) {}
}
",
        ),
];

#[test]
fn zero_two_or_another_crates_candidates_stay_unbound() {
    let tmp = tree(&AMBIGUITY_AND_CRATES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    assert_eq!(
        call_rows(rt, "amb/src/lib.rs"),
        vec![
            ("amb/src/lib.rs:go@5".to_string(), "Self::elsewhere".to_string(), RefForm::Path, false),
            ("amb/src/lib.rs:go@5".to_string(), "Self::nowhere".to_string(), RefForm::Path, false),
            ("amb/src/lib.rs:go@5".to_string(), "Self::shared".to_string(), RefForm::Path, true),
            ("amb/src/lib.rs:go@5".to_string(), "Self::twin".to_string(), RefForm::Path, false),
        ],
        "two `twin`s across files and no `nowhere` stay unbound; another crate's \
         same-named `S` supplies no `elsewhere`"
    );
    let from_go: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(from, _)| from.starts_with("amb/src/lib.rs:go"))
        .collect();
    assert_eq!(
        from_go,
        vec![edge("amb/src/lib.rs:go@5", "amb/src/lib.rs:shared@11")],
        "`shared` binds to the caller's crate's `S`, never the other crate's"
    );
}

/// Receivers other than `self`, and a trait's default method body, are never
/// bound through the self type: a trait default body's `self.f()` is a plain
/// method call, and `self.field.f()` / `other.f()` — whose types this file
/// proves — are the Path-form `Inner::helper` of shape `other` (S-587), which
/// binds nothing.
#[test]
fn other_receivers_and_trait_default_bodies_are_recorded_as_before() {
    let src = "\
pub struct Inner;
impl Inner {
    pub fn helper(&self) {}
}
pub struct Outer {
    field: Inner,
}
impl Outer {
    pub fn by_field(&self) {
        self.field.helper();
    }
    pub fn by_other(&self, other: &Inner) {
        other.helper();
    }
}
pub trait Greet {
    fn helper(&self) {}
    fn hello(&self) {
        self.helper();
    }
}
";
    let tmp = tree(&[("src/lib.rs", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let rows: Vec<(String, String, RefForm)> = call_rows(rt, "src/lib.rs")
        .into_iter()
        .map(|(from, target, form, _)| (from, target, form))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("src/lib.rs:by_field@9".to_string(), "Inner::helper".to_string(), RefForm::Path),
            ("src/lib.rs:by_other@12".to_string(), "Inner::helper".to_string(), RefForm::Path),
            ("src/lib.rs:hello@18".to_string(), "helper".to_string(), RefForm::Method),
        ],
        "`self.field.f()` and `other.f()` are retyped (S-587); a trait default body's \
         `self.f()` stays a bare Method-form row"
    );
    assert!(
        call_edges(rt).iter().all(|(from, _)| !from.contains(":by_field@") && !from.contains(":by_other@")),
        "a retyped receiver call binds nothing (S-587)"
    );
    assert!(
        !self_types(rt).keys().any(|k| k.contains(":hello@") || k.contains(":helper@17")),
        "a trait's default methods record no self type"
    );
}

/// A one-file edit that moves a candidate's self type re-binds through sync to
/// exactly what a fresh index of the edited tree yields, and leaves the ledger
/// rows of every untouched file as they were.
#[test]
fn a_synced_edit_matches_a_fresh_reindex_and_rederives_only_its_file() {
    let tmp = tree(&AMBIGUITY_AND_CRATES);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let rows_of = |rt: &Runtime, file: &str| -> Vec<(i64, String)> {
        let file = file.to_string();
        let mut rows: Vec<(i64, String)> = rt
            .submit_read(move |store| {
                let ids: HashMap<i64, String> = store
                    .indexed_files()?
                    .into_iter()
                    .map(|f| (f.id, f.path))
                    .collect();
                Ok(store
                    .unresolved_refs()?
                    .into_iter()
                    .filter(|r| r.file_id.and_then(|id| ids.get(&id)) == Some(&file))
                    .map(|r| (r.id, r.target))
                    .collect())
            })
            .expect("read runs");
        rows.sort();
        rows
    };
    let lib_rows_before = rows_of(rt, "amb/src/lib.rs");
    let other_rows_before = rows_of(rt, "other/src/lib.rs");

    // `y.rs`'s block now belongs to another type: `twin` has one candidate left.
    let edited = "pub struct T;\nimpl T {\n    fn twin(&self) {}\n}\n";
    write(tmp.path(), "amb/src/y.rs", edited);
    let changed: Vec<PathBuf> = vec!["amb/src/y.rs".into()];
    engine.sync(&changed);

    assert!(
        call_edges(rt).contains(&edge("amb/src/lib.rs:go@5", "amb/src/x.rs:twin@2")),
        "the remaining `twin` binds on sync"
    );
    assert_eq!(rows_of(rt, "amb/src/lib.rs"), lib_rows_before, "an untouched file's rows are not re-derived");
    assert_eq!(rows_of(rt, "other/src/lib.rs"), other_rows_before, "an untouched file's rows are not re-derived");

    let mut files = AMBIGUITY_AND_CRATES;
    files[2].1 = edited;
    let fresh_tmp = tree(&files);
    let fresh = index(fresh_tmp.path());
    let fresh_rt = fresh.runtime().unwrap();
    assert_eq!(call_edges(rt), call_edges(fresh_rt), "sync ≡ reindex: edges");
    for file in ["amb/src/lib.rs", "amb/src/x.rs", "amb/src/y.rs", "other/src/lib.rs"] {
        assert_eq!(call_rows(rt, file), call_rows(fresh_rt, file), "sync ≡ reindex: {file} rows");
    }
    assert_eq!(self_types(rt), self_types(fresh_rt), "sync ≡ reindex: self types");
}

/// The shape measured on this repository: two modules of one crate each declare
/// a private `UnionFind` whose `union` calls `self.find()`. The crate holds two
/// `UnionFind::find`s; each call binds the one of its own module.
#[test]
fn same_named_types_in_two_modules_each_bind_their_own_method() {
    let uf = "\
struct UnionFind;
impl UnionFind {
    fn find(&mut self) {}
    fn union(&mut self) {
        self.find();
    }
}
pub fn run() {
    UnionFind.union();
}
";
    let tmp = tree(&[("src/lib.rs", "pub mod a;\npub mod b;\n"), ("src/a.rs", uf), ("src/b.rs", uf)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let unions: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(from, _)| from.contains(":union@"))
        .collect();
    assert_eq!(
        unions,
        vec![
            edge("src/a.rs:union@4", "src/a.rs:find@3"),
            edge("src/b.rs:union@4", "src/b.rs:find@3"),
        ]
    );
}

/// A call that binds through the crate rung — the crate declares its self type
/// once, and the method lives in another module's impl — is re-selected on sync
/// when a second type of that name appears, though the edit spells no name the
/// call's row does, and its row reads unbound exactly as a cold index's does.
/// The edge it bound before is retracted (S-596, FR-SY-12), so the synced call
/// edges equal a cold index's; until S-596 the commit only flipped the row's
/// `resolved` flag and kept the edge.
#[test]
fn sync_equals_a_full_reindex_when_a_self_call_type_name_gains_a_twin() {
    let files = [
        ("src/lib.rs", "pub mod a;\npub mod c;\n"),
        ("src/a.rs", "pub struct X;\nimpl X {\n    pub fn m(&self) {}\n}\n"),
        ("src/c.rs", "impl crate::a::X {\n    pub fn go(&self) {\n        self.m();\n    }\n}\n"),
    ];
    let tmp = tree(&files);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let bound = edge("src/c.rs:go@2", "src/a.rs:m@3");
    assert!(
        call_edges(rt).contains(&bound),
        "one `X` in the crate: the impl in another module is the caller's type's"
    );

    let second = "pub struct X;\n";
    write(tmp.path(), "src/b.rs", second);
    engine.sync(&["src/b.rs".into()]);
    let mut edited = files.to_vec();
    edited.push(("src/b.rs", second));
    let fresh_tmp = tree(&edited);
    let fresh = index(fresh_tmp.path());
    let fresh_rt = fresh.runtime().unwrap();

    // The selection: the row is re-bound on sync and reads as a cold index's.
    let unbound = vec![("src/c.rs:go@2".to_string(), "Self::m".to_string(), RefForm::Path, false)];
    assert_eq!(call_rows(fresh_rt, "src/c.rs"), unbound, "two `X`s: the caller's module decides, and it holds no `m`");
    assert_eq!(call_rows(rt, "src/c.rs"), unbound, "sync re-selects the row and re-binds it to nothing");
    assert!(!call_edges(fresh_rt).contains(&bound), "a cold index binds no edge");
    assert_eq!(call_edges(rt), call_edges(fresh_rt));
}

/// A foreign type's impl never binds a self call to a crate type of the same
/// name (NFR-RA-05) — the shapes the S-493 review reproduced. `impl Ext for
/// std::io::Error` records no self type, so its `self.kind()` stays the plain
/// method call it always was; `use std::io::Error; impl Ext2 for Error` records
/// `Error` but the binder sees the foreign import; `impl Total for Vec<u8>`
/// names a type the crate does not declare, whose inherent `len` the graph
/// cannot see. The crate's own `Error::kind` gains no caller from any of them.
#[test]
fn a_foreign_self_type_never_binds_to_a_crate_type_of_its_name() {
    let tmp = tree(&[
        ("src/lib.rs", "pub mod error;\npub mod ext;\npub mod ext2;\npub mod vecs;\n"),
        (
            "src/error.rs",
            "pub struct Error {\n    k: u8,\n}\nimpl Error {\n    pub fn kind(&self) -> u8 {\n        self.k\n    }\n}\n",
        ),
        (
            "src/ext.rs",
            "\
pub trait IoErrorExt {
    fn is_timeout(&self) -> bool;
}
impl IoErrorExt for std::io::Error {
    fn is_timeout(&self) -> bool {
        self.kind() == std::io::ErrorKind::TimedOut
    }
}
",
        ),
        (
            "src/ext2.rs",
            "\
use std::io::Error;
pub trait Ext2 {
    fn timed(&self) -> bool;
}
impl Ext2 for Error {
    fn timed(&self) -> bool {
        self.kind() == std::io::ErrorKind::TimedOut
    }
}
",
        ),
        (
            "src/vecs.rs",
            "\
pub trait Size {
    fn len(&self) -> usize;
}
pub trait Total {
    fn total(&self) -> usize;
}
impl Size for Vec<String> {
    fn len(&self) -> usize {
        0
    }
}
impl Total for Vec<u8> {
    fn total(&self) -> usize {
        self.len()
    }
}
",
        ),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();

    let types = self_types(rt);
    assert!(
        !types.keys().any(|k| k.starts_with("src/ext.rs:")),
        "a header path-qualified outside the crate records no self type: {types:?}"
    );
    assert_eq!(types.get("src/ext2.rs:timed@6").map(String::as_str), Some("Error"));
    assert_eq!(types.get("src/vecs.rs:total@13").map(String::as_str), Some("Vec"));

    let rows: Vec<(String, String, RefForm, bool)> = ["src/ext.rs", "src/ext2.rs", "src/vecs.rs"]
        .iter()
        .flat_map(|f| call_rows(rt, f))
        .filter(|(_, target, _, _)| target.ends_with("kind") || target.ends_with("len"))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("src/ext.rs:is_timeout@5".to_string(), "kind".to_string(), RefForm::Method, false),
            ("src/ext2.rs:timed@6".to_string(), "Self::kind".to_string(), RefForm::Path, false),
            ("src/vecs.rs:total@13".to_string(), "Self::len".to_string(), RefForm::Path, false),
        ],
        "none of the three binds"
    );
    let into_kind: Vec<(String, String)> = call_edges(rt)
        .into_iter()
        .filter(|(_, to)| to.starts_with("src/error.rs:kind") || to.starts_with("src/vecs.rs:len"))
        .collect();
    assert!(into_kind.is_empty(), "no fabricated caller: {into_kind:?}");
}

/// The generic crate-scoped header shape (`impl<T> crate::m::G<T>`) records the
/// base name `G` and binds its self call — the one `symbols.scm` pattern no other
/// fixture here reaches.
#[test]
fn a_generic_crate_scoped_impl_records_its_base_name_and_binds() {
    let src = "\
pub mod m {
    pub struct G<T>(pub T);
}
impl<T> crate::m::G<T> {
    pub fn run(&self) {
        self.h();
    }
    fn h(&self) {}
}
";
    let tmp = tree(&[("src/lib.rs", src)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        self_types(rt),
        BTreeMap::from([
            ("src/lib.rs:run@5".to_string(), "G".to_string()),
            ("src/lib.rs:h@8".to_string(), "G".to_string()),
        ])
    );
    assert_eq!(call_edges(rt), vec![edge("src/lib.rs:run@5", "src/lib.rs:h@8")]);
}
