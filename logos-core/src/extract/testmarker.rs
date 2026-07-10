//! Extraction-time test-marker evidence ([FR-EX-06], [CR-001], [ADR-18]).
//!
//! This is the *only* layer that can see a function's in-file test attributes
//! (a Rust `#[test]`, a function nested in a `#[cfg(test)] mod tests`, a Java
//! `@Test`) — signals that path conventions alone miss. [`test_evidence`]
//! computes, while the tree-sitter AST is still in hand (the same moment
//! [`super::shape`] captures `exported`/`fingerprint`), whether one declaration
//! carries language-native test-marker evidence.
//!
//! # Optional per plugin (absence ≠ error)
//!
//! Detection is driven by the descriptor's [`TestConvention`] ([NFR-MA-01]): a
//! language reusing an existing idiom sets one `plugin.toml` line; a language
//! that declares nothing is [`TestConvention::None`] and emits no evidence,
//! still indexing normally ([FR-EX-06]). The path/marker fallbacks of the
//! unified `is_test` annotation ([FR-AN-05], S-028) backstop any plugin gap.
//!
//! # Positive evidence only — never inference
//!
//! Evidence comes from an attribute, a name idiom, a lexically enclosing
//! `it`/`test`/`describe` call, or a filename — **never** from call-graph
//! reachability ([ADR-18]). A production
//! function adjacent to (a sibling of) a test therefore carries no evidence: a
//! false positive would silently exempt production code from the quality gate.
//!
//! [FR-EX-06]: ../../../docs/specs/requirements/FR-EX-06.md
//! [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
//! [CR-001]: ../../../docs/requests/CR-001-test-aware-quality-metrics.md

use tree_sitter::Node;

use crate::model::NodeKind;
use crate::plugin::TestConvention;

/// `true` when this declaration carries language-native test-marker evidence
/// under its grammar's [`TestConvention`] (S-027, [FR-EX-06]).
///
/// Evidence is only ever attached to callable nodes (`Function`/`Method`) — the
/// scope the unified `is_test` annotation ([FR-AN-05]) and the production-scope
/// metrics ([FR-QM-08]) reason about. Deterministic ([NFR-RA-06]): a pure
/// function of the parse tree, the declared name, and the file path.
///
/// `node` is the declaration node (the captured name's parent — a
/// `function_item`, `method_declaration`, `variable_declarator`, …), `source`
/// the file's bytes for reading attribute/identifier text.
///
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn test_evidence(
    node: Node<'_>,
    name: &str,
    kind: NodeKind,
    path: &str,
    convention: TestConvention,
    source: &[u8],
) -> bool {
    // Only functions/methods carry the evidence the production scope reasons
    // about; a type or module is never a test in its own right.
    if !matches!(kind, NodeKind::Function | NodeKind::Method) {
        return false;
    }
    match convention {
        TestConvention::None => false,
        TestConvention::RustAttributes => rust_attributes(node, source),
        TestConvention::PythonTest => python_test(node, name, source),
        TestConvention::JsCallback => js_callback(node, source),
        TestConvention::GoTestFunc => go_test_func(name, path),
        TestConvention::JavaAnnotations => java_annotations(node, source),
        TestConvention::KotlinAnnotations => kotlin_annotations(node, source),
        TestConvention::CSharpAttributes => csharp_attributes(node, source),
        TestConvention::CppTestMacros => cpp_test_macro(name),
        TestConvention::RubyTest => ruby_test(node, name, source),
        TestConvention::PhpUnit => php_unit(node, name, source),
        TestConvention::ScalaTest => scala_test(node, source),
    }
}

// ── Rust ─────────────────────────────────────────────────────────────────────

/// Rust: a `#[test]`-family attribute on the function, or containment in a
/// `#[cfg(test)]` module.
///
/// tree-sitter-rust models an item's outer attributes as `attribute_item`
/// nodes *preceding it as siblings* (not as children), so both checks walk
/// preceding siblings of the relevant item.
fn rust_attributes(node: Node<'_>, source: &[u8]) -> bool {
    // (a) a `#[test]`-family attribute directly on this function.
    if has_preceding_attribute(node, source, |attr_name, _attr, _src| attr_name == "test") {
        return true;
    }
    // (b) an enclosing `#[cfg(test)]` module.
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if n.kind() == "mod_item" && has_preceding_attribute(n, source, is_cfg_test) {
            return true;
        }
        ancestor = n.parent();
    }
    false
}

/// Apply `pred` to each `attribute_item` in the outer-attribute run preceding
/// `item`, passing the attribute's last path segment, the `attribute` node, and
/// the source bytes. Comments are tree-sitter *extras* and appear as named
/// siblings between an attribute and the item it annotates (`#[test]` then a
/// `/// doc` line then the `fn`), so they are skipped — not treated as the end
/// of the run; any other node ends it.
fn has_preceding_attribute(
    item: Node<'_>,
    source: &[u8],
    pred: impl Fn(&str, Node<'_>, &[u8]) -> bool,
) -> bool {
    let mut sib = item.prev_sibling();
    while let Some(n) = sib {
        // A comment between the attribute run and the item does not detach the
        // attributes from it — skip past it without ending the run.
        if n.is_extra() {
            sib = n.prev_sibling();
            continue;
        }
        if n.kind() != "attribute_item" {
            break;
        }
        if let Some(attr) = child_of_kind(n, "attribute") {
            if let Some(last) = rust_attribute_last_segment(attr, source) {
                if pred(last, attr, source) {
                    return true;
                }
            }
        }
        sib = n.prev_sibling();
    }
    false
}

/// The last path segment of a Rust `attribute`'s name: `test` for `#[test]`,
/// `test` for `#[tokio::test]` (a `scoped_identifier`), `cfg` for `#[cfg(…)]`.
fn rust_attribute_last_segment<'s>(attr: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    let mut cursor = attr.walk();
    let head = attr
        .children(&mut cursor)
        .find(|c| matches!(c.kind(), "identifier" | "scoped_identifier"))?;
    last_name_segment(head, source)
}

