//! A Rust call on a proven receiver binds among its type's methods (S-588,
//! CR-188, FR-RS-42, FR-RS-11, NFR-RA-05, NFR-RA-06) — exercised end to end
//! through the public [`Engine`] façade against the SHIPPED Rust
//! `references.scm` and `plugin.toml`.
//!
//! S-587 records `x.f()` on a receiver whose type the file proves as the
//! Path-form `T::f` of shape `other`, with the wrappers peeled to reach `T`.
//! The binder reads `T` through the caller file's `use` declarations to one
//! type declared in the repository — in the caller's crate or another — and
//! binds exactly one of that type's methods, an inherent one before a trait
//! impl's. A method the peeled wrapper provides itself, and a type the
//! repository does not declare, bind nothing.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.
//!
//! [`Engine`]: logos_core::Engine

#![cfg(feature = "lang-rust")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{discover, workspace_status, EngineRegistry, RegistryMode};
use logos_core::graph_store::NewUnresolvedRef;
use logos_core::model::{EdgeKind, NodeId, RefForm};
use logos_core::models::{CallResidueReason as R, LanguageResolution, ResidueScope};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

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

/// The bound `Calls` edges of `files`, keyed by source label.
fn edges_by_source(files: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    let tmp = tree(files);
    let engine = index(tmp.path());
    by_source(call_edges(engine.runtime().unwrap()))
}

fn by_source(edges: Vec<(String, String)>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (from, to) in edges {
        out.entry(from).or_default().push(to);
    }
    out
}

/// The targets the caller labelled `source` binds, empty when none.
fn targets<'a>(edges: &'a BTreeMap<String, Vec<String>>, source: &str) -> &'a [String] {
    edges.get(source).map_or(&[], Vec::as_slice)
}

// ── Every proof form, two types sharing a method name ───────────────────────

/// `A` and `B` both define `f`; each form proves one of them, in the file
/// that declares both (S-587's constructor and own-field proofs are
/// module-local).
const FORMS: &str = "\
use std::rc::Rc;
pub struct A;
pub struct B;
impl A { pub fn new() -> Self { A } pub fn f(&self) {} }
impl B { pub fn new() -> B { B } pub fn f(&self) {} }
pub struct Holder { a: A, b: Box<B> }
pub fn param_a(x: &A) { x.f(); }
pub fn param_b(x: &mut B) { x.f(); }
pub fn let_a() { let x: A = make(); x.f(); }
pub fn let_b() { let x: Rc<B> = make(); x.f(); }
pub fn ctor_a() { let x = A::new(); x.f(); }
pub fn ctor_b() { let x = B::new(); x.f(); }
pub fn literal_a() { let x = A {}; x.f(); }
pub fn literal_b() { let x = B {}; x.f(); }
impl Holder { pub fn own_a(&self) { self.a.f(); } pub fn own_b(&self) { self.b.f(); } }
pub fn unproven(xs: Vec<A>) { for x in xs { x.f(); } }
";

#[test]
fn every_proof_form_binds_the_proven_types_method_and_only_it() {
    let edges = edges_by_source(&[("src/lib.rs", "pub mod forms;\n"), ("src/forms.rs", FORMS)]);
    let (a_f, b_f) = ("src/forms.rs:f@4", "src/forms.rs:f@5");
    for (caller, line, f) in [
        ("param_a", 7, a_f),
        ("param_b", 8, b_f),
        ("let_a", 9, a_f),
        ("let_b", 10, b_f),
        ("ctor_a", 11, a_f),
        ("ctor_b", 12, b_f),
        ("literal_a", 13, a_f),
        ("literal_b", 14, b_f),
        ("own_a", 15, a_f),
        ("own_b", 15, b_f),
    ] {
        let source = format!("src/forms.rs:{caller}@{line}");
        let bound: Vec<&String> = targets(&edges, &source).iter().filter(|t| t.contains(":f@")).collect();
        assert_eq!(bound, [f], "{caller}: {edges:#?}");
    }
    // An unproven receiver (a loop variable) stays unbound.
    assert!(targets(&edges, "src/forms.rs:unproven@16").is_empty(), "{edges:#?}");
}

// ── Inherent before trait; wrappers; external types ─────────────────────────

const METHODS: &str = "\
use std::sync::Arc;
pub struct Store;
pub trait Tr { fn put(&self); fn only(&self); }
pub trait Open { fn close(&self); }
pub trait Shut { fn close(&self); }
impl Store { pub fn put(&self) {} pub fn len(&self) -> usize { 0 } pub fn kind(&self) {} }
impl Tr for Store { fn put(&self) {} fn only(&self) {} }
impl Open for Store { fn close(&self) {} }
impl Shut for Store { fn close(&self) {} }
impl Clone for Store { fn clone(&self) -> Self { Store } }
pub fn inherent(x: &Store) { x.put(); }
pub fn trait_only(x: &Store) { x.only(); }
pub fn two_traits(x: &Store) { x.close(); }
pub fn through_arc(x: Arc<Store>) { x.clone(); x.only(); }
pub fn through_box(x: Box<Store>) { x.clone(); }
pub fn through_ref(x: &Store) { x.clone(); }
pub fn std_string(x: String) { x.len(); }
pub fn std_vec(x: Vec<Store>) { x.len(); }
pub fn std_error(x: std::io::Error) { x.kind(); }
pub fn std_option(x: Option<Store>) { x.only(); }
";

