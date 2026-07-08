//! End-to-end tests for the full `init` experience (S-023, FR-IN-01..04).
//!
//! Everything runs through the public façade ([`Engine::init`] /
//! [`Engine::init_with`]) on throwaway roots, pinning the DL-07 contract:
//! idempotent, non-clobbering, managed blocks.

use std::fs;
use std::path::Path;
use std::process::Command;

use logos_core::config::{Config, Rules};
use logos_core::init::InitOptions;
use logos_core::models::pipeline::{InitAction, InitResult, InitStep};
use logos_core::Engine;
use tempfile::TempDir;

/// The full `-i` step set minus hooks — the non-TTY `init -i` default.
fn interactive() -> InitOptions {
    InitOptions {
        inject_mcp: true,
        write_claude_md: true,
        install_hooks: false,
        materialize_skill: true,
        install_quality_report_hook: true,
    }
}

fn step<'a>(result: &'a InitResult, target: &str) -> &'a InitStep {
    result
        .steps
        .iter()
        .find(|s| s.target == target)
        .unwrap_or_else(|| panic!("no step for target {target:?}: {:?}", result.steps))
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

fn git(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git binary available")
}

fn git_init(root: &Path) {
    assert!(git(root, &["init", "-q"]).status.success(), "git init");
}

// ── FR-IN-01 / UAT-IN-01: policy templates, idempotent + non-clobbering ──

#[test]
fn plain_init_creates_policy_files_gitignore_and_store() {
    let tmp = TempDir::new().unwrap();
    let result = Engine::init(tmp.path()).expect("init succeeds");

    for rel in [
        ".logos/config.toml",
        ".logos/rules.toml",
        ".logos/.gitignore",
        ".logos/logos.db",
    ] {
        assert!(tmp.path().join(rel).exists(), "{rel} must exist after init");
    }
    for target in [
        ".logos/config.toml",
        ".logos/rules.toml",
        ".logos/.gitignore",
    ] {
        assert_eq!(
            step(&result, target).action,
            InitAction::Created,
            "fresh init creates {target}"
        );
    }

    // Plain init must NOT touch host-integration targets (FR-IN-02 is -i only).
    assert!(!tmp.path().join(".mcp.json").exists());
    assert!(!tmp.path().join("CLAUDE.md").exists());

    // The generated templates are pure-default starters: they must parse and
    // deserialize to the exact built-in defaults (FR-CF-01 alignment).
    let config = logos_core::config::load_config(&tmp.path().join(".logos/config.toml"))
        .expect("generated config.toml parses");
    assert_eq!(config, Config::default(), "template == built-in defaults");
    let rules = logos_core::config::load_rules(&tmp.path().join(".logos/rules.toml"))
        .expect("generated rules.toml parses");
    assert_eq!(rules, Rules::default(), "template == built-in defaults");
}

#[test]
fn re_init_never_clobbers_edited_policy_files() {
    let tmp = TempDir::new().unwrap();
    Engine::init(tmp.path()).unwrap();

    let custom_config = "languages = [\"rust\"]\n";
    let custom_rules = "[constraints]\nmax_cycles = 0\n";
    fs::write(tmp.path().join(".logos/config.toml"), custom_config).unwrap();
    fs::write(tmp.path().join(".logos/rules.toml"), custom_rules).unwrap();

    let result = Engine::init(tmp.path()).expect("re-init succeeds");

    assert_eq!(read(tmp.path(), ".logos/config.toml"), custom_config);
    assert_eq!(read(tmp.path(), ".logos/rules.toml"), custom_rules);
    assert_eq!(
        step(&result, ".logos/config.toml").action,
        InitAction::Unchanged
    );
    assert_eq!(
        step(&result, ".logos/rules.toml").action,
        InitAction::Unchanged
    );
}

// ── FR-IN-04: the generated .gitignore and its managed block ──────────────

