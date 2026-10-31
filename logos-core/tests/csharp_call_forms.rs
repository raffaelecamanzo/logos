//! C# calls keep their type, generic and conditional calls are captured, and
//! `new T()` records `Instantiates` (S-553, CR-177, FR-RS-28, FR-RS-12,
//! FR-RS-13, NFR-RA-05) — exercised end to end through the public [`Engine`]
//! façade against temp-directory fixtures, with the SHIPPED `references.scm` and
//! `plugin.toml` of the C# plugin (no query override).
//!
//! What the fixture pins, over methods of `App.A` calling `App.Helpers`:
//!
//! - `Helpers.Run()` — a call on a type name — is the Path-form `Helpers::Run`
//!   (no receiver shape: it is a written qualified call) and binds `Helpers.Run`
//!   in another file; the generic `Helpers.Make<int>()` is the same row;
//! - a lowercase receiver (`svc.Go()`, `helpers.Run()`) is a variable, never a
//!   type name: typed by the type its parameter declares, it is the Path row
//!   `Svc::Go` (S-558, `csharp_typed_receivers.rs`); a PascalCase name no type of the
//!   repository carries (`Ghost.Run()`), or one two types carry (`Twin.Run()`),
//!   records its Path row and stays unbound;
//! - a generic call `Pick<int>()` / `this.Pick<int>()` is captured like its
//!   plain form — the bare one a `self` call binding the caller's own `Pick`;
//! - a conditional call `s?.Go()` is a Method-form row of shape `other`;
//! - a receiver the file declares as a value — a property, field, parameter or
//!   local (`Logger.Log()` on an `IAuditSink Logger { get; }`), or a type
//!   parameter (`T.Parse()`) — is no type name, though a repository type is
//!   spelled alike: it records no Path row and binds nothing;
//! - `new Widget()` is an `Instantiates` row bound to the class, a generic
//!   `new List<Widget>()` records the type `List` alone, and a type parameter
//!   (`new T()`), an array, a target-typed `new()` record none.
//!
//! Fixtures are written inline into temp directories, like every sibling
//! binding suite (`receiver_shape_static.rs`): a fixture tree checked into this
//! repository would be indexed into its own graph.

#![cfg(feature = "lang-c-sharp")]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, ReceiverShape, RefForm};
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

/// Every bound edge of `kind` as `(source label, target label)`, sorted.
fn edges(rt: &Runtime, kind: EdgeKind) -> Edges {
    let label = labels(rt);
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| (label[&e.source].clone(), label[&e.target].clone()))
        .collect();
    out.sort();
    out
}

/// Bound edges as `(source label, target label)`.
type Edges = Vec<(String, String)>;

/// One ledger row: `(source label, target, form, receiver, resolved)`.
type Row = (String, String, RefForm, Option<ReceiverShape>, bool);

/// The rows of `kind` sourced in `file`, sorted.
fn rows(rt: &Runtime, file: &str, kind: EdgeKind) -> Vec<Row> {
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
    let prefix = format!("{file}:");
    let rows: Vec<Row> = rt
        .submit_read(|store| store.unresolved_refs())
        .expect("read runs")
        .into_iter()
        .filter(|r| r.kind == kind && r.form != RefForm::Symbol)
        .filter_map(|r| {
            let label = by_symbol.get(&r.source_symbol)?;
            label
                .starts_with(&prefix)
                .then(|| (label.clone(), r.target, r.form, r.receiver, r.resolved))
        })
        .collect();
    sorted(rows)
}

fn row(source: &str, target: &str, form: RefForm, receiver: Option<ReceiverShape>, resolved: bool) -> Row {
    (source.to_string(), target.to_string(), form, receiver, resolved)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by(|a, b| {
        (&a.0, &a.1, a.2.as_i32(), a.3.map(ReceiverShape::as_i32))
            .cmp(&(&b.0, &b.1, b.2.as_i32(), b.3.map(ReceiverShape::as_i32)))
    });
    rows
}

