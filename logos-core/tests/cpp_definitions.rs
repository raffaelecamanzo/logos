//! C++ declarations complete one another (S-577, [FR-EX-29], [CR-184]):
//! an out-of-line member definition merges into its in-class declaration, a
//! prototype pairs with its definition, and constructors and template members
//! are methods — every symbol a declaration had before unchanged.
//!
//! Driven through the public extraction API and the [`Engine`] façade against
//! the SHIPPED `plugins/cpp/queries/symbols.scm`, never a query override.
//! Fixtures are written inline, like every sibling C++ suite.
//!
//! [FR-EX-29]: ../../docs/specs/requirements/FR-EX-29.md
//! [CR-184]: ../../docs/requests/CR-184-cpp-resolves-includes-out-of-line-members-and-relations.md
#![cfg(feature = "lang-cpp")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::extract::{extract, Facts, FileInput, NodeFact, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind, ParamRange};
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

#[path = "support/graph_fingerprint.rs"]
mod graph_fingerprint;
use graph_fingerprint::graph_fingerprint;

#[path = "support/coordinate.rs"]
mod coordinate;

// ── Extraction-level helpers ─────────────────────────────────────────────────

fn facts_for(path: &str, src: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_path(path).expect("the C++ grammar claims the path");
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// The nodes named `name`, in emission order.
fn named<'f>(facts: &'f Facts, name: &str) -> Vec<&'f NodeFact> {
    facts.nodes.iter().filter(|n| n.name == name).collect()
}

/// The one node named `name`.
fn one<'f>(facts: &'f Facts, name: &str) -> &'f NodeFact {
    match named(facts, name).as_slice() {
        [only] => only,
        many => panic!("expected one node named {name}, got {}: {many:#?}", many.len()),
    }
}

/// A symbol's descriptors after the coordinate (`` src/`a.cpp`/W#m(). ``).
fn descriptors(node: &NodeFact) -> String {
    let symbol = node.symbol.as_str();
    symbol.splitn(5, ' ').nth(4).unwrap_or(symbol).to_string()
}

/// Whether `facts` holds an edge of `kind` from `source` to `target`.
fn has_edge(facts: &Facts, source: &NodeFact, target: &NodeFact, kind: EdgeKind) -> bool {
    facts
        .edges
        .iter()
        .any(|e| e.source == source.symbol && e.target == target.symbol && e.kind == kind)
}

// ── Engine-level helpers ─────────────────────────────────────────────────────

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
    coordinate::pin(tmp.path());
    tmp
}

fn index(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    engine
}

/// Every bound `Calls` edge as `(source name@line, target name@line)`, sorted.
fn call_edges(rt: &Runtime) -> Vec<(String, String)> {
    let (nodes, edges) = rt
        .submit_read(|store| Ok((store.all_nodes()?, store.all_edges()?)))
        .expect("read runs");
    let label = |id| {
        let n = nodes.iter().find(|n| n.id == id).expect("edge endpoint is a node");
        format!("{}@{}", n.name, n.start_line.unwrap_or(0))
    };
    let mut out: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| (label(e.source), label(e.target)))
        .collect();
    out.sort();
    out
}

// ── Out-of-line members merge into their declaration ─────────────────────────

/// A class and its out-of-line definitions in one file — a header holding its
/// inline definitions below the class, ccache's own shape.
const WIDGET_HEADER: &str = "\
class Widget {
public:
  void set(int v);
  int get() const;
private:
  int x_;
};

inline void Widget::set(int v) {
  if (v > 0) {
    x_ = v;
  }
}

inline int Widget::get() const {
  return x_;
}
";

#[test]
fn an_out_of_line_definition_merges_into_its_in_class_declaration() {
    let facts = facts_for("src/widget.hpp", WIDGET_HEADER);
    let widget = one(&facts, "Widget");

    let set = one(&facts, "set");
    assert_eq!(set.kind, NodeKind::Method);
    assert_eq!(descriptors(set), "src/`widget.hpp`/Widget#set().", "the in-class symbol, unchanged");
    let metrics = set.metrics.expect("a method carries metrics");
    assert!(metrics.has_body, "the merged node carries the definition's body");
    assert_eq!(metrics.cyclomatic_complexity, 2, "the body's branch is counted");
    assert_eq!((set.start_line, set.end_line), (9, 13), "the node spans the definition");
    assert!(has_edge(&facts, widget, set, EdgeKind::Contains), "Widget contains set");

    let get = one(&facts, "get");
    assert_eq!(descriptors(get), "src/`widget.hpp`/Widget#get().");
    assert!(get.metrics.expect("metrics").has_body);
}

