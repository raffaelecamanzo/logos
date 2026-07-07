//! Test-quality smell detection through the public Engine façade
//! ([FR-CV-08], [CR-007], [UAT-CV-04]).
//!
//! Each per-language fixture seeds exactly four tests — one assertion-free, one
//! empty-body, one sleeping, and one healthy — and asserts the advisory
//! appendix on `test_gaps` flags exactly the three smelly ones (with the right
//! kind) and never the healthy one. Plus: shadowing a smell query on disk
//! changes detection with no rebuild ([FR-PL-04]); the `gate` is byte-identical
//! before and after smells are reported and nothing is written to the canonical
//! store ([BR-28]).

use std::fs;
use std::path::Path;

use logos_core::models::quality::{TestSmell, TestSmellKind};
use logos_core::Engine;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The `(name, kind)` pairs of the smell findings, sorted — the comparable
/// shape for an exact-match assertion.
fn smell_pairs(findings: &[TestSmell]) -> Vec<(String, TestSmellKind)> {
    let mut pairs: Vec<(String, TestSmellKind)> =
        findings.iter().map(|s| (s.name.clone(), s.kind)).collect();
    pairs.sort();
    pairs
}

/// Index `tmp`, run `test_gaps`, and return the sorted smell `(name, kind)`
/// pairs. The advisory label is always present.
fn smells_of(tmp: &TempDir) -> Vec<(String, TestSmellKind)> {
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.test_gaps(None, true).expect("test_gaps runs");
    assert!(
        report.smells.label.contains("advisory"),
        "the appendix carries the advisory label: {}",
        report.smells.label
    );
    assert!(
        report.smells.not_analyzed.is_empty(),
        "every curated language ships the smell query: {:?}",
        report.smells.not_analyzed
    );
    smell_pairs(&report.smells.findings)
}

#[cfg(feature = "lang-rust")]
#[test]
fn rust_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
#[test]
fn healthy() {
    assert_eq!(1 + 1, 2);
}

#[test]
fn assertion_free() {
    let _x = 1 + 1;
}

#[test]
fn empty_body() {}

#[test]
fn sleeping() {
    std::thread::sleep(std::time::Duration::from_millis(1));
    assert_eq!(1, 1);
}
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("assertion_free".to_string(), TestSmellKind::AssertionFree),
            ("empty_body".to_string(), TestSmellKind::EmptyBody),
            ("sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-python")]
#[test]
fn python_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "calc_test.py",
        "\
def test_healthy():
    assert 1 + 1 == 2

def test_assertion_free():
    x = compute()

def test_empty():
    pass

def test_sleeping():
    import time
    time.sleep(0.001)
    assert True

def compute():
    return 1
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            (
                "test_assertion_free".to_string(),
                TestSmellKind::AssertionFree
            ),
            ("test_empty".to_string(), TestSmellKind::EmptyBody),
            ("test_sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-go")]
#[test]
fn go_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "calc_test.go",
        "\
package calc

import (
\t\"testing\"
\t\"time\"
)

func TestHealthy(t *testing.T) {
\tif 1+1 != 2 {
\t\tt.Errorf(\"bad\")
\t}
}

func TestAssertionFree(t *testing.T) {
\t_ = 1 + 1
}

func TestEmpty(t *testing.T) {}

func TestSleeping(t *testing.T) {
\ttime.Sleep(time.Millisecond)
\tif false {
\t\tt.Fatal(\"x\")
\t}
}
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            (
                "TestAssertionFree".to_string(),
                TestSmellKind::AssertionFree
            ),
            ("TestEmpty".to_string(), TestSmellKind::EmptyBody),
            ("TestSleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-java")]
#[test]
fn java_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "CalcTest.java",
        "\
import org.junit.jupiter.api.Test;

class CalcTest {
    @Test
    void healthy() {
        assertEquals(2, 1 + 1);
    }

    @Test
    void assertionFree() {
        int x = 1 + 1;
    }

    @Test
    void empty() {}

    @Test
    void sleeping() throws InterruptedException {
        Thread.sleep(1);
        assertTrue(true);
    }

    void helper() {
        assertEquals(1, 1);
    }
}
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("assertionFree".to_string(), TestSmellKind::AssertionFree),
            ("empty".to_string(), TestSmellKind::EmptyBody),
            ("sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn kotlin_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "CalcTest.kt",
        "\
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class CalcTest {
    @Test
    fun healthy() {
        assertEquals(2, 1 + 1)
    }

    @Test
    fun assertionFree() {
        val x = 1 + 1
    }

    @Test
    fun empty() {}

    @Test
    fun sleeping() {
        Thread.sleep(1)
        assertTrue(true)
    }

    fun helper() {
        assertEquals(1, 1)
    }
}
",
    );
    // assertionFree (no assertion call), empty (empty body), sleeping
    // (Thread.sleep) are flagged; healthy asserts and `helper` is not @Test, so
    // neither is reported.
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("assertionFree".to_string(), TestSmellKind::AssertionFree),
            ("empty".to_string(), TestSmellKind::EmptyBody),
            ("sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-c-sharp")]
