//! End-to-end tests for the `logos` binary surface (S-016, FR-CL-01..05,
//! UAT-CL-01..03, NFR-UX-03), exercised through the real executable so exit
//! codes and stdout/stderr separation are asserted exactly as CI sees them.
//!
//! Coverage by acceptance criterion:
//! - every subcommand parses, with global flags honoured everywhere
//!   (FR-CL-01/02, UAT-CL-01);
//! - exit codes 0/1/2/3 are all triggered end-to-end — the violation path
//!   (1) through the S-020-wired `check`/`gate` (FR-CL-03, FR-GV-03,
//!   UAT-CL-02);
//! - `affected` returns the reverse-transitive closure through the binary
//!   (FR-CL-04, UAT-CL-03);
//! - `--json` emits machine-readable output and `--quiet` suppresses the
//!   human rendering (FR-CL-02).

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Run the built `logos` binary with `args` against `project` via the global
/// `--project` flag (so tests never depend on the harness cwd).
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

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A small indexable fixture with a known cross-file dependency chain:
/// top.rs → mid.rs → core.rs.
fn fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/core.rs", "pub fn base() {}\n");
    write(
        tmp.path(),
        "src/mid.rs",
        "use crate::core::base;\npub fn mid() {\n    base();\n}\n",
    );
    write(
        tmp.path(),
        "src/top.rs",
        "use crate::mid::mid;\npub fn top() {\n    mid();\n}\n",
    );
    tmp
}

/// Every subcommand with representative args (FR-CL-01). Used both for the
/// parse sweep and the global-flag sweep.
const REPRESENTATIVE: &[&[&str]] = &[
    &["init"],
    &["init", "-i", "--hooks"],
    &["init", "--workspace", "--yes", "--exclude", "nope-*"],
    &["index"],
    &["sync", "src/lib.rs"],
    &["status"],
    &["search", "alpha", "--kind", "function", "--limit", "5"],
    &["query", "alpha", "--callers"],
    &[
        "context",
        "find",
        "the",
        "thing",
        "--max-nodes",
        "10",
        "--no-code",
    ],
    &["explore", "alpha", "--max-files", "3"],
    &["node", "alpha", "--code"],
    &["callers", "alpha", "--limit", "5"],
    &["callees", "alpha", "--limit", "5"],
    &["impact", "alpha", "--depth", "2"],
    &[
        "impact-intersection",
        "--item",
        "S-1=top",
        "--item",
        "S-2=base",
        "--depth",
        "2",
    ],
    &["precedent", "top", "--limit", "5"],
    &[
        "branch-overlap",
        "--ref",
        "main",
        "--ref",
        "topic",
        "--base",
        "main",
        "--merge",
        "main",
    ],
    &["affected", "src/core.rs", "--tests-only"],
    &["scan", "src"],
    &["check"],
    &["gate", "--threshold", "7000"],
    &["health"],
    &["health", "--no-reconcile"],
    &["session-start"],
    &["session-end"],
    &["doctor"],
    &["verify"],
    &["evolution"],
    &["dsm"],
    &["hotspots", "--untested"],
    &["hotspots", "--production-scope"],
    &["coverage", "status"],
    &["coverage", "ingest", "coverage.lcov", "--format", "lcov"],
    &["coverage", "refresh"],
    &[
        "wiki",
        "write",
        "guide/a",
        "--title",
        "About a",
        "--generator",
        "gen",
        "--anchor",
        "file:src/a.rs",
        "the body",
    ],
    &["wiki", "read", "guide/a"],
    &["wiki", "search", "phrase"],
    &["wiki", "search", "--list"],
    &["wiki", "status"],
    &["wiki", "generate"],
    &["wiki", "materialize"],
    &["wiki", "hook", "--emit"],
    &["wiki", "delete", "guide/a"],
    &["stats", "--window", "30"],
    &["languages"],
    &["serve", "--mcp"],
];

// ── FR-CL-01 / FR-CL-02 / UAT-CL-01: parsing + global flags ─────────────────

#[test]
fn every_subcommand_parses_with_global_flags() {
    let tmp = TempDir::new().unwrap();
    for args in REPRESENTATIVE {
        // Global flags up front; whatever the command then does (succeed,
        // refuse the missing index, hit a not-yet-wired stub) it must never
        // be a usage error: parsing succeeded (UAT-CL-01).
        let mut full = vec!["--json", "--quiet"];
        full.extend_from_slice(args);
        let out = logos(tmp.path(), &full);
        assert_ne!(
            exit_code(&out),
            2,
            "`logos {}` failed to parse: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn every_subcommand_exposes_help() {
    let tmp = TempDir::new().unwrap();
    for args in REPRESENTATIVE {
        let out = logos(tmp.path(), &[args[0], "--help"]);
        assert_eq!(exit_code(&out), 0, "`logos {} --help` exits 0", args[0]);
    }
}

#[test]
fn global_flags_are_accepted_after_the_subcommand() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["languages", "--json", "--quiet"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── FR-CL-03 / UAT-CL-02: exit codes ────────────────────────────────────────

#[test]
fn success_exits_zero_with_machine_readable_json() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["languages", "--json"]);
    assert_eq!(exit_code(&out), 0);
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    assert!(json.get("languages").is_some(), "read-model shape: {json}");
}

#[test]
fn zero_admission_index_warns_but_still_exits_zero() {
    // FR-IX-13 AC1 + FR-CL-03: at a parent folder of sibling git repositories
    // every child is pruned as a nested git boundary, so `index --json` reports
    // `files_indexed: 0` with exactly one warning naming the prune count, a
    // bounded sample and `logos init --workspace` — and still exits 0. The
    // diagnostic is advisory: it must never move the exit code (CR-098).
    let tmp = TempDir::new().unwrap();
    for i in 0..5 {
        let svc = format!("svc-{i:02}");
        write(tmp.path(), &format!("{svc}/.git/HEAD"), "ref: refs/heads/main\n");
        write(tmp.path(), &format!("{svc}/src/lib.rs"), "pub fn f() {}\n");
    }

    let out = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "a zero-admission index still exits 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    assert_eq!(json["files_indexed"], 0, "nothing is admitted: {json}");

    let warnings = json["warnings"].as_array().expect("warnings array: {json}");
    let named: Vec<&str> = warnings
        .iter()
        .filter_map(|w| w.as_str())
        .filter(|w| w.contains("logos init --workspace"))
        .collect();
    assert_eq!(named.len(), 1, "exactly one prune warning: {warnings:?}");
    assert!(named[0].contains("5 directories were pruned"), "count named: {}", named[0]);
    assert!(named[0].contains("svc-00"), "bounded sample given: {}", named[0]);
}

#[test]
fn usage_errors_exit_two() {
    let tmp = TempDir::new().unwrap();
    // Unknown subcommand, missing required argument, bad --kind value, and
    // conflicting query flags are all usage errors (clap-owned, FR-CL-03).
    for args in [
        &["definitely-not-a-command"] as &[&str],
        &["search"],
        &["search", "x", "--kind", "nonsense"],
        &["query", "x", "--callers", "--callees"],
        // `--mcp` is clap-required: `serve` bare is a parser-owned usage error.
        &["serve"],
    ] {
        let out = logos(tmp.path(), args);
        assert_eq!(
            exit_code(&out),
            2,
            "`logos {}` is a usage error: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ── FR-DS-03 / ADR-60: `ui` is the default feature, `--no-default-features`
// stays the slim, listener-free build ─────────────────────────────────────

/// True if a `--ui` *option line* (not just a prose mention — the `--mcp`
/// flag's own description text references `` `--ui` `` even when the flag
/// doesn't exist) is present in `serve --help` output.
fn help_lists_ui_flag(stdout: &str) -> bool {
    stdout.lines().any(|l| l.trim_start().starts_with("--ui"))
}

/// Guards the S-287 feature-graph flip itself: since `default = ["lang-all",
/// "ui"]`, a plain (default-features) build must expose `--ui`. Without this
/// assertion, an accidental revert of the `default` line would only be
/// caught by a human re-running the manual verification from the impl notes.
#[test]
#[cfg(feature = "ui")]
fn default_build_serve_help_lists_ui() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["serve", "--help"]);
    assert_eq!(exit_code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        help_lists_ui_flag(&stdout),
        "serve --help should list --ui under the default (ui-enabled) feature set: {stdout}"
    );
}

/// The mirror image: the slim `--no-default-features --features lang-all`
/// build must stay listener-free — `--ui` must not exist as a flag.
#[test]
#[cfg(not(feature = "ui"))]
fn slim_build_serve_help_has_no_ui() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["serve", "--help"]);
    assert_eq!(exit_code(&out), 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !help_lists_ui_flag(&stdout),
        "the slim build must not expose --ui: {stdout}"
    );
}