#[test]
fn an_overloaded_member_merges_each_definition_into_the_prototype_its_parameters_match() {
    let facts = facts_for(
        "src/codec.hpp",
        "\
class Codec {
public:
  void put(int v, int n = 0);
  void put(const char* s);
  void put(char* s);
  void put(void* p);
};

void Codec::put(const char* text) {
  if (text) {}
}

void Codec::put(int value, const int n) {
}

void Codec::put(char* const s) {
}

void Codec::put(void* p) {
}
",
    );
    // A default value and a top-level `const` are not part of the signature —
    // `const int n` is `int n` (ccache's `Failure(const Statistic)` against
    // `Failure(Statistic)`) and `char* const s` is `char* s` (its
    // `FileStream(FILE* const)`) — while `const char*` is not `char*`.
    let puts = named(&facts, "put");
    assert_eq!(puts.len(), 4, "four members, each merged once: {puts:#?}");
    let line_of = |d: &str| puts.iter().find(|n| descriptors(n) == d).expect(d).start_line;
    assert_eq!(line_of("src/`codec.hpp`/Codec#put()."), 13, "put(int, int) carries its own body");
    assert_eq!(line_of("src/`codec.hpp`/Codec#put(1)."), 9, "put(const char*) carries its own body");
    assert_eq!(line_of("src/`codec.hpp`/Codec#put(2)."), 16, "put(char*) carries put(char* const)");
    assert_eq!(line_of("src/`codec.hpp`/Codec#put(3)."), 19);
}

/// A prototype's parameters are often unnamed: `m(int = 0)` and `m(const int)`
/// declare `m(int)` exactly as named ones do, so they merge and pair the same.
#[test]
fn unnamed_parameters_match_by_type_whatever_their_default_or_top_level_const() {
    let facts = facts_for(
        "src/anon.cpp",
        "\
struct A { void m(int = 0); void m(double); void k(const int); void k(char*); };
void A::m(int x) {}
void A::m(double d) {}
void A::k(int x) {}
void A::k(char* s) {}
static void log(const char*, int = 0);
static void log(const char* msg, int level) {}
",
    );
    let lines = |name: &str| -> Vec<(String, u32)> {
        named(&facts, name).into_iter().map(|n| (descriptors(n), n.start_line)).collect()
    };
    let mut ms = lines("m");
    ms.sort();
    assert_eq!(ms, [("src/`anon.cpp`/A#m().".to_string(), 2), ("src/`anon.cpp`/A#m(1).".to_string(), 3)]);
    let mut ks = lines("k");
    ks.sort();
    assert_eq!(ks, [("src/`anon.cpp`/A#k().".to_string(), 4), ("src/`anon.cpp`/A#k(1).".to_string(), 5)]);
    let logs = named(&facts, "log");
    let prototype = logs.iter().find(|n| n.start_line == 6).expect("the prototype stays");
    assert!(prototype.signature, "an unnamed `const char*, int = 0` pairs with its definition");
}

#[test]
fn a_definition_whose_class_another_file_declares_is_a_bodied_method_of_its_own() {
    let facts = facts_for(
        "src/widget.cpp",
        "\
#include \"widget.hpp\"

namespace app {
void helper();

void Widget::set(int v) {
  helper();
}
}
",
    );
    let set = one(&facts, "set");
    assert_eq!(set.kind, NodeKind::Method);
    assert_eq!(descriptors(set), "src/`widget.cpp`/app/Widget#set().", "the class it names, in its own file");
    assert!(set.metrics.expect("metrics").has_body);
    let app = one(&facts, "app");
    assert!(has_edge(&facts, app, set, EdgeKind::Contains), "its lexical scope contains it");
    assert!(
        facts.refs.iter().any(|r| r.source == set.symbol && r.target == "helper"),
        "a call in its body is its own, not the namespace's: {:#?}",
        facts.refs
    );
}

