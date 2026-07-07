//! The embedded wiki-generation skill and its materialization ([FR-WK-08],
//! [ADR-24], [CR-008]).
//!
//! The generation playbook ships **inside the binary** as a static asset (the
//! [FR-PL-04] embedding precedent — `include_str!`), so a Logos install is
//! wiki-functional out of the box with **zero network** ([NFR-SE-01]): only the
//! skill's *execution* (the LLM part) happens in the agent, never in this
//! binary. Materialization is pure local filesystem I/O plus the
//! [`std::os::unix::fs::symlink`] syscall — no outbound connections.
//!
//! `logos init -i` materializes the skill into the **canonical layout**
//! ([FR-IN-02] as modified): the content lives at `.agents/skills/logos-wiki/`
//! and a symlink `.claude/skills/logos-wiki` points to it, skip-if-present so a
//! re-run preserves local edits. `logos wiki skill --emit [dir] [--force]`
//! re-materializes for an existing install or after a binary upgrade — the
//! emitted skill carries its **source binary version** in a header line so an
//! aged skill is recognizable. Where symlink creation is unsupported, a copy is
//! written with a one-line notice.
//!
//! [FR-WK-08]: ../../../docs/specs/requirements/FR-WK-08.md
//! [FR-IN-02]: ../../../docs/specs/requirements/FR-IN-02.md
//! [FR-PL-04]: ../../../docs/specs/requirements/FR-PL-04.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [ADR-24]: ../../../docs/specs/architecture/decisions/ADR-24.md
//! [CR-008]: ../../../docs/requests/CR-008-wiki-store-and-serve.md

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// The skill's canonical content directory, repo-relative ([FR-WK-08]).
pub const SKILL_DIR_REL: &str = ".agents/skills/logos-wiki";

/// The `.claude/skills` symlink that points at [`SKILL_DIR_REL`], repo-relative.
pub const SKILL_LINK_REL: &str = ".claude/skills/logos-wiki";

/// The single skill file materialized under [`SKILL_DIR_REL`].
const SKILL_FILE: &str = "SKILL.md";

/// The embedded skill source, compiled into the binary ([FR-PL-04] precedent).
/// Carries a `{{LOGOS_VERSION}}` placeholder substituted at materialization time
/// with the source binary version ([FR-WK-08] binary-version header).
const SKILL_TEMPLATE: &str = include_str!("skill/SKILL.md");

/// The placeholder the version header substitutes — kept in one place so the
/// asset and the renderer can never disagree.
const VERSION_PLACEHOLDER: &str = "{{LOGOS_VERSION}}";

/// The materialized `SKILL.md` bytes: the embedded asset with its binary-version
/// header resolved to this build's version ([FR-WK-08]).
///
/// Both the writer and the idempotence/force checks compare against this single
/// rendering, so "the materialized content equals the embedded asset including
/// the binary-version header" ([UAT-WK-04]) is one source of truth.
pub fn rendered_skill() -> String {
    SKILL_TEMPLATE.replace(VERSION_PLACEHOLDER, env!("CARGO_PKG_VERSION"))
}

/// What materialization did to the skill ([FR-WK-08]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EmitAction {
    /// No skill was present; the content + link were written.
    Created,
    /// A skill was already present and `--force` re-materialized the embedded
    /// content, overwriting local edits.
    Forced,
    /// A skill was already present and `--force` was not given — left untouched,
    /// preserving local edits (skip-if-present, [FR-IN-02] idempotence).
    Skipped,
}

/// How the `.claude/skills/logos-wiki` pointer was realized ([FR-WK-08]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkKind {
    /// A symlink to the canonical directory (the preferred layout).
    Symlink,
    /// A directory copy — the fallback where symlink creation is unsupported.
    Copy,
}

