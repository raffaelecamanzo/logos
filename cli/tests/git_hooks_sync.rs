//! The installed git hooks, through the real `logos` binary (S-783, CR-242,
//! [FR-SY-05](../../docs/specs/requirements/FR-SY-05.md),
//! [FR-SY-06](../../docs/specs/requirements/FR-SY-06.md),
//! [UAT-IN-01](../../docs/specs/requirements/UAT-IN-01.md) hook half).
//!
//! `logos-core/tests/git_hooks.rs` proves the hooks hand a stub `logos` every
//! path exactly as git records it. This is the other half: with the real
//! binary behind the hook, a commit of a non-ASCII, a quoted and a tab-bearing
//! file name leaves the graph consistent (`verify` reports no drift) and the
//! symbols those files define are findable.

#![cfg(unix)] // The hook scripts are /bin/sh.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Run the built `logos` against `project`. Git discovery stops at the
/// fixture's parent so the repository is never taken for part of an enclosing
/// one, and the user's Claude Code directory points nowhere.
fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .env("CLAUDE_CONFIG_DIR", project.join(".no-claude-config"))
        .env("GIT_CEILING_DIRECTORIES", project.parent().unwrap_or(project))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

/// Run git in `project` with the binary's own directory first on `PATH`, so the
/// hooks it fires find the real `logos` the way a user's do; the repository's
/// housekeeping stays off.
fn git(project: &Path, args: &[&str]) {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_logos"))
        .parent()
        .expect("bin dir");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new("git")
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(args)
        .env("PATH", path)
        .env("CLAUDE_CONFIG_DIR", project.join(".no-claude-config"))
        .env("GIT_CEILING_DIRECTORIES", project.parent().unwrap_or(project))
        .current_dir(project)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// After a commit of `café.rs`, `a"b.rs` and a tab-named file, the graph the
/// hook left matches a fresh reindex, and `search` finds each file's symbol.
#[test]
fn committing_awkwardly_named_files_leaves_the_graph_consistent() {
    let tmp = TempDir::new().expect("fixture dir");
    let root = tmp.path();
    git(root, &["init", "--quiet", "-b", "main"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Logos Test"]);
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/base.rs"), "pub fn base() {}\n").unwrap();
    git(root, &["add", "--", "src"]);
    git(root, &["commit", "--quiet", "-m", "base"]);

    let init = logos(root, &["init", "--hooks", "--json"]);
    assert_eq!(init.status.code(), Some(0), "{}", String::from_utf8_lossy(&init.stderr));
    let index = logos(root, &["index"]);
    assert_eq!(index.status.code(), Some(0), "{}", String::from_utf8_lossy(&index.stderr));

    let files = [
        ("café.rs", "cafe_brew"),
        ("a\"b.rs", "quoted_one"),
        ("tab\there.rs", "tabbed_one"),
    ];
    for (name, symbol) in files {
        fs::write(root.join(name), format!("pub fn {symbol}() {{}}\n")).unwrap();
    }
    git(root, &["add", "--", "café.rs", "a\"b.rs", "tab\there.rs"]);
    git(root, &["commit", "--quiet", "-m", "awkward names"]);

    let verify = logos(root, &["verify", "--json"]);
    assert_eq!(
        verify.status.code(),
        Some(0),
        "no drift after the hook's sync: {}{}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&verify.stdout).expect("verify json");
    assert_eq!(report["ok"], serde_json::json!(true), "{report}");

    for (name, symbol) in files {
        let search = logos(root, &["search", symbol, "--json"]);
        let hits = String::from_utf8_lossy(&search.stdout).into_owned();
        // The file name as JSON spells it inside a string (quote and tab escaped).
        let spelled = serde_json::to_string(name).expect("json string");
        let spelled = &spelled[1..spelled.len() - 1];
        assert!(
            hits.contains(symbol) && hits.contains(spelled),
            "`search {symbol}` must find it in {name:?}: {hits}"
        );
    }
}
