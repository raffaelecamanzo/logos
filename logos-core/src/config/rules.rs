//! [`Rules`] — the parsed `.logos/rules.toml` architecture contract ([FR-CF-03]).
//!
//! `rules.toml` is checked-in policy that the [governance-engine] consumes to
//! drive `check_rules`/`gate` ([FR-GV-01], [FR-GV-02]). It declares
//! `[constraints]` (numeric budgets), `[[layers]]` (named path bands with an
//! order), and `[[boundaries]]` (forbidden `from → to` dependencies).
//!
//! This story is the **loader/validator**: it parses the contract, fails loud on
//! invalid TOML / unknown keys / non-compiling layer globs (exit 2, [FR-CF-03]),
//! and returns the read-model. Building the compiled layer matchers and caching
//! the parse result by hash is the [governance-engine]'s job ([FR-GV-01],
//! S-011+); here we only *validate* that the layer globs compile.
//!
//! [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
//! [FR-GV-01]: ../../../../docs/specs/requirements/FR-GV-01.md
//! [FR-GV-02]: ../../../../docs/specs/requirements/FR-GV-02.md
//! [governance-engine]: ../../../../docs/specs/architecture/components/governance-engine.md

use serde::{Deserialize, Serialize};

/// The parsed `rules.toml` read-model — the architecture contract.
///
/// `Eq` is intentionally not derived: the [`Constraints::max_clone_ratio`] budget
/// ([FR-GV-11], [CR-005]) is an `f64`, so the contract is only `PartialEq`.
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [CR-005]: ../../../../docs/requests/CR-005-extended-structural-metrics.md
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Numeric architecture budgets (`[constraints]`).
    #[serde(default)]
    pub constraints: Constraints,

    /// Tunable metric detection thresholds (`[metric_thresholds]`, [FR-QM-14],
    /// [BR-25], [CR-005]). Each key is optional; an omitted key falls back to the
    /// documented [CR-005] §5.1 default. The effective set is hashed into every
    /// snapshot and the gate baseline, so any change triggers the announced
    /// auto-re-baseline ([FR-GV-10]) — threshold tuning is never silent.
    ///
    /// [FR-QM-14]: ../../../../docs/specs/requirements/FR-QM-14.md
    /// [BR-25]: ../../../../docs/specs/software-spec.md#311-quality-metrics
    /// [FR-GV-10]: ../../../../docs/specs/requirements/FR-GV-10.md
    /// [CR-005]: ../../../../docs/requests/CR-005-extended-structural-metrics.md
    #[serde(default)]
    pub metric_thresholds: MetricThresholds,

    /// Ordered architectural layers (`[[layers]]`).
    #[serde(default)]
    pub layers: Vec<Layer>,

    /// Forbidden dependency boundaries (`[[boundaries]]`).
    #[serde(default)]
    pub boundaries: Vec<Boundary>,

    /// Glob-level forbidden imports (`[[forbidden_imports]]`, [FR-GV-12], [CR-002]).
    ///
    /// [FR-GV-12]: ../../../../docs/specs/requirements/FR-GV-12.md
    /// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
    #[serde(default)]
    pub forbidden_imports: Vec<ForbiddenImport>,

    /// Require-tested coverage contracts (`[[require_tested]]`, [FR-GV-13], [CR-002]).
    ///
    /// [FR-GV-13]: ../../../../docs/specs/requirements/FR-GV-13.md
    /// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
    #[serde(default)]
    pub require_tested: Vec<RequireTested>,

    /// Require-documented contracts (`[[require_documented]]`, [FR-GV-15], [CR-003]).
    ///
    /// [FR-GV-15]: ../../../../docs/specs/requirements/FR-GV-15.md
    /// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
    #[serde(default)]
    pub require_documented: Vec<RequireDocumented>,

    /// Git-history analytics tuning (`[history]`, [FR-CF-03] ext., [FR-GH-09],
    /// [CR-006]). Every key is optional; an omitted key falls back to the
    /// documented [CR-006] §5 default. Unlike the rest of the contract, these
    /// keys tune the **non-gated temporal tier only** — they never affect
    /// `check_rules` or the gate ([BR-26]). The [`History::effective`] set hashes
    /// into every temporal snapshot ([BR-27]), so any change is visible.
    ///
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    /// [FR-GH-09]: ../../../../docs/specs/requirements/FR-GH-09.md
    /// [BR-26]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [BR-27]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
    /// [CR-006]: ../../../../docs/requests/CR-006-git-history-analytics.md
    #[serde(default)]
    pub history: History,

    /// Coverage-ingestion tuning (`[coverage]`, [FR-CF-03] ext., [FR-CV-09],
    /// [CR-007]). Every key is optional; an omitted table behaves as all-defaults.
    /// Like `[history]`, these keys tune the **non-gated advisory evidence tier
    /// only** — they never affect `check_rules` or the gate ([BR-28]). The
    /// [`Coverage::effective`] set hashes into every coverage snapshot ([FR-CV-09],
    /// the [BR-27] provenance pattern reapplied), so any change is visible.
    ///
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    /// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
    /// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
    /// [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md
    #[serde(default)]
    pub coverage: Coverage,
}

/// The `max_dead` budget's two shapes ([FR-GV-11], [CR-043], [ADR-39]).
///
/// `max_dead` parses as an untagged value: a bare TOML integer is the original
/// **absolute** ceiling; a TOML inline table is the **delta-from-blessed-
/// baseline** form. The two are disambiguated by value type at load, so the
/// single `max_dead` key carries both modes with no ambiguity:
///
/// ```toml
/// [constraints]
/// max_dead = 5                          # absolute: fail iff dead > 5
/// # — or —
/// max_dead = { baseline = 42 }          # delta: fail iff dead > 42 (delta defaults to 0)
/// max_dead = { baseline = 42, delta = 3 }  # delta: fail iff dead > 45
/// ```
///
/// Delta mode reuses the gate-baseline *discipline* ([ADR-11], [governance-engine]):
/// the blessed steady-state count is recorded in `rules.toml` (re-blessed exactly
/// as the metric baseline is, after the precision stories land) and the gate fails
/// only when the count rises above it — the property an absolute integer cannot
/// express against an irreducible steady-state. The blessed baseline lives in
/// config, not the metric `baseline` table, so the rules gate stays orthogonal to
/// the metrics gate ([CR-043] §6) and a clean re-run is byte-identical ([NFR-RA-06]).
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../../docs/specs/architecture/decisions/ADR-39.md
/// [ADR-11]: ../../../../docs/specs/architecture/decisions/ADR-11.md
/// [governance-engine]: ../../../../docs/specs/architecture/components/governance-engine.md
/// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaxDead {
    /// The original absolute ceiling: the gate fails iff the project-wide dead
    /// count exceeds this integer ([CR-002], retained unchanged by [CR-043]).
    ///
    /// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
    Absolute(u32),
    /// The delta-from-blessed-baseline form ([CR-043], [ADR-39]).
    Baseline(MaxDeadBaseline),
}

/// The `max_dead` delta-from-blessed-baseline table ([CR-043], [ADR-39]).
///
/// The gate fails when the dead count exceeds `baseline + delta`. `baseline` is
/// the blessed steady-state dead count (human-reviewed, re-blessed after the
/// precision work lands); `delta` is the tolerated increase above it before the
/// gate fails and defaults to `0` ("no *new* dead code"). Both fields are `u32`,
/// so the form is always in range — there is nothing further to range-check at
/// load; `#[serde(deny_unknown_fields)]` still fails a typo'd key loud (exit 2,
/// [NFR-UX-02]).
///
/// [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../../docs/specs/architecture/decisions/ADR-39.md
/// [NFR-UX-02]: ../../../../docs/specs/requirements/NFR-UX-02.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaxDeadBaseline {
    /// The blessed steady-state dead-function count.
    pub baseline: u32,
    /// Tolerated increase above `baseline` before the gate fails (default 0).
    #[serde(default)]
    pub delta: u32,
}

