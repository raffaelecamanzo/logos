//! [`Review`] — the branch review ([FR-GV-24], [FR-GV-28], [FR-GV-29]), reached
//! by [`Quality::review`](super::Quality::review) (S-829, [CR-259]): the branch
//! delta, the shared-budget projection and the headroom under every budget.
//!
//! [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
//! [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
//! [FR-GV-29]: ../../../docs/specs/requirements/FR-GV-29.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use anyhow::Result;

use super::{today_utc, Engine};
use crate::observability::Tool;

/// The branch review over one engine, reached by
/// [`Quality::review`](super::Quality::review).
#[derive(Clone, Copy)]
pub struct Review<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Review<'e> {
    /// The **branch delta** ([FR-GV-24]): every offender and every `check`
    /// violation, classified `new`, `worsened`, `improved`, `resolved` or
    /// `unchanged` per symbol against the merge-base of `HEAD` and `base` (the
    /// repository's default branch when `None`), each with its value before
    /// and after, its threshold and its language's trust label, plus the
    /// near-clone groups the branch introduced. The single entry point behind
    /// the CLI `review` and the MCP `review` tool ([NFR-CC-01]).
    ///
    /// The before-state is the snapshot recorded at the merge-base, else a
    /// shadow index of that tree cached by commit under `.logos/review-cache/`
    /// — never another commit ([ADR-82]). The after-state is the graph as last
    /// synced, its derived annotations re-materialised under the current
    /// contract as `check` does; nothing is reconciled and no snapshot or
    /// violation set is recorded, so `check` and `gate` are untouched and a
    /// rerun is byte-identical ([NFR-RA-06]). A
    /// base that does not resolve answers `measured: false` with the reason.
    ///
    /// # Errors
    /// Returns an error on a transient engine, a store fault, an invalid
    /// `rules.toml`, or a failed shadow index.
    ///
    /// [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
    pub fn delta(&self, base: Option<&str>) -> Result<crate::models::quality::BranchReview> {
        crate::observability::traced(Tool::Review, || {
            crate::governance::branch_review(self.engine, base, |merge_base| self.security_evidence(merge_base))
        })
    }

    /// The **shared-budget projection** ([FR-GV-28], S-757): each of
    /// `branches`' deltas against `base` (the default branch when `None`),
    /// summed and projected onto every declared `[constraints]` budget and a
    /// numeric `[review] drift_budget`, listing each limit the combined merge
    /// would exceed with every branch's share. Report only. The single entry
    /// point behind `logos review --branches` and the MCP `review` tool's
    /// `branches` ([NFR-CC-01]).
    ///
    /// Every side is a committed tree, shadow-indexed once per commit and
    /// cached under `.logos/review-cache/` ([ADR-82]); the live graph and
    /// `HEAD` are not measured, so a rerun is byte-identical ([NFR-RA-06]). A
    /// branch that cannot be measured is named and left out of the sum.
    ///
    /// # Errors
    /// Returns an error on a transient engine, a store fault or an invalid
    /// `rules.toml`.
    ///
    /// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
    pub fn branches(
        &self,
        base: Option<&str>,
        branches: &[String],
    ) -> Result<crate::models::quality::BranchesProjection> {
        crate::observability::traced(Tool::Review, || crate::governance::branches_review(self.engine, base, branches))
    }

    /// The room left under every budget [`branches`](Self::branches)
    /// projects ([FR-GV-29], S-798): each subject's value on the base commit,
    /// its allowance and its headroom, and the margin under a numeric `[review]
    /// drift_budget`. The single entry point behind `logos review --headroom`
    /// and the MCP `review` tool's `headroom` ([NFR-CC-01]).
    ///
    /// The base and its measurement are `branches`'s — the same commit,
    /// the same cache entry — and neither the working tree nor `HEAD` is
    /// measured, so a rerun is byte-identical ([NFR-RA-06]). `subjects` are
    /// project-relative paths and symbol names; `near` keeps the subjects
    /// whose headroom is at most that.
    ///
    /// # Errors
    /// Returns an error on a transient engine, a store fault or an invalid
    /// `rules.toml`.
    ///
    /// [FR-GV-29]: ../../../docs/specs/requirements/FR-GV-29.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    pub fn headroom(
        &self,
        base: Option<&str>,
        subjects: &[String],
        near: Option<u64>,
    ) -> Result<crate::models::quality::HeadroomReport> {
        crate::observability::traced(Tool::Review, || crate::governance::headroom_review(self.engine, base, subjects, near))
    }

    /// The security evidence the branch review classifies ([FR-SC-18], S-686):
    /// every stored finding of each scanner's latest report as
    /// [`Findings::list`](super::Findings::list) lists it unfiltered (its
    /// suppression, OWASP categories and graph context), each scanner's latest
    /// snapshot, and the reports recorded at `merge_base` with their categories
    /// and context. Read here so governance never opens `history.db`; the
    /// merge-base tree is never scanned. Untraced: it is part of `review`.
    ///
    /// [FR-SC-18]: ../../../docs/specs/requirements/FR-SC-18.md
    fn security_evidence(&self, merge_base: &str) -> Result<crate::governance::SecurityEvidence> {
        // Every code scope, and no scope asked for: the delta keeps test code
        // as any other finding ([BR-98]).
        let filter = crate::security::FindingsFilter::default();
        let current = crate::security::findings(
            &self.engine.root,
            &self.engine.security().tools().environment(),
            self.engine.security().tools().manifest(),
            &filter,
            today_utc(),
            crate::security::no_test_symbols,
            |listed| crate::security::context::contexts(self.engine, listed),
        )?;
        let latest = crate::history::security::latest_snapshots(&self.engine.root)?;
        let base = crate::history::security::read_at(&self.engine.root, merge_base)?;
        let base_owasp = crate::history::security::classify(&base.findings, filter.edition, &[] as &[&str])
            .findings
            .into_iter()
            .map(|c| c.categories)
            .collect();
        let base_contexts = crate::security::context::contexts(self.engine, &base.findings.iter().collect::<Vec<_>>())?;
        Ok(crate::governance::SecurityEvidence { current, latest, base, base_owasp, base_contexts })
    }
}
