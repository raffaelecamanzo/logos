//! The `init -i` wiki-skill step and its standalone refresh path ([FR-WK-08],
//! UAT-WK-04), moved from `logos-core/tests/init.rs` with the skill (S-644).
//!
//! The step is no longer a core `init` step: it is the [`SkillInit`]
//! contribution the CLI supplies to [`Engine::init_with_contributions`], so these
//! tests supply it the same way. Everything else is as it was in the core.
//!
//! [FR-WK-08]: ../../docs/specs/requirements/FR-WK-08.md

use std::fs;
use std::path::Path;
use std::sync::Arc;

use logos_core::init::{InitContribution, InitOptions};
use logos_core::models::pipeline::{InitAction, InitResult, InitStep};
use logos_core::Engine;
use logos_wiki::{SkillInit, WikiEngine};
use tempfile::TempDir;

/// The full `-i` step set minus hooks — the non-TTY `init -i` default.
fn interactive() -> InitOptions {
    InitOptions {
        inject_mcp: true,
        write_claude_md: true,
        install_hooks: false,
        materialize_skill: true,
        install_quality_report_hook: true,
        workspace_member: false,
    }
}

/// `init` as the CLI runs it: the core steps plus the wiki's contributed step.
fn init_with_skill(root: &Path, options: &InitOptions) -> anyhow::Result<InitResult> {
    let contributions: Vec<Arc<dyn InitContribution>> = vec![Arc::new(SkillInit)];
    Engine::init_with_contributions(root, options, &contributions)
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

/// `init -i` lays down the canonical layout: `.agents/skills/logos-wiki/` holds
/// the content, `.claude/skills/logos-wiki` resolves to it, and the content
/// equals the embedded asset including the binary-version header (UAT-WK-04).
#[test]
fn init_i_materializes_the_wiki_skill_in_the_canonical_layout() {
    let tmp = TempDir::new().unwrap();
    let result = init_with_skill(tmp.path(), &interactive()).unwrap();

    let skill_dir = tmp.path().join(logos_wiki::SKILL_DIR_REL);
    let link = tmp.path().join(logos_wiki::SKILL_LINK_REL);
    assert!(skill_dir.join("SKILL.md").exists(), "canonical dir exists");

    // Content equals the embedded asset, version header resolved.
    let expected = logos_wiki::rendered_skill();
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
    init_with_skill(tmp.path(), &interactive()).unwrap();

    let skill_file = tmp.path().join(".agents/skills/logos-wiki/SKILL.md");
    fs::write(&skill_file, "LOCAL EDIT").unwrap();

    let again = init_with_skill(tmp.path(), &interactive()).unwrap();
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
    init_with_skill(tmp.path(), &InitOptions::default()).unwrap();
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
    assert_eq!(created.action, logos_core::hooks::EmitAction::Created);

    let skill_file = tmp.path().join(".agents/skills/logos-wiki/SKILL.md");
    fs::write(&skill_file, "LOCAL EDIT").unwrap();

    let skipped = engine.wiki_skill_emit(None, false).unwrap();
    assert_eq!(skipped.action, logos_core::hooks::EmitAction::Skipped);
    assert_eq!(fs::read_to_string(&skill_file).unwrap(), "LOCAL EDIT");

    let forced = engine.wiki_skill_emit(None, true).unwrap();
    assert_eq!(forced.action, logos_core::hooks::EmitAction::Forced);
    assert_eq!(
        fs::read_to_string(&skill_file).unwrap(),
        logos_wiki::rendered_skill(),
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
