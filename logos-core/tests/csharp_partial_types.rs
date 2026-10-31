//! The parts of a C# partial type form one type (S-557, CR-177, FR-RS-29,
//! FR-RS-13, NFR-RA-05, NFR-RA-06) — exercised end to end through the public
//! [`Engine`] façade against temp-directory fixtures, with the SHIPPED
//! `references.scm` and `plugin.toml` of the C# plugin (no query override).
//!
//! What the fixture pins, over the two parts of `App.Foo` in `src/Foo.1.cs`
//! (first in path order) and `src/Foo.2.cs`:
//!
//! - a call from one part to a member another part declares binds, in both
//!   directions, bare (`B()`) and through `this` (`this.A()`), and so does an
//!   own-field access (`this.F`) to a field of another part;
//! - the base list one part writes (`: Base`, in the later part) is the whole
//!   type's: a call in the first part reaches `Base.H()`;
//! - the type's name names one type: `Foo.S()`, `new Foo()` and `: Foo` bind,
//!   to the first-declared part;
//! - each part keeps its node, and the later part records `Extends` to the
//!   first; the first part's own row binds nothing;
//! - nested parts (`Outer.Inner`, written in both parts of `Outer`) are one type
//!   too;
//! - a `partial` declaration beside a plain one of the same name, or a partial
//!   class beside a partial struct, is two types: their name stays ambiguous and
//!   nothing is united (NFR-RA-05);
//! - the parts of one type lie in one directory tree, which stands in for the
//!   project: a part below another's directory joins it, while two projects'
//!   `partial class Program` are two types;
//! - Focus and Cohesion score the parts as one container, and Cohesion links a
//!   `this.x` read of a field another part declares but not a bare `x` (S-556
//!   sees a bare read only of a field its own part declares).
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`csharp_call_forms.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-c-sharp")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId};
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

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// Every bound edge of `kind` as `(source label, target label)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Edges {
    let label = labels(rt);
    let mut out: Edges = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// The `Extends` rows aliased `partial` as `(source label, resolved)`, sorted.
fn partial_rows(rt: &Runtime) -> Vec<(String, bool)> {
    let label = labels(rt);
    let by_symbol: HashMap<String, String> = rt
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .map(|n| (n.symbol.as_str().to_string(), label[&n.id].clone()))
                .collect())
        })
        .expect("read runs");
    let mut rows: Vec<(String, bool)> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Extends && r.alias.as_deref() == Some("partial"))
        .map(|r| (by_symbol[&r.source_symbol].clone(), r.resolved))
        .collect();
    rows.sort();
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const BASE: &str = "\
namespace App {
    public class Base { public int H() { return 1; } }
}
";

const FOO_A: &str = "\
namespace App {
    public partial class Foo {
        public int F;
        public int A() { return B(); }
        public int K() { return H(); }
    }
}
";

// The later part writes the base list: it is the whole type's, and the part's
// own row is no second base beside it.
const FOO_B: &str = "\
namespace App {
    partial class Foo : Base {
        public int B() { return this.A(); }
        public int G() { return this.F; }
        public static int S() { return 2; }
    }
}
";

const USER: &str = "\
namespace App {
    class User {
        int U() { return Foo.S(); }
        object N() { return new Foo(); }
    }
    class Sub : Foo {
        int V() { return A(); }
    }
}
";

fn foo() -> TempDir {
    tree(&[
        ("src/Base.cs", BASE),
        ("src/Foo.1.cs", FOO_A),
        ("src/Foo.2.cs", FOO_B),
        ("src/User.cs", USER),
    ])
}

#[test]
fn a_call_between_parts_binds_and_the_base_one_part_writes_is_the_types() {
    let tmp = foo();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            // `B()` in part 1, declared in part 2.
            edge("src/Foo.1.cs:A@4", "src/Foo.2.cs:B@3"),
            // `H()` in part 1, inherited through the base part 2 writes.
            edge("src/Foo.1.cs:K@5", "src/Base.cs:H@2"),
            // `this.A()` in part 2, declared in part 1.
            edge("src/Foo.2.cs:B@3", "src/Foo.1.cs:A@4"),
            // `Foo.S()` names one type, whose later part declares `S`.
            edge("src/User.cs:U@3", "src/Foo.2.cs:S@5"),
            // A subclass climbs into the union too.
            edge("src/User.cs:V@7", "src/Foo.1.cs:A@4"),
        ]
    );
    // `this.F` in part 2 reads the field part 1 declares.
    assert_eq!(
        edges(rt, EdgeKind::Accesses),
        vec![edge("src/Foo.2.cs:G@4", "src/Foo.1.cs:F@3")]
    );
}

