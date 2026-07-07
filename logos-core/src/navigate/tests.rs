//! Unit tests for the navigation service's pure helpers (S-013).
//!
//! Everything that needs a live runtime/store is exercised end-to-end in
//! `tests/navigation.rs`; here we pin the I/O-free seams: code slicing with
//! its path-containment guard, and line-number wire conversion.

use std::fs;

use tempfile::TempDir;

use super::{line_u32, read_code};
use crate::graph_store::NodeRow;
use crate::model::{LogosSymbol, NodeId, NodeKind};

/// A node row bound to `file` with the given 1-based line span.
fn row(file: Option<&str>, start: Option<i64>, end: Option<i64>) -> NodeRow {
    NodeRow {
        id: NodeId(1),
        symbol: LogosSymbol::parse("local 1").expect("local symbol parses"),
        kind: NodeKind::Function,
        name: "f".to_string(),
        file_path: file.map(str::to_string),
        start_line: start,
        end_line: end,
    }
}

#[test]
fn read_code_slices_the_declared_line_span() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("lib.rs"), "l1\nl2\nl3\nl4\nl5\n").unwrap();

    let code = read_code(tmp.path(), &row(Some("lib.rs"), Some(2), Some(4)));
    assert_eq!(code.as_deref(), Some("l2\nl3\nl4"));

    // A single-line declaration (no end_line) yields exactly that line.
    let one = read_code(tmp.path(), &row(Some("lib.rs"), Some(3), None));
    assert_eq!(one.as_deref(), Some("l3"));
}

#[test]
fn read_code_is_none_without_a_file_or_line_binding() {
    let tmp = TempDir::new().unwrap();
    assert!(read_code(tmp.path(), &row(None, Some(1), Some(1))).is_none());
    assert!(read_code(tmp.path(), &row(Some("lib.rs"), None, None)).is_none());
}

#[test]
fn read_code_tolerates_drift_and_missing_files() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("lib.rs"), "only one line\n").unwrap();

    // The file shrank since indexing (best-effort drift, NFR-DM-02) → None,
    // never an error or a panic.
    assert!(read_code(tmp.path(), &row(Some("lib.rs"), Some(10), Some(12))).is_none());
    // The file vanished since indexing.
    assert!(read_code(tmp.path(), &row(Some("gone.rs"), Some(1), Some(1))).is_none());
}

#[test]
fn read_code_refuses_paths_that_escape_the_root() {
    let tmp = TempDir::new().unwrap();
    // Defensive containment: stored paths are project-relative by
    // construction, but a hostile/corrupt store must not read outside root.
    assert!(read_code(tmp.path(), &row(Some("../secrets.txt"), Some(1), Some(1))).is_none());
    assert!(read_code(tmp.path(), &row(Some("/etc/hosts"), Some(1), Some(1))).is_none());
}

/// A repo-controlled symlink has an innocent relative path but points outside
/// the root — the canonical-path check must refuse it (review round 1,
/// security finding).
#[cfg(unix)]
#[test]
fn read_code_refuses_symlinks_that_escape_the_root() {
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.txt"), "leak me\n").unwrap();

    let tmp = TempDir::new().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        tmp.path().join("evil.rs"),
    )
    .unwrap();

    assert!(
        read_code(tmp.path(), &row(Some("evil.rs"), Some(1), Some(1))).is_none(),
        "a symlink escaping the root must not be readable"
    );

    // A symlink that stays INSIDE the root is fine (vendored layouts do this).
    fs::write(tmp.path().join("real.rs"), "fn ok() {}\n").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("real.rs"), tmp.path().join("alias.rs")).unwrap();
    assert_eq!(
        read_code(tmp.path(), &row(Some("alias.rs"), Some(1), Some(1))).as_deref(),
        Some("fn ok() {}")
    );
}

#[test]
fn line_numbers_convert_to_wire_u32_dropping_nonsense() {
    assert_eq!(line_u32(Some(42)), Some(42));
    assert_eq!(line_u32(Some(0)), None, "0 is not a valid 1-based line");
    assert_eq!(line_u32(Some(-3)), None);
    assert_eq!(line_u32(None), None);
}

// ── FR-CL-04 `--tests-only`: the test-path naming heuristic ─────────────────

#[test]
fn is_test_path_recognises_per_language_test_conventions() {
    use super::is_test_path;
    // Directory segments, any language.
    assert!(is_test_path("tests/navigation.rs"));
    assert!(is_test_path("pkg/test/helper.go"));
    assert!(is_test_path("src/__tests__/app.tsx"));
    // Filename idioms: Rust/Go `_test`, Python `test_`, JS `.test`/`.spec`,
    // Java `*Test(s)`.
    assert!(is_test_path("src/core_test.rs"));
    assert!(is_test_path("pkg/server_test.go"));
    assert!(is_test_path("app/test_models.py"));
    assert!(is_test_path("src/Button.test.tsx"));
    assert!(is_test_path("src/api.spec.ts"));
    assert!(is_test_path("src/main/UserServiceTest.java"));
    assert!(is_test_path("src/main/UserServiceTests.java"));
    // Ruby: RSpec `spec/` directory + `*_spec.rb`, minitest `*_test.rb`.
    assert!(is_test_path("spec/models/user_spec.rb"));
    assert!(is_test_path("models/user_spec.rb"));
    assert!(is_test_path("test/user_test.rb"));
}

#[test]
fn is_test_path_does_not_mark_production_files() {
    use super::is_test_path;
    assert!(!is_test_path("src/lib.rs"));
    assert!(!is_test_path("src/testing_utils.rs")); // prefix ≠ marker
    assert!(!is_test_path("src/test_utils.rs")); // test_ prefix marks .py only
    assert!(!is_test_path("src/contest.rs")); // substring ≠ suffix token
    assert!(!is_test_path("app/models.py"));
    assert!(!is_test_path("src/latest.ts")); // "test" inside a word
    assert!(!is_test_path("src/protest/march.rs")); // segment must equal
}

// ── FR-NV-01 search: raw query → safe FTS5 phrase ───────────────────────────

#[test]
fn phrase_query_wraps_raw_text_so_punctuation_is_inert() {
    use super::phrase_query;
    // The reported bug: `web-surface` must become one quoted phrase, not a
    // bare expression whose `-` FTS5 reads as syntax.
    assert_eq!(phrase_query("web-surface").as_deref(), Some("\"web-surface\""));
    // A plain token is quoted too, but matches exactly as before.
    assert_eq!(phrase_query("surface").as_deref(), Some("\"surface\""));
    // Embedded double-quotes are doubled per FTS5 string quoting.
    assert_eq!(phrase_query("a\"b").as_deref(), Some("\"a\"\"b\""));
    // Empty/whitespace is a well-defined no-op, not an FTS syntax error.
    assert_eq!(phrase_query(""), None);
    assert_eq!(phrase_query("   "), None);
}
