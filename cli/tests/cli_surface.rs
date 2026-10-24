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

/// The binary's composition, compiled from the file `main` composes from
/// — so a roster read in-process here is the shipped binary's, its extensions'
/// tools included (S-644), as `surface_snapshots.rs` reads it.
#[path = "../src/composition.rs"]
mod composition;

/// Run the built `logos` binary with `args` against `project` via the global
/// `--project` flag (so tests never depend on the harness cwd).
///
/// The user's Claude Code directory is pointed at a path that does not exist,
/// so a developer who enabled the Logos plugin for themselves does not turn
/// every `init -i` here into the plugin-aware one (FR-IN-12).
fn logos(project: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .env("CLAUDE_CONFIG_DIR", project.join(".no-claude-config"))
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("the logos binary runs")
}

/// [`logos`] with the user's Claude Code directory (`CLAUDE_CONFIG_DIR`) at
/// `claude_dir`, and git discovery stopped at `project`'s parent so the
/// fixture is never taken for part of an enclosing repository.
fn logos_with_claude(project: &Path, claude_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_logos"))
        .env("CLAUDE_CONFIG_DIR", claude_dir)
        .env("GIT_CEILING_DIRECTORIES", project.parent().unwrap_or(project))
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
    &["init", "--uninstall"],
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
    // The underscore spellings are the point of the `alias` attributes: an
    // agent that knows the MCP tool name `session_start` must reach the twin.
    // A dropped alias becomes an unknown subcommand — exit 2 — which the sweep
    // below is exactly the assertion for (S-361, FR-CL-06).
    &["session_start"],
    &["session_end"],
    &["doctor"],
    &["verify"],
    &["evolution"],
    &["dsm"],
    &["hotspots", "--untested"],
    &["hotspots", "--production-scope"],
    &["hotspots", "--with-findings"],
    &["coverage", "status"],
    &["coverage", "ingest", "coverage.lcov", "--format", "lcov"],
    &["coverage", "refresh"],
    &["coverage", "run"],
    &["coverage", "run", "--tool", "go", "--consent"],
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

/// S-570 / FR-PL-09: `logos languages --json` carries, for every code
/// language, exactly the `reach` its descriptor declares — level and
/// cross-file set in their wire spelling — and omits it from the documentation
/// and artifact plugins, which bind no code reference.
///
/// Every language is compared against its own declaration rather than a
/// literal: a story that changes a language's reach updates its `plugin.toml`
/// (verified against the graph by `logos-core`'s `reach_declared`), and this
/// surface must follow it without a second edit here.
#[test]
fn languages_json_carries_each_code_languages_declared_reach() {
    let tmp = TempDir::new().unwrap();
    let out = logos(tmp.path(), &["languages", "--json"]);
    assert_eq!(exit_code(&out), 0);
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = json["languages"].as_array().expect("languages array");
    let registry = logos_core::plugin::LanguageRegistry::load(tmp.path()).expect("embedded plugins load");

    let mut code = 0;
    for row in rows {
        let name = row["name"].as_str().expect("a language row names itself");
        let plugin = registry
            .iter()
            .find(|p| p.name() == name)
            .unwrap_or_else(|| panic!("{name} is a loaded plugin"));
        let is_code = row["capabilities"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c == "symbols"));
        assert_eq!(plugin.reach().is_some(), is_code, "{name}: a code language, and only one, declares a reach");
        match plugin.reach() {
            Some(reach) => {
                code += 1;
                let cross_file: Vec<&str> = reach.cross_file.iter().map(|r| r.as_str()).collect();
                assert_eq!(
                    row["reach"],
                    serde_json::json!({ "level": reach.level.as_str(), "cross_file": cross_file }),
                    "{name} carries the reach its descriptor declares"
                );
            }
            None => assert!(row.get("reach").is_none(), "{name} has no reach: {row}"),
        }
    }
    assert!(code >= 1, "the default build lists code languages: {json}");
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

// ── CR-119: the `index` root-scope note over an enabled workspace root ─────

/// A committed git repo at `dir` with one source file — real enough for
/// `init --workspace`'s candidate discovery (`git rev-parse --show-toplevel`
/// must actually succeed), unlike the fake `.git/HEAD` fixtures above that
/// only need to satisfy the existence-only nested-git prune rule.
fn init_git_repo(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    sh_git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("lib.rs"), "pub fn f() {}\n").unwrap();
    sh_git(dir, &["add", "."]);
    sh_git(dir, &["commit", "-q", "-m", "init"]);
}

/// At a root with no `logos.workspace.toml`, `index --json` carries no
/// `notes` field at all — not an empty array, an absent key — which is what
/// makes the output byte-identical to before this field existed (CR-119).
#[test]
fn index_without_a_workspace_manifest_has_no_notes_field() {
    let tmp = fixture();
    let out = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    assert!(
        json.as_object().unwrap().get("notes").is_none(),
        "no manifest ⇒ no `notes` key at all, not merely an empty one: {json}"
    );
}

/// A root carrying `logos.workspace.toml` gets a root-scope note from
/// `index`: it built the root project only, members are indexed
/// independently, and `workspace status` is named. Advisory throughout — no
/// `warnings` entry, exit 0 unchanged (CR-119, FR-CL-03).
#[test]
fn index_at_an_enabled_workspace_root_emits_a_root_scope_note() {
    let tmp = fixture();
    fs::write(
        tmp.path().join("logos.workspace.toml"),
        "[workspace]\nname = \"w\"\nmembers = []\n",
    )
    .unwrap();

    let out = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let notes = json["notes"].as_array().expect("a manifest yields a notes array: {json}");
    assert_eq!(notes.len(), 1, "exactly one root-scope note: {notes:?}");
    let note = notes[0].as_str().unwrap();
    assert!(note.contains("root project only"), "{note}");
    assert!(note.contains("logos workspace status"), "{note}");

    // Advisory: no warnings entry, and the exit code already asserted 0 above.
    assert!(
        json["warnings"].as_array().unwrap().is_empty(),
        "the note must never ride `warnings`: {json}"
    );
}