#[test]
fn a_malformed_config_makes_index_fail_loud_with_exit_two() {
    // FR-CF-03 / configuration.md: a typo in config.toml must fail loud with
    // exit 2, not degrade to a silent empty index. `Engine::index` is an
    // infallible surface (ADR-14), so the CLI validates config up front — the
    // same loud-failure contract `check`/`gate` honour for rules.toml.
    let tmp = fixture();
    write(
        tmp.path(),
        ".logos/config.toml",
        "[config_artifacts]\nenabled = true\nbogus_key = 7\n",
    );
    let out = logos(tmp.path(), &["index"]);
    assert_eq!(
        exit_code(&out),
        2,
        "an unknown config.toml key is a usage fault (exit 2): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A valid config still indexes cleanly (exit 0) — the guard rejects only
    // genuine faults.
    write(
        tmp.path(),
        ".logos/config.toml",
        "[config_artifacts]\nenabled = true\n",
    );
    assert_eq!(
        exit_code(&logos(tmp.path(), &["index"])),
        0,
        "a valid config.toml indexes cleanly"
    );
}

#[test]
fn a_missing_index_exits_three_with_an_actionable_message() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["search", "anything"]);
    assert_eq!(
        exit_code(&out),
        3,
        "no .logos/ → internal/environment (FR-EH-01)"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("logos index"),
        "the error names the remedy (NFR-UX-02): {stderr}"
    );
}

/// FR-GV-03 / UAT-CL-02: a repo violating its checked-in contract exits 1
/// through the real binary.
#[test]
fn check_exits_one_on_a_rule_violation() {
    let tmp = fixture();
    // A layered contract the fixture deliberately violates: core (order 0)
    // must not be depended on upward... mid.rs (order 1) calling core.rs is
    // downward and fine, so forbid it explicitly with a boundary instead.
    write(
        tmp.path(),
        ".logos/rules.toml",
        "\
[[layers]]
name  = \"core\"
paths = [\"src/core.rs\"]
order = 0

[[layers]]
name  = \"mid\"
paths = [\"src/mid.rs\"]
order = 1

[[boundaries]]
from = \"mid\"
to   = \"core\"
",
    );
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["check", "--json"]);
    assert_eq!(
        exit_code(&out),
        1,
        "an error violation exits 1 (FR-GV-03): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["passed"], false);
    assert!(
        json["violations"].as_array().is_some_and(|v| !v.is_empty()),
        "the report carries the violations: {json}"
    );
}

/// FR-GV-22 / FR-CL-03: no rules contract loaded exits 4 — distinct from the
/// clean-pass exit 0 — and the report neither claims `passed: true` nor omits
/// the honest `rules_present: false` signal.
#[test]
fn check_exits_four_with_no_rules_contract() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["check", "--json"]);
    assert_eq!(
        exit_code(&out),
        4,
        "no rules contract loaded exits 4 (FR-GV-22): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["rules_present"], false);
    assert!(
        json["passed"].is_null(),
        "no verdict is reported over an empty evaluated set: {json}"
    );
}

/// FR-GV-22: `--allow-no-rules` restores exit 0 for callers that have
/// deliberately authored no contract yet.
#[test]
fn check_allow_no_rules_restores_exit_zero() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["check", "--json", "--allow-no-rules"]);
    assert_eq!(
        exit_code(&out),
        0,
        "the opt-out flag collapses the absent-contract state to 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["rules_present"], false);
    assert!(json["passed"].is_null(), "the opt-out changes the exit code, not the report: {json}");
}

/// FR-GV-22: the human-readable surface names the absent-contract condition
/// explicitly, rather than rendering it as a zero violation count.
#[test]
fn check_human_readable_surface_names_the_absent_contract_condition() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["check"]);
    assert_eq!(exit_code(&out), 4);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no rules contract found — nothing was evaluated"),
        "the condition is named explicitly: {stdout}"
    );
    assert!(
        !stdout.contains("checked_rules"),
        "not rendered as a violation count: {stdout}"
    );
}

/// FR-GV-22 AC: exit 4 never fires when a contract is present, whatever the
/// violation count — a clean, loaded contract still exits 0.
#[test]
fn check_exits_zero_with_a_clean_loaded_contract() {
    let tmp = fixture();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[[forbidden_imports]]\nfrom = \"src/nope_*.rs\"\nto = \"src/also_nope_*.rs\"\n",
    );
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["check", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "a loaded, clean contract exits 0, never 4: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["rules_present"], true);
    assert_eq!(json["passed"], true);
}

/// FR-GV-05 / UAT-CL-02: a regression after `gate --save` exits 1 through
/// the real binary.
#[test]
fn gate_exits_one_on_regression() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);
    let saved = logos(tmp.path(), &["gate", "--save", "--quiet"]);
    assert_eq!(
        exit_code(&saved),
        0,
        "{}",
        String::from_utf8_lossy(&saved.stderr)
    );

    // Degrade the graph: a dependency cycle drops acyclicity.
    write(
        tmp.path(),
        "src/core.rs",
        "\
pub fn base() {}
fn tangle_a() {
    tangle_b();
}
fn tangle_b() {
    tangle_a();
}
",
    );
    let out = logos(tmp.path(), &["gate", "--json"]);
    assert_eq!(
        exit_code(&out),
        1,
        "a regression past the baseline exits 1 (FR-GV-05): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(json["passed"], false);
}

#[test]
fn scan_on_an_indexed_fixture_succeeds_with_a_freshness_line() {
    // S-020 wired `scan` (this test previously pinned the pre-S-020 stub's
    // exit-3 panic mapping): a scan on an indexed fixture exits 0 and its
    // JSON carries the FR-RC-03 freshness line and a signal.
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);
    let out = logos(tmp.path(), &["scan", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let freshness = json["freshness"].as_str().expect("freshness line present");
    assert!(
        freshness.contains("HEAD") && freshness.contains("unresolved refs"),
        "FR-RC-03 freshness line: {freshness}"
    );
    assert!(json["signal"].is_u64(), "a non-empty fixture has a signal");
}

#[test]
fn non_stub_subcommands_emit_valid_json_with_json_flag() {
    // The sprint test plan: "Pass --json on every subcommand and assert JSON
    // output." Since S-020 the quality commands are wired too: on a clean
    // fixture (no rules.toml, no baseline) every one of them succeeds.
    // `check` is exercised separately below: on this same contract-less
    // fixture it exits 4, not 0 (FR-GV-22), so it does not belong in this
    // uniform-exit-0 sweep.
    let tmp = fixture();
    for args in [
        &["index", "--json"] as &[&str],
        &["status", "--json"],
        &["affected", "src/core.rs", "--json"],
        &["languages", "--json"],
        &["scan", "--json"],
        &["gate", "--json"],
        &["doctor", "--json"],
        &["verify", "--json"],
        &["evolution", "--json"],
        &["dsm", "--json"],
        &["hotspots", "--json"],
        &["hotspots", "--untested", "--json"],
        &["hotspots", "--production-scope", "--json"],
        &["coverage", "status", "--json"],
    ] {
        let out = logos(tmp.path(), args);
        assert_eq!(
            exit_code(&out),
            0,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).is_ok(),
            "`logos {}` stdout is one JSON document",
            args.join(" ")
        );
    }
}

// ── CR-052 / FR-GV-19: `verify` deep consistency check ─────────────────────

