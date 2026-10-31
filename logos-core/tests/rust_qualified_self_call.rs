//! A Rust `<T>::m()` call — a qualified self naming no trait — binds through
//! the one associated-item lookup and never a free function (S-627, CR-205,
//! FR-RS-47, FR-EX-34, FR-RS-43, NFR-RA-05) — exercised end to end through the
//! public [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! Rust queries and `plugin.toml`.
//!
//! - `<W>::new()` is the written path `W::new()`, outside and inside a macro:
//!   it binds `W`'s `new`, never a same-named free `fn new` (1.14.0 recorded
//!   the bare `new` and bound the free one).
//! - A bracket whose type is no repository type binds nothing, and reads
//!   `external-type`: `<Vec<u8>>::new()`, `<[u8]>::len(x)`, `<&str>::len(s)`,
//!   `<dyn Tr>::m(x)` — none reaches a same-named free `fn`.
//! - `<Q as Run>::go(q)` is S-608's and stays bound as it was.
//! - A `self`-taking `m` called as `<T>::m(x)` counts its receiver (FR-RS-43).
//! - A `gen`, `union` or `default` call inside a macro binds as outside it
//!   (S-628): the grammar tokenizes the three as keywords in a token tree.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite: a fixture tree checked into this repository would be indexed
//! into its own graph.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
use logos_core::models::{CallResidue, CallResidueReason as R};
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

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
    coordinate::pin(root);
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

/// Every bound `Calls` edge, source label → sorted target labels.
fn call_edges(rt: &Runtime) -> BTreeMap<String, Vec<String>> {
    let label = labels(rt);
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in rt.submit_read(|store| store.all_edges()).expect("read runs") {
        if e.kind == EdgeKind::Calls {
            out.entry(label[&e.source].clone()).or_default().push(label[&e.target].clone());
        }
    }
    for targets in out.values_mut() {
        targets.sort();
    }
    out
}

/// The targets the callable labelled `caller` calls, sorted; empty when none.
fn from<'a>(edges: &'a BTreeMap<String, Vec<String>>, caller: &str) -> &'a [String] {
    edges.get(caller).map_or(&[], Vec::as_slice)
}

/// The Rust row's call residue: its non-zero reasons, after checking that the
/// reasons partition the unbound rows and that none is unclassified.
fn residue(engine: &Engine) -> BTreeMap<R, u64> {
    let residue: CallResidue = engine
        .status()
        .resolution_by_language
        .into_iter()
        .find(|row| row.language == "rust")
        .expect("a rust row")
        .call_residue
        .expect("the rust row states its call residue");
    assert_eq!(residue.unclassified, 0, "every unbound Rust call has a reason: {residue:?}");
    assert_eq!(
        residue.unbound,
        residue.reasons.values().sum::<u64>(),
        "the reasons partition the unbound rows"
    );
    residue.reasons.into_iter().filter(|(_, n)| *n > 0).collect()
}

fn reasons(pairs: &[(R, u64)]) -> BTreeMap<R, u64> {
    pairs.iter().copied().collect()
}

fn targets(labels: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = labels.iter().map(|l| (*l).to_string()).collect();
    out.sort();
    out
}


// ── `<T>::m()` is the written path `T::m()` ────────────────────────────────

/// `<W>::new()` with a free `fn new` in scope binds `W::new` — outside a macro
/// and inside one, plain and with a turbofish — and never the free fn. 1.14.0
/// recorded the bare `new` and bound the free function.
#[test]
fn a_bracketed_type_call_binds_the_types_associated_fn_and_never_the_free_one() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct W;
impl W { pub fn new() -> W { W } pub fn make<T>() -> W { W } }
pub fn new() -> u8 { 0 }
pub fn make() -> u8 { 0 }
pub fn plain() { let _ = <W>::new(); }
pub fn in_macro() { let _ = vec![<W>::new()]; }
pub fn turbofish() { let _ = <W>::make::<u8>(); }
pub fn turbofish_in_macro() { let _ = vec![<W>::make::<u8>()]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for (caller, member) in [
        ("plain@5", "new@2"),
        ("in_macro@6", "new@2"),
        ("turbofish@7", "make@2"),
        ("turbofish_in_macro@8", "make@2"),
    ] {
        assert_eq!(
            from(&edges, &format!("src/lib.rs:{caller}")),
            targets(&[&format!("src/lib.rs:{member}")]),
            "{caller}: {edges:?}"
        );
    }
    assert!(residue(&engine).is_empty(), "every call binds: {:?}", residue(&engine));
}