/// `true` for a `#[cfg(test)]`-family attribute: name `cfg` whose predicate
/// names the `test` configuration key **positively** — `cfg(test)` and
/// `cfg(all(test, …))`/`cfg(any(test, …))` mark a test module, but
/// `cfg(not(test))` is a production-only module and must NOT (a false positive
/// there would silently exempt production code from the gate, [ADR-18]).
///
/// [ADR-18]: ../../../docs/specs/architecture/decisions/ADR-18.md
fn is_cfg_test(attr_name: &str, attr: Node<'_>, source: &[u8]) -> bool {
    if attr_name != "cfg" {
        return false;
    }
    let Some(args) = attr.child_by_field_name("arguments") else {
        return false;
    };
    cfg_predicate_marks_test(args, source, true)
}

/// Evaluate a `cfg` predicate token tree for a *positive* (non-negated) use of
/// the bare `test` key. `polarity` flips under each `not(…)`, so `test` counts
/// only under an even number of negations. `all`/`any` recurse with the current
/// polarity; a `name = "value"` pair (e.g. `feature = "test"`) is not the bare
/// `test` key and is ignored. Iterative-friendly shallow recursion bounded by
/// `cfg` nesting depth (not attacker-controlled AST depth).
fn cfg_predicate_marks_test(token_tree: Node<'_>, source: &[u8], polarity: bool) -> bool {
    let mut cursor = token_tree.walk();
    let named: Vec<Node<'_>> = token_tree
        .children(&mut cursor)
        .filter(Node::is_named)
        .collect();
    let mut i = 0;
    while i < named.len() {
        let node = named[i];
        if node.kind() == "identifier" {
            let text = node.utf8_text(source).unwrap_or("");
            // Function form: `not(…)` / `all(…)` / `any(…)` — an identifier
            // immediately followed by a nested token tree.
            if let Some(inner) = named.get(i + 1).filter(|n| n.kind() == "token_tree") {
                let marks = match text {
                    "not" => cfg_predicate_marks_test(*inner, source, !polarity),
                    "all" | "any" => cfg_predicate_marks_test(*inner, source, polarity),
                    _ => false, // an unknown predicate function — not `test`
                };
                if marks {
                    return true;
                }
                i += 2;
                continue;
            }
            // Bare `test` key: positive evidence only under even negation.
            if polarity && text == "test" {
                return true;
            }
        }
        i += 1;
    }
    false
}

// ── Python ─────────────────────────────────────────────────────────────────

/// Python: a `test_*`-named function (pytest), or a `test`-prefixed method of a
/// `unittest.TestCase` subclass.
fn python_test(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    // pytest / unittest naming: the dominant convention.
    if name.starts_with("test_") {
        return true;
    }
    // unittest: a `test`-prefixed method (incl. camelCase `testFoo`) of a
    // *TestCase subclass. Requiring the `test` prefix keeps `setUp`/helpers out
    // — positive evidence on the test methods only.
    name.starts_with("test") && in_testcase_class(node, source)
}

/// `true` when `node`'s nearest enclosing `class_definition` lists a superclass
/// whose name ends in `TestCase` (`unittest.TestCase`, a bare `TestCase`, or a
/// project base class).
fn in_testcase_class(node: Node<'_>, source: &[u8]) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if n.kind() == "class_definition" {
            if let Some(bases) = n.child_by_field_name("superclasses") {
                let mut cursor = bases.walk();
                return bases.children(&mut cursor).any(|b| {
                    b.is_named() && b.utf8_text(source).is_ok_and(|t| t.ends_with("TestCase"))
                });
            }
            return false; // the enclosing class declares no base
        }
        ancestor = n.parent();
    }
    false
}

// ── TypeScript / JavaScript ──────────────────────────────────────────────────

/// TS/JS: a function lexically enclosed by a call to `it`/`test`/`describe`
/// (the callback body and any helper declared inside it). An anonymous inline
/// arrow is not a captured symbol node, so evidence lands on the named
/// functions and `const`-bound arrows the symbols query does capture.
fn js_callback(node: Node<'_>, source: &[u8]) -> bool {
    let mut ancestor = node.parent();
    while let Some(n) = ancestor {
        if n.kind() == "call_expression" {
            if let Some(callee) = n.child_by_field_name("function") {
                if js_callee_is_test(callee, source) {
                    return true;
                }
            }
        }
        ancestor = n.parent();
    }
    false
}

/// `true` when a call's callee is the `it`/`test`/`describe` family — either a
/// bare identifier or a member access like `it.only` / `describe.skip`.
fn js_callee_is_test(callee: Node<'_>, source: &[u8]) -> bool {
    const MARKERS: [&str; 3] = ["it", "test", "describe"];
    match callee.kind() {
        "identifier" => callee.utf8_text(source).is_ok_and(|t| MARKERS.contains(&t)),
        // `it.only(…)`, `describe.skip(…)`, `test.each(…)(…)`: the object is the
        // marker.
        "member_expression" => callee
            .child_by_field_name("object")
            .and_then(|o| o.utf8_text(source).ok())
            .is_some_and(|t| MARKERS.contains(&t)),
        _ => false,
    }
}

// ── Go ───────────────────────────────────────────────────────────────────────