#[test]
fn gitignore_ignores_derived_state_and_keeps_policy_tracked() {
    let tmp = TempDir::new().unwrap();
    git_init(tmp.path());
    Engine::init(tmp.path()).unwrap();

    let gitignore = read(tmp.path(), ".logos/.gitignore");
    for pattern in [
        // All five stores are covered (FR-IN-04 as modified by CR-008/CR-006;
        // chat.db added by S-168/CR-045 — the ui-gated conversation store).
        "logos.db*",
        "telemetry.db*",
        "history.db*",
        "wiki.db*",
        "chat.db*",
        "baseline.json",
        "history.jsonl",
        "logs/",
        // S-169 / FR-CF-06: the chat API key store is gitignored — the one
        // non-derived file that must never be committed (NFR-SE-07).
        "secrets.toml",
    ] {
        assert!(
            gitignore.contains(pattern),
            ".gitignore must ignore {pattern}"
        );
    }

    // Ground truth from git itself: derived state ignored, policy tracked.
    let check = |rel: &str| {
        git(tmp.path(), &["check-ignore", "-q", rel])
            .status
            .success()
    };
    assert!(check(".logos/logos.db"), "logos.db ignored");
    assert!(check(".logos/logos.db-wal"), "WAL sidecar ignored (.db-*)");
    assert!(
        check(".logos/telemetry.db-shm"),
        "telemetry sidecars ignored"
    );
    assert!(check(".logos/wiki.db"), "wiki.db ignored");
    assert!(check(".logos/chat.db"), "chat.db ignored");
    assert!(check(".logos/chat.db-wal"), "chat WAL sidecar ignored (.db-*)");
    assert!(check(".logos/history.db-wal"), "history sidecars ignored");
    // S-169 / FR-CF-06: the secret store is git-ignored, never committed.
    assert!(check(".logos/secrets.toml"), "secrets.toml ignored");
    assert!(!check(".logos/config.toml"), "config.toml stays tracked");
    assert!(!check(".logos/rules.toml"), "rules.toml stays tracked");
    assert!(
        !check(".logos/.gitignore"),
        ".gitignore itself stays tracked"
    );
}

#[test]
fn gitignore_managed_block_refreshes_and_preserves_user_lines() {
    let tmp = TempDir::new().unwrap();
    Engine::init(tmp.path()).unwrap();

    // The user appends an ignore of their own and tampers inside the block.
    let original = read(tmp.path(), ".logos/.gitignore");
    let tampered = original.replace("telemetry.db*\n", "") + "my-custom-ignore\n";
    fs::write(tmp.path().join(".logos/.gitignore"), &tampered).unwrap();

    let result = Engine::init(tmp.path()).unwrap();

    let refreshed = read(tmp.path(), ".logos/.gitignore");
    assert!(refreshed.contains("telemetry.db*"), "managed body restored");
    assert!(
        refreshed.contains("my-custom-ignore"),
        "user line preserved"
    );
    assert_eq!(
        step(&result, ".logos/.gitignore").action,
        InitAction::Updated
    );

    // And a second run with nothing to do reports Unchanged.
    let again = Engine::init(tmp.path()).unwrap();
    assert_eq!(
        step(&again, ".logos/.gitignore").action,
        InitAction::Unchanged
    );
}

#[test]
fn unbalanced_gitignore_markers_are_skipped_untouched() {
    let tmp = TempDir::new().unwrap();
    Engine::init(tmp.path()).unwrap();

    // The user deletes the end marker: the block is structurally malformed —
    // init must refuse to guess (DL-07), leaving the file byte-identical.
    let truncated = read(tmp.path(), ".logos/.gitignore")
        .lines()
        .filter(|l| !l.starts_with("# logos:managed:end"))
        .map(|l| format!("{l}\n"))
        .collect::<String>();
    fs::write(tmp.path().join(".logos/.gitignore"), &truncated).unwrap();

    let result = Engine::init(tmp.path()).expect("init still succeeds");

    let s = step(&result, ".logos/.gitignore");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(!s.detail.is_empty(), "skip carries a reason");
    assert_eq!(
        read(tmp.path(), ".logos/.gitignore"),
        truncated,
        "malformed file left byte-identical"
    );
}

// ── FR-IN-02 / UAT-IN-02: .mcp.json injection — idempotent, non-clobbering ─

