//! [`Quality`] — the quality and governance evaluations (S-020), reached by
//! [`Engine::quality`] (S-829, [CR-259]), with the persisted-snapshot
//! [`Readouts`](super::Readouts) and the branch [`Review`](super::Review)
//! beneath it.
//!
//! All of them are guaranteed-fresh aggregate evaluations ([ADR-11],
//! NFR-DM-02): each reconciles the working tree first (O(changed),
//! FR-RC-01/02) unless told not to (FR-RC-04), and every result carries the
//! FR-RC-03 freshness line. Unlike the infallible navigation surface, these
//! return `Result` — a structural failure (store fault, invalid rules.toml,
//! broken config) fails loud (ADR-14 Correctness), while per-file degradations
//! ride inside the read-model as an `INCOMPLETE` stamp + warnings (NFR-RA-11,
//! Degraded).
//!
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::Path;

use anyhow::Result;

use super::{Engine, Readouts, Review};
use crate::models::quality::{DoctorReport, GateResult, HealthInfo, RulesReport, ScanResult, SessionInfo, VerifyReport};
use crate::observability::Tool;

/// The quality and governance evaluations over one engine, reached by
/// [`Engine::quality`].
#[derive(Clone, Copy)]
pub struct Quality<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Quality<'e> {
    /// The read-only readouts of the last persisted snapshot.
    pub fn readouts(&self) -> Readouts<'e> {
        Readouts { engine: self.engine }
    }

    /// The branch review: deltas, shared-budget projections and headroom.
    pub fn review(&self) -> Review<'e> {
        Review { engine: self.engine }
    }

    /// Full architecture-quality scan: reconcile-then-score (ADR-11).
    ///
    /// Returns the 0–10000 geometric-mean signal (ADR-12), the rule
    /// violations, and persists a `metric_snapshots` row (FR-GV-09).
    /// `reconcile = false` is the `--no-reconcile` escape hatch (FR-RC-04).
    ///
    /// # Errors
    /// Returns an error on a structural failure: transient engine, store
    /// fault, invalid `rules.toml` (a [`ConfigError`](crate::config::ConfigError),
    /// usage exit 2), or a failed write batch.
    pub fn scan(&self, reconcile: bool) -> Result<ScanResult> {
        self.engine.governance.record_scan(reconcile);
        crate::observability::traced(Tool::Scan, || crate::governance::scan(self.engine, reconcile))
    }

    /// Re-scan with the same parameters as the last [`scan`](Self::scan)
    /// (ADR-11); behaves like a default scan when none ran yet.
    ///
    /// # Errors
    /// Same as [`scan`](Self::scan).
    pub fn rescan(&self) -> Result<ScanResult> {
        let reconcile = self.engine.governance.last_scan_reconcile();
        crate::observability::traced(Tool::Rescan, || crate::governance::scan(self.engine, reconcile))
    }

    /// Gate check (FR-GV-04/05, BR-10): compute a fresh snapshot (always
    /// persisted, FR-GV-09), then either upsert it as the baseline (`save`)
    /// or fail iff `current < baseline − ε` (ε ≈ 1.0 absorbs float noise on
    /// the integer signal). `threshold` adds an explicit floor. No baseline
    /// → informational pass. Under a `rules.toml [review]` table (FR-GV-25)
    /// a compare fails on a gating branch-delta entry, and on the aggregate
    /// only past `drift_budget`.
    ///
    /// # Errors
    /// Returns an error on a structural failure (transient engine, store
    /// fault, failed write batch).
    pub fn gate(&self, threshold: Option<u32>, save: bool, reconcile: bool) -> Result<GateResult> {
        self.gate_with_reason(threshold, save, reconcile, None)
    }

    /// [`gate`](Self::gate) with the `--reason` a `save` records beside the
    /// baseline, and the attribution of the change it blesses (FR-GV-16,
    /// FR-GV-26). Under a `rules.toml [review]` table a drop past a numeric
    /// `drift_budget` requires one.
    ///
    /// # Errors
    /// As [`gate`](Self::gate); a `save` past the drift budget without a
    /// reason is a [`ConfigError`](crate::config::ConfigError) (usage exit 2),
    /// and the baseline stays as it was.
    pub fn gate_with_reason(
        &self,
        threshold: Option<u32>,
        save: bool,
        reconcile: bool,
        reason: Option<&str>,
    ) -> Result<GateResult> {
        crate::observability::traced(Tool::Gate, || {
            crate::governance::gate(self.engine, threshold, save, reconcile, reason, &crate::security::SecurityGateEvidence(self.engine))
        })
    }

    /// Architecture-rules compliance report against `rules.toml` (FR-GV-02):
    /// constraints, layer ordering (unassigned files exempt), and
    /// boundaries; re-materialises the derived policy graph each run
    /// (BR-12, idempotent). `rules_path` overrides the default
    /// `<root>/.logos/rules.toml` (`check --rules FILE`).
    ///
    /// # Errors
    /// Returns an error on a structural failure; an invalid contract is a
    /// [`ConfigError`](crate::config::ConfigError) (usage exit 2).
    pub fn check_rules(&self, rules_path: Option<&Path>, reconcile: bool) -> Result<RulesReport> {
        crate::observability::traced(Tool::CheckRules, || {
            crate::governance::check_rules(self.engine, rules_path, reconcile, &crate::security::SecurityGateEvidence(self.engine))
        })
    }

    /// The ratchet's bless command (`check --bless`, [FR-GV-23]): write every
    /// current offender of a `ratchet = true` constraint to the checked-in
    /// `.logos/rules-baseline.json` — sorted and deterministic, untouched when
    /// unchanged — then return the [`check_rules`](Self::check_rules) report
    /// against the set just written. The only writer of that file: `check`
    /// and `scan` only read it, and `gate` never touches it.
    ///
    /// # Errors
    /// As [`check_rules`](Self::check_rules); a failed write of the blessed set
    /// is a [`ConfigError`](crate::config::ConfigError) (usage exit 2).
    ///
    /// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
    pub fn bless_rules(&self, rules_path: Option<&Path>, reconcile: bool) -> Result<RulesReport> {
        crate::observability::traced(Tool::BlessRules, || {
            crate::governance::bless_rules(self.engine, rules_path, reconcile, &crate::security::SecurityGateEvidence(self.engine))
        })
    }

    /// ARCHITECTURE health check — DB presence/size, schema version, FTS
    /// coherence, structural integrity, the [FR-GV-20] admission tripwire, and
    /// graph counts. For INDEX freshness see [`status`](super::Graph::status).
    ///
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    ///
    /// # Errors
    /// Returns an error on a structural failure (an FTS desync is *reported*
    /// in the read-model, not an error — `health` exists to diagnose it).
    pub fn health(&self, reconcile: bool) -> Result<HealthInfo> {
        crate::observability::traced(Tool::Health, || crate::governance::health(self.engine, reconcile))
    }

    /// The fast **structural-integrity + admission-tripwire** check (CR-052,
    /// [FR-GV-18]; S-215, [FR-GV-20]; [NFR-RA-13], [ADR-46]/[ADR-48]): asserts
    /// one node per `symbol_id` and zero orphan rows, and flags every indexed
    /// file the *current* admission rules would reject — in O(a handful of
    /// indexed queries) plus O(files) matcher work, no reindex — returning the
    /// `doctor` verdict. A pure read of the persisted graph — it does not
    /// reconcile. The CLI exits 1 on `!report.ok`, and the same verdict
    /// hard-fails `session_end` / `check_rules`.
    ///
    /// # Errors
    /// Returns an error on a structural failure (a missing graph runtime).
    ///
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
    /// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    /// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
    pub fn doctor(&self) -> Result<DoctorReport> {
        crate::observability::traced(Tool::Doctor, || crate::governance::doctor(self.engine))
    }

    /// The on-demand **deep** consistency check (CR-052, [FR-GV-19], [NFR-RA-06],
    /// [ADR-46]): reindex the project into a throwaway shadow store via the
    /// always-purge [`index`](super::Pipeline::index) path and diff node/edge/file counts +
    /// symbol sets against the live graph, reporting drift with a capped sample of
    /// leaked/orphaned symbols and of the lost/extra edges (CR-236), and
    /// embedding the fast structural + admission check ([FR-GV-18], [FR-GV-20]).
    /// Catches the Channel-B orphans (files the
    /// live store retains but a fresh index drops) the fast `doctor` cannot see.
    ///
    /// The live store is opened read-only for the census and never mutated; the
    /// shadow store is torn down (db + `-wal`/`-shm`) on completion. On-demand
    /// only — a full reindex is seconds-to-minutes, so `verify` is never on a hot
    /// path. The CLI exits 1 on `!report.ok`.
    ///
    /// # Errors
    /// Returns an error on a transient engine, an unloadable plugin registry, or a
    /// failed shadow reindex / read.
    ///
    /// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
    pub fn verify(&self) -> Result<VerifyReport> {
        crate::observability::traced(Tool::Verify, || crate::governance::verify(self.engine))
    }

    /// Begin a quality session (FR-GV-04): compute a fresh snapshot and, on a
    /// first run only, save it as the baseline `session_end` compares against —
    /// an existing gate or security baseline is never moved (CR-238), except a
    /// gate baseline that no longer compares (other metric semantics or
    /// thresholds), which it resets before the edits ([FR-GV-10]).
    ///
    /// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn session_start(&self) -> Result<SessionInfo> {
        crate::observability::traced(Tool::SessionStart, || {
            crate::governance::session_start(self.engine, &crate::security::SecurityGateEvidence(self.engine))
        })
    }

    /// End the quality session (FR-GV-05): re-score and compare to the
    /// baseline — the MCP spelling of a bare `gate`, `[review]` policy included.
    ///
    /// # Errors
    /// Returns an error on a structural failure.
    pub fn session_end(&self) -> Result<GateResult> {
        crate::observability::traced(Tool::SessionEnd, || {
            crate::governance::gate(self.engine, None, false, true, None, &crate::security::SecurityGateEvidence(self.engine))
        })
    }

    /// The governance engine's in-process state ([S-020]).
    ///
    /// [S-020]: ../../../docs/planning/journal.md#s-020-governance-engine-and-quality-gate
    pub(crate) fn state(&self) -> &'e crate::governance::GovernanceState {
        &self.engine.governance
    }
}