/// **The transition CR-119 exists to close.** The same parent-of-repos root
/// gives the FR-IX-13 zero-admission warning before `init --workspace`, and
/// the FR-WS-02 root-scope note after — never both, never neither. Asserting
/// either state alone would not have caught the gap CR-119 describes: a
/// diagnostic that fires before enablement and falls silent after, with
/// nothing filling the silence.
#[test]
fn the_zero_admission_warning_becomes_a_root_scope_note_after_enrolment() {
    let tmp = TempDir::new().unwrap();
    init_git_repo(&tmp.path().join("api"));
    init_git_repo(&tmp.path().join("web"));

    // ── before: the FR-IX-13 diagnostic fires ──────────────────────────────
    let before = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(exit_code(&before), 0, "{}", String::from_utf8_lossy(&before.stderr));
    let before_json: serde_json::Value = serde_json::from_slice(&before.stdout).unwrap();
    assert_eq!(before_json["files_indexed"], 0, "nothing admitted before enrolment");
    let before_warnings = before_json["warnings"].as_array().unwrap();
    assert!(
        before_warnings.iter().any(|w| w.as_str().unwrap_or("").contains("logos init --workspace")),
        "the zero-admission warning fires before enrolment: {before_warnings:?}"
    );
    assert!(
        before_json.get("notes").is_none(),
        "no manifest yet, so no root-scope note either: {before_json}"
    );

    // ── enable the workspace ────────────────────────────────────────────────
    let enabled = logos(tmp.path(), &["--json", "init", "--workspace", "--yes"]);
    assert_eq!(exit_code(&enabled), 0, "{}", String::from_utf8_lossy(&enabled.stderr));
    assert!(tmp.path().join("logos.workspace.toml").is_file());

    // ── after: the root-scope note fires, the zero-admission warning does not ──
    let after = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(exit_code(&after), 0, "{}", String::from_utf8_lossy(&after.stderr));
    let after_json: serde_json::Value = serde_json::from_slice(&after.stdout).unwrap();
    assert!(
        after_json["files_indexed"].as_u64().unwrap() > 0,
        "the root itself now admits its own manifest: {after_json}"
    );
    let after_warnings = after_json["warnings"].as_array().unwrap();
    assert!(
        !after_warnings.iter().any(|w| w.as_str().unwrap_or("").contains("logos init --workspace")),
        "post-enrolment, the zero-admission trigger cannot fire (non-zero admission): {after_warnings:?}"
    );
    let after_notes = after_json["notes"].as_array().expect("post-enrolment, the root-scope note fires: {after_json}");
    assert!(
        after_notes.iter().any(|n| n.as_str().unwrap_or("").contains("root project only")),
        "{after_notes:?}"
    );
}

// ── HF-1: the minified-JS exclusion notice is advisory, never a warning ────

/// A tree holding two minified files the default `exclude` keeps out.
fn minified_fixture() -> TempDir {
    let tmp = fixture();
    write(tmp.path(), "app.min.js", "function a(){return 1}\n");
    write(tmp.path(), "static/js/lib.min.js", "function l(){return 2}\n");
    tmp
}

