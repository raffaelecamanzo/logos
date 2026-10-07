//! Trait defaults, `Deref` targets, trait-typed receivers and qualified paths
//! join the one Rust associated-item lookup (S-608, CR-202, FR-RS-47 rules 1,
//! 4 and 5, FR-RS-08, NFR-RA-05) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with the SHIPPED Rust
//! queries and `plugin.toml`.
//!
//! - A trait's default body is a candidate for every type whose impl of the
//!   trait does not override it — an empty `impl Greet for X {}` included —
//!   ranked as that trait's function: an override beats it, a trait out of
//!   scope supplies nothing, two traits in scope bind nothing.
//! - A method-syntax miss retries on the type's `Deref` target, each type once,
//!   to a bounded depth; path syntax never retries.
//! - `self.m()` in a trait's default body, a receiver bounded by a trait
//!   (inline, `where`, `impl Tr`) and `Tr::m(x)` fan out to FR-RS-08's set —
//!   every impl of the method plus the default where an impl does not override
//!   it. Two bounds that each provide `m` bind nothing.
//! - `<T as Tr>::m` binds `T`'s impl of `Tr::m`, else `Tr`'s default.
//!
//! Every fixture here that names "1.13.0" bound otherwise on that release; the
//! implementation notes record each such run.
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

// ── Trait defaults ─────────────────────────────────────────────────────────

/// An empty `impl Greet for X {}` lends `X` its trait's default `hello`: a
/// proven `x.hello()`, a written `X::hello(x)` and a `self.hello()` in `X`'s
/// own impl all bind it — 1.13.0 bound none of them (`supertype-unreached`).
#[test]
fn an_empty_trait_impl_lends_its_type_the_default_body() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Greet { fn hello(&self) {} }
pub struct X;
impl Greet for X {}
impl X { pub fn own(&self) { self.hello(); } }
pub fn proven(x: &X) { x.hello(); }
pub fn written(x: &X) { X::hello(x); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["src/lib.rs:own@4", "src/lib.rs:proven@5", "src/lib.rs:written@6"] {
        assert_eq!(from(&edges, caller), targets(&["src/lib.rs:hello@1"]), "{caller}: {edges:?}");
    }
}