#[test]
fn interactive_init_injects_the_mcp_server_block_idempotently() {
    let tmp = TempDir::new().unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(step(&result, ".mcp.json").action, InitAction::Created);

    let parsed: serde_json::Value =
        serde_json::from_str(&read(tmp.path(), ".mcp.json")).expect("valid JSON");
    assert_eq!(parsed["mcpServers"]["logos"]["command"], "logos");
    assert_eq!(
        parsed["mcpServers"]["logos"]["args"],
        serde_json::json!(["serve", "--mcp"])
    );

    // Re-run: byte-identical file, no duplicate block (UAT-IN-02).
    let before = read(tmp.path(), ".mcp.json");
    let again = Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(read(tmp.path(), ".mcp.json"), before);
    assert_eq!(step(&again, ".mcp.json").action, InitAction::Unchanged);
}

#[test]
fn mcp_injection_preserves_other_servers_and_an_existing_logos_entry() {
    let tmp = TempDir::new().unwrap();

    // A host config that already wires another server.
    fs::write(
        tmp.path().join(".mcp.json"),
        r#"{ "mcpServers": { "other": { "command": "other-bin" } } }"#,
    )
    .unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&read(tmp.path(), ".mcp.json")).unwrap();
    assert_eq!(parsed["mcpServers"]["other"]["command"], "other-bin");
    assert_eq!(parsed["mcpServers"]["logos"]["command"], "logos");
    assert_eq!(step(&result, ".mcp.json").action, InitAction::Updated);

    // A custom `logos` entry is the user's: skip, never clobber (FR-IN-02).
    fs::write(
        tmp.path().join(".mcp.json"),
        r#"{ "mcpServers": { "logos": { "command": "my-wrapper" } } }"#,
    )
    .unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&read(tmp.path(), ".mcp.json")).unwrap();
    assert_eq!(parsed["mcpServers"]["logos"]["command"], "my-wrapper");
    assert_eq!(step(&result, ".mcp.json").action, InitAction::Unchanged);
    assert!(
        step(&result, ".mcp.json").detail.contains("already"),
        "detail names the skip reason"
    );
}

#[test]
fn malformed_mcp_json_is_skipped_never_clobbered() {
    let tmp = TempDir::new().unwrap();
    let malformed = "{ this is not json";
    fs::write(tmp.path().join(".mcp.json"), malformed).unwrap();

    let result = Engine::init_with(tmp.path(), &interactive()).expect("init still succeeds");

    assert_eq!(read(tmp.path(), ".mcp.json"), malformed, "file untouched");
    let s = step(&result, ".mcp.json");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(!s.detail.is_empty(), "skip carries a reason");
}

#[test]
fn mcp_json_with_non_object_mcp_servers_is_skipped() {
    let tmp = TempDir::new().unwrap();
    // Valid JSON, but `mcpServers` is an array — a shape we cannot extend
    // safely; the dedicated skip branch must refuse without rewriting.
    let original = r#"{ "mcpServers": [] }"#;
    fs::write(tmp.path().join(".mcp.json"), original).unwrap();

    let result = Engine::init_with(tmp.path(), &interactive()).expect("init still succeeds");

    let s = step(&result, ".mcp.json");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(
        s.detail.contains("mcpServers"),
        "detail names the offending shape: {}",
        s.detail
    );
    assert_eq!(read(tmp.path(), ".mcp.json"), original, "file untouched");
}

// ── FR-IN-02: managed CLAUDE.md block ──────────────────────────────────────

#[test]
fn claude_md_managed_block_is_created_and_user_content_preserved() {
    let tmp = TempDir::new().unwrap();

    // The user already has a CLAUDE.md — init must append, not replace.
    fs::write(tmp.path().join("CLAUDE.md"), "# My project rules\n").unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();

    let claude = read(tmp.path(), "CLAUDE.md");
    assert!(
        claude.starts_with("# My project rules\n"),
        "user content first"
    );
    assert!(claude.contains("<!-- logos:managed:begin -->"));
    assert!(claude.contains("<!-- logos:managed:end -->"));
    assert!(claude.contains("context"), "carries the graph-first steer");
    assert_eq!(step(&result, "CLAUDE.md").action, InitAction::Updated);

    // Idempotent: nothing changes on re-run.
    let before = claude;
    let again = Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(read(tmp.path(), "CLAUDE.md"), before);
    assert_eq!(step(&again, "CLAUDE.md").action, InitAction::Unchanged);

    // A tampered managed body is regenerated; user content survives.
    let tampered = before.replace("context", "CONTEXT-GONE");
    fs::write(tmp.path().join("CLAUDE.md"), &tampered).unwrap();
    let third = Engine::init_with(tmp.path(), &interactive()).unwrap();
    let refreshed = read(tmp.path(), "CLAUDE.md");
    assert!(refreshed.starts_with("# My project rules\n"));
    assert!(refreshed.contains("context"), "managed body restored");
    assert_eq!(step(&third, "CLAUDE.md").action, InitAction::Updated);
}