/// `index --json` and the reconcile-backed `scan --json` state the minified-JS
/// exclusion on `notes` and keep it off `warnings` — the channel a CI parser
/// scans (CR-119, FR-CL-03). Exit 0 throughout.
#[test]
fn the_minified_js_exclusion_is_a_note_on_index_and_scan_never_a_warning() {
    let tmp = minified_fixture();
    let mentions = |v: &serde_json::Value| {
        v.as_array()
            .map(|a| a.iter().any(|e| e.as_str().unwrap_or("").contains("minified JavaScript file(s)")))
            .unwrap_or(false)
    };

    let index = logos(tmp.path(), &["index", "--json"]);
    assert_eq!(exit_code(&index), 0, "{}", String::from_utf8_lossy(&index.stderr));
    let index_json: serde_json::Value = serde_json::from_slice(&index.stdout).unwrap();
    assert!(mentions(&index_json["notes"]), "index states it on `notes`: {index_json}");
    assert!(!mentions(&index_json["warnings"]), "index must not warn: {index_json}");

    let scan = logos(tmp.path(), &["scan", "--json"]);
    assert_eq!(exit_code(&scan), 0, "{}", String::from_utf8_lossy(&scan.stderr));
    let scan_json: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    assert!(mentions(&scan_json["notes"]), "scan states it on `notes`: {scan_json}");
    assert!(!mentions(&scan_json["warnings"]), "scan must not warn: {scan_json}");

    // The human rendering is the same read-model, so it carries the note too.
    let human = logos(tmp.path(), &["index"]);
    assert_eq!(exit_code(&human), 0, "{}", String::from_utf8_lossy(&human.stderr));
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("minified JavaScript file(s)"),
        "the human index output carries the note"
    );
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
fn a_malformed_config_makes_index_and_sync_fail_loud_with_exit_two() {
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
    // `sync` over that index validates the same way (S-513): without the guard
    // the fault would reach `Engine::sync`, which reports it as a failed run —
    // exit 1, not 2.
    write(
        tmp.path(),
        ".logos/config.toml",
        "[config_artifacts]\nenabled = true\nbogus_key = 7\n",
    );
    let out = logos(tmp.path(), &["sync", "src/core.rs"]);
    assert_eq!(
        exit_code(&out),
        2,
        "sync: an unknown config.toml key is a usage fault (exit 2): {}",
        String::from_utf8_lossy(&out.stderr)
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

/// S-313 / FR-GV-21: `check --json` gains `ran_at` **additively** — every
/// field the surface already carried is still there, unrenamed, and the exit
/// code is unmoved by the new one.
///
/// `ran_at` is the run marker this run recorded, so it is present even on a
/// clean pass: the recorded-clean-run case is the one the violations array
/// cannot express. The field is `null` only where nothing was persisted.
#[test]
fn check_json_carries_the_run_timestamp_additively() {
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
        "the added field moves no verdict: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");

    // Additive: nothing renamed or removed.
    for existing in [
        "passed",
        "checked_rules",
        "rules_present",
        "violations",
        "freshness",
        "warnings",
    ] {
        assert!(
            json.get(existing).is_some(),
            "`{existing}` must survive the widening: {json}"
        );
    }
    let ran_at = json["ran_at"]
        .as_i64()
        .unwrap_or_else(|| panic!("check --json must carry ran_at: {json}"));
    assert!(
        ran_at > 1_600_000_000,
        "ran_at is a unix-seconds run time, not a counter: {ran_at}"
    );
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

/// `scan --json` and `quality-report --json` both render the graph dimensions
/// as not applicable ([FR-QM-18], [CR-156]): the fixture's three files share one
/// directory, so none of its edges crosses a module and the five cross-module
/// edges Modularity, Acyclicity and Depth need are absent. Each surface carries
/// the reason with the n-of-5 count, `n` being the very `cross_module_edge_count`
/// the scan scored — while the computed pairs are still present beside it.
///
/// [FR-QM-18]: ../../docs/specs/requirements/FR-QM-18.md
/// [CR-156]: ../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
#[test]
fn scan_and_quality_report_render_not_applicable_graph_dimensions() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);

    let out = logos(tmp.path(), &["scan", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let scan: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    let metrics = &scan["metrics"];
    let n = metrics["cross_module_edge_count"].as_u64().expect("cross_module_edge_count");
    assert!(metrics["edge_count"].as_u64().expect("edge_count") > n, "intra-module edges exist and do not count");
    assert_eq!(n, 0, "the three files share one directory");
    let expected = serde_json::json!({
        "edges": n,
        "min_edges": 5,
        "reason": format!("{n} of 5 cross-module edges — too few to measure module structure"),
    });
    for dimension in ["modularity", "acyclicity", "depth"] {
        assert_eq!(metrics[format!("{dimension}_not_applicable")], expected, "scan --json: {dimension}");
        assert!(
            metrics[dimension]["normalized"].is_f64(),
            "the computed {dimension} pair is still reported"
        );
    }

    let out = logos(tmp.path(), &["quality-report", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let readout: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    for dimension in ["modularity", "acyclicity", "depth"] {
        assert_eq!(readout[format!("{dimension}_not_applicable")], expected, "quality-report --json: {dimension}");
    }
}

/// On a project whose one language binds same-file only (C), `scan --json`,
/// `gate --json` and `session-start --json` each carry exactly one coverage
/// warning, the same text on all three, naming `c` and its share ([FR-QM-18]) —
/// and each exits as it did before the warning existed: 0 for the scan, the
/// first gate's informational pass and the session start.
///
/// [FR-QM-18]: ../../docs/specs/requirements/FR-QM-18.md
#[test]
fn the_coverage_warning_rides_scan_gate_and_session_start_without_moving_an_exit_code() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/core/core.c", "int helper(int x) { return x + 1; }
int step(int x) { return helper(x); }
");
    write(tmp.path(), "src/net/net.c", "int step(int x);
int send(int x) { return step(x); }
");
    logos(tmp.path(), &["index", "--quiet"]);

    let mut seen = Vec::new();
    for args in [&["scan", "--json"][..], &["gate", "--json"], &["session-start", "--json"]] {
        let out = logos(tmp.path(), args);
        assert_eq!(exit_code(&out), 0, "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
        let mut coverage: Vec<String> = json["warnings"]
            .as_array()
            .expect("a warnings array")
            .iter()
            .filter_map(|w| w.as_str().filter(|w| w.starts_with("coverage: ")).map(str::to_owned))
            .collect();
        assert_eq!(coverage.len(), 1, "{args:?}: {coverage:?}");
        assert!(coverage[0].starts_with("coverage: c is 100% of production callables"), "{}", coverage[0]);
        seen.push(coverage.remove(0));
    }
    assert!(seen.windows(2).all(|pair| pair[0] == pair[1]), "one text on every surface: {seen:?}");
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
        &["health", "--json"],
        &["doctor", "--json"],
        &["verify", "--json"],
        &["session-start", "--json"],
        &["session-end", "--json"],
        &["evolution", "--json"],
        &["dsm", "--json"],
        &["hotspots", "--json"],
        &["hotspots", "--untested", "--json"],
        &["hotspots", "--production-scope", "--json"],
        &["hotspots", "--with-findings", "--json"],
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
    // CR-236: both edge lists are present and empty on a clean store.
    for (total, list) in [("lost_edges_total", "lost_edges"), ("extra_edges_total", "extra_edges")] {
        assert_eq!(report[total], serde_json::json!(0), "{report}");
        assert_eq!(report[list], serde_json::json!([]), "{report}");
    }

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
    // CR-236: the stale edges the leak keeps are named — kind, both symbols, both files.
    assert!(
        report["extra_edges_total"].as_u64().unwrap() > 0
            && report["extra_edges"].as_array().is_some_and(|e| {
                !e.is_empty()
                    && e.iter().all(|edge| {
                        ["kind", "source_symbol", "target_symbol", "source_file", "target_file"]
                            .iter()
                            .all(|k| edge.get(*k).is_some())
                    })
            }),
        "the extra (live-only) edges of the deleted file are named: {report}"
    );
    assert_eq!(report["lost_edges_total"], serde_json::json!(0), "{report}");
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

// ── FR-SC-14: `--with-findings` end-to-end through the binary ──────────────

/// `logos hotspots` carries a findings column only once a scan is stored, and
/// `--with-findings` ranks the file with a fresh high finding above an
/// otherwise identical one ([FR-SC-14]).
#[test]
fn hotspots_with_findings_flag_ranks_the_file_with_a_fresh_finding_first() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    let branchy = |n: usize| -> String {
        let body: String = (0..4).map(|i| format!("    if x == {i} {{ return {i}; }}\n")).collect();
        format!("pub fn f(x: i64) -> i64 {{\n{body}    x\n}}\n// rev {n}\n")
    };
    for name in ["a", "z"] {
        for n in 0..2 {
            git_commit(repo, &format!("src/{name}.rs"), &branchy(n), &format!("{name} v{n}"));
        }
    }
    assert_eq!(exit_code(&logos(repo, &["index", "--quiet"])), 0);
    let paths_of = |json: &serde_json::Value| -> Vec<String> {
        json["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
    };

    // Nothing scanned: no column, and the filter has nothing to prefer.
    let bare = json_of(&logos(repo, &["hotspots", "--with-findings", "--json"]));
    assert!(bare["files"][0].get("findings").is_none(), "no scan, no column: {bare}");
    assert!(bare.get("with_findings").is_none(), "{bare}");
    assert_eq!(paths_of(&bare), ["src/a.rs", "src/z.rs"]);

    let report = repo.join(".logos/z.sarif");
    std::fs::write(
        &report,
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"Semgrep OSS","semanticVersion":"1.85.0","rules":[]}},
        "results":[{"ruleId":"r","level":"error","message":{"text":"m"},"locations":[{"physicalLocation":
        {"artifactLocation":{"uri":"src/z.rs"},"region":{"startLine":1}}}]}]}]}"#,
    )
    .unwrap();
    let ingested = logos(repo, &["security", "ingest", report.to_str().unwrap()]);
    assert_eq!(exit_code(&ingested), 0, "{}", String::from_utf8_lossy(&ingested.stderr));

    let plain = json_of(&logos(repo, &["hotspots", "--json"]));
    assert_eq!(paths_of(&plain), ["src/a.rs", "src/z.rs"], "the column alone moves nothing");
    assert_eq!(plain["files"][1]["findings"]["count"], 1, "{plain}");
    assert_eq!(plain["files"][1]["findings"]["highest"], "high", "{plain}");

    let filtered = json_of(&logos(repo, &["hotspots", "--with-findings", "--json"]));
    assert_eq!(paths_of(&filtered), ["src/z.rs", "src/a.rs"], "{filtered}");
    assert_eq!(filtered["with_findings"], true);
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
    // A skill is materialized only by a step a composed extension contributes,
    // and this build composes none (FR-DS-05): no step writes under `.agents/`.
    assert!(
        !targets.iter().any(|t| t.starts_with(".agents")),
        "no skill step runs in a build that composes no extension: {targets:?}"
    );
    assert!(
        !targets.contains(&".logos/hooks"),
        "hooks stay opt-in on non-TTY -i: {targets:?}"
    );
    assert!(tmp.path().join(".mcp.json").is_file());
    assert!(tmp.path().join("CLAUDE.md").is_file());
    assert!(!tmp.path().join(".agents").exists(), "no skill is materialized");
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
    let init = init.unwrap();
    assert_eq!(
        init["result"]["serverInfo"]["name"], "logos",
        "the host derives logos:<tool> from this identity (FR-MC-01)"
    );
    // The other half of that identity, and the half that had rotted: `mcp`
    // carried its own `0.3.0`, so `env!("CARGO_PKG_VERSION")` in its
    // `get_info` reported 0.3.0 while the binary answering this very probe was
    // 1.4.6. Asserting it HERE is the point — this crate's
    // `CARGO_PKG_VERSION` is the product version, and the response came off a
    // spawned `logos serve --mcp`, so the two can only agree if the shipped
    // artifact really advertises what it really is. A per-crate version
    // reintroduced anywhere in the chain fails this line.
    assert_eq!(
        init["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "the stdio server advertises a version that is not the shipped \
         binary's — hosts surface this string to identify the server"
    );
    // Derived from the real router, never written down (S-361): a hand-written
    // count is what two Sprint 64 sessions each got wrong on the same line, and
    // git merged the identical edits without a conflict. `list_tools` reads the
    // static tool attrs, so the throwaway engine needs no index — this asserts
    // that the SHIPPED BINARY's stdio server registers the same roster the
    // in-process router declares over the binary's own composition, which is the
    // half a spawned process can hide.
    let registered = mcp::LogosMcp::new(logos_core::Engine::open(
        TempDir::new().expect("tempdir").path(),
    ), &composition::host())
    .list_tools()
    .len();
    assert_eq!(
        tools.unwrap()["result"]["tools"]
            .as_array()
            .expect("tools array")
            .len(),
        registered,
        "all {registered} registered tools reach the host through the shipped binary (FR-MC-01)"
    );

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
    // A pinned coordinate (S-703): the queried symbol names it.
    write(tmp.path(), ".logos/config.toml", "[project]\nid = \"example.com/fixture\"\n");
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
        &["precedent", "logos . example.com/fixture . src/`rustarm.rs`/extract().", "--json"],
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

// ── CR-070 / FR-WK-14 retirement: the PostToolUse augmentation hook is gone ──

/// The file names under `<root>/.claude/hooks/`, sorted — what a hook install left
/// there, so an assertion holds the whole directory rather than one known name.
fn hook_scripts(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join(".claude/hooks"))
        .expect(".claude/hooks/ is written")
        .map(|entry| entry.expect("a directory entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// `logos hook --emit` materializes only the marker-tagged session-start
/// quality-report script + settings entry (exit 0), is idempotent on re-run,
/// `--force` re-emits without duplicating, and a foreign settings file is
/// never overwritten (FR-IN-07). Run through the real binary so the offline
/// path (NFR-SE-01) and exit codes are exactly as CI sees them.
#[test]
fn hook_emit_installs_and_is_idempotent() {
    let tmp = TempDir::new().unwrap();

    let out = logos(tmp.path(), &["--json", "hook", "--emit"]);
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
    let again = logos(tmp.path(), &["--json", "hook", "--emit"]);
    assert_eq!(exit_code(&again), 0);
    assert!(String::from_utf8_lossy(&again.stdout).contains("\"action\":\"skipped\""));
    assert_eq!(
        fs::read(tmp.path().join(".claude/settings.json")).unwrap(),
        before,
        "an unforced re-emit is byte-identical"
    );

    // --force re-emits exactly one entry (no duplicate).
    let forced = logos(tmp.path(), &["--json", "hook", "--emit", "--force"]);
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
    // serialized contract.
    let mut drifted = settings.clone();
    drifted["hooks"]["SessionStart"][0]["hooks"][0]["timeout"] = serde_json::json!(1);
    fs::write(
        tmp.path().join(".claude/settings.json"),
        serde_json::to_string_pretty(&drifted).unwrap(),
    )
    .unwrap();
    let healed = logos(tmp.path(), &["--json", "hook", "--emit"]);
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

/// [CR-070]/[CR-095]: `hook --emit` materializes only the session-start
/// quality-report hook — still a single summary object, not an array, since
/// [CR-095] replaced one hook with one hook — and produces no augmentation
/// script or PostToolUse entry (the retired [FR-WK-14] hook), and no retired
/// SessionEnd quality-report entry.
#[test]
fn hook_emit_installs_only_the_quality_report_hook() {
    let tmp = TempDir::new().unwrap();

    let out = logos(tmp.path(), &["--json", "hook", "--emit"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let summary: serde_json::Value = serde_json::from_slice(&out.stdout).expect("object JSON");
    assert!(summary.is_object(), "a single summary object, not an array: {summary}");
    assert_eq!(summary["action"], "created");
    assert_eq!(summary["settings"], ".claude/settings.json");

    // No augmentation script or PostToolUse entry anywhere: the one script in
    // `.claude/hooks/` is the quality-report hook, so neither retired script — the
    // augmentation hook's nor the SessionEnd one's (CR-095) — is materialized.
    let quality_script = tmp.path().join(".claude/hooks/logos-quality-open.sh");
    assert_eq!(hook_scripts(tmp.path()), ["logos-quality-open.sh"], "only the quality-report script is written");
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

/// [CR-047]/[CR-070] retirement regression: no artifact `hook --emit` (or
/// `init -i`, covered separately in `logos-core/tests/init.rs`) materializes
/// references a `claude -p` invocation or the retired autogen hook script, no
/// augmentation script is written, and no `.claude/settings.local.json` is
/// ever written (FR-WK-14, FR-WK-16, NFR-SE-01).
#[test]
fn no_autogen_hook_or_claude_p_reference_remains() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["--json", "hook", "--emit"])), 0);

    assert_eq!(
        hook_scripts(tmp.path()),
        ["logos-quality-open.sh"],
        "neither the retired autogen nor the augmentation hook script is materialized"
    );
    assert!(
        !tmp.path().join(".claude/settings.local.json").exists(),
        "no hook ever writes the per-developer settings.local.json"
    );
    let body = fs::read_to_string(tmp.path().join(".claude/hooks/logos-quality-open.sh")).unwrap();
    assert!(
        !body.contains("claude -p") && !body.contains("autogen"),
        "the quality-report hook carries no claude -p invocation and no autogen reference"
    );
}

// ── CR-095 retirement: the SessionEnd quality-report hook is swept ───────────

/// [CR-095]: `hook --emit` through the real binary sweeps a retired
/// SessionEnd quality-report entry and its orphaned script, reports the removal
/// in its `--json` summary, and leaves a foreign SessionEnd entry untouched.
/// Without the sweep an existing install keeps emitting `SessionEnd hook [...]
/// failed: Hook cancelled` on every exit and every `/clear`.
#[test]
fn hook_emit_sweeps_the_retired_session_end_hook() {
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

    let out = logos(tmp.path(), &["--json", "hook", "--emit"]);
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
    let again = logos(tmp.path(), &["--json", "hook", "--emit"]);
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

// ── S-361 / FR-CL-06: the session-gate CLI twins, through the real binary ────
//
// `cli/src/lib.rs::surface_parity` proves the three commands EXIST on both
// surfaces and are declared with a reason where they do not. That is a
// registration contract; it says nothing about what they do. These tests cover
// the half a roster cannot: the exit-code projection and the read-model each
// one actually emits, through the shipped executable.

/// FR-GV-04 / FR-GV-05 through the binary: `session-start` records a baseline
/// one process later `session-end` compares against, and a regression exits 1.
///
/// This is the assertion behind the sentence [FR-IN-02]'s managed `CLAUDE.md`
/// block now ships into every initialised project — *"a failing gate exits 1"*.
/// Modelled on [`gate_exits_one_on_regression`], because the pair IS the gate:
/// `Engine::session_end` is `governance::gate(.., None, false, true)`.
///
/// [FR-IN-02]: ../../docs/specs/requirements/FR-IN-02.md
#[test]
fn the_session_gate_pair_carries_a_baseline_across_processes_and_exits_one_on_regression() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);

    let started = logos(tmp.path(), &["session-start", "--json"]);
    assert_eq!(
        exit_code(&started),
        0,
        "session-start always exits 0 — it records, it does not gate: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let start: serde_json::Value =
        serde_json::from_slice(&started.stdout).expect("session-start emits valid JSON");
    assert!(
        start["session_id"].as_str().is_some_and(|id| !id.is_empty()),
        "the baseline snapshot id is the session handle: {start}"
    );
    let baseline = start["signal"].as_u64().expect("a recorded baseline signal");

    // An unchanged tree passes, and — the load-bearing half — `session-end` in a
    // SEPARATE process compares against the signal the earlier process recorded.
    // That only works because the baseline is a store row, not process state.
    let held = logos(tmp.path(), &["session-end", "--json"]);
    assert_eq!(
        exit_code(&held),
        0,
        "an unchanged session holds its gate: {}",
        String::from_utf8_lossy(&held.stderr)
    );
    let end: serde_json::Value =
        serde_json::from_slice(&held.stdout).expect("session-end emits valid JSON");
    assert_eq!(end["passed"], true);
    assert_eq!(
        end["baseline_signal"].as_u64(),
        Some(baseline),
        "session-end compares against the baseline session-start wrote: {end}"
    );

    // Degrade the graph exactly as `gate_exits_one_on_regression` does.
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
    let regressed = logos(tmp.path(), &["session-end", "--json"]);
    assert_eq!(
        exit_code(&regressed),
        1,
        "a regression past the session baseline exits 1 (FR-GV-05): {}",
        String::from_utf8_lossy(&regressed.stderr)
    );
    let failed: serde_json::Value =
        serde_json::from_slice(&regressed.stdout).expect("valid JSON on the failing path too");
    assert_eq!(failed["passed"], false);
}

/// `health` emits the ARCHITECTURE read-model — not `status`'s, and not
/// `doctor`'s — and honours `--no-reconcile` (FR-RC-04).
///
/// `schema_version` is the field `mcp/tests/protocol.rs` picked as unique to
/// this read-model precisely so a dispatch swap fails loudly; the CLI needs the
/// same guard, because `health` is the command whose own doc comment has to
/// spend three lines distinguishing it from `status`.
#[test]
fn health_reports_the_architecture_read_model_through_the_binary() {
    let tmp = fixture();
    logos(tmp.path(), &["index", "--quiet"]);

    let out = logos(tmp.path(), &["health", "--json"]);
    assert_eq!(
        exit_code(&out),
        0,
        "a healthy graph exits 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("health emits valid JSON");
    assert!(
        json.get("schema_version").is_some(),
        "the ARCHITECTURE read-model, not another command's: {json}"
    );
    assert_eq!(json["ok"], true);
    assert_eq!(json["fts_ok"], true);
    assert_eq!(json["structural_ok"], true);
    assert!(
        json["nodes"].as_u64().is_some_and(|n| n > 0),
        "an indexed project reports its graph counts: {json}"
    );

    // `--no-reconcile` reaches the engine as `health(false)`. Asserting the
    // freshness line is what kills the inverted-boolean class: a flipped `!`
    // would silently reconcile and this string would not appear.
    let fast = logos(tmp.path(), &["health", "--no-reconcile", "--json"]);
    assert_eq!(exit_code(&fast), 0);
    let fast_json: serde_json::Value =
        serde_json::from_slice(&fast.stdout).expect("valid JSON");
    assert!(
        fast_json["freshness"]
            .as_str()
            .is_some_and(|f| f.starts_with("assumed-fresh (--no-reconcile)")),
        "--no-reconcile skips the pre-evaluation reconcile (FR-RC-04): {fast_json}"
    );
}

#[test]
fn sync_help_states_that_no_path_re_reads_nothing() {
    let tmp = TempDir::new().expect("temp root");
    let out = logos(tmp.path(), &["sync", "--help"]);
    assert!(out.status.success(), "`sync --help` exits 0");
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    assert!(
        !text.contains("defaults to all changed files"),
        "sync help still carries the retired claim — with no path sync re-reads nothing:\n{text}"
    );
    for wanted in ["exactly", "re-reads no file", "logos scan", "logos index"] {
        assert!(
            text.contains(wanted),
            "sync help must carry {wanted:?} (the manual's wording):\n{text}"
        );
    }
}

/// S-550 / FR-NV-15: `logos node <bare name>` prints the code declaration and
/// names the module it passed over; the SCIP symbol of that module reaches it
/// with no alternatives.
#[test]
fn node_on_a_bare_name_prints_the_class_and_names_the_module_alternative() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Monolog/Utils.php",
        "<?php\n\nnamespace Monolog;\n\nfinal class Utils\n{\n    public static function canonicalize(string $path): string\n    {\n        return $path;\n    }\n}\n",
    );
    logos(tmp.path(), &["index"]);

    let out = logos(tmp.path(), &["node", "Utils", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let payload: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits one machine-readable object");
    assert_eq!(payload["node"]["kind"], "class", "{payload}");
    assert_eq!(payload["alternatives"].as_array().map(Vec::len), Some(1), "{payload}");
    assert_eq!(payload["alternatives"][0]["kind"], "module", "{payload}");

    let module = payload["alternatives"][0]["symbol"].as_str().expect("a symbol");
    let out = logos(tmp.path(), &["node", module, "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let by_symbol: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(by_symbol["node"]["kind"], "module", "{by_symbol}");
    assert!(by_symbol.get("alternatives").is_none(), "{by_symbol}");
}

/// HF-1 / FR-NV-15: `callers`, `callees`, `impact` and `explore` resolve a bare
/// name as `node` does — to the class — and print the module they passed over;
/// that module's SCIP symbol reaches it with no `alternatives` key.
#[test]
fn the_other_navigation_tools_on_a_bare_name_resolve_the_class_and_name_the_module() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/Monolog/Utils.php",
        "<?php\n\nnamespace Monolog;\n\nfinal class Utils\n{\n    public static function canonicalize(string $path): string\n    {\n        return $path;\n    }\n}\n",
    );
    logos(tmp.path(), &["index"]);

    for (tool, anchor) in [
        ("callers", "resolved"),
        ("callees", "resolved"),
        ("impact", "resolved"),
        ("explore", "anchor"),
    ] {
        let out = logos(tmp.path(), &[tool, "Utils", "--json"]);
        assert_eq!(exit_code(&out), 0, "{tool}: {}", String::from_utf8_lossy(&out.stderr));
        let payload: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{tool}: --json emits one machine-readable object: {e}"));
        assert_eq!(payload[anchor]["kind"], "class", "{tool}: {payload}");
        assert_eq!(payload["alternatives"].as_array().map(Vec::len), Some(1), "{tool}: {payload}");
        assert_eq!(payload["alternatives"][0]["kind"], "module", "{tool}: {payload}");

        let module = payload["alternatives"][0]["symbol"].as_str().expect("a symbol");
        let out = logos(tmp.path(), &[tool, module, "--json"]);
        assert_eq!(exit_code(&out), 0, "{tool}: {}", String::from_utf8_lossy(&out.stderr));
        let by_symbol: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(by_symbol[anchor]["kind"], "module", "{tool}: {by_symbol}");
        assert!(by_symbol.get("alternatives").is_none(), "{tool}: {by_symbol}");
    }
}

/// S-507 / FR-GV-23 through the real binary: `check` against a ratchet budget
/// with no blessed set exits 1 and writes nothing; `check --bless` writes
/// `.logos/rules-baseline.json` and exits 0; a plain `check` then passes on the
/// blessed debt and leaves the file as it was.
#[test]
fn check_bless_writes_the_blessed_set_and_check_then_passes() {
    let tmp = fixture();
    write(
        tmp.path(),
        ".logos/rules.toml",
        "[constraints]\nmax_fn_lines = { max = 1, ratchet = true }\n",
    );
    let blessed = tmp.path().join(".logos/rules-baseline.json");
    logos(tmp.path(), &["index", "--quiet"]);

    let out = logos(tmp.path(), &["check", "--json"]);
    assert_eq!(exit_code(&out), 1, "unblessed ratchet debt fails: {}", String::from_utf8_lossy(&out.stderr));
    assert!(!blessed.exists(), "a plain check never writes the blessed set");

    let out = logos(tmp.path(), &["check", "--bless", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(json["ratchet"]["written"].as_u64().is_some_and(|n| n > 0), "{json}");
    let bytes = fs::read(&blessed).expect("--bless wrote the blessed set");

    let out = logos(tmp.path(), &["check", "--json"]);
    assert_eq!(exit_code(&out), 0, "blessed debt passes: {}", String::from_utf8_lossy(&out.stdout));
    assert_eq!(fs::read(&blessed).unwrap(), bytes, "check left the blessed set as it was");
}

// ── FR-IN-12: plugin-aware `init -i`, `init --uninstall`, doctor findings ────

/// The pinned plugin-absent `init -i` output: captured from the binary built
/// at the commit before FR-IN-12 landed (`cli/tests/fixtures/init_plugin_absent`).
const ABSENT_GOLDEN: &[(&str, &str)] = &[
    (".mcp.json", include_str!("fixtures/init_plugin_absent/mcp.json.golden")),
    ("CLAUDE.md", include_str!("fixtures/init_plugin_absent/CLAUDE.md.golden")),
    (".claude/settings.json", include_str!("fixtures/init_plugin_absent/settings.json.golden")),
    (
        ".claude/hooks/logos-quality-open.sh",
        include_str!("fixtures/init_plugin_absent/logos-quality-open.sh.golden"),
    ),
    (".logos/config.toml", include_str!("fixtures/init_plugin_absent/config.toml.golden")),
    (".logos/rules.toml", include_str!("fixtures/init_plugin_absent/rules.toml.golden")),
    (".logos/.gitignore", include_str!("fixtures/init_plugin_absent/gitignore.golden")),
];

/// Every file and directory (with a trailing `/`) under `root` outside
/// `.logos/` and `.git/`, sorted.
fn files_outside_logos(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if rel == ".logos" || rel == ".git" {
                continue;
            }
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                out.push(format!("{rel}/"));
                walk(root, &path, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

/// ADR-81's fitness function: with the plugin absent, `init -i` and its
/// re-run print, and write, exactly what they did before FR-IN-12.
#[test]
fn plugin_absent_init_i_output_is_byte_identical_to_the_pinned_golden() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("fixture-repo");
    let claude = tmp.path().join("claude");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&claude).unwrap();
    let shown = root.display().to_string();

    let first = logos_with_claude(&root, &claude, &["init", "-i", "--json"]);
    assert_eq!(exit_code(&first), 0, "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(
        String::from_utf8_lossy(&first.stdout).replace(&shown, "{ROOT}"),
        include_str!("fixtures/init_plugin_absent/stdout.json")
    );
    for (rel, golden) in ABSENT_GOLDEN {
        assert_eq!(&fs::read_to_string(root.join(rel)).unwrap(), golden, "{rel} differs from the pinned bytes");
    }
    let mut written: Vec<&str> = ABSENT_GOLDEN.iter().map(|(rel, _)| *rel).filter(|r| !r.starts_with(".logos")).collect();
    written.extend([".claude/", ".claude/hooks/"]);
    written.sort_unstable();
    assert_eq!(files_outside_logos(&root), written, "and nothing else is written, not even a directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join(".claude/hooks/logos-quality-open.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "the readout script stays executable");
    }

    let again = logos_with_claude(&root, &claude, &["init", "-i", "--json"]);
    assert_eq!(
        String::from_utf8_lossy(&again.stdout).replace(&shown, "{ROOT}"),
        include_str!("fixtures/init_plugin_absent/rerun.json")
    );
}

/// A committed repository with foreign content in every file `init -i`
/// touches — JSON already in the pretty shape `init` writes, so the tree can
/// be compared byte for byte after an uninstall.
fn repo_with_foreign_content(root: &Path) {
    fs::create_dir_all(root.join(".claude")).unwrap();
    fs::write(root.join("CLAUDE.md"), "# Team notes\n\nKeep this.\n").unwrap();
    fs::write(root.join(".claude/settings.json"), "{\n  \"model\": \"team-model\"\n}\n").unwrap();
    fs::write(
        root.join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    \"other\": {\n      \"command\": \"other\"\n    }\n  }\n}\n",
    )
    .unwrap();
    sh_git(root, &["init", "-q", "-b", "main"]);
    sh_git(root, &["add", "-A"]);
    sh_git(root, &["commit", "-q", "-m", "foreign content"]);
}

fn git_status(root: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(root).args(["status", "--porcelain"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn json_of(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)))
}

/// The acceptance path through the binary: a repository set up by today's
/// `init -i`, the plugin enabled at user scope, then `init -i` again — no
/// legacy entry, script or hook is left and `doctor` reports nothing. Before
/// the migration, `doctor` names the duplicate.
#[test]
fn plugin_enabled_init_i_migrates_a_legacy_repository_and_doctor_is_clean() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    let claude = tmp.path().join("claude");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&claude).unwrap();
    repo_with_foreign_content(&root);
    assert_eq!(exit_code(&logos_with_claude(&root, &claude, &["init", "-i", "--json"])), 0);

    fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"logos@logos":true}}"#).unwrap();
    let doctor = logos_with_claude(&root, &claude, &["doctor", "--json"]);
    assert_eq!(exit_code(&doctor), 0, "a finding is diagnostic, not a failure");
    let warnings = json_of(&doctor)["plugin_warnings"].clone();
    assert_eq!(warnings.as_array().map(Vec::len), Some(1), "{warnings}");
    assert!(warnings[0].as_str().unwrap().starts_with("duplicate registration"), "{warnings}");

    let migrated = logos_with_claude(&root, &claude, &["init", "-i", "--json"]);
    assert_eq!(exit_code(&migrated), 0, "{}", String::from_utf8_lossy(&migrated.stderr));
    let mcp: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).unwrap()).unwrap();
    assert!(mcp["mcpServers"].get("logos").is_none() && mcp["mcpServers"].get("other").is_some(), "{mcp}");
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".claude/settings.json")).unwrap()).unwrap();
    assert!(settings.get("hooks").is_none(), "no readout entry: {settings}");
    assert_eq!(settings["enabledPlugins"]["logos@logos"], true, "{settings}");
    assert_eq!(settings["model"], "team-model");
    assert!(!root.join(".claude/hooks").exists(), "no readout script");

    let doctor = json_of(&logos_with_claude(&root, &claude, &["doctor", "--json"]));
    assert_eq!(doctor["plugin_warnings"], serde_json::json!([]), "{doctor}");
    assert_eq!(doctor["hook_warnings"], serde_json::json!([]), "{doctor}");
}

