//! The source-tool sandbox suite (S-167, [NFR-SE-04]).
//!
//! Drives `read`/`grep`/`glob` against `../` traversal, absolute paths outside
//! the root, and `ignored_dirs` targets — all rejected/skipped — plus a symlink
//! escape, and confirms the happy path returns grounded file content.
//!
//! [NFR-SE-04]: filesystem walk is contained.

use std::sync::Arc;

use agent_core::{source_toolset, Sandbox, SandboxError};

/// A fixture tree:
/// ```text
/// src/lib.rs        "pub fn visible() { needle(); }"
/// src/inner/mod.rs  "fn nested() {}"
/// notes.txt         "needle in a text file"
/// target/junk.rs    "fn buried() { needle(); }"   (under an ignored dir)
/// ```
fn fixture() -> (Arc<Sandbox>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/inner")).expect("mkdir src/inner");
    std::fs::create_dir_all(root.join("target")).expect("mkdir target");
    std::fs::write(root.join("src/lib.rs"), "pub fn visible() { needle(); }\n").expect("lib.rs");
    std::fs::write(root.join("src/inner/mod.rs"), "fn nested() {}\n").expect("mod.rs");
    std::fs::write(root.join("notes.txt"), "needle in a text file\n").expect("notes.txt");
    std::fs::write(root.join("target/junk.rs"), "fn buried() { needle(); }\n").expect("junk.rs");

    let sandbox = Sandbox::new(root, ["target".to_string()]).expect("sandbox");
    (Arc::new(sandbox), dir)
}

// ── resolve(): the containment core ─────────────────────────────────────────

#[test]
fn resolve_accepts_a_confined_relative_path() {
    let (sandbox, _dir) = fixture();
    let resolved = sandbox.resolve("src/lib.rs").expect("a confined path resolves");
    assert!(resolved.starts_with(sandbox.root()));
}

#[test]
fn resolve_rejects_parent_traversal() {
    let (sandbox, _dir) = fixture();
    let err = sandbox.resolve("../escape.txt").expect_err("`..` is refused");
    assert!(matches!(err, SandboxError::Traversal(_)), "got {err:?}");
}

#[test]
fn resolve_rejects_nested_parent_traversal() {
    let (sandbox, _dir) = fixture();
    let err = sandbox
        .resolve("src/../../escape.txt")
        .expect_err("a `..` anywhere is refused");
    assert!(matches!(err, SandboxError::Traversal(_)), "got {err:?}");
}

#[test]
fn resolve_rejects_absolute_paths() {
    let (sandbox, _dir) = fixture();
    let err = sandbox
        .resolve("/etc/passwd")
        .expect_err("an absolute path outside the root is refused");
    assert!(matches!(err, SandboxError::AbsolutePath(_)), "got {err:?}");
}

#[test]
fn resolve_rejects_ignored_directories() {
    let (sandbox, _dir) = fixture();
    let err = sandbox
        .resolve("target/junk.rs")
        .expect_err("a path under an ignored dir is refused");
    assert!(
        matches!(err, SandboxError::Ignored(_, ref d) if d == "target"),
        "got {err:?}"
    );
}

#[test]
fn resolve_reports_a_missing_path() {
    let (sandbox, _dir) = fixture();
    let err = sandbox
        .resolve("src/does_not_exist.rs")
        .expect_err("a missing path is reported, not fabricated");
    assert!(matches!(err, SandboxError::NotFound(_)), "got {err:?}");
}

#[cfg(unix)]
#[test]
fn resolve_rejects_a_symlink_into_an_ignored_subtree() {
    use std::os::unix::fs::symlink;

    let (sandbox, _dir) = fixture();
    // A symlink inside the root whose target is also inside the root but under
    // the ignored `target/` — the canonical re-scan must still refuse it.
    symlink(
        sandbox.root().join("target/junk.rs"),
        sandbox.root().join("sneaky.rs"),
    )
    .expect("symlink");

    let err = sandbox
        .resolve("sneaky.rs")
        .expect_err("a symlink resolving into an ignored subtree is refused");
    assert!(
        matches!(err, SandboxError::Ignored(_, ref d) if d == "target"),
        "got {err:?}"
    );
}