#[test]
fn verify_exits_1_on_an_orphan_file_leak() {
    // FR-GV-19 end-to-end through the binary: index the fixture, delete a file
    // on disk WITHOUT a sync (so the live store retains its nodes — a Channel-B
    // orphan leak), then `verify`. A fresh shadow reindex sees the smaller tree,
    // so the live graph has surplus nodes → drift → exit 1, exactly as CI sees it.
    let tmp = fixture();
    assert_eq!(exit_code(&logos(tmp.path(), &["index"])), 0);

    // A clean, freshly-indexed graph matches a fresh reindex → exit 0.
    let clean = logos(tmp.path(), &["verify", "--json"]);
    assert_eq!(
        exit_code(&clean),
        0,
        "clean graph verifies ok: {}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(report["ok"], serde_json::json!(true));
    assert_eq!(report["node_delta"], serde_json::json!(0));

    // Delete a leaf file on disk; no sync, so the live store keeps its nodes.
    fs::remove_file(tmp.path().join("src/top.rs")).unwrap();

    let drifted = logos(tmp.path(), &["verify", "--json"]);
    assert_eq!(
        exit_code(&drifted),
        1,
        "an orphan-file leak drifts → exit 1: {}",
        String::from_utf8_lossy(&drifted.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&drifted.stdout).unwrap();
    assert_eq!(report["ok"], serde_json::json!(false));
    assert!(
        report["node_delta"].as_i64().unwrap() > 0,
        "the live graph has surplus nodes vs a fresh reindex: {report}"
    );
    assert!(
        report["leaked_total"].as_u64().unwrap() > 0
            && report["leaked_symbols"]
                .as_array()
                .is_some_and(|s| !s.is_empty()),
        "the leaked (live-only) symbols of the deleted file are reported: {report}"
    );
}

// ── FR-GH-08 / UAT-GH-04: `hotspots` degrades when `git` is absent ─────────

/// The third degraded mode end-to-end through the shipped binary: with `git`
/// absent from `PATH`, `logos hotspots` reports `n/a` + a notice and exits 0 —
/// never an error, never a fabricated number ([FR-GH-08], [UAT-GH-04]). Run as
/// a subprocess (the binary launches by absolute path, so a git-free `PATH`
/// only starves its internal `git` calls), avoiding any racy in-process env
/// mutation.
#[test]
fn hotspots_degrades_to_na_when_git_is_absent() {
    let tmp = fixture();
    // Build the index with `git` available; the degrade is about the temporal
    // read, which needs an existing index.
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    // Re-run hotspots with a PATH that contains no `git` binary (the temp dir
    // itself) — the miner resolves HEAD via `git` and degrades to `GitAbsent`.
    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(tmp.path())
        .args(["hotspots", "--json"])
        .env("PATH", tmp.path())
        .output()
        .expect("the logos binary runs");
    assert_eq!(
        exit_code(&out),
        0,
        "a degraded temporal tier is exit 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(
        json["degraded"], "GitAbsent",
        "git absent → the GitAbsent degrade ({json})"
    );
    assert_eq!(
        json["files"].as_array().map(Vec::len),
        Some(0),
        "no fabricated hotspots when git is absent"
    );
    assert!(
        json["notice"].as_str().is_some_and(|n| !n.is_empty()),
        "a one-line notice explains the git-absent degrade"
    );
}

// ── FR-GH-06 / CR-076: `--production-scope` end-to-end through the binary ───

/// Commit `rel` with `contents` at HEAD — a small git-history fixture builder
/// local to this file (the other integration-test crates have their own).
fn git_commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    write(cwd, rel, contents);
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// `logos hotspots --production-scope` drops a whole test file (`src/tests.rs`,
/// the bare-name convention) from the candidate set before ranking, while the
/// default (flag omitted) board is unchanged ([CR-076], [FR-GH-06]).
#[test]
fn hotspots_production_scope_flag_excludes_whole_test_files() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    let branchy = |ifs: usize| -> String {
        let body: String = (0..ifs)
            .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
            .collect();
        format!("pub fn f(x: i64) -> i64 {{\n{body}    x\n}}\n")
    };

    // tests.rs: the hottest file overall, but a whole test file by path.
    for n in 0..4 {
        git_commit(
            repo,
            "src/tests.rs",
            &format!("{}// rev {n}\n", branchy(8)),
            &format!("tests v{n}"),
        );
    }
    // hot.rs: the hottest production file.
    for n in 0..2 {
        git_commit(
            repo,
            "src/hot.rs",
            &format!("{}// rev {n}\n", branchy(4)),
            &format!("hot v{n}"),
        );
    }

    assert_eq!(exit_code(&logos(repo, &["index", "--quiet"])), 0);

    let off = logos(repo, &["hotspots", "--json"]);
    assert_eq!(exit_code(&off), 0);
    let off_json: serde_json::Value = serde_json::from_slice(&off.stdout).expect("valid JSON");
    assert_eq!(off_json["production_scope"], false);
    assert_eq!(
        off_json["files"][0]["path"].as_str(),
        Some("src/tests.rs"),
        "filter off: the whole-repo board is unchanged: {off_json}"
    );

    let on = logos(repo, &["hotspots", "--production-scope", "--json"]);
    assert_eq!(exit_code(&on), 0);
    let on_json: serde_json::Value = serde_json::from_slice(&on.stdout).expect("valid JSON");
    assert_eq!(on_json["production_scope"], true);
    let paths: Vec<&str> = on_json["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(
        !paths.contains(&"src/tests.rs"),
        "--production-scope drops the whole test file: {paths:?}"
    );
    assert_eq!(
        paths,
        ["src/hot.rs"],
        "only the production file remains: {paths:?}"
    );
}

// ── FR-IN-01 (minimal contract): `init` bootstraps the store ───────────────

/// The minimal `init` contract ahead of the full S-023 experience: create
/// `.logos/` + the canonical DB, exit 0; idempotent and non-clobbering on an
/// already-initialised (and even already-indexed) project.
#[test]
fn init_bootstraps_the_store_idempotently_without_clobbering() {
    let tmp = fixture();

    // First run: creates .logos/logos.db from nothing.
    let out = logos(tmp.path(), &["init", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert!(json["db_path"].as_str().is_some(), "reports the db path");
    assert!(tmp.path().join(".logos/logos.db").is_file());

    // Index, then re-run init: the graph must survive (non-clobbering).
    assert_eq!(exit_code(&logos(tmp.path(), &["index"])), 0);
    let before = fs::metadata(tmp.path().join(".logos/logos.db"))
        .unwrap()
        .len();
    assert_eq!(exit_code(&logos(tmp.path(), &["init"])), 0, "idempotent");
    let out = logos(tmp.path(), &["search", "base", "--json"]);
    assert_eq!(exit_code(&out), 0);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert!(
        json["hits"].as_array().is_some_and(|h| !h.is_empty()),
        "indexed data survives a re-init: {json}"
    );
    let after = fs::metadata(tmp.path().join(".logos/logos.db"))
        .unwrap()
        .len();
    assert_eq!(before, after, "re-init must not rewrite the store");
}

// ── FR-IN-02/03 (S-023): `init -i` / `init --hooks` through the binary ─────

/// Non-TTY `init -i` takes the safe defaults: MCP block + managed CLAUDE.md
/// = yes, git hooks = no (hooks stay opt-in via `--hooks`, FR-IN-03). The
/// test harness pipes stdin, so this IS the non-TTY path — no prompt may
/// block, nothing may land on stdout besides the JSON read-model.
#[test]
fn interactive_init_non_tty_applies_safe_defaults() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["init", "-i", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let targets: Vec<&str> = json["steps"]
        .as_array()
        .expect("steps array")
        .iter()
        .filter_map(|s| s["target"].as_str())
        .collect();
    assert!(targets.contains(&".mcp.json"), "MCP step ran: {targets:?}");
    assert!(
        targets.contains(&"CLAUDE.md"),
        "CLAUDE.md step ran: {targets:?}"
    );
    // The wiki-skill materialization is also a non-TTY `-i` default (FR-WK-08):
    // confirm the CLI's init_options wiring enables it through the binary.
    assert!(
        targets.contains(&".agents/skills/logos-wiki"),
        "wiki skill step ran: {targets:?}"
    );
    assert!(
        !targets.contains(&".logos/hooks"),
        "hooks stay opt-in on non-TTY -i: {targets:?}"
    );
    assert!(tmp.path().join(".mcp.json").is_file());
    assert!(tmp.path().join("CLAUDE.md").is_file());
    assert!(
        tmp.path()
            .join(".agents/skills/logos-wiki/SKILL.md")
            .is_file(),
        "wiki skill materialized on non-TTY -i"
    );
    assert!(!tmp.path().join(".logos/hooks").exists());
}

/// `init --hooks` (no `-i`) installs only the hooks extra (FR-IN-03): in a
/// git repo, core.hooksPath is wired and the sync hooks exist.
#[test]
fn init_hooks_flag_installs_hooks_in_a_git_repo() {
    let tmp = TempDir::new().unwrap();
    assert!(Command::new("git")
        .args(["-C", tmp.path().to_str().unwrap(), "init", "-q"])
        .status()
        .expect("git available")
        .success());

    let out = logos(tmp.path(), &["init", "--hooks", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(tmp.path().join(".logos/hooks/post-commit").is_file());
    let hooks_path = Command::new("git")
        .args([
            "-C",
            tmp.path().to_str().unwrap(),
            "config",
            "core.hooksPath",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&hooks_path.stdout).trim(),
        ".logos/hooks"
    );
    // No -i: the host-integration steps must NOT run.
    assert!(!tmp.path().join(".mcp.json").exists());
    assert!(!tmp.path().join("CLAUDE.md").exists());
}

// ── FR-CL-04 / UAT-CL-03: `affected` through the binary ────────────────────

#[test]
fn affected_returns_the_reverse_transitive_closure_as_json() {
    let tmp = fixture();
    let indexed = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(
        exit_code(&indexed),
        0,
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let out = logos(tmp.path(), &["affected", "src/core.rs", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    let files: Vec<&str> = json["affected"]
        .as_array()
        .expect("affected array")
        .iter()
        .map(|f| f["file"].as_str().unwrap())
        .collect();
    assert!(files.contains(&"src/mid.rs"), "direct dependent: {files:?}");
    assert!(
        files.contains(&"src/top.rs"),
        "transitive dependent: {files:?}"
    );
}

// ── FR-MC-01: `serve --mcp` serves MCP over stdio through this binary ───────

/// The S-016 → S-017 wiring seam: the shipped `logos` binary speaks MCP on
/// stdout (every line JSON-RPC framing, NFR-RA-01) with trace logging live
/// through the S-019 observability stack, and a host disconnect (stdin EOF)
/// exits 0 with no orphaned process (FR-MC-06, NFR-RA-12).
#[test]
fn serve_mcp_speaks_jsonrpc_on_stdout_and_exits_cleanly_on_disconnect() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let tmp = TempDir::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(tmp.path())
        .args(["serve", "--mcp"])
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn logos serve --mcp");
    let mut stdin = child.stdin.take().expect("stdin");

    // Drain stdout on a thread (a full pipe would block the server and fake
    // a hang), asserting EVERY line is JSON-RPC framing (NFR-RA-01).
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(&line).unwrap_or_else(|e| {
                panic!("non-JSON-RPC bytes on stdout (NFR-RA-01): {e}\nline: {line}")
            });
            assert_eq!(value["jsonrpc"], "2.0", "stdout framing: {line}");
            if tx.send(value).is_err() {
                break;
            }
        }
    });

    for message in [
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "story-review", "version": "0"}
            }
        }),
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ] {
        writeln!(stdin, "{message}").expect("write request");
    }
    stdin.flush().expect("flush");

    let (mut init, mut tools) = (None, None);
    while init.is_none() || tools.is_none() {
        let value = rx
            .recv_timeout(Duration::from_secs(60))
            .expect("response before timeout");
        match value["id"].as_u64() {
            Some(1) => init = Some(value),
            Some(2) => tools = Some(value),
            _ => {}
        }
    }
    assert_eq!(
        init.unwrap()["result"]["serverInfo"]["name"],
        "logos",
        "the host derives logos:<tool> from this identity (FR-MC-01)"
    );
    assert_eq!(
        tools.unwrap()["result"]["tools"]
            .as_array()
            .expect("tools array")
            .len(),
        30,
        "all 30 tools register through the shipped binary (FR-MC-01)"
    );
    // The roster this count tracks is declared as a LIST in two places, and a
    // tool added to only one surface is named for you by the guards there
    // rather than reduced to an off-by-one here: `mcp/tests/support/roster.rs`
    // (the MCP names) and `cli/src/main.rs::surface_parity` (their CLI twins,
    // FR-CL-06). Neither is reachable from this crate's integration tests — the
    // first is another crate's test module, the second is inside the binary —
    // so this number stays hand-written. Update it there first.

    // Host disconnect → the process winds down by itself with exit 0.
    drop(stdin);
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("server did not exit after host disconnect (NFR-RA-12)");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "clean exit on disconnect, got {status}");
}