/// The outcome of materializing the embedded skill ([FR-WK-08]) — a `Serialize`
/// read-model the CLI surface renders and `init` folds into its step list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EmitSummary {
    /// The canonical content directory, repo-relative.
    pub skill_dir: String,
    /// The `.claude/skills` pointer, repo-relative.
    pub link: String,
    /// What happened to the skill.
    pub action: EmitAction,
    /// How the pointer was realized — `None` when nothing was (re)written
    /// (the skip-if-present case).
    pub link_kind: Option<LinkKind>,
    /// The source binary version stamped into the emitted skill.
    pub version: &'static str,
    /// A one-line notice when the symlink fell back to a copy; else `None`.
    pub notice: Option<String>,
}

/// Materialize the embedded skill under `base` ([FR-WK-08]).
///
/// Writes `<base>/.agents/skills/logos-wiki/SKILL.md` (the binary-version header
/// resolved) and points `<base>/.claude/skills/logos-wiki` at it via a relative
/// symlink, copying with a notice where symlinks are unsupported.
///
/// **Skip-if-present:** when the canonical directory already exists and `force`
/// is false, nothing is written — local edits are preserved ([FR-IN-02]
/// idempotence). With `force`, the embedded content is restored and the pointer
/// re-created.
///
/// Pure local filesystem I/O — no network ([NFR-SE-01]).
///
/// # Errors
/// Returns an error only when a Logos-owned path cannot be created or written.
pub fn materialize(base: &Path, force: bool) -> Result<EmitSummary> {
    let skill_dir = base.join(SKILL_DIR_REL);
    let link = base.join(SKILL_LINK_REL);
    let present = skill_dir.exists();

    if present && !force {
        return Ok(EmitSummary {
            skill_dir: SKILL_DIR_REL.to_string(),
            link: SKILL_LINK_REL.to_string(),
            action: EmitAction::Skipped,
            link_kind: None,
            version: env!("CARGO_PKG_VERSION"),
            notice: None,
        });
    }

    fs::create_dir_all(&skill_dir)
        .with_context(|| format!("creating the skill directory {}", skill_dir.display()))?;
    let skill_file = skill_dir.join(SKILL_FILE);
    fs::write(&skill_file, rendered_skill())
        .with_context(|| format!("writing {}", skill_file.display()))?;

    let (link_kind, notice) = install_link(&skill_dir, &link)?;

    tracing::info!(
        skill_dir = SKILL_DIR_REL,
        link = SKILL_LINK_REL,
        ?link_kind,
        forced = present,
        "wiki generation skill materialized"
    );

    Ok(EmitSummary {
        skill_dir: SKILL_DIR_REL.to_string(),
        link: SKILL_LINK_REL.to_string(),
        action: if present {
            EmitAction::Forced
        } else {
            EmitAction::Created
        },
        link_kind: Some(link_kind),
        version: env!("CARGO_PKG_VERSION"),
        notice,
    })
}

/// (Re)create the `.claude/skills/logos-wiki` pointer at `link`, targeting
/// `skill_dir` ([FR-WK-08]).
///
/// Prefers a **relative** symlink (`../../.agents/skills/logos-wiki`) so the
/// install survives the project being moved. Any existing pointer (symlink or a
/// prior copy) is removed first so `--force` re-materializes cleanly. On a
/// filesystem where symlink creation fails, the canonical directory is copied
/// and a one-line notice is returned ([FR-WK-08] copy fallback).
fn install_link(skill_dir: &Path, link: &Path) -> Result<(LinkKind, Option<String>)> {
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {} for the skill symlink", parent.display()))?;
    }
    remove_existing(link)?;

    // The target is expressed relative to the link's parent so a moved project
    // keeps a valid pointer: from `.claude/skills/` up two levels to the root,
    // then down into `.agents/skills/logos-wiki`.
    let rel_target = Path::new("../..").join(SKILL_DIR_REL);
    match symlink(&rel_target, link) {
        Ok(()) => Ok((LinkKind::Symlink, None)),
        Err(_) => {
            copy_dir(skill_dir, link)?;
            Ok((
                LinkKind::Copy,
                Some(format!(
                    "symlinks are unsupported here — wrote a copy at {SKILL_LINK_REL}; \
                     after a binary upgrade refresh both with `logos wiki skill --emit --force`"
                )),
            ))
        }
    }
}

