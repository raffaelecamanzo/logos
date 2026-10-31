//! Focus counts a Rust `impl` method and a Go receiver method toward the
//! struct their recorded type names (S-510, [CR-166], metric semantics v9),
//! driven end to end through the public [`Engine`] façade against real Rust and
//! Go extraction.
//!
//! - The 25-method AC holds across the shapes real code has: a Rust struct
//!   whose methods sit in an inherent `impl` and a trait `impl` in two files of
//!   one crate, a Go type whose methods have pointer and value receivers in two
//!   files of one package. A 5-method struct of each is not god.
//! - A method naming zero structs of its crate/package, or two same-named
//!   structs of it, counts toward none, and the unattributed count is reported
//!   with its denominator. A same-named struct of another crate/package is
//!   never a candidate.
//! - Nothing else moves: the method stays `Contains`-ed by its module, and
//!   Cohesion (which scores `Class` alone) reads nothing of the attribution.
//!
//! [CR-166]: ../../docs/requests/CR-166-rust-and-go-methods-count-toward-their-type-for-focus.md

use std::fs;
use std::path::Path;

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::{Engine, Granularity};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Index `root` and record one snapshot under the default thresholds.
fn scan(root: &Path) -> RecordedSnapshot {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    metrics::snapshot(
        rt,
        &view,
        None,
        metrics::Thresholds::default(),
        &engine
            .registry()
            .expect("a started engine holds its registry")
            .member_access_extensions(),
    )
    .expect("snapshot runs")
}

/// The names the Focus offender list reports (the god containers).
fn god_names(s: &RecordedSnapshot) -> Vec<&str> {
    s.worst_offenders.focus.iter().map(|o| o.name.as_str()).collect()
}

/// `n` Rust methods `w0..` reading `self.x`, as the body of an `impl`.
fn rust_methods(n: usize) -> String {
    (0..n)
        .map(|i| format!("    pub fn w{i}(&self) -> i32 {{\n        self.x + {i}\n    }}\n"))
        .collect()
}

/// `n` Go methods `W0..` on `recv` (`s *T` or `s T`) reading `s.x`.
fn go_methods(recv: &str, first: usize, n: usize) -> String {
    (first..first + n)
        .map(|i| format!("func ({recv}) W{i}() int {{\n\treturn s.x + {i}\n}}\n\n"))
        .collect()
}

/// The 25-method AC on Rust with the methods split the way real code splits
/// them: `Big`'s impls sit in two files of one crate — 13 inherent methods in
/// the file declaring it, 12 of a trait impl in a submodule's directory — and a 5-method
/// `Small` beside it. Both are counted toward their struct, `Big` alone is god.
#[test]
fn a_rust_struct_with_inherent_and_trait_impls_in_two_files_is_god() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        &format!(
            "mod inner;\n\npub struct Big {{\n    x: i32,\n}}\n\nimpl Big {{\n{}}}\n\npub struct Small {{\n    x: i32,\n}}\n\nimpl Small {{\n{}}}\n",
            rust_methods(13),
            rust_methods(5),
        ),
    );
    let trait_methods: String = (0..12)
        .map(|i| format!("    fn t{i}(&self) -> i32 {{\n        self.x + {i}\n    }}\n"))
        .collect();
    let trait_decls: String = (0..12).map(|i| format!("    fn t{i}(&self) -> i32;\n")).collect();
    write(tmp.path(), "src/inner/mod.rs", "pub mod traits;\n");
    write(
        tmp.path(),
        "src/inner/traits.rs",
        &format!(
            "use crate::Big;\n\npub trait Worker {{\n{trait_decls}}}\n\nimpl Worker for Big {{\n{trait_methods}}}\n"
        ),
    );
    let s = scan(tmp.path());

    assert_eq!(god_names(&s), ["Big"], "13 + 12 bodied methods → god; 5 → not");
    assert_eq!(s.metrics.focus.expect("structs are containers").raw, 0.5);
    let reach = s.metrics.focus_attribution.expect("the methods record a self type");
    assert_eq!((reach.unattributed, reach.denominator), (0, 30), "every method reached its struct");
}

/// The 25-method AC on Go with pointer and value receivers in two files of one
/// package: 14 pointer-receiver methods beside the type, 11 value-receiver
/// methods in another file, and a 5-method type.
#[test]
fn a_go_type_with_pointer_and_value_receivers_in_two_files_is_god() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "svc/big.go",
        &format!(
            "package svc\n\ntype Big struct {{\n\tx int\n}}\n\ntype Small struct {{\n\tx int\n}}\n\n{}{}",
            go_methods("s *Big", 0, 14),
            go_methods("s Small", 0, 5),
        ),
    );
    write(
        tmp.path(),
        "svc/more.go",
        &format!("package svc\n\n{}", go_methods("s Big", 14, 11)),
    );
    let s = scan(tmp.path());

    assert_eq!(god_names(&s), ["Big"], "14 pointer + 11 value methods → god; 5 → not");
    assert_eq!(s.metrics.focus.expect("Go types are containers").raw, 0.5);
    let reach = s.metrics.focus_attribution.expect("the receivers record a base type");
    assert_eq!((reach.unattributed, reach.denominator), (0, 30));
}