#[test]
fn a_namespace_qualified_definition_merges_into_its_namespace_prototype_as_a_function() {
    let facts = facts_for(
        "src/util.cpp",
        "\
namespace util {
int parse(const char* s);
}

int util::parse(const char* s) {
  return s ? 1 : 0;
}

int util::render(int n) {
  return n;
}
",
    );
    let parse = one(&facts, "parse");
    assert_eq!(parse.kind, NodeKind::Function, "a namespace member is a free function");
    assert_eq!(descriptors(parse), "src/`util.cpp`/util/parse().");
    assert!(parse.metrics.expect("metrics").has_body);
    let render = one(&facts, "render");
    assert_eq!(render.kind, NodeKind::Function, "a namespace member without a prototype is one too");
    assert_eq!(descriptors(render), "src/`util.cpp`/util/render().");
}

/// A namespace reopened in a second block is one scope (C++ lookup): a
/// definition in one block merges into, or pairs with, a declaration in the
/// other, never adding a second node for one function.
#[test]
fn a_definition_in_a_reopened_namespace_completes_the_declaration_in_another_block() {
    let facts = facts_for(
        "src/reopen.cpp",
        "\
namespace ns { struct Cls { void m(); }; }
namespace ns { void Cls::m() { if (1) {} } }
namespace a { int f(); }
namespace a { int g(); }
int a::f() { return 1; }
int a::g() { return 2; }
namespace p { int h(int); }
namespace p { int h(int a) { return a; } }
",
    );
    let m = one(&facts, "m");
    assert_eq!((descriptors(m).as_str(), m.start_line), ("src/`reopen.cpp`/ns/Cls#m().", 2));
    assert!(m.metrics.expect("metrics").has_body);
    let f = one(&facts, "f");
    assert_eq!((descriptors(f).as_str(), f.start_line), ("src/`reopen.cpp`/a/f().", 5));
    let g = one(&facts, "g");
    assert_eq!((descriptors(g).as_str(), g.start_line), ("src/`reopen.cpp`/a/1:g().", 6), "merged into the second block's g");
    let hs = named(&facts, "h");
    let prototype = hs.iter().find(|n| n.start_line == 7).expect("the prototype stays");
    let definition = hs.iter().find(|n| n.start_line == 8).expect("the definition stays");
    assert!(prototype.signature, "the prototype pairs across the blocks");
    assert!(has_edge(&facts, prototype, definition, EdgeKind::References));
}

// ── Every pre-existing symbol is unchanged ───────────────────────────────────

/// Out-of-line definitions placed where a careless numbering would shift a
/// pre-existing declaration: before a same-named free function, around a
/// macro the grammar reads as a nested prototype, and beside a class's own
/// members. Each symbol is the one the declaration had before the definitions
/// were captured.
#[test]
fn out_of_line_definitions_move_no_pre_existing_symbol() {
    let facts = facts_for(
        "src/mixed.cpp",
        "\
class Local {
public:
  void run();
  void stop();
};

void Remote::run() {
  TRY_ASSIGN(auto a, make());
}

void Local::run() {
}

void Local::extra() {
}

void run();
void run() {
}

void stop() {
}
",
    );
    let symbols: Vec<String> = facts.nodes.iter().map(descriptors).collect();
    for expected in [
        "src/`mixed.cpp`/Local#",
        "src/`mixed.cpp`/Local#run().",
        "src/`mixed.cpp`/Local#stop().",
        // The free `run` prototype and definition keep ordinals 0 and 1 — the
        // `Remote::run` and `Local::run` definitions number apart from them.
        "src/`mixed.cpp`/run().",
        "src/`mixed.cpp`/run(1).",
        "src/`mixed.cpp`/stop().",
        // The macro in the out-of-line body keeps its file-scope symbol.
        "src/`mixed.cpp`/TRY_ASSIGN().",
        // The new nodes: one naming the class it is not declared with, one a
        // member of the class beside, numbered after its members.
        "src/`mixed.cpp`/Remote#run().",
        "src/`mixed.cpp`/Local#extra().",
    ] {
        assert!(symbols.iter().any(|s| s == expected), "{expected} in {symbols:#?}");
    }
    assert_eq!(named(&facts, "run").len(), 4, "Local::run merged; Remote::run, the free pair: {symbols:#?}");

    // A definition that names its own namespace precedes a same-named function
    // there: it is numbered after it, never before.
    let facts = facts_for(
        "src/ns.cpp",
        "\
namespace ns {
void ns::step() {
}
void step() {
}
}
",
    );
    let steps: Vec<(String, u32)> = named(&facts, "step").into_iter().map(|n| (descriptors(n), n.start_line)).collect();
    assert_eq!(
        steps,
        [("src/`ns.cpp`/ns/step(1).".to_string(), 2), ("src/`ns.cpp`/ns/step().".to_string(), 4)],
        "the pre-existing step() keeps ordinal 0"
    );
}

