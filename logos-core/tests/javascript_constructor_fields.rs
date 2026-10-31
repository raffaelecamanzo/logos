//! Constructor-assigned fields are declared, and a callee is never an access
//! (S-542, [FR-EX-19], [FR-EX-08], [FR-QM-11], [CR-174]).
//!
//! A JavaScript class declares most of its fields by assigning them in its
//! constructor (`this.pubClient = …`), which declared no `Field` for a later
//! `this.pubClient` to bind to, and the TypeScript family recorded `this.setState()`
//! — a call — as an access of a field named `setState`. Now the `javascript`
//! plugin declares a Field once per class for each `this.x = …` of a constructor
//! or method (TypeScript's Field set is unchanged), a `this.f()` callee records no
//! access in `typescript`, `tsx` and `javascript`, and `javascript` declares
//! `member_access`, so Cohesion is computed for it.
//!
//! Capture-level checks run one in-memory file through the embedded plugin; the
//! binding and Cohesion checks index a temp tree, like `cpp_bare_access.rs`. The
//! fixtures follow the shapes that dominate NodeBB (a constructor assigning a
//! handful of `this.x`, a method reading them) and preact (`this.setState(…)`).
//!
//! [FR-EX-19]: ../../docs/specs/requirements/FR-EX-19.md
//! [FR-EX-08]: ../../docs/specs/requirements/FR-EX-08.md
//! [FR-QM-11]: ../../docs/specs/requirements/FR-QM-11.md
//! [CR-174]: ../../docs/requests/CR-174-javascript-is-its-own-language-and-its-node-idioms-resolve.md
#![cfg(feature = "lang-typescript")]

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Granularity, Runtime};
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;
#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

fn extract_as(path: &str, source: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_path(path).unwrap_or_else(|| panic!("a plugin claims {path}"));
    let facts = extract(&FileInput::new(path, source), plugin, &SymbolContext::default());
    assert!(facts.warnings.is_empty(), "{path}: {:?}", facts.warnings);
    facts
}

/// `Class.field` of every Field node, from the `Contains` edges: the class that
/// owns each field is what the story declares.
fn fields(facts: &Facts) -> BTreeSet<String> {
    let names: HashMap<&str, &str> = facts
        .nodes
        .iter()
        .map(|n| (n.symbol.as_str(), n.name.as_str()))
        .collect();
    let field_symbols: BTreeSet<&str> = facts
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Field)
        .map(|n| n.symbol.as_str())
        .collect();
    let owned: BTreeSet<String> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Contains && field_symbols.contains(e.target.as_str()))
        .map(|e| format!("{}.{}", names[e.source.as_str()], names[e.target.as_str()]))
        .collect();
    assert_eq!(owned.len(), field_symbols.len(), "every Field has one owner: {owned:?}");
    owned
}

fn set(of: &[&str]) -> BTreeSet<String> {
    of.iter().map(|s| s.to_string()).collect()
}

/// The names of the `Accesses` rows the file records, sorted.
fn access_targets(facts: &Facts) -> Vec<String> {
    let mut targets: Vec<String> = facts
        .refs
        .iter()
        .filter(|r| r.kind == EdgeKind::Accesses)
        .map(|r| r.target.clone())
        .collect();
    targets.sort();
    targets
}

// ── Fields ───────────────────────────────────────────────────────────────────

/// NodeBB's shape: a constructor assigns the instance's state and methods
/// assign more of it. One Field per name per class, whichever writes it.
#[test]
fn a_constructor_or_method_assignment_declares_one_field_per_name_per_class() {
    let facts = extract_as(
        "a.js",
        "class Cache {\n  constructor(opts) {\n    this.pubClient = opts.pub;\n    this.subClient = opts.sub;\n    this.pubClient = null;\n  }\n\
         \x20 connect() {\n    this.subClient = 1;\n    this.ready = true;\n  }\n}\n\
         class Other {\n  constructor() {\n    this.pubClient = 1;\n  }\n}\n",
    );
    assert_eq!(
        fields(&facts),
        set(&["Cache.pubClient", "Cache.subClient", "Cache.ready", "Other.pubClient"]),
        "a rewrite of a name is the same field; a second class keeps its own"
    );
}

