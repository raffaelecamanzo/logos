//! [`Readouts`] — the quality readouts, reached by
//! [`Quality::readouts`](super::Quality::readouts) (S-829, [CR-259]): the
//! signal's history and structure (`evolution`, `dsm`, `doc_gaps`), the
//! read-only accessors of the last persisted snapshot (S-082, CR-018,
//! [ADR-28]) and the non-persisting session-start readout ([FR-IN-07]).
//!
//! The `latest_*` accessors are the non-persisting twins of the
//! evaluate-and-persist `scan`/`gate` paths, consumed **only** by the
//! [web-surface] Health, Overview and Metrics views so a dashboard GET
//! reflects the last persisted snapshot and never triggers an
//! evaluate-and-persist write ([FR-UI-03], [ADR-28]). The CLI/MCP `scan`/`gate`
//! methods keep persisting on purpose.
//!
//! [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
//! [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
//! [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
//! [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use anyhow::Result;

use super::Engine;
use crate::models::quality::{
    DocGapsReport, DsmReport, EvolutionReport, GateResult, LatestHealth, MetricSnapshot, QualityReadout, ScanResult,
};
use crate::observability::Tool;

/// The quality readouts over one engine, reached by
/// [`Quality::readouts`](super::Quality::readouts).
#[derive(Clone, Copy)]
pub struct Readouts<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Readouts<'e> {
    /// Signal evolution over stored snapshots (FR-GV-06): the most recent
    /// `limit` (default 30) `metric_snapshots` rows with per-metric deltas.
    /// Reports history — does not reconcile (BR-03).
    ///
    /// # Errors
    /// Returns an error for a transient engine or on a read failure.
    pub fn evolution(&self, limit: Option<u32>) -> Result<EvolutionReport> {
        crate::observability::traced(Tool::Evolution, || crate::governance::evolution(self.engine, limit))
    }

    /// Dependency structure matrix (FR-GV-07): cell `(i, j)` counts dep
    /// edges `i → j`; rows ordered by layer order then name; module
    /// granularity by default.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn dsm(
        &self,
        granularity: Option<crate::governance::DsmGranularity>,
        reconcile: bool,
    ) -> Result<DsmReport> {
        crate::observability::traced(Tool::Dsm, || {
            crate::governance::dsm(self.engine, granularity, reconcile)
        })
    }

    /// Documentation-gap analysis (FR-GV-14): exported functions/methods
    /// referenced by no `DocSection` over `DocReference` edges, capped at
    /// `limit` (default 50), always carrying the reference-presence caveat.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn doc_gaps(&self, limit: Option<u32>, reconcile: bool) -> Result<DocGapsReport> {
        crate::observability::traced(Tool::DocGaps, || {
            crate::governance::doc_gaps(self.engine, limit, reconcile)
        })
    }

    /// The most-recent **persisted** metric snapshot, or `None` on a
    /// never-`scan`-ned store — a pure read that computes and persists nothing
    /// ([ADR-28]). Backs the dashboard's Metrics/Health metric breakdown; on
    /// `None` the view renders the honest "run `logos scan`" empty state
    /// ([NFR-CC-04]).
    ///
    /// # Errors
    /// Returns an error only on a transient engine (no runtime) or a store-read
    /// failure — never from compute (there is none).
    pub fn latest_metrics(&self) -> Result<Option<MetricSnapshot>> {
        crate::observability::traced(Tool::LatestMetrics, || {
            Ok(crate::governance::latest_metrics(self.engine)?.map(|s| s.metrics))
        })
    }

    /// The read-only twin of [`scan`](super::Quality::scan): the last persisted snapshot's
    /// metric breakdown, the worst-offender lists persisted with it
    /// ([FR-QM-15]) and the read-only temporal tier, or `None` when no snapshot
    /// exists ([ADR-28]). Reads the last row — no reconcile, no score, no
    /// persist; carries no violations (those are not persisted on the snapshot).
    /// A snapshot written before the lists were persisted reports them as
    /// [`recorded: false`](crate::models::quality::WorstOffenders::recorded).
    ///
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    pub fn latest_scan(&self) -> Result<ScanResult> {
        crate::observability::traced(Tool::LatestScan, || crate::governance::latest_scan(self.engine))
    }

    /// The read-only gate **verdict** ([ADR-28]): compare the last persisted
    /// snapshot to the saved baseline without computing or persisting one. Backs
    /// the dashboard's Health verdict band; mirrors [`gate`](super::Quality::gate)'s
    /// comparison (BR-10) but never re-baselines or writes.
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    pub fn latest_gate(&self) -> Result<GateResult> {
        crate::observability::traced(Tool::LatestGate, || crate::governance::latest_gate(self.engine))
    }

    /// **One** read of the last persisted snapshot, projected into the Health
    /// bundle's gate verdict *and* its scan result ([FR-UI-04], [CR-135] §3.2) —
    /// the scan result carrying the worst-offender lists persisted with that
    /// same snapshot ([FR-QM-15]).
    ///
    /// The pair a caller that wants both fields must use.
    /// [`latest_gate`](Self::latest_gate) and [`latest_scan`](Self::latest_scan)
    /// remain for callers that want one, and project from this same seam — but
    /// calling *both* takes two reads of the same logical row in two
    /// transactions, and a `scan` committing between them yields a bundle whose
    /// two halves describe different generations with nothing in the payload
    /// saying so. This takes the read once, so that window does not exist.
    ///
    /// Like its two halves it computes nothing and persists nothing ([ADR-28]).
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    ///
    /// [FR-UI-04]: ../../../docs/specs/requirements/FR-UI-04.md
    /// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
    pub fn latest_health(&self) -> Result<LatestHealth> {
        crate::observability::traced(Tool::LatestHealth, || crate::governance::latest_health(self.engine))
    }

    /// **Every** offender the last snapshot recorded, uncapped, narrowed by a
    /// path glob and a dimension ([FR-QM-19]): each with its symbol id, the
    /// measure its value counts, the value and the threshold it met, in the
    /// order the capped lists use. Equality's entries, the functions above the
    /// mean complexity, are no offenders and are listed only when `filter`
    /// names their dimension. The single entry point behind the CLI
    /// `offenders` and the MCP `offenders` tool, so the two serialize one
    /// value ([NFR-CC-01]).
    ///
    /// A pure read of the last snapshot ([ADR-28]): no reconcile, no score, no
    /// write, so the signal, the gate and the capped lists are untouched and a
    /// rerun over an unchanged store is byte-identical ([NFR-RA-06]). With no
    /// snapshot, or one written before the uncapped list was persisted, the
    /// listing is `recorded: false` with the reason — never an empty "clean"
    /// list ([NFR-CC-04]).
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    ///
    /// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    pub fn offenders(
        &self,
        filter: &crate::governance::OffendersFilter,
    ) -> Result<crate::models::quality::OffenderListing> {
        crate::observability::traced(Tool::Offenders, || crate::governance::offender_listing(self.engine, filter))
    }

    /// The **"where to start"** list the last snapshot recorded ([FR-QM-17]): the
    /// ranked fix units, each with its estimated recoverable signal points, and
    /// the per-member outlier labels. The single entry point behind the CLI
    /// `improvements` and the MCP `improvements` tool; `GET /api/v1/health`
    /// carries the same value as `scan.where_to_start`, read through
    /// [`latest_health`](Self::latest_health) — so every surface serialises one
    /// value ([NFR-CC-01]).
    ///
    /// A pure read of the last snapshot ([ADR-28]): no reconcile, no score, no
    /// write, advisory and never gated ([BR-61]). A never-scanned store, or a
    /// snapshot written before the ranking was persisted, reads
    /// [`recorded: false`](crate::models::quality::WhereToStart::recorded) — never
    /// an empty "clean" list ([NFR-CC-04]); a recorded empty list means no single
    /// unit recovers a whole point.
    ///
    /// # Errors
    /// Returns an error only on a transient engine or a store-read failure.
    ///
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    /// [BR-61]: ../../../docs/specs/software-spec.md#311-quality-metrics
    /// [NFR-CC-01]: ../../../docs/specs/requirements/NFR-CC-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    pub fn improvements(&self) -> Result<crate::models::quality::WhereToStart> {
        crate::observability::traced(Tool::Improvements, || {
            Ok(crate::governance::latest_metrics(self.engine)?.map(|s| s.where_to_start).unwrap_or_default())
        })
    }

    /// The **non-persisting** quality readout for the report tier ([FR-IN-07],
    /// [CR-095]): a freshly computed signal, the blessed baseline and their
    /// delta, plus the last recorded `check_rules` findings — with **no write**.
    ///
    /// Distinct from both neighbours by design. [`gate`](super::Quality::gate) computes the
    /// same signal but *persists* it ([FR-GV-09]), which is wrong for a hook that
    /// fires at every session boundary: it would take the graph-DB write lock and
    /// grow the [FR-GV-06] `evolution` series on every `/clear`.
    /// [`latest_gate`](Self::latest_gate) writes nothing but reports the *last
    /// persisted* signal rather than computing one. This computes fresh and
    /// writes nothing.
    ///
    /// It never reconciles (a write), so `freshness` is always assumed-fresh, and
    /// its violations are the last recorded run's — see [`QualityReadout`] for
    /// why re-evaluating them read-only is not possible. Those findings are
    /// dated and attributed to the `HEAD` they were measured at from the
    /// [FR-GV-21] run marker, whose absence is reported as "no rule check has run"
    /// and never as a clean bill of health ([CR-096]).
    /// The message list is bounded by the rendering cap
    /// ([`governance::readout`](crate::governance::readout)); the true total
    /// always rides `violation_count`, so a truncated list can say what it
    /// dropped. The cap is not a caller's parameter: it is a property of the
    /// readout, and letting a surface choose it would let two surfaces disagree
    /// about the same read-model.
    ///
    /// # Errors
    /// Returns an error on a structural failure (transient engine, store fault,
    /// invalid `rules.toml`).
    ///
    /// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
    /// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
    /// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
    /// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
    pub fn readout(&self) -> Result<QualityReadout> {
        crate::observability::traced(Tool::QualityReadout, || {
            crate::governance::quality_readout(self.engine, crate::governance::readout::message_cap())
        })
    }

    /// The agent-host session-start hook payload ([FR-IN-07], [CR-095]): a
    /// [`readout`](Self::readout) rendered into the JSON object
    /// the host consumes — a user-visible `systemMessage` plus agent-visible
    /// `additionalContext`.
    ///
    /// Composed here rather than in the hook script because a malformed payload
    /// makes the host discard the readout entirely, and correct JSON escaping is
    /// `serde_json`'s job, not `sed`'s. Writes nothing, per
    /// [`readout`](Self::readout).
    ///
    /// # Errors
    /// Returns an error on a structural failure (transient engine, store fault,
    /// invalid `rules.toml`).
    ///
    /// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
    /// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
    pub fn report_hook_payload(&self) -> Result<crate::governance::readout::HookPayload> {
        Ok(crate::governance::readout::session_start_payload(
            &self.readout()?,
        ))
    }

    /// Materialize the Claude Code **session-start** quality-report hook
    /// ([FR-IN-07], [FR-GV-02], [FR-GV-05], [ADR-49], [CR-095], CLI-only
    /// surface): a marker-tagged hook script plus a non-clobbering merge into the
    /// project's **shared** `.claude/settings.json`, and a sweep of the retired
    /// SessionEnd entry and its orphaned script. When a session starts, resumes,
    /// or is reopened by `/clear`, the hook surfaces the current quality signal,
    /// the blessed baseline and their delta, and the last recorded rule
    /// violations as a non-blocking readout ([FR-GV-05]); `force` re-emits it.
    ///
    /// Pure local filesystem I/O behind `logos hook --emit [--force]` (the
    /// `init -i` step runs the same engine through [`crate::init`]). Installing
    /// the hook performs **no** LLM call and opens **no** network connection
    /// ([NFR-SE-01]) — it only shells out to `logos quality-report`, whose
    /// read-model is computed without persisting a snapshot
    /// ([`Self::readout`]).
    ///
    /// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    /// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
    /// [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
    /// [ADR-49]: ../../../docs/specs/architecture/decisions/ADR-49.md
    /// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
    pub fn report_hook_emit(&self, force: bool) -> Result<crate::hooks::HookEmitSummary> {
        crate::observability::traced(Tool::WikiQualityReportHookEmit, || {
            crate::hooks::materialize_quality_report_hook(&self.engine.root, force)
        })
    }
}