// ── FR-NV-11 / CR-114: impact-set intersection across planned work items ────

/// The scheduling query, end-to-end through the shipped binary in `--json`
/// mode (the acceptance criterion "ships with a CLI command carrying `--json`").
///
/// The fixture chain is `top` -> `mid` -> `base`, so an item intending to change
/// `top` and one intending to change `base` are NOT independent: `base` is in
/// both impact sets. That is the collision a planner otherwise infers from
/// architecture prose.
#[test]
fn impact_intersection_reports_the_collision_through_the_binary() {
    let tmp = fixture();
    logos(tmp.path(), &["index"]);

    let out = logos(
        tmp.path(),
        &[
            "impact-intersection",
            "--item",
            "S-1=top",
            "--item",
            "S-2=base",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits one machine-readable object");

    // The pair is reported as intersecting, and the shared symbols are named.
    let intersecting = payload["intersecting"].as_array().expect("intersecting");
    assert_eq!(intersecting.len(), 1, "{payload}");
    assert_eq!(intersecting[0]["left"], "S-1");
    assert_eq!(intersecting[0]["right"], "S-2");
    let shared: Vec<&str> = intersecting[0]["shared"]
        .as_array()
        .expect("shared symbols")
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(shared.contains(&"base"), "{shared:?}");
    assert!(
        payload["safe_parallel"].as_array().unwrap().is_empty(),
        "a colliding pair is never also reported safely parallel: {payload}"
    );

    // NFR-CC-04: the verdict ships with the limits of the graph it was computed
    // over, so a `safe_parallel` answer is never read as ground truth.
    let statement = payload["coverage"]["statement"]
        .as_str()
        .expect("the coverage statement rides the payload");
    assert!(statement.contains("indexed code graph"), "{statement}");
    assert_eq!(payload["depth"], 3, "the default depth is reported");

    // `--depth` reaches the engine through the binary: at depth 0 only directly
    // declared symbols can collide, so the transitive S-1/S-2 overlap vanishes.
    // The parity test compares read-models, not argv, so this is the only place
    // a dropped or mis-plumbed CLI flag would be caught.
    let out = logos(
        tmp.path(),
        &[
            "impact-intersection",
            "--item",
            "S-1=top",
            "--item",
            "S-2=base",
            "--depth",
            "0",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0);
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["depth"], 0, "--depth reaches the engine: {payload}");
    assert!(
        payload["intersecting"].as_array().unwrap().is_empty(),
        "at depth 0 the transitive collision is gone: {payload}"
    );
    assert_eq!(payload["safe_parallel"].as_array().unwrap().len(), 1);

    // An unknown symbol is a coverage limit on an exit-0 payload, never an error.
    let out = logos(
        tmp.path(),
        &[
            "impact-intersection",
            "--item",
            "S-1=top",
            "--item",
            "S-2=no_such_symbol",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0, "an unknown symbol is not a fault");
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["coverage"]["unresolved"][0]["item"], "S-2", "{payload}");
    assert_eq!(
        payload["coverage"]["unresolved"][0]["symbol"], "no_such_symbol",
        "{payload}"
    );
    assert_eq!(
        payload["coverage"]["items_without_resolved_symbols"],
        serde_json::json!(["S-2"]),
        "the item resting on nothing is named, so `safe_parallel` is not read as \
         evidence of independence: {payload}"
    );

    // A malformed spec is warned about, not silently dropped, and never an error.
    let out = logos(
        tmp.path(),
        &[
            "impact-intersection",
            "--item",
            "S-1=top",
            "--item",
            "no-equals-sign",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0, "a malformed item is not a usage fault");
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let warnings = payload["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or_default().contains("no-equals-sign")),
        "{warnings:?}"
    );

    // `--item` is required: an intersection over nothing is a usage fault (2).
    let out = logos(tmp.path(), &["impact-intersection"]);
    assert_eq!(exit_code(&out), 2, "clap requires at least one --item");
}

/// A language-plugin registry fixture: two capability arms behind one trait,
/// through the same two helpers, plus a symbol attached to nothing.
fn precedent_fixture() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/plugin.rs",
        "pub trait LanguagePlugin {\n    fn extract(&self);\n}\n",
    );
    write(
        tmp.path(),
        "src/shared.rs",
        "pub fn parse_source() {}\npub fn emit_facts() {}\n",
    );
    for (file, ty) in [("src/rustarm.rs", "RustArm"), ("src/pyarm.rs", "PyArm")] {
        write(
            tmp.path(),
            file,
            &format!(
                "use crate::plugin::LanguagePlugin;\n\
                 use crate::shared::{{parse_source, emit_facts}};\n\n\
                 pub struct {ty};\n\n\
                 impl LanguagePlugin for {ty} {{\n\
                 \x20   fn extract(&self) {{\n\
                 \x20       parse_source();\n\
                 \x20       emit_facts();\n\
                 \x20   }}\n\
                 }}\n\n\
                 pub fn {init}_init() {{}}\n",
                init = file.trim_start_matches("src/").trim_end_matches(".rs")
            ),
        );
    }
    // A dispatcher that CALLS each arm's init, so the file target finds a
    // second, lower-ranked precedent (the init sibling, one registration facet)
    // behind the method sibling (two facets). A module `use` list is
    // deliberately not a registration — see `is_registration_edge`.
    write(
        tmp.path(),
        "src/registry.rs",
        "use crate::rustarm::rustarm_init;\n\
         use crate::pyarm::pyarm_init;\n\n\
         pub fn register_all() {\n\
         \x20   rustarm_init();\n\
         \x20   pyarm_init();\n\
         }\n",
    );
    write(tmp.path(), "src/lonely.rs", "pub fn lonely_helper() {}\n");
    tmp
}

/// The structural-precedent query, end-to-end through the shipped binary in
/// `--json` mode (the acceptance criterion "ships with a CLI command carrying
/// `--json` … in the same increment").
///
/// The parity test compares read-models, not argv, so this is the only place a
/// dropped positional or a mis-plumbed `--limit` would be caught.
#[test]
fn precedent_reports_the_sibling_arms_through_the_binary() {
    let tmp = precedent_fixture();
    logos(tmp.path(), &["index"]);

    let out = logos(
        tmp.path(),
        &["precedent", "logos . . . src/`rustarm.rs`/extract().", "--json"],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits one machine-readable object");

    // The sibling arm comes back, and it says WHY (FR-NV-12 AC 2/AC 3).
    let precedents = payload["precedents"].as_array().expect("precedents");
    assert_eq!(precedents.len(), 1, "{payload}");
    assert_eq!(precedents[0]["file"], "src/pyarm.rs", "{payload}");
    let facets: Vec<&str> = precedents[0]["reasons"]
        .as_array()
        .expect("reasons")
        .iter()
        .map(|r| r["facet"].as_str().unwrap())
        .collect();
    assert_eq!(facets, vec!["shared_supertype", "shared_callee"], "{payload}");
    // The notion and the ranking rule the order rests on ride the payload, and
    // the rank is counted facts rather than a score (AC 1).
    assert!(payload["notion"].as_str().unwrap().contains("never a score"));
    assert!(payload["ranked_by"].as_str().unwrap().contains("lexicographically"));
    assert_eq!(precedents[0]["rank"]["facets"], 2, "{payload}");

    // `--limit` reaches the engine through the binary, and the remainder is
    // counted rather than silently dropped.
    let out = logos(tmp.path(), &["precedent", "src/rustarm.rs", "--limit", "1", "--json"]);
    assert_eq!(exit_code(&out), 0);
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["target_kind"], "file", "a file target resolves: {payload}");
    assert_eq!(payload["precedents"].as_array().unwrap().len(), 1);
    assert!(
        payload["total_found"].as_u64().unwrap() > 1 && payload["elided"].as_u64().unwrap() >= 1,
        "--limit reaches the engine and the remainder is counted: {payload}"
    );

    // An unknown target is an honest empty answer naming its reason at exit 0,
    // never an error (FR-NV-12 AC 4, ADR-14).
    let out = logos(tmp.path(), &["precedent", "no_such_thing", "--json"]);
    assert_eq!(exit_code(&out), 0, "an unknown target is not a fault");
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert!(payload["precedents"].as_array().unwrap().is_empty());
    assert_eq!(payload["empty_reason"]["code"], "target_unresolved", "{payload}");

    // A resolved symbol with nothing to compare on says exactly that, rather
    // than reaching for a looser notion.
    let out = logos(tmp.path(), &["precedent", "lonely_helper", "--json"]);
    assert_eq!(exit_code(&out), 0);
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["empty_reason"]["code"], "no_structural_anchors", "{payload}");

    // The target is required: a precedent query over nothing is a usage fault.
    let out = logos(tmp.path(), &["precedent"]);
    assert_eq!(exit_code(&out), 2, "clap requires the target");
}