/// A class field written in the body is the declaration: a constructor write
/// of the same name adds no second one.
#[test]
fn a_class_field_and_a_constructor_write_of_one_name_are_one_field() {
    let facts = extract_as("a.js", "class A {\n  count = 0;\n  constructor() {\n    this.count = 1;\n    this.other = 2;\n  }\n}\n");
    assert_eq!(fields(&facts), set(&["A.count", "A.other"]));
    assert_eq!(facts.nodes.iter().filter(|n| n.kind == NodeKind::Field).count(), 2);
}

/// What declares nothing: no class to own it, or not a plain write of a plain
/// name through `this`.
#[test]
fn a_write_no_class_owns_or_not_a_plain_this_write_declares_nothing() {
    let facts = extract_as(
        "a.js",
        "function Counter() {\n  this.n = 0;\n}\n\
         const o = {\n  m() {\n    this.y = 1;\n  },\n};\n\
         const E = class {\n  constructor() {\n    this.e = 1;\n  }\n};\n\
         class A {\n  #p = 0;\n  constructor(other) {\n    this.#p = 1;\n    this.t += 1;\n    other.o = 1;\n    this.#q = 1;\n    this.a.b = 1;\n    this['k'] = 1;\n    [this.d] = [1];\n  }\n}\n",
    );
    assert_eq!(fields(&facts), set(&["A.#p"]), "only the class-body private field");
}

/// A `function` expression binds its own `this`, so a callback's `this.t = …`
/// is not the instance's; an arrow function keeps the lexical `this`.
#[test]
fn a_function_expression_callback_declares_nothing_and_an_arrow_declares() {
    let facts = extract_as(
        "a.js",
        "class A {\n  constructor() {\n    setTimeout(function () {\n      this.t = 1;\n    });\n    setTimeout(() => {\n      this.u = 1;\n    });\n    this.v = function () {\n      this.w = 1;\n    };\n  }\n}\n",
    );
    assert_eq!(fields(&facts), set(&["A.u", "A.v"]), "`t` and `w` are written under a function expression's own `this`");
}

/// A function nested in a method is not the method: its `this` is not the
/// class's, and the Python rule it follows drops it.
#[test]
fn a_function_declaration_nested_in_a_method_declares_nothing() {
    let facts = extract_as("a.js", "class A {\n  m() {\n    function inner() {\n      this.x = 1;\n    }\n  }\n}\n");
    assert_eq!(fields(&facts), set(&[]));
}

/// TypeScript's Field set is unchanged: the same source in a `.ts` or `.tsx`
/// file declares no field for a constructor assignment.
#[test]
fn typescript_declares_no_field_for_a_constructor_assignment() {
    let source = "class A {\n  declared = 0;\n  constructor(private p: number) {\n    this.assigned = 1;\n  }\n}\n";
    for path in ["a.ts", "a.tsx"] {
        assert_eq!(fields(&extract_as(path, source)), set(&["A.declared", "A.p"]), "{path}");
    }
}

// ── A callee is never an access ──────────────────────────────────────────────

const CALLEE: &str = "class C {\n  constructor() {\n    this.n = 0;\n  }\n  m() {\n    this.setState({ n: 1 });\n    this.items.push(this.n);\n    this.f?.(1);\n    const g = this.n;\n    return this.cb;\n  }\n}\n";

