//! The 5-language test-marker evidence fixture (S-027 / FR-EX-06, UAT-EX-05),
//! driving the public extraction API exactly as the pipeline-orchestrator does.
//!
//! Each language's idiom must yield extraction-time test-marker evidence on
//! *exactly* the marked functions, and a production function declared next to a
//! test must carry none (no proximity false positives, ADR-18). A plugin whose
//! file contains no test idiom still indexes normally — evidence is optional
//! per plugin (absence ≠ error, FR-EX-06, NFR-MA-01).
//!
//! Per-language tests are feature-gated like the grammars themselves, so a
//! `--no-default-features` build that excludes a language excludes its test.
//!
//! Gated on `lang-rust` for the shared harness (the registry + extract calls);
//! each language assertion additionally gates on its own grammar feature.
#![cfg(feature = "lang-rust")]

use std::collections::BTreeMap;

use logos_core::extract::{extract, FileInput, SymbolContext};
use logos_core::model::NodeKind;
use logos_core::plugin::LanguageRegistry;

/// Load the registry with the embedded grammars, no on-disk overrides.
fn registry() -> LanguageRegistry {
    let tmp = tempfile::tempdir().expect("tempdir");
    LanguageRegistry::load(tmp.path()).expect("embedded grammars load")
}

/// Extract `src` at `path` (extension selects the grammar) and return a
/// `name → test_evidence` map over the function/method nodes.
fn function_evidence(path: &str, src: &str) -> BTreeMap<String, bool> {
    let ext = path.rsplit('.').next().expect("path has an extension");
    let reg = registry();
    let plugin = reg
        .for_extension(ext)
        .unwrap_or_else(|| panic!("a grammar claims .{ext}"));
    let facts = extract(
        &FileInput::new(path, src),
        plugin,
        &SymbolContext::default(),
    );
    facts
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Function | NodeKind::Method))
        .map(|n| (n.name.clone(), n.test_evidence))
        .collect()
}

/// Assert `name` is present and is (not) test evidence.
fn assert_evidence(map: &BTreeMap<String, bool>, name: &str, expected: bool) {
    let got = map
        .get(name)
        .unwrap_or_else(|| panic!("function {name:?} extracted; have {map:?}"));
    assert_eq!(
        *got, expected,
        "{name}: expected test_evidence={expected}, got {got} (map: {map:?})"
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "src/widget.rs",
        r#"
pub fn produce() {}

#[test]
fn marked() {}

#[cfg(test)]
mod tests {
    fn helper() {}
}
"#,
    );
    assert_evidence(&map, "marked", true);
    assert_evidence(&map, "helper", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-python")]
#[test]
fn python_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "tests/test_widget.py",
        "\
def test_marked():
    pass

def produce():
    pass

class WidgetTest(unittest.TestCase):
    def test_method(self):
        pass
",
    );
    assert_evidence(&map, "test_marked", true);
    assert_evidence(&map, "test_method", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "src/widget.test.ts",
        "\
describe('widget', () => {
  const check = () => {};
  it('does x', () => {
    function helperInsideIt() {}
  });
});

test('top-level', () => {
  const fromTest = () => {};
});

export function produce() {}
",
    );
    // All three callee idioms (describe / it / test) mark their enclosed fns.
    assert_evidence(&map, "check", true);
    assert_evidence(&map, "helperInsideIt", true);
    assert_evidence(&map, "fromTest", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-go")]
#[test]
fn go_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "widget_test.go",
        "\
package widget

import \"testing\"

func TestMarked(t *testing.T) {}

func BenchmarkMarked(b *testing.B) {}

func FuzzMarked(f *testing.F) {}

func Produce() {}
",
    );
    // All three Go idioms (Test / Benchmark / Fuzz) mark their functions.
    assert_evidence(&map, "TestMarked", true);
    assert_evidence(&map, "BenchmarkMarked", true);
    assert_evidence(&map, "FuzzMarked", true);
    assert_evidence(&map, "Produce", false);
}