#[test]
fn csharp_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "CalcTests.cs",
        "\
using System.Threading;
using Xunit;

public class CalcTests {
    [Fact]
    public void Healthy() {
        Assert.Equal(2, 1 + 1);
    }

    [Fact]
    public void AssertionFree() {
        int x = 1 + 1;
    }

    [Fact]
    public void Empty() {}

    [Fact]
    public void Sleeping() {
        Thread.Sleep(1);
        Assert.True(true);
    }

    public void Helper() {
        Assert.Equal(1, 1);
    }
}
",
    );
    // Only the `[Fact]`-attributed methods are candidates (the post-filter gates
    // on the canonical test-marker logic); `Helper` carries no evidence so it is
    // never flagged even though it has no assertion.
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("AssertionFree".to_string(), TestSmellKind::AssertionFree),
            ("Empty".to_string(), TestSmellKind::EmptyBody),
            ("Sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-typescript")]
#[test]
fn typescript_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "calc.test.ts",
        "\
import { add } from './calc';

test('healthy', () => {
  expect(add(1, 2)).toBe(3);
});

test('assertion free', () => {
  const x = add(1, 2);
});

test('empty', () => {});

test('sleeping', () => {
  setTimeout(() => {}, 1);
  expect(add(1, 1)).toBe(2);
});
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("assertion free".to_string(), TestSmellKind::AssertionFree),
            ("empty".to_string(), TestSmellKind::EmptyBody),
            ("sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-ruby")]
#[test]
fn ruby_smells_flag_exactly_the_seeded_tests() {
    // The dual idiom (S-059): minitest seeds assertion-free + sleeping, RSpec
    // seeds the empty-body example — all three confirmed through the canonical
    // Ruby test-marker logic, with the healthy tests of both idioms left alone.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "test/calc_test.rb",
        "\
require \"minitest/autorun\"

class CalcTest < Minitest::Test
  def test_healthy
    assert_equal 2, compute
  end

  def test_assertion_free
    result = compute
  end

  def test_sleeping
    sleep 1
    assert_equal 2, compute
  end

  def compute
    2
  end
end
",
    );
    write(
        tmp.path(),
        "spec/calc_spec.rb",
        "\
RSpec.describe \"Calc\" do
  it \"is empty\" do
  end

  it \"is healthy\" do
    expect(compute).to eq(2)
  end
end
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            ("is empty".to_string(), TestSmellKind::EmptyBody),
            (
                "test_assertion_free".to_string(),
                TestSmellKind::AssertionFree
            ),
            ("test_sleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

#[cfg(feature = "lang-php")]
#[test]
fn php_smells_flag_exactly_the_seeded_tests() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "tests/CalcTest.php",
        "<?php
class CalcTest extends TestCase {
    public function testHealthy() { $this->assertEquals(2, 1 + 1); }

    public function testAssertionFree() { $x = 1 + 1; }

    public function testEmpty() {}

    public function testSleeping() {
        sleep(1);
        $this->assertTrue(true);
    }

    public function helper() { $this->assertEquals(1, 1); }
}
",
    );
    assert_eq!(
        smells_of(&tmp),
        vec![
            (
                "testAssertionFree".to_string(),
                TestSmellKind::AssertionFree
            ),
            ("testEmpty".to_string(), TestSmellKind::EmptyBody),
            ("testSleeping".to_string(), TestSmellKind::Sleeping),
        ]
    );
}