/// tl-expected's shape (ccache vendors it): a macro before a constructor whose
/// template parameters error recovery tears reads as `MACRO::expected` across
/// a MISSING `::`. That is no definition: it is not extracted, and the
/// constructors around it keep their ordinals.
#[test]
fn a_qualified_name_error_recovery_invented_is_not_a_definition() {
    let facts = facts_for(
        "tl/expected.hpp",
        "\
namespace tl {
template <class T> class expected {
public:
  expected(int a);
  template <class U, detail::enable_if_t<!(std::is_convertible<U const &, T>::value)> * =
                nullptr>
  explicit TL_CONSTEXPR expected(const expected<U> &rhs)
      : ctor_base(detail::tag{}) {
    this->construct(*rhs);
  }
  expected(double d);
};
}
",
    );
    let ctors: Vec<(String, u32)> = named(&facts, "expected")
        .into_iter()
        .filter(|n| n.kind == NodeKind::Method)
        .map(|n| (descriptors(n), n.start_line))
        .collect();
    assert_eq!(
        ctors,
        [
            ("tl/`expected.hpp`/tl/expected#expected().".to_string(), 4),
            ("tl/`expected.hpp`/tl/expected#expected(1).".to_string(), 11),
        ]
    );
}

// ── Constructors and template members are methods ────────────────────────────

#[test]
fn constructors_and_template_members_are_methods_under_their_unchanged_symbols() {
    let facts = facts_for(
        "src/box.hpp",
        "\
class Box {
public:
  Box(int n);
  explicit Box() {}
  template <typename T> void fill(T value);
  template <typename T> T take() { return T(); }
  friend void swap(Box& a, Box& b);
};

void free_function();
",
    );
    let ctors = named(&facts, "Box");
    let methods: Vec<String> = ctors
        .iter()
        .filter(|n| n.kind == NodeKind::Method)
        .map(|n| descriptors(n))
        .collect();
    assert_eq!(methods, ["src/`box.hpp`/Box#Box().", "src/`box.hpp`/Box#Box(1)."]);
    for name in ["fill", "take"] {
        let member = one(&facts, name);
        assert_eq!(member.kind, NodeKind::Method, "{name} is a template member");
        assert_eq!(descriptors(member), format!("src/`box.hpp`/Box#{name}()."));
    }
    assert_eq!(one(&facts, "swap").kind, NodeKind::Function, "a friend is a free function");
    assert_eq!(one(&facts, "free_function").kind, NodeKind::Function);
}

/// A class body's `#if`/`#ifdef` block holds members too: its constructors are
/// methods in either branch, and a free function under a file-scope `#ifdef`
/// stays a function.
#[test]
fn a_constructor_in_a_preprocessor_block_of_a_class_body_is_a_method() {
    let facts = facts_for(
        "src/dir.hpp",
        "\
struct DirEntry {
#ifndef _WIN32
  DirEntry(int fd);
#else
  DirEntry(void* handle);
#endif
#if HAVE_STAT
  explicit DirEntry(const char* path) {}
#endif
  DirEntry();
};
#ifdef X
void free_function();
#endif
",
    );
    let ctors: Vec<(NodeKind, u32)> = named(&facts, "DirEntry")
        .into_iter()
        .filter(|n| n.kind != NodeKind::Struct)
        .map(|n| (n.kind, n.start_line))
        .collect();
    assert_eq!(
        ctors,
        [(NodeKind::Method, 3), (NodeKind::Method, 5), (NodeKind::Method, 8), (NodeKind::Method, 10)]
    );
    assert_eq!(one(&facts, "free_function").kind, NodeKind::Function);
}