/// Go: a `Test`/`Benchmark`/`Fuzz` function in a `*_test.go` file. The filename
/// gate is what `go test` itself requires, and it is what keeps a production
/// `TestRunner` type's constructor out of the test scope.
fn go_test_func(name: &str, path: &str) -> bool {
    if !path.ends_with("_test.go") {
        return false;
    }
    ["Test", "Benchmark", "Fuzz"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|rest| {
            // `go test` recognises `Test`/`TestXxx` but not `Testify` —
            // the char after the prefix must not be a lowercase letter.
            rest.chars().next().is_none_or(|c| !c.is_ascii_lowercase())
        })
    })
}

// ── Java ─────────────────────────────────────────────────────────────────────

/// Java: a `@Test`-family annotation on the method, read from its `modifiers`
/// child (`@Test`, `@ParameterizedTest`, `@RepeatedTest`, `@TestFactory`,
/// `@TestTemplate`).
fn java_annotations(node: Node<'_>, source: &[u8]) -> bool {
    let Some(modifiers) = child_of_kind(node, "modifiers") else {
        return false;
    };
    let mut cursor = modifiers.walk();
    // Bound to a local so the cursor's borrow of `modifiers` ends before
    // return (the codebase idiom — see `child_of_kind` / `shape::is_exported`).
    let found = modifiers.children(&mut cursor).any(|m| {
        matches!(m.kind(), "marker_annotation" | "annotation")
            && m.child_by_field_name("name")
                // The simple name of `@Test` and of `@org.junit.jupiter.api.Test`.
                .and_then(|nm| last_name_segment(nm, source))
                .is_some_and(is_junit_test_annotation)
    });
    found
}

/// Kotlin: a `@Test`-family annotation on the function, read from its
/// `modifiers` child. Kotlin models an annotation as `annotation → user_type`
/// (a marker like `@Test`) or `annotation → constructor_invocation → user_type`
/// (one with arguments) with **no `name` field**, so [`java_annotations`]
/// cannot read it — hence this distinct convention (S-055, [CR-009]). The
/// simple type name is the last `identifier` segment of the annotation's
/// `user_type` (`@org.junit.jupiter.api.Test` → `Test`), covering both JUnit
/// and `kotlin.test`.
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
fn kotlin_annotations(node: Node<'_>, source: &[u8]) -> bool {
    let Some(modifiers) = child_of_kind(node, "modifiers") else {
        return false;
    };
    let mut cursor = modifiers.walk();
    // Bound to a local so the cursor's borrow of `modifiers` ends before return
    // (the codebase idiom — see `java_annotations`).
    let found = modifiers
        .children(&mut cursor)
        .filter(|m| m.kind() == "annotation")
        .any(|annotation| {
            kotlin_annotation_simple_name(annotation, source).is_some_and(is_junit_test_annotation)
        });
    found
}

/// The simple (unqualified) type name of a Kotlin `annotation` node: the last
/// `.`-separated `identifier` segment of its `user_type`, found directly for a
/// marker annotation or inside the `constructor_invocation` for one with
/// arguments (`@a.b.Test` → `Test`, `@Test("x")` → `Test`).
fn kotlin_annotation_simple_name<'s>(annotation: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    // marker `@Test` → user_type direct; `@Test(...)` → constructor_invocation
    // → user_type.
    let user_type = child_of_kind(annotation, "user_type").or_else(|| {
        child_of_kind(annotation, "constructor_invocation")
            .and_then(|ci| child_of_kind(ci, "user_type"))
    })?;
    let mut cursor = user_type.walk();
    let last = user_type
        .children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .last()?;
    last.utf8_text(source).ok()
}

// ── C++ ──────────────────────────────────────────────────────────────────────

/// C++ (S-058): a GoogleTest/Catch2 function-like test macro, identified by the
/// declaration's *name*. `TEST(Suite, Name) { … }` and its `TEST_F`/`TEST_P`/
/// `TYPED_TEST` siblings parse as return-type-less `function_definition`s whose
/// `function_declarator` identifier is the macro keyword itself, so the captured
/// symbol name *is* `TEST`/`TEST_F`/… — a name match is the single signal
/// available at extraction time (the captured declarator), keeping the test
/// classification stable ([FR-AN-05]).
///
/// Positive-evidence-only ([ADR-18]): the recognised set is the exact macro
/// keywords, never a prefix, so a production function is not swept in. The Catch2
/// keywords are recognised for completeness, but their string-argument form
/// (`TEST_CASE("name", "[tag]")`) does not parse as a function under
/// `tree-sitter-cpp`, so no node ever reaches this check for them — the measured
/// precision floor, surfaced honestly ([NFR-RA-05]).
fn cpp_test_macro(name: &str) -> bool {
    matches!(
        name,
        "TEST"
            | "TEST_F"
            | "TEST_P"
            | "TYPED_TEST"
            | "TYPED_TEST_P"
            | "TEST_CASE"
            | "SCENARIO"
    )
}

/// The JUnit `@Test`-family simple names.
fn is_junit_test_annotation(name: &str) -> bool {
    matches!(
        name,
        "Test" | "ParameterizedTest" | "RepeatedTest" | "TestFactory" | "TestTemplate"
    )
}

// ── C# ───────────────────────────────────────────────────────────────────────

/// C# (S-057, [CR-009]): a test attribute in one of the method's
/// `attribute_list` children — `[Fact]`/`[Theory]` (xUnit), `[Test]` (NUnit),
/// `[TestMethod]` (MSTest). Unlike Rust's preceding-sibling attributes and
/// Java's `modifiers`-wrapped annotations, tree-sitter-c-sharp hangs each
/// `attribute_list` directly under the `method_declaration`.
///
/// [CR-009]: ../../../docs/requests/CR-009-seven-language-plugins.md
fn csharp_attributes(node: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|list| {
        if list.kind() != "attribute_list" {
            return false;
        }
        let mut inner = list.walk();
        // Bound to a local so the cursor's borrow of `list` ends before the
        // closure returns (the codebase idiom — see `java_annotations`).
        let hit = list.children(&mut inner).any(|attr| {
            attr.kind() == "attribute"
                && attr
                    .child_by_field_name("name")
                    .and_then(|nm| csharp_attribute_name(nm, source))
                    .is_some_and(is_csharp_test_attribute)
        });
        hit
    });
    found
}