// ── FR-CL-02: --quiet suppresses human output, never the JSON ──────────────

#[test]
fn quiet_suppresses_human_output_but_not_json() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);

    let human = logos(tmp.path(), &["status"]);
    assert!(!human.stdout.is_empty(), "human mode prints the read-model");

    let quiet = logos(tmp.path(), &["status", "--quiet"]);
    assert_eq!(exit_code(&quiet), 0);
    assert!(
        quiet.stdout.is_empty(),
        "--quiet silences the human rendering"
    );

    let quiet_json = logos(tmp.path(), &["status", "--quiet", "--json"]);
    assert!(
        !quiet_json.stdout.is_empty(),
        "machine output is essential output — --json always emits"
    );
}

// ── S-021 / FR-WT-01..04: worktree-aware operation through the binary ───────

/// Run a git command in `cwd`, panicking on failure — fixtures only.
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

/// FR-WT-01/03 + UAT-WT-01 end-to-end: after indexing the primary checkout,
/// a graph command in a freshly-added DB-less worktree is NOT refused with
/// "run `logos index`" — the engine seeds from the primary and serves — and
/// the worktree ends up owning its own DB.
#[test]
fn a_db_less_worktree_seeds_instead_of_refusing() {
    let tmp = TempDir::new().unwrap();
    let main = tmp.path().join("main");
    write(&main, "src/lib.rs", "pub fn seeded_fn() {}\n");
    write(&main, ".gitignore", ".logos/*.db\n.logos/*.db-*\n");
    sh_git(&main, &["init", "-q", "-b", "main"]);
    sh_git(&main, &["add", "."]);
    sh_git(&main, &["commit", "-q", "-m", "initial"]);

    let indexed = logos(&main, &["--json", "index"]);
    assert_eq!(exit_code(&indexed), 0, "primary index succeeds");

    let wt = tmp.path().join("wt");
    sh_git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    assert!(
        !wt.join(".logos/logos.db").exists(),
        "the derived DB does not travel through git (NFR-DM-04)"
    );

    let out = logos(&wt, &["--json", "status"]);
    assert_eq!(
        exit_code(&out),
        0,
        "a seedable worktree is served, not refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        wt.join(".logos/logos.db").exists(),
        "first use seeded the worktree's own DB (FR-WT-01/03)"
    );
}

// ── FR-WK-08: the embedded wiki-generation skill through the binary ─────────

/// `logos wiki skill --emit` materializes the canonical layout (exit 0) and
/// `--json` emits the machine-readable summary; an unforced re-emit skips,
/// `--force` restores (UAT-WK-04). Run through the real binary so the offline
/// path (NFR-SE-01) and exit codes are exactly as CI sees them.
#[test]
fn wiki_skill_emit_materializes_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();

    let out = logos(tmp.path(), &["--json", "wiki", "skill", "--emit"]);
    assert_eq!(
        exit_code(&out),
        0,
        "emit succeeds: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let body = String::from_utf8_lossy(&out.stdout);
    assert!(body.contains("\"action\":\"created\""), "json: {body}");
    let skill_file = tmp.path().join(".agents/skills/logos-wiki/SKILL.md");
    assert!(skill_file.exists(), "canonical dir written");
    assert!(
        tmp.path().join(".claude/skills/logos-wiki").exists(),
        ".claude pointer written"
    );
    // Capture the materialized content so the --force restore can be checked
    // positively (byte-equality), not merely as "no longer the local edit".
    let embedded = fs::read_to_string(&skill_file).unwrap();

    // A local edit survives an unforced re-emit (skip-if-present).
    fs::write(&skill_file, "LOCAL EDIT").unwrap();
    let again = logos(tmp.path(), &["--json", "wiki", "skill", "--emit"]);
    assert_eq!(exit_code(&again), 0);
    assert!(String::from_utf8_lossy(&again.stdout).contains("\"action\":\"skipped\""));
    assert_eq!(fs::read_to_string(&skill_file).unwrap(), "LOCAL EDIT");

    // --force restores the embedded content byte-for-byte.
    let forced = logos(
        tmp.path(),
        &["--json", "wiki", "skill", "--emit", "--force"],
    );
    assert_eq!(exit_code(&forced), 0);
    assert!(String::from_utf8_lossy(&forced.stdout).contains("\"action\":\"forced\""));
    assert_eq!(
        fs::read_to_string(&skill_file).unwrap(),
        embedded,
        "--force restores the embedded content through the binary"
    );
}

/// `wiki skill` without `--emit` is a clap usage error (exit 2): `--emit` is
/// required so the verb reads `wiki skill --emit` (FR-CL-03).
#[test]
fn wiki_skill_requires_emit() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["wiki", "skill"]);
    assert_eq!(exit_code(&out), 2, "missing --emit is a usage error");
}

// ── FR-WK-13: `logos wiki generate` offline generation queue ────────────────