/// A curated language with the smell query but only healthy tests reports an
/// empty appendix — `findings` empty AND `not_analyzed` empty. This is the
/// "clean" state, distinct from `n/a` (no query): a clean language is analyzed
/// and found smell-free, never reported as un-analyzable.
#[cfg(feature = "lang-rust")]
#[test]
fn a_clean_curated_language_is_smell_free_not_na() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
#[test]
fn healthy() {
    assert_eq!(1 + 1, 2);
}
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let report = engine.test_gaps(None, true).expect("test_gaps runs");
    assert!(
        report.smells.findings.is_empty(),
        "a healthy curated-language test yields no smell findings: {:?}",
        report.smells.findings
    );
    assert!(
        report.smells.not_analyzed.is_empty(),
        "a language with the query is analyzed and clean, never n/a: {:?}",
        report.smells.not_analyzed
    );
    assert!(report.smells.label.contains("advisory"));
}

// ── Disk-shadow: changing the query on disk changes detection, no rebuild ────

#[cfg(feature = "lang-rust")]
#[test]
fn shadowing_the_smell_query_on_disk_changes_detection_without_a_rebuild() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
#[test]
fn healthy() {
    assert_eq!(1 + 1, 2);
}
",
    );

    // Embedded query: a healthy assert_eq! test is NOT assertion-free.
    let before = smells_of(&tmp);
    assert!(
        before.is_empty(),
        "the embedded query recognises assert_eq! as an assertion: {before:?}"
    );

    // Drop an override that captures test units but recognises NO assertions,
    // so the same healthy test now reads as assertion-free — detection changed
    // with no recompile, only a fresh registry load (FR-PL-04, FR-PL-05,
    // UAT-CV-04).
    write(
        tmp.path(),
        ".logos/plugins/rust/queries/smells.scm",
        "(function_item name: (identifier) @smell.name body: (block) @smell.body) @smell.test\n",
    );

    let after = smells_of(&tmp);
    assert_eq!(
        after,
        vec![("healthy".to_string(), TestSmellKind::AssertionFree)],
        "the disk-shadowed query flips the healthy test to assertion-free"
    );
}

// ── Gate-immunity: smells never move the gate, never touch the store (BR-28) ──

#[cfg(feature = "lang-rust")]
#[test]
fn reporting_smells_leaves_gate_and_store_byte_identical() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn produce() -> i32 {
    1 + 1
}

#[test]
fn empty_body() {}

#[test]
fn assertion_free() {
    let _ = produce();
}
",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Save a baseline, then capture a steady-state (no-save) gate.
    engine.gate(None, true, true).expect("gate --save");
    let before = engine.gate(None, false, true).expect("gate runs");

    // Compute the advisory appendix — smells are present.
    let tg = engine.test_gaps(None, true).expect("test_gaps runs");
    assert!(
        !tg.smells.findings.is_empty(),
        "the fixture seeds smells so the appendix is non-empty"
    );

    // The gate is identical after smells were reported (BR-28): the same fresh
    // signal, the same baseline it matches, the same verdict and excluded-test
    // count — advisory smells touch no canonical state the gate scores.
    let after = engine.gate(None, false, true).expect("gate runs");
    assert_eq!(
        before.signal, after.signal,
        "fresh signal unchanged (BR-28)"
    );
    assert_eq!(
        before.baseline_signal, after.baseline_signal,
        "baseline unchanged — the smell pass saved nothing"
    );
    assert_eq!(before.passed, after.passed, "verdict unchanged");
    assert_eq!(
        before.test_function_count, after.test_function_count,
        "excluded-test count unchanged"
    );
    assert_eq!(before.message, after.message, "message unchanged");
}