#[test]
fn the_types_name_names_the_first_part_and_the_later_part_extends_it() {
    let tmp = foo();
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        vec![edge("src/User.cs:N@4", "src/Foo.1.cs:Foo@2")]
    );
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        vec![
            // The base list part 2 writes is its own row's edge alone.
            edge("src/Foo.2.cs:Foo@2", "src/Base.cs:Base@2"),
            // The later part links to the first-declared one.
            edge("src/Foo.2.cs:Foo@2", "src/Foo.1.cs:Foo@2"),
            edge("src/User.cs:Sub@6", "src/Foo.1.cs:Foo@2"),
        ]
    );
    // Each part keeps its node and records its row; only the later one binds.
    assert_eq!(
        partial_rows(rt),
        vec![("src/Foo.1.cs:Foo@2".to_string(), false), ("src/Foo.2.cs:Foo@2".to_string(), true)]
    );
    // The link is told from the supertypes: it alone is what the metrics unite
    // the parts by.
    assert_eq!(partial_links(rt), vec![edge("src/Foo.2.cs:Foo@2", "src/Foo.1.cs:Foo@2")]);
}

/// The partial type links the metrics read (`partial_type_links`), labelled.
fn partial_links(rt: &Runtime) -> Edges {
    let label = labels(rt);
    rt.submit_read(|store| store.partial_type_links())
        .expect("read runs")
        .into_iter()
        .map(|(part, first)| (label[&part].clone(), label[&first].clone()))
        .collect()
}