fn edge(from: &str, to: &str) -> (String, String) {
    (from.to_string(), to.to_string())
}

const SELF: Option<ReceiverShape> = Some(ReceiverShape::SelfInstance);
const OTHER: Option<ReceiverShape> = Some(ReceiverShape::Other);

const HELPERS: &str = "\
namespace App {
    public static class Helpers {
        public static int Run() { return 1; }
        public static T Make<T>() { return default(T); }
    }
    public class Widget { }
    public class Svc { public int Go() { return 2; } }
    public class Twin { public static int Run() { return 3; } }
}
";

const TWIN2: &str = "\
namespace App {
    public class Twin { public static int Run() { return 4; } }
}
";

const A: &str = "\
namespace App {
    class A {
        public int P() { return Helpers.Run(); }
        public int Q() { return Helpers.Make<int>(); }
        public int R(Svc s) { return s?.Go() ?? 0; }
        public int S() { return Pick<int>(); }
        public int Pick<T>() { return 0; }
        public object U() { return new Widget(); }
        public int V(Svc svc) { return svc.Go(); }
        public int W() { return Ghost.Run(); }
        public int X() { return this.Pick<int>(); }
        public int Y(Helpers helpers) { return helpers.Run(); }
        public int Z() { return Twin.Run(); }
        public object L() { return new List<Widget>(); }
        public object M() { return new Widget[3]; }
        public Widget N() { Widget w = new(); return w; }
    }
    class Box<T> where T : new() {
        public T Make() { return new T(); }
    }
}
";