#[test]
fn an_out_of_line_constructor_merges_into_its_in_class_constructor() {
    let facts = facts_for(
        "src/box.hpp",
        "\
class Box {
public:
  Box(int n);
private:
  int n_;
};

Box::Box(int n) : n_(n) {
  if (n < 0) {}
}
",
    );
    assert_eq!(named(&facts, "Box").len(), 2, "the class and its one constructor");
    let ctor = facts
        .nodes
        .iter()
        .find(|n| n.name == "Box" && n.kind == NodeKind::Method)
        .expect("the constructor is a method");
    assert_eq!(descriptors(ctor), "src/`box.hpp`/Box#Box().");
    assert!(ctor.metrics.expect("metrics").has_body);
    assert_eq!(ctor.start_line, 8);
}

// ── Prototypes pair with their definition ────────────────────────────────────

const PAIRED: &str = "\
static int compile(int ctx);
static int helper(int n);

int main_entry() {
  return compile(1);
}

static int helper(int n) {
  return n;
}

static int compile(int ctx) {
  if (ctx) {
    return helper(ctx);
  }
  return 0;
}
";

#[test]
fn a_prototype_pairs_with_its_definition_and_binds_nothing() {
    let facts = facts_for("src/main.cpp", PAIRED);
    let compiles = named(&facts, "compile");
    assert_eq!(compiles.len(), 2, "both nodes stay: {compiles:#?}");
    let (definition, prototype) = (compiles[0], compiles[1]);
    assert_eq!(
        (descriptors(prototype).as_str(), descriptors(definition).as_str()),
        ("src/`main.cpp`/compile().", "src/`main.cpp`/compile(1)."),
        "both symbols unchanged; the definition is emitted first"
    );
    assert!(definition.metrics.expect("metrics").has_body);
    assert!(prototype.signature, "the prototype binds nothing");
    assert_eq!(prototype.takes_self, None, "a C++ prototype records no Rust receiver fact");
    assert!(!definition.signature);
    assert!(
        has_edge(&facts, prototype, definition, EdgeKind::References),
        "the prototype links to its definition: {:#?}",
        facts.edges
    );
}

#[test]
fn a_prototype_of_other_parameters_is_an_overload_not_a_pair() {
    let facts = facts_for(
        "src/over.cpp",
        "\
int scale(double d);
int scale(int n) {
  return n;
}
",
    );
    assert!(named(&facts, "scale").iter().all(|n| !n.signature), "no pair across overloads");
    assert!(!facts.edges.iter().any(|e| e.kind == EdgeKind::References));
}

#[test]
fn a_call_binds_the_definition_and_a_lookup_lands_on_it() {
    let tmp = tree(&[("src/main.cpp", PAIRED)]);
    let engine = index(tmp.path());
    let rt = engine.runtime().unwrap();
    assert!(
        call_edges(rt).contains(&("main_entry@4".to_string(), "compile@12".to_string())),
        "the call binds the bodied node exactly once: {:#?}",
        call_edges(rt)
    );

    let node = engine.node("compile", false);
    assert_eq!(
        node.node.expect("compile resolves").symbol.line,
        Some(12),
        "a bare-name lookup lands on the definition"
    );
    let callees = engine.callees("compile", None);
    let names: Vec<&str> = callees.callees.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["helper"], "callees of the definition, not of the prototype");
}

// ── Sync ≡ reindex ───────────────────────────────────────────────────────────

/// Index `initial`, overwrite `edits` and sync them; then index the post-edit
/// tree from scratch. The two graphs must be identical; the synced one's bound
/// `Calls` edges are returned.
fn synced_equals_reindexed(initial: &[(&str, &str)], edits: &[(&str, &str)]) -> Vec<(String, String)> {
    let tmp_a = tree(initial);
    let engine_a = index(tmp_a.path());
    let root_a = tmp_a.path().canonicalize().expect("canonicalize root");
    let mut changed: Vec<PathBuf> = Vec::new();
    for (rel, text) in edits {
        write(tmp_a.path(), rel, text);
        changed.push(root_a.join(rel));
    }
    engine_a.sync(&changed);

    let mut final_state: std::collections::BTreeMap<&str, &str> = initial.iter().copied().collect();
    final_state.extend(edits.iter().copied());
    let files: Vec<(&str, &str)> = final_state.into_iter().collect();
    let tmp_b = tree(&files);
    let engine_b = index(tmp_b.path());
    assert_eq!(
        graph_fingerprint(engine_a.runtime().unwrap()),
        graph_fingerprint(engine_b.runtime().unwrap()),
        "sync-to-state must equal index-of-state"
    );
    call_edges(engine_a.runtime().unwrap())
}