/// Remove a pre-existing pointer, whether it is a symlink/file or a copied
/// directory, so a re-emit starts from a clean slot. A missing path is fine.
fn remove_existing(link: &Path) -> Result<()> {
    match fs::symlink_metadata(link) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(link)
            .with_context(|| format!("removing the existing copy at {}", link.display())),
        Ok(_) => fs::remove_file(link)
            .with_context(|| format!("removing the existing pointer at {}", link.display())),
        Err(_) => Ok(()),
    }
}

/// Create a symlink `link -> target`. Unix uses the real syscall; other
/// platforms always error so [`install_link`] takes the copy fallback.
#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks are not supported on this platform",
    ))
}

/// Recursively copy `from` into `to` — the symlink-unsupported fallback. The
/// skill is a flat directory of files today; the recursion keeps the fallback
/// correct if it ever gains subdirectories.
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    for entry in fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)
                .with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn rendered_skill_resolves_the_version_header() {
        let rendered = rendered_skill();
        assert!(
            !rendered.contains(VERSION_PLACEHOLDER),
            "the version placeholder must be substituted"
        );
        assert!(
            rendered.contains(env!("CARGO_PKG_VERSION")),
            "the rendered skill carries this build's version"
        );
        // Frontmatter must stay first so the skill is discoverable.
        assert!(
            rendered.starts_with("---\n"),
            "frontmatter stays at the top"
        );
        assert!(
            rendered.contains("name: logos-wiki"),
            "the skill is named logos-wiki"
        );
    }

    /// [FR-WK-24] AC: "the embedded skill's Overview guidance instructs a
    /// user-facing page (reader goals, behavior, workflows) and scopes the
    /// symbol-centric template to code-level fallback pages" — asserted against
    /// the actual rendered asset, not just by human reading of the markdown.
    /// Anchored on stable heading/phrase text rather than a full-body snapshot so
    /// prose polish elsewhere in the file cannot spuriously break this test.
    #[test]
    fn rendered_skill_gives_overview_pages_a_user_facing_template_scoped_from_code_level() {
        let rendered = rendered_skill();
        assert!(
            rendered.contains("Overview page template — write for the reader"),
            "a dedicated user-facing Overview template section exists"
        );
        assert!(
            rendered.contains("this is not a symbol tour"),
            "the Overview template explicitly rejects the symbol-tour framing"
        );
        assert!(
            rendered.contains("Page structure — code-level pages"),
            "the prior generic template is retitled to scope it away from Overview pages"
        );
        assert!(
            rendered.contains(
                "For a page about a specific symbol, module, or component — **not** an Overview"
            ),
            "the symbol-centric template is scoped to code-level pages, explicitly excluding Overview pages"
        );
    }

    /// [FR-WK-24] AC (strengthened by [CR-064]): dogfooding the shipped tier
    /// found the Overview/Summary pages still read too concise and
    /// code-technical — the template must tell the agent to open with a
    /// concrete reader outcome before any structural scaffolding, and to name
    /// the actual commands/workflows the grounding docs document rather than
    /// paraphrase them into generic capability language.
    #[test]
    fn rendered_skill_overview_template_leads_with_outcome_and_names_concrete_commands() {
        let rendered = rendered_skill();
        assert!(
            rendered.contains("Open with a concrete reader outcome"),
            "the template instructs opening with a concrete reader outcome before any structural detail"
        );
        assert!(
            rendered.contains("Quote the actual command, subcommand, or workflow name"),
            "the template instructs naming/quoting the actual commands or workflows the grounding docs \
             document, not paraphrasing them into generic capability language"
        );
    }

    /// [S-263]/[FR-WK-20]/[FR-WK-21] AC: the embedded skill instructs Summary-only
    /// scope in SRS mode and never to touch the binary-owned presented Design/Specs
    /// pages, and it documents the `wiki materialize` command that owns them —
    /// asserted against the actual rendered asset (CR-062).
    #[test]
    fn rendered_skill_scopes_case_one_to_summary_only_and_documents_materialize() {
        let rendered = rendered_skill();
        assert!(
            rendered.contains("wiki_materialize"),
            "the MCP twin inventory names wiki_materialize"
        );
        assert!(
            rendered.contains("Presented tier — Logos owns it, you never write it or touch it."),
            "the presented tier is called out as a distinct, binary-owned tier"
        );
        assert!(
            rendered.contains("never** hand-author"),
            "the presented tier explicitly forbids hand-authoring its pages"
        );
        assert!(
            rendered.contains(
                "you never author the consolidated ADRs/Components/Requirements/UAT pages or the\n  Architecture page in that mode"
            ),
            "Case-1 agent scope is restricted to Summary-only"
        );
    }

    #[test]
    fn materialize_writes_canonical_layout_with_a_symlink() {
        let tmp = TempDir::new().unwrap();
        let summary = materialize(tmp.path(), false).unwrap();

        assert_eq!(summary.action, EmitAction::Created);
        let skill_file = tmp.path().join(SKILL_DIR_REL).join(SKILL_FILE);
        assert_eq!(fs::read_to_string(&skill_file).unwrap(), rendered_skill());

        // The pointer resolves to the canonical content.
        let link = tmp.path().join(SKILL_LINK_REL);
        let resolved = fs::read_to_string(link.join(SKILL_FILE)).unwrap();
        assert_eq!(resolved, rendered_skill());

        #[cfg(unix)]
        {
            assert_eq!(summary.link_kind, Some(LinkKind::Symlink));
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "the pointer is a symlink on unix"
            );
            assert!(summary.notice.is_none());
        }
    }

    #[test]
    fn second_materialize_skips_and_preserves_edits() {
        let tmp = TempDir::new().unwrap();
        materialize(tmp.path(), false).unwrap();

        let skill_file = tmp.path().join(SKILL_DIR_REL).join(SKILL_FILE);
        fs::write(&skill_file, "LOCAL EDIT").unwrap();

        let summary = materialize(tmp.path(), false).unwrap();
        assert_eq!(summary.action, EmitAction::Skipped);
        assert_eq!(summary.link_kind, None);
        assert_eq!(
            fs::read_to_string(&skill_file).unwrap(),
            "LOCAL EDIT",
            "an unforced re-emit preserves the local edit"
        );
    }

    #[test]
    fn force_restores_the_embedded_content() {
        let tmp = TempDir::new().unwrap();
        materialize(tmp.path(), false).unwrap();

        let skill_file = tmp.path().join(SKILL_DIR_REL).join(SKILL_FILE);
        fs::write(&skill_file, "LOCAL EDIT").unwrap();

        let summary = materialize(tmp.path(), true).unwrap();
        assert_eq!(summary.action, EmitAction::Forced);
        assert_eq!(
            fs::read_to_string(&skill_file).unwrap(),
            rendered_skill(),
            "--force restores the embedded asset"
        );
    }

    /// `--force` over a copy-fallback install (a real directory at the link
    /// path, not a symlink) must replace it cleanly rather than nesting.
    #[test]
    fn force_replaces_a_copy_fallback_pointer() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join(SKILL_DIR_REL);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join(SKILL_FILE), "old").unwrap();
        // Simulate a prior copy fallback: a real directory at the link path.
        let link = tmp.path().join(SKILL_LINK_REL);
        fs::create_dir_all(&link).unwrap();
        fs::write(link.join(SKILL_FILE), "stale copy").unwrap();

        let summary = materialize(tmp.path(), true).unwrap();
        assert_eq!(summary.action, EmitAction::Forced);
        // The pointer now resolves to the fresh embedded content (no nesting).
        assert_eq!(
            fs::read_to_string(link.join(SKILL_FILE)).unwrap(),
            rendered_skill()
        );
        assert!(!link.join(SKILL_LINK_REL).exists(), "no nested re-copy");
    }
}