#[test]
fn fresh_claude_md_is_created_when_absent() {
    let tmp = TempDir::new().unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(step(&result, "CLAUDE.md").action, InitAction::Created);
    assert!(read(tmp.path(), "CLAUDE.md").contains("<!-- logos:managed:begin -->"));
}

// ── FR-IN-03 / FR-SY-05: git hooks via core.hooksPath ─────────────────────

fn hook_opts() -> InitOptions {
    InitOptions {
        install_hooks: true,
        ..InitOptions::default()
    }
}

#[test]
fn hooks_install_sets_hooks_path_and_writes_sync_hooks() {
    let tmp = TempDir::new().unwrap();
    git_init(tmp.path());

    let result = Engine::init_with(tmp.path(), &hook_opts()).unwrap();
    assert_eq!(step(&result, ".logos/hooks").action, InitAction::Created);

    let hooks_path = git(tmp.path(), &["config", "core.hooksPath"]);
    assert_eq!(
        String::from_utf8_lossy(&hooks_path.stdout).trim(),
        ".logos/hooks"
    );

    for hook in ["post-commit", "post-checkout", "post-merge"] {
        let path = tmp.path().join(".logos/hooks").join(hook);
        let body = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{hook}: {e}"));
        assert!(
            body.contains("logos sync"),
            "{hook} triggers sync (FR-SY-05)"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "{hook} must be executable");
        }
    }

    // Idempotent re-run.
    let again = Engine::init_with(tmp.path(), &hook_opts()).unwrap();
    assert_eq!(step(&again, ".logos/hooks").action, InitAction::Unchanged);
}

#[test]
fn a_foreign_hooks_path_is_never_overwritten() {
    let tmp = TempDir::new().unwrap();
    git_init(tmp.path());
    assert!(git(tmp.path(), &["config", "core.hooksPath", ".husky"])
        .status
        .success());

    let result = Engine::init_with(tmp.path(), &hook_opts()).unwrap();

    let s = step(&result, ".logos/hooks");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(
        s.detail.contains(".husky"),
        "skip reason names the conflict"
    );
    let hooks_path = git(tmp.path(), &["config", "core.hooksPath"]);
    assert_eq!(String::from_utf8_lossy(&hooks_path.stdout).trim(), ".husky");
}

#[test]
fn a_non_managed_hook_file_blocks_the_install() {
    let tmp = TempDir::new().unwrap();
    git_init(tmp.path());

    // A user-authored hook (no `logos:managed` marker) at one of our target
    // paths vetoes the whole install (DL-07) — even though core.hooksPath is
    // still unset.
    let foreign = "#!/bin/sh\nmy own hook\n";
    fs::create_dir_all(tmp.path().join(".logos/hooks")).unwrap();
    fs::write(tmp.path().join(".logos/hooks/post-commit"), foreign).unwrap();

    let result = Engine::init_with(tmp.path(), &hook_opts()).expect("init still succeeds");

    let s = step(&result, ".logos/hooks");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(
        s.detail.contains("post-commit"),
        "skip reason names the file: {}",
        s.detail
    );
    assert_eq!(
        read(tmp.path(), ".logos/hooks/post-commit"),
        foreign,
        "user hook untouched"
    );
    let hooks_path = git(tmp.path(), &["config", "core.hooksPath"]);
    assert!(
        !hooks_path.status.success(),
        "core.hooksPath stays unset on veto"
    );
}

#[test]
fn deleted_hook_file_triggers_updated_on_reinstall() {
    let tmp = TempDir::new().unwrap();
    git_init(tmp.path());
    Engine::init_with(tmp.path(), &hook_opts()).unwrap();

    fs::remove_file(tmp.path().join(".logos/hooks/post-commit")).unwrap();

    let result = Engine::init_with(tmp.path(), &hook_opts()).unwrap();

    assert_eq!(step(&result, ".logos/hooks").action, InitAction::Updated);
    assert!(
        read(tmp.path(), ".logos/hooks/post-commit").contains("logos sync"),
        "deleted hook regenerated"
    );
}