/// `init --uninstall` leaves only the foreign content and `.logos/` — after
/// today's `init -i --hooks` and after a plugin-enabled one — and a second
/// run, spelled as the manual spells it (`--hooks --uninstall`), changes
/// nothing. `core.hooksPath` is unset.
#[test]
fn init_uninstall_leaves_only_foreign_content_and_a_second_run_is_a_no_op() {
    for plugin in [false, true] {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("repo");
        let claude = tmp.path().join("claude");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&claude).unwrap();
        if plugin {
            fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"logos@logos":true}}"#).unwrap();
        }
        repo_with_foreign_content(&root);
        assert_eq!(exit_code(&logos_with_claude(&root, &claude, &["init", "-i", "--hooks", "--json"])), 0);
        assert_ne!(git_status(&root), "?? .logos/\n", "init wrote into tracked files");

        let first = logos_with_claude(&root, &claude, &["init", "--uninstall", "--json"]);
        assert_eq!(exit_code(&first), 0, "{}", String::from_utf8_lossy(&first.stderr));
        assert_eq!(git_status(&root), "?? .logos/\n", "plugin={plugin}: {}", String::from_utf8_lossy(&first.stdout));
        assert!(root.join(".logos/logos.db").is_file(), ".logos/ data stays");
        let hooks_path = Command::new("git").arg("-C").arg(&root).args(["config", "core.hooksPath"]).output().unwrap();
        assert!(!hooks_path.status.success(), "core.hooksPath unset");

        let second = logos_with_claude(&root, &claude, &["init", "--hooks", "--uninstall", "--json"]);
        assert_eq!(exit_code(&second), 0);
        let steps = json_of(&second)["steps"].clone();
        assert!(
            steps.as_array().unwrap().iter().all(|s| s["action"] == "unchanged"),
            "a second run is a no-op: {steps}"
        );
        assert_eq!(git_status(&root), "?? .logos/\n");
    }
}