/// A Rust method counts toward a struct of its **crate** only. Three crates:
/// `app` holds `Lone` (25 methods, its impl in a sibling module) and `Twin`
/// declared in two modules; `lib2` holds a `Big` whose 25 methods are
/// implemented in `app` — where no `Big` exists. Only `Lone` is god; `Twin`'s
/// methods and the 25 on `Big` are unattributed, and `lib2`'s own `Big` is
/// never a candidate for them.
#[test]
fn a_rust_method_names_one_struct_of_its_own_crate_or_none() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "app/src/lib.rs",
        &format!(
            "mod big_impl;\nmod twin_a;\nmod twin_b;\n\npub struct Lone {{\n    x: i32,\n}}\n\nimpl Lone {{\n{}}}\n",
            rust_methods(25)
        ),
    );
    write(
        tmp.path(),
        "app/src/big_impl.rs",
        &format!("use lib2::Big;\n\nimpl Big {{\n{}}}\n", rust_methods(25)),
    );
    write(
        tmp.path(),
        "app/src/twin_a.rs",
        &format!("pub struct Twin {{\n    x: i32,\n}}\n\nimpl Twin {{\n{}}}\n", rust_methods(22)),
    );
    write(tmp.path(), "app/src/twin_b.rs", "pub struct Twin {\n    x: i32,\n}\n");
    write(tmp.path(), "lib2/src/lib.rs", "pub struct Big {\n    x: i32,\n}\n");
    let s = scan(tmp.path());

    assert_eq!(
        god_names(&s),
        ["Lone"],
        "Twin (two structs of the crate) and Big (another crate's) attribute nothing"
    );
    let reach = s.metrics.focus_attribution.expect("the methods record a self type");
    assert_eq!(
        (reach.unattributed, reach.denominator),
        (22 + 25, 25 + 22 + 25),
        "Twin's 22 and the 25 naming lib2's Big, of 72 methods"
    );
}

/// A Go receiver method counts toward the struct of its **package** only: a
/// second package declaring `Big` too is no candidate for the first's methods,
/// whichever of the two has the 25, and neither is one declared in a package
/// nested beneath it; and a package declaring `Dup` twice attributes nothing.
#[test]
fn a_go_method_names_one_struct_of_its_own_package_or_none() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "one/big.go",
        &format!("package one\n\ntype Big struct {{\n\tx int\n}}\n\n{}", go_methods("s *Big", 0, 25)),
    );
    write(tmp.path(), "two/big.go", "package two\n\ntype Big struct {\n\tx int\n}\n");
    // A package nested beneath `one` is another package, not part of it.
    write(tmp.path(), "one/inner/big.go", "package inner\n\ntype Big struct {\n\tx int\n}\n");
    write(
        tmp.path(),
        "three/dup.go",
        &format!("package three\n\ntype Dup struct {{\n\tx int\n}}\n\n{}", go_methods("s *Dup", 0, 25)),
    );
    write(tmp.path(), "three/dup2.go", "package three\n\ntype Dup struct {\n\tx int\n}\n");
    let s = scan(tmp.path());

    assert_eq!(
        god_names(&s),
        ["Big"],
        "package one's Big is god; package three's Dup is ambiguous, package two's Big has none"
    );
    let reach = s.metrics.focus_attribution.expect("the receivers record a base type");
    assert_eq!((reach.unattributed, reach.denominator), (25, 50), "the 25 on the doubled Dup");
}

/// Associated functions count: `Edge` has 19 methods taking `&self` and a
/// `new` that takes none — 20, the god threshold — and an `impl` on an enum
/// names no struct, so its 25 methods count toward none.
#[test]
fn associated_functions_count_and_an_enum_impl_names_no_struct() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        &format!(
            "pub struct Edge {{\n    x: i32,\n}}\n\nimpl Edge {{\n    pub fn new() -> Self {{\n        Edge {{ x: 0 }}\n    }}\n{}}}\n\npub enum Mode {{\n    A,\n}}\n\nimpl Mode {{\n{}}}\n",
            rust_methods(19),
            (0..25)
                .map(|i| format!("    pub fn m{i}(&self) -> i32 {{\n        {i}\n    }}\n"))
                .collect::<String>(),
        ),
    );
    let s = scan(tmp.path());

    assert_eq!(god_names(&s), ["Edge"], "19 methods + the associated `new` is 20; the enum's 25 are unattributed");
    let reach = s.metrics.focus_attribution.expect("the methods record a self type");
    assert_eq!((reach.unattributed, reach.denominator), (25, 45));
}