#[cfg(feature = "lang-go")]
#[test]
fn go_test_name_in_a_non_test_file_is_not_evidence() {
    // The filename gate is load-bearing: `TestMarked` in widget.go (not
    // *_test.go) is a production function, never a test (no false positive).
    let map = function_evidence("widget.go", "package widget\nfunc TestMarked(t *T) {}\n");
    assert_evidence(&map, "TestMarked", false);
}

#[cfg(feature = "lang-java")]
#[test]
fn java_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "src/WidgetTest.java",
        "\
class WidgetTest {
  @Test
  public void marked() {}

  public void produce() {}
}
",
    );
    assert_evidence(&map, "marked", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_idioms_mark_exactly_the_tests() {
    let map = function_evidence(
        "src/WidgetTest.kt",
        "\
class WidgetTest {
  @Test
  fun marked() {}

  @org.junit.jupiter.api.ParameterizedTest
  fun param() {}

  fun produce() {}
}
",
    );
    // Both a marker `@Test` and a fully-qualified `@ParameterizedTest` (matched
    // on the simple name) carry evidence; the unannotated neighbour carries none
    // (no proximity false positive, ADR-18).
    assert_evidence(&map, "marked", true);
    assert_evidence(&map, "param", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-c-sharp")]
#[test]
fn csharp_idioms_mark_exactly_the_tests() {
    // The three attribute families coexist (xUnit `[Fact]`/`[Theory]`, NUnit
    // `[Test]`, MSTest `[TestMethod]`); a plain method carries none.
    let map = function_evidence(
        "src/WidgetTests.cs",
        "\
class WidgetTests {
    [Fact]
    public void Fact_marked() {}

    [Theory]
    public void Theory_marked() {}

    [Test]
    public void Nunit_marked() {}

    [TestMethod]
    public void Mstest_marked() {}

    public void Produce() {}
}
",
    );
    assert_evidence(&map, "Fact_marked", true);
    assert_evidence(&map, "Theory_marked", true);
    assert_evidence(&map, "Nunit_marked", true);
    assert_evidence(&map, "Mstest_marked", true);
    assert_evidence(&map, "Produce", false);
}

#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_minitest_and_rspec_idioms_mark_exactly_the_tests() {
    // The dual idiom (S-059, CR-009): a minitest `test_*` method and a method
    // lexically enclosed by an RSpec `describe`/`it` block both carry evidence;
    // a non-test method outside any block carries none (positive-evidence-only).
    let map = function_evidence(
        "test/widget_test.rb",
        "\
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
",
    );
    assert_evidence(&map, "test_marked", true);
    assert_evidence(&map, "helper_inside", true);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-php")]
#[test]
fn php_idioms_mark_exactly_the_tests() {
    // All three PHPUnit idioms (test*-name, #[Test] attribute, @test docblock)
    // mark their methods; @testWith (a data-provider tag) and a plain helper do
    // not (S-060, FR-EX-06).
    let map = function_evidence(
        "tests/WidgetTest.php",
        "<?php
class WidgetTest extends TestCase {
    public function testNamed() { $this->assertTrue(true); }

    #[Test]
    public function viaAttribute() { $this->assertTrue(true); }

    /** @test */
    public function viaDocblock() { $this->assertTrue(true); }

    /** @testWith [1] */
    public function withProvider() {}

    public function produce() {}
}
",
    );
    assert_evidence(&map, "testNamed", true);
    assert_evidence(&map, "viaAttribute", true);
    assert_evidence(&map, "viaDocblock", true);
    assert_evidence(&map, "withProvider", false);
    assert_evidence(&map, "produce", false);
}

#[cfg(feature = "lang-python")]
#[test]
fn a_file_without_any_test_idiom_still_indexes_with_no_evidence() {
    // Absence ≠ error (FR-EX-06, NFR-MA-01): a production-only file extracts
    // its functions normally; none carry evidence.
    let map = function_evidence(
        "src/widget.py",
        "\
def alpha():
    pass

def beta():
    pass
",
    );
    assert!(!map.is_empty(), "the file indexed (functions extracted)");
    assert!(
        map.values().all(|&e| !e),
        "no function carries evidence: {map:?}"
    );
}
