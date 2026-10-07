//! A Rust call to a type's method resolves through one associated-item lookup
//! (S-607, CR-202, FR-RS-47, NFR-RA-05) — exercised end to end through the
//! public [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! Rust queries and `plugin.toml`.
//!
//! `Self::m()`, `self.m()`, a proven `x.m()` and a written `T::m()` all bind
//! through `lookup(T, m, syntax)`: among the functions of every `impl` block
//! whose self type resolves to `T`'s node, from any crate and through `pub use`
//! re-exports. Syntax decides the `takes_self` and arity filters; an inherent
//! function beats a trait's in any module; a trait not in scope is no
//! candidate (the caller's own impl's trait is in scope, CRA-02); an inherent
//! `&self` beside a trait's by-value `self` binds nothing. Every fixture here
//! that names "1.13.0" bound otherwise on that release; the implementation
//! notes record each such run.
//!
//! Every unbound Rust call carries a reason (`unclassified` is 0), two of
//! them new: `not-a-callable` and `name-not-in-scope`.
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

// ── The four fixtures 1.13.0 bound otherwise ───────────────────────────────

/// `A::default()` beside `impl Bb { fn default() }`: A's `default` is derived,
/// which no source node records, so the call binds nothing — 1.13.0 bound it
/// to `Bb::default`, the module's only `default` (the `Type::func` collapse).
#[test]
fn a_derived_method_binds_nothing_and_never_a_same_named_method_of_another_type() {
    let engine = index(
        tree(&[(
            "src/lib.rs",
            "\
#[derive(Default)]
pub struct A;
pub struct Bb;
impl Bb {
    pub fn default() -> Bb { Bb }
}
pub fn make_a() -> A { A::default() }
",
        )])
        .path(),
    );
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/lib.rs:make_a@7").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::SupertypeUnreached, 1)]));
}

/// Two types with `make` in one module: each `T::make()` binds its own type's
/// — 1.13.0 left both unbound (two `make` at module scope).
#[test]
fn two_types_of_one_module_each_bind_their_own_associated_function() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct A;
pub struct B;
impl A { pub fn make() -> A { A } }
impl B { pub fn make() -> B { B } }
pub fn both() { A::make(); B::make(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(
        from(&edges, "src/lib.rs:both@5"),
        targets(&["src/lib.rs:make@3", "src/lib.rs:make@4"])
    );
}

/// Inherent `start(a)` beside a trait's `start(a, b)`: `E::start(1)` binds the
/// inherent one — 1.13.0 left it unbound.
#[test]
fn a_path_call_binds_the_inherent_function_over_a_trait_function_of_the_name() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct E;
pub trait Starter { fn start(a: u8, b: u8); }
impl E { pub fn start(a: u8) {} }
impl Starter for E { fn start(a: u8, b: u8) {} }
pub fn go() { E::start(1); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:go@5"), targets(&["src/lib.rs:start@3"]));
}

/// `impl Q { fn go }` beside `impl Run for Q { fn go }` in one module:
/// `self.go()` binds the inherent `go` — 1.13.0 read it `overload-ambiguous`
/// (the module rung saw two), while `q.go()` already bound the inherent one.
#[test]
fn a_self_call_binds_the_inherent_method_over_a_trait_method_of_the_name() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct Q;
pub trait Run { fn go(&self); }
impl Q {
    pub fn go(&self) {}
    pub fn twice(&self) { self.go(); }
}
impl Run for Q { fn go(&self) {} }
pub fn outside(q: &Q) { q.go(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:twice@5"), targets(&["src/lib.rs:go@4"]));
    assert_eq!(from(&edges, "src/lib.rs:outside@8"), targets(&["src/lib.rs:go@4"]));
}

// ── Syntax decides the filters ─────────────────────────────────────────────

/// `self.m()` never calls an associated function: it binds the trait's
/// `m(&self)`, while `Self::m()` binds the inherent `fn m()`.
#[test]
fn method_syntax_drops_an_associated_function_and_path_syntax_keeps_it() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct S;
pub trait T { fn m(&self); }
impl S {
    pub fn m() {}
    pub fn by_method(&self) { self.m(); }
    pub fn by_path(&self) { Self::m(); }
}
impl T for S { fn m(&self) {} }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:by_method@5"), targets(&["src/lib.rs:m@8"]));
    assert_eq!(from(&edges, "src/lib.rs:by_path@6"), targets(&["src/lib.rs:m@4"]));
}

/// Arity counts the receiver for a path call to a self-taking function
/// (`P::m(p, 1)` fits `m(&self, a)`) and not for method syntax (`p.m(1)`);
/// a count no candidate admits is `no-applicable-overload`.
#[test]
fn arity_excludes_the_receiver_for_method_syntax_and_counts_it_for_a_path_call() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct P;
impl P { pub fn m(&self, a: u8) {} }
pub fn by_path(p: &P) { P::m(p, 1); }
pub fn by_method(p: &P) { p.m(1); }
pub fn too_few(p: &P) { P::m(p); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:by_path@3"), targets(&["src/lib.rs:m@2"]));
    assert_eq!(from(&edges, "src/lib.rs:by_method@4"), targets(&["src/lib.rs:m@2"]));
    assert!(from(&edges, "src/lib.rs:too_few@5").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::NoApplicableOverload, 1)]));
}