/// `wiki generate` formats the `wiki status` work-list into a deterministic
/// generation queue through the real binary: the default is a human prompt
/// block, `--json` is a byte-identical machine queue, the run writes no wiki
/// page (a pure read), and the queue carries the agent-tier sections with
/// runnable `wiki write` skeletons — never any native-tier (Configuration)
/// content (FR-WK-13, NFR-RA-06, NFR-SE-01).
#[test]
fn wiki_generate_emits_a_deterministic_offline_queue() {
    let tmp = fixture();
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    // Default: a non-empty human prompt block with runnable skeletons, exit 0.
    let human = logos(tmp.path(), &["wiki", "generate"]);
    assert_eq!(
        exit_code(&human),
        0,
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let block = String::from_utf8_lossy(&human.stdout);
    assert!(
        block.contains("Wiki generation queue —") && block.contains("logos wiki write "),
        "the prompt block lists runnable `wiki write` skeletons: {block}"
    );

    // `--json`: a machine queue, exit 0, byte-identical across two runs.
    let j1 = logos(tmp.path(), &["wiki", "generate", "--json"]);
    let j2 = logos(tmp.path(), &["wiki", "generate", "--json"]);
    assert_eq!(exit_code(&j1), 0, "{}", String::from_utf8_lossy(&j1.stderr));
    assert_eq!(
        j1.stdout, j2.stdout,
        "the --json queue is byte-identical for a fixed wiki.db + revision"
    );
    let json: serde_json::Value = serde_json::from_slice(&j1.stdout).expect("one JSON document");
    let items = json["items"].as_array().expect("items array");
    assert!(
        !items.is_empty(),
        "an indexed project has agent-tier sections to generate"
    );
    // The six Overview children lead the queue, each with a runnable skeleton
    // and target slug; the native-tier Configuration section never appears.
    assert!(
        items
            .iter()
            .any(|i| i["slug"] == "overview/project-overview"),
        "the Overview sections are queued: {json}"
    );
    assert!(
        items.iter().all(|i| {
            let cmd = i["command"].as_str().unwrap_or_default();
            let slug = i["slug"].as_str().unwrap_or_default();
            cmd.starts_with("logos wiki write ") && !slug.is_empty()
        }),
        "every item carries its target slug and a runnable skeleton: {json}"
    );
    assert!(
        items.iter().all(|i| {
            let slug = i["slug"].as_str().unwrap_or_default();
            !slug.contains("config")
        }),
        "no native-tier Configuration content is ever queued: {json}"
    );

    // The run is a pure read: no wiki page was written (the store enumerates empty).
    let list = logos(tmp.path(), &["wiki", "search", "--list", "--json"]);
    assert_eq!(exit_code(&list), 0);
    let pages: serde_json::Value = serde_json::from_slice(&list.stdout).expect("JSON");
    assert_eq!(
        pages.as_array().map(Vec::len),
        Some(0),
        "wiki generate wrote no page — it is a pure read of the work-list"
    );
}

/// AC3 end-to-end through the binary: an empty work-list yields the explicit
/// "nothing to generate" result and exit 0, never a fabricated queue (FR-WK-13).
/// An empty project still surfaces the five Overview sections after `index`, so
/// the empty path is reached by writing those five (zero-anchor) pages at the
/// current revision — they then drop off the work-list, leaving nothing to do.
#[test]
fn wiki_generate_on_an_empty_work_list_reports_nothing_to_generate() {
    // An empty project (no source files) → after index, the only work is the
    // five Overview sections; no per-file objectives, no unanchored entities.
    // (The synthesized overview/architecture page is retired by CR-062.)
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    for (slug, title) in [
        ("overview/project-overview", "Project Overview"),
        ("overview/getting-started", "Getting Started"),
        ("overview/key-concepts", "Key concepts"),
        ("overview/how-it-works", "How It Works"),
        ("overview/known-issues", "Known issues"),
    ] {
        // Zero-anchor overview pages, written at the current graph revision, so
        // they are neither absent nor revision-stale → off the work-list.
        let out = logos(
            tmp.path(),
            &[
                "wiki", "write", slug, "--title", title, "--generator", "test",
                    "# Page\n\nPlaceholder prose long enough to clear the write-path guard.",
            ],
        );
        assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    }

    // Default human path: the explicit sentinel, exit 0.
    let human = logos(tmp.path(), &["wiki", "generate"]);
    assert_eq!(exit_code(&human), 0);
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("Nothing to generate"),
        "empty work-list → explicit 'nothing to generate': {}",
        String::from_utf8_lossy(&human.stdout)
    );

    // Machine path: an empty items array, exit 0, no fabricated queue.
    let json = logos(tmp.path(), &["wiki", "generate", "--json"]);
    assert_eq!(exit_code(&json), 0);
    let queue: serde_json::Value = serde_json::from_slice(&json.stdout).expect("JSON");
    assert_eq!(
        queue["items"].as_array().map(Vec::len),
        Some(0),
        "empty work-list → empty items array, never fabricated: {queue}"
    );
}

// ── FR-WK-20 / FR-WK-09: `logos wiki materialize` deterministic presented tier ──

/// In SRS mode (Case 1), `logos wiki materialize` presents the Architecture
/// page and the present Design/Specs categories from `docs/specs/**` into
/// `wiki.db` — a pure deterministic write, no LLM/network — and re-running is
/// byte-identical (FR-WK-20, NFR-SE-01).
#[test]
fn wiki_materialize_presents_design_specs_pages_in_srs_mode() {
    let tmp = fixture();
    write(
        tmp.path(),
        "docs/specs/architecture.md",
        "# Architecture\n\nThe system design.\n",
    );
    write(
        tmp.path(),
        "docs/specs/requirements/FR-X-01.md",
        "# FR-X-01\n\nA requirement.\n",
    );
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    let out = logos(tmp.path(), &["--json", "wiki", "materialize"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON document");
    assert_eq!(summary["srs_mode"], true, "architecture.md + a requirement → Case 1");
    assert_eq!(
        summary["materialized"],
        serde_json::json!(["overview/architecture", "specs/functional-requirements"]),
    );

    // The presented page reads with doc-present provenance, never the
    // generated-content marker.
    let read = logos(tmp.path(), &["--json", "wiki", "read", "overview/architecture"]);
    assert_eq!(exit_code(&read), 0);
    let page: serde_json::Value = serde_json::from_slice(&read.stdout).expect("JSON");
    assert_eq!(page["generator"], "logos:doc-present");

    // Re-running is byte-identical (idempotent, no new work, nothing pruned).
    let again = logos(tmp.path(), &["--json", "wiki", "materialize"]);
    assert_eq!(exit_code(&again), 0);
    assert_eq!(
        out.stdout, again.stdout,
        "wiki materialize is byte-identical on re-run over an unchanged tree"
    );
}

/// Outside SRS mode (Case 2, no `docs/specs/architecture.md`), `wiki
/// materialize` is a no-op: the empty summary, no `wiki.db` write (FR-WK-21).
#[test]
fn wiki_materialize_is_a_no_op_outside_srs_mode() {
    let tmp = fixture();
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    let out = logos(tmp.path(), &["--json", "wiki", "materialize"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    assert_eq!(
        summary,
        serde_json::json!({"srs_mode": false, "materialized": [], "pruned": []}),
        "Case 2 is the empty summary — no LLM/network, no wiki.db write"
    );

    let list = logos(tmp.path(), &["wiki", "search", "--list", "--json"]);
    assert_eq!(exit_code(&list), 0);
    let pages: serde_json::Value = serde_json::from_slice(&list.stdout).expect("JSON");
    assert_eq!(
        pages.as_array().map(Vec::len),
        Some(0),
        "materialize wrote no page outside SRS mode"
    );
}

// ── FR-WK-19: content-validity guard on the CLI `wiki write` stdin path ──────

/// Run `logos wiki write <args> --body-file -` with `body` piped on stdin — the
/// external-agent write surface ([FR-WK-19]).
fn wiki_write_via_stdin(project: &Path, slug: &str, title: &str, body: &str) -> Output {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_logos"))
        .arg("--project")
        .arg(project)
        .args(["wiki", "write", slug, "--title", title, "--generator", "test", "--body-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the logos binary runs");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write body to stdin");
    child.wait_with_output().expect("the process exits")
}

/// A garbage body (a `<tool_call>` dump) piped via stdin is rejected with a
/// non-zero exit and leaves no page behind — the CLI `wiki write` stdin path is
/// the same guard the in-process run goes through ([FR-WK-19]).
#[test]
fn wiki_write_rejects_a_garbage_body_piped_via_stdin() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    let out = wiki_write_via_stdin(
        tmp.path(),
        "guide/noise",
        "Noise",
        "<tool_call>\n{\"name\": \"read_file\"}\n</tool_call>",
    );
    assert_ne!(exit_code(&out), 0, "a tool-call dump body is rejected");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("FR-WK-19"),
        "the rejection names the content-validity guard: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let read = logos(tmp.path(), &["--json", "wiki", "read", "guide/noise"]);
    assert_eq!(exit_code(&read), 0);
    assert_eq!(
        String::from_utf8_lossy(&read.stdout).trim(),
        "null",
        "the rejected write left no page to read"
    );
}

/// A well-formed body piped via the same stdin path is stored — the guard
/// rejects agent-noise, not the stdin surface itself ([FR-WK-19]). The
/// read-back body is compared byte-for-byte, not just checked for presence,
/// so a stdin-path bug that truncated or mangled the piped body would fail
/// this test rather than slip through on a bare exit-code/null check.
#[test]
fn wiki_write_accepts_a_well_formed_body_piped_via_stdin() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["index", "--quiet"])), 0);

    let body = "# OK\n\nPlaceholder prose long enough to satisfy the write-path content-validity guard.";
    let out = wiki_write_via_stdin(tmp.path(), "guide/ok", "OK", body);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let read = logos(tmp.path(), &["--json", "wiki", "read", "guide/ok"]);
    assert_eq!(exit_code(&read), 0, "the accepted write is readable");
    let page: serde_json::Value = serde_json::from_slice(&read.stdout).expect("JSON");
    assert_eq!(
        page["body"].as_str(),
        Some(body),
        "the read-back body is byte-identical to what was piped via stdin: {page}"
    );
}

