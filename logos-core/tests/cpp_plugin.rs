//! C++ language-plugin acceptance suite (S-058, CR-009).
//!
//! Drives the public extraction API and the [`Engine`] façade exactly as the
//! pipeline does, covering the story's acceptance criteria:
//! - the verification preflight (registry loads the grammar at the workspace
//!   ABI and compiles every query) and parity extraction of
//!   nodes/edges/exports/complexity, including from a `.h` header in a mixed
//!   C/C++ fixture ([FR-PL-07](../../docs/specs/requirements/FR-PL-07.md),
//!   [UAT-PL-04](../../docs/specs/requirements/UAT-PL-04.md));
//! - `TEST`/`TEST_F` macro test evidence ([FR-EX-06](../../docs/specs/requirements/FR-EX-06.md));
//! - class-bearing applicability — a class whose method reads its own field
//!   yields the Method→Field `Accesses` edge LCOM4 consumes
//!   ([FR-QM-11](../../docs/specs/requirements/FR-QM-11.md),
//!   [FR-EX-08](../../docs/specs/requirements/FR-EX-08.md));
//! - never-fabricate: an unbindable template/preprocessor call produces no edge
//!   and an honest sub-1.0 coverage signal; a resolvable call binds
//!   ([NFR-RA-05](../../docs/specs/requirements/NFR-RA-05.md)).
//!
//! Gated on `lang-cpp`: a build without the C++ grammar excludes the suite.
#![cfg(feature = "lang-cpp")]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::plugin::LanguageRegistry;
use logos_core::{Engine, Runtime};
use tempfile::TempDir;

// ── Extraction-level helpers (in-memory, no Engine) ──────────────────────────

/// Load the registry with the embedded grammars, no on-disk overrides. Its
/// success *is* the verification preflight: the C++ grammar binds at the
/// workspace ABI and every embedded `.scm` query compiles, or `load` errors.
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load (preflight)")
}

/// Extract one in-memory source string, picking the plugin by `path`'s extension.
fn facts_for(reg: &LanguageRegistry, path: &str, src: &str) -> Facts {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .expect("path has an extension");
    let plugin = reg
        .for_extension(ext)
        .unwrap_or_else(|| panic!("a grammar claims .{ext}"));
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// The first node named `name`, if any.
fn node<'f>(facts: &'f Facts, name: &str) -> Option<&'f logos_core::extract::NodeFact> {
    facts.nodes.iter().find(|n| n.name == name)
}

// ── Engine-level helpers (index a temp workspace) ────────────────────────────

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

/// The id of the unique node with `name` and `kind`.
fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    rt.submit_read(move |store| {
        let rows = store.search(name, Some(kind), 16)?;
        Ok(rows.into_iter().find(|r| r.name == name).map(|r| r.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name}"))
}

// ── FR-PL-07 / UAT-PL-04: preflight + parity extraction ──────────────────────

#[test]
fn cpp_parity_extracts_nodes_exports_and_complexity() {
    let reg = registry();
    // A namespace-scoped class with a method and a field, a free function with a
    // branch, a `static` (internal-linkage) function, and an enum/struct/typedef.
    let facts = facts_for(
        &reg,
        "src/widget.cpp",
        "\
namespace app {

struct Point { int x; int y; };

enum Color { Red, Green };

typedef int Integer;

class Widget {
  int count_;
public:
  int classify(int n) {
    if (n > 0 && n < 10) {
      return 1;
    }
    return 0;
  }
};

int add(int a, int b) { return a + b; }

static int secret() { return 42; }

}
",
    );

    // Nodes: the full code ontology extracts (UAT-PL-04 parity).
    assert_eq!(node(&facts, "app").map(|n| n.kind), Some(NodeKind::Module));
    assert_eq!(
        node(&facts, "Widget").map(|n| n.kind),
        Some(NodeKind::Class)
    );
    assert_eq!(node(&facts, "Point").map(|n| n.kind), Some(NodeKind::Struct));
    assert_eq!(node(&facts, "Color").map(|n| n.kind), Some(NodeKind::Enum));
    assert_eq!(
        node(&facts, "Integer").map(|n| n.kind),
        Some(NodeKind::TypeAlias)
    );
    assert_eq!(
        node(&facts, "classify").map(|n| n.kind),
        Some(NodeKind::Method)
    );
    assert_eq!(
        node(&facts, "count_").map(|n| n.kind),
        Some(NodeKind::Field)
    );
    assert_eq!(node(&facts, "add").map(|n| n.kind), Some(NodeKind::Function));

    // Export convention (cpp-external-linkage): external linkage is exported, a
    // `static` free function is internal linkage and not a root.
    assert!(node(&facts, "add").unwrap().exported, "add() is exported");
    assert!(
        node(&facts, "Widget").unwrap().exported,
        "a namespace-scoped class is exported"
    );
    assert!(
        !node(&facts, "secret").unwrap().exported,
        "a static free function is internal linkage"
    );

    // Complexity: the lift makes the body visible, so a branchy method scores
    // above the flat baseline (`if` + `&&` over the base of 1).
    let classify = node(&facts, "classify").unwrap();
    let cc = classify
        .metrics
        .as_ref()
        .expect("a method carries function metrics")
        .cyclomatic_complexity;
    assert!(cc >= 3, "if + && lifts complexity above 1 (got {cc})");
    let add_cc = node(&facts, "add")
        .unwrap()
        .metrics
        .as_ref()
        .unwrap()
        .cyclomatic_complexity;
    assert_eq!(add_cc, 1, "a flat function is complexity 1 (got {add_cc})");

    // Contains edges: the namespace owns the class, the class owns its members.
    let widget = node(&facts, "Widget").unwrap().symbol.clone();
    let classify_sym = node(&facts, "classify").unwrap().symbol.clone();
    let count_sym = node(&facts, "count_").unwrap().symbol.clone();
    let has_contains = |parent: &logos_core::model::LogosSymbol, child: &logos_core::model::LogosSymbol| {
        facts.edges.iter().any(|e| {
            e.kind == EdgeKind::Contains
                && e.source.as_str() == parent.as_str()
                && e.target.as_str() == child.as_str()
        })
    };
    assert!(has_contains(&widget, &classify_sym), "Widget Contains classify");
    assert!(has_contains(&widget, &count_sym), "Widget Contains count_");

    assert!(facts.warnings.is_empty(), "clean parse: {:?}", facts.warnings);
}

#[test]
fn cpp_owns_dot_h_headers() {
    // The fixed `.h` → C++ ownership rule (FR-PL-07): a `.h` header is parsed by
    // the C++ grammar and its declarations extract, including a class.
    let reg = registry();
    assert!(
        reg.for_extension("h").is_some(),
        "the C++ plugin claims .h"
    );
    let facts = facts_for(
        &reg,
        "include/widget.h",
        "\
namespace app {
class Widget {
public:
  void run();
};
int helper();
}
",
    );
    assert_eq!(
        node(&facts, "Widget").map(|n| n.kind),
        Some(NodeKind::Class),
        "a class in a .h header extracts"
    );
    assert_eq!(
        node(&facts, "run").map(|n| n.kind),
        Some(NodeKind::Method),
        "a method prototype in a .h header extracts"
    );
    assert_eq!(
        node(&facts, "helper").map(|n| n.kind),
        Some(NodeKind::Function)
    );
}

