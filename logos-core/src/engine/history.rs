//! [`History`] — the git-history surfaces ([FR-GH-02]..[FR-GH-09], [ADR-22]),
//! reached by [`Engine::history`] (S-829, [CR-259]): the lazy mining trigger,
//! the temporal report and the hotspot board, each with its read-only twin
//! over the last-mined facts ([ADR-28], [CR-018]).
//!
//! All of them are deliberately **off** the gate/sync/navigation paths
//! ([BR-26]): those paths run no history mining and no git subprocess beyond
//! the existing HEAD tag.
//!
//! [FR-GH-02]: ../../../docs/specs/requirements/FR-GH-02.md
//! [FR-GH-09]: ../../../docs/specs/requirements/FR-GH-09.md
//! [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
//! [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
//! [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
//! [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use anyhow::Result;

use super::{today_utc, Engine};
use crate::observability::Tool;

/// The git-history surfaces over one engine, reached by [`Engine::history`].
#[derive(Clone, Copy)]
pub struct History<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> History<'e> {
    /// The **lazy** git-history mining trigger ([FR-GH-02], [ADR-22]): resolve
    /// the effective `[history]` config, then mine `history.db` incrementally.
    ///
    /// This is the seam the temporal surfaces ([S-048]) call on a read — and the
    /// *only* path that mines. It is deliberately **absent** from
    /// `gate`/`session_start`/`session_end` ([super::Quality::gate]), `sync` ([super::Pipeline::sync]),
    /// and every navigation method, so those paths run no history mining and no
    /// git subprocess beyond the existing HEAD tag ([BR-26], [FR-GH-02]
    /// acceptance).
    ///
    /// It is `pub` — the api-facade-facing library seam, returning the
    /// [`MineOutcome`](crate::history::MineOutcome) `Serialize` read-model per the
    /// [ADR-01] Engine convention. It is **not** an MCP tool: tools are
    /// registered explicitly in the `mcp` crate, so the `logos:*` tool-set is
    /// unchanged until [S-048] wires the surface.
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure or invalid
    /// `rules.toml`; the expected degraded states ride inside the
    /// [`MineOutcome`](crate::history::MineOutcome) ([NFR-RA-05]).
    ///
    /// [FR-GH-02]: ../../../docs/specs/requirements/FR-GH-02.md
    /// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
    /// [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
    pub fn ensure_mined(&self) -> Result<crate::history::MineOutcome> {
        crate::history::mine(&self.engine.root, &self.config()?)
    }

    /// The temporal-tier read seam ([FR-GH-03]..[FR-GH-05], [FR-GH-09]): lazily
    /// mine (via the same [`ensure_mined`](Self::ensure_mined)
    /// trigger), then compute the per-file temporal metrics and append a snapshot
    /// row, returning the [`TemporalReport`](crate::history::TemporalReport)
    /// `Serialize` read-model ([ADR-01]).
    ///
    /// Like `ensure_mined` this is `pub` (the api-facade-facing library
    /// seam the hotspot surface [S-048] ranks from) and deliberately **off** the
    /// gate/sync/navigation paths ([BR-26]). Output is a pure function of the tree
    /// at HEAD + the effective `[history]` config ([BR-27], [NFR-RA-06]); a
    /// degraded repository rides inside the report as `n/a`, never an error
    /// ([NFR-RA-05]). The `logos:*` MCP tool-set is unchanged until [S-048] wires
    /// the surface.
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure, an invalid
    /// `rules.toml`, or a non-compiling `[history] defect_patterns`.
    ///
    /// [FR-GH-03]: ../../../docs/specs/requirements/FR-GH-03.md
    /// [FR-GH-05]: ../../../docs/specs/requirements/FR-GH-05.md
    /// [FR-GH-09]: ../../../docs/specs/requirements/FR-GH-09.md
    /// [BR-27]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [S-048]: ../../../docs/planning/journal.md#s-048-hotspot-ranking-and-temporal-reporting-surfaces
    pub fn temporal_report(&self) -> Result<crate::history::TemporalReport> {
        crate::history::temporal_report(&self.engine.root, &self.config()?)
    }

    /// The effective `[history]` configuration of this root's `rules.toml`,
    /// which every temporal read mines and computes under.
    ///
    /// # Errors
    /// An invalid `rules.toml`.
    fn config(&self) -> Result<crate::config::EffectiveHistory> {
        Ok(crate::config::load_rules_from_root(&self.engine.root)?.history.effective())
    }

    /// The hotspot surface ([FR-GH-06]): rank indexed files by
    /// `churn rank × structural-complexity rank` — a **Rust-side join** of the
    /// temporal tier's churn (`history.db`, [`temporal_report`](Self::temporal_report))
    /// against per-file aggregated cyclomatic complexity (`logos.db`), never an
    /// SQL `ATTACH` across the two stores ([ADR-22], [BR-26]). `limit` caps the
    /// returned board (`logos hotspots --limit N`).
    ///
    /// The CLI `hotspots` subcommand and the MCP `hotspots` tool both reach this
    /// through [`hotspots_with_findings`](Self::hotspots_with_findings), of which
    /// this is the filter-off form ([NFR-CC-01]) — they share its `Serialize`
    /// read-model byte-for-byte ([FR-GH-06] CLI/MCP parity). Like
    /// [`temporal_report`](Self::temporal_report) it is lazy and **off** the
    /// gate/sync/navigation paths ([BR-26]); a degraded repository rides inside
    /// the report as `n/a` + notice, never an error ([FR-GH-08], [NFR-RA-05]).
    ///
    /// `production_scope = true` applies the optional production-scope filter
    /// ([CR-076]): whole test files (`is_test`-only, [FR-AN-05]) are dropped
    /// from the candidate set before ranking. `false` (the default) is
    /// byte-identical to the pre-[CR-076] whole-repo board — the filter is
    /// opt-in and never moves the gate ([BR-26]).
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure, an invalid
    /// `rules.toml`, a non-compiling `[history] defect_patterns`, or — once a
    /// security report is stored — an invalid `.logos/security-ignore.toml`
    /// (exit 2).
    ///
    /// [FR-GH-06]: ../../../docs/specs/requirements/FR-GH-06.md
    /// [FR-GH-08]: ../../../docs/specs/requirements/FR-GH-08.md
    /// [ADR-22]: ../../../docs/specs/architecture/decisions/ADR-22.md
    /// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    /// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
    pub fn hotspots(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
    ) -> Result<crate::history::HotspotReport> {
        self.hotspots_with_findings(limit, untested, production_scope, false)
    }

    /// [`hotspots`](Self::hotspots) with the findings filter ([FR-SC-14]):
    /// `with_findings = true` ranks the hotspots carrying a fresh unsuppressed
    /// security finding ahead of those without, highest severity first. Every
    /// board carries the findings column when a scan is stored, with or without
    /// the filter.
    ///
    /// # Errors
    /// As [`hotspots`](Self::hotspots).
    ///
    /// [FR-SC-14]: ../../../docs/specs/requirements/FR-SC-14.md
    pub fn hotspots_with_findings(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
        with_findings: bool,
    ) -> Result<crate::history::HotspotReport> {
        self.board(false, limit, untested, production_scope, with_findings)
    }

    /// The hotspot board behind [`hotspots_with_findings`](Self::hotspots_with_findings)
    /// and, with `latest`, [`latest_hotspots_with_findings`](Self::latest_hotspots_with_findings):
    /// the churn axis read from the temporal tier — mined lazily and computed
    /// per file, or with `latest` the last-mined facts, neither mined nor
    /// appended — ranked by [`rank_hotspots`](Self::rank_hotspots), under its
    /// surface's own span.
    fn board(
        &self,
        latest: bool,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
        with_findings: bool,
    ) -> Result<crate::history::HotspotReport> {
        let tool = if latest { Tool::LatestHotspots } else { Tool::Hotspots };
        crate::observability::traced(tool, || {
            let temporal = if latest { self.latest_temporal_report()? } else { self.temporal_report()? };
            self.rank_hotspots(temporal, limit, untested, production_scope, with_findings)
        })
    }

    /// Join a temporal report with the complexity, coverage and findings axes
    /// into the ranked [`HotspotReport`](crate::history::HotspotReport). The
    /// complexity (`logos.db`), coverage and findings (`history.db`) reads are
    /// pure point queries —
    /// this is the shared, **read-only** core of both [`hotspots`](Self::hotspots)
    /// (which mines first) and [`latest_hotspots`](Self::latest_hotspots) (which
    /// reads the last-mined facts). Never an `ATTACH` across the two stores
    /// ([ADR-22], [BR-26], [BR-28]).
    ///
    /// `production_scope` gates the optional whole-test-file exclusion
    /// ([CR-076](../../../docs/requests/CR-076-hotspots-production-scope-filter.md)):
    /// the `is_test` verdict ([FR-AN-05]) is read from the same `nodes`/
    /// `functions` rows already fetched for the complexity axis, so enabling
    /// the filter costs one extra point query (`test_node_ids`), never a new
    /// store read shape.
    fn rank_hotspots(
        &self,
        temporal: crate::history::TemporalReport,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
        with_findings: bool,
    ) -> Result<crate::history::HotspotReport> {
        // Complexity axis: per-file Σ cyclomatic_complexity from logos.db,
        // read in Rust — joined here, never via a cross-DB ATTACH (BR-26).
        // A transient engine with no graph runtime contributes no
        // complexity, so every file is honestly excluded (NFR-RA-05). The
        // same rows feed the production-scope candidate-set filter (CR-076).
        let (complexity, test_only_files) = match self.engine.runtime() {
            Some(rt) => {
                let (nodes, functions, test_node_ids) = rt.submit_read(|store| {
                    Ok((
                        store.all_nodes()?,
                        store.function_metrics()?,
                        store.test_node_ids()?,
                    ))
                })?;
                let complexity = crate::history::aggregate_complexity(&nodes, &functions);
                let test_ids: std::collections::HashSet<_> = test_node_ids.into_iter().collect();
                let test_only_files = crate::history::test_only_files(&nodes, &functions, &test_ids);
                (complexity, test_only_files)
            }
            None => (
                std::collections::BTreeMap::new(),
                std::collections::BTreeSet::new(),
            ),
        };
        // Coverage axis ([FR-CV-07]): the latest snapshot, freshness-resolved
        // against the current tree. `None` = no coverage ingested → the
        // labeled static-reachability fallback ([FR-GV-08]). Read from
        // `history.db` only — never an `ATTACH` to `logos.db` ([BR-28]).
        let coverage = crate::history::coverage::read_latest(&self.engine.root)?.map(|view| {
            view.files
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        crate::history::FileCoverage {
                            fresh: f.fresh,
                            coverage_bp: f.coverage_bp().unwrap_or(0),
                        },
                    )
                })
                .collect()
        });
        // Findings axis ([FR-SC-14]): counted findings per path (fresh per finding,
        // unsuppressed, production code: `history::security::counts`) from the
        // security read model and the suppression labeller, each finding's code
        // scope by this graph ([FR-SC-28]). `None` = no report stored → no
        // findings column.
        let findings = crate::history::path_findings(&self.engine.root, today_utc(), |stored| {
            crate::security::context::code_scopes(self.engine, stored)
        })?;
        Ok(crate::history::rank_with_findings(
            temporal,
            &complexity,
            coverage.as_ref(),
            limit,
            untested,
            production_scope,
            &test_only_files,
            crate::history::FindingsOverlay { by_path: findings.as_ref(), with_findings },
        ))
    }

    /// The read-only twin of [`temporal_report`](Self::temporal_report): recompute
    /// the per-file temporal metrics from the **already-mined** facts at the
    /// stored cursor, **without mining and without appending a snapshot**
    /// ([ADR-28], [CR-018]). Backs the dashboard's Commits view (and the Health
    /// temporal tier) so a GET reflects the last `logos hotspots`/`scan` mine.
    ///
    /// # Errors
    /// Returns an error only on an unexpected store-read failure or a
    /// non-compiling `[history] defect_patterns`.
    ///
    /// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
    pub fn latest_temporal_report(&self) -> Result<crate::history::TemporalReport> {
        crate::observability::traced(Tool::LatestTemporalReport, || {
            crate::history::latest_temporal_report(&self.engine.root, &self.config()?)
        })
    }

    /// The read-only twin of [`hotspots`](Self::hotspots): rank the
    /// last-mined temporal facts (via [`latest_temporal_report`](Self::latest_temporal_report))
    /// against the complexity and coverage axes, **without** mining or appending
    /// a snapshot ([ADR-28], [CR-018]). Backs the dashboard's Hotspots view (and
    /// the Coverage view's untested-hotspots join) so a GET never writes. Shares
    /// the optional `production_scope` filter ([CR-076]) with [`hotspots`](Self::hotspots).
    ///
    /// # Errors
    /// Returns an error only on an unexpected git/store failure, a
    /// non-compiling `[history] defect_patterns`, or — once a security report is
    /// stored — an invalid `.logos/security-ignore.toml` (exit 2).
    ///
    /// [CR-076]: ../../../docs/requests/CR-076-hotspots-production-scope-filter.md
    pub fn latest_hotspots(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
    ) -> Result<crate::history::HotspotReport> {
        self.latest_hotspots_with_findings(limit, untested, production_scope, false)
    }

    /// [`latest_hotspots`](Self::latest_hotspots) with the findings filter
    /// ([FR-SC-14]), the read-only twin of
    /// [`hotspots_with_findings`](Self::hotspots_with_findings).
    ///
    /// # Errors
    /// As [`latest_hotspots`](Self::latest_hotspots).
    ///
    /// [FR-SC-14]: ../../../docs/specs/requirements/FR-SC-14.md
    pub fn latest_hotspots_with_findings(
        &self,
        limit: Option<usize>,
        untested: bool,
        production_scope: bool,
        with_findings: bool,
    ) -> Result<crate::history::HotspotReport> {
        self.board(true, limit, untested, production_scope, with_findings)
    }
}