// ── CR-070 / FR-WK-14 retirement: the PostToolUse augmentation hook is gone ──

/// `logos wiki hook --emit` materializes only the marker-tagged session-start
/// quality-report script + settings entry (exit 0), is idempotent on re-run,
/// `--force` re-emits without duplicating, and a foreign settings file is
/// never overwritten (FR-IN-07). Run through the real binary so the offline
/// path (NFR-SE-01) and exit codes are exactly as CI sees them.
#[test]
fn wiki_hook_emit_installs_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();

    let out = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(
        exit_code(&out),
        0,
        "emit succeeds: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("\"action\":\"created\""));
    assert!(
        tmp.path()
            .join(".claude/hooks/logos-quality-open.sh")
            .exists(),
        "script written"
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join(".claude/settings.json")).unwrap())
            .expect("settings JSON");
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().map(Vec::len),
        Some(1)
    );

    // Idempotent: an unforced re-emit skips, leaving the settings byte-identical.
    let before = fs::read(tmp.path().join(".claude/settings.json")).unwrap();
    let again = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&again), 0);
    assert!(String::from_utf8_lossy(&again.stdout).contains("\"action\":\"skipped\""));
    assert_eq!(
        fs::read(tmp.path().join(".claude/settings.json")).unwrap(),
        before,
        "an unforced re-emit is byte-identical"
    );

    // --force re-emits exactly one entry (no duplicate).
    let forced = logos(tmp.path(), &["--json", "wiki", "hook", "--emit", "--force"]);
    assert_eq!(exit_code(&forced), 0);
    assert!(String::from_utf8_lossy(&forced.stdout).contains("\"action\":\"forced\""));
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().map(Vec::len),
        Some(1),
        "force never duplicates the managed entry"
    );

    // A drifted managed entry is rewritten unforced, and the JSON says
    // `reconciled` — NOT `forced`, which promises local edits were overwritten
    // (CR-095 §3.5). Asserted through the binary because `action` is a
    // serialized contract shared with `wiki skill --emit`.
    let mut drifted = settings.clone();
    drifted["hooks"]["SessionStart"][0]["hooks"][0]["timeout"] = serde_json::json!(1);
    fs::write(
        tmp.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&drifted).unwrap(),
    )
    .unwrap();
    let healed = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&healed), 0);
    let out = String::from_utf8_lossy(&healed.stdout);
    assert!(out.contains("\"action\":\"reconciled\""), "{out}");
    assert!(
        !out.contains("\"action\":\"forced\""),
        "no --force was passed and no local edit was discarded: {out}"
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join(".claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(
        settings["hooks"]["SessionStart"][0]["hooks"][0]["timeout"], 30,
        "the drifted timeout is corrected"
    );
}

/// `wiki hook` without `--emit` is a clap usage error (exit 2).
#[test]
fn wiki_hook_requires_emit() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["wiki", "hook"])), 2);
}

/// [CR-070]/[CR-095]: `wiki hook --emit` materializes only the session-start
/// quality-report hook — still a single summary object, not an array, since
/// [CR-095] replaced one hook with one hook — and produces no augmentation
/// script or PostToolUse entry (the retired [FR-WK-14] hook), and no retired
/// SessionEnd quality-report entry.
#[test]
fn wiki_hook_emit_installs_only_the_quality_report_hook() {
    let tmp = TempDir::new().unwrap();

    let out = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("object JSON");
    assert!(summary.is_object(), "a single summary object, not an array: {summary}");
    assert_eq!(summary["action"], "created");
    assert_eq!(summary["settings"], ".claude/settings.json");

    // No augmentation script or PostToolUse entry anywhere.
    assert!(
        !tmp.path().join(".claude/hooks/logos-wiki-augment.sh").exists(),
        "the retired augmentation script is never materialized"
    );
    let quality_script = tmp.path().join(".claude/hooks/logos-quality-open.sh");
    assert!(quality_script.exists(), "the quality-report script is written");
    assert!(
        !tmp.path().join(".claude/hooks/logos-quality-report.sh").exists(),
        "the retired SessionEnd script is never materialized (CR-095)"
    );
    assert!(
        fs::read_to_string(&quality_script)
            .unwrap()
            .contains("logos:quality-report:managed"),
        "the quality-report script carries its marker"
    );
    let shared: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join(".claude/settings.json")).unwrap())
            .expect("settings.json");
    assert!(
        shared["hooks"]["PostToolUse"].is_null(),
        "no PostToolUse entry is installed: {shared}"
    );
    assert!(
        shared["hooks"]["SessionEnd"].is_null(),
        "no SessionEnd entry is installed (CR-095): {shared}"
    );
    let shared_start = shared["hooks"]["SessionStart"]
        .as_array()
        .expect("SessionStart array");
    assert_eq!(shared_start.len(), 1, "exactly one SessionStart entry");
    assert!(shared_start[0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .contains("logos-quality-open.sh"));
    assert_eq!(shared_start[0]["matcher"], "startup|resume|clear");
    assert!(
        shared_start[0]["hooks"][0]["timeout"].is_number(),
        "the entry declares its own timeout (CR-095): {shared}"
    );

    // No hook writes the per-developer settings.local.json — that file was
    // the retired autogen hook's alone (FR-WK-16 retirement).
    assert!(
        !tmp.path().join(".claude/settings.local.json").exists(),
        "no hook merges into the per-developer settings.local.json"
    );
}

/// [CR-047]/[CR-070] retirement regression: no artifact `wiki hook --emit` (or
/// `init -i`, covered separately in `logos-core/tests/init.rs`) materializes
/// references a `claude -p` invocation or the retired autogen hook script, no
/// augmentation script is written, and no `.claude/settings.local.json` is
/// ever written (FR-WK-14, FR-WK-16, NFR-SE-01).
#[test]
fn no_autogen_hook_or_claude_p_reference_remains() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["--json", "wiki", "hook", "--emit"])), 0);

    assert!(
        !tmp.path().join(".claude/hooks/logos-wiki-autogen.sh").exists(),
        "the retired autogen hook script is never materialized"
    );
    assert!(
        !tmp.path().join(".claude/hooks/logos-wiki-augment.sh").exists(),
        "the retired augmentation hook script is never materialized"
    );
    assert!(
        !tmp.path().join(".claude/settings.local.json").exists(),
        "no hook ever writes the per-developer settings.local.json"
    );
    let body = fs::read_to_string(tmp.path().join(".claude/hooks/logos-quality-open.sh")).unwrap();
    assert!(
        !body.contains("claude -p") && !body.contains("logos-wiki-autogen"),
        "the quality-report hook carries no claude -p invocation and no autogen reference"
    );
}

// ── CR-095 retirement: the SessionEnd quality-report hook is swept ───────────

