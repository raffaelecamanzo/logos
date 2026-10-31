//! C macro-defined tests (S-583, CR-185, FR-EX-06, FR-AN-05): a function a known
//! test macro defines is a named test node whose calls are its own, and C
//! declares a `test_convention`. Driven through the public extraction API
//! exactly as the pipeline does.
//!
//! Every negative here is the near miss of a positive: a production function
//! beside a test, a macro one letter off the list, a header whose name merely
//! ends in a framework's, a framework header the file includes only inside an
//! `#ifdef` block. A false positive would silently exempt production code from
//! the quality gate ([ADR-18]).
//!
//! [ADR-18]: ../../docs/specs/architecture/decisions/ADR-18.md

#![cfg(feature = "lang-c")]

use std::collections::BTreeMap;

use logos_core::extract::{extract, Facts, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::{LanguageRegistry, TestConvention};

fn extract_c(path: &str, src: &str) -> Facts {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let plugin = reg.for_extension("c").expect("c claims .c");
    extract(&FileInput::new(path, src), plugin, &SymbolContext::default())
}

/// `function name → test_evidence` over the extracted callables.
fn evidence(facts: &Facts) -> BTreeMap<String, bool> {
    facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name.clone(), n.test_evidence))
        .collect()
}

#[test]
fn c_declares_the_test_macro_convention() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let reg = LanguageRegistry::load(tmp.path()).expect("embedded grammars load");
    let c = reg.for_extension("c").expect("c claims .c");
    assert_eq!(c.semantics().test_convention, TestConvention::CTestMacros);
}