/// A candidate struct is of the method's own language and its own crate. A Go
/// struct named like a Rust one — in the repository root, where the crate and
/// the directory share one path — is no candidate
/// for the Rust `impl` (which stays attributed to its own struct, or stays
/// unattributed when it names an enum), and two crates whose directories share
/// a last name are two crates.
#[test]
fn a_candidate_is_of_the_methods_language_and_crate() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        &format!(
            "pub struct Server {{\n    x: i32,\n}}\n\nimpl Server {{\n{}}}\n\npub enum Mode {{\n    A,\n}}\n\nimpl Mode {{\n{}}}\n",
            rust_methods(25),
            (0..25)
                .map(|i| format!("    pub fn m{i}(&self) -> i32 {{\n        {i}\n    }}\n"))
                .collect::<String>(),
        ),
    );
    write(tmp.path(), "server.go", "package tool\n\ntype Server struct {\n\tx int\n}\n\ntype Mode struct {\n\tx int\n}\n");
    write(
        tmp.path(),
        "libs/util/src/lib.rs",
        &format!("pub struct Buf {{\n    x: i32,\n}}\n\nimpl Buf {{\n{}}}\n", rust_methods(25)),
    );
    write(tmp.path(), "tools/util/src/lib.rs", "pub struct Buf {\n    x: i32,\n}\n");
    let s = scan(tmp.path());

    let mut gods = god_names(&s);
    gods.sort_unstable();
    assert_eq!(
        gods,
        ["Buf", "Server"],
        "the Go `Server` and `Mode` and the other crate's `Buf` are no candidates"
    );
    let reach = s.metrics.focus_attribution.expect("the methods record a self type");
    assert_eq!((reach.unattributed, reach.denominator), (25, 75), "the enum's 25 of 75");
}

/// Attribution moves nothing but Focus: the method stays `Contains`-ed by its
/// module — the struct has no `Contains` child — and Cohesion, which scores
/// `Class` alone, is unscoreable both with and without the methods.
#[test]
fn attribution_leaves_containment_and_cohesion_alone() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        &format!("pub struct Big {{\n    x: i32,\n}}\n\nimpl Big {{\n{}}}\n", rust_methods(25)),
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let rt = engine.runtime().expect("runtime");
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("the graph reads");
    let big = nodes.iter().find(|n| n.name == "Big").expect("the struct");
    let w0 = nodes.iter().find(|n| n.name == "w0").expect("a method");
    let contains: Vec<_> = edges
        .iter()
        .filter(|e| e.kind == logos_core::model::EdgeKind::Contains)
        .collect();
    assert!(
        contains.iter().all(|e| e.source != big.id),
        "the struct is the Contains parent of nothing"
    );
    let parent = contains.iter().find(|e| e.target == w0.id).expect("the method has a parent");
    let module = nodes.iter().find(|n| n.id == parent.source).expect("parent node");
    assert_eq!(module.kind, logos_core::model::NodeKind::Module, "its module stays its parent");
    let s = scan(tmp.path());
    assert!(s.metrics.cohesion.is_none(), "a Struct is no Cohesion class");
}

/// The `no_god_containers` budget counts the same containers Focus does: over
/// the attributed methods of a Rust struct and of a Go type, under
/// `god_methods = 4`, exactly the 5-method one of each is a violation and the
/// 2-method one is not. The budget reads the links through `check`/`scan`'s own
/// store read, not through the metrics'.
#[test]
fn the_god_container_budget_counts_attributed_methods() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[constraints]\nno_god_containers = true\n\n[metric_thresholds]\ngod_methods = 4\n",
    );
    write(
        tmp.path(),
        "src/lib.rs",
        &format!(
            "pub struct Wide {{\n    x: i32,\n}}\n\nimpl Wide {{\n{}}}\n\npub struct Narrow {{\n    x: i32,\n}}\n\nimpl Narrow {{\n{}}}\n",
            rust_methods(5),
            rust_methods(2)
        ),
    );
    write(
        tmp.path(),
        "svc/svc.go",
        &format!(
            "package svc\n\ntype GoWide struct {{\n\tx int\n}}\n\ntype GoNarrow struct {{\n\tx int\n}}\n\n{}{}",
            go_methods("s *GoWide", 0, 5),
            go_methods("s GoNarrow", 0, 2)
        ),
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let scanned = engine.scan(false).expect("scan runs");

    let mut gods: Vec<&str> = scanned
        .violations
        .iter()
        .filter(|v| v.rule == "no_god_containers")
        .map(|v| v.message.as_str())
        .collect();
    gods.sort_unstable();
    assert_eq!(gods.len(), 2, "{gods:?}");
    assert!(gods[0].contains("GoWide") && gods[1].contains("Wide"), "{gods:?}");
}

/// A repository whose languages record no method self type reports no
/// attribution — `null`, not `0 of 0`.
#[test]
fn a_repository_without_typed_methods_reports_no_attribution() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Svc.java",
        "package s;\n\npublic class Svc {\n    private int x;\n\n    public int work() {\n        return this.x;\n    }\n}\n",
    );
    let s = scan(tmp.path());
    assert!(s.metrics.focus_attribution.is_none());
    assert!(s.metrics.focus.is_some(), "the class is still a Focus container");
}
