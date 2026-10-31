//! C++ tests are named by their macro's arguments (S-580, CR-184, FR-EX-06) —
//! exercised through the public extraction API with the SHIPPED `symbols.scm`
//! and `plugin.toml` of the C++ plugin (no query override).
//!
//! What the fixtures pin:
//!
//! - a GoogleTest `TEST(Suite, Name)` — and `TEST_F`, `TEST_P`, `TYPED_TEST` — is
//!   the test node `Suite.Name`, with test evidence;
//! - a Catch2 `TEST_CASE("name", "[tag]")` and `SCENARIO("name")` is the test
//!   node named by its string, with test evidence, spanning from the macro to
//!   the end of its block, and the calls in its block are the test's;
//! - the near misses sit one token from matching and are no named test: a block
//!   headed by a call that is no test macro (a Catch2 `SECTION` nests in its
//!   `TEST_CASE` and is no test of its own), a `TEST_CASE` whose name is no
//!   string (an identifier parses as a function, which stays the one it was), a
//!   constructor shaped like a `TEST` (two type names) whose name is no macro, a
//!   `TEST` whose arguments are no two names (it stays the function it was).

#![cfg(feature = "lang-cpp")]

use std::path::Path;

use logos_core::extract::{extract, Facts, FileInput, NodeFact, SymbolContext};
use logos_core::model::{EdgeKind, NodeKind};
use logos_core::plugin::LanguageRegistry;

fn facts(path: &str, src: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let registry = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).expect("extension");
    let plugin = registry.for_extension(ext).expect("C++ claims the extension");
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// The callable nodes as `(name, test evidence)`, in file order.
fn callables(facts: &Facts) -> Vec<(String, bool)> {
    facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name.clone(), n.test_evidence))
        .collect()
}

fn named<'f>(facts: &'f Facts, name: &str) -> &'f NodeFact {
    facts
        .nodes
        .iter()
        .find(|n| n.name == name)
        .unwrap_or_else(|| panic!("no node named {name:?}: {:?}", callables(facts)))
}

fn pair(name: &str, evidence: bool) -> (String, bool) {
    (name.to_string(), evidence)
}

#[test]
fn a_googletest_test_is_named_suite_dot_name() {
    let f = facts(
        "test/math_test.cc",
        "\
TEST(MathSuite, Adds) { EXPECT_EQ(2, 1 + 1); }
TEST_F(MathFixture, Subtracts) { EXPECT_EQ(0, 1 - 1); }
TEST_P(MathParam, Scales) { EXPECT_EQ(2, 1 * 2); }
TYPED_TEST(MathTyped, Negates) { EXPECT_EQ(-1, -1); }
TYPED_TEST_P(MathTypedP, Halves) { EXPECT_EQ(0, 0); }
int production(int a) { return a; }
",
    );
    assert_eq!(
        callables(&f),
        vec![
            pair("MathSuite.Adds", true),
            pair("MathFixture.Subtracts", true),
            pair("MathParam.Scales", true),
            pair("MathTyped.Negates", true),
            pair("MathTypedP.Halves", true),
            pair("production", false),
        ]
    );
}

#[test]
fn a_catch2_test_is_named_by_its_string() {
    let f = facts(
        "test/math_test.cpp",
        "\
TEST_CASE(\"adds numbers\", \"[math]\") {
  REQUIRE(helper(1) == 2);
}

SCENARIO(\"a stack grows\") {
  helper(2);
}

int helper(int a) { return a + 1; }
",
    );
    assert_eq!(
        callables(&f),
        vec![pair("adds numbers", true), pair("a stack grows", true), pair("helper", false)]
    );
    assert!(f.nodes.iter().all(|n| n.kind != NodeKind::Class), "a test is no type");
}

