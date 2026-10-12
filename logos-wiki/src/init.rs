//! The `init -i` wiki-skill step, contributed to the core's `init` ([FR-DS-04],
//! [ADR-74], S-644).
//!
//! It was a core step until the wiki left `logos-core`; the core cannot name
//! this crate (it would be a dependency cycle), so the step reaches `init`
//! through the core-owned [`InitContribution`] seam instead, and the surface
//! that runs `init` supplies it.
//!
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md

use std::path::Path;

use anyhow::Result;
use logos_core::hooks::EmitAction;
use logos_core::init::{InitContribution, InitOptions};
use logos_core::models::pipeline::{InitAction, InitStep};

use crate::LinkKind;

/// Materialize the embedded wiki-generation skill into the canonical layout,
/// skip-if-present (FR-IN-02 as modified by CR-008, [FR-WK-08]), when
/// [`InitOptions::materialize_skill`] asks for it — the `-i` step. Delegates to
/// [`crate::materialize_skill`], the sole owner of the skill asset, and maps
/// the [`crate::EmitSummary`] onto an [`InitStep`]. A plain `init` gets no row.
///
/// [FR-WK-08]: ../../docs/specs/requirements/FR-WK-08.md
#[derive(Debug, Clone, Copy, Default)]
pub struct SkillInit;

impl InitContribution for SkillInit {
    fn run(&self, root: &Path, options: &InitOptions) -> Result<Vec<InitStep>> {
        if !options.materialize_skill {
            return Ok(Vec::new());
        }
        // `init -i` never clobbers local edits: unforced, so an existing skill skips.
        let summary = crate::materialize_skill(root, false)?;
        let action = match summary.action {
            EmitAction::Created => InitAction::Created,
            // The skill's skip-if-present means an unforced re-emit never writes, so
            // `Reconciled` is unreachable here — mapped alongside `Forced` because
            // both are writes, rather than left to a catch-all that would silently
            // absorb a future variant (CR-095).
            EmitAction::Forced | EmitAction::Reconciled => InitAction::Updated,
            EmitAction::Skipped => InitAction::Unchanged,
        };
        let detail = match (summary.action, summary.link_kind, &summary.notice) {
            (EmitAction::Skipped, _, _) => {
                "already present — never overwritten; `logos wiki skill --emit --force` refreshes"
                    .to_string()
            }
            (_, _, Some(notice)) => notice.clone(),
            (_, Some(LinkKind::Symlink), None) => format!("{} → {}", summary.link, summary.skill_dir),
            _ => String::new(),
        };
        Ok(vec![InitStep {
            target: summary.skill_dir,
            action,
            detail,
        }])
    }
}