#[test]
fn adding_a_definition_pairs_and_merges_on_sync_as_on_reindex() {
    const PROTOTYPES_ONLY: &str = "\
class Widget {
public:
  void set(int v);
};

static int compile(int ctx);

int main_entry() {
  return compile(1);
}
";
    let completed = format!(
        "{PROTOTYPES_ONLY}
void Widget::set(int v) {{
  if (v) {{}}
}}

static int compile(int ctx) {{
  return ctx;
}}
"
    );
    let paired = synced_equals_reindexed(&[("src/main.cpp", PROTOTYPES_ONLY)], &[("src/main.cpp", &completed)]);
    assert!(
        paired.contains(&("main_entry@8".to_string(), "compile@16".to_string())),
        "the synced call binds the definition the edit added: {paired:#?}"
    );
    let unpaired = synced_equals_reindexed(&[("src/main.cpp", &completed)], &[("src/main.cpp", PROTOTYPES_ONLY)]);
    assert!(
        unpaired.contains(&("main_entry@8".to_string(), "compile@6".to_string())),
        "with the definition gone the call binds the prototype again: {unpaired:#?}"
    );
}

// ── Review coverage (S-577 task review) ──────────────────────────────────────

/// The descriptors and start lines of the nodes named `name`, sorted.
fn placed(facts: &Facts, name: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = named(facts, name).into_iter().map(|n| (descriptors(n), n.start_line)).collect();
    out.sort();
    out
}

/// The source of the one reference to `target`.
fn caller_of(facts: &Facts, target: &str) -> String {
    let sources: Vec<&str> = facts.refs.iter().filter(|r| r.target == target).map(|r| r.source.as_str()).collect();
    match sources.as_slice() {
        [only] => only.splitn(5, ' ').nth(4).unwrap_or(only).to_string(),
        many => panic!("expected one reference to {target}, got {many:?}"),
    }
}

#[test]
fn a_definition_qualified_two_or_three_scopes_deep_merges() {
    let facts = facts_for(
        "src/deep.cpp",
        "\
namespace n { class W { void f(); }; }
namespace a { namespace b { class W { void g(); }; } }
void n::W::f() { if (1) {} }
void a::b::W::g() { if (1) {} }
",
    );
    assert_eq!(placed(&facts, "f"), [("src/`deep.cpp`/n/W#f().".to_string(), 3)]);
    assert_eq!(placed(&facts, "g"), [("src/`deep.cpp`/a/b/W#g().".to_string(), 4)]);
    assert!(one(&facts, "g").metrics.expect("metrics").has_body);
}

#[test]
fn a_definition_returning_a_pointer_or_a_reference_merges() {
    let facts = facts_for(
        "src/ret.cpp",
        "\
class W { int* p(); int& r(); int** pp(); };
int* W::p() { return nullptr; }
int& W::r() { static int x; return x; }
int** W::pp() { return nullptr; }
",
    );
    assert_eq!(placed(&facts, "p"), [("src/`ret.cpp`/W#p().".to_string(), 2)]);
    assert_eq!(placed(&facts, "r"), [("src/`ret.cpp`/W#r().".to_string(), 3)]);
    assert_eq!(placed(&facts, "pp"), [("src/`ret.cpp`/W#pp().".to_string(), 4)]);
}

/// The merged node is the declaration's — its parameter range, defaults
/// included — carrying the definition's body: its nesting, its shingles and
/// the fingerprint the same body has when written in a class.
#[test]
fn a_merged_node_takes_its_range_from_the_declaration_and_its_shape_from_the_body() {
    let facts = facts_for(
        "src/merged.cpp",
        "\
class M {
public:
  int run(int a, int b = 2);
};
int M::run(int a, int b) {
  for (int i = 0; i < a; ++i) {
    if (i) { b++; }
  }
  return b;
}
class N {
public:
  int run(int a, int b) {
    for (int i = 0; i < a; ++i) {
      if (i) { b++; }
    }
    return b;
  }
};
",
    );
    let runs = named(&facts, "run");
    let merged = runs.iter().find(|n| n.start_line == 5).expect("the merged node spans the definition");
    let inline = runs.iter().find(|n| n.start_line == 13).expect("the in-class twin");
    assert_eq!(merged.params, Some(ParamRange { min: 1, max: Some(2) }), "the declaration's range");
    assert_eq!(merged.max_nesting_depth, inline.max_nesting_depth);
    assert!(merged.max_nesting_depth.is_some_and(|d| d >= 2));
    assert!(!merged.shingles.is_empty(), "the body is shingled");
    assert_eq!(merged.shingles, inline.shingles);
    assert_eq!(merged.fingerprint, inline.fingerprint, "the body's shape, not the prototype's");
}