#[test]
fn init_uninstall_conflicts_with_workspace() {
    let tmp = TempDir::new().unwrap();
    assert_eq!(exit_code(&logos(tmp.path(), &["init", "--uninstall", "--workspace"])), 2);
}

/// With the plugin enabled for the project, `doctor` names both findings: the
/// `.mcp.json` duplicate and a binary older than the plugin's declared minimum
/// (read from `plugins/logos/.claude-plugin/plugin.json`) — and still exits 0.
#[test]
fn doctor_names_a_duplicate_registration_and_a_version_skew() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    assert_eq!(exit_code(&logos(&root, &["init"])), 0);
    write(&root, ".claude/settings.json", r#"{"enabledPlugins":{"logos@logos":true}}"#);
    write(&root, ".mcp.json", r#"{"mcpServers":{"logos":{"command":"logos","args":["serve","--mcp"]}}}"#);
    write(
        &root,
        "plugins/logos/.claude-plugin/plugin.json",
        r#"{"name":"logos","version":"999.0.0","metadata":{"minBinaryVersion":"999.0.0"}}"#,
    );

    let out = logos(&root, &["doctor", "--json"]);
    assert_eq!(exit_code(&out), 0, "{}", String::from_utf8_lossy(&out.stderr));
    let report = json_of(&out);
    let warnings: Vec<&str> = report["plugin_warnings"].as_array().unwrap().iter().filter_map(|w| w.as_str()).collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].starts_with("duplicate registration"), "{warnings:?}");
    assert!(
        warnings[1].starts_with("version skew") && warnings[1].contains(env!("CARGO_PKG_VERSION")) && warnings[1].contains("999.0.0"),
        "{warnings:?}"
    );
    assert_eq!(report["ok"], true, "the findings never move the verdict");
}