#[test]
fn a_call_on_a_type_name_is_a_path_row_that_binds_among_the_types_members() {
    let tmp = tree(&[("src/Helpers.cs", HELPERS), ("src/Twin2.cs", TWIN2), ("src/A.cs", A)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("src/A.cs:{name}@{line}");
    assert_eq!(
        rows(rt, "src/A.cs", EdgeKind::Calls),
        sorted(vec![
            row(&a(3, "P"), "Helpers::Run", RefForm::Path, None, true),
            row(&a(4, "Q"), "Helpers::Make", RefForm::Path, None, true),
            // `s?.Go()`: another object's method, no type proven.
            row(&a(5, "R"), "Go", RefForm::Method, OTHER, false),
            // The bare generic call is a `self` call, like the plain one.
            row(&a(6, "S"), "Pick", RefForm::Method, SELF, true),
            // A parameter's declared type types its call (S-558).
            row(&a(9, "V"), "Svc::Go", RefForm::Path, None, true),
            // A name no type of the repository carries.
            row(&a(10, "W"), "Ghost::Run", RefForm::Path, None, false),
            row(&a(11, "X"), "Pick", RefForm::Method, SELF, true),
            // A lowercase receiver is a variable, never a type name — here
            // one whose declared type is `Helpers` (S-558).
            row(&a(12, "Y"), "Helpers::Run", RefForm::Path, None, true),
            // Two types `App.Twin`: it names neither (NFR-RA-05).
            row(&a(13, "Z"), "Twin::Run", RefForm::Path, None, false),
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            edge(&a(3, "P"), "src/Helpers.cs:Run@3"),
            edge(&a(4, "Q"), "src/Helpers.cs:Make@4"),
            edge(&a(6, "S"), &a(7, "Pick")),
            edge(&a(9, "V"), "src/Helpers.cs:Go@7"),
            edge(&a(11, "X"), &a(7, "Pick")),
            edge(&a(12, "Y"), "src/Helpers.cs:Run@3"),
        ]
    );
}

#[test]
fn new_t_records_instantiates_and_only_for_a_named_type() {
    let tmp = tree(&[("src/Helpers.cs", HELPERS), ("src/Twin2.cs", TWIN2), ("src/A.cs", A)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let a = |line: u32, name: &str| format!("src/A.cs:{name}@{line}");
    assert_eq!(
        rows(rt, "src/A.cs", EdgeKind::Instantiates),
        sorted(vec![
            row(&a(8, "U"), "Widget", RefForm::Path, None, true),
            // A generic construction names its type alone; `List` is no type here.
            row(&a(14, "L"), "List", RefForm::Path, None, false),
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Instantiates),
        vec![edge(&a(8, "U"), "src/Helpers.cs:Widget@6")]
    );
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical (NFR-RA-06); the synced
/// one's `Calls` and `Instantiates` edges are returned.
fn synced_equals_reindexed(
    initial: &[(&str, &str)],
    edits: &[(&str, &str)],
) -> (Edges, Edges) {
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
    (edges(rt_a, EdgeKind::Calls), edges(rt_a, EdgeKind::Instantiates))
}

const CALLER: &str = "\
namespace App {
    class A {
        public int P() { return Helpers.Run(); }
        public object U() { return new Widget(); }
    }
}
";

const TARGETS: &str = "\
namespace App {
    public static class Helpers { public static int Run() { return 1; } }
    public class Widget { }
}
";

#[test]
fn a_synced_edit_to_the_called_type_matches_a_fresh_reindex() {
    let initial = [("src/A.cs", CALLER), ("src/Targets.cs", TARGETS)];
    // The caller moves down a line: its rows are re-extracted and re-bound.
    let moved = format!("// moved\n{CALLER}");
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/A.cs", &moved)]);
    assert_eq!(calls, vec![edge("src/A.cs:P@4", "src/Targets.cs:Run@2")]);
    assert_eq!(news, vec![edge("src/A.cs:U@5", "src/Targets.cs:Widget@3")]);

    // The member is renamed away and the class removed: both edges go, though
    // the caller's file never changes.
    let gone = "namespace App {\n    public static class Helpers { public static int Go() { return 1; } }\n}\n";
    let (calls, news) = synced_equals_reindexed(&initial, &[("src/Targets.cs", gone)]);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(news.is_empty(), "{news:?}");

    // A second `Helpers` in the namespace makes the call name neither.
    let twin = "namespace App {\n    public static class Helpers { public static int Run() { return 2; } }\n}\n";
    let (calls, _) = synced_equals_reindexed(&initial, &[("src/Twin.cs", twin)]);
    assert!(calls.is_empty(), "{calls:?}");

    // And the types arrive after the caller: they bind on sync.
    let (calls, news) = synced_equals_reindexed(&[("src/A.cs", CALLER)], &[("src/Targets.cs", TARGETS)]);
    assert_eq!(calls, vec![edge("src/A.cs:P@3", "src/Targets.cs:Run@2")]);
    assert_eq!(news, vec![edge("src/A.cs:U@4", "src/Targets.cs:Widget@3")]);
}

// ── A name the file declares as a value is no type name ─────────────────────

const LOGGER: &str = "\
namespace App {
    public class Logger { public void Log() { } }
    public class T { public static int Parse(string s) { return 1; } }
    public interface IAuditSink { void Log(); }
}
";

const SHADOWED: &str = "\
namespace App {
    class Svc {
        public IAuditSink Logger { get; set; }
        void X() { Logger.Log(); }
        static int P<T>(string s) { return T.Parse(s); }
        void Y(Logger Logger) { Logger.Log(); }
        void Z() { Logger local = null; Logger.Log(); }
    }
    class Plain {
        void W() { Logger.Log(); }
    }
}
";

#[test]
fn a_name_the_file_declares_as_a_value_never_binds_as_a_type() {
    let tmp = tree(&[("src/Logger.cs", LOGGER), ("src/Svc.cs", SHADOWED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    // Every call in the file is silent, `Plain.W` included: the declaration is
    // read file-wide, so a file that declares a value of the name never reads it
    // as a type — a lost binding, never a guessed one (NFR-RA-05).
    assert_eq!(rows(rt, "src/Svc.cs", EdgeKind::Calls), Vec::<Row>::new());
    assert_eq!(edges(rt, EdgeKind::Calls), Vec::<(String, String)>::new());
}

#[test]
fn a_type_name_without_a_value_of_that_name_still_binds() {
    // The same classes, with nothing in the caller's file declaring `Logger`.
    let plain = "\
namespace App {
    class Plain {
        void W() { Logger.Log(); }
        static int Q() { return T.Parse(\"1\"); }
    }
}
";
    let tmp = tree(&[("src/Logger.cs", LOGGER), ("src/Plain.cs", plain)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            edge("src/Plain.cs:Q@4", "src/Logger.cs:Parse@3"),
            edge("src/Plain.cs:W@3", "src/Logger.cs:Log@2"),
        ]
    );
}

// ── The shapes real C# is written in ────────────────────────────────────────
//
// File-scoped namespaces, a `using` across them, a generic call on a variable
// (`GetRequiredService<T>()` is this shape), and every receiver that is not a
// bare type name.

const UTIL: &str = "\
namespace Lib.Util;
public static class Tools { public static int Do() { return 1; } public static T Make<T>() { return default; } }
public class Gadget { public T Fetch<T>() { return default; } }
";

const SCOPED_CALLER: &str = "\
using Lib.Util;
namespace App.Svc;
class Caller {
    int A() { return Tools.Do(); }
    object B() { return new Gadget { }; }
    int C(Gadget g) { return g.Fetch<int>(); }
    int D(Gadget g) { return g?.Fetch<int>() ?? 0; }
    int E() { return System.Console.Read(); }
    int F() { return Tools.Make<int>().GetHashCode(); }
    string G() { return string.Join(\",\", \"a\"); }
    T H<T>() where T : new() { return new T(); }
}
struct S<T> where T : new() { T M() { return new T(); } }
record R<T>(int N) where T : new() { T M() { return new T(); } }
";

#[test]
fn file_scoped_namespaces_and_the_receivers_that_are_no_type_name() {
    let tmp = tree(&[("src/Lib/Util.cs", UTIL), ("src/App/Caller.cs", SCOPED_CALLER)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    let c = |line: u32, name: &str| format!("src/App/Caller.cs:{name}@{line}");
    assert_eq!(
        rows(rt, "src/App/Caller.cs", EdgeKind::Calls),
        sorted(vec![
            // A type name through a `using` of another file-scoped namespace.
            row(&c(4, "A"), "Tools::Do", RefForm::Path, None, true),
            // `x.M<T>()` on a typed parameter is typed (S-558); `x?.M<T>()`,
            // a conditional call, is not.
            row(&c(6, "C"), "Gadget::Fetch", RefForm::Path, None, true),
            row(&c(7, "D"), "Fetch", RefForm::Method, OTHER, false),
            // A dotted receiver, a predefined type and a call result are no
            // bare type name: each keeps its one Method row.
            row(&c(8, "E"), "Read", RefForm::Method, OTHER, false),
            row(&c(9, "F"), "Tools::Make", RefForm::Path, None, true),
            row(&c(9, "F"), "GetHashCode", RefForm::Method, OTHER, false),
            row(&c(10, "G"), "Join", RefForm::Method, OTHER, false),
        ])
    );
    assert_eq!(
        edges(rt, EdgeKind::Calls),
        vec![
            edge(&c(4, "A"), "src/Lib/Util.cs:Do@2"),
            edge(&c(6, "C"), "src/Lib/Util.cs:Fetch@3"),
            edge(&c(9, "F"), "src/Lib/Util.cs:Make@2"),
        ]
    );
    // `new Gadget { }` binds across namespaces; `new T()` of a method-, struct-
    // and record-level type parameter records nothing.
    assert_eq!(
        rows(rt, "src/App/Caller.cs", EdgeKind::Instantiates),
        vec![row(&c(5, "B"), "Gadget", RefForm::Path, None, true)]
    );
}
