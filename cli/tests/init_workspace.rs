//! End-to-end tests for `logos init --workspace` (S-246, FR-WS-02), exercised
//! through the real binary over a multi-repo fixture:
//!
//! - `--yes` returns promptly without blocking on indexing;
//! - an existing member's `.logos/config.toml` is never overwritten;
//! - stdout stays machine-clean while the approval gate goes to stderr;
//! - `--exclude` drops a candidate member;
//! - a second run injects no duplicate workspace MCP entry (idempotent).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Instant;

use tempfile::TempDir;

fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

fn exit_code(out: &Output) -> i32 {
    out.status.code().expect("no signal termination")
}

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=test@logos", "-c", "user.name=logos-test"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A committed git repo at `dir` with one file.
fn init_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("f.txt"), "x\n").unwrap();
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// A parent folder with two sibling repos: `api` and `web`.
fn two_member_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    init_repo(&tmp.path().join("api"));
    init_repo(&tmp.path().join("web"));
    tmp
}

/// `--yes` returns promptly (indexing is deferred to a detached background
/// warm, never blocking this command) and reports both members initialised.
#[test]
fn yes_returns_promptly_without_blocking_on_indexing() {
    let tmp = two_member_fixture();
    let start = Instant::now();
    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    let elapsed = start.elapsed();

    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        elapsed.as_secs() < 10,
        "init --workspace must return promptly, not block on indexing N members: {elapsed:?}"
    );

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON on stdout");
    let members = report["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    for m in members {
        assert_eq!(m["status"], "ready", "{m:?}");
    }
}

/// An existing member's hand-edited `.logos/config.toml` is left byte-for-byte
/// untouched (FR-IN-01 non-clobber, inherited verbatim).
#[test]
fn existing_member_config_is_never_overwritten() {
    let tmp = two_member_fixture();
    let api_config_dir = tmp.path().join("api").join(".logos");
    fs::create_dir_all(&api_config_dir).unwrap();
    fs::write(api_config_dir.join("config.toml"), "# hand-edited by the user\n").unwrap();

    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    assert_eq!(
        fs::read_to_string(api_config_dir.join("config.toml")).unwrap(),
        "# hand-edited by the user\n",
        "a pre-existing member config must never be overwritten"
    );
}

/// stdout carries only the machine-readable report; the approval gate (when
/// it would prompt) writes to stderr, never stdout (FR-CL-02). Without a TTY
/// on stdin, `ask()` degrades to its default without printing at all — so
/// even the non-`--yes` path here produces no stdout noise beyond the report.
#[test]
fn stdout_stays_machine_clean() {
    let tmp = two_member_fixture();
    let out = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0);

    // Every line of stdout must be the single JSON report — no prompt text.
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout is exactly one JSON document, nothing else");
}

/// `--exclude` drops a matching candidate from the proposed member set.
#[test]
fn exclude_drops_a_matching_member() {
    let tmp = two_member_fixture();
    let out = logos(
        tmp.path(),
        &["--json", "init", "--workspace", "--yes", "--exclude", "web"],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let names: Vec<&str> = report["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["api"], "web is excluded, api remains");

    let manifest = fs::read_to_string(tmp.path().join("logos.workspace.toml")).unwrap();
    assert!(manifest.contains("api"));
    assert!(!manifest.contains("web"));
}

/// A second run injects no duplicate workspace MCP entry — exactly one
/// `logos-workspace` key, idempotently.
#[test]
fn second_run_does_not_duplicate_the_workspace_mcp_entry() {
    let tmp = two_member_fixture();
    let first = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&first), 0);

    let second = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&second), 0, "{}", String::from_utf8_lossy(&second.stderr));

    let mcp: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(tmp.path().join(".mcp.json")).unwrap()).unwrap();
    let servers = mcp["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1, "still exactly one server entry: {servers:?}");
    assert!(servers.contains_key("logos-workspace"));

    let report: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(report["mcp"]["action"], "unchanged");
    assert_eq!(report["manifest"]["action"], "unchanged");
}

/// A folder with no sibling git repos is a no-op, not a fabricated
/// zero-member workspace.
#[test]
fn no_candidates_is_a_noop() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&out), 0);
    assert!(!tmp.path().join("logos.workspace.toml").exists());
}