#[test]
fn a_test_impl_macro_defines_a_test_node_named_by_its_argument() {
    // libuv's shape. The file is outside any `test/` path on purpose: the
    // evidence is the macro, not the path convention.
    let facts = extract_c(
        "src/cases.c",
        "\
#include \"task.h\"

TEST_IMPL(fs_stat_missing_path) {
  uv_fs_t req;
  int r = uv_fs_stat(NULL, &req, \"x\", NULL);
  ASSERT(r == 0);
  return 0;
}

TEST_IMPL(loop_close) {
  return uv_loop_close(uv_default_loop());
}

int production(int a) { return a + 1; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("fs_stat_missing_path"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("loop_close"), Some(&true), "{ev:?}");
    assert_eq!(
        ev.get("production"),
        Some(&false),
        "a function beside the tests carries no evidence"
    );
    assert_eq!(
        ev.values().filter(|&&t| t).count(),
        2,
        "exactly the two macro-defined cases: {ev:?}"
    );
}

#[test]
fn a_test_impl_cases_calls_are_attributed_to_the_test_node() {
    let facts = extract_c(
        "src/cases.c",
        "\
TEST_IMPL(fs_stat_missing_path) {
  int r = uv_fs_stat(NULL, NULL, \"x\", NULL);
  return r;
}
",
    );
    let case = facts
        .nodes
        .iter()
        .find(|n| n.name == "fs_stat_missing_path")
        .expect("the macro-defined case is a node");
    assert_eq!(case.kind, NodeKind::Function);
    let call = facts
        .refs
        .iter()
        .find(|r| r.target == "uv_fs_stat")
        .expect("the call in the body is a reference");
    assert_eq!(
        call.source, case.symbol,
        "the call belongs to the test node, not the file module"
    );
}

#[test]
fn a_macro_off_the_list_is_not_a_function() {
    // Same shape, one letter off `TEST_IMPL`: not a known test macro, so it
    // declares nothing and marks nothing.
    let facts = extract_c(
        "src/cases.c",
        "\
TEST_IMPLS(not_a_case) { return 0; }
HELPER_IMPL(not_a_case_either) { return 0; }
int production(int a) { return a; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(
        ev.keys().map(String::as_str).collect::<Vec<_>>(),
        ["production"],
        "only the real function is a node: {ev:?}"
    );
    assert_eq!(ev.get("production"), Some(&false));
}

#[test]
fn a_greatest_test_macro_return_type_marks_an_ordinary_declarator() {
    let facts = extract_c(
        "src/cases.c",
        "\
TEST adds(void) { return 0; }
int adds_helper(void) { return 1; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("adds"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("adds_helper"), Some(&false), "{ev:?}");
}

#[test]
fn a_unity_test_function_is_evidence_only_with_the_unity_header() {
    let with = extract_c(
        "src/math.c",
        "\
#include \"unity.h\"
#include \"math.h\"

void setUp(void) {}
void test_adds(void) { TEST_ASSERT_EQUAL(2, add(1, 1)); }
int add(int a, int b) { return a + b; }
",
    );
    let ev = evidence(&with);
    assert_eq!(ev.get("test_adds"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("setUp"), Some(&false), "a fixture hook is not a case");
    assert_eq!(ev.get("add"), Some(&false), "a production function is not");

    let without = extract_c(
        "src/math.c",
        "\
#include \"math.h\"

void test_adds(void) {}
",
    );
    assert_eq!(
        evidence(&without).get("test_adds"),
        Some(&false),
        "a `test_` name alone is not evidence"
    );
}

#[test]
fn a_framework_header_is_matched_by_file_name_not_suffix() {
    let facts = extract_c(
        "src/math.c",
        "\
#include \"myunity.h\"
#include <notcmocka.h>

void test_adds(void) {}
",
    );
    assert_eq!(
        evidence(&facts).get("test_adds"),
        Some(&false),
        "myunity.h / notcmocka.h are not the frameworks' headers"
    );

    let nested = extract_c(
        "src/math.c",
        "\
#include \"vendor/unity/unity.h\"

void test_adds(void) {}
",
    );
    assert_eq!(
        evidence(&nested).get("test_adds"),
        Some(&true),
        "a path to unity.h is unity.h"
    );
}

#[test]
fn a_framework_header_counts_wherever_the_file_includes_it_at_top_level() {
    // The evidence is the file's include, not its position: one read per file,
    // shared by every function in it.
    let facts = extract_c(
        "src/math.c",
        "\
int add(int a, int b) { return a + b; }
#include \"unity.h\"

void test_adds(void) {}
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("test_adds"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("add"), Some(&false), "{ev:?}");
}

#[test]
fn an_include_inside_a_conditional_block_is_not_read() {
    // The documented limitation: only top-level includes are read.
    let facts = extract_c(
        "src/math.c",
        "\
#ifdef HAVE_UNITY
#include \"unity.h\"
#endif

void test_adds(void) {}
",
    );
    assert_eq!(evidence(&facts).get("test_adds"), Some(&false));
}

#[test]
fn a_cmocka_registration_names_the_case_whatever_its_name() {
    let facts = extract_c(
        "src/cases.c",
        "\
#include <stdarg.h>
#include <cmocka.h>

static void null_test_success(void **state) { (void)state; }
static void setup_fixture(void **state) { (void)state; }
static void helper(void) {}

int main(void) {
  const struct CMUnitTest tests[] = {
    cmocka_unit_test(null_test_success),
    cmocka_unit_test_setup(other_case, setup_fixture),
  };
  return cmocka_run_group_tests(tests, NULL, NULL);
}
static void other_case(void **state) { (void)state; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("null_test_success"), Some(&true), "{ev:?}");
    assert_eq!(
        ev.get("other_case"),
        Some(&true),
        "the first argument of a setup-form registration is the case"
    );
    assert_eq!(
        ev.get("setup_fixture"),
        Some(&false),
        "a setup hook is registered but is not a case"
    );
    assert_eq!(ev.get("helper"), Some(&false), "{ev:?}");
    assert_eq!(ev.get("main"), Some(&false), "{ev:?}");
}

#[test]
fn a_cmocka_registration_without_the_header_is_not_evidence() {
    let facts = extract_c(
        "src/cases.c",
        "\
static void null_test_success(void **state) { (void)state; }
void register_all(void) { cmocka_unit_test(null_test_success); }
",
    );
    assert_eq!(evidence(&facts).get("null_test_success"), Some(&false));
}

#[test]
fn production_code_is_unchanged() {
    // A redis-shaped production file: ordinary functions, macros and a
    // function-pointer table. Nothing in it is, or becomes, a test.
    let facts = extract_c(
        "src/server.c",
        "\
#include <stdio.h>
#define MAX(a, b) ((a) > (b) ? (a) : (b))

static int compare(const void *a, const void *b) { return MAX(1, 2); }
int main(int argc, char **argv) { return compare(argv, argv); }
void test_mode_enable(void) { puts(\"x\"); }
",
    );
    let ev = evidence(&facts);
    assert!(
        ev.values().all(|&t| !t),
        "no function of a production file is a test: {ev:?}"
    );
    assert_eq!(ev.len(), 3, "{ev:?}");
}

#[test]
fn a_one_argument_test_or_test_case_macro_defines_a_test_node() {
    // The parenthesised-declarator shape for the other two listed macros: each
    // is named by its argument, and a production function beside them is not.
    let facts = extract_c(
        "src/cases.c",
        "\
TEST(adds) { return add(1, 1); }
TEST_CASE(subs) { return sub(2, 1); }
int production(int a) { return a; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("adds"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("subs"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("production"), Some(&false), "{ev:?}");
}

#[test]
fn a_two_argument_test_macro_declares_nothing() {
    // `TEST(group, name) { … }` parses as a statement and a bare block, so no
    // function exists: the documented floor, pinned so a change that fabricates
    // a node or marks the neighbour shows up here.
    let facts = extract_c(
        "src/cases.c",
        "\
TEST(group, name) { helper(); }
int production(int a) { return a; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(
        ev.keys().map(String::as_str).collect::<Vec<_>>(),
        ["production"],
        "no node for the two-argument form: {ev:?}"
    );
    assert_eq!(ev.get("production"), Some(&false), "{ev:?}");
}

#[test]
fn a_return_type_macro_is_matched_by_exact_name() {
    // The ordinary-declarator shape, where only the Rust-side list decides:
    // names that merely start with a listed macro mark nothing.
    let facts = extract_c(
        "src/cases.c",
        "\
TESTING adds(void) { return 0; }
TEST_X subs(void) { return 0; }
TEST_IMPL listed(void) { return 0; }
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("adds"), Some(&false), "{ev:?}");
    assert_eq!(ev.get("subs"), Some(&false), "{ev:?}");
    assert_eq!(ev.get("listed"), Some(&true), "{ev:?}");
}

#[test]
fn a_function_inside_a_conditional_block_is_read_against_the_files_includes() {
    let facts = extract_c(
        "src/math.c",
        "\
#include \"unity.h\"
#ifdef HAVE_ADD
void test_adds(void) {}
#endif
",
    );
    assert_eq!(evidence(&facts).get("test_adds"), Some(&true));
}

#[test]
fn unity_fixture_header_and_cmocka_without_registration() {
    let fixture = extract_c(
        "src/math.c",
        "\
#include \"unity_fixture.h\"
void test_adds(void) {}
",
    );
    assert_eq!(evidence(&fixture).get("test_adds"), Some(&true));

    // A `test_` function of a cmocka file needs no registration to be a case;
    // an unregistered, differently named function is not one.
    let cmocka = extract_c(
        "src/cases.c",
        "\
#include <cmocka.h>
static void test_a(void **state) { (void)state; }
static void other(void **state) { (void)state; }
",
    );
    let ev = evidence(&cmocka);
    assert_eq!(ev.get("test_a"), Some(&true), "{ev:?}");
    assert_eq!(ev.get("other"), Some(&false), "{ev:?}");
}

#[test]
fn only_a_cmocka_registration_names_a_case() {
    // The registration prefix is `cmocka_unit_test`: a run call or a lookalike
    // naming a function registers nothing.
    let facts = extract_c(
        "src/cases.c",
        "\
#include <cmocka.h>
static void ran(void **state) { (void)state; }
static void lookalike(void **state) { (void)state; }
static void near_miss(void **state) { (void)state; }
int main(void) {
  cmocka_run_group_tests(ran, NULL, NULL);
  my_cmocka_unit_test(lookalike);
  cmocka_unit_testing(near_miss);
  return 0;
}
",
    );
    let ev = evidence(&facts);
    assert_eq!(ev.get("ran"), Some(&false), "{ev:?}");
    assert_eq!(ev.get("lookalike"), Some(&false), "{ev:?}");
    assert_eq!(
        ev.get("near_miss"),
        Some(&false),
        "`cmocka_unit_testing` is not a registration: {ev:?}"
    );
}

#[test]
fn platform_variants_of_a_test_are_each_a_node() {
    // libuv defines ten tests twice under `#if`/`#else`; each definition is its
    // own node and both are marked.
    let facts = extract_c(
        "src/cases.c",
        "\
#ifdef _WIN32
TEST_IMPL(tcp_rst) { return 0; }
#else
TEST_IMPL(tcp_rst) { return 1; }
#endif
",
    );
    let marked = facts
        .nodes
        .iter()
        .filter(|n| n.name == "tcp_rst" && n.test_evidence)
        .count();
    assert_eq!(marked, 2, "one marked node per definition");
}