impl MaxDead {
    /// The effective ceiling the project-wide dead count must not exceed: the
    /// absolute integer, or `baseline + delta` (saturating) for the delta form.
    ///
    /// Internal to the crate (the governance evaluator and this module's tests
    /// are the only callers) — the public budget seam is [`Self::exceeded_message`].
    pub(crate) fn ceiling(&self) -> u64 {
        match self {
            MaxDead::Absolute(max) => u64::from(*max),
            MaxDead::Baseline(b) => u64::from(b.baseline).saturating_add(u64::from(b.delta)),
        }
    }

    /// The absolute ceiling when the budget is in absolute mode, else `None`.
    ///
    /// Lets a scalar-only consumer (the web config editor's typed `max_dead`
    /// field, [FR-UI-12]) render the absolute form while a delta-form contract
    /// is edited through the authoritative raw-TOML pane.
    ///
    /// [FR-UI-12]: ../../../../docs/specs/requirements/FR-UI-12.md
    pub fn as_absolute(&self) -> Option<u32> {
        match self {
            MaxDead::Absolute(max) => Some(*max),
            MaxDead::Baseline(_) => None,
        }
    }

    /// The `check_rules` violation message when `dead` exceeds the budget, or
    /// `None` when the count is within budget ([FR-GV-11]). The phrasing is
    /// mode-specific but deterministic, so a re-run is byte-identical ([NFR-RA-06]).
    ///
    /// The absolute-mode message is preserved verbatim from the pre-[CR-043]
    /// gate, so the absolute form behaves and reports exactly as before.
    ///
    /// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    pub fn exceeded_message(&self, dead: u64) -> Option<String> {
        if dead <= self.ceiling() {
            return None;
        }
        Some(match self {
            MaxDead::Absolute(max) => format!("{dead} dead functions exceed max_dead = {max}"),
            MaxDead::Baseline(b) => format!(
                "{dead} dead functions exceed the blessed max_dead baseline {} \
                 (+{} delta, ceiling {})",
                b.baseline,
                b.delta,
                self.ceiling()
            ),
        })
    }
}

/// `[constraints]` — numeric budgets enforced by `check_rules` ([FR-GV-01]).
///
/// Each is optional: an omitted constraint is simply not enforced. `no_god_files`
/// is the per-file symbol-count ceiling (SRS OQ-06; threshold left to the author).
///
/// The coupling and redundancy budgets ([FR-GV-11], [CR-002]) extend the family:
/// `max_fan_in`/`max_fan_out` cap a symbol's inbound/outbound dependency edges
/// over the canonical dependency graph (every edge kind except `Contains`,
/// [BR-19]); `max_dead`/`max_duplicates` cap the project-wide count of
/// `is_dead`/`is_duplicate` functions ([FR-AN-04]). No schema migration — they
/// read existing annotation columns and the hydrated edge set.
///
/// The four [CR-005] structural budgets extend the family again ([FR-GV-11]
/// extended, [UAT-GV-08]): `max_nesting_depth` (per-function nesting cap,
/// [FR-EX-07]), `max_brain_methods` (project-wide brain-method count,
/// [FR-QM-10]), `max_clone_ratio` (project-wide near-clone function ratio,
/// [FR-AN-06]), and `no_god_containers` (no class-like container over the god
/// thresholds, [FR-QM-12]). Same semantics as the rest: an omitted key is not
/// enforced; every violation is `severity='error'`; ordering is deterministic.
/// They are **production-scoped** (test functions/containers excluded) so each
/// budget and the dimension it enforces agree by construction — the same posture
/// `max_dead`/`max_duplicates` take with the Redundancy metric.
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [FR-AN-04]: ../../../../docs/specs/requirements/FR-AN-04.md
/// [FR-AN-06]: ../../../../docs/specs/requirements/FR-AN-06.md
/// [FR-EX-07]: ../../../../docs/specs/requirements/FR-EX-07.md
/// [FR-QM-10]: ../../../../docs/specs/requirements/FR-QM-10.md
/// [FR-QM-12]: ../../../../docs/specs/requirements/FR-QM-12.md
/// [UAT-GV-08]: ../../../../docs/specs/requirements/UAT-GV-08.md
/// [BR-19]: ../../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
/// [CR-005]: ../../../../docs/requests/CR-005-extended-structural-metrics.md
///
/// `Eq` is not derived: `max_clone_ratio` is an `f64`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    /// Maximum allowed dependency cycles.
    pub max_cycles: Option<u32>,
    /// Maximum cyclomatic complexity per function.
    pub max_cc: Option<u32>,
    /// Maximum lines per function.
    pub max_fn_lines: Option<u32>,
    /// Maximum symbols per file before a file is flagged a "god file" (OQ-06).
    pub no_god_files: Option<u32>,
    /// Maximum inbound dependency edges for any one symbol ([FR-GV-11], [BR-19]).
    pub max_fan_in: Option<u32>,
    /// Maximum outbound dependency edges for any one symbol ([FR-GV-11], [BR-19]).
    pub max_fan_out: Option<u32>,
    /// Project-wide dead-function budget ([FR-GV-11], [CR-043], [ADR-39]).
    ///
    /// Accepts two shapes (see [`MaxDead`]): the original **absolute** integer
    /// (`max_dead = 5`), or a **delta-from-blessed-baseline** inline table
    /// (`max_dead = { baseline = 42, delta = 0 }`) that fails the gate only when
    /// the dead count rises above the blessed baseline — catching *newly*-
    /// introduced dead code against an irreducible steady-state. Omitted = not
    /// enforced. Both modes are deterministic ([NFR-RA-06]).
    ///
    /// [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md
    /// [ADR-39]: ../../../../docs/specs/architecture/decisions/ADR-39.md
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    pub max_dead: Option<MaxDead>,
    /// Maximum project-wide count of `is_duplicate` functions ([FR-GV-11]).
    pub max_duplicates: Option<u32>,
    /// Maximum per-function maximum nesting depth ([FR-GV-11] ext., [FR-EX-07],
    /// [CR-005]); a production function deeper than this is one error violation.
    pub max_nesting_depth: Option<u32>,
    /// Maximum project-wide count of brain methods ([FR-GV-11] ext., [FR-QM-10],
    /// [CR-005]); a brain method meets all three brain thresholds.
    pub max_brain_methods: Option<u32>,
    /// Maximum project-wide near-clone production-function ratio ([FR-GV-11] ext.,
    /// [FR-AN-06], [CR-005]); `clone_group IS NOT NULL` over production functions.
    pub max_clone_ratio: Option<f64>,
    /// When `true`, no class-like container may be over the god thresholds
    /// ([FR-GV-11] ext., [FR-QM-12], [CR-005]); each god container is one error
    /// violation. `false`/omitted is not enforced.
    pub no_god_containers: Option<bool>,
}