/// A bracketed path type reads in the caller's scope as a written one does:
/// a module-qualified `<m::W>::new()` and `<Self>::new()` in `W`'s own impl.
#[test]
fn a_bracketed_path_type_and_self_read_as_written() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub mod m {
    pub struct W;
    impl W { pub fn new() -> W { W } }
}
pub fn new() -> u8 { 0 }
pub fn qualified() { let _ = <m::W>::new(); }
pub fn qualified_in_macro() { let _ = vec![<m::W>::new()]; }
pub struct S;
impl S {
    pub fn new() -> S { S }
    pub fn own() { let _ = <Self>::new(); }
    pub fn own_in_macro() { let _ = vec![<Self>::new()]; }
}
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["qualified@6", "qualified_in_macro@7"] {
        assert_eq!(from(&edges, &format!("src/lib.rs:{caller}")), targets(&["src/lib.rs:new@3"]), "{caller}: {edges:?}");
    }
    for caller in ["own@11", "own_in_macro@12"] {
        assert_eq!(from(&edges, &format!("src/lib.rs:{caller}")), targets(&["src/lib.rs:new@10"]), "{caller}: {edges:?}");
    }
}

/// A bracket whose type is no repository type — an external type, a slice, a
/// reference, a trait object — binds nothing, reads `external-type`, and never
/// reaches a same-named free fn, outside a macro and inside one.
#[test]
fn a_bracket_naming_no_repository_type_binds_nothing_and_is_external() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Tr { fn m(&self); }
pub fn new() {}
pub fn len() {}
pub fn m() {}
pub fn vec_new() { let _ = <Vec<u8>>::new(); }
pub fn slice_len(x: &[u8]) { let _ = <[u8]>::len(x); }
pub fn str_len(s: &str) { let _ = <&str>::len(s); }
pub fn dyn_m(x: &dyn Tr) { <dyn Tr>::m(x); }
pub fn vec_new_in_macro() { let _ = vec![<Vec<u8>>::new()]; }
pub fn slice_len_in_macro(x: &[u8]) { let _ = vec![<[u8]>::len(x)]; }
pub fn str_len_in_macro(s: &str) { let _ = vec![<&str>::len(s)]; }
pub fn dyn_m_in_macro(x: &dyn Tr) { vec![<dyn Tr>::m(x)]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(edges.is_empty(), "no bracketed call binds: {edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::ExternalType, 8)]));
}

/// A bracket that is itself a qualified path — `<<A as B>::X>::m()`, an
/// associated type's function — binds nothing, inside a macro as outside one:
/// the macro's `<<` opener is one token, and reading it as a single `<` once
/// split the bracket at its ` as ` and bound the call to a trait named `X`.
#[test]
fn a_bracket_that_is_a_qualified_path_binds_nothing_inside_a_macro_too() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct A;
pub trait B { type X; }
impl B for A { type X = u8; }
pub trait X { fn m(); }
impl X for A { fn m() {} }
pub fn outside() { <<A as B>::X>::m(); }
pub fn in_macro() { let _ = vec![<<A as B>::X>::m()]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(edges.is_empty(), "no call through a bracketed qualified path binds: {edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::ExternalType, 2)]));
}

/// `<Q as Run>::go(q)` is S-608's: it binds as it did, beside the new form.
#[test]
fn a_qualified_trait_call_is_unchanged() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Run { fn go(&self) {} }
pub struct Q;
impl Run for Q { fn go(&self) {} }
pub fn go() {}
pub fn q(q: &Q) { <Q as Run>::go(q); }
pub fn q_in_macro(q: &Q) { vec![<Q as Run>::go(q)]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["q@5", "q_in_macro@6"] {
        assert_eq!(from(&edges, &format!("src/lib.rs:{caller}")), targets(&["src/lib.rs:go@3"]), "{caller}: {edges:?}");
    }
}

/// A `self`-taking `m` called as `<T>::m(x, 1)` counts its receiver: two
/// arguments reach a one-parameter method, as `T::m(x, 1)` does (FR-RS-43).
#[test]
fn a_self_taking_method_called_through_a_bracket_counts_its_receiver() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct T;
impl T {
    pub fn m(&self, a: u8) {}
}
pub fn written(x: &T) { T::m(x, 1); }
pub fn bracketed(x: &T) { <T>::m(x, 1); }
pub fn bracketed_in_macro(x: &T) { vec![<T>::m(x, 1)]; }
pub fn too_few(x: &T) { <T>::m(x); }
pub fn too_few_in_macro(x: &T) { vec![<T>::m(x)]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["written@5", "bracketed@6", "bracketed_in_macro@7"] {
        assert_eq!(from(&edges, &format!("src/lib.rs:{caller}")), targets(&["src/lib.rs:m@3"]), "{caller}: {edges:?}");
    }
    for caller in ["too_few@8", "too_few_in_macro@9"] {
        assert!(from(&edges, &format!("src/lib.rs:{caller}")).is_empty(), "{caller}: {edges:?}");
    }
    assert_eq!(residue(&engine), reasons(&[(R::NoApplicableOverload, 2)]));
}

// ── Sync ≡ reindex ─────────────────────────────────────────────────────────

/// A synced edit equals a fresh index of the edited tree.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    let tmp = tree(initial);
    let engine = index(tmp.path());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        write(tmp.path(), rel, text);
        changed.push(root.join(rel));
    }
    engine.sync(&changed);
    let rt = engine.runtime().unwrap();

    let mut final_state: BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let fresh = tree(&files);
    let reindexed = index(fresh.path());
    assert_eq!(
        graph_fingerprint(rt),
        graph_fingerprint(reindexed.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    call_edges(rt)
}

const W_LIB: &str = "pub mod w;\npub mod caller;\n";
const W_IMPL: &str = "pub struct W;\nimpl W { pub fn new() -> W { W } }\npub fn new() -> u8 { 0 }\n";
const WITH_CALL: &str = "use crate::w::W;\npub fn f() { let _ = <W>::new(); }\n";
const WITHOUT_CALL: &str = "use crate::w::W;\npub fn f() { let _ = W::new; }\n";

/// A `<T>::m` call added binds `T`'s `m`; removed, the edge goes — and each
/// synced state equals a fresh index of it.
#[test]
fn a_bracketed_call_added_or_removed_rebinds_on_sync() {
    let files = |caller: &'static str| [("src/lib.rs", W_LIB), ("src/w.rs", W_IMPL), ("src/caller.rs", caller)];
    let added = synced_equals_reindexed(&files(WITHOUT_CALL), &[("src/caller.rs", WITH_CALL)]);
    assert_eq!(from(&added, "src/caller.rs:f@2"), targets(&["src/w.rs:new@2"]), "{added:?}");
    let removed = synced_equals_reindexed(&files(WITH_CALL), &[("src/caller.rs", WITHOUT_CALL)]);
    assert!(from(&removed, "src/caller.rs:f@2").is_empty(), "{removed:?}");
}

// ── `gen`, `union` and `default` calls inside a macro (S-628) ──────────────

/// `gen`, `union` and `default` are keyword tokens in a macro's token tree and
/// identifiers outside it; a free, path, `self.` and proven-receiver call named
/// by one binds the same inside a macro as outside — 1.14.0 recorded none of
/// them inside.
#[test]
fn a_keyword_named_call_inside_a_macro_binds_as_outside() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub fn gen(n: u8) -> u8 { n }
pub fn union(n: u8) -> u8 { n }
pub fn default(n: u8) -> u8 { n }
pub struct W;
impl W {
    pub fn gen() -> u8 { 0 }
    pub fn union() -> u8 { 0 }
    pub fn default() -> W { W }
}
pub struct V;
impl V {
    pub fn gen(&self) {}
    pub fn union(&self) {}
    pub fn default(&self) {}
    pub fn own(&self) { self.gen(); self.union(); self.default(); }
    pub fn own_in_macro(&self) { vec![self.gen(), self.union(), self.default()]; }
}
pub fn free() { let _ = (gen(1), union(1), default(1)); }
pub fn free_in_macro() { let _ = vec![gen(1), union(1), default(1)]; }
pub fn path() { let _ = (W::gen(), W::union(), W::default()); }
pub fn path_in_macro() { let _ = vec![W::gen(), W::union(), W::default()]; }
pub fn proven(v: &V) { v.gen(); v.union(); v.default(); }
pub fn proven_in_macro(v: &V) { assert_eq!(vec![v.gen(), v.union(), v.default()], vec![]); }
pub fn assert_wrapped() { assert_eq!(vec![W::default()], vec![W]); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    let at = |c: &str| from(&edges, &format!("src/lib.rs:{c}")).to_vec();
    for (outside, inside) in [
        ("free@18", "free_in_macro@19"),
        ("path@20", "path_in_macro@21"),
        ("own@15", "own_in_macro@16"),
        ("proven@22", "proven_in_macro@23"),
    ] {
        assert_eq!(at(outside).len(), 3, "outside binds every call: {outside}: {edges:?}");
        assert_eq!(at(inside), at(outside), "{inside} binds as {outside}: {edges:?}");
    }
    assert_eq!(at("assert_wrapped@24"), targets(&["src/lib.rs:default@8"]), "{edges:?}");
}

/// `auto`, `raw` and `safe` named calls inside a macro still record; a keyword
/// that opens a construct records no call named by it, so nothing binds to the
/// free `gen`, `union` or `default`.
#[test]
fn a_keyword_that_opens_a_construct_binds_no_call_inside_a_macro() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub fn gen() {}
pub fn union() {}
pub fn default() {}
pub fn auto(n: u8) -> u8 { n }
pub fn raw(n: u8) -> u8 { n }
pub fn safe(n: u8) -> u8 { n }
pub fn named() { let _ = vec![auto(1), raw(1), safe(1)]; }
pub fn constructs() { vec![gen { 1 }, union U { a: u8 }, default fn g()]; }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:named@7").len(), 3, "{edges:?}");
    let constructs = from(&edges, "src/lib.rs:constructs@8");
    assert!(
        constructs.iter().all(|t| !["gen@1", "union@2", "default@3"].iter().any(|k| t.ends_with(k))),
        "no construct keyword binds as a call: {edges:?}"
    );
}

const KW_LIB: &str = "pub struct W;\nimpl W { pub fn default() -> W { W } }\npub fn default() -> u8 { 0 }\n";
const KW_WITH: &str = "use crate::w::W;\npub fn f() { assert_eq!(vec![W::default()], vec![]); }\n";
const KW_WITHOUT: &str = "use crate::w::W;\npub fn f() { assert_eq!(vec![W::new], vec![]); }\n";

/// An in-macro `default` call added binds; removed, the edge goes — and each
/// synced state equals a fresh index of it.
#[test]
fn an_in_macro_default_call_added_or_removed_rebinds_on_sync() {
    let files = |caller: &'static str| [("src/lib.rs", W_LIB), ("src/w.rs", KW_LIB), ("src/caller.rs", caller)];
    let added = synced_equals_reindexed(&files(KW_WITHOUT), &[("src/caller.rs", KW_WITH)]);
    assert_eq!(from(&added, "src/caller.rs:f@2"), targets(&["src/w.rs:default@2"]), "{added:?}");
    let removed = synced_equals_reindexed(&files(KW_WITH), &[("src/caller.rs", KW_WITHOUT)]);
    assert!(from(&removed, "src/caller.rs:f@2").is_empty(), "{removed:?}");
}
