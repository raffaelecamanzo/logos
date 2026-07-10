//! The Scala language plugin (S-061, CR-009): an end-to-end acceptance fixture
//! driving the public extraction + Engine APIs exactly as the pipeline does.
//!
//! Scala is the highest-risk grammar of the language-breadth set, gated on its
//! verification preflight (crate resolves / grammar loads at the workspace ABI /
//! queries compile / fixture corpus parses — CR-009 CRA-01). Loading the
//! registry here *is* the preflight: `LanguageRegistry::load` asserts the ABI of
//! every compiled grammar and compiles every `.scm` query, so any test below
//! that obtains the Scala plugin has already proved the grammar binds at ABI 15
//! and its queries compile against the built grammar.
//!
//! Gated on `lang-scala`, so a `--no-default-features` build that excludes Scala
//! excludes this whole file.
#![cfg(feature = "lang-scala")]

use std::collections::BTreeMap;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::plugin::{ExportConvention, LanguageRegistry, TestConvention};

/// Load the registry with the embedded grammars, no on-disk overrides. The call
/// itself exercises the Scala preflight (ABI assertion + query compilation).
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load (Scala preflight)")
}

/// Extract `src` at `path` through the Scala plugin.
fn extract_scala(path: &str, src: &str) -> Facts {
    let reg = registry();
    let plugin = reg.for_extension("scala").expect("scala claims .scala");
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// A `name → NodeKind` map over the extracted nodes (for membership assertions).
fn kinds_by_name(facts: &Facts) -> BTreeMap<String, NodeKind> {
    facts
        .nodes
        .iter()
        .map(|n| (n.name.clone(), n.kind))
        .collect()
}

// ── Preflight + descriptor listing (FR-PL-07, UAT-PL-04) ─────────────────────

#[test]
fn scala_loads_at_the_workspace_abi_and_lists_with_its_descriptor() {
    let reg = registry();
    // The grammar bound (preflight passed) and claims both extensions.
    assert!(reg.for_extension("scala").is_some(), "scala claims .scala");
    assert!(reg.for_extension("sc").is_some(), "scala claims .sc");
    // It was not skipped (no ABI mismatch).
    assert!(
        reg.skipped().iter().all(|s| s.name != "scala"),
        "scala must not be skipped: {:?}",
        reg.skipped()
    );

    let scala = reg.for_extension("scala").unwrap();
    assert_eq!(scala.name(), "scala");
    assert_eq!(scala.extensions(), ["scala", "sc"]);
    // Symbols + references, but no frameworks (an honest absence for Scala).
    assert!(scala.capabilities().iter().any(|c| c == "symbols"));
    assert!(scala.capabilities().iter().any(|c| c == "references"));
    assert!(
        !scala.capabilities().iter().any(|c| c == "frameworks"),
        "Scala declares no framework detector in this increment"
    );

    let sem = scala.semantics();
    assert_eq!(sem.module_separator, ".");
    assert_eq!(sem.export_convention, ExportConvention::PublicDefault);
    assert_eq!(sem.test_convention, TestConvention::ScalaTest);
}

// ── Parity: nodes / edges / exports / complexity (FR-EX-06, UAT-PL-04) ───────

const PARITY_SRC: &str = "\
package com.example

class Calculator(precision: Int) {
  private val secret = 42
  val label = \"calc\"

  def add(a: Int, b: Int): Int = a + b

  def classify(n: Int): String =
    if (n < 0) \"neg\"
    else if (n == 0) \"zero\"
    else \"pos\"

  private def helper(): Int = secret
}

object Calculator {
  def make(): Calculator = new Calculator(2)
}

trait Shape {
  def area: Double
}
";

#[test]
fn parity_extracts_nodes_with_exports_and_complexity() {
    let facts = extract_scala("src/main/scala/Calculator.scala", PARITY_SRC);
    assert!(!facts.partial, "the parity fixture parses cleanly: {facts:?}");
    let kinds = kinds_by_name(&facts);

    // Types: class, companion object, trait.
    assert_eq!(kinds.get("Calculator"), Some(&NodeKind::Class));
    assert_eq!(kinds.get("Shape"), Some(&NodeKind::Trait));
    // Methods and a member field.
    assert_eq!(kinds.get("add"), Some(&NodeKind::Method));
    assert_eq!(kinds.get("classify"), Some(&NodeKind::Method));
    assert_eq!(kinds.get("make"), Some(&NodeKind::Method));
    assert_eq!(kinds.get("label"), Some(&NodeKind::Field));
    assert_eq!(kinds.get("secret"), Some(&NodeKind::Field));

    // Exports: public-by-default; `private` members are non-roots.
    let exported: BTreeMap<&str, bool> =
        facts.nodes.iter().map(|n| (n.name.as_str(), n.exported)).collect();
    assert_eq!(exported.get("add"), Some(&true), "a public def is exported");
    assert_eq!(exported.get("label"), Some(&true), "a public val is exported");
    assert_eq!(
        exported.get("helper"),
        Some(&false),
        "a private def is not a root"
    );
    assert_eq!(
        exported.get("secret"),
        Some(&false),
        "a private val is not a root"
    );

    // Complexity: `classify` has nested if/else-if branches → cc > 1; `add` is
    // straight-line → cc == 1. Metrics are present on callables only.
    let cc = |name: &str| {
        facts
            .nodes
            .iter()
            .find(|n| n.name == name)
            .and_then(|n| n.metrics.as_ref())
            .map(|m| m.cyclomatic_complexity)
    };
    assert_eq!(cc("add"), Some(1), "straight-line def is cc 1");
    assert!(
        cc("classify").is_some_and(|c| c >= 3),
        "two `if`/`else if` decision points raise cc, got {:?}",
        cc("classify")
    );

    // Edges: the class lexically Contains its methods (a `Contains` edge whose
    // target is the `add` method symbol exists).
    let add_symbol = facts
        .nodes
        .iter()
        .find(|n| n.name == "add")
        .map(|n| n.symbol.clone())
        .expect("add extracted");
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::Contains && e.target == add_symbol),
        "the class Contains its `add` method"
    );
}