impl Constraints {
    /// The curated recommended baselines ([CR-067], [BR-37]) — `[constraints]`
    /// have no code default (an omitted key is simply "not enforced",
    /// [FR-CF-03]), so this is the single source of truth for the "reasonable
    /// starting point" numbers the web Config editor shows beside that honest
    /// `unset → not enforced` state. **Advisory display only**: these values are
    /// never written, never enforced, and never alter the gate — promoting a
    /// project onto them is a deliberate opt-in edit like any other constraint.
    ///
    /// The values mirror what [configuration.md](../../../../docs/howto/configuration.md)
    /// has long documented as its example `[constraints]` table; this function
    /// makes that set the single curated source docs/UI both cite, rather than
    /// two hand-kept-in-sync copies ([CR-067] risk mitigation).
    ///
    /// [CR-067]: ../../../../docs/requests/CR-067-config-default-surfacing.md
    /// [BR-37]: ../../../../docs/specs/software-spec.md#326-web-ui
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    pub fn recommended() -> Self {
        Self {
            max_cycles: Some(0),
            max_cc: Some(15),
            max_fn_lines: Some(80),
            no_god_files: Some(40),
            max_fan_in: Some(30),
            max_fan_out: Some(30),
            max_dead: Some(MaxDead::Absolute(0)),
            max_duplicates: Some(0),
            max_nesting_depth: Some(4),
            max_brain_methods: Some(0),
            max_clone_ratio: Some(0.0),
            no_god_containers: Some(true),
        }
    }
}

/// `[metric_thresholds]` — the tunable CR-005 detection thresholds ([FR-QM-14],
/// [BR-25], [CR-005]).
///
/// Each key is optional; an omitted key falls back to the documented [CR-005]
/// §5.1 default (the same defaults [`Thresholds::default`] carries in the
/// metrics engine). The governance engine composes these overrides onto the
/// defaults to build the effective threshold set, whose hash is persisted in
/// every snapshot and the gate baseline so any change is visible and triggers
/// the announced auto-re-baseline ([FR-GV-10], [UAT-QM-13]).
///
/// The keys mirror the dimension thresholds the five new metrics read: nesting
/// (`T_nest`), the three brain-method thresholds (`T_cc`/`T_loc`/`T_bn`), the
/// two god-container thresholds (`T_m`/`T_span`), and — since [CR-013] — the two
/// near-clone parameters feeding Uniqueness ([FR-QM-13]): `clone_similarity`
/// ([FR-AN-06]) and `clone_min_tokens` ([FR-EX-09]). [`MetricThresholds::effective`]
/// composes these onto the documented defaults to build the effective set whose
/// hash gates the baseline.
///
/// `Eq` is not derived: `clone_similarity` is an `f64` (the same reason
/// [`Constraints`] and [`Rules`] are `PartialEq`-only).
///
/// [`Thresholds::default`]: crate::metrics::Thresholds
/// [FR-QM-13]: ../../../../docs/specs/requirements/FR-QM-13.md
/// [FR-AN-06]: ../../../../docs/specs/requirements/FR-AN-06.md
/// [FR-EX-09]: ../../../../docs/specs/requirements/FR-EX-09.md
/// [FR-QM-14]: ../../../../docs/specs/requirements/FR-QM-14.md
/// [BR-25]: ../../../../docs/specs/software-spec.md#311-quality-metrics
/// [FR-GV-10]: ../../../../docs/specs/requirements/FR-GV-10.md
/// [UAT-QM-13]: ../../../../docs/specs/requirements/UAT-QM-13.md
/// [CR-005]: ../../../../docs/requests/CR-005-extended-structural-metrics.md
/// [CR-013]: ../../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricThresholds {
    /// `T_nest` — nesting depth at/above which a function is "deeply nested"
    /// ([FR-QM-09]); default 4.
    ///
    /// [FR-QM-09]: ../../../../docs/specs/requirements/FR-QM-09.md
    pub nesting_depth: Option<i64>,
    /// `T_cc` — cyclomatic-complexity floor of a brain method ([FR-QM-10]);
    /// default 15.
    ///
    /// [FR-QM-10]: ../../../../docs/specs/requirements/FR-QM-10.md
    pub brain_complexity: Option<i64>,
    /// `T_loc` — line-count floor of a brain method ([FR-QM-10]); default 100.
    pub brain_lines: Option<i64>,
    /// `T_bn` — max-nesting floor of a brain method ([FR-QM-10]); default 3.
    pub brain_nesting: Option<i64>,
    /// `T_m` — method count at/above which a container is a god container
    /// ([FR-QM-12]); default 20.
    ///
    /// [FR-QM-12]: ../../../../docs/specs/requirements/FR-QM-12.md
    pub god_methods: Option<i64>,
    /// `T_span` — line span at/above which a container is a god container
    /// ([FR-QM-12]); default 500.
    pub god_span: Option<i64>,
    /// The Jaccard clone-similarity threshold feeding Uniqueness ([FR-AN-06],
    /// [FR-QM-13], [CR-013]); default 0.85, valid range `(0, 1]`. An out-of-range
    /// value fails validation (exit 2).
    ///
    /// [FR-AN-06]: ../../../../docs/specs/requirements/FR-AN-06.md
    /// [CR-013]: ../../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
    pub clone_similarity: Option<f64>,
    /// The minimum-token floor below which a function produces no clone shingles
    /// ([FR-EX-09], [FR-QM-13], [CR-013]); default 50, a positive integer. A
    /// non-positive value fails validation (exit 2).
    ///
    /// [FR-EX-09]: ../../../../docs/specs/requirements/FR-EX-09.md
    pub clone_min_tokens: Option<i64>,
}

impl MetricThresholds {
    /// Compose the raw `[metric_thresholds]` overrides onto the documented
    /// [`Thresholds::default`](crate::metrics::Thresholds) values to build the
    /// **effective** detection-threshold set ([BR-25], [FR-QM-14]). An omitted
    /// key keeps its documented default, so a partial table is honoured
    /// key-by-key ([UAT-QM-13]).
    ///
    /// This is the single seam where config policy becomes the metrics threshold
    /// set: the [governance-engine] passes the result to
    /// [`metrics::snapshot`](crate::metrics::snapshot) (its hash gates the
    /// baseline) and the [annotation-engine] reads its near-clone parameters for
    /// clustering ([FR-AN-06]) — so the snapshot, the gate, and the clone groups
    /// all derive from one threshold set ([CR-013]). It mirrors the
    /// [`History::effective`]/[`Coverage::effective`] raw→effective split.
    ///
    /// [governance-engine]: ../../../../docs/specs/architecture/components/governance-engine.md
    /// [annotation-engine]: ../../../../docs/specs/architecture/components/annotation-engine.md
    /// [FR-AN-06]: ../../../../docs/specs/requirements/FR-AN-06.md
    /// [FR-QM-14]: ../../../../docs/specs/requirements/FR-QM-14.md
    /// [BR-25]: ../../../../docs/specs/software-spec.md#311-quality-metrics
    /// [UAT-QM-13]: ../../../../docs/specs/requirements/UAT-QM-13.md
    /// [CR-013]: ../../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
    pub fn effective(&self) -> crate::metrics::Thresholds {
        let d = crate::metrics::Thresholds::default();
        crate::metrics::Thresholds {
            nest: self.nesting_depth.unwrap_or(d.nest),
            brain_cc: self.brain_complexity.unwrap_or(d.brain_cc),
            brain_loc: self.brain_lines.unwrap_or(d.brain_loc),
            brain_nest: self.brain_nesting.unwrap_or(d.brain_nest),
            god_methods: self.god_methods.unwrap_or(d.god_methods),
            god_span: self.god_span.unwrap_or(d.god_span),
            clone_similarity: self.clone_similarity.unwrap_or(d.clone_similarity),
            clone_min_tokens: self.clone_min_tokens.unwrap_or(d.clone_min_tokens),
        }
    }
}