#[test]
fn hooks_outside_a_git_repository_skip_gracefully() {
    let tmp = TempDir::new().unwrap();
    let result = Engine::init_with(tmp.path(), &hook_opts()).expect("init still succeeds");
    let s = step(&result, ".logos/hooks");
    assert_eq!(s.action, InitAction::Skipped);
    assert!(!s.detail.is_empty());
}

// ── FR-WK-08 / UAT-WK-04: the embedded wiki-generation skill, materialized ──

/// `init -i` lays down the canonical layout: `.agents/skills/logos-wiki/` holds
/// the content, `.claude/skills/logos-wiki` resolves to it, and the content
/// equals the embedded asset including the binary-version header (UAT-WK-04).
#[test]
fn init_i_materializes_the_wiki_skill_in_the_canonical_layout() {
    let tmp = TempDir::new().unwrap();
    let result = Engine::init_with(tmp.path(), &interactive()).unwrap();

    let skill_dir = tmp.path().join(logos_core::wiki::SKILL_DIR_REL);
    let link = tmp.path().join(logos_core::wiki::SKILL_LINK_REL);
    assert!(skill_dir.join("SKILL.md").exists(), "canonical dir exists");

    // Content equals the embedded asset, version header resolved.
    let expected = logos_core::wiki::rendered_skill();
    assert_eq!(
        read(tmp.path(), ".agents/skills/logos-wiki/SKILL.md"),
        expected
    );
    assert!(expected.contains("name: logos-wiki"), "frontmatter present");
    assert!(
        !expected.contains("{{LOGOS_VERSION}}"),
        "the version header is resolved"
    );

    // The pointer resolves to the same content (symlink on unix, copy elsewhere).
    assert_eq!(fs::read_to_string(link.join("SKILL.md")).unwrap(), expected);
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        ".claude/skills/logos-wiki is a symlink on unix"
    );

    let s = step(&result, ".agents/skills/logos-wiki");
    assert_eq!(s.action, InitAction::Created);
}

/// A second `init -i` skips the skill, preserving a local edit (FR-IN-02
/// idempotence); plain `init` (no `-i`) never materializes it.
#[test]
fn second_init_i_skips_the_skill_preserving_edits() {
    let tmp = TempDir::new().unwrap();
    Engine::init_with(tmp.path(), &interactive()).unwrap();

    let skill_file = tmp.path().join(".agents/skills/logos-wiki/SKILL.md");
    fs::write(&skill_file, "LOCAL EDIT").unwrap();

    let again = Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(
        step(&again, ".agents/skills/logos-wiki").action,
        InitAction::Unchanged
    );
    assert_eq!(
        fs::read_to_string(&skill_file).unwrap(),
        "LOCAL EDIT",
        "the unforced re-run preserves the local edit"
    );
}

/// Plain `init` (no host-integration steps) leaves the skill unmaterialized —
/// it rides the `-i` integration only.
#[test]
fn plain_init_does_not_materialize_the_skill() {
    let tmp = TempDir::new().unwrap();
    Engine::init(tmp.path()).unwrap();
    assert!(
        !tmp.path().join(".agents/skills/logos-wiki").exists(),
        "no skill without -i"
    );
}

/// `Engine::wiki_skill_emit` is the standalone refresh path behind
/// `logos wiki skill --emit [dir] [--force]`: unforced skips an existing
/// install; `--force` restores the embedded content (UAT-WK-04).
#[test]
fn wiki_skill_emit_skip_and_force_round_trip() {
    let tmp = TempDir::new().unwrap();
    let engine = Engine::open(tmp.path());

    let created = engine.wiki_skill_emit(None, false).unwrap();
    assert_eq!(created.action, logos_core::wiki::EmitAction::Created);

    let skill_file = tmp.path().join(".agents/skills/logos-wiki/SKILL.md");
    fs::write(&skill_file, "LOCAL EDIT").unwrap();

    let skipped = engine.wiki_skill_emit(None, false).unwrap();
    assert_eq!(skipped.action, logos_core::wiki::EmitAction::Skipped);
    assert_eq!(fs::read_to_string(&skill_file).unwrap(), "LOCAL EDIT");

    let forced = engine.wiki_skill_emit(None, true).unwrap();
    assert_eq!(forced.action, logos_core::wiki::EmitAction::Forced);
    assert_eq!(
        fs::read_to_string(&skill_file).unwrap(),
        logos_core::wiki::rendered_skill(),
        "--force restores the embedded content"
    );
}