#[test]
fn a_doc_mention_of_a_partial_types_name_names_its_first_part() {
    let tmp = tree(&[
        ("src/Base.cs", BASE),
        ("src/Foo.1.cs", FOO_A),
        ("src/Foo.2.cs", FOO_B),
        ("README.md", "# App\n\nThe type `Foo` is partial.\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // A code name in prose binds on exactly one candidate: the type, never
    // two of its parts (`by_name` holds the first part alone).
    let targets: Vec<String> = edges(rt, EdgeKind::DocReference)
        .into_iter()
        .filter(|(from, _)| from.starts_with("README.md:"))
        .map(|(_, to)| to)
        .collect();
    assert_eq!(targets, vec!["src/Foo.1.cs:Foo@2".to_string()]);
}

#[test]
fn parts_naming_two_base_classes_have_no_supertype_to_climb() {
    let bases = "namespace App {\n    public class A { public int H() { return 1; } }\n    public class B { }\n}\n";
    let tmp = tree(&[
        ("src/Bases.cs", bases),
        ("src/Foo.1.cs", "namespace App {\n    partial class Foo : A { int K() { return H(); } }\n}\n"),
        ("src/Foo.2.cs", "namespace App {\n    partial class Foo : B { }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Two base classes across the parts are several bases, as one declaration
    // naming both would be: `H()` names neither base's member (NFR-RA-05).
    assert_eq!(edges(rt, EdgeKind::Calls), Edges::new());
}

#[test]
fn a_partial_interface_pools_the_super_interfaces_its_parts_name() {
    let supers = "namespace App {\n    interface IA { int MA() { return 1; } }\n    interface IB { }\n}\n";
    let tmp = tree(&[
        ("src/Supers.cs", supers),
        ("src/IFoo.1.cs", "namespace App {\n    partial interface IFoo : IA { }\n}\n"),
        ("src/IFoo.2.cs", "namespace App {\n    partial interface IFoo : IB { int X() { return MA(); } }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // An interface's super-interfaces are pooled, never several bases: the
    // default body `MA()` of the one the other part names is reached.
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![edge("src/IFoo.2.cs:X@2", "src/Supers.cs:MA@2")]
    );
}

const OUTER_A: &str = "\
namespace App {
    partial class Outer {
        partial class Inner { int X() { return Y(); } }
    }
}
";

const OUTER_B: &str = "\
namespace App {
    partial class Outer {
        partial class Inner { int Y() { return 1; } }
        int Z() { return Inner.W(); }
    }
}
";

const OUTER_C: &str = "\
namespace App {
    partial class Outer {
        partial class Inner { public static int W() { return 2; } }
    }
}
";

#[test]
fn nested_parts_in_the_parts_of_their_enclosing_type_are_one_type() {
    let tmp = tree(&[
        ("src/Outer.1.cs", OUTER_A),
        ("src/Outer.2.cs", OUTER_B),
        ("src/Outer.3.cs", OUTER_C),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            edge("src/Outer.1.cs:X@3", "src/Outer.2.cs:Y@3"),
            // `Inner` read in `Outer` names one type, whose parts declare `W`.
            edge("src/Outer.2.cs:Z@4", "src/Outer.3.cs:W@3"),
        ]
    );
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        vec![
            edge("src/Outer.2.cs:Inner@3", "src/Outer.1.cs:Inner@3"),
            edge("src/Outer.2.cs:Outer@2", "src/Outer.1.cs:Outer@2"),
            edge("src/Outer.3.cs:Inner@3", "src/Outer.1.cs:Inner@3"),
            edge("src/Outer.3.cs:Outer@2", "src/Outer.1.cs:Outer@2"),
        ]
    );
}

const TWIN_PARTIAL: &str = "\
namespace App {
    public partial class Twin { public static int Run() { return 1; } int P() { return Q(); } }
    public partial class Pair { public static int Run() { return 1; } }
}
";

const TWIN_PLAIN: &str = "\
namespace App {
    public class Twin { public int Q() { return 2; } }
    public partial struct Pair { }
}
";

const TWIN_CALLER: &str = "\
namespace App {
    class C {
        int A() { return Twin.Run(); }
        int B() { return Pair.Run(); }
    }
}
";

#[test]
fn a_partial_beside_a_plain_or_other_kind_declaration_is_two_types() {
    let tmp = tree(&[
        ("src/Twin.1.cs", TWIN_PARTIAL),
        ("src/Twin.2.cs", TWIN_PLAIN),
        ("src/C.cs", TWIN_CALLER),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Neither name names one type, and `Q()` is not read in the plain `Twin`.
    assert_eq!(edges(rt, EdgeKind::Calls), Edges::new());
    assert_eq!(edges(rt, EdgeKind::Extends), Edges::new());
    // Every part's row stays unbound: no part is any other's.
    assert!(partial_rows(rt).iter().all(|(_, resolved)| !resolved), "{:?}", partial_rows(rt));
}

const PROGRAM: &str = "\
public partial class Program { public static int Run() { return 1; } }
";

const PROGRAM_CALLER: &str = "\
class Probe { int P() { return Program.Run(); } }
";

const MODEL: &str = "\
namespace App.Models {
    public partial class Order { public int Total() { return Tax(); } }
}
";

const MODEL_GENERATED: &str = "\
namespace App.Models {
    partial class Order { int Tax() { return 1; } }
}
";

#[test]
fn the_parts_of_one_type_lie_in_one_directory_tree() {
    let tmp = tree(&[
        ("src/Catalog/Program.cs", PROGRAM),
        ("src/Ordering/Program.cs", PROGRAM),
        ("src/Probe.cs", PROGRAM_CALLER),
        ("src/Models/Order.cs", MODEL),
        ("src/Models/Generated/Order.g.cs", MODEL_GENERATED),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Two projects' `Program`s are two types: `Program.Run()` names neither.
    // The generated part below the model's directory is the model's — and,
    // first in path order, its first-declared part.
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![edge("src/Models/Order.cs:Total@2", "src/Models/Generated/Order.g.cs:Tax@2")]
    );
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        vec![edge("src/Models/Order.cs:Order@2", "src/Models/Generated/Order.g.cs:Order@2")]
    );
}

const BOX: &str = "\
namespace App { public partial class Box { public static int Make() { return 1; } } }
";

const BOX_T: &str = "\
namespace App { public partial class Box<T> { int X() { return Make(); } public static int Only() { return 2; } } }
";

const BOX_T_PART: &str = "\
namespace App { partial class Box<T> { int Y() { return Only(); } } }
";

const BOX_USER: &str = "\
namespace App { class User { int U() { return Box.Only(); } } }
";

#[test]
fn a_generic_and_a_plain_partial_of_one_name_are_two_types() {
    let tmp = tree(&[
        ("src/Box.cs", BOX),
        ("src/BoxT.1.cs", BOX_T),
        ("src/BoxT.2.cs", BOX_T_PART),
        ("src/User.cs", BOX_USER),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // `Box<T>`'s two parts are one type: `Only()` in its later part binds. Its
    // `Make()` is `Box`'s, another type, and stays unbound; so does `Box.Only()`,
    // whose name still names two types (S-557 reads no type arity at a use).
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![edge("src/BoxT.2.cs:Y@1", "src/BoxT.1.cs:Only@1")]
    );
    assert_eq!(
        edges(rt, EdgeKind::Extends),
        vec![edge("src/BoxT.2.cs:Box@1", "src/BoxT.1.cs:Box@1")]
    );
    // One row per declaration, generic or not.
    assert_eq!(partial_rows(rt).len(), 3, "{:?}", partial_rows(rt));
}

#[test]
fn a_part_at_the_repository_root_holds_the_parts_below_it() {
    let tmp = tree(&[
        ("Foo.cs", "partial class Foo { int A() { return B(); } }\n"),
        ("Gen/Foo.g.cs", "partial class Foo { int B() { return 1; } }\n"),
    ]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // The root directory encloses every other: its part's tree holds them all.
    assert_eq!(edges(rt, EdgeKind::Calls), vec![edge("Foo.cs:A@1", "Gen/Foo.g.cs:B@1")]);
}

// ── The metrics, from the store ─────────────────────────────────────────────

#[test]
fn a_scan_scores_a_partial_type_as_one_container() {
    // `god_methods = 4`: each part's two methods are under it, the type's four
    // are not — god only as the union. The scan reads the links from the store
    // into Focus, its offenders, the ranking and the `no_god_containers` budget.
    let tmp = tree(&[
        (".logos/rules.toml", "[constraints]\nno_god_containers = true\n\n[metric_thresholds]\ngod_methods = 4\n"),
        ("src/Foo.1.cs", "namespace App {\n    partial class Foo { int A() { return 1; } int B() { return 2; } }\n}\n"),
        ("src/Foo.2.cs", "namespace App {\n    partial class Foo { int C() { return 3; } int D() { return 4; } }\n}\n"),
        ("src/Bar.cs", "namespace App {\n    class Bar { int E() { return 5; } }\n}\n"),
    ]);
    let engine = index(tmp.path());
    let scanned = engine.scan(false).expect("scan runs");
    assert_eq!(scanned.metrics.focus.expect("containers exist").raw, 0.5, "Foo and Bar; Foo god");
    let focus: Vec<(&str, &str)> = scanned
        .worst_offenders
        .focus
        .iter()
        .map(|o| (o.name.as_str(), o.file.as_str()))
        .collect();
    assert_eq!(focus, vec![("Foo", "src/Foo.1.cs")]);
    let gods: Vec<&str> = scanned
        .violations
        .iter()
        .filter(|v| v.rule == "no_god_containers")
        .map(|v| v.message.as_str())
        .collect();
    assert_eq!(gods.len(), 1, "{gods:?}");
    assert!(gods[0].contains("Foo"), "{gods:?}");
    let ranked: Vec<&str> = scanned
        .where_to_start
        .units
        .iter()
        .filter(|u| u.dimensions.contains(&"focus"))
        .map(|u| u.file.as_str())
        .collect();
    assert_eq!(ranked, vec!["src/Foo.1.cs"]);
}

#[test]
fn cohesion_links_a_this_access_across_parts_but_not_a_bare_read() {
    // S-556 × S-557: `_n` is declared in the first part. `Get` reads it bare in
    // that part and `Peek` through `this` in the other: both bind, one component.
    // `Put`'s bare `_n` in the other part records nothing — a bare read is an
    // access only of a field its own part declares — so it links nothing and
    // the union scores LCOM4 2, not 1.
    let tmp = tree(&[
        ("src/Foo.1.cs", "namespace App {\n    partial class Foo { int _n; int Get() { return _n; } }\n}\n"),
        (
            "src/Foo.2.cs",
            "namespace App {\n    partial class Foo { void Put(int v) { _n = v; } int Peek() { return this._n; } }\n}\n",
        ),
    ]);
    let engine = index(tmp.path());
    let scanned = engine.scan(false).expect("scan runs");
    let lcom4: Vec<(&str, &str, &str)> = scanned
        .worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.file.as_str(), o.detail.as_str()))
        .collect();
    assert_eq!(lcom4, vec![("Foo", "src/Foo.1.cs", "LCOM4 2")]);
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` and `Extends` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> (Edges, Edges) {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        let path = tmp_a.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
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
    (edges(rt_a, EdgeKind::Calls), edges(rt_a, EdgeKind::Extends))
}

const PART_A: &str = "\
namespace App {
    partial class Foo { int A() { return B(); } }
}
";

const PART_B: &str = "\
namespace App {
    partial class Foo { int B() { return 1; } }
}
";

#[test]
fn a_synced_part_matches_a_fresh_reindex() {
    let initial = [("src/Foo.1.cs", PART_A), ("src/Foo.2.cs", PART_B)];

    // The part declaring the callee loses `partial`: the two declarations are
    // two types, and the call in the unchanged part unbinds.
    let plain = PART_B.replace("partial class", "class");
    let (calls, extends) = synced_equals_reindexed(&initial, &[("src/Foo.2.cs", &plain)]);
    assert_eq!(calls, Edges::new());
    assert_eq!(extends, Edges::new());

    // A third part loses `partial`: the declarations are no longer one type,
    // and the call between the two unchanged parts unbinds, though neither
    // file changed and the change spells neither the caller nor the callee.
    let third = "namespace App {\n    partial class Foo { }\n}\n";
    let plain_third = "namespace App {\n    class Foo { }\n}\n";
    let three = [("src/Foo.1.cs", PART_A), ("src/Foo.2.cs", PART_B), ("src/Foo.3.cs", third)];
    let (calls, extends) = synced_equals_reindexed(&three, &[("src/Foo.3.cs", plain_third)]);
    assert_eq!(calls, Edges::new());
    assert_eq!(extends, Edges::new());

    // The same for an own-field access across the parts: a third part losing
    // `partial` unbinds `this.F`, re-selected though it spells neither file.
    let field = "namespace App {\n    partial class Foo { public int F; }\n}\n";
    let reader = "namespace App {\n    partial class Foo { int G() { return this.F; } }\n}\n";
    let three = [("src/Foo.1.cs", field), ("src/Foo.2.cs", reader), ("src/Foo.3.cs", third)];
    synced_equals_reindexed(&three, &[("src/Foo.3.cs", plain_third)]);

    // The callee's part goes away: the call unbinds, though its file is unchanged.
    let gone = "namespace App {\n    partial class Foo { int C() { return 1; } }\n}\n";
    let (calls, extends) = synced_equals_reindexed(&initial, &[("src/Foo.2.cs", gone)]);
    assert_eq!(calls, Edges::new());
    assert_eq!(extends, vec![edge("src/Foo.2.cs:Foo@2", "src/Foo.1.cs:Foo@2")]);

    // A part arrives first in path order: it becomes the first-declared part,
    // and both earlier parts link to it.
    let first = "namespace App {\n    partial class Foo { }\n}\n";
    let (calls, extends) = synced_equals_reindexed(&initial, &[("src/Foo.0.cs", first)]);
    assert_eq!(calls, vec![edge("src/Foo.1.cs:A@2", "src/Foo.2.cs:B@2")]);
    assert_eq!(
        extends,
        vec![
            edge("src/Foo.1.cs:Foo@2", "src/Foo.0.cs:Foo@2"),
            edge("src/Foo.2.cs:Foo@2", "src/Foo.0.cs:Foo@2"),
        ]
    );

    // Both parts change in one sync, the first part's file persisted first: its
    // capture of the later part's link must not cost the link its payload, or
    // the metrics would count two containers where a reindex counts one.
    let moved_a = format!("// moved\n{PART_A}");
    let moved_b = format!("// moved\n{PART_B}");
    let tmp = tree(&initial);
    let engine = index(tmp.path());
    let root = tmp.path().canonicalize().expect("canonicalize root");
    fs::write(tmp.path().join("src/Foo.1.cs"), &moved_a).unwrap();
    fs::write(tmp.path().join("src/Foo.2.cs"), &moved_b).unwrap();
    engine.sync(&[root.join("src/Foo.1.cs"), root.join("src/Foo.2.cs")]);
    assert_eq!(
        partial_links(engine.runtime().unwrap()),
        vec![edge("src/Foo.2.cs:Foo@3", "src/Foo.1.cs:Foo@3")]
    );

    // And the second part arrives after the first: the call binds on sync.
    let (calls, extends) = synced_equals_reindexed(&[("src/Foo.1.cs", PART_A)], &[("src/Foo.2.cs", PART_B)]);
    assert_eq!(calls, vec![edge("src/Foo.1.cs:A@2", "src/Foo.2.cs:B@2")]);
    assert_eq!(extends, vec![edge("src/Foo.2.cs:Foo@2", "src/Foo.1.cs:Foo@2")]);
}