// ── FR-EX-06: GoogleTest macro test evidence ─────────────────────────────────

#[test]
fn cpp_gtest_macros_mark_exactly_the_tests() {
    let reg = registry();
    let facts = facts_for(
        &reg,
        "test/widget_test.cc",
        "\
TEST(WidgetSuite, Adds) { EXPECT_EQ(2, 1 + 1); }

TEST_F(WidgetFixture, Subtracts) { EXPECT_EQ(0, 1 - 1); }

int production(int a) { return a; }
",
    );
    let evidence: BTreeMap<String, bool> = facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name.clone(), n.test_evidence))
        .collect();
    // Both GoogleTest macros parse as functions named for the macro keyword and
    // carry evidence; the production function does not.
    assert_eq!(evidence.get("TEST"), Some(&true), "TEST is evidence: {evidence:?}");
    assert_eq!(
        evidence.get("TEST_F"),
        Some(&true),
        "TEST_F is evidence: {evidence:?}"
    );
    assert_eq!(
        evidence.get("production"),
        Some(&false),
        "a production function carries none: {evidence:?}"
    );
}

// ── FR-QM-11 / FR-EX-08: class applicability — the LCOM4 input edge ───────────

#[test]
fn cpp_class_method_field_access_yields_an_accesses_edge() {
    // A class whose method reads its own field through `this->` produces the
    // Method→Field `Accesses` edge LCOM4 consumes — the class-bearing
    // applicability declaration realized as graph structure (ADR-21).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/counter.cpp",
        "\
class Counter {
  int value_;
public:
  int read() { return this->value_; }
};
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    let read = node_id(rt, "read", NodeKind::Method);
    let value = node_id(rt, "value_", NodeKind::Field);
    let accesses = edges_of(rt, EdgeKind::Accesses);
    assert!(
        accesses.contains(&(read, value)),
        "read() Accesses its own field value_ (the bound LCOM4 input): {accesses:?}"
    );
}

// ── NFR-RA-05: never-fabricate + recorded resolution coverage ────────────────

#[test]
fn cpp_unbindable_calls_yield_no_edge_with_honest_coverage() {
    // One resolvable in-file call binds; one call to an unbound external/templated
    // target stays unresolved — never fabricated — so coverage is honest (<1.0).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/app.cpp",
        "\
int helper() { return 1; }

int run() {
  helper();              // resolvable: same file
  external_unbound();    // unbindable: defined nowhere in the graph
  return 0;
}
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    let run = node_id(rt, "run", NodeKind::Function);
    let helper = node_id(rt, "helper", NodeKind::Function);
    let calls = edges_of(rt, EdgeKind::Calls);
    assert!(
        calls.contains(&(run, helper)),
        "the in-file call binds: {calls:?}"
    );
    // The unbound call fabricated no edge: run() has exactly one outgoing call.
    assert_eq!(
        calls.iter().filter(|(s, _)| *s == run).count(),
        1,
        "exactly one Calls edge from run() — the external call is not invented"
    );
    // Honest coverage signal (recorded in the impl notes per NFR-RA-05).
    assert!(
        result.resolution.refs_unresolved >= 1,
        "the external call persists unresolved"
    );
    assert!(
        result.resolution.coverage < 1.0,
        "coverage is honestly below 1.0 (got {})",
        result.resolution.coverage
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
}