/// `--emit [dir]` targets an explicit base directory, not the project root.
#[test]
fn wiki_skill_emit_honours_an_explicit_dir() {
    let tmp = TempDir::new().unwrap();
    let sub = tmp.path().join("elsewhere");
    fs::create_dir_all(&sub).unwrap();

    Engine::open(tmp.path())
        .wiki_skill_emit(Some(&sub), false)
        .unwrap();

    assert!(
        sub.join(".agents/skills/logos-wiki/SKILL.md").exists(),
        "skill materialized under the explicit dir"
    );
    assert!(
        !tmp.path().join(".agents/skills/logos-wiki").exists(),
        "the project root is untouched"
    );
}

// ── CR-070 / FR-WK-14 retirement: the PostToolUse augmentation hook is gone ──

/// [CR-070] regression: `init -i` installs no PostToolUse augmentation entry
/// and writes no `logos-wiki-augment.sh` — only the embedded skill and the
/// [FR-IN-07] SessionEnd quality-report hook install.
#[test]
fn init_i_installs_no_augmentation_hook() {
    let tmp = TempDir::new().unwrap();
    Engine::init_with(tmp.path(), &interactive()).unwrap();

    assert!(
        !tmp.path().join(".claude/hooks/logos-wiki-augment.sh").exists(),
        "the retired augmentation hook script is never materialized"
    );
    let settings: serde_json::Value =
        serde_json::from_str(&read(tmp.path(), ".claude/settings.json")).expect("settings JSON");
    assert!(
        settings["hooks"]["PostToolUse"].is_null(),
        "no PostToolUse entry is installed: {settings}"
    );
}

/// [CR-047] / [FR-WK-16] retirement regression: `init -i` never writes the
/// per-developer `.claude/settings.local.json` (the file the retired SessionEnd
/// autogen hook alone used to merge into) and no artifact anywhere under the
/// project references a `claude -p` invocation or the retired autogen script.
#[test]
fn init_i_installs_no_autogen_hook_and_no_claude_p_reference_remains() {
    let tmp = TempDir::new().unwrap();
    Engine::init_with(tmp.path(), &interactive()).unwrap();

    assert!(
        !tmp.path().join(".claude/settings.local.json").exists(),
        "init -i never creates the per-developer settings.local.json — \
         the retired autogen hook was its only writer"
    );
    assert!(
        !tmp.path().join(".claude/hooks/logos-wiki-autogen.sh").exists(),
        "the retired autogen hook script is never materialized"
    );
    let settings = read(tmp.path(), ".claude/settings.json");
    assert!(
        !settings.contains("claude -p") && !settings.contains("logos-wiki-autogen"),
        "no claude -p invocation and no autogen reference remain: {settings}"
    );
}

// ── FR-IN-07 / FR-GV-05 / FR-GV-09 / ADR-49: the SessionEnd quality-report hook ──

/// Read the **shared** `.claude/settings.json` SessionEnd array, asserting
/// exactly one entry wires our quality-report script.
fn assert_one_managed_quality_report_hook(root: &Path) {
    let settings: serde_json::Value =
        serde_json::from_str(&read(root, ".claude/settings.json")).expect("settings JSON");
    let end = settings["hooks"]["SessionEnd"]
        .as_array()
        .expect("SessionEnd array");
    let ours: Vec<_> = end
        .iter()
        .filter(|e| {
            e["hooks"].as_array().is_some_and(|hs| {
                hs.iter().any(|h| {
                    h["command"]
                        .as_str()
                        .is_some_and(|c| c.contains("logos-quality-report.sh"))
                })
            })
        })
        .collect();
    assert_eq!(ours.len(), 1, "exactly one managed quality-report entry: {settings}");
}