/// The simple name of a C# attribute, descending a `qualified_name`/
/// `alias_qualified_name` to its last segment (`Xunit.Fact` → `Fact`) and a
/// `generic_name` to its leading identifier.
fn csharp_attribute_name<'s>(name_node: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    let leaf = match name_node.kind() {
        "qualified_name" | "alias_qualified_name" => name_node.child_by_field_name("name")?,
        _ => name_node,
    };
    if leaf.kind() == "generic_name" {
        let mut cursor = leaf.walk();
        let id = leaf
            .children(&mut cursor)
            .find(|c| c.kind() == "identifier");
        return id.unwrap_or(leaf).utf8_text(source).ok();
    }
    leaf.utf8_text(source).ok()
}

/// The xUnit/NUnit/MSTest test-attribute simple names. C# lets an attribute be
/// written with or without its `Attribute` suffix (`[Fact]` == `[FactAttribute]`),
/// so strip one before matching.
fn is_csharp_test_attribute(name: &str) -> bool {
    let base = name.strip_suffix("Attribute").unwrap_or(name);
    matches!(base, "Fact" | "Theory" | "Test" | "TestMethod")
}

// ── Ruby ─────────────────────────────────────────────────────────────────────

/// The RSpec block-DSL call names whose enclosure marks a function as a test —
/// the example forms (`it`/`specify`/`example`/`scenario`) and the grouping
/// forms (`describe`/`context`/`feature`), with their focus/skip prefixes.
const RSPEC_MARKERS: [&str; 13] = [
    "describe", "context", "feature", "it", "specify", "example", "scenario", "xdescribe",
    "xcontext", "xit", "fdescribe", "fcontext", "fit",
];

/// Ruby: a minitest `test_*`-named method, or a method/example that is — or is
/// lexically enclosed by — an RSpec `it`/`describe`/`context` block call.
///
/// The dual idiom mirrors two existing conventions at once: the `test_*` naming
/// is the [`python_test`] posture, and the block-callback enclosure is the
/// [`js_callback`] posture (a helper declared inside `describe` is test scope).
/// A captured RSpec example *is* such a call (the `it` node itself), so the
/// node is checked before its ancestry.
fn ruby_test(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    // minitest: a `test_*`-named method (the dominant convention). Positive
    // evidence on the name alone, the same posture as Python's `test_*`.
    if name.starts_with("test_") {
        return true;
    }
    // RSpec: the node itself is an example/group call, or one encloses it (a
    // helper `def` inside `describe` is test scope — the JS-callback posture).
    let mut current = Some(node);
    while let Some(n) = current {
        if ruby_call_is_rspec(n, source) {
            return true;
        }
        current = n.parent();
    }
    false
}

/// `true` when `node` is a Ruby `call` whose method name is an RSpec block-DSL
/// marker (`it`/`describe`/…), whether bare (`describe "x" do`) or qualified
/// (`RSpec.describe "x" do`) — the method field is the same identifier either way.
fn ruby_call_is_rspec(node: Node<'_>, source: &[u8]) -> bool {
    if node.kind() != "call" {
        return false;
    }
    node.child_by_field_name("method")
        .and_then(|m| m.utf8_text(source).ok())
        .is_some_and(|m| RSPEC_MARKERS.contains(&m))
}

// ── PHP / PHPUnit ────────────────────────────────────────────────────────────

/// PHP/PHPUnit: a method marked a test by any of the three coexisting idioms
/// (S-060) — a `test`-prefixed name, a PHP 8 `#[Test]` attribute, or a `@test`
/// tag in the preceding docblock. Positive-evidence-only: a helper with none of
/// the three carries no evidence ([ADR-18]).
fn php_unit(node: Node<'_>, name: &str, source: &[u8]) -> bool {
    // (a) PHPUnit naming: a `test`-prefixed method — the dominant idiom and
    // exactly what PHPUnit's own auto-discovery matches.
    if name.starts_with("test") {
        return true;
    }
    // (b) a PHP 8 `#[Test]` attribute (`PHPUnit\Framework\Attributes\Test`).
    if php_has_test_attribute(node, source) {
        return true;
    }
    // (c) a `@test` tag in the method's preceding docblock comment.
    php_has_test_docblock(node, source)
}

/// `true` when the method carries a `#[Test]` attribute. tree-sitter-php nests
/// the method's attributes as `attribute_list > attribute_group > attribute`;
/// the attribute's name is a `name` (`#[Test]`) or a `qualified_name`
/// (`#[PHPUnit\Framework\Attributes\Test]`) whose last segment is read.
fn php_has_test_attribute(node: Node<'_>, source: &[u8]) -> bool {
    let Some(attrs) = node.child_by_field_name("attributes") else {
        return false;
    };
    let mut stack = vec![attrs];
    while let Some(n) = stack.pop() {
        if n.kind() == "attribute" && php_attr_last_segment(n, source) == Some("Test") {
            return true;
        }
        let mut cursor = n.walk();
        let children: Vec<Node<'_>> = n.children(&mut cursor).collect();
        stack.extend(children);
    }
    false
}