/// `[history]` — the optional git-history analytics tuning table ([FR-CF-03]
/// ext., [FR-GH-09], [CR-006]).
///
/// The raw parsed shape: every key is `Option`, so an omitted key means "use the
/// documented default" rather than "zero". [`History::effective`] resolves the
/// defaults into an [`EffectiveHistory`] whose [`hash`](EffectiveHistory::hash)
/// is recorded in every temporal snapshot ([BR-27]) — the only place these keys
/// surface. They tune the **non-gated** temporal tier exclusively ([BR-26]); the
/// gate never reads `history.db`, so changing them can never move the signal.
///
/// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
/// [FR-GH-09]: ../../../../docs/specs/requirements/FR-GH-09.md
/// [BR-26]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
/// [BR-27]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
/// [CR-006]: ../../../../docs/requests/CR-006-git-history-analytics.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct History {
    /// HEAD-anchored window length in calendar months ([FR-GH-03]); default
    /// [`EffectiveHistory::DEFAULT_WINDOW_MONTHS`] (12). The cutoff is computed
    /// from the HEAD committer timestamp, never the wall clock ([BR-27]).
    ///
    /// [FR-GH-03]: ../../../../docs/specs/requirements/FR-GH-03.md
    pub window_months: Option<u32>,
    /// Mega-commit cap for co-change pairing ([FR-GH-04]); default
    /// [`EffectiveHistory::DEFAULT_CO_CHANGE_MAX_COMMIT_FILES`] (50). A commit
    /// touching more than this many files is skipped **for pairing only** — it
    /// still counts toward churn.
    ///
    /// [FR-GH-04]: ../../../../docs/specs/requirements/FR-GH-04.md
    pub co_change_max_commit_files: Option<u32>,
    /// Fix-commit message patterns for the defect-history heuristic ([FR-GH-05]);
    /// default [`EffectiveHistory::default_defect_patterns`]. Always rendered with
    /// an explicit "heuristic" label downstream ([NFR-CC-04]).
    ///
    /// [FR-GH-05]: ../../../../docs/specs/requirements/FR-GH-05.md
    /// [NFR-CC-04]: ../../../../docs/specs/requirements/NFR-CC-04.md
    pub defect_patterns: Option<Vec<String>>,
}

/// The **effective** history configuration: [`History`] with every default
/// resolved ([CR-006] §5). This is what the [history-engine] mines against, and
/// its [`hash`](Self::hash) is the value persisted in every temporal snapshot
/// ([BR-27], [FR-GH-09]).
///
/// Mirrors the [`MetricThresholds`] → [`Thresholds`](crate::metrics::Thresholds)
/// split: a raw, all-`Option` parsed table and a resolved, hashable effective
/// set ([ADR-21]'s config-hash-into-snapshot pattern reapplied per [ADR-22]).
///
/// [history-engine]: ../../../../docs/specs/architecture/components/history-engine.md
/// [BR-27]: ../../../../docs/specs/software-spec.md#322-git-history-analytics
/// [FR-GH-09]: ../../../../docs/specs/requirements/FR-GH-09.md
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [ADR-22]: ../../../../docs/specs/architecture/decisions/ADR-22.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveHistory {
    /// Window length in calendar months (≥ 1).
    pub window_months: u32,
    /// Mega-commit co-change cap (≥ 2 — pairing needs at least two files).
    pub co_change_max_commit_files: u32,
    /// Fix-commit message patterns, in declared order.
    pub defect_patterns: Vec<String>,
}

impl EffectiveHistory {
    /// Default HEAD-anchored window ([CR-006] §5): 12 calendar months.
    pub const DEFAULT_WINDOW_MONTHS: u32 = 12;
    /// Default co-change mega-commit cap ([CR-006] §5, [FR-GH-04]): 50 files.
    pub const DEFAULT_CO_CHANGE_MAX_COMMIT_FILES: u32 = 50;

    /// The documented default defect-message patterns ([CR-006] §5, [FR-GH-05]).
    ///
    /// Case-insensitive word-boundary matches for the common fix vocabulary.
    /// They are validated for non-emptiness at config load, but compiled and
    /// matched only by the defect heuristic ([S-047]) — not here.
    ///
    /// [S-047]: ../../../../docs/planning/journal.md#s-047-temporal-metrics-co-change-and-defect-heuristic
    pub fn default_defect_patterns() -> Vec<String> {
        ["(?i)\\bfix(es|ed)?\\b", "(?i)\\bbug\\b", "(?i)\\bhotfix\\b"]
            .iter()
            .map(|p| (*p).to_string())
            .collect()
    }

    /// A blake3 digest over a fixed-order canonical `key=value` string —
    /// byte-identical across the four CI targets ([NFR-RA-06]). Changing any
    /// `[history]` key changes the hash ([FR-GH-09] acceptance), exactly as
    /// [`Thresholds::hash`](crate::metrics::Thresholds) does for the gated tier.
    ///
    /// The patterns are joined with a Unit-Separator (`\x1f`) that cannot occur
    /// in a TOML string value, so two pattern lists can never collide by
    /// concatenation.
    ///
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    /// [FR-GH-09]: ../../../../docs/specs/requirements/FR-GH-09.md
    pub fn hash(&self) -> String {
        let canonical = format!(
            "window_months={};co_change_max_commit_files={};defect_patterns={}",
            self.window_months,
            self.co_change_max_commit_files,
            self.defect_patterns.join("\u{1f}"),
        );
        blake3::hash(canonical.as_bytes()).to_hex().to_string()
    }
}

impl History {
    /// Resolve the raw table into the [`EffectiveHistory`] set, substituting the
    /// documented [CR-006] §5 default for every omitted key.
    pub fn effective(&self) -> EffectiveHistory {
        EffectiveHistory {
            window_months: self
                .window_months
                .unwrap_or(EffectiveHistory::DEFAULT_WINDOW_MONTHS),
            co_change_max_commit_files: self
                .co_change_max_commit_files
                .unwrap_or(EffectiveHistory::DEFAULT_CO_CHANGE_MAX_COMMIT_FILES),
            defect_patterns: self
                .defect_patterns
                .clone()
                .unwrap_or_else(EffectiveHistory::default_defect_patterns),
        }
    }
}

/// `[coverage]` — the optional coverage-ingestion tuning table ([FR-CF-03] ext.,
/// [FR-CV-09], [CR-007]).
///
/// The raw parsed shape (every key `Option`, so an omitted key means "use the
/// documented default"), mirroring [`History`]. [`Coverage::effective`] resolves
/// the defaults into an [`EffectiveCoverage`] whose [`hash`](EffectiveCoverage::hash)
/// is recorded in every coverage snapshot ([FR-CV-09]) — the only place these
/// keys surface. They tune the **non-gated** advisory evidence tier exclusively
/// ([BR-28]); the gate never reads `history.db`, so changing them can never move
/// the signal.
///
/// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
/// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
/// [BR-28]: ../../../../docs/specs/software-spec.md#323-coverage-test-evidence
/// [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Coverage {
    /// Path prefixes stripped from a coverage-report path before longest-unique-
    /// suffix matching ([FR-CV-03]); default empty. Lets absolute build-dir paths
    /// (cargo-llvm-cov, pytest-cov) bind to repo-relative indexed files, and
    /// disambiguates otherwise-ambiguous suffixes by revealing more of the real
    /// path. Separator normalization is applied by the mapper, not here.
    ///
    /// [FR-CV-03]: ../../../../docs/specs/requirements/FR-CV-03.md
    pub path_strip_prefixes: Option<Vec<String>>,
}