/// [CR-095]: `wiki hook --emit` through the real binary sweeps a retired
/// SessionEnd quality-report entry and its orphaned script, reports the removal
/// in its `--json` summary, and leaves a foreign SessionEnd entry untouched.
/// Without the sweep an existing install keeps emitting `SessionEnd hook [...]
/// failed: Hook cancelled` on every exit and every `/clear`.
#[test]
fn wiki_hook_emit_sweeps_the_retired_session_end_hook() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join(".claude/hooks")).unwrap();
    let retired_script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
    // The managed marker licenses the delete — an unmarked file at that now-
    // unclaimed path belongs to the user and is kept instead.
    fs::write(
        &retired_script,
        "#!/bin/sh\n# logos:quality-report:managed — retired\nexit 0\n",
    )
    .unwrap();
    fs::write(
        tmp.path().join(".claude/settings.json"),
        r#"{"hooks":{"SessionEnd":[
            {"hooks":[{"type":"command","command":"${CLAUDE_PROJECT_DIR}/.claude/hooks/logos-quality-report.sh"}]},
            {"hooks":[{"type":"command","command":"their-cleanup.sh"}]}
        ]}}"#,
    )
    .unwrap();

    let out = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("object JSON");
    // Both halves of the sweep are reported: the deleted script and the removed
    // settings entry. Reporting only the file would stay silent where
    // `.claude/hooks/` is ignored but `settings.json` is committed.
    assert_eq!(
        summary["retired_removed"],
        serde_json::json!([
            ".claude/hooks/logos-quality-report.sh",
            ".claude/hooks/logos-quality-report.sh (settings entry)"
        ]),
        "the sweep is reported, not silent: {summary}"
    );
    assert_eq!(
        summary["retired_skipped"].as_array().map(Vec::len),
        Some(0),
        "nothing was left behind: {summary}"
    );
    assert!(!retired_script.exists(), "the orphaned retired script is deleted");

    let shared: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join(".claude/settings.json")).unwrap())
            .expect("settings.json");
    let end = shared["hooks"]["SessionEnd"].as_array().expect("SessionEnd array");
    assert_eq!(end.len(), 1, "only the foreign SessionEnd entry remains: {shared}");
    assert_eq!(end[0]["hooks"][0]["command"], "their-cleanup.sh");
    assert_eq!(
        shared["hooks"]["SessionStart"].as_array().map(Vec::len),
        Some(1),
        "the replacement is installed: {shared}"
    );

    // Idempotent: a second emit has nothing left to sweep and claims nothing.
    let again = logos(tmp.path(), &["--json", "wiki", "hook", "--emit"]);
    assert_eq!(exit_code(&again), 0);
    let summary: serde_json::Value = serde_json::from_slice(&again.stdout).expect("object JSON");
    assert_eq!(
        summary["retired_removed"].as_array().map(Vec::len),
        Some(0),
        "the sweep does not keep claiming a removal it already made: {summary}"
    );
}

// ── FR-NV-13 / CR-114: branch and merge symbol overlap ──────────────────────

/// A git fixture with two branches that both edit `mid`, plus a merge result
/// that carries only one of them.
///
/// `mid.rs` is deliberately multi-line so the innermost-symbol rule has real
/// nesting to resolve: the hunks land inside `mid`, not merely inside the file.
fn overlap_fixture() -> TempDir {
    let tmp = fixture();
    let root = tmp.path();
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.email=dev@logos",
                "-c",
                "user.name=Logos Dev",
                "-c",
                "commit.gpgsign=false",
                // See the note in logos-core/tests/branch_overlap.rs: a global
                // `core.hooksPath` would let a foreign pre-commit hook veto
                // these commits.
                "-c",
                "core.hooksPath=",
            ])
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let mid = |body: &str| format!("use crate::core::base;\npub fn mid() {{\n    {body}\n}}\n");
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["branch", "-M", "main"]);
    for (branch, body) in [("left", "base();\n    // left"), ("right", "base();\n    // right")] {
        git(&["checkout", "-q", "-b", branch, "main"]);
        write(root, "src/mid.rs", &mid(body));
        if branch == "right" {
            // Work only `right` did: a symbol edit and a whole file, neither of
            // which the merge result below takes.
            write(root, "src/core.rs", "pub fn base() {\n    // right\n}\n");
            write(root, "src/right_only.rs", "pub fn right_only() {}\n");
        }
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", branch]);
        git(&["checkout", "-q", "main"]);
    }
    // The merge result took `left`'s edit and a new file of its own, and never
    // took `right`'s — the silent-drop shape, made explicit.
    git(&["checkout", "-q", "-b", "merged", "left"]);
    write(root, "src/extra.rs", "pub fn extra() {}\n");
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "merge"]);
    tmp
}

/// `branch-overlap` through the shipped binary in `--json` mode ([FR-NV-13],
/// S-360, CR-114).
///
/// The MCP parity test compares read-models, not argv, so this is the only
/// place a dropped or mis-plumbed CLI flag (`--ref`, `--base`, `--merge`) would
/// be caught.
#[test]
fn branch_overlap_reports_contention_and_loss_through_the_binary() {
    let tmp = overlap_fixture();
    logos(tmp.path(), &["index"]);

    let out = logos(
        tmp.path(),
        &["branch-overlap", "--ref", "left", "--ref", "right", "--json"],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits a parseable read-model");
    let contended = payload["contended"].as_array().expect("contended");
    assert!(
        contended.iter().any(|row| row["name"] == "mid"),
        "both refs edit `mid`: {payload}"
    );
    let mid = contended.iter().find(|row| row["name"] == "mid").unwrap();
    assert_eq!(mid["modified_by"], serde_json::json!(["left", "right"]));
    assert!(
        !payload["coverage"]["statement"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the coverage limits ride the payload (NFR-CC-04): {payload}"
    );

    // `--merge` reaches the engine. What is reported lost is `right`'s edit to
    // `base` and its whole file `src/right_only.rs` — work the merge result
    // does not carry at all. Its edit to `mid` is NOT reported lost, because
    // the merge changed that symbol too (taking `left`'s version): the merge
    // check compares which symbols changed, not their content, and the payload
    // states that limit. `contended` above is what carries that shape.
    let out = logos(
        tmp.path(),
        &[
            "branch-overlap",
            "--ref",
            "left",
            "--ref",
            "right",
            "--merge",
            "merged",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["merge"]["ref"], "merged", "{payload}");
    assert!(
        payload["merge"]["lost_symbols"]
            .as_array()
            .expect("lost_symbols")
            .iter()
            .any(|row| row["name"] == "base" && row["modified_by"] == serde_json::json!(["right"])),
        "`right`'s edit to `base` never reached the merge: {payload}"
    );
    assert!(
        payload["merge"]["lost_files"]
            .as_array()
            .expect("lost_files")
            .iter()
            .any(|row| row["path"] == "src/right_only.rs"),
        "the whole file the merge never took: {payload}"
    );
    // …and that same file is a stated limit of the symbol half: `right` added
    // it, the indexed merge result never had it, so no symbol of its own can be
    // reported for it ([NFR-CC-04], the AC's own words).
    assert!(
        payload["coverage"]["files_without_indexed_symbols"]
            .as_array()
            .expect("files_without_indexed_symbols")
            .iter()
            .any(|path| path == "src/right_only.rs"),
        "{payload}"
    );

    // `--base` reaches the engine and changes the comparison point.
    let out = logos(
        tmp.path(),
        &[
            "branch-overlap",
            "--ref",
            "left",
            "--ref",
            "right",
            "--base",
            "main",
            "--json",
        ],
    );
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(payload["base_origin"], "stated by the caller as main", "{payload}");

    // An unresolvable ref is a coverage limit on an exit-0 payload, never an error.
    let out = logos(
        tmp.path(),
        &["branch-overlap", "--ref", "left", "--ref", "nope", "--json"],
    );
    assert_eq!(exit_code(&out), 0, "an unknown ref is not a failure");
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        payload["coverage"]["unresolved_refs"],
        serde_json::json!(["nope"]),
        "{payload}"
    );

    // `--ref` is required: that is a clap parse contract, not a graph answer.
    let out = logos(tmp.path(), &["branch-overlap", "--json"]);
    assert_eq!(exit_code(&out), 2, "a missing --ref is a usage fault");
}

/// `git` absent from `PATH` is a distinct branch from "not a git repository":
/// the spawn itself fails, taking the `.ok()?` arm rather than the
/// `status.success()` arm. Both must degrade to an exit-0 payload that says why
/// it is empty ([ADR-14], [NFR-CC-04]) — a bare "no collisions" here would be
/// the most dangerous answer this tool can give.
#[test]
fn branch_overlap_without_git_on_path_still_exits_zero() {
    let tmp = overlap_fixture();
    logos(tmp.path(), &["index"]);

    let out = Command::new(env!("CARGO_BIN_EXE_logos"))
        .env("PATH", "")
        .arg("--project")
        .arg(tmp.path())
        .args(["branch-overlap", "--ref", "left", "--ref", "right", "--json"])
        .output()
        .expect("the logos binary runs");
    assert_eq!(
        exit_code(&out),
        0,
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json still emits a read-model");
    assert_eq!(
        payload["coverage"]["unresolved_refs"],
        serde_json::json!(["left", "right"]),
        "{payload}"
    );
    assert!(payload["base"].is_null(), "{payload}");
    assert!(payload["contended"].as_array().unwrap().is_empty(), "{payload}");
    assert!(
        !payload["coverage"]["statement"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "an answer with no data behind it still states its limits: {payload}"
    );
}