#[test]
fn an_inherent_method_outranks_a_trait_impls_and_wrapper_and_std_methods_bind_nothing() {
    let edges = edges_by_source(&[("src/lib.rs", "pub mod methods;\n"), ("src/methods.rs", METHODS)]);
    let at = |caller: &str, line: u32| targets(&edges, &format!("src/methods.rs:{caller}@{line}")).to_vec();
    // Inherent `put` (line 6) over the trait impl's (line 7); the trait impl's
    // `only` where no inherent one exists; nothing between two trait impls.
    assert_eq!(at("inherent", 11), ["src/methods.rs:put@6"]);
    assert_eq!(at("trait_only", 12), ["src/methods.rs:only@7"]);
    assert!(at("two_traits", 13).is_empty(), "{edges:#?}");
    // `x.clone()` through `Arc`/`Box` is the wrapper's own; a method the
    // wrapper does not provide reaches `Store`'s; through a reference,
    // `clone` is `Store`'s.
    assert_eq!(at("through_arc", 14), ["src/methods.rs:only@7"]);
    assert!(at("through_box", 15).is_empty(), "{edges:#?}");
    assert_eq!(at("through_ref", 16), ["src/methods.rs:clone@10"]);
    // A std receiver never reaches the crate's same-named `len`/`kind`/`only`.
    for (caller, line) in [("std_string", 17), ("std_vec", 18), ("std_error", 19), ("std_option", 20)] {
        assert!(at(caller, line).is_empty(), "{caller}: {edges:#?}");
    }
}

#[test]
fn a_method_whose_impl_sits_in_another_module_binds() {
    // The common split layout: `X` declared in `a.rs`, its methods in `c.rs`.
    let edges = edges_by_source(&[
        ("src/lib.rs", "pub mod a;\npub mod c;\npub mod user;\n"),
        ("src/a.rs", "pub struct X;\n"),
        ("src/c.rs", "impl crate::a::X {\n    pub fn m(&self) {}\n}\n"),
        ("src/user.rs", "use crate::a::X;\npub fn f(x: &X) { x.m(); }\n"),
    ]);
    assert_eq!(targets(&edges, "src/user.rs:f@2"), ["src/c.rs:m@2"]);
}

// ── Across crates, through `use` ────────────────────────────────────────────

/// A `cli` crate over an `app-core` crate, and a third crate declaring a
/// same-named `Store` with the same method.
fn workspace(cli_main: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Cargo.toml", "[workspace]\nmembers = [\"app-core\", \"cli\", \"other\"]\n".to_string()),
        ("app-core/src/lib.rs", "pub mod store;\n".to_string()),
        ("app-core/src/store.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) -> u8 { 0 }\n}\n".to_string()),
        ("other/src/lib.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) -> u8 { 1 }\n}\n".to_string()),
        ("cli/src/main.rs", cli_main.to_string()),
    ]
}

fn edges_of_workspace(cli_main: &str) -> BTreeMap<String, Vec<String>> {
    let files = workspace(cli_main);
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    edges_by_source(&borrowed)
}

#[test]
fn a_cli_crate_binds_a_use_imported_type_of_a_core_crate_and_never_a_same_named_one() {
    let edges = edges_of_workspace("use app_core::store::Store;\npub fn run(x: &Store) -> u8 { x.get() }\n");
    assert_eq!(targets(&edges, "cli/src/main.rs:run@2"), ["app-core/src/store.rs:get@3"]);
    let edges = edges_of_workspace("use other::Store;\npub fn run(x: &Store) -> u8 { x.get() }\n");
    assert_eq!(targets(&edges, "cli/src/main.rs:run@2"), ["other/src/lib.rs:get@3"]);
    // No `use` names `Store`: neither crate's is a candidate.
    let edges = edges_of_workspace("pub fn run(x: &Store) -> u8 { x.get() }\n");
    assert!(targets(&edges, "cli/src/main.rs:run@1").is_empty(), "{edges:#?}");
    // A written-out path names it without a `use`.
    let edges = edges_of_workspace("pub fn run(x: &app_core::store::Store) -> u8 { x.get() }\n");
    assert_eq!(targets(&edges, "cli/src/main.rs:run@1"), ["app-core/src/store.rs:get@3"]);
}

#[test]
fn a_type_the_core_crate_re_exports_from_its_root_binds_across_the_crates() {
    // `use app_core::Store` names `store::Store` through the root's
    // `pub use store::Store;` — the shape this repository's `cli` uses for
    // `logos_core::Engine`.
    let mut files = workspace("use app_core::Store;\npub fn run(x: &Store) -> u8 { x.get() }\n");
    files[1].1 = "pub mod store;\npub use store::Store;\n".to_string();
    let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let edges = edges_by_source(&borrowed);
    assert_eq!(targets(&edges, "cli/src/main.rs:run@2"), ["app-core/src/store.rs:get@3"]);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical, and the synced one's
/// edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        write(tmp_a.path(), rel, text);
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);
    let rt_a = engine_a.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());

    assert_eq!(
        graph_fingerprint(rt_a),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    by_source(call_edges(rt_a))
}