/// `this.setState()` is a call, not a read of a field named `setState`;
/// `this.n` and `this.cb` are reads, and `this.items.push()` reads `items`.
#[test]
fn a_callee_through_this_records_no_access_in_javascript_typescript_and_tsx() {
    for path in ["a.js", "a.ts", "a.tsx"] {
        let facts = extract_as(path, CALLEE);
        assert_eq!(access_targets(&facts), ["cb", "items", "n", "n"], "{path}: the callees `setState` and `f` are no access");
        let calls: Vec<&str> = facts
            .refs
            .iter()
            .filter(|r| r.kind == EdgeKind::Calls)
            .map(|r| r.target.as_str())
            .collect();
        assert!(calls.contains(&"setState"), "{path}: the call itself is still a call row: {calls:?}");
    }
}

/// A call through a field that holds a function (`this.callback()`) is a call
/// of that name and no read of the field: the method keeps no `Accesses` row for
/// it (FR-EX-19; stated in `metrics.md`). The JavaScript constructor's own write
/// is its one access.
#[test]
fn a_call_through_a_function_valued_field_records_no_access() {
    for (path, source, accesses) in [
        ("a.js", "class A {\n  constructor(cb) {\n    this.callback = cb;\n  }\n  run() {\n    this.callback();\n  }\n}\n", vec!["callback"]),
        ("a.ts", "class A {\n  callback = () => 1;\n  run() {\n    this.callback();\n  }\n}\n", vec![]),
    ] {
        let facts = extract_as(path, source);
        assert_eq!(access_targets(&facts), accesses, "{path}: only the constructor's write is an access");
        assert!(
            facts.refs.iter().any(|r| r.kind == EdgeKind::Calls && r.target == "callback"),
            "{path}: the call is still a call row"
        );
    }
}

/// A generic call applies the selection too (`this.f<T>()`), in the grammars
/// that parse type arguments.
#[test]
fn a_generic_callee_through_this_records_no_access() {
    for path in ["a.ts", "a.tsx"] {
        let facts = extract_as(path, "class C {\n  m() {\n    this.f<number>(1);\n    this.g;\n  }\n}\n");
        assert_eq!(access_targets(&facts), ["g"], "{path}");
    }
}

// ── Binding and Cohesion ─────────────────────────────────────────────────────

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
    let names: HashMap<NodeId, String> = rt
        .submit_read(|store| Ok(store.all_nodes()?.into_iter().map(|n| (n.id, n.name)).collect()))
        .expect("read runs");
    let mut out: Vec<(String, String)> = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs")
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Accesses)
        .map(|e| (names[&e.source].clone(), names[&e.target].clone()))
        .collect();
    out.sort();
    out
}

fn pair(method: &str, field: &str) -> (String, String) {
    (method.to_string(), field.to_string())
}

/// Spot check N23: `this.pubClient` of a NodeBB class reads the Field its
/// constructor assigned, and the call `this.connect()` binds no access.
#[test]
fn a_this_read_binds_to_the_field_the_constructor_assigned() {
    let tmp = tree(&[(
        "src/cache.js",
        "class Cache {\n  constructor(o) {\n    this.pubClient = o.pub;\n  }\n  publish(m) {\n    this.connect();\n    return this.pubClient.send(m);\n  }\n  connect() {}\n}\n",
    )]);
    let engine = index(tmp.path());
    assert_eq!(
        accesses(engine.runtime().unwrap()),
        vec![pair("constructor", "pubClient"), pair("publish", "pubClient")],
        "the constructor's own write is its access, the read is publish's; `this.connect()` is a call"
    );
}

/// Two field groups, each declared by a setter and read by a getter, with no
/// constructor: `a` and `b` share no method, so the class has two components.
fn split_class(name: &str) -> String {
    format!("class {name} {{\n  getA() {{ return this.a; }}\n  setA(v) {{ this.a = v; }}\n  getB() {{ return this.b; }}\n  setB(v) {{ this.b = v; }}\n}}\n")
}

