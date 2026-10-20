//! `logos review --base` through the binary (S-677, [FR-GV-24]): a rerun
//! prints the same bytes, the shadow before-state is built once and then read
//! from the cache (said on stderr, never on stdout), a base that does not
//! resolve exits 1, and `check`/`gate` exit as they did before the review.
//!
//! `review --branches` (S-757, [FR-GV-28]) exits 0 whatever it reports: an
//! exceeded limit, no exceeded limit, a branch it cannot measure, or a base
//! that does not resolve.
//!
//! [FR-GV-24]: ../../docs/specs/requirements/FR-GV-24.md
//! [FR-GV-28]: ../../docs/specs/requirements/FR-GV-28.md

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// `git -C <root> <args…>` hermetically, asserting success.
fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c", "user.email=dev@logos", "-c", "user.name=Logos Dev",
            "-c", "commit.gpgsign=false", "-c", "core.hooksPath=",
            "-c", "maintenance.auto=false", "-c", "gc.auto=0",
        ])
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `name` nesting `depth` `if`s.
fn nested(name: &str, depth: usize) -> String {
    let open: String = (0..depth).map(|i| format!("if x > {i} {{\n")).collect();
    format!("pub fn {name}(x: i32) -> i32 {{\n{open}return x;\n{}0\n}}\n", "}\n".repeat(depth))
}

/// The binary over `root`, never escaping into an enclosing repository.
fn logos(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(root)
        .args(args)
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .expect("logos runs")
}

/// `main` with a 4-deep function; `feature` adds a 6-deep one; indexed.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", &nested("alpha", 4));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);
    git(root, &["checkout", "-q", "-b", "feature"]);
    write(root, "src/lib.rs", &format!("{}{}", nested("alpha", 4), nested("beta", 6)));
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "deep"]);
    let out = logos(root, &["index"]);
    assert!(out.status.success(), "index: {}", String::from_utf8_lossy(&out.stderr));
    tmp
}

#[test]
fn review_is_byte_stable_caches_its_before_state_and_moves_no_other_exit_code() {
    let tmp = fixture();
    let root = tmp.path();
    let check = logos(root, &["check", "--allow-no-rules"]).status.code();
    let gate = logos(root, &["gate"]).status.code();

    let args = ["--json", "review", "--base", "main"];
    let (first, second) = (logos(root, &args), logos(root, &args));
    assert_eq!(first.status.code(), Some(0), "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(second.status.code(), Some(0), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(first.stdout, second.stdout, "a rerun prints the same bytes");
    assert!(String::from_utf8_lossy(&first.stderr).contains("built and cached"), "{first:?}");
    assert!(String::from_utf8_lossy(&second.stderr).contains("from the cache"), "{second:?}");

    let review: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(review["measured"], true, "{review}");
    let beta = review["offenders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["dimension"] == "nesting" && o["name"] == "beta")
        .unwrap_or_else(|| panic!("beta is listed: {review}"));
    assert_eq!((&beta["status"], &beta["after"], &beta["threshold"]), (&Value::from("new"), &Value::from(6), &Value::from(4)));
    let security = &review["sections"]["security"];
    assert_eq!(security["measured"], false, "never scanned is not clean: {security}");
    assert!(
        security["not_measured_because"].as_str().is_some_and(|why| why.contains("no security report")),
        "{security}"
    );

    assert_eq!(logos(root, &["check", "--allow-no-rules"]).status.code(), check);
    assert_eq!(logos(root, &["gate"]).status.code(), gate);

    let missing = logos(root, &["--json", "review", "--base", "no-such-ref"]);
    assert_eq!(missing.status.code(), Some(1), "could not measure: {}", String::from_utf8_lossy(&missing.stderr));
    let missing: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["measured"], false, "{missing}");
    assert_eq!(missing["sections"], serde_json::json!({}), "an unmeasured review carries no section");
}

/// `src/engine.rs` calling one function in each of `modules`.
fn engine_calling(modules: &[&str]) -> String {
    let calls: String = modules.iter().map(|m| format!("    crate::{m}::work_{m}();\n")).collect();
    format!("pub fn run() {{\n{calls}}}\n")
}

/// The engine module at `max_fan_out − 1` on `main`; `b1` and `b2` each add
/// one outgoing module edge to it. Indexed, as every query is: the projection
/// itself measures the committed trees.
fn branches_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    write(root, ".gitignore", ".logos/\n");
    write(root, "src/lib.rs", "pub mod engine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\n");
    for m in ["a", "b", "c", "d"] {
        write(root, &format!("src/{m}.rs"), &format!("pub fn work_{m}() {{}}\n"));
    }
    write(root, "src/engine.rs", &engine_calling(&["a", "b"]));
    write(root, ".logos/rules.toml", "[constraints]\nmax_fan_out = 3\n");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "base"]);
    git(root, &["branch", "-M", "main"]);
    for (branch, extra) in [("b1", "c"), ("b2", "d")] {
        git(root, &["checkout", "-q", "-b", branch, "main"]);
        write(root, "src/engine.rs", &engine_calling(&["a", "b", extra]));
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "one more edge"]);
    }
    git(root, &["checkout", "-q", "main"]);
    let out = logos(root, &["index"]);
    assert!(out.status.success(), "index: {}", String::from_utf8_lossy(&out.stderr));
    tmp
}

#[test]
fn review_branches_exits_0_whatever_it_reports() {
    let tmp = branches_fixture();
    let root = tmp.path();
    let run = |args: &[&str]| {
        let out = logos(root, args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };

    let both = run(&["--json", "review", "--base", "main", "--branches", "b1", "b2"]);
    let exceeded = both["exceeded"].as_array().unwrap();
    assert_eq!(exceeded.len(), 1, "{both}");
    assert_eq!((&exceeded[0]["rule"], &exceeded[0]["projected"]), (&Value::from("max_fan_out"), &Value::from(4)));
    let shares: Vec<(&str, i64)> = exceeded[0]["shares"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["branch"].as_str().unwrap(), s["share"].as_i64().unwrap()))
        .collect();
    assert_eq!(shares, [("b1", 1), ("b2", 1)]);

    let alone = run(&["--json", "review", "--base", "main", "--branches", "b1"]);
    assert_eq!(alone["exceeded"], serde_json::json!([]), "{alone}");
    let missing = run(&["--json", "review", "--base", "main", "--branches", "b1", "no-such-branch"]);
    assert_eq!(missing["branches"][1]["measured"], false, "{missing}");
    let no_base = run(&["--json", "review", "--base", "no-such-ref", "--branches", "b1"]);
    assert_eq!(no_base["measured"], false, "{no_base}");
}