/// `init -i` installs the SessionEnd quality-report hook default-on: the
/// marker-tagged script plus a SessionEnd entry in `.claude/settings.json`
/// (FR-IN-07, ADR-49).
#[test]
fn init_i_installs_the_quality_report_hook() {
    let tmp = TempDir::new().unwrap();
    Engine::init_with(tmp.path(), &interactive()).unwrap();

    let script = tmp.path().join(".claude/hooks/logos-quality-report.sh");
    assert!(script.exists(), "the quality-report hook script is written");
    let body = read(tmp.path(), ".claude/hooks/logos-quality-report.sh");
    assert!(
        body.contains("logos:quality-report:managed"),
        "the script carries its managed marker"
    );
    // The documented off-switch env var (FR-IN-07).
    assert!(
        body.contains("LOGOS_QUALITY_REPORT_DISABLE"),
        "the script honors the documented off-switch env var"
    );
    // Report-only: it always exits 0 and runs check + scan (+ gate for baseline).
    assert!(body.trim_end().ends_with("exit 0"), "always exits 0 — never blocks teardown");
    assert!(body.contains("logos scan --json") && body.contains("logos check"));

    assert_one_managed_quality_report_hook(tmp.path());
}

/// Two-run idempotency: a second `init -i` leaves the already-present
/// quality-report SessionEnd entry unchanged and the shared `settings.json`
/// byte-identical (FR-IN-07 idempotent, non-clobbering).
#[test]
fn second_init_i_leaves_the_quality_report_hook_unchanged() {
    let tmp = TempDir::new().unwrap();
    Engine::init_with(tmp.path(), &interactive()).unwrap();
    let before = read(tmp.path(), ".claude/settings.json");

    Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(
        read(tmp.path(), ".claude/settings.json"),
        before,
        "the shared settings.json is byte-identical on a re-run"
    );
    assert_one_managed_quality_report_hook(tmp.path());
}

/// Plain `init` (no `-i`) installs no quality-report hook — it rides `-i` only.
#[test]
fn plain_init_does_not_install_the_quality_report_hook() {
    let tmp = TempDir::new().unwrap();
    Engine::init(tmp.path()).unwrap();
    assert!(
        !tmp.path().join(".claude/hooks/logos-quality-report.sh").exists(),
        "no quality-report script without -i"
    );
}

/// Foreign-settings preservation: `init -i` merges the quality-report SessionEnd
/// entry into an existing `.claude/settings.json` while preserving a foreign
/// SessionEnd entry and unrelated keys (FR-IN-07 non-clobbering).
#[test]
fn init_i_quality_report_preserves_foreign_settings_entries() {
    let tmp = TempDir::new().unwrap();
    let settings = tmp.path().join(".claude/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"permissions":{"allow":["Bash"]},"hooks":{"SessionEnd":[{"hooks":[{"type":"command","command":"their-cleanup.sh"}]}]}}"#,
    )
    .unwrap();

    Engine::init_with(tmp.path(), &interactive()).unwrap();
    let merged: serde_json::Value =
        serde_json::from_str(&read(tmp.path(), ".claude/settings.json")).expect("settings JSON");

    let end = merged["hooks"]["SessionEnd"].as_array().unwrap();
    assert_eq!(end.len(), 2, "the user's SessionEnd hook survives alongside ours");
    assert!(end
        .iter()
        .any(|e| e["hooks"][0]["command"] == "their-cleanup.sh"));
    assert_eq!(
        merged["permissions"]["allow"][0], "Bash",
        "unrelated keys are preserved verbatim"
    );
    assert_one_managed_quality_report_hook(tmp.path());
}

/// A foreign (unparseable) `.claude/settings.json` is never overwritten by the
/// quality-report merge either — the file is left byte-identical (FR-IN-07).
#[test]
fn init_i_quality_report_never_overwrites_a_foreign_settings_file() {
    let tmp = TempDir::new().unwrap();
    let settings = tmp.path().join(".claude/settings.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let foreign = "{ this is not valid json";
    fs::write(&settings, foreign).unwrap();

    Engine::init_with(tmp.path(), &interactive()).unwrap();
    assert_eq!(
        read(tmp.path(), ".claude/settings.json"),
        foreign,
        "the foreign file is untouched by the quality-report merge"
    );
}