/// An inherent `&self` method beside a trait's by-value `self` one: rustc's
/// by-value probe may pick the trait's, so the method call binds nothing —
/// while the path call `V::eat(&v)` binds the inherent one.
#[test]
fn an_inherent_by_reference_method_beside_a_by_value_trait_method_binds_nothing() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct V;
pub trait Consume { fn eat(self); }
impl V { pub fn eat(&self) {} }
impl Consume for V { fn eat(self) {} }
pub fn by_method(v: V) { v.eat(); }
pub fn by_path(v: V) { V::eat(&v); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/lib.rs:by_method@5").is_empty(), "{edges:?}");
    assert_eq!(from(&edges, "src/lib.rs:by_path@6"), targets(&["src/lib.rs:eat@3"]));
    assert_eq!(residue(&engine), reasons(&[(R::OverloadAmbiguous, 1)]));
}

// ── The universe: any crate, any header, re-exports ────────────────────────

/// A module-relative header `impl a::X` records its functions for `X`: the
/// written `a::X::new()` and the proven `x.hi()` bind (1.13.0: neither).
#[test]
fn a_module_relative_impl_header_binds() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub mod a { pub struct X; }
impl a::X {
    pub fn new() -> a::X { a::X }
    pub fn hi(&self) {}
}
pub fn make() { a::X::new(); }
pub fn greet(x: &a::X) { x.hi(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:make@6"), targets(&["src/lib.rs:new@3"]));
    assert_eq!(from(&edges, "src/lib.rs:greet@7"), targets(&["src/lib.rs:hi@4"]));
}

/// A trait impl written in another crate for a type of `base`: the impl index
/// is keyed by the type's node, not by the crate, so `t.m()` in `app` binds
/// it (1.13.0: unbound, keyed by the method's crate).
#[test]
fn a_trait_impl_written_in_another_crate_binds() {
    let tmp = tree(&[
        ("base/src/lib.rs", "pub struct T;\n"),
        (
            "app/src/lib.rs",
            "\
pub trait Tr { fn m(&self); }
impl Tr for base::T { fn m(&self) {} }
pub fn f(t: &base::T) { t.m(); }
",
        ),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "app/src/lib.rs:f@3"), targets(&["app/src/lib.rs:m@2"]));
}

/// A type reached through a `pub use inner::*` glob — written through the
/// re-exporting module (`crate::reexp::H::new()`) or through a glob import of
/// it (`H::new()`) — and one reached through a named `pub use` bind.
#[test]
fn a_type_reached_through_a_glob_or_named_re_export_binds() {
    let tmp = tree(&[
        (
            "src/inner.rs",
            "\
pub struct H;
impl H { pub fn new() -> H { H } }
pub struct G;
impl G { pub fn new() -> G { G } }
",
        ),
        ("src/reexp.rs", "pub use crate::inner::*;\n"),
        ("src/named.rs", "pub use crate::inner::G;\n"),
        (
            "src/lib.rs",
            "\
pub mod inner;
pub mod reexp;
pub mod named;
pub mod user;
pub fn written() { crate::reexp::H::new(); }
pub fn via_named() { crate::named::G::new(); }
",
        ),
        ("src/user.rs", "use crate::reexp::*;\npub fn globbed() { H::new(); }\n"),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:written@5"), targets(&["src/inner.rs:new@2"]));
    assert_eq!(from(&edges, "src/lib.rs:via_named@6"), targets(&["src/inner.rs:new@4"]));
    assert_eq!(from(&edges, "src/user.rs:globbed@2"), targets(&["src/inner.rs:new@2"]));
}

// ── Trait scope ────────────────────────────────────────────────────────────

/// A trait not in scope is no candidate: `w.sec()` binds nothing without
/// `use hidden::Secret` (1.13.0 bound it), and binds with it — by name or
/// `as _`.
#[test]
fn a_trait_method_binds_only_where_its_trait_is_in_scope() {
    let tmp = tree(&[
        (
            "src/hidden.rs",
            "\
pub trait Secret { fn sec(&self); }
impl Secret for crate::W { fn sec(&self) {} }
",
        ),
        (
            "src/lib.rs",
            "\
pub mod hidden;
pub mod named;
pub mod anon;
pub struct W;
pub fn unseen(w: &W) { w.sec(); }
",
        ),
        ("src/named.rs", "use crate::hidden::Secret;\npub fn seen(w: &crate::W) { w.sec(); }\n"),
        ("src/anon.rs", "use crate::hidden::Secret as _;\npub fn seen(w: &crate::W) { w.sec(); }\n"),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/lib.rs:unseen@5").is_empty(), "{edges:?}");
    assert_eq!(from(&edges, "src/named.rs:seen@2"), targets(&["src/hidden.rs:sec@2"]));
    assert_eq!(from(&edges, "src/anon.rs:seen@2"), targets(&["src/hidden.rs:sec@2"]));
    assert_eq!(residue(&engine), reasons(&[(R::SupertypeUnreached, 1)]));
}

/// CRA-02: the trait named in the caller's own impl header is in scope there
/// — `impl traits::Run for Q`'s `self.go()` binds with no `use` of `Run` —
/// while a free function of the same file, which does not import it, binds
/// nothing.
#[test]
fn the_trait_of_the_callers_own_impl_is_in_scope_without_an_import() {
    let tmp = tree(&[
        ("src/traits.rs", "pub trait Run { fn go(&self); fn twice(&self); }\n"),
        (
            "src/lib.rs",
            "\
pub mod traits;
pub struct Q;
impl traits::Run for Q {
    fn go(&self) {}
    fn twice(&self) { self.go(); }
}
pub fn outside(q: &Q) { q.go(); }
",
        ),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:twice@5"), targets(&["src/lib.rs:go@4"]));
    assert!(from(&edges, "src/lib.rs:outside@7").is_empty(), "{edges:?}");
}

// ── Every unbound call has a reason ────────────────────────────────────────

/// `not-a-callable` (an enum variant, a tuple-struct constructor, `Self(..)`),
/// `name-not-in-scope` (a prelude function, a local closure, a module that
/// declares no such item) and `external-type` (a std path) — and nothing is
/// unclassified.
#[test]
fn every_unbound_rust_call_carries_a_reason() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub mod m { pub fn here() {} }
pub enum E { A(u8), B }
pub struct W(u8);
impl W { pub fn again(&self) -> W { Self(1) } }
pub fn variant() { E::A(1); }
pub fn tuple() { W(1); }
pub fn prelude() { drop(1); }
pub fn local() { let c = |x: u8| x; c(1); }
pub fn absent() { m::gone(); }
pub fn std_path() { std::mem::take(&mut 0u8); }
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        residue(&engine),
        reasons(&[(R::NotACallable, 3), (R::NameNotInScope, 3), (R::ExternalType, 1)])
    );
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

/// A one-file edit moving `make` from `A`'s impl block to `B`'s rebinds the
/// calls in another file that name each type.
#[test]
fn moving_a_function_between_impl_blocks_rebinds_on_sync() {
    let initial = [
        (
            "src/types.rs",
            "\
pub struct A;
pub struct B;
impl A { pub fn make() {} }
impl B {}
",
        ),
        ("src/lib.rs", "pub mod types;\nuse types::{A, B};\npub fn a() { A::make(); }\npub fn b() { B::make(); }\n"),
    ];
    let edges = synced_equals_reindexed(
        &initial,
        &[(
            "src/types.rs",
            "\
pub struct A;
pub struct B;
impl A {}
impl B { pub fn make() {} }
",
        )],
    );
    assert!(from(&edges, "src/lib.rs:a@3").is_empty(), "{edges:?}");
    assert_eq!(from(&edges, "src/lib.rs:b@4"), targets(&["src/types.rs:make@4"]));
}

/// A `pub use inner::*` glob added on sync rebinds the call written through
/// the re-exporting module.
#[test]
fn a_glob_re_export_added_on_sync_rebinds() {
    let initial = [
        ("src/inner.rs", "pub struct H;\nimpl H { pub fn new() -> H { H } }\n"),
        ("src/reexp.rs", "\n"),
        ("src/lib.rs", "pub mod inner;\npub mod reexp;\npub fn f() { crate::reexp::H::new(); }\n"),
    ];
    let edges = synced_equals_reindexed(&initial, &[("src/reexp.rs", "pub use crate::inner::*;\n")]);
    assert_eq!(from(&edges, "src/lib.rs:f@3"), targets(&["src/inner.rs:new@2"]));
}

/// A `self.m()` reads its `T` from the caller's own impl header: retargeting
/// the re-export that header reads through rebinds the call on sync, although
/// neither the call's row (`m`, shape `self` — a module-relative header records
/// no self type, so no rewrite) nor the caller's file spells the change.
#[test]
fn a_self_call_rebinds_when_the_re_export_its_header_reads_is_retargeted() {
    let initial = [
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod facade;\npub mod c;\n"),
        ("src/a.rs", "pub struct X;\nimpl X { pub fn m(&self) {} }\n"),
        ("src/b.rs", "pub struct X;\nimpl X { pub fn m(&self) {} }\n"),
        ("src/facade.rs", "pub use crate::a::X;\n"),
        ("src/c.rs", "use crate::facade;\nimpl facade::X {\n    pub fn go(&self) { self.m(); }\n}\n"),
    ];
    let edges = synced_equals_reindexed(&initial, &[("src/facade.rs", "pub use crate::b::X;\n")]);
    assert_eq!(from(&edges, "src/c.rs:go@3"), targets(&["src/b.rs:m@2"]));
}

/// A path's type, once its scope names one, decides the call: `T::m()` in
/// `sub`, whose own `T` has no `m`, binds nothing — never the crate root's
/// `T::m`, a wider rung's same-named type.
#[test]
fn a_path_call_never_reaches_a_same_named_type_of_a_wider_scope() {
    let tmp = tree(&[
        ("src/lib.rs", "pub mod sub;\npub struct T;\nimpl T { pub fn m() {} }\n"),
        ("src/sub.rs", "pub struct T;\npub fn f() { T::m(); }\n"),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/sub.rs:f@2").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::SupertypeUnreached, 1)]));
}

/// A bare call never reaches a function of an impl block, whatever its header:
/// `impl a::X`'s `helper` records no self type, and still binds no bare
/// `helper()` beside it.
#[test]
fn a_bare_call_never_binds_a_function_of_a_module_relative_impl() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub mod a { pub struct X; }
impl a::X { pub fn helper() {} }
pub fn caller() { helper(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/lib.rs:caller@3").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::NameNotInScope, 1)]));
}

/// A trait's scope moved by a third file: a `pub use` of it added to, or
/// removed from, the prelude a caller globs in rebinds the call on sync,
/// though neither the row (`W::sec`) nor the caller's file spells `Secret`.
#[test]
fn a_trait_brought_into_scope_by_a_globbed_re_export_rebinds_on_sync() {
    let base = [
        ("src/lib.rs", "pub mod hidden;\npub mod prelude;\npub mod user;\npub struct W;\n"),
        ("src/hidden.rs", "pub trait Secret { fn sec(&self); }\nimpl Secret for crate::W { fn sec(&self) {} }\n"),
        ("src/user.rs", "use crate::prelude::*;\npub fn seen(w: &crate::W) { w.sec(); }\n"),
    ];
    let bare = ("src/prelude.rs", "pub fn helper() {}\n");
    let reexporting = ("src/prelude.rs", "pub fn helper() {}\npub use crate::hidden::Secret;\n");
    let mut without = base.to_vec();
    without.push(bare);
    let edges = synced_equals_reindexed(&without, &[reexporting]);
    assert_eq!(from(&edges, "src/user.rs:seen@2"), targets(&["src/hidden.rs:sec@2"]));
    let mut with = base.to_vec();
    with.push(reexporting);
    let edges = synced_equals_reindexed(&with, &[bare]);
    assert!(from(&edges, "src/user.rs:seen@2").is_empty(), "{edges:?}");
}

/// A written `T::m()` reaches an impl whose header names its type through a
/// third file's renaming re-export: retargeting that re-export moves the
/// function from `Z` to `Y` on sync.
#[test]
fn an_impl_header_read_through_a_retargeted_re_export_rebinds_written_calls_on_sync() {
    let initial = [
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod facade;\npub mod w;\npub mod user;\n"),
        ("src/a.rs", "pub struct Y;\n"),
        ("src/b.rs", "pub struct Z;\n"),
        ("src/facade.rs", "pub use crate::b::Z as X;\n"),
        ("src/w.rs", "use crate::facade::X;\nimpl X { pub fn mk() {} }\n"),
        ("src/user.rs", "use crate::a::Y;\nuse crate::b::Z;\npub fn z() { Z::mk(); }\npub fn y() { Y::mk(); }\n"),
    ];
    let edges = synced_equals_reindexed(&initial, &[("src/facade.rs", "pub use crate::a::Y as X;\n")]);
    assert!(from(&edges, "src/user.rs:z@3").is_empty(), "{edges:?}");
    assert_eq!(from(&edges, "src/user.rs:y@4"), targets(&["src/w.rs:mk@2"]));
}