/// The last segment of an `attribute`'s name: the `name`'s text for `#[Test]`,
/// the final `name` child of a `qualified_name` for a namespaced attribute.
fn php_attr_last_segment<'s>(attr: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    let mut cursor = attr.walk();
    let children: Vec<Node<'_>> = attr.children(&mut cursor).collect();
    let head = children
        .into_iter()
        .find(|c| matches!(c.kind(), "name" | "qualified_name"))?;
    if head.kind() == "qualified_name" {
        let mut c2 = head.walk();
        let names: Vec<Node<'_>> = head.children(&mut c2).filter(|n| n.kind() == "name").collect();
        return names.last().and_then(|n| n.utf8_text(source).ok());
    }
    head.utf8_text(source).ok()
}

/// `true` when a `comment` directly preceding the method is a docblock carrying
/// a `@test` tag. PHP attaches the docblock as a preceding sibling of the
/// `method_declaration` (attributes live *inside* the declaration), so the run
/// of preceding `comment` siblings is scanned; the first non-comment ends it.
fn php_has_test_docblock(node: Node<'_>, source: &[u8]) -> bool {
    let mut sib = node.prev_sibling();
    while let Some(n) = sib {
        if n.kind() != "comment" {
            break;
        }
        if n.utf8_text(source).is_ok_and(comment_has_test_tag) {
            return true;
        }
        sib = n.prev_sibling();
    }
    false
}

/// `true` when `text` contains a `@test` docblock tag as a whole word — so
/// `@test` matches but PHPUnit's distinct `@testWith` data-provider tag does
/// not (a false positive there would mis-scope a non-test helper).
fn comment_has_test_tag(text: &str) -> bool {
    let mut rest = text;
    while let Some(pos) = rest.find("@test") {
        let after = &rest[pos + "@test".len()..];
        if after.chars().next().is_none_or(|c| !c.is_alphanumeric()) {
            return true;
        }
        rest = &rest[pos + "@test".len()..];
    }
    false
}

// ── Scala ────────────────────────────────────────────────────────────────────

/// The munit / ScalaTest (`FunSuite`/`FunSpec`) marker-call names. A test case
/// in these frameworks is a `test("name") { … }` / `it("name") { … }` *call*,
/// not a declaration — so evidence attaches to the call itself and to any
/// callable lexically enclosed by it.
const SCALA_TEST_MARKERS: [&str; 2] = ["test", "it"];

/// Scala: a node that **is**, or is lexically enclosed by, a `test(…)`/`it(…)`
/// marker call (S-061, [FR-EX-06]). Walking the node itself plus its ancestors
/// covers both shapes: a marker call passed directly, and a `def` that may sit
/// inside a test block.
/// Positive-evidence-only ([ADR-18]): only the `test`/`it` names match, so a
/// production call enclosing a helper is never a false positive.
fn scala_test(node: Node<'_>, source: &[u8]) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "call_expression" && scala_call_head_is_marker(n, source) {
            return true;
        }
        current = n.parent();
    }
    false
}

/// `true` when a `call_expression`'s leading callee identifier is a Scala test
/// marker. The munit/ScalaTest form `test("name") { … }` nests as a curried call
/// — the outer call's `function` is the inner `test("name")` call — so the head
/// identifier is reached by descending `function` (through a nested call or a
/// `field_expression` receiver) to the first `identifier`.
fn scala_call_head_is_marker(call: Node<'_>, source: &[u8]) -> bool {
    let mut head = call.child_by_field_name("function");
    while let Some(node) = head {
        match node.kind() {
            "identifier" => {
                return node
                    .utf8_text(source)
                    .is_ok_and(|t| SCALA_TEST_MARKERS.contains(&t));
            }
            // `test("name")(…)` curried application, or `it.should(…)` receiver:
            // the marker is the head of the nested callee / receiver value.
            "call_expression" | "generic_function" => head = node.child_by_field_name("function"),
            "field_expression" => head = node.child_by_field_name("value"),
            _ => return false,
        }
    }
    false
}

// ── Shared AST helpers ───────────────────────────────────────────────────────

/// The first direct child of `node` whose kind is `kind`.
fn child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).find(|c| c.kind() == kind);
    found
}