#[test]
fn sc_script_extension_is_claimed() {
    // `.sc` worksheet/script files bind to the same grammar.
    let reg = registry();
    let plugin = reg.for_extension("sc").expect("scala claims .sc");
    let facts = extract(
        &FileInput::new("build.sc", "def task(): Int = 1\n"),
        plugin,
        &SymbolContext::default(),
    );
    assert!(
        facts.nodes.iter().any(|n| n.name == "task"),
        "a top-level def in a .sc file extracts"
    );
}

// ── Test-marker evidence (FR-EX-06, munit/ScalaTest) ─────────────────────────

#[test]
fn munit_scalatest_marker_calls_carry_evidence_on_enclosed_defs() {
    // A `def` lexically enclosed by a `test(…)`/`it(…)` marker call carries
    // evidence; a production `def` beside the tests does not.
    let facts = extract_scala(
        "src/test/scala/MathSuite.scala",
        "\
class MathSuite extends munit.FunSuite {
  test(\"addition\") {
    def inner(): Int = 1 + 1
    assertEquals(inner(), 2)
  }
  it(\"subtraction\") {
    def diff(): Int = 3 - 1
    assert(diff() == 2)
  }
  def prod(): Int = 0
}
",
    );
    let evidence: BTreeMap<String, bool> = facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name.clone(), n.test_evidence))
        .collect();
    assert_eq!(
        evidence.get("inner"),
        Some(&true),
        "a def inside test(\"…\") is evidence"
    );
    assert_eq!(
        evidence.get("diff"),
        Some(&true),
        "a def inside it(\"…\") is evidence"
    );
    assert_eq!(
        evidence.get("prod"),
        Some(&false),
        "a production def carries none (no proximity false positive)"
    );
}