fn scan_classes(path: impl Fn(usize) -> String, n: usize) -> (TempDir, RecordedSnapshot) {
    let files: Vec<(String, String)> = (0..n).map(|i| (path(i), split_class(&format!("Box{i}")))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let tmp = tree(&refs);
    let engine = index(tmp.path());
    let view = engine.hydrate(Granularity::ExcludeContains).expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    let member_access = engine.registry().expect("a started engine holds its registry").member_access_extensions();
    let snapshot = metrics::snapshot(rt, &view, None, metrics::Thresholds::default(), &member_access).expect("snapshot runs");
    (tmp, snapshot)
}

/// `javascript` declares `member_access`: five JS classes score Cohesion, from
/// the links through the `this.x = …`-declared fields (each class has two
/// components, one per field).
#[test]
fn javascript_classes_are_scored_for_cohesion() {
    let (_tmp, s) = scan_classes(|i| format!("src/box{i}.js"), 5);
    assert!(s.metrics.cohesion.is_some(), "five JS classes: Cohesion is scored, not n/a");
    assert!(s.metrics.cohesion_not_applicable.is_none());
    let lcom4: Vec<&str> = s.worst_offenders.cohesion.iter().map(|o| o.detail.as_str()).collect();
    assert_eq!(lcom4, ["LCOM4 2"; 5], "each class is two components, one per declared field");
}

/// A constructor that assigns every field accesses every field, so it links the
/// methods that read any of them — as a TypeScript or Python constructor does.
#[test]
fn a_constructor_assigning_every_field_links_the_methods_that_read_them() {
    let class = "class Box {\n  constructor() {\n    this.a = 0;\n    this.b = 0;\n  }\n  getA() { return this.a; }\n  getB() { return this.b; }\n}\n";
    let files: Vec<(String, String)> = (0..5).map(|i| (format!("src/box{i}.js"), class.replace("Box", &format!("Box{i}")))).collect();
    let refs: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
    let tmp = tree(&refs);
    let engine = index(tmp.path());
    let view = engine.hydrate(Granularity::ExcludeContains).expect("dependency view hydrates");
    let member_access = engine.registry().expect("a started engine holds its registry").member_access_extensions();
    let s = metrics::snapshot(engine.runtime().expect("runtime"), &view, None, metrics::Thresholds::default(), &member_access)
        .expect("snapshot runs");
    assert_eq!(s.metrics.cohesion.map(|c| c.raw), Some(1.0), "five classes of one component each");
    assert!(s.worst_offenders.cohesion.is_empty(), "one component: {:?}", s.worst_offenders.cohesion);
}

#[test]
fn the_javascript_extensions_are_member_access_extensions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let declared = LanguageRegistry::load(tmp.path()).expect("embedded grammars load").member_access_extensions();
    for ext in ["js", "mjs", "cjs", "jsx"] {
        assert!(declared.contains(ext), "{ext} in {declared:?}");
    }
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Fields declared by a synced edit, and removed by the next, leave the graph a
/// cold index of the edited tree builds (NFR-RA-06).
#[test]
fn a_synced_edit_of_constructor_fields_equals_a_fresh_index() {
    let before = "class A {\n  constructor() {\n    this.n = 0;\n  }\n  get() { return this.n; }\n}\n";
    let after = "class A {\n  constructor() {\n    this.n = 0;\n    this.m = 1;\n  }\n  get() { this.run(); return this.n + this.m; }\n  run() {}\n}\n";

    let live = tree(&[("src/a.js", before)]);
    let engine = index(live.path());
    for edited in [after, before] {
        fs::write(live.path().join("src/a.js"), edited).unwrap();
        engine.sync(&["src/a.js".into()]);

        let fresh = tree(&[("src/a.js", edited)]);
        let cold = index(fresh.path());

        let synced = graph_fingerprint(engine.runtime().unwrap());
        assert_eq!(synced, graph_fingerprint(cold.runtime().unwrap()), "sync-to-state must equal index-of-state");
        assert_eq!(synced.contains("A#m."), edited == after, "the field is in the synced graph exactly while the source declares it: {synced}");
    }
}