const CALLER: &str = "use crate::store::Store;\npub fn run(x: &Store) { x.get(); }\n";
/// [`CALLER`] importing the trait `G` too, so its method is in scope (S-607).
const TRAIT_CALLER: &str = "use crate::store::{Store, G};\npub fn run(x: &Store) { x.get(); }\n";
const WITHOUT_GET: &str = "pub struct Store;\nimpl Store {\n    pub fn put(&self) {}\n}\n";
const WITH_GET: &str = "pub struct Store;\nimpl Store {\n    pub fn put(&self) {}\n    pub fn get(&self) {}\n}\n";
const WITH_TRAIT_GET: &str = "pub struct Store;\npub trait G { fn get(&self); }\nimpl G for Store {\n    fn get(&self) {}\n}\n";

#[test]
fn the_callees_type_gaining_or_losing_the_method_rebinds_on_sync() {
    let lib = ("src/lib.rs", "pub mod store;\npub mod caller;\n");
    let caller = ("src/caller.rs", CALLER);
    // Gaining `get` binds the call, without touching the caller's file.
    let edges = synced_equals_reindexed(&[lib, caller, ("src/store.rs", WITHOUT_GET)], &[("src/store.rs", WITH_GET)]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/store.rs:get@4"]);
    // Losing it unbinds the call.
    let edges = synced_equals_reindexed(&[lib, caller, ("src/store.rs", WITH_GET)], &[("src/store.rs", WITHOUT_GET)]);
    assert!(targets(&edges, "src/caller.rs:run@2").is_empty(), "{edges:#?}");
    // A trait impl's `get` replacing the inherent one binds through the impl
    // where the caller imports the trait — and nowhere else (S-607).
    let traited = ("src/caller.rs", TRAIT_CALLER);
    let edges = synced_equals_reindexed(&[lib, traited, ("src/store.rs", WITH_GET)], &[("src/store.rs", WITH_TRAIT_GET)]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/store.rs:get@4"]);
    let edges = synced_equals_reindexed(&[lib, caller, ("src/store.rs", WITH_GET)], &[("src/store.rs", WITH_TRAIT_GET)]);
    assert!(targets(&edges, "src/caller.rs:run@2").is_empty(), "{edges:#?}");
}

#[test]
fn the_callers_use_changing_type_rebinds_on_sync() {
    let lib = ("src/lib.rs", "pub mod a;\npub mod b;\npub mod caller;\n");
    let a = ("src/a.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n");
    let b = ("src/b.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n");
    let via_a = ("src/caller.rs", "use crate::a::Store;\npub fn run(x: &Store) { x.get(); }\n");
    let via_b = ("src/caller.rs", "use crate::b::Store;\npub fn run(x: &Store) { x.get(); }\n");
    let edges = synced_equals_reindexed(&[lib, a, b, via_a], &[via_b]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/b.rs:get@3"]);
}

#[test]
fn a_test_module_reaches_its_parents_imported_type_through_its_glob() {
    // `use super::*` in a test module of its own file brings in what the
    // parent module imports — the shape of this repository's
    // `#[cfg(test)] mod tests;` — and `crate::Store` reads the root's
    // re-export. Under a named crate (`app`), as `crate` is otherwise also the
    // crate's own key.
    let edges = edges_by_source(&[
        ("app/src/lib.rs", "pub mod store;\npub mod user;\npub mod root_user;\npub use store::Store;\n"),
        ("app/src/store.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n"),
        (
            "app/src/user.rs",
            "use crate::store::Store;\npub fn by_parent(x: &Store) { x.get(); }\n#[cfg(test)]\nmod tests;\n",
        ),
        ("app/src/root_user.rs", "pub fn by_root(x: &crate::Store) { x.get(); }\n"),
        ("app/src/user/tests.rs", "use super::*;\nfn probe(x: &Store) { x.get(); }\n"),
    ]);
    assert_eq!(targets(&edges, "app/src/user.rs:by_parent@2"), ["app/src/store.rs:get@3"]);
    assert_eq!(targets(&edges, "app/src/root_user.rs:by_root@1"), ["app/src/store.rs:get@3"]);
    assert_eq!(targets(&edges, "app/src/user/tests.rs:probe@2"), ["app/src/store.rs:get@3"]);
}

#[test]
fn a_re_export_added_or_removed_on_sync_rebinds_the_importing_crates_call() {
    let files = workspace("use app_core::Store;\npub fn run(x: &Store) -> u8 { x.get() }\n");
    let initial: Vec<(&str, &str)> = files.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let with_reexport = ("app-core/src/lib.rs", "pub mod store;\npub use store::Store;\n");
    let edges = synced_equals_reindexed(&initial, &[with_reexport]);
    assert_eq!(targets(&edges, "cli/src/main.rs:run@2"), ["app-core/src/store.rs:get@3"]);
    let mut exported = initial.clone();
    exported[1] = with_reexport;
    let edges = synced_equals_reindexed(&exported, &[("app-core/src/lib.rs", "pub mod store;\n")]);
    assert!(targets(&edges, "cli/src/main.rs:run@2").is_empty(), "{edges:#?}");
}

#[test]
fn a_capture_row_never_lends_its_file_to_the_re_exporting_module() {
    // `config/mod.rs` re-exports `Config` and itself calls into `settings.rs`.
    // Syncing `settings.rs` captures that inbound call as a row filed under
    // `settings.rs` whose source is in `config/mod.rs`; the re-export must
    // still be read from `config/mod.rs`'s own `use`s, as a cold index reads
    // it.
    let initial = [
        ("app/src/lib.rs", "pub mod config;\npub mod user;\n"),
        ("app/src/config/mod.rs", "mod settings;\npub use settings::Config;\npub fn boot() { settings::touch(); }\n"),
        (
            "app/src/config/settings.rs",
            "pub struct Config;\nimpl Config {\n    pub fn get(&self) {}\n}\npub fn touch() {}\n",
        ),
        ("app/src/user.rs", "use crate::config::Config;\npub fn run(x: &Config) { x.get(); }\n"),
    ];
    let edited = (
        "app/src/config/settings.rs",
        "pub struct Config;\nimpl Config {\n    pub fn get(&self) {}\n    pub fn put(&self) {}\n}\npub fn touch() {}\n",
    );
    let edges = synced_equals_reindexed(&initial, &[edited]);
    assert_eq!(targets(&edges, "app/src/user.rs:run@2"), ["app/src/config/settings.rs:get@3"]);
}

#[test]
fn a_re_export_retargeted_on_sync_rebinds_the_importing_crates_call() {
    // The re-export names a module unrelated to the type, so no node the sync
    // touches spells `Store`: only the import's own name selects the row.
    let initial = [
        ("Cargo.toml", "[workspace]\nmembers = [\"app-core\", \"cli\"]\n"),
        ("app-core/src/lib.rs", "pub mod a;\npub mod b;\npub use a::Store;\n"),
        ("app-core/src/a.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) -> u8 { 0 }\n}\n"),
        ("app-core/src/b.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) -> u8 { 1 }\n}\n"),
        ("cli/src/main.rs", "use app_core::Store;\npub fn run(x: &Store) -> u8 { x.get() }\n"),
    ];
    let retargeted = ("app-core/src/lib.rs", "pub mod a;\npub mod b;\npub use b::Store;\n");
    let edges = synced_equals_reindexed(&initial, &[retargeted]);
    assert_eq!(targets(&edges, "cli/src/main.rs:run@2"), ["app-core/src/b.rs:get@3"]);
    // A glob of another crate's root reads none of its imports — the graph
    // cannot tell its `pub use`s from its private ones — so the call stays
    // unbound before and after, and sync still equals a cold index.
    let mut globbed = initial;
    globbed[4] = ("cli/src/main.rs", "use app_core::*;\npub fn run(x: &Store) -> u8 { x.get() }\n");
    let edges = synced_equals_reindexed(&globbed, &[retargeted]);
    assert!(targets(&edges, "cli/src/main.rs:run@2").is_empty(), "{edges:#?}");
}

#[test]
fn a_parent_import_retargeted_on_sync_rebinds_its_glob_test_modules_call() {
    let initial = [
        ("app/src/lib.rs", "pub mod a;\npub mod b;\npub mod user;\n"),
        ("app/src/a.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n"),
        ("app/src/b.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n"),
        ("app/src/user.rs", "use crate::a::Store;\npub fn keep() {}\n#[cfg(test)]\nmod tests;\n"),
        ("app/src/user/tests.rs", "use super::*;\nfn probe(x: &Store) { x.get(); }\n"),
    ];
    let edited = ("app/src/user.rs", "use crate::b::Store;\npub fn keep() {}\n#[cfg(test)]\nmod tests;\n");
    let edges = synced_equals_reindexed(&initial, &[edited]);
    assert_eq!(targets(&edges, "app/src/user/tests.rs:probe@2"), ["app/src/b.rs:get@3"]);
}

#[test]
fn a_glob_of_a_non_ancestor_module_never_brings_in_its_private_import() {
    // `helpers` imports a crate `String` privately; a glob of `helpers` does
    // not bring that import in, so `String` in `caller.rs` is the prelude's
    // and `x.len()` never reaches the crate type's `len`.
    let edges = edges_by_source(&[
        ("app/src/lib.rs", "pub mod text;\npub mod helpers;\npub mod caller;\n"),
        ("app/src/text.rs", "pub struct String;\nimpl String {\n    pub fn len(&self) -> usize { 0 }\n}\n"),
        ("app/src/helpers.rs", "use crate::text::String;\npub fn noop(_: &String) {}\n"),
        ("app/src/caller.rs", "use crate::helpers::*;\npub fn run(x: &String) -> usize { x.len() }\n"),
    ]);
    assert!(targets(&edges, "app/src/caller.rs:run@2").is_empty(), "{edges:#?}");
}

#[test]
fn an_inline_modules_use_never_retargets_the_files_own_type() {
    // `a.rs` declares `Store`; its inline test module imports `b`'s. The
    // file's ledger holds one scope for both, so the top-level `run` must read
    // its own module's declaration, and the test module's `probe` — whose
    // module declares none — the import.
    let a = "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\npub fn run(x: &Store) { x.get(); }\n\
             #[cfg(test)]\nmod tests {\n    use crate::b::Store;\n    fn probe(x: &Store) { x.get(); }\n}\n";
    let b = "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n";
    let edges = edges_by_source(&[("src/lib.rs", "pub mod a;\npub mod b;\n"), ("src/a.rs", a), ("src/b.rs", b)]);
    assert_eq!(targets(&edges, "src/a.rs:run@5"), ["src/a.rs:get@3"]);
    assert_eq!(targets(&edges, "src/a.rs:probe@9"), ["src/b.rs:get@3"]);
    // Two imports of one name in the file (top level and an inline module):
    // the scope cannot tell which is the caller's, so neither binds.
    let c = "use crate::b::Store;\npub fn run(x: &Store) { x.get(); }\n\
             #[cfg(test)]\nmod tests {\n    use crate::a::Store;\n    fn probe(x: &Store) { x.get(); }\n}\n";
    let edges = edges_by_source(&[("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n"), ("src/a.rs", a), ("src/b.rs", b), ("src/c.rs", c)]);
    assert!(targets(&edges, "src/c.rs:run@2").is_empty(), "{edges:#?}");
    assert!(targets(&edges, "src/c.rs:probe@6").is_empty(), "{edges:#?}");
}

#[test]
fn a_wrapper_associated_function_is_no_method_of_the_wrapper() {
    // `Arc::downgrade(&a)` takes no `self`, so `a.downgrade()` autoderefs to
    // `P`'s own method.
    let edges = edges_by_source(&[
        ("src/lib.rs", "pub mod p;\n"),
        ("src/p.rs", "use std::sync::Arc;\npub struct P;\nimpl P {\n    pub fn downgrade(&self) {}\n}\npub fn call(a: Arc<P>) { a.downgrade(); }\n"),
    ]);
    assert_eq!(targets(&edges, "src/p.rs:call@6"), ["src/p.rs:downgrade@4"]);
}

// ── The Rust row's call residue (S-589) ─────────────────────────────────────

/// One unbound proven call per reason, beside one that binds: `String` and the
/// `Arc`-own `clone` are `external-type`, a chain receiver proves nothing, two
/// in-scope trait impls' `twice` tie, `Solo` declares no `absent`, and `m` sits
/// only on `b::A`'s impl while the caller's `A` is `a::A` — another type, so
/// `a::A` has no `m` (S-607; 1.13.0 read the name the crate declares twice as
/// `type-ambiguous`). `plain` makes a path call whose head leaves the
/// repository (`external-type`) and a bare call naming nothing in scope
/// (`name-not-in-scope`).
const RESIDUE: [(&str, &str); 4] = [
    (
        "src/lib.rs",
        "use std::sync::Arc;
use crate::a::A;
pub mod a;
pub mod b;
pub mod c;
pub struct Solo;
impl Solo { pub fn run(&self) {} pub fn me(&self) -> &Solo { self } }
pub trait P { fn twice(&self); }
pub trait Q { fn twice(&self); }
impl P for Solo { fn twice(&self) {} }
impl Q for Solo { fn twice(&self) {} }
pub fn bound(x: &Solo) { x.run(); }
pub fn external(s: &String) { s.len(); }
pub fn wrapper(x: Arc<Solo>) { x.clone(); }
pub fn chain(x: &Solo) { x.me().run(); }
pub fn overload(x: &Solo) { x.twice(); }
pub fn missing(x: &Solo) { x.absent(); }
pub fn ambiguous(x: &A) { x.m(); }
pub fn plain() { Vec::<u8>::new(); nowhere(); }
",
    ),
    ("src/a.rs", "pub struct A;\n"),
    ("src/b.rs", "pub struct A;\n"),
    ("src/c.rs", "impl crate::b::A {\n    pub fn m(&self) {}\n}\n"),
];

// ── S-604: a method call binds only a callable that takes `self` ────────────

/// `P` has an inherent associated `name()` beside `Named`'s `name(&self)`
/// (line 6), and two genuine `pair(&self)`s, inherent (line 5) and `Pair`'s
/// (line 8); each proof form calls both. `Q`'s only `make` takes no `self`.
const SELF_FORMS: &str = "\
use std::rc::Rc;
pub trait Named { fn name(&self) -> &'static str; }
pub struct P;
pub trait Pair { fn pair(&self); }
impl P { pub fn new() -> Self { P } pub fn name() -> &'static str { \"p\" } pub fn pair(&self) {} }
impl Named for P { fn name(&self) -> &'static str { \"named\" } }
pub struct Q;
impl Pair for P { fn pair(&self) {} }
impl Q { pub fn make() -> Q { Q } }
pub struct Holder { p: P, q: Box<P> }
pub fn param(x: &P) { x.name(); x.pair(); }
pub fn typed_let() { let x: Rc<P> = make(); x.name(); x.pair(); }
pub fn ctor() { let x = P::new(); x.name(); x.pair(); }
pub fn literal() { let x = P {}; x.name(); x.pair(); }
impl Holder { pub fn own(&self) { self.p.name(); self.q.pair(); } }
pub fn only_assoc(x: &Q) { x.make(); }
";

#[test]
fn every_proof_form_binds_the_trait_method_over_an_associated_function_and_the_inherent_method_over_a_trait_one() {
    let files = [("src/lib.rs", "pub mod forms;\n"), ("src/forms.rs", SELF_FORMS)];
    let edges = edges_by_source(&files);
    for (caller, line) in [("param", 11), ("typed_let", 12), ("ctor", 13), ("literal", 14), ("own", 15)] {
        let source = format!("src/forms.rs:{caller}@{line}");
        let bound: Vec<&String> = targets(&edges, &source)
            .iter()
            .filter(|t| t.contains(":name@") || t.contains(":pair@"))
            .collect();
        assert_eq!(bound, ["src/forms.rs:name@6", "src/forms.rs:pair@5"], "{caller}: {edges:#?}");
    }
    // `Q`'s only `make` takes no `self`: `x.make()` binds nothing, and the Rust
    // row states why.
    assert!(targets(&edges, "src/forms.rs:only_assoc@16").is_empty(), "{edges:#?}");
    let tmp = tree(&files);
    let residue = rust_row(&index(tmp.path())).call_residue.expect("the rust row states its call residue");
    assert_eq!(residue.reasons.get(&R::SupertypeUnreached), Some(&1), "{residue:#?}");
}

#[test]
fn a_trait_impls_associated_function_never_takes_a_method_call() {
    // `Build::m` is an associated function, `Use::m` a method: `x.m()` calls
    // `Use`'s, which is no longer tied with the other trait impl's.
    let forms = "\
pub struct P;
pub trait Build { fn m() -> Self; }
pub trait Use { fn m(&self); }
impl Build for P { fn m() -> Self { P } }
impl Use for P { fn m(&self) {} }
pub fn param(x: &P) { x.m(); }
";
    let edges = edges_by_source(&[("src/lib.rs", "pub mod forms;\n"), ("src/forms.rs", forms)]);
    assert_eq!(targets(&edges, "src/forms.rs:param@6"), ["src/forms.rs:m@5"]);
}

const SELF_CALLER: &str = "use crate::store::{Store, G};\npub fn run(x: &Store) { x.get(); }\n";
const ASSOC_GET: &str =
    "pub struct Store;\npub trait G { fn get(&self); }\nimpl Store {\n    pub fn get() {}\n}\nimpl G for Store {\n    fn get(&self) {}\n}\n";
const METHOD_GET: &str =
    "pub struct Store;\npub trait G { fn get(&self); }\nimpl Store {\n    pub fn get(&self) {}\n}\nimpl G for Store {\n    fn get(&self) {}\n}\n";

#[test]
fn a_self_receiver_added_or_removed_on_sync_rebinds_the_call() {
    let lib = ("src/lib.rs", "pub mod store;\npub mod caller;\n");
    let caller = ("src/caller.rs", SELF_CALLER);
    // The inherent `get` gaining `&self` outranks the trait impl's, without
    // touching the caller's file…
    let edges = synced_equals_reindexed(&[lib, caller, ("src/store.rs", ASSOC_GET)], &[("src/store.rs", METHOD_GET)]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/store.rs:get@4"]);
    // …and losing it hands the call back to the trait impl's.
    let edges = synced_equals_reindexed(&[lib, caller, ("src/store.rs", METHOD_GET)], &[("src/store.rs", ASSOC_GET)]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/store.rs:get@7"]);
}

/// The Rust row of `status`.
fn rust_row(engine: &Engine) -> LanguageResolution {
    engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "rust")
        .expect("a rust row")
}

#[test]
fn the_rust_row_states_its_call_residue_by_reason_over_its_unbound_calls() {
    let tmp = tree(&RESIDUE);
    let engine = index(tmp.path());
    let row = rust_row(&engine);
    let residue = row.call_residue.expect("the rust row states its call residue");

    assert_eq!(
        residue.unbound,
        row.calls.references - row.calls.bound,
        "the denominator is the row's own unbound count"
    );
    let expected: BTreeMap<R, u64> = [
        (R::ExternalType, 3),
        (R::NameNotInScope, 1),
        (R::NoApplicableOverload, 0),
        (R::NoReceiverEvidence, 1),
        (R::NotACallable, 0),
        (R::OverloadAmbiguous, 1),
        (R::SupertypeUnreached, 2),
        (R::TypeAmbiguous, 0),
    ]
    .into_iter()
    .collect();
    assert_eq!(residue.reasons, expected, "{residue:#?}");
    assert_eq!(
        residue.unbound,
        residue.reasons.values().sum::<u64>() + residue.unclassified,
        "the reasons and `unclassified` partition the denominator"
    );
    // Every unbound Rust call carries a reason (S-607), the path call and the
    // bare call included: nothing is `unclassified`.
    assert_eq!(residue.unclassified, 0, "{residue:#?}");
    assert_eq!(residue.scope, ResidueScope::Repository);
    // The one bound call is in no figure of the residue.
    assert_eq!(
        targets(&by_source(call_edges(engine.runtime().unwrap())), "src/lib.rs:bound@12"),
        ["src/lib.rs:run@7"]
    );
}

/// A capture-before-delete row (`RefForm::Symbol`) sits under its target's file
/// and duplicates a call its source's own row records, so it is no call site:
/// planted unbound under a Rust file, it moves neither the row's counts nor any
/// figure of its residue (S-598).
#[test]
fn a_capture_before_delete_row_is_in_no_figure_of_the_rust_residue() {
    let tmp = tree(&RESIDUE);
    let engine = index(tmp.path());
    let before = rust_row(&engine);
    let rt = engine.runtime().unwrap();
    let caller = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .find(|n| n.name == "missing")
                .map(|n| n.symbol.as_str().to_string())
                .expect("the missing node"))
        })
        .unwrap();
    rt.submit_write(move |w| {
        w.insert_unresolved_ref(&NewUnresolvedRef {
            file_id: w.file_id("src/a.rs")?,
            source_symbol: &caller,
            target: "planted vanished target",
            alias: None,
            form: RefForm::Symbol,
            kind: EdgeKind::Calls,
            line: None,
            payload: None,
            receiver: None,
            peeled: None,
            arg_count: None,
            exported: None,
        })
    })
    .expect("plant the capture");
    let planted = rt
        .submit_read(|store| {
            Ok(store
                .unresolved_refs()?
                .iter()
                .filter(|r| r.form == RefForm::Symbol && !r.resolved)
                .count())
        })
        .unwrap();
    assert_eq!(planted, 1, "the capture is in the ledger, or this pins nothing");

    let after = rust_row(&engine);
    assert_eq!(after, before);
    assert!(after.call_residue.is_some_and(|r| r.unbound == 8));
}

fn git_init(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir member");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init {}", dir.display());
}

/// In `workspace status` the Rust row is workspace-scoped like every residue
/// row, yet none of its `external-type` rows moves: a Rust row names no
/// candidate type, so a receiver typed with another member's `Store` reads
/// `external-type` there exactly as it does alone, beside std's `String`.
#[test]
fn a_rust_rows_external_type_stays_external_in_a_workspace() {
    let ws = TempDir::new().unwrap();
    let lib = ws.path().join("lib");
    let app = ws.path().join("app");
    git_init(&lib);
    git_init(&app);
    write(&lib, "src/lib.rs", "pub struct Store;\nimpl Store {\n    pub fn len(&self) -> usize { 0 }\n}\n");
    write(
        &app,
        "src/lib.rs",
        "use lib::Store;\npub fn other_member(x: &Store) { x.len(); }\npub fn std_type(s: &String) { s.len(); }\n",
    );
    for member in [&lib, &app] {
        let _ = index(member).sync(&[] as &[PathBuf]);
    }
    let alone = rust_row(&Engine::start(&app).expect("engine starts"))
        .call_residue
        .expect("the rust row states its call residue");
    assert_eq!(alone.scope, ResidueScope::Repository);
    assert_eq!(alone.reasons[&R::ExternalType], 2, "{alone:#?}");
    assert!(!alone.reasons.contains_key(&R::TypeInAnotherMember));

    write(ws.path(), "logos.workspace.toml", "[workspace]\nname = \"w\"\nmembers = [\"lib\", \"app\"]\n");
    let federation = discover(ws.path()).expect("discovers").expect("a workspace");
    let registry = EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy);
    let residue = workspace_status(&registry)
        .members
        .iter()
        .find(|m| m.status.member == "app")
        .and_then(|m| m.status.result.as_ref())
        .and_then(|info| {
            info.resolution_by_language
                .iter()
                .find(|row| row.language == "rust")
                .and_then(|row| row.call_residue.clone())
        })
        .expect("app's rust residue");
    assert_eq!(residue.scope, ResidueScope::Workspace);
    assert_eq!(residue.reasons[&R::TypeInAnotherMember], 0, "{residue:#?}");
    assert_eq!(residue.reasons[&R::ExternalType], alone.reasons[&R::ExternalType]);
    assert_eq!(residue.unbound, alone.unbound);
}

// ── A renamed re-export: the row spells the new name, the binding reads the old ──
//
// `pub use crate::inner::Store as Db;` makes a proven `x.get()` on `x: &Db` the
// row `Db::get`, which binds through the re-export to `inner::Store`'s method.
// A change that spells only `Store` — the re-export chain's next hop
// retargeted, the impl header's import retargeted — still moves that binding;
// a second `Store` elsewhere in the crate does not, as the header names the
// `Store` it imports (S-607).

const RENAMED_LIB: (&str, &str) = (
    "src/lib.rs",
    "pub mod inner;\npub mod imp;\npub mod facade;\npub mod other;\npub mod caller;\n",
);
const RENAMED_INNER: (&str, &str) = ("src/inner.rs", "pub struct Store;\n");
/// `Store`'s only `get` lives outside `Store`'s module: its header names the
/// `Store` `imp.rs` imports, however many `Store`s the crate declares.
const RENAMED_IMP: (&str, &str) = (
    "src/imp.rs",
    "use crate::inner::Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n",
);
const RENAMED_FACADE: (&str, &str) = ("src/facade.rs", "pub use crate::inner::Store as Db;\n");
const RENAMED_CALLER: (&str, &str) = ("src/caller.rs", "use crate::facade::Db;\npub fn run(x: &Db) { x.get(); }\n");
const OTHER_PLAIN: (&str, &str) = ("src/other.rs", "pub fn unrelated() {}\n");
const OTHER_STORE: (&str, &str) = ("src/other.rs", "pub fn unrelated() {}\npub struct Store;\n");

#[test]
fn a_second_same_named_type_behind_a_renamed_re_export_keeps_the_binding_on_sync() {
    // 1.13.0 read `imp.rs`'s header by name, and a second `Store` in the crate
    // made its `get` `type-ambiguous`; the header resolves to the `Store` it
    // imports (S-607), so the call stays bound, on sync as on a cold index.
    let initial = [RENAMED_LIB, RENAMED_INNER, RENAMED_IMP, RENAMED_FACADE, RENAMED_CALLER, OTHER_PLAIN];
    let before = edges_by_source(&initial);
    assert_eq!(targets(&before, "src/caller.rs:run@2"), ["src/imp.rs:get@3"], "{before:#?}");
    let edges = synced_equals_reindexed(&initial, &[OTHER_STORE]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/imp.rs:get@3"]);
}

#[test]
fn the_impl_headers_import_retargeted_on_sync_moves_the_renamed_call() {
    // `imp.rs` importing `other`'s `Store` instead makes its `get` that type's:
    // the call through `Db` — `inner`'s `Store` — binds nothing, and binds again
    // once the import names `inner`'s.
    let imp_other = ("src/imp.rs", "use crate::other::Store;
impl Store {
    pub fn get(&self) {}
}
");
    let initial = [RENAMED_LIB, RENAMED_INNER, RENAMED_IMP, RENAMED_FACADE, RENAMED_CALLER, OTHER_STORE];
    let edges = synced_equals_reindexed(&initial, &[imp_other]);
    assert!(targets(&edges, "src/caller.rs:run@2").is_empty(), "{edges:#?}");
    let initial = [RENAMED_LIB, RENAMED_INNER, imp_other, RENAMED_FACADE, RENAMED_CALLER, OTHER_STORE];
    let edges = synced_equals_reindexed(&initial, &[RENAMED_IMP]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/imp.rs:get@3"]);
}

#[test]
fn the_second_hop_of_a_renamed_re_export_retargeted_on_sync_rebinds_the_call() {
    let initial = [
        ("src/lib.rs", "pub mod deep;\npub mod deep2;\npub mod inner;\npub mod facade;\npub mod caller;\n"),
        ("src/deep.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n"),
        ("src/deep2.rs", "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n"),
        ("src/inner.rs", "pub use crate::deep::Store;\n"),
        RENAMED_FACADE,
        RENAMED_CALLER,
    ];
    let before = edges_by_source(&initial);
    assert_eq!(targets(&before, "src/caller.rs:run@2"), ["src/deep.rs:get@3"], "{before:#?}");
    let edges = synced_equals_reindexed(&initial, &[("src/inner.rs", "pub use crate::deep2::Store;\n")]);
    assert_eq!(targets(&edges, "src/caller.rs:run@2"), ["src/deep2.rs:get@3"]);
}

// ── An inline module's re-export, in a file that also imports the name ──────
//
// One file holds one import scope: `a.rs`'s top-level `use crate::b::Store`
// and its inline `mod inner { pub use crate::c::Store; }` are both "the
// import of `Store`" there, and which one `a::inner` holds is unknown. So
// `use crate::a::inner::Store` binds nothing — never `b`'s method, whichever
// import comes first. Without the top-level one, `inner`'s re-export binds.

const INLINE_LIB: &str = "pub mod a;\npub mod b;\npub mod c;\npub mod user;\n";
const INLINE_STORE: &str = "pub struct Store;\nimpl Store {\n    pub fn get(&self) {}\n}\n";
const INLINE_USER: &str = "use crate::a::inner::Store;\npub fn run(x: &Store) { x.get(); }\n";

fn inline_reexport_edges(a: &str) -> BTreeMap<String, Vec<String>> {
    edges_by_source(&[
        ("src/lib.rs", INLINE_LIB),
        ("src/a.rs", a),
        ("src/b.rs", INLINE_STORE),
        ("src/c.rs", INLINE_STORE),
        ("src/user.rs", INLINE_USER),
    ])
}

#[test]
fn an_inline_modules_re_export_beside_a_top_level_import_of_the_name_binds_nothing() {
    let top_first = "use crate::b::Store;\npub fn keep(_: &Store) {}\npub mod inner {\n    pub use crate::c::Store;\n}\n";
    let inner_first = "pub mod inner {\n    pub use crate::c::Store;\n}\nuse crate::b::Store;\npub fn keep(_: &Store) {}\n";
    for (order, a) in [("top-level use first", top_first), ("inline module first", inner_first)] {
        let edges = inline_reexport_edges(a);
        assert!(targets(&edges, "src/user.rs:run@2").is_empty(), "{order}: {edges:#?}");
    }
    let alone = inline_reexport_edges("pub mod inner {\n    pub use crate::c::Store;\n}\n");
    assert_eq!(targets(&alone, "src/user.rs:run@2"), ["src/c.rs:get@3"], "{alone:#?}");
}