/// The **effective** coverage configuration: [`Coverage`] with every default
/// resolved ([CR-007]). This is what the coverage ingest maps against, and its
/// [`hash`](Self::hash) is the value persisted in every coverage snapshot
/// ([FR-CV-09], [ADR-21]'s config-hash-into-snapshot pattern reapplied per
/// [ADR-23]).
///
/// Mirrors the [`History`] → [`EffectiveHistory`] split exactly.
///
/// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
/// [ADR-21]: ../../../../docs/specs/architecture/decisions/ADR-21.md
/// [ADR-23]: ../../../../docs/specs/architecture/decisions/ADR-23.md
/// [CR-007]: ../../../../docs/requests/CR-007-coverage-ingestion.md
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveCoverage {
    /// Path prefixes stripped before suffix matching, in declared order.
    pub path_strip_prefixes: Vec<String>,
}

impl EffectiveCoverage {
    /// A blake3 digest over a fixed-order canonical `key=value` string —
    /// byte-identical across the four CI targets ([NFR-RA-06]). Changing
    /// `path_strip_prefixes` changes the hash ([FR-CV-09] acceptance), exactly as
    /// [`EffectiveHistory::hash`] does for the temporal tier.
    ///
    /// The prefixes are joined with a Unit-Separator (`\x1f`) that cannot occur in
    /// a TOML string value, so two prefix lists can never collide by
    /// concatenation.
    ///
    /// [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md
    /// [FR-CV-09]: ../../../../docs/specs/requirements/FR-CV-09.md
    pub fn hash(&self) -> String {
        let canonical = format!(
            "path_strip_prefixes={}",
            self.path_strip_prefixes.join("\u{1f}")
        );
        blake3::hash(canonical.as_bytes()).to_hex().to_string()
    }
}

impl Coverage {
    /// Resolve the raw table into the [`EffectiveCoverage`] set, substituting the
    /// documented default (empty) for an omitted key.
    pub fn effective(&self) -> EffectiveCoverage {
        EffectiveCoverage {
            path_strip_prefixes: self.path_strip_prefixes.clone().unwrap_or_default(),
        }
    }
}

/// A `[[layers]]` entry: a named band of paths with an ordering position.
///
/// Layer ordering drives the upward-dependency check (BR-11); `paths` are globs
/// matched against discovered files (first-glob-wins assignment, [FR-GV-02]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Layer name (e.g. `domain`, `application`, `infrastructure`).
    pub name: String,
    /// Globs that assign a file to this layer.
    pub paths: Vec<String>,
    /// Ordering position; a higher layer may not depend on a lower one (BR-11).
    pub order: u32,
}

/// A `[[boundaries]]` entry: a forbidden `from → to` dependency ([FR-GV-02]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
    /// The layer/module that may not depend on `to`.
    pub from: String,
    /// The forbidden dependency target.
    pub to: String,
    /// Human-readable rationale surfaced in violation reports (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A `[[forbidden_imports]]` entry: a glob-level import ban ([FR-GV-12], [CR-002]).
///
/// Unlike a [`Boundary`] (which names declared *layers*), `from`/`to` are
/// **path globs** matched against file paths, and the contract acts on
/// `Imports`/`References` edges rather than every dependency kind. Any such
/// edge whose source file matches `from` and whose target file matches `to` is
/// a `severity='error'` violation, materialised as a `forbidden_dependency`
/// edge through the same idempotent pass as `[[boundaries]]` ([FR-GV-02]).
///
/// [FR-GV-12]: ../../../../docs/specs/requirements/FR-GV-12.md
/// [FR-GV-02]: ../../../../docs/specs/requirements/FR-GV-02.md
/// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForbiddenImport {
    /// Path glob for the importing (source) file.
    pub from: String,
    /// Path glob for the imported (target) file.
    pub to: String,
    /// Human-readable rationale surfaced in violation reports (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A `[[require_tested]]` entry: a coverage contract over a set of path globs
/// ([FR-GV-13], [CR-002]).
///
/// Every `exported` Function/Method whose defining file matches any `paths`
/// glob must be reachable by transitive `calls` BFS from an `is_test` node
/// ([FR-AN-05]) — the SAME static reachability `test_gaps` reports ([FR-GV-08]).
/// An unreached exported symbol is a `severity='error'` violation carrying
/// `reason`; **non-exported symbols are exempt** — the contract enforces a
/// *public-API* test path, not total coverage. Like a [`ForbiddenImport`],
/// `paths` are path globs (not declared layer names); unlike it, the contract
/// reads node reachability rather than edges, so it materialises no derived
/// edge and needs no schema migration ([CR-002] "no migration").
///
/// [FR-GV-13]: ../../../../docs/specs/requirements/FR-GV-13.md
/// [FR-AN-05]: ../../../../docs/specs/requirements/FR-AN-05.md
/// [FR-GV-08]: ../../../../docs/specs/requirements/FR-GV-08.md
/// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequireTested {
    /// Path globs selecting the files whose exported symbols must be tested.
    pub paths: Vec<String>,
    /// Human-readable rationale surfaced in violation reports (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A `[[require_documented]]` entry: a documentation contract over a set of path
/// globs ([FR-GV-15], [CR-003]).
///
/// Every `exported` Function/Method whose defining file matches any `paths` glob
/// must be referenced by some [`DocSection`](crate::model::NodeKind::DocSection)
/// over a [`DocReference`](crate::model::EdgeKind::DocReference) edge ([FR-DG-04]) —
/// the SAME reference set `doc_gaps` reports ([FR-GV-14]). An unreferenced
/// exported symbol is a `severity='error'` violation carrying `reason`;
/// **non-exported symbols are exempt** — the contract enforces a *public-API*
/// documentation gate, not total documentation. It is the documentation analog
/// of [`RequireTested`] ([FR-GV-13]): like it, `paths` are path globs (not
/// declared layer names), and it reads node reference state rather than the code
/// dependency graph, so it materialises no derived edge and needs no schema
/// migration ([CR-003] rides the existing `violations` CHECK).
///
/// [FR-GV-15]: ../../../../docs/specs/requirements/FR-GV-15.md
/// [FR-DG-04]: ../../../../docs/specs/requirements/FR-DG-04.md
/// [FR-GV-14]: ../../../../docs/specs/requirements/FR-GV-14.md
/// [FR-GV-13]: ../../../../docs/specs/requirements/FR-GV-13.md
/// [CR-003]: ../../../../docs/requests/CR-003-documentation-graph-layer.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequireDocumented {
    /// Path globs selecting the files whose exported symbols must be documented.
    pub paths: Vec<String>,
    /// Human-readable rationale surfaced in violation reports (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