#[test]
fn a_call_in_a_merged_overload_belongs_to_that_overload() {
    let facts = facts_for(
        "src/calls.cpp",
        "\
class C { void a(int); void a(char*); };
void C::a(char* s) { xcall(); }
void C::a(int n) { ycall(); }
",
    );
    assert_eq!(caller_of(&facts, "xcall"), "src/`calls.cpp`/C#a(1).");
    assert_eq!(caller_of(&facts, "ycall"), "src/`calls.cpp`/C#a().");
    let emitted: Vec<&str> = facts.nodes.iter().map(|n| n.symbol.as_str()).collect();
    assert!(facts.refs.iter().all(|r| emitted.contains(&r.source.as_str())), "every source is a node");
}

/// Platform twins: one prototype, a definition under each branch. The first
/// merges; the second is a member of its own, keeping its body and calls.
#[test]
fn a_second_definition_of_one_prototype_keeps_its_own_node() {
    let facts = facts_for(
        "src/twins.cpp",
        "\
class W { void f(); };
#ifdef _WIN32
void W::f() { win(); }
#else
void W::f() { posix(); }
#endif
",
    );
    assert_eq!(
        placed(&facts, "f"),
        [("src/`twins.cpp`/W#f().".to_string(), 3), ("src/`twins.cpp`/W#f(1).".to_string(), 5)]
    );
    assert_eq!(caller_of(&facts, "win"), "src/`twins.cpp`/W#f().");
    assert_eq!(caller_of(&facts, "posix"), "src/`twins.cpp`/W#f(1).");
}

#[test]
fn unmerged_definitions_of_a_class_are_numbered_densely_after_its_members() {
    let facts = facts_for(
        "src/dense.cpp",
        "\
class O { void k(int); };
void O::k(int) {}
void O::k(double) {}
void O::k(char) {}
",
    );
    assert_eq!(
        placed(&facts, "k"),
        [
            ("src/`dense.cpp`/O#k().".to_string(), 2),
            ("src/`dense.cpp`/O#k(1).".to_string(), 3),
            ("src/`dense.cpp`/O#k(2).".to_string(), 4),
        ]
    );
}

/// A prototype pairs only with a definition of its own scope: never a
/// member definition of the same name, never a function of another namespace.
#[test]
fn a_prototype_never_pairs_outside_its_scope() {
    let facts = facts_for(
        "src/scopes.cpp",
        "\
void run();
void Remote::run() {}
namespace a { void f(); }
namespace b { void f() {} }
class Local {};
void stop();
void Local::stop() {}
",
    );
    assert!(named(&facts, "run").iter().all(|n| !n.signature), "not with Remote::run");
    assert!(named(&facts, "stop").iter().all(|n| !n.signature), "not with a member of a class of this file");
    assert!(named(&facts, "f").iter().all(|n| !n.signature), "not across namespaces");
    assert!(!facts.edges.iter().any(|e| e.kind == EdgeKind::References));
}

/// `&`/`&&` and `const` overloads differ only in the declarator's qualifiers.
#[test]
fn qualifier_overloads_merge_by_their_qualifiers() {
    let facts = facts_for(
        "src/quals.cpp",
        "\
struct R { int g() &; int g() &&; int h(); int h() const; };
int R::g() && { return 2; }
int R::g() & { return 1; }
int R::h() const { return 4; }
int R::h() { return 3; }
",
    );
    assert_eq!(
        placed(&facts, "g"),
        [("src/`quals.cpp`/R#g().".to_string(), 3), ("src/`quals.cpp`/R#g(1).".to_string(), 2)]
    );
    assert_eq!(
        placed(&facts, "h"),
        [("src/`quals.cpp`/R#h().".to_string(), 5), ("src/`quals.cpp`/R#h(1).".to_string(), 4)]
    );
}