#[test]
fn a_doctest_decorator_after_the_string_leaves_the_name_alone() {
    let f = facts(
        "test/math_test.cpp",
        "\
TEST_CASE(\"skipped\" * doctest::skip()) { helper(1); }
TEST_CASE(\"suite member\" * doctest::test_suite(\"udt\") * doctest::skip()) { helper(2); }
TEST_CASE(\"\" * doctest::skip()) { helper(3); }
int helper(int a) { return a; }
",
    );
    assert_eq!(
        callables(&f),
        vec![pair("skipped", true), pair("suite member", true), pair("helper", false)]
    );
}

#[test]
fn a_catch2_test_spans_its_macro_and_its_block_and_owns_the_calls_in_it() {
    let f = facts(
        "test/math_test.cpp",
        "\
// header
TEST_CASE(\"adds numbers\", \"[math]\")
{
  REQUIRE(helper(1) == 2);
}

int helper(int a) { return a + 1; }
",
    );
    let test = named(&f, "adds numbers");
    assert_eq!((test.start_line, test.end_line), (2, 5), "from the macro to the closing brace");
    assert!(test.metrics.is_some(), "a test is a callable with metrics");
    let from_test: Vec<&str> = f
        .refs
        .iter()
        .filter(|r| r.source == test.symbol && r.kind == EdgeKind::Calls)
        .map(|r| r.target.as_str())
        .collect();
    assert!(from_test.contains(&"helper"), "the call in the block is the test's: {:?}", f.refs);
}

#[test]
fn a_comment_between_the_macro_and_its_block_leaves_the_test_named() {
    let f = facts(
        "test/math_test.cpp",
        "TEST_CASE(\"first\") /* note */ { helper(1); }\nTEST_CASE(\"second\")\n// note\n{ helper(2); }\nint helper(int a) { return a; }\n",
    );
    assert_eq!(
        callables(&f),
        vec![pair("first", true), pair("second", true), pair("helper", false)]
    );
}

#[test]
fn a_catch2_test_in_a_namespace_and_a_repeated_name_are_each_a_test() {
    let f = facts(
        "test/math_test.cpp",
        "\
namespace math {
TEST_CASE(\"same name\") { REQUIRE(true); }
TEST_CASE(\"same name\") { REQUIRE(true); }
}
",
    );
    let tests: Vec<&NodeFact> = f.nodes.iter().filter(|n| n.name == "same name").collect();
    assert_eq!(tests.len(), 2, "two tests: {:?}", callables(&f));
    assert_ne!(tests[0].symbol, tests[1].symbol, "each its own symbol");
    assert!(tests.iter().all(|t| t.test_evidence));
}

#[test]
fn a_test_name_with_spaces_and_punctuation_is_kept_whole() {
    let f = facts(
        "test/math_test.cpp",
        "TEST_CASE(\"parses `a.b` (and \\\"c\\\")\") { REQUIRE(true); }\n",
    );
    assert_eq!(callables(&f), vec![pair("parses `a.b` (and \\\"c\\\")", true)]);
}

#[test]
fn the_near_misses_are_no_named_test() {
    let f = facts(
        "src/near.cpp",
        "\
SECTION(\"a section\") { helper(1); }
TEST_CASE(NAME) { helper(2); }
TEST_CASE(\"\") { helper(3); }
class Suite { public: Suite(Alpha, Beta) {} };
TEST(Not, Two, Names) { helper(4); }
TEST(Foo x, Bar y) { helper(5); }
TEST_CASE(NAME, \"[tag]\") { helper(6); }
int helper(int a) { return a; }
",
    );
    // No named test: the section, the identifier, the empty string and the
    // two-name constructor are each no test, and no block is named for its text.
    assert_eq!(
        callables(&f),
        vec![
            pair("TEST_CASE", true),
            pair("Suite", false),
            pair("TEST", true),
            pair("TEST", true),
            pair("helper", false)
        ],
        "`TEST_CASE(NAME)` parses as a function and stays the one it was; so do `TEST(Not, Two, Names)` and \
         `TEST(Foo x, Bar y)` (named parameters); `TEST_CASE(NAME, \"[tag]\")` is named for no tag; the \
         constructor keeps its name and carries no evidence"
    );
}

#[test]
fn a_test_macro_of_another_shape_keeps_the_name_it_had() {
    let f = facts("test/odd_test.cc", "TEST(int a, int b) { helper(a); }\nint helper(int a) { return a; }\n");
    assert_eq!(callables(&f), vec![pair("TEST", true), pair("helper", false)]);
}