impl Rules {
    /// Validate the contract: every layer glob and every `[[forbidden_imports]]`
    /// glob must compile and stay within the project root ([NFR-SE-04]). Called
    /// by the loader so an invalid contract fails at load time (exit 2,
    /// [FR-CF-03], [NFR-UX-02]).
    ///
    /// [NFR-SE-04]: ../../../../docs/specs/requirements/NFR-SE-04.md
    /// [NFR-UX-02]: ../../../../docs/specs/requirements/NFR-UX-02.md
    /// [FR-CF-03]: ../../../../docs/specs/requirements/FR-CF-03.md
    pub(crate) fn validate(&self) -> Result<(), super::error::ConfigError> {
        for layer in &self.layers {
            super::globs::validate(&layer.paths)?;
        }
        // FR-GV-12: the `from`/`to` globs compile at load, so an invalid glob
        // fails before any evaluation runs (exit 2, NFR-UX-02 actionable error).
        for fi in &self.forbidden_imports {
            super::globs::validate(std::slice::from_ref(&fi.from))?;
            super::globs::validate(std::slice::from_ref(&fi.to))?;
        }
        // FR-GV-13: the `paths` globs compile at load, so an invalid glob fails
        // before any evaluation runs (exit 2, NFR-UX-02 actionable error).
        for rt in &self.require_tested {
            super::globs::validate(&rt.paths)?;
        }
        // FR-GV-15: the `require_documented` `paths` globs likewise compile at
        // load (exit 2, NFR-UX-02 actionable error).
        for rd in &self.require_documented {
            super::globs::validate(&rd.paths)?;
        }
        // FR-QM-14 / FR-GV-11 (CR-005): the tunable thresholds and the
        // `max_clone_ratio` budget must be in range, so a misconfiguration fails
        // loud at load (exit 2) rather than silently skewing the signal — e.g. a
        // negative `max_clone_ratio` would make every clean project violate, and a
        // non-positive `T_nest` would flag every function as deeply nested.
        if let Some(ratio) = self.constraints.max_clone_ratio {
            if !(0.0..=1.0).contains(&ratio) {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "constraints.max_clone_ratio".to_string(),
                    message: format!("{ratio} is outside the valid ratio range [0.0, 1.0]"),
                });
            }
        }
        let mt = &self.metric_thresholds;
        for (key, value) in [
            ("nesting_depth", mt.nesting_depth),
            ("brain_complexity", mt.brain_complexity),
            ("brain_lines", mt.brain_lines),
            ("brain_nesting", mt.brain_nesting),
            ("god_methods", mt.god_methods),
            ("god_span", mt.god_span),
            // CR-013: the near-clone token floor is a positive integer, validated
            // on the same exit-2 path as the structural thresholds.
            ("clone_min_tokens", mt.clone_min_tokens),
        ] {
            if let Some(v) = value {
                if v <= 0 {
                    return Err(super::error::ConfigError::InvalidValue {
                        key: format!("metric_thresholds.{key}"),
                        message: format!("{v} must be a positive integer"),
                    });
                }
            }
        }
        // CR-013 / FR-AN-06: the near-clone similarity is a Jaccard ratio in the
        // half-open range (0, 1] — a value at or below 0 (or above 1) is not a
        // valid similarity and is rejected loud at load (exit 2) rather than
        // silently collapsing or disabling Uniqueness clustering.
        if let Some(sim) = mt.clone_similarity {
            if !(sim > 0.0 && sim <= 1.0) {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "metric_thresholds.clone_similarity".to_string(),
                    message: format!("{sim} is outside the valid range (0, 1]"),
                });
            }
        }
        // FR-CF-03 / FR-GH-03 / FR-GH-04 (CR-006): the `[history]` tuning keys
        // must be in range so a misconfiguration fails loud at load (exit 2)
        // rather than silently producing a degenerate temporal tier — a zero
        // window would exclude every commit, and a cap below 2 would make
        // co-change pairing impossible (a pair needs at least two files).
        if let Some(months) = self.history.window_months {
            if months == 0 {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "history.window_months".to_string(),
                    message: "must be at least 1 month".to_string(),
                });
            }
        }
        if let Some(cap) = self.history.co_change_max_commit_files {
            if cap < 2 {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "history.co_change_max_commit_files".to_string(),
                    message: format!("{cap} must be at least 2 (a co-change pair needs two files)"),
                });
            }
        }
        // The defect patterns are compiled by the heuristic (S-047); here we
        // only reject an empty pattern string, which would match everything and
        // make the heuristic meaningless (never-fabricate, NFR-CC-04).
        if let Some(patterns) = &self.history.defect_patterns {
            for (idx, pat) in patterns.iter().enumerate() {
                if pat.trim().is_empty() {
                    return Err(super::error::ConfigError::InvalidValue {
                        key: format!("history.defect_patterns[{idx}]"),
                        message: "must not be empty".to_string(),
                    });
                }
            }
        }
        // FR-CF-03 / FR-CV-09 (CR-007): an empty/whitespace `[coverage]` strip
        // prefix would strip nothing yet perturb the snapshot config hash for no
        // reason — reject it loud at load (exit 2) rather than silently carrying a
        // no-op prefix, mirroring the `[history]` defect-pattern check.
        if let Some(prefixes) = &self.coverage.path_strip_prefixes {
            for (idx, prefix) in prefixes.iter().enumerate() {
                if prefix.trim().is_empty() {
                    return Err(super::error::ConfigError::InvalidValue {
                        key: format!("coverage.path_strip_prefixes[{idx}]"),
                        message: "must not be empty".to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_history() -> EffectiveHistory {
        EffectiveHistory {
            window_months: EffectiveHistory::DEFAULT_WINDOW_MONTHS,
            co_change_max_commit_files: EffectiveHistory::DEFAULT_CO_CHANGE_MAX_COMMIT_FILES,
            defect_patterns: EffectiveHistory::default_defect_patterns(),
        }
    }

    /// FR-GH-09 / FR-CF-03 acceptance: changing ANY `[history]` key changes the
    /// exposed history-config hash, and identical configs hash identically (the
    /// determinism the snapshot series relies on). The gated-tier analog is
    /// `metrics::tests` for `Thresholds::hash`.
    #[test]
    fn history_hash_changes_when_any_key_changes() {
        let base = base_history();
        let changed_window = EffectiveHistory {
            window_months: 6,
            ..base.clone()
        };
        let changed_cap = EffectiveHistory {
            co_change_max_commit_files: 10,
            ..base.clone()
        };
        let changed_patterns = EffectiveHistory {
            defect_patterns: vec!["(?i)\\bcrash\\b".to_string()],
            ..base.clone()
        };

        assert_eq!(base.hash(), base.hash(), "hash is deterministic");
        assert_ne!(
            base.hash(),
            changed_window.hash(),
            "window_months change must change the hash"
        );
        assert_ne!(
            base.hash(),
            changed_cap.hash(),
            "co_change_max_commit_files change must change the hash"
        );
        assert_ne!(
            base.hash(),
            changed_patterns.hash(),
            "defect_patterns change must change the hash"
        );
    }

    /// A pattern-list reordering changes the hash (the canonical form preserves
    /// declared order — patterns are not a set), and the US join means two lists
    /// can never collide by concatenation.
    #[test]
    fn history_hash_is_order_sensitive_and_collision_safe() {
        let a = EffectiveHistory {
            defect_patterns: vec!["fix".to_string(), "bug".to_string()],
            ..base_history()
        };
        let reordered = EffectiveHistory {
            defect_patterns: vec!["bug".to_string(), "fix".to_string()],
            ..base_history()
        };
        // `["fixbug"]` vs `["fix","bug"]` must not collide (US separator).
        let concatenated = EffectiveHistory {
            defect_patterns: vec!["fixbug".to_string()],
            ..base_history()
        };
        assert_ne!(a.hash(), reordered.hash(), "order is significant");
        assert_ne!(
            a.hash(),
            concatenated.hash(),
            "the US join prevents collision"
        );
    }

    /// `History::effective` substitutes the documented CR-006 §5 defaults for
    /// omitted keys and honours set values.
    #[test]
    fn effective_resolves_defaults_and_overrides() {
        let all_default = History::default().effective();
        assert_eq!(
            all_default.window_months,
            EffectiveHistory::DEFAULT_WINDOW_MONTHS
        );
        assert_eq!(
            all_default.co_change_max_commit_files,
            EffectiveHistory::DEFAULT_CO_CHANGE_MAX_COMMIT_FILES
        );
        assert_eq!(
            all_default.defect_patterns,
            EffectiveHistory::default_defect_patterns()
        );

        let overridden = History {
            window_months: Some(6),
            co_change_max_commit_files: Some(8),
            defect_patterns: None,
        }
        .effective();
        assert_eq!(overridden.window_months, 6);
        assert_eq!(overridden.co_change_max_commit_files, 8);
        assert_eq!(
            overridden.defect_patterns,
            EffectiveHistory::default_defect_patterns(),
            "an omitted key still falls back to the default"
        );
    }

    /// FR-CF-03: misconfigured `[history]` keys fail loud at load (exit 2) rather
    /// than silently producing a degenerate temporal tier.
    #[test]
    fn validate_rejects_out_of_range_history_keys() {
        let zero_window = Rules {
            history: History {
                window_months: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(zero_window.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "history.window_months"),
            "window_months = 0 is rejected"
        );

        let tiny_cap = Rules {
            history: History {
                co_change_max_commit_files: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(tiny_cap.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "history.co_change_max_commit_files"),
            "co_change_max_commit_files < 2 is rejected"
        );

        let empty_pattern = Rules {
            history: History {
                defect_patterns: Some(vec!["  ".to_string()]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            matches!(empty_pattern.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "history.defect_patterns[0]"),
            "an empty/whitespace defect pattern is rejected"
        );

        // A valid `[history]` table passes.
        let ok = Rules {
            history: History {
                window_months: Some(24),
                co_change_max_commit_files: Some(2),
                defect_patterns: Some(vec!["(?i)fix".to_string()]),
            },
            ..Default::default()
        };
        assert!(ok.validate().is_ok(), "a valid [history] table validates");
    }

    /// FR-CV-09 acceptance: changing `path_strip_prefixes` changes the exposed
    /// coverage-config hash, and identical configs hash identically (the
    /// determinism every coverage snapshot's provenance relies on). Mirrors
    /// `history_hash_changes_when_any_key_changes`.
    #[test]
    fn coverage_hash_changes_when_prefixes_change() {
        let base = EffectiveCoverage::default();
        let changed = EffectiveCoverage {
            path_strip_prefixes: vec!["/build/ci/".to_string()],
        };
        assert_eq!(base.hash(), base.hash(), "hash is deterministic");
        assert_ne!(
            base.hash(),
            changed.hash(),
            "a path_strip_prefixes change must change the hash"
        );
    }

    /// A prefix-list reordering changes the hash (order is significant — prefixes
    /// are applied in order), and the US join means two lists can never collide by
    /// concatenation. Mirrors `history_hash_is_order_sensitive_and_collision_safe`.
    #[test]
    fn coverage_hash_is_order_sensitive_and_collision_safe() {
        let a = EffectiveCoverage {
            path_strip_prefixes: vec!["/a/".to_string(), "/b/".to_string()],
        };
        let reordered = EffectiveCoverage {
            path_strip_prefixes: vec!["/b/".to_string(), "/a/".to_string()],
        };
        let concatenated = EffectiveCoverage {
            path_strip_prefixes: vec!["/a//b/".to_string()],
        };
        assert_ne!(a.hash(), reordered.hash(), "order is significant");
        assert_ne!(
            a.hash(),
            concatenated.hash(),
            "the US join prevents collision"
        );
    }

    /// `Coverage::effective` substitutes the documented default (empty) for an
    /// omitted key and honours a set value.
    #[test]
    fn coverage_effective_resolves_default_and_override() {
        assert!(
            Coverage::default()
                .effective()
                .path_strip_prefixes
                .is_empty(),
            "a missing [coverage] table is all-defaults (empty prefixes)"
        );
        let overridden = Coverage {
            path_strip_prefixes: Some(vec!["/build/".to_string()]),
        }
        .effective();
        assert_eq!(overridden.path_strip_prefixes, vec!["/build/".to_string()]);
    }

    /// FR-CF-03: a misconfigured `[coverage]` key fails loud at load (exit 2)
    /// rather than silently carrying a no-op prefix.
    #[test]
    fn validate_rejects_empty_coverage_prefix() {
        let empty_prefix = Rules {
            coverage: Coverage {
                path_strip_prefixes: Some(vec!["  ".to_string()]),
            },
            ..Default::default()
        };
        assert!(
            matches!(empty_prefix.validate(), Err(crate::config::ConfigError::InvalidValue { ref key, .. }) if key == "coverage.path_strip_prefixes[0]"),
            "an empty/whitespace strip prefix is rejected"
        );

        let ok = Rules {
            coverage: Coverage {
                path_strip_prefixes: Some(vec!["/build/ci/".to_string()]),
            },
            ..Default::default()
        };
        assert!(ok.validate().is_ok(), "a valid [coverage] table validates");
    }

    /// CR-013 / FR-AN-06: `clone_similarity` must lie in the half-open range
    /// `(0, 1]`; a value at or below 0, or above 1, fails loud at load (exit 2).
    #[test]
    fn validate_rejects_out_of_range_clone_similarity() {
        for bad in [-0.1, 0.0, 1.01, 2.0] {
            let rules = Rules {
                metric_thresholds: MetricThresholds {
                    clone_similarity: Some(bad),
                    ..Default::default()
                },
                ..Default::default()
            };
            let err = rules.validate().unwrap_err();
            assert!(
                matches!(&err, crate::config::ConfigError::InvalidValue { key, .. } if key == "metric_thresholds.clone_similarity"),
                "clone_similarity = {bad} is rejected"
            );
            assert_eq!(err.exit_code(), 2);
        }

        // The boundaries: just above 0 and exactly 1 are both valid.
        for ok in [0.01, 0.85, 1.0] {
            let rules = Rules {
                metric_thresholds: MetricThresholds {
                    clone_similarity: Some(ok),
                    ..Default::default()
                },
                ..Default::default()
            };
            assert!(
                rules.validate().is_ok(),
                "clone_similarity = {ok} is in range (0, 1]"
            );
        }
    }

    /// CR-013 / FR-EX-09: `clone_min_tokens` must be a positive integer; a
    /// non-positive value fails loud at load (exit 2) on the same path as the
    /// structural thresholds.
    #[test]
    fn validate_rejects_non_positive_clone_min_tokens() {
        for bad in [0, -1, -50] {
            let rules = Rules {
                metric_thresholds: MetricThresholds {
                    clone_min_tokens: Some(bad),
                    ..Default::default()
                },
                ..Default::default()
            };
            let err = rules.validate().unwrap_err();
            assert!(
                matches!(&err, crate::config::ConfigError::InvalidValue { key, .. } if key == "metric_thresholds.clone_min_tokens"),
                "clone_min_tokens = {bad} is rejected"
            );
            assert_eq!(err.exit_code(), 2);
        }

        let ok = Rules {
            metric_thresholds: MetricThresholds {
                clone_min_tokens: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(ok.validate().is_ok(), "clone_min_tokens = 1 is positive");
    }

    /// CR-013: `MetricThresholds::effective` resolves the documented defaults for
    /// omitted near-clone keys and honours set values — the single composition
    /// seam the gate and the annotation pass both read.
    #[test]
    fn effective_resolves_clone_defaults_and_overrides() {
        let d = crate::metrics::Thresholds::default();

        // An empty table is all-defaults, including the two near-clone keys.
        let none = MetricThresholds::default().effective();
        assert_eq!(none.clone_similarity.to_bits(), d.clone_similarity.to_bits());
        assert_eq!(none.clone_min_tokens, d.clone_min_tokens);

        // A partial table overrides only its keys.
        let t = MetricThresholds {
            clone_similarity: Some(0.9),
            clone_min_tokens: Some(80),
            nesting_depth: Some(5),
            ..Default::default()
        }
        .effective();
        assert_eq!(t.clone_similarity.to_bits(), 0.9_f64.to_bits());
        assert_eq!(t.clone_min_tokens, 80);
        assert_eq!(t.nest, 5, "an unrelated structural key still resolves");
        assert_eq!(t.god_span, d.god_span, "an omitted key keeps its default");
    }

    // ── CR-043 / ADR-39: `max_dead` delta-from-blessed-baseline mode ──────────

    /// FR-GV-11 (CR-043): the retained absolute form parses to `MaxDead::Absolute`
    /// — a bare integer is unambiguously the original ceiling.
    #[test]
    fn max_dead_absolute_form_parses() {
        let rules: Rules = toml::from_str("[constraints]\nmax_dead = 5\n").unwrap();
        assert_eq!(rules.constraints.max_dead, Some(MaxDead::Absolute(5)));
        assert!(rules.validate().is_ok());
    }

    /// FR-GV-11 (CR-043): the delta form parses to `MaxDead::Baseline`; an omitted
    /// `delta` defaults to 0 ("no *new* dead code").
    #[test]
    fn max_dead_delta_form_parses_with_default_delta() {
        let rules: Rules = toml::from_str("[constraints]\nmax_dead = { baseline = 42 }\n").unwrap();
        assert_eq!(
            rules.constraints.max_dead,
            Some(MaxDead::Baseline(MaxDeadBaseline {
                baseline: 42,
                delta: 0
            }))
        );
        assert!(rules.validate().is_ok());

        let with_delta: Rules =
            toml::from_str("[constraints]\nmax_dead = { baseline = 42, delta = 3 }\n").unwrap();
        assert_eq!(
            with_delta.constraints.max_dead,
            Some(MaxDead::Baseline(MaxDeadBaseline {
                baseline: 42,
                delta: 3
            }))
        );
    }

    /// FR-CF-03 / NFR-UX-02: `#[serde(deny_unknown_fields)]` survives the untagged
    /// wrapper — a typo'd key in the delta table fails loud at load (no variant
    /// matches), never silently parsing to the absolute form or dropping the key.
    #[test]
    fn max_dead_delta_form_rejects_unknown_key() {
        let err = toml::from_str::<Rules>(
            "[constraints]\nmax_dead = { baseline = 42, bogus = 1 }\n",
        )
        .unwrap_err();
        // The exact message is serde-version-specific; the contract is that an
        // unknown key is an error rather than a silent accept.
        let _ = err;

        // A delta table that is missing its mandatory `baseline` is likewise an
        // error (it matches neither variant).
        assert!(toml::from_str::<Rules>("[constraints]\nmax_dead = { delta = 1 }\n").is_err());
    }

    /// The budget seam: `ceiling` is the absolute integer or `baseline + delta`
    /// (saturating); `as_absolute` exposes the scalar only in absolute mode.
    #[test]
    fn max_dead_ceiling_and_absolute_accessor() {
        assert_eq!(MaxDead::Absolute(5).ceiling(), 5);
        assert_eq!(MaxDead::Absolute(5).as_absolute(), Some(5));

        let delta = MaxDead::Baseline(MaxDeadBaseline {
            baseline: 42,
            delta: 3,
        });
        assert_eq!(delta.ceiling(), 45);
        assert_eq!(delta.as_absolute(), None, "delta mode has no single scalar");

        // Saturating: a degenerate baseline+delta cannot overflow the u64 ceiling.
        let huge = MaxDead::Baseline(MaxDeadBaseline {
            baseline: u32::MAX,
            delta: u32::MAX,
        });
        assert_eq!(huge.ceiling(), u64::from(u32::MAX) + u64::from(u32::MAX));
    }

    /// FR-GV-11 (CR-043): `exceeded_message` fails only when the count rises ABOVE
    /// the ceiling — the blessed steady-state passes, a newly-introduced dead
    /// function fails, and both modes are deterministic.
    #[test]
    fn max_dead_exceeded_message_semantics() {
        // Absolute: the pre-CR-043 message is preserved verbatim.
        let abs = MaxDead::Absolute(2);
        assert_eq!(abs.exceeded_message(2), None, "at the ceiling passes");
        assert_eq!(
            abs.exceeded_message(3).as_deref(),
            Some("3 dead functions exceed max_dead = 2"),
            "the absolute message is unchanged from before CR-043"
        );

        // Delta: the blessed baseline passes; one new dead function fails.
        let delta = MaxDead::Baseline(MaxDeadBaseline {
            baseline: 5,
            delta: 0,
        });
        assert_eq!(
            delta.exceeded_message(5),
            None,
            "the blessed steady-state holds the baseline"
        );
        assert_eq!(
            delta.exceeded_message(6).as_deref(),
            Some("6 dead functions exceed the blessed max_dead baseline 5 (+0 delta, ceiling 5)"),
            "a newly-introduced dead function rises above the baseline and fails"
        );

        // Delta with slack: the tolerated increase passes, one beyond it fails.
        let slack = MaxDead::Baseline(MaxDeadBaseline {
            baseline: 5,
            delta: 2,
        });
        assert_eq!(slack.exceeded_message(7), None, "baseline + delta passes");
        assert!(
            slack.exceeded_message(8).is_some(),
            "one beyond the ceiling fails"
        );
    }

    /// NFR-RA-06: both forms round-trip through serde_json byte-identically — the
    /// `rules_cache` table persists the parsed contract as JSON, so a cache
    /// hit must reconstruct the exact same budget ([FR-GV-01]).
    #[test]
    fn max_dead_round_trips_through_serde_json() {
        for budget in [
            MaxDead::Absolute(7),
            MaxDead::Baseline(MaxDeadBaseline {
                baseline: 42,
                delta: 0,
            }),
            MaxDead::Baseline(MaxDeadBaseline {
                baseline: 42,
                delta: 5,
            }),
        ] {
            let rules = Rules {
                constraints: Constraints {
                    max_dead: Some(budget.clone()),
                    ..Constraints::default()
                },
                ..Default::default()
            };
            let json = serde_json::to_string(&rules).unwrap();
            let back: Rules = serde_json::from_str(&json).unwrap();
            assert_eq!(
                back.constraints.max_dead,
                Some(budget),
                "the persisted-cache JSON round-trip is lossless"
            );
        }
    }

    // ── CR-067 / BR-37: curated recommended constraint baselines ──────────────

    /// [`Constraints::recommended`] fills every field (never leaves one `None`,
    /// which would silently omit it from the Config-editor affordance) with the
    /// documented `configuration.md` example values ([CR-067] CRA-01).
    ///
    /// [CR-067]: ../../../../docs/requests/CR-067-config-default-surfacing.md
    #[test]
    fn recommended_populates_every_field_with_documented_baselines() {
        let r = Constraints::recommended();
        assert_eq!(r.max_cycles, Some(0));
        assert_eq!(r.max_cc, Some(15));
        assert_eq!(r.max_fn_lines, Some(80));
        assert_eq!(r.no_god_files, Some(40));
        assert_eq!(r.max_fan_in, Some(30));
        assert_eq!(r.max_fan_out, Some(30));
        assert_eq!(r.max_dead, Some(MaxDead::Absolute(0)));
        assert_eq!(r.max_duplicates, Some(0));
        assert_eq!(r.max_nesting_depth, Some(4));
        assert_eq!(r.max_brain_methods, Some(0));
        assert_eq!(r.max_clone_ratio, Some(0.0));
        assert_eq!(r.no_god_containers, Some(true));
    }

    /// The recommended set is advisory-only: applying it as a `rules.toml`
    /// `[constraints]` table validates cleanly (every value is in-range) but is
    /// obviously not itself enforced by `recommended()` — enforcement only
    /// happens if a user opts in by actually writing these values ([BR-37]).
    #[test]
    fn recommended_set_is_a_valid_constraints_table() {
        let rules = Rules {
            constraints: Constraints::recommended(),
            ..Default::default()
        };
        assert!(
            rules.validate().is_ok(),
            "the curated recommended baselines must themselves pass validation"
        );
    }
}