#[cfg(unix)]
#[test]
fn resolve_rejects_a_symlink_escaping_the_root() {
    use std::os::unix::fs::symlink;

    let (sandbox, _dir) = fixture();
    // A separate tree, outside the sandbox root.
    let outside = tempfile::tempdir().expect("outside tempdir");
    std::fs::write(outside.path().join("secret.txt"), "top secret\n").expect("secret");
    // A symlink *inside* the root that points out of it.
    symlink(
        outside.path().join("secret.txt"),
        sandbox.root().join("escape_link"),
    )
    .expect("symlink");

    let err = sandbox
        .resolve("escape_link")
        .expect_err("a symlink whose target is outside the root is refused");
    assert!(matches!(err, SandboxError::Escape(_)), "got {err:?}");
}

// The `is_containment_refusal` arm-partition predicate is unit-tested in-file
// alongside its definition (`agent-core/src/tools/source.rs` `mod tests`); this
// suite stays focused on behavioral `resolve`/tool integration coverage.

// ── read ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn read_returns_grounded_file_content() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let out = tools
        .call("read", serde_json::json!({ "path": "src/lib.rs" }).to_string())
        .await
        .expect("read a confined file");
    let value: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert!(
        value["content"].as_str().unwrap().contains("visible"),
        "read returns the real file content: {value}"
    );
    assert_eq!(value["truncated"], false);
}

#[tokio::test]
async fn read_refuses_a_traversal_attempt() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let err = tools
        .call("read", serde_json::json!({ "path": "../../etc/passwd" }).to_string())
        .await
        .expect_err("traversal is refused");
    assert!(err.to_string().contains("escapes the project root"));
}

#[tokio::test]
async fn read_refuses_a_directory() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let err = tools
        .call("read", serde_json::json!({ "path": "src" }).to_string())
        .await
        .expect_err("a directory is not a readable file");
    assert!(err.to_string().contains("not a regular file"));
}

#[tokio::test]
async fn read_truncates_at_the_byte_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("big.txt"), "x".repeat(10_000)).expect("big file");
    let sandbox = Arc::new(
        Sandbox::new(dir.path(), std::iter::empty())
            .expect("sandbox")
            .with_max_read_bytes(1_000),
    );
    let tools = source_toolset(sandbox);
    let out = tools
        .call("read", serde_json::json!({ "path": "big.txt" }).to_string())
        .await
        .expect("read");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["bytes_read"], 1_000);
    assert_eq!(value["truncated"], true);
}

// ── grep ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn grep_finds_matches_and_skips_ignored_dirs() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let out = tools
        .call("grep", serde_json::json!({ "pattern": "needle" }).to_string())
        .await
        .expect("grep");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    let files: Vec<&str> = value["matches"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["path"].as_str())
        .collect();

    assert!(
        files.iter().any(|p| p.contains("lib.rs")),
        "grep finds the needle in src/lib.rs: {value}"
    );
    assert!(
        files.iter().any(|p| p.contains("notes.txt")),
        "grep crosses file types (text too): {value}"
    );
    assert!(
        !files.iter().any(|p| p.contains("target")),
        "grep never descends into the ignored `target/`: {value}"
    );
}

#[tokio::test]
async fn grep_refuses_a_scope_inside_an_ignored_dir() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let err = tools
        .call(
            "grep",
            serde_json::json!({ "pattern": "needle", "path": "target" }).to_string(),
        )
        .await
        .expect_err("scoping the search into an ignored dir is refused");
    assert!(err.to_string().contains("ignored directory"));
}

#[tokio::test]
async fn grep_refuses_an_invalid_regex() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let err = tools
        .call("grep", serde_json::json!({ "pattern": "[" }).to_string())
        .await
        .expect_err("a malformed regex is refused, not silently empty");
    assert!(err.to_string().contains("invalid regex"));
}

// ── glob ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn glob_refuses_an_invalid_pattern() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let err = tools
        .call("glob", serde_json::json!({ "pattern": "src/[" }).to_string())
        .await
        .expect_err("a malformed glob (unclosed character class) is refused");
    assert!(err.to_string().contains("invalid glob"));
}

#[tokio::test]
async fn glob_matches_within_root_and_skips_ignored_dirs() {
    let (sandbox, _dir) = fixture();
    let tools = source_toolset(sandbox);
    let out = tools
        .call("glob", serde_json::json!({ "pattern": "**/*.rs" }).to_string())
        .await
        .expect("glob");
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    let paths: Vec<&str> = value["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.as_str())
        .collect();

    assert!(paths.iter().any(|p| p.ends_with("src/lib.rs")), "{value}");
    assert!(
        paths.iter().any(|p| p.ends_with("src/inner/mod.rs")),
        "glob recurses with `**`: {value}"
    );
    assert!(
        !paths.iter().any(|p| p.contains("target")),
        "glob never lists files under the ignored `target/`: {value}"
    );
}