/// The last (unqualified) segment of an identifier-or-scoped-identifier node:
/// the node's own text for a bare `identifier`, else the `name` field of a
/// `scoped_identifier` (`tokio::test` → `test`, `org.junit…​.Test` → `Test`).
/// Shared by the Rust attribute and Java annotation name reads.
fn last_name_segment<'s>(node: Node<'_>, source: &'s [u8]) -> Option<&'s str> {
    let leaf = match node.kind() {
        "scoped_identifier" => node.child_by_field_name("name").unwrap_or(node),
        _ => node,
    };
    leaf.utf8_text(source).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::{Language, Parser};

    /// Parse `source` with `language` and probe [`test_evidence`] on the first
    /// node whose kind is `decl_kind` (pre-order) and whose declared name
    /// (`name` field, else the node text) is `target`, under `convention`.
    #[cfg(any(
        feature = "lang-rust",
        feature = "lang-python",
        feature = "lang-typescript",
        feature = "lang-go",
        feature = "lang-java",
        feature = "lang-kotlin",
        feature = "lang-c-sharp",
        feature = "lang-cpp",
        feature = "lang-ruby",
        feature = "lang-php",
        feature = "lang-scala"
    ))]
    fn probe(
        language: &Language,
        path: &str,
        source: &str,
        decl_kind: &str,
        target: &str,
        node_kind: NodeKind,
        convention: TestConvention,
    ) -> bool {
        let mut parser = Parser::new();
        parser.set_language(language).expect("grammar binds");
        let tree = parser.parse(source, None).expect("parses");
        let bytes = source.as_bytes();
        let mut stack = vec![tree.root_node()];
        // Pre-order, left-to-right (push children reversed).
        while let Some(n) = stack.pop() {
            if n.kind() == decl_kind && node_name(n, bytes) == target {
                return test_evidence(n, target, node_kind, path, convention, bytes);
            }
            for i in (0..n.child_count()).rev() {
                if let Some(c) = n.child(i) {
                    stack.push(c);
                }
            }
        }
        panic!("no {decl_kind} named {target:?} in fixture");
    }

    /// The declared name of a declaration node: its `name` field's text, else
    /// the node's own text (matches how extraction reads the captured name).
    #[cfg(any(
        feature = "lang-rust",
        feature = "lang-python",
        feature = "lang-typescript",
        feature = "lang-go",
        feature = "lang-java",
        feature = "lang-kotlin",
        feature = "lang-c-sharp",
        feature = "lang-cpp",
        feature = "lang-ruby",
        feature = "lang-php",
        feature = "lang-scala"
    ))]
    fn node_name(node: Node<'_>, source: &[u8]) -> String {
        node.child_by_field_name("name")
            .or_else(|| {
                // TS `const f = () => …`: the declarator's name field is the id.
                node.child_by_field_name("declarator")
            })
            .and_then(|c| c.utf8_text(source).ok())
            .unwrap_or("")
            .to_string()
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_attribute_and_cfg_test_module_yield_evidence_but_neighbours_do_not() {
        let lang: Language = tree_sitter_rust::LANGUAGE.into();
        let src = "\
#[test]
fn marked() {}

#[tokio::test]
async fn marked_path() {}

fn prod() {}

#[cfg(test)]
mod tests {
    fn helper() {}
}
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "src/lib.rs",
                src,
                "function_item",
                name,
                NodeKind::Function,
                TestConvention::RustAttributes,
            )
        };
        assert!(probe1("marked"), "#[test] function is evidence");
        assert!(probe1("marked_path"), "#[tokio::test] function is evidence");
        assert!(probe1("helper"), "fn inside #[cfg(test)] mod is evidence");
        assert!(
            !probe1("prod"),
            "a production fn adjacent to tests carries no evidence"
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_cfg_test_on_a_bare_function_is_not_test_evidence() {
        // `#[cfg(test)]` on a *function* is conditional compilation, not a test
        // marker — only on a module does it imply its contents are tests.
        let lang: Language = tree_sitter_rust::LANGUAGE.into();
        let src = "#[cfg(test)]\nfn gated() {}\n";
        assert!(!probe(
            &lang,
            "src/lib.rs",
            src,
            "function_item",
            "gated",
            NodeKind::Function,
            TestConvention::RustAttributes,
        ));
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_attribute_is_seen_through_an_intervening_doc_comment() {
        // tree-sitter models comments as `extras` — a `/// doc` line between the
        // `#[test]` attribute and the `fn` is a named sibling. The attribute run
        // must be read through it, not truncated at it (else the ubiquitous
        // `#[test]` + doc-comment pattern is a false negative).
        let lang: Language = tree_sitter_rust::LANGUAGE.into();
        for src in [
            "#[test]\n/// docs\nfn marked() {}\n",
            "#[test]\n// a line comment\nfn marked() {}\n",
        ] {
            assert!(
                probe(
                    &lang,
                    "src/lib.rs",
                    src,
                    "function_item",
                    "marked",
                    NodeKind::Function,
                    TestConvention::RustAttributes,
                ),
                "#[test] must be detected through an intervening comment: {src:?}"
            );
        }
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_cfg_not_test_module_is_production_not_test_evidence() {
        // `#[cfg(not(test))]` is a production-only module: a false positive here
        // would silently exempt production code from the gate (ADR-18).
        let lang: Language = tree_sitter_rust::LANGUAGE.into();
        let src = "#[cfg(not(test))]\nmod prod {\n    fn helper() {}\n}\n";
        assert!(
            !probe(
                &lang,
                "src/lib.rs",
                src,
                "function_item",
                "helper",
                NodeKind::Function,
                TestConvention::RustAttributes,
            ),
            "a fn in a #[cfg(not(test))] module is NOT test evidence"
        );
        // And the positive composite `#[cfg(all(test, …))]` still marks tests.
        let src_all = "#[cfg(all(test, feature = \"x\"))]\nmod tests {\n    fn helper() {}\n}\n";
        assert!(
            probe(
                &lang,
                "src/lib.rs",
                src_all,
                "function_item",
                "helper",
                NodeKind::Function,
                TestConvention::RustAttributes,
            ),
            "a fn in a #[cfg(all(test, …))] module is test evidence"
        );
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_test_naming_and_unittest_methods_yield_evidence() {
        let lang: Language = tree_sitter_python::LANGUAGE.into();
        let src = "\
def test_foo():
    pass

def prod():
    pass

class MyCase(unittest.TestCase):
    def testCamel(self):
        pass
    def setUp(self):
        pass

class Plain:
    def test_method(self):
        pass
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "tests/test_mod.py",
                src,
                "function_definition",
                name,
                NodeKind::Function,
                TestConvention::PythonTest,
            )
        };
        assert!(probe1("test_foo"), "test_* function is evidence");
        assert!(probe1("testCamel"), "testCamel in a TestCase is evidence");
        assert!(
            probe1("test_method"),
            "test_* prefix is evidence even outside a TestCase"
        );
        assert!(!probe1("prod"), "a production function carries none");
        assert!(
            !probe1("setUp"),
            "setUp is not test-prefixed → not evidence (positive-evidence-only)"
        );
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn ts_functions_inside_describe_it_yield_evidence_but_top_level_does_not() {
        let lang: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let src = "\
describe('suite', () => {
  function helper() {}
  const check = () => {};
  it('does x', () => { check(); });
});

function prod() {}
";
        assert!(
            probe(
                &lang,
                "src/a.test.ts",
                src,
                "function_declaration",
                "helper",
                NodeKind::Function,
                TestConvention::JsCallback,
            ),
            "a function declared inside describe() is evidence"
        );
        assert!(
            probe(
                &lang,
                "src/a.test.ts",
                src,
                "variable_declarator",
                "check",
                NodeKind::Function,
                TestConvention::JsCallback,
            ),
            "a const-bound arrow inside describe() is evidence"
        );
        assert!(
            !probe(
                &lang,
                "src/a.test.ts",
                src,
                "function_declaration",
                "prod",
                NodeKind::Function,
                TestConvention::JsCallback,
            ),
            "a top-level production function carries none"
        );
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn ts_member_callee_runners_like_it_only_yield_evidence() {
        // `it.only(…)` / `describe.skip(…)` are member-expression callees whose
        // object is the runner — a function inside one is still test evidence.
        let lang: Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let src = "\
describe.skip('suite', () => {
  function inner() {}
});

it.only('x', () => {
  const probe = () => {};
});
";
        assert!(
            probe(
                &lang,
                "src/a.test.ts",
                src,
                "function_declaration",
                "inner",
                NodeKind::Function,
                TestConvention::JsCallback,
            ),
            "a fn inside describe.skip() is evidence"
        );
        assert!(
            probe(
                &lang,
                "src/a.test.ts",
                src,
                "variable_declarator",
                "probe",
                NodeKind::Function,
                TestConvention::JsCallback,
            ),
            "a const-arrow inside it.only() is evidence"
        );
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn go_test_functions_need_both_the_name_and_the_test_file() {
        let lang: Language = tree_sitter_go::LANGUAGE.into();
        let src = "\
package p

func TestThing(t *testing.T) {}
func BenchmarkThing(b *testing.B) {}
func FuzzThing(f *testing.F) {}
func Testify() {}
func prod() {}
";
        let probe_in = |path: &str, name: &str| {
            probe(
                &lang,
                path,
                src,
                "function_declaration",
                name,
                NodeKind::Function,
                TestConvention::GoTestFunc,
            )
        };
        assert!(probe_in("foo_test.go", "TestThing"), "TestXxx in _test.go");
        assert!(
            probe_in("foo_test.go", "BenchmarkThing"),
            "BenchmarkXxx in _test.go"
        );
        assert!(probe_in("foo_test.go", "FuzzThing"), "FuzzXxx in _test.go");
        assert!(
            !probe_in("foo_test.go", "Testify"),
            "Test followed by a lowercase letter is not a test func"
        );
        assert!(
            !probe_in("foo_test.go", "prod"),
            "a plain func is not a test"
        );
        assert!(
            !probe_in("foo.go", "TestThing"),
            "the same name in a non-_test.go file is NOT a test (no false positive)"
        );
    }

    #[cfg(feature = "lang-java")]
    #[test]
    fn java_test_family_annotations_yield_evidence() {
        let lang: Language = tree_sitter_java::LANGUAGE.into();
        let src = "\
class A {
  @Test
  public void marked() {}

  @ParameterizedTest
  void param() {}

  @Override
  public void notATest() {}

  public void prod() {}
}
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "src/ATest.java",
                src,
                "method_declaration",
                name,
                NodeKind::Method,
                TestConvention::JavaAnnotations,
            )
        };
        assert!(probe1("marked"), "@Test method is evidence");
        assert!(probe1("param"), "@ParameterizedTest method is evidence");
        assert!(
            !probe1("notATest"),
            "@Override is not a @Test-family marker"
        );
        assert!(!probe1("prod"), "an unannotated method carries none");
    }

    #[cfg(feature = "lang-kotlin")]
    #[test]
    fn kotlin_test_family_annotations_yield_evidence() {
        let lang: Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        // A marker `@Test`, an annotation-with-args `@RepeatedTest(2)`, a
        // qualified `@org.junit.jupiter.api.ParameterizedTest`, a non-test
        // annotation, and an unannotated neighbour.
        let src = "\
class A {
  @Test
  fun marked() {}

  @RepeatedTest(2)
  fun repeated() {}

  @org.junit.jupiter.api.ParameterizedTest
  fun param() {}

  @Deprecated(\"x\")
  fun notATest() {}

  fun prod() {}
}
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "src/ATest.kt",
                src,
                "function_declaration",
                name,
                NodeKind::Function,
                TestConvention::KotlinAnnotations,
            )
        };
        assert!(probe1("marked"), "@Test function is evidence");
        assert!(probe1("repeated"), "@RepeatedTest(2) function is evidence");
        assert!(
            probe1("param"),
            "a fully-qualified @ParameterizedTest is evidence (simple-name match)"
        );
        assert!(
            !probe1("notATest"),
            "@Deprecated is not a @Test-family marker"
        );
        assert!(!probe1("prod"), "an unannotated function carries none");
    }

    #[cfg(feature = "lang-c-sharp")]
    #[test]
    fn csharp_attribute_families_yield_evidence_across_xunit_nunit_mstest() {
        let lang: Language = tree_sitter_c_sharp::LANGUAGE.into();
        let src = "\
class WidgetTests {
  [Fact]
  public void Fact_marked() {}

  [Theory]
  public void Theory_marked() {}

  [Test]
  public void Nunit_marked() {}

  [TestMethod]
  public void Mstest_marked() {}

  [Xunit.Fact]
  public void Qualified_marked() {}

  [Obsolete]
  public void NotATest() {}

  public void Prod() {}
}
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "src/WidgetTests.cs",
                src,
                "method_declaration",
                name,
                NodeKind::Method,
                TestConvention::CSharpAttributes,
            )
        };
        assert!(probe1("Fact_marked"), "[Fact] (xUnit) is evidence");
        assert!(probe1("Theory_marked"), "[Theory] (xUnit) is evidence");
        assert!(probe1("Nunit_marked"), "[Test] (NUnit) is evidence");
        assert!(probe1("Mstest_marked"), "[TestMethod] (MSTest) is evidence");
        assert!(
            probe1("Qualified_marked"),
            "a namespace-qualified [Xunit.Fact] is evidence"
        );
        assert!(!probe1("NotATest"), "[Obsolete] is not a test attribute");
        assert!(!probe1("Prod"), "an unattributed method carries none");
    }

    #[cfg(feature = "lang-cpp")]
    #[test]
    fn cpp_gtest_macros_yield_evidence_but_plain_functions_do_not() {
        // `TEST(Suite, Name) { … }` / `TEST_F(...)` parse as return-type-less
        // function_definitions whose declarator identifier *is* the macro
        // keyword — extraction captures the name as `TEST`/`TEST_F`, and the
        // name match is the test-evidence signal. A plain function carries none.
        let lang: Language = tree_sitter_cpp::LANGUAGE.into();
        let src = "\
TEST(MathSuite, Adds) { EXPECT_EQ(2, 1 + 1); }
TEST_F(MathFixture, Subtracts) { EXPECT_EQ(0, 1 - 1); }
int production(int a) { return a; }
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "test/math_test.cc",
                src,
                "function_declarator",
                name,
                NodeKind::Function,
                TestConvention::CppTestMacros,
            )
        };
        assert!(probe1("TEST"), "TEST(...) macro is evidence");
        assert!(probe1("TEST_F"), "TEST_F(...) macro is evidence");
        assert!(
            !probe1("production"),
            "a plain production function carries no evidence"
        );
    }

    #[cfg(feature = "lang-ruby")]
    #[test]
    fn ruby_minitest_and_rspec_idioms_mark_exactly_the_tests() {
        let lang: Language = tree_sitter_ruby::LANGUAGE.into();
        let src = "\
class WidgetTest < Minitest::Test
  def test_marked
  end

  def produce
  end
end

RSpec.describe \"Widget\" do
  def helper_inside
  end
end
";
        let probe1 = |decl: &str, name: &str| {
            probe(
                &lang,
                "test/widget_test.rb",
                src,
                decl,
                name,
                NodeKind::Method,
                TestConvention::RubyTest,
            )
        };
        assert!(probe1("method", "test_marked"), "minitest test_* is evidence");
        assert!(
            probe1("method", "helper_inside"),
            "a method inside an RSpec describe block is evidence (lexical enclosure)"
        );
        assert!(
            !probe1("method", "produce"),
            "a non-test_* method outside any RSpec block carries none"
        );
    }

    #[cfg(feature = "lang-php")]
    #[test]
    fn php_unit_three_idioms_yield_evidence_but_helpers_do_not() {
        let lang: Language = tree_sitter_php::LANGUAGE_PHP.into();
        let src = "<?php
class MyTest extends TestCase {
    public function testAddsItem() { $this->assertTrue(true); }

    #[Test]
    public function addsAnother() { $this->assertEquals(1, 1); }

    /** @test */
    public function viaDocblock() { $this->assertSame(1, 1); }

    /** @testWith [1] */
    public function withProvider() {}

    public function helperNotATest() {}
}
";
        let probe1 = |name: &str| {
            probe(
                &lang,
                "tests/MyTest.php",
                src,
                "method_declaration",
                name,
                NodeKind::Method,
                TestConvention::PhpUnit,
            )
        };
        assert!(probe1("testAddsItem"), "a test*-named method is evidence");
        assert!(probe1("addsAnother"), "a #[Test] method is evidence");
        assert!(probe1("viaDocblock"), "a @test docblock method is evidence");
        assert!(
            !probe1("withProvider"),
            "@testWith is a data-provider tag, not a @test marker (no false positive)"
        );
        assert!(
            !probe1("helperNotATest"),
            "an unmarked helper carries no evidence (positive-evidence-only)"
        );
    }

    #[cfg(feature = "lang-scala")]
    #[test]
    fn scala_defs_inside_test_calls_yield_evidence_but_top_level_does_not() {
        // munit / ScalaTest: a `def` lexically enclosed by a `test(…)`/`it(…)`
        // marker call carries evidence; a production `def` does not.
        let lang: tree_sitter::Language = tree_sitter_scala::LANGUAGE.into();
        let src = "\
class MathSuite extends munit.FunSuite {
  test(\"addition\") {
    def helper(): Int = 1
    assertEquals(helper(), 1)
  }
  def prod(): Int = 2
}
";
        assert!(
            probe(
                &lang,
                "src/MathSuite.scala",
                src,
                "function_definition",
                "helper",
                NodeKind::Function,
                TestConvention::ScalaTest,
            ),
            "a def inside test(\"…\") {{ … }} is evidence"
        );
        assert!(
            !probe(
                &lang,
                "src/MathSuite.scala",
                src,
                "function_definition",
                "prod",
                NodeKind::Function,
                TestConvention::ScalaTest,
            ),
            "a production def beside the tests carries none"
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn none_convention_and_non_callables_never_yield_evidence() {
        let lang: Language = tree_sitter_rust::LANGUAGE.into();
        let src = "#[test]\nfn marked() {}\n";
        // TestConvention::None: detection disabled entirely.
        assert!(!probe(
            &lang,
            "src/lib.rs",
            src,
            "function_item",
            "marked",
            NodeKind::Function,
            TestConvention::None,
        ));
        // A non-callable kind is never evidence even under an active convention.
        assert!(!probe(
            &lang,
            "src/lib.rs",
            src,
            "function_item",
            "marked",
            NodeKind::Struct,
            TestConvention::RustAttributes,
        ));
    }
}