/// An empty impl inside an inline module reads its header there: `super::Greet`
/// and `X` are that module's names.
#[test]
fn an_empty_impl_in_an_inline_module_reads_its_header_there() {
    let tmp = tree(&[(
        "src/lib.rs",
        "pub trait Greet { fn hello(&self) {} }\npub mod inner { pub struct X; impl super::Greet for X {} }\npub fn f(x: &inner::X) { x.hello(); }\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(from(&call_edges(engine.runtime().unwrap()), "src/lib.rs:f@3"), targets(&["src/lib.rs:hello@1"]));
}

/// One default lent through two blocks of one type (generics stripped) is one
/// candidate, never two.
#[test]
fn a_default_lent_through_two_blocks_is_one_candidate() {
    let tmp = tree(&[(
        "src/lib.rs",
        "pub trait Greet { fn hello(&self) {} }\npub struct W<T>(T);\nimpl Greet for W<u8> {}\nimpl Greet for W<u16> {}\npub fn f(w: &W<u8>) { w.hello(); }\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(from(&call_edges(engine.runtime().unwrap()), "src/lib.rs:f@5"), targets(&["src/lib.rs:hello@1"]));
}

/// An impl's own method beats the trait default it overrides.
#[test]
fn an_impls_override_beats_the_trait_default() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Greet { fn hello(&self) {} }
pub struct X;
impl Greet for X {
    fn hello(&self) {}
}
pub fn proven(x: &X) { x.hello(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:proven@6"), targets(&["src/lib.rs:hello@4"]));
}

/// `Greet`'s default `hello` and `impl b::Loud for X { fn hello }`: the trait
/// the caller has in scope supplies `hello`, and both in scope bind nothing —
/// 1.13.0 bound `Loud`'s `hello` with only `Greet` in scope (the wrong one).
#[test]
fn the_trait_in_scope_supplies_the_method_and_two_in_scope_bind_nothing() {
    let tmp = tree(&[
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod x;\npub mod only_a;\npub mod only_b;\npub mod both;\n"),
        ("src/a.rs", "pub trait Greet { fn hello(&self) {} }\n"),
        ("src/b.rs", "pub trait Loud { fn hello(&self); }\n"),
        (
            "src/x.rs",
            "pub struct X;\nimpl crate::a::Greet for X {}\nimpl crate::b::Loud for X {\n    fn hello(&self) {}\n}\n",
        ),
        ("src/only_a.rs", "use crate::a::Greet;\nuse crate::x::X;\npub fn f(x: &X) { x.hello(); }\n"),
        ("src/only_b.rs", "use crate::b::Loud;\nuse crate::x::X;\npub fn f(x: &X) { x.hello(); }\n"),
        (
            "src/both.rs",
            "use crate::a::Greet;\nuse crate::b::Loud;\nuse crate::x::X;\npub fn f(x: &X) { x.hello(); }\n",
        ),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/only_a.rs:f@3"), targets(&["src/a.rs:hello@1"]));
    assert_eq!(from(&edges, "src/only_b.rs:f@3"), targets(&["src/x.rs:hello@4"]));
    assert!(from(&edges, "src/both.rs:f@4").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::OverloadAmbiguous, 1)]));
}

/// An inherent method beats a trait's default as it beats a trait impl's.
#[test]
fn an_inherent_method_beats_a_trait_default() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Greet { fn hello(&self) {} }
pub struct X;
impl Greet for X {}
impl X { pub fn hello(&self) {} }
pub fn proven(x: &X) { x.hello(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:proven@5"), targets(&["src/lib.rs:hello@4"]));
}

// ── `Deref` ────────────────────────────────────────────────────────────────

/// `o.deep()` through `Deref<Target = Inner>` binds `Inner::deep`, a `Deref`
/// chain is followed, and a path call never retries — 1.13.0 left the method
/// calls `supertype-unreached`.
#[test]
fn a_method_call_retries_on_the_deref_target_and_a_path_call_never_does() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
use std::ops::Deref;
pub struct Inner;
impl Inner { pub fn deep(&self) {} }
pub struct Outer { inner: Inner }
impl Deref for Outer {
    type Target = Inner;
    fn deref(&self) -> &Inner { &self.inner }
}
pub struct Top { outer: Outer }
impl Deref for Top {
    type Target = Outer;
    fn deref(&self) -> &Outer { &self.outer }
}
impl Top { pub fn own(&self) { self.deep(); } }
pub fn proven(o: &Outer) { o.deep(); }
pub fn chained(t: &Top) { t.deep(); }
pub fn written(o: &Outer) { Outer::deep(o); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["src/lib.rs:own@14", "src/lib.rs:proven@15", "src/lib.rs:chained@16"] {
        assert_eq!(from(&edges, caller), targets(&["src/lib.rs:deep@3"]), "{caller}: {edges:?}");
    }
    assert!(from(&edges, "src/lib.rs:written@17").is_empty(), "{edges:?}");
}

/// A `Deref` cycle visits each type once and terminates, and a chain longer
/// than the depth bound is not followed to its end; both read
/// `supertype-unreached`.
#[test]
fn a_deref_cycle_terminates_and_a_chain_past_the_bound_binds_nothing() {
    let deref = |from: &str, to: &str| {
        format!(
            "pub struct {from};\nimpl std::ops::Deref for {from} {{\n    type Target = {to};\n    fn deref(&self) -> &{to} {{ loop {{}} }}\n}}\n"
        )
    };
    let mut cycle = String::new();
    cycle.push_str(&deref("P", "Q"));
    cycle.push_str(&deref("Q", "P"));
    cycle.push_str("pub fn spin(p: &P) { p.nothing(); }\n");
    // T0 → T1 → … → T9, and `T9::far`: nine hops past `T0`.
    let mut long = String::new();
    for i in 0..9 {
        long.push_str(&deref(&format!("T{i}"), &format!("T{}", i + 1)));
    }
    long.push_str("pub struct T9;\nimpl T9 { pub fn far(&self) {} }\n");
    long.push_str("pub fn near(t: &T1) { t.far(); }\npub fn reach(t: &T0) { t.far(); }\n");
    let tmp = tree(&[("src/lib.rs", "pub mod cycle;\npub mod long;\n"), ("src/cycle.rs", &cycle), ("src/long.rs", &long)]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/cycle.rs:spin@11").is_empty(), "{edges:?}");
    assert_eq!(from(&edges, "src/long.rs:near@48"), targets(&["src/long.rs:far@47"]), "eight hops bind");
    assert!(from(&edges, "src/long.rs:reach@49").is_empty(), "nine hops pass the bound: {edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::SupertypeUnreached, 2)]));
}

/// A `Deref` target the repository does not declare (`Vec<u8>`) supplies
/// methods the graph does not hold: `external-type`.
#[test]
fn a_miss_through_an_external_deref_target_is_an_external_type() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub struct Buf(Vec<u8>);
impl std::ops::Deref for Buf {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> { &self.0 }
}
pub fn size(b: &Buf) -> usize { b.len() }
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(residue(&engine), reasons(&[(R::ExternalType, 1)]));
}

/// Two `Deref` impls of one type (generics stripped) naming two targets
/// decide no retry: `type-ambiguous`, never the first target's method.
#[test]
fn two_deref_impls_naming_two_targets_decide_no_retry() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
use std::ops::Deref;
pub struct A;
impl A { pub fn a(&self) {} }
pub struct B;
impl B { pub fn a(&self) {} }
pub struct W<T>(T);
impl Deref for W<u8> {
    type Target = A;
    fn deref(&self) -> &A { &A }
}
impl Deref for W<u16> {
    type Target = B;
    fn deref(&self) -> &B { &B }
}
pub fn f(w: &W<u8>) { w.a(); }
",
    )]);
    let engine = index(tmp.path());
    assert!(from(&call_edges(engine.runtime().unwrap()), "src/lib.rs:f@15").is_empty());
    assert_eq!(residue(&engine), reasons(&[(R::TypeAmbiguous, 1)]));
}

// ── Trait-typed receivers fan out ──────────────────────────────────────────

const RUN: &str = "\
pub trait Run {
    fn go(&self) {}
    fn meta(&self);
    fn both(&self) { self.meta(); self.go(); }
    fn by_path(&self) { Self::meta(self); }
}
pub struct Q;
impl Run for Q {
    fn go(&self) {}
    fn meta(&self) {}
}
pub struct P;
impl Run for P {
    fn meta(&self) {}
}
";

/// `self.m()` / `Self::m()` in a trait's default body, a receiver bounded by
/// the trait inline or in a `where`, `&impl Run`, and `Run::go(q)` fan out to
/// every impl of the method plus the default `P` does not override — 1.13.0
/// bound none of them (`no-receiver-evidence`), and the body's `self.go()`
/// only the default.
#[test]
fn trait_typed_receivers_fan_out_to_the_impls_and_the_unoverridden_default() {
    let callers = "\
use crate::run::{Run, Q};
pub fn inline<T: Run>(t: &T) { t.go(); }
pub fn wher<T>(t: T) where T: Run { t.go(); }
pub fn opaque(t: &impl Run) { t.go(); }
pub fn path(q: &Q) { Run::go(q); }
";
    let tmp = tree(&[("src/lib.rs", "pub mod run;\npub mod callers;\n"), ("src/run.rs", RUN), ("src/callers.rs", callers)]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    let go = targets(&["src/run.rs:go@2", "src/run.rs:go@9"]);
    assert_eq!(
        from(&edges, "src/run.rs:both@4"),
        targets(&["src/run.rs:go@2", "src/run.rs:go@9", "src/run.rs:meta@10", "src/run.rs:meta@14"])
    );
    assert_eq!(from(&edges, "src/run.rs:by_path@5"), targets(&["src/run.rs:meta@10", "src/run.rs:meta@14"]));
    for caller in ["src/callers.rs:inline@2", "src/callers.rs:wher@3", "src/callers.rs:opaque@4", "src/callers.rs:path@5"] {
        assert_eq!(from(&edges, caller), go, "{caller}: {edges:?}");
    }
}

/// A default body stays a dispatch target where every repository impl
/// overrides it — an implementor outside the repository may not, and
/// FR-RS-08's set never lost an edge it held — and with no impl at all it is
/// the only one.
#[test]
fn a_default_stays_a_fan_out_target_beside_overriding_impls() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Run { fn go(&self) {} }
pub trait Idle { fn rest(&self) {} }
pub struct Q;
impl Run for Q {
    fn go(&self) {}
}
pub fn a(r: &dyn Run) { r.go(); }
pub fn b(i: &dyn Idle) { i.rest(); }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:a@7"), targets(&["src/lib.rs:go@1", "src/lib.rs:go@5"]));
    assert_eq!(from(&edges, "src/lib.rs:b@8"), targets(&["src/lib.rs:rest@2"]));
}

/// Two bounds that each provide `go` bind nothing (`overload-ambiguous`); a
/// bound the graph does not hold (`Send`, `Clone`) provides nothing it can
/// see, so `Run` alone decides beside it, and a method only an external bound
/// can provide is an `external-type`.
#[test]
fn two_providing_bounds_bind_nothing_and_an_external_bound_provides_nothing() {
    let callers = "\
use crate::run::{Run, Q};
pub trait Walk { fn go(&self); }
impl Walk for Q { fn go(&self) {} }
pub fn two<T: Run + Walk>(t: &T) { t.go(); }
pub fn marker<T: Run + Send + Clone>(t: &T) { t.go(); }
pub fn cloned<T: Run + Clone>(t: &T) -> T { t.clone() }
";
    let tmp = tree(&[("src/lib.rs", "pub mod run;\npub mod callers;\n"), ("src/run.rs", RUN), ("src/callers.rs", callers)]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/callers.rs:two@4").is_empty(), "{edges:?}");
    assert_eq!(
        from(&edges, "src/callers.rs:marker@5"),
        targets(&["src/run.rs:go@2", "src/run.rs:go@9"])
    );
    assert!(from(&edges, "src/callers.rs:cloned@6").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::OverloadAmbiguous, 1), (R::ExternalType, 1)]));
}

/// A bound is the trait the caller names: where the file imports
/// `std::io::Write`, a `W: Write`, `impl Write` or `dyn Write` receiver never
/// reaches a repository trait that happens to be called `Write` (an
/// `external-type`), while a caller naming the repository trait by path
/// reaches its impls.
#[test]
fn a_bound_is_the_trait_the_caller_names_never_a_same_named_repository_one() {
    let tmp = tree(&[
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n"),
        ("src/a.rs", "pub trait Write { fn flush(&mut self) {} }\npub struct Foo;\nimpl Write for Foo {\n    fn flush(&mut self) {}\n}\n"),
        (
            "src/b.rs",
            "use std::io::Write;\npub fn generic<W: Write>(w: &mut W) { w.flush(); }\npub fn opaque(w: &mut impl Write) { w.flush(); }\npub fn object(w: &mut dyn Write) { w.flush(); }\n",
        ),
        ("src/c.rs", "pub fn named(w: &mut dyn crate::a::Write) { w.flush(); }\n"),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    for caller in ["src/b.rs:generic@2", "src/b.rs:opaque@3", "src/b.rs:object@4"] {
        assert!(from(&edges, caller).is_empty(), "{caller}: {edges:?}");
    }
    assert_eq!(from(&edges, "src/c.rs:named@1"), targets(&["src/a.rs:flush@1", "src/a.rs:flush@4"]));
    assert_eq!(residue(&engine), reasons(&[(R::ExternalType, 3)]));
}

/// A bound whose name two repository traits share may be either, and either
/// may supply the method: beside another providing bound it binds nothing
/// (`overload-ambiguous`), and alone it is `type-ambiguous` — never the other
/// bound's set, and never an `external-type`.
#[test]
fn a_bound_two_repository_traits_name_is_ambiguous() {
    let tmp = tree(&[
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod walk;\npub mod callers;\n"),
        ("src/a.rs", "pub trait Run { fn go(&self) {} }\n"),
        ("src/b.rs", "pub trait Run { fn go(&self) {} }\n"),
        ("src/walk.rs", "pub trait Walk { fn go(&self) {} }\n"),
        (
            "src/callers.rs",
            "use crate::walk::Walk;\npub fn both<T: a::b::Run + Walk>(t: &T) { t.go(); }\npub fn alone(t: &dyn x::Run) { t.go(); }\n",
        ),
    ]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert!(from(&edges, "src/callers.rs:both@2").is_empty(), "{edges:?}");
    assert!(from(&edges, "src/callers.rs:alone@3").is_empty(), "{edges:?}");
    assert_eq!(residue(&engine), reasons(&[(R::OverloadAmbiguous, 1), (R::TypeAmbiguous, 1)]));
}

// ── Qualified paths ────────────────────────────────────────────────────────

/// `<Q as Run>::go(q)` binds `Q`'s impl of `Run::go`, `<P as Run>::go(p)` the
/// default `P` does not override, and `<X as Display>::fmt` the impl of the
/// external trait the type's block names — 1.13.0 left each unbound
/// (`supertype-unreached`).
#[test]
fn a_qualified_path_binds_the_types_impl_of_the_trait_or_its_default() {
    let callers = "\
use crate::run::{Run, P, Q};
pub struct X;
impl X { pub fn fmt(&self) {} }
impl std::fmt::Display for X {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { Ok(()) }
}
pub fn q(q: &Q) { <Q as Run>::go(q); }
pub fn p(p: &P) { <P as Run>::go(p); }
pub fn shown(x: &X, f: &mut std::fmt::Formatter<'_>) { <X as std::fmt::Display>::fmt(x, f); }
pub fn foreign(e: &std::io::Error) { <std::io::Error as Run>::go(e); }
";
    let tmp = tree(&[("src/lib.rs", "pub mod run;\npub mod callers;\n"), ("src/run.rs", RUN), ("src/callers.rs", callers)]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/callers.rs:q@7"), targets(&["src/run.rs:go@9"]));
    assert_eq!(from(&edges, "src/callers.rs:p@8"), targets(&["src/run.rs:go@2"]));
    assert_eq!(from(&edges, "src/callers.rs:shown@9"), targets(&["src/callers.rs:fmt@5"]));
    assert!(from(&edges, "src/callers.rs:foreign@10").is_empty(), "{edges:?}");
}

/// `<Self as Run>::go(self)` reads `Self` as the caller's own impl's type —
/// binding that type's impl of `Run::go` — and in `Run`'s default body fans
/// out through `Run` as `self.go()` there does.
#[test]
fn a_qualified_self_path_reads_the_callers_own_type() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Run {
    fn go(&self) {}
    fn twice(&self) { <Self as Run>::go(self); }
}
pub struct X;
impl Run for X {
    fn go(&self) {}
}
impl X { pub fn own(&self) { <Self as Run>::go(self); } }
",
    )]);
    let engine = index(tmp.path());
    let edges = call_edges(engine.runtime().unwrap());
    assert_eq!(from(&edges, "src/lib.rs:own@9"), targets(&["src/lib.rs:go@7"]));
    assert_eq!(from(&edges, "src/lib.rs:twice@3"), targets(&["src/lib.rs:go@2", "src/lib.rs:go@7"]));
}

/// A qualified path that names no trait as `Tr` reads `supertype-unreached`,
/// and one whose `T` is outside the repository `external-type`.
#[test]
fn a_qualified_path_that_binds_nothing_carries_its_reason() {
    let tmp = tree(&[(
        "src/lib.rs",
        "\
pub trait Run { fn go(&self) {} }
pub struct S;
impl Run for S {}
pub struct NotTrait;
pub fn a(s: &S) { <S as NotTrait>::go(s); }
pub fn b(e: &std::io::Error) { <std::io::Error as Run>::go(e); }
",
    )]);
    let engine = index(tmp.path());
    assert_eq!(residue(&engine), reasons(&[(R::SupertypeUnreached, 1), (R::ExternalType, 1)]));
}

/// A qualified path names its trait, so it binds whether or not the file
/// imports it.
#[test]
fn a_qualified_path_binds_without_its_trait_in_scope() {
    let callers = "use crate::run::Q;\npub fn q(q: &Q) { <Q as crate::run::Run>::go(q); }\n";
    let tmp = tree(&[("src/lib.rs", "pub mod run;\npub mod callers;\n"), ("src/run.rs", RUN), ("src/callers.rs", callers)]);
    let engine = index(tmp.path());
    assert_eq!(from(&call_edges(engine.runtime().unwrap()), "src/callers.rs:q@2"), targets(&["src/run.rs:go@9"]));
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

/// The fixture the sync cases edit: a trait with a default, a type, and
/// callers in another file — a proven call, a `self` call in the type's own
/// impl, a `dyn` call and a trait body's `self` call.
fn greet_tree(imp: &str) -> Vec<(&'static str, String)> {
    vec![
        ("src/lib.rs", "pub mod greet;\npub mod x;\npub mod imp;\npub mod callers;\n".to_string()),
        (
            "src/greet.rs",
            "pub trait Greet {\n    fn hello(&self) {}\n    fn twice(&self) { self.hello(); }\n}\n".to_string(),
        ),
        (
            "src/x.rs",
            "use crate::greet::Greet;\npub struct X;\nimpl X { pub fn own(&self) { self.hello(); } }\n".to_string(),
        ),
        ("src/imp.rs", imp.to_string()),
        (
            "src/callers.rs",
            "use crate::greet::Greet;\nuse crate::x::X;\npub fn f(x: &X) { x.hello(); }\npub fn g(d: &dyn Greet) { d.hello(); }\n"
                .to_string(),
        ),
    ]
}

fn sync_case(before: &str, after: &str) -> BTreeMap<String, Vec<String>> {
    let initial = greet_tree(before);
    let initial: Vec<(&str, &str)> = initial.iter().map(|(p, s)| (*p, s.as_str())).collect();
    synced_equals_reindexed(&initial, &[("src/imp.rs", after)])
}

// The impl file imports nothing, so no name it spells reaches a sync through
// an import: only its impl block's header does.
const NO_IMPL: &str = "\n";
const EMPTY_IMPL: &str = "impl crate::greet::Greet for crate::x::X {}\n";
const OVERRIDE: &str = "impl crate::greet::Greet for crate::x::X {\n    fn hello(&self) {}\n}\n";

/// An empty impl added in a file that declares neither the type nor the trait
/// lends the default to every caller on sync; removed, it takes it back.
#[test]
fn an_empty_impl_added_or_removed_rebinds_on_sync() {
    let added = sync_case(NO_IMPL, EMPTY_IMPL);
    for caller in ["src/callers.rs:f@3", "src/x.rs:own@3"] {
        assert_eq!(from(&added, caller), targets(&["src/greet.rs:hello@2"]), "{caller}: {added:?}");
    }
    let removed = sync_case(EMPTY_IMPL, NO_IMPL);
    for caller in ["src/callers.rs:f@3", "src/x.rs:own@3"] {
        assert!(from(&removed, caller).is_empty(), "{caller}: {removed:?}");
    }
}

/// An override added or removed moves the type's callers between the default
/// and the override, and the fan-out set of a `dyn` call and of the trait
/// body's `self` call gains or loses it beside the default.
#[test]
fn an_override_added_or_removed_rebinds_on_sync() {
    let added = sync_case(EMPTY_IMPL, OVERRIDE);
    assert_eq!(from(&added, "src/callers.rs:f@3"), targets(&["src/imp.rs:hello@2"]));
    for caller in ["src/callers.rs:g@4", "src/greet.rs:twice@3"] {
        assert_eq!(
            from(&added, caller),
            targets(&["src/greet.rs:hello@2", "src/imp.rs:hello@2"]),
            "{caller}: {added:?}"
        );
    }
    let removed = sync_case(OVERRIDE, EMPTY_IMPL);
    for caller in ["src/callers.rs:f@3", "src/callers.rs:g@4", "src/greet.rs:twice@3"] {
        assert_eq!(from(&removed, caller), targets(&["src/greet.rs:hello@2"]), "{caller}: {removed:?}");
    }
}

/// A config change that stops admitting the file holding only an empty impl
/// takes the lent default back, as a fresh index of the narrowed project
/// would: the purge's change-set carries the impl header's names too.
#[test]
fn a_purged_empty_impl_takes_its_default_back() {
    let files = greet_tree(EMPTY_IMPL);
    let files: Vec<(&str, &str)> = files.iter().map(|(p, s)| (*p, s.as_str())).collect();
    let tmp = tree(&files);
    let engine = index(tmp.path());
    assert_eq!(
        from(&call_edges(engine.runtime().unwrap()), "src/callers.rs:f@3"),
        targets(&["src/greet.rs:hello@2"])
    );
    let narrowed = "exclude = [\"src/imp.rs\"]\n";
    write(tmp.path(), ".logos/config.toml", narrowed);
    engine.scan(true).expect("scan reconciles");
    let rt = engine.runtime().unwrap();

    let fresh = tree(&files);
    write(fresh.path(), ".logos/config.toml", narrowed);
    let reindexed = index(fresh.path());
    assert_eq!(
        graph_fingerprint(rt),
        graph_fingerprint(reindexed.runtime().unwrap()),
        "purge-to-state must equal index-of-state"
    );
    assert!(from(&call_edges(rt), "src/callers.rs:f@3").is_empty());
}

/// An empty impl added for a `Deref` target lends its default to the outer
/// type's method calls on sync, although neither the calls nor the change
/// spell the outer type — and removed, it takes the default back, although
/// nothing left in the graph lends it (sprint 92 review).
#[test]
fn an_empty_impl_on_a_deref_target_rebinds_on_sync() {
    let base = [
        ("src/lib.rs", "pub mod t;\npub mod d;\npub mod callers;\n"),
        (
            "src/t.rs",
            "pub trait Greet { fn hello(&self) {} }\npub struct Inner;\npub struct Outer;\nimpl std::ops::Deref for Outer {\n    type Target = Inner;\n    fn deref(&self) -> &Inner { &Inner }\n}\n",
        ),
        ("src/callers.rs", "use crate::t::{Greet, Outer};\npub fn o(x: &Outer) { x.hello(); }\n"),
    ];
    let imp = "use super::t::*;\nimpl Greet for Inner {}\n";
    let mut without = base.to_vec();
    without.push(("src/d.rs", "\n"));
    let added = synced_equals_reindexed(&without, &[("src/d.rs", imp)]);
    assert_eq!(from(&added, "src/callers.rs:o@2"), targets(&["src/t.rs:hello@1"]));
    let mut with = base.to_vec();
    with.push(("src/d.rs", imp));
    let removed = synced_equals_reindexed(&with, &[("src/d.rs", "\n")]);
    assert!(from(&removed, "src/callers.rs:o@2").is_empty(), "{removed:?}");
}

/// The last hop of a `Deref` chain removed takes back the default a type
/// past it lends, although the change spells neither the caller's type nor
/// the method, and nothing left in the graph reaches the lending type from
/// the caller's (sprint 92 review).
#[test]
fn removing_the_last_deref_hop_takes_back_a_lent_default_on_sync() {
    let initial = [
        ("src/lib.rs", "pub mod t;\npub mod d;\npub mod callers;\n"),
        (
            "src/t.rs",
            "\
pub trait Greet { fn hello(&self) {} }
pub struct Inner;
impl Greet for Inner {}
pub struct Mid;
pub struct Outer;
impl std::ops::Deref for Outer {
    type Target = Mid;
    fn deref(&self) -> &Mid { &Mid }
}
",
        ),
        ("src/callers.rs", "use crate::t::{Greet, Outer};\npub fn o(x: &Outer) { x.hello(); }\n"),
        (
            "src/d.rs",
            "use super::t::*;\nimpl std::ops::Deref for Mid {\n    type Target = Inner;\n    fn deref(&self) -> &Inner { &Inner }\n}\n",
        ),
    ];
    let index_before = tree(&initial);
    let before = index(index_before.path());
    assert_eq!(
        from(&call_edges(before.runtime().unwrap()), "src/callers.rs:o@2"),
        targets(&["src/t.rs:hello@1"])
    );
    let removed = synced_equals_reindexed(&initial, &[("src/d.rs", "\n")]);
    assert!(from(&removed, "src/callers.rs:o@2").is_empty(), "{removed:?}");
}

/// A `Deref` impl added in a file of its own makes the outer types' callers
/// reach the methods along the whole chain on sync — the target's own, a
/// method and a trait default of a type further down the chain that the
/// change never spells, and through a first hop it does not touch — and
/// removed, it takes them back.
#[test]
fn a_deref_impl_added_or_removed_rebinds_on_sync() {
    let base = [
        ("src/lib.rs", "pub mod t;\npub mod d;\npub mod callers;\n"),
        (
            "src/t.rs",
            "\
pub trait Wave { fn wave(&self) {} }
pub struct Deeper;
impl Deeper { pub fn far(&self) {} }
impl Wave for Deeper {}
pub struct Inner;
impl Inner { pub fn deep(&self) {} }
impl std::ops::Deref for Inner {
    type Target = Deeper;
    fn deref(&self) -> &Deeper { &Deeper }
}
pub struct Mid;
pub struct Outer;
impl std::ops::Deref for Outer {
    type Target = Mid;
    fn deref(&self) -> &Mid { &Mid }
}
",
        ),
        (
            "src/callers.rs",
            "use crate::t::{Mid, Outer, Wave};\npub fn m(x: &Mid) { x.deep(); }\npub fn o(x: &Outer) { x.deep(); x.far(); x.wave(); }\n",
        ),
    ];
    // A glob, not a named import: only the impl block's header spells the
    // types, and nothing it spells is a segment of the callers' imports.
    let deref = "use super::t::*;\nimpl std::ops::Deref for Mid {\n    type Target = Inner;\n    fn deref(&self) -> &Inner { &Inner }\n}\n";
    let mut without = base.to_vec();
    without.push(("src/d.rs", "\n"));
    let added = synced_equals_reindexed(&without, &[("src/d.rs", deref)]);
    assert_eq!(from(&added, "src/callers.rs:m@2"), targets(&["src/t.rs:deep@6"]), "{added:?}");
    assert_eq!(
        from(&added, "src/callers.rs:o@3"),
        targets(&["src/t.rs:deep@6", "src/t.rs:far@3", "src/t.rs:wave@1"]),
        "{added:?}"
    );
    let mut with = base.to_vec();
    with.push(("src/d.rs", deref));
    let removed = synced_equals_reindexed(&with, &[("src/d.rs", "\n")]);
    for caller in ["src/callers.rs:m@2", "src/callers.rs:o@3"] {
        assert!(from(&removed, caller).is_empty(), "{caller}: {removed:?}");
    }
}
