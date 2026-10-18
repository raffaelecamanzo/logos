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

    /// The security gate table (`[security]`, [FR-SC-10], [ADR-79]) — `None`
    /// when the contract has no such table. Its presence is the whole opt-in:
    /// without it no security evidence is read and `check` / `gate` output is
    /// byte-identical whatever evidence exists ([BR-65]). Omitted from the
    /// serialised contract when absent, so every other read-model of the
    /// contract (the rules cache, the config view) is unchanged.
    ///
    /// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
    /// [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
    /// [BR-65]: ../../../../docs/specs/software-spec.md#328-security-findings
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<SecurityRules>,
}

/// `rules.toml [security]` — the security gate ([FR-SC-10], [ADR-79] §6–7).
///
/// ```toml
/// [security]
/// required_scanners = ["semgrep", "gitleaks"]  # each needs a fresh scan at HEAD, else exit 5
/// ratchet_min_severity = "high"                # new unsuppressed findings at or above fail (default high)
/// max_severity = "critical"                    # any unsuppressed finding at or above fails
/// fail_on_owasp = ["A05", "A01:2021"]          # any unsuppressed finding in a category fails
/// ```
///
/// - **Ratchet.** An unsuppressed finding at or above `ratchet_min_severity`
///   whose fingerprint is not in the set `gate --save` recorded fails the run.
///   Findings already recorded are tolerated; the set lives in `history.db`.
/// - **Caps.** `max_severity` and `fail_on_owasp` fail on any unsuppressed
///   finding they match, recorded or not.
/// - **Required scanners.** A scanner named here that has no fresh scan at HEAD
///   — never run, stale, not installed or disabled — makes `check` and `gate`
///   exit **5**. A scanner this list does not name never does.
///
/// Suppressions in `.logos/security-ignore.toml` exclude a finding from every
/// rule ([FR-SC-11]).
///
/// [FR-SC-10]: ../../../../docs/specs/requirements/FR-SC-10.md
/// [FR-SC-11]: ../../../../docs/specs/requirements/FR-SC-11.md
/// [ADR-79]: ../../../../docs/specs/architecture/decisions/ADR-79.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecurityRules {
    /// Scanners that must have a fresh scan at HEAD: a registry name
    /// (`semgrep`) or a report's driver name (`Semgrep OSS`), either case.
    #[serde(default)]
    pub required_scanners: Vec<String>,
    /// The least severity the ratchet gates on; default `high`.
    #[serde(default = "SecurityRules::default_ratchet_min_severity")]
    pub ratchet_min_severity: crate::model::Severity,
    /// The cap: any unsuppressed finding at or above it fails, recorded or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_severity: Option<crate::model::Severity>,
    /// OWASP Top 10 categories that fail on any unsuppressed finding: `A05`
    /// (the 2025 edition) or an explicit `A05:2025` / `A05:2021`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fail_on_owasp: Vec<String>,
}

impl Default for SecurityRules {
    fn default() -> Self {
        Self {
            required_scanners: Vec::new(),
            ratchet_min_severity: Self::default_ratchet_min_severity(),
            max_severity: None,
            fail_on_owasp: Vec::new(),
        }
    }
}

impl SecurityRules {
    fn default_ratchet_min_severity() -> crate::model::Severity {
        crate::model::Severity::High
    }

    /// The `fail_on_owasp` entries as full category ids (`A05:2025`), in
    /// declared order and each once: a bare `A05` names the 2025 edition, the
    /// default, so `A05`, ` A05 ` and `A05:2025` are one category.
    pub fn owasp_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = Vec::new();
        for token in &self.fail_on_owasp {
            let token = token.trim();
            let id = if token.contains(':') { token.to_string() } else { format!("{token}:2025") };
            if !categories.contains(&id) {
                categories.push(id);
            }
        }
        categories
    }

    /// Reject a blank scanner name and an `fail_on_owasp` entry that is not an
    /// OWASP Top 10 category (`A01`..`A10`, optionally `:2025` or `:2021`).
    fn validate(&self) -> Result<(), super::error::ConfigError> {
        for (idx, name) in self.required_scanners.iter().enumerate() {
            if name.trim().is_empty() {
                return Err(super::error::ConfigError::InvalidValue {
                    key: format!("security.required_scanners[{idx}]"),
                    message: "must not be empty".to_string(),
                });
            }
        }
        for (idx, token) in self.fail_on_owasp.iter().enumerate() {
            let (id, edition) = token.trim().split_once(':').map_or((token.trim(), None), |(i, e)| (i, Some(e)));
            let number = id.strip_prefix('A').filter(|n| n.len() == 2 && n.bytes().all(|b| b.is_ascii_digit()));
            let in_range = number.and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=10).contains(&n));
            if !in_range || !matches!(edition, None | Some("2025") | Some("2021")) {
                return Err(super::error::ConfigError::InvalidValue {
                    key: format!("security.fail_on_owasp[{idx}]"),
                    message: format!(
                        "{token:?} is not an OWASP Top 10 category: use A01..A10, optionally with :2025 or :2021"
                    ),
                });
            }
        }
        Ok(())
    }
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
#[serde(untagged, expecting = "an integer, `{ baseline = N, delta = D }`, or `{ max = 0, ratchet = true }`")]
pub enum MaxDead {
    /// The original absolute ceiling: the gate fails iff the project-wide dead
    /// count exceeds this integer ([CR-002], retained unchanged by [CR-043]).
    ///
    /// [CR-002]: ../../../../docs/requests/CR-002-extended-architecture-contracts.md
    Absolute(u32),
    /// The delta-from-blessed-baseline form ([CR-043], [ADR-39]).
    Baseline(MaxDeadBaseline),
    /// The ratchet form `{ max = 0, ratchet = true }` ([FR-GV-23], [ADR-73]):
    /// every dead production function is one offender, classified against the
    /// checked-in blessed set. With `ratchet = false` it is the absolute
    /// ceiling `max`.
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    /// [ADR-73]: ../../../../docs/specs/architecture/decisions/ADR-73.md
    Ratchet(RatchetBudget),
}

/// The count-delta-from-blessed-baseline table ([CR-043], [ADR-39]), shared by
/// every budget over a count that names no symbol: `max_dead`'s delta form and,
/// since [FR-GV-23], `max_cycles` ([`CountBudget`]).
///
/// The gate fails when the count exceeds `baseline + delta`. `baseline` is
/// the blessed steady-state count (human-reviewed, re-blessed after the
/// precision work lands); `delta` is the tolerated increase above it before the
/// gate fails and defaults to `0` ("no *new* debt"). Both fields are `u32`,
/// so the form is always in range — there is nothing further to range-check at
/// load; `#[serde(deny_unknown_fields)]` still fails a typo'd key loud (exit 2,
/// [NFR-UX-02]).
///
/// [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../../docs/specs/architecture/decisions/ADR-39.md
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
/// [NFR-UX-02]: ../../../../docs/specs/requirements/NFR-UX-02.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountBaseline {
    /// The blessed steady-state count.
    pub baseline: u32,
    /// Tolerated increase above `baseline` before the gate fails (default 0).
    #[serde(default)]
    pub delta: u32,
}

/// `max_dead`'s name for the shared [`CountBaseline`] table — the spelling
/// [CR-043] introduced, kept so its form and its callers read unchanged.
///
/// [CR-043]: ../../../../docs/requests/CR-043-dead-code-detector-precision.md
pub type MaxDeadBaseline = CountBaseline;

/// A count-delta table's ceiling: `baseline + delta`, saturating.
fn baseline_ceiling(b: &CountBaseline) -> u64 {
    u64::from(b.baseline).saturating_add(u64::from(b.delta))
}

/// The count-delta message every delta-form budget uses when `count` exceeds
/// `b`'s ceiling, else `None` — the sibling of [`absolute_exceeded`]. `noun`
/// names what is counted (`dead functions`, `dependency cycles`).
fn baseline_exceeded(b: &CountBaseline, count: u64, key: &str, noun: &str) -> Option<String> {
    let ceiling = baseline_ceiling(b);
    (count > ceiling).then(|| {
        format!(
            "{count} {noun} exceed the blessed {key} baseline {} (+{} delta, ceiling {ceiling})",
            b.baseline, b.delta
        )
    })
}

/// The absolute-ceiling message every count budget has always used:
/// `"{count} {noun} exceed {key} = {max}"`, or `None` within budget.
fn absolute_exceeded(count: u64, max: u32, key: &str, noun: &str) -> Option<String> {
    (count > u64::from(max)).then(|| format!("{count} {noun} exceed {key} = {max}"))
}

/// The ratchet table `{ max = N, ratchet = true }` a symbol-naming budget may
/// declare in place of its scalar ([FR-GV-23], [ADR-73]).
///
/// `max` keeps the scalar's meaning. With `ratchet = true`, each offender is
/// classified against the checked-in blessed set (`.logos/rules-baseline.json`):
/// blessed at or below its blessed value is a `warning`, new or worse is an
/// `error`. `ratchet` defaults to `false`, which is the scalar form spelled as
/// a table. `#[serde(deny_unknown_fields)]` fails a typo'd key loud (exit 2).
///
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
/// [ADR-73]: ../../../../docs/specs/architecture/decisions/ADR-73.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatchetBudget {
    /// The budget's ceiling — per symbol for a per-symbol budget, and `0` for
    /// a symbol-set budget under ratchet (each member is one offender).
    pub max: u32,
    /// Classify offenders against the blessed set (default `false`).
    #[serde(default)]
    pub ratchet: bool,
}

/// A symbol-naming numeric budget ([FR-GV-11], [FR-GV-23]): the original
/// scalar (`max_cc = 15`), or the [`RatchetBudget`] table
/// (`max_cc = { max = 15, ratchet = true }`). Disambiguated by value type at
/// load, exactly as [`MaxDead`] is; the scalar serialises back as the bare
/// integer, so every existing contract and read-model is unchanged.
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, expecting = "a non-negative integer, or `{ max = N, ratchet = true }`")]
pub enum Budget {
    /// The original scalar ceiling.
    Absolute(u32),
    /// The ratchet table.
    Ratchet(RatchetBudget),
}

impl Budget {
    /// The ceiling, whichever form declared it.
    pub fn max(&self) -> u32 {
        match self {
            Budget::Absolute(max) => *max,
            Budget::Ratchet(r) => r.max,
        }
    }

    /// `true` when the budget classifies against the blessed set.
    pub fn is_ratchet(&self) -> bool {
        matches!(self, Budget::Ratchet(r) if r.ratchet)
    }
}

/// A budget over a count that names no symbol ([FR-GV-23]): `max_cycles`.
/// The scalar ceiling, or [`max_dead`](MaxDead)'s count-delta table
/// `{ baseline = B, delta = D }` ([ADR-39]) generalised to it.
///
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
/// [ADR-39]: ../../../../docs/specs/architecture/decisions/ADR-39.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, expecting = "a non-negative integer, or `{ baseline = N, delta = D }`")]
pub enum CountBudget {
    /// The scalar ceiling: fail iff the count exceeds it.
    Absolute(u32),
    /// The delta form: fail iff the count exceeds `baseline + delta`.
    Baseline(CountBaseline),
}

impl CountBudget {
    /// The `check_rules` violation message when `count` exceeds the budget, or
    /// `None` within it. `key` is the constraint name and `noun` what is
    /// counted. The scalar form's phrasing is the pre-[FR-GV-23] one verbatim.
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    pub fn exceeded_message(&self, count: u64, key: &str, noun: &str) -> Option<String> {
        match self {
            CountBudget::Absolute(max) => absolute_exceeded(count, *max, key, noun),
            CountBudget::Baseline(b) => baseline_exceeded(b, count, key, noun),
        }
    }
}

/// The `max_clone_ratio` budget ([FR-GV-11] ext., [FR-GV-23]): the scalar ratio
/// ceiling, or the count-delta form in ratio units,
/// `{ baseline = 0.12, delta = 0 }` — fail iff the ratio exceeds
/// `baseline + delta`. Both values are range-checked at load (exit 2).
///
/// `Eq` is not derived: the ratio is an `f64`.
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged, expecting = "a ratio in [0.0, 1.0], or `{ baseline = R, delta = D }`")]
pub enum RatioBudget {
    /// The scalar ratio ceiling in `[0.0, 1.0]`.
    Absolute(f64),
    /// The blessed-baseline form.
    Baseline(RatioBaseline),
}

/// The `max_clone_ratio` delta table: a blessed ratio and a tolerated rise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatioBaseline {
    /// The blessed near-clone ratio, in `[0.0, 1.0]`.
    pub baseline: f64,
    /// Tolerated rise above `baseline` before the gate fails (default 0).
    #[serde(default)]
    pub delta: f64,
}

impl RatioBudget {
    /// The ratio the measured value must not exceed.
    pub fn ceiling(&self) -> f64 {
        match self {
            RatioBudget::Absolute(max) => *max,
            RatioBudget::Baseline(b) => b.baseline + b.delta,
        }
    }

    /// How the budget names itself in a violation message: `max_clone_ratio =
    /// 0.1` for the scalar (the pre-[FR-GV-23] phrasing), the blessed baseline
    /// and delta for the table.
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    pub fn describe(&self) -> String {
        match self {
            RatioBudget::Absolute(max) => format!("max_clone_ratio = {max}"),
            RatioBudget::Baseline(b) => format!(
                "the blessed max_clone_ratio baseline {} (+{} delta, ceiling {})",
                b.baseline,
                b.delta,
                self.ceiling()
            ),
        }
    }
}

/// The `no_god_containers` budget ([FR-GV-11] ext., [FR-GV-23]): the original
/// flag, or `{ ratchet = true }` — enforced, with each god container classified
/// against the blessed set by its member-method count.
///
/// [FR-GV-11]: ../../../../docs/specs/requirements/FR-GV-11.md
/// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, expecting = "`true`, `false`, or `{ ratchet = true }`")]
pub enum GodContainersBudget {
    /// `true` enforces, `false` does not.
    Flag(bool),
    /// The ratchet table — always enforced.
    Ratchet(RatchetFlag),
}

/// `{ ratchet = true }` — the ratchet table of a flag budget, which has no `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RatchetFlag {
    /// Classify offenders against the blessed set.
    pub ratchet: bool,
}

impl GodContainersBudget {
    /// `true` when the budget is enforced at all.
    pub fn enforced(&self) -> bool {
        match self {
            GodContainersBudget::Flag(on) => *on,
            GodContainersBudget::Ratchet(_) => true,
        }
    }

    /// `true` when offenders classify against the blessed set.
    pub fn is_ratchet(&self) -> bool {
        matches!(self, GodContainersBudget::Ratchet(r) if r.ratchet)
    }
}

impl MaxDead {
    /// The effective ceiling the project-wide dead count must not exceed: the
    /// absolute integer, or `baseline + delta` (saturating) for the delta form,
    /// or the ratchet table's `max`.
    ///
    /// Internal to the crate (the governance evaluator and this module's tests
    /// are the only callers) — the public budget seam is [`Self::exceeded_message`].
    pub(crate) fn ceiling(&self) -> u64 {
        match self {
            MaxDead::Absolute(max) => u64::from(*max),
            MaxDead::Baseline(b) => baseline_ceiling(b),
            MaxDead::Ratchet(r) => u64::from(r.max),
        }
    }

    /// `true` when the dead set classifies against the blessed set
    /// ([FR-GV-23]).
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    pub fn is_ratchet(&self) -> bool {
        matches!(self, MaxDead::Ratchet(r) if r.ratchet)
    }

    /// The absolute ceiling when the budget is in absolute mode, else `None`.
    ///
    /// Lets a scalar-only consumer (the web config editor's typed `max_dead`
    /// field, [FR-UI-12]) render the absolute form while a delta- or
    /// ratchet-form contract is edited through the authoritative raw-TOML pane.
    ///
    /// [FR-UI-12]: ../../../../docs/specs/requirements/FR-UI-12.md
    pub fn as_absolute(&self) -> Option<u32> {
        match self {
            MaxDead::Absolute(max) => Some(*max),
            MaxDead::Baseline(_) | MaxDead::Ratchet(_) => None,
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
        const KEY: &str = "max_dead";
        const NOUN: &str = "dead functions";
        if dead <= self.ceiling() {
            return None;
        }
        match self {
            MaxDead::Absolute(max) => absolute_exceeded(dead, *max, KEY, NOUN),
            MaxDead::Baseline(b) => baseline_exceeded(b, dead, KEY, NOUN),
            MaxDead::Ratchet(r) => absolute_exceeded(dead, r.max, KEY, NOUN),
        }
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
    /// Maximum allowed dependency cycles — the scalar, or the count-delta form
    /// `{ baseline = B, delta = D }` ([FR-GV-23], [`CountBudget`]).
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    pub max_cycles: Option<CountBudget>,
    /// Maximum cyclomatic complexity per function ([`Budget`]: scalar or ratchet).
    pub max_cc: Option<Budget>,
    /// Maximum lines per function ([`Budget`]).
    pub max_fn_lines: Option<Budget>,
    /// Maximum symbols per file before a file is flagged a "god file" (OQ-06);
    /// under ratchet the file path is the offender's identity ([`Budget`]).
    pub no_god_files: Option<Budget>,
    /// Maximum inbound dependency edges for any one symbol ([FR-GV-11], [BR-19]);
    /// under ratchet the module key is the offender's identity ([`Budget`]).
    pub max_fan_in: Option<Budget>,
    /// Maximum outbound dependency edges for any one symbol ([FR-GV-11], [BR-19]);
    /// under ratchet the module key is the offender's identity ([`Budget`]).
    pub max_fan_out: Option<Budget>,
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
    /// Maximum project-wide count of `is_duplicate` functions ([FR-GV-11]);
    /// under ratchet (`{ max = 0, ratchet = true }`) each duplicate is one
    /// offender of a symbol set ([`Budget`]).
    pub max_duplicates: Option<Budget>,
    /// Maximum per-function maximum nesting depth ([FR-GV-11] ext., [FR-EX-07],
    /// [CR-005]); a production function deeper than this is one error violation.
    pub max_nesting_depth: Option<Budget>,
    /// Maximum project-wide count of brain methods ([FR-GV-11] ext., [FR-QM-10],
    /// [CR-005]); a brain method meets all three brain thresholds. Under ratchet
    /// each brain method is one offender of a symbol set ([`Budget`]).
    pub max_brain_methods: Option<Budget>,
    /// Maximum project-wide near-clone production-function ratio ([FR-GV-11] ext.,
    /// [FR-AN-06], [CR-005]); `clone_group IS NOT NULL` over production functions.
    /// The scalar, or the count-delta form in ratio units ([`RatioBudget`]).
    pub max_clone_ratio: Option<RatioBudget>,
    /// When `true`, no class-like container may be over the god thresholds
    /// ([FR-GV-11] ext., [FR-QM-12], [CR-005]); each god container is one error
    /// violation. `false`/omitted is not enforced; `{ ratchet = true }` enforces
    /// against the blessed set ([`GodContainersBudget`]).
    pub no_god_containers: Option<GodContainersBudget>,
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
            max_cycles: Some(CountBudget::Absolute(0)),
            max_cc: Some(Budget::Absolute(15)),
            max_fn_lines: Some(Budget::Absolute(80)),
            no_god_files: Some(Budget::Absolute(40)),
            max_fan_in: Some(Budget::Absolute(30)),
            max_fan_out: Some(Budget::Absolute(30)),
            max_dead: Some(MaxDead::Absolute(0)),
            max_duplicates: Some(Budget::Absolute(0)),
            max_nesting_depth: Some(Budget::Absolute(4)),
            max_brain_methods: Some(Budget::Absolute(0)),
            max_clone_ratio: Some(RatioBudget::Absolute(0.0)),
            no_god_containers: Some(GodContainersBudget::Flag(true)),
        }
    }

    /// The constraints declared `ratchet = true` ([FR-GV-23]), in the canonical
    /// evaluation order — the rules whose offenders classify against the
    /// blessed set, and whose blessed entries can be reported fixed.
    ///
    /// [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
    pub fn ratchet_rules(&self) -> Vec<&'static str> {
        let budget = |b: &Option<Budget>| b.as_ref().is_some_and(Budget::is_ratchet);
        [
            ("max_cc", budget(&self.max_cc)),
            ("max_fn_lines", budget(&self.max_fn_lines)),
            ("no_god_files", budget(&self.no_god_files)),
            ("max_fan_in", budget(&self.max_fan_in)),
            ("max_fan_out", budget(&self.max_fan_out)),
            ("max_dead", self.max_dead.as_ref().is_some_and(MaxDead::is_ratchet)),
            ("max_duplicates", budget(&self.max_duplicates)),
            ("max_nesting_depth", budget(&self.max_nesting_depth)),
            ("max_brain_methods", budget(&self.max_brain_methods)),
            (
                "no_god_containers",
                self.no_god_containers
                    .as_ref()
                    .is_some_and(GodContainersBudget::is_ratchet),
            ),
        ]
        .into_iter()
        .filter_map(|(rule, on)| on.then_some(rule))
        .collect()
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
/// ([FR-AN-06]) and `clone_min_tokens` ([FR-EX-09]) — and, since S-501, the
/// exact-duplicate token floor `duplicate_min_tokens` ([FR-AN-02]).
/// [`MetricThresholds::effective`] composes these onto the documented defaults
/// to build the effective set whose hash gates the baseline.
///
/// `Eq` is not derived: `clone_similarity` is an `f64` (the same reason
/// [`Constraints`] and [`Rules`] are `PartialEq`-only).
///
/// [`Thresholds::default`]: crate::metrics::Thresholds
/// [FR-QM-13]: ../../../../docs/specs/requirements/FR-QM-13.md
/// [FR-AN-02]: ../../../../docs/specs/requirements/FR-AN-02.md
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
    /// The normalized-token floor a function's body needs to be an **exact
    /// duplicate** ([FR-AN-02], [S-501], [CR-163]); default 50, a positive
    /// integer. Beside the has-body requirement it keeps a four-line constant
    /// override, or a bodyless declaration, from counting as copy-paste. A
    /// non-positive value fails validation (exit 2). Distinct from
    /// `clone_min_tokens`, which gates near-clone shingling only.
    ///
    /// [FR-AN-02]: ../../../../docs/specs/requirements/FR-AN-02.md
    /// [S-501]: ../../../../docs/planning/journal.md#s-501-exact-duplicates-require-a-body-and-a-token-floor
    /// [CR-163]: ../../../../docs/requests/CR-163-structural-metrics-stop-misfiring-on-declarative-code.md
    pub duplicate_min_tokens: Option<i64>,
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
            duplicate_min_tokens: self
                .duplicate_min_tokens
                .unwrap_or(d.duplicate_min_tokens),
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
/// ([FR-AN-05]) — the SAME static reachability `test_reachable_set` computes.
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
        match self.constraints.max_clone_ratio {
            Some(RatioBudget::Absolute(ratio)) if !(0.0..=1.0).contains(&ratio) => {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "constraints.max_clone_ratio".to_string(),
                    message: format!("{ratio} is outside the valid ratio range [0.0, 1.0]"),
                });
            }
            Some(RatioBudget::Baseline(b))
                if !(0.0..=1.0).contains(&b.baseline) || !(0.0..=1.0).contains(&b.delta) =>
            {
                return Err(super::error::ConfigError::InvalidValue {
                    key: "constraints.max_clone_ratio".to_string(),
                    message: format!(
                        "baseline {} and delta {} must each lie in [0.0, 1.0]",
                        b.baseline, b.delta
                    ),
                });
            }
            _ => {}
        }
        // FR-GV-23: a symbol-set budget under ratchet names every member of the
        // set as one offender, so a `max` above 0 would silently tolerate that
        // many *unnamed* members and make the classification meaningless.
        // Reject it loud (exit 2) rather than guess which members it meant.
        let c = &self.constraints;
        let set_max = |b: &Option<Budget>| b.filter(Budget::is_ratchet).map(|b| b.max());
        let dead_max = match &c.max_dead {
            Some(MaxDead::Ratchet(r)) if r.ratchet => Some(r.max),
            _ => None,
        };
        for (key, max) in [
            ("max_dead", dead_max),
            ("max_duplicates", set_max(&c.max_duplicates)),
            ("max_brain_methods", set_max(&c.max_brain_methods)),
        ] {
            if let Some(max) = max.filter(|m| *m != 0) {
                return Err(super::error::ConfigError::InvalidValue {
                    key: format!("constraints.{key}"),
                    message: format!(
                        "max = {max} under ratchet: a symbol-set budget names each member as \
                         one offender, so its ratchet form is `{{ max = 0, ratchet = true }}`"
                    ),
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
            // S-501: the exact-duplicate token floor, same exit-2 path.
            ("duplicate_min_tokens", mt.duplicate_min_tokens),
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
        // FR-SC-10: the `[security]` table's scanner names and OWASP categories
        // are checked at load (exit 2), so a typo never reads as a clean gate.
        if let Some(security) = &self.security {
            security.validate()?;
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

    /// S-501 / FR-AN-02: `duplicate_min_tokens` must be a positive integer, on
    /// the same exit-2 path as `clone_min_tokens`.
    #[test]
    fn validate_rejects_non_positive_duplicate_min_tokens() {
        for bad in [0, -1, -50] {
            let rules = Rules {
                metric_thresholds: MetricThresholds {
                    duplicate_min_tokens: Some(bad),
                    ..Default::default()
                },
                ..Default::default()
            };
            let err = rules.validate().unwrap_err();
            assert!(
                matches!(&err, crate::config::ConfigError::InvalidValue { key, .. } if key == "metric_thresholds.duplicate_min_tokens"),
                "duplicate_min_tokens = {bad} is rejected"
            );
            assert_eq!(err.exit_code(), 2);
        }

        let ok = Rules {
            metric_thresholds: MetricThresholds {
                duplicate_min_tokens: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(ok.validate().is_ok(), "duplicate_min_tokens = 1 is positive");
    }

    /// S-501: `effective` resolves `duplicate_min_tokens` to its default of 50 when
    /// omitted, honours an override, and keeps it independent of `clone_min_tokens`.
    #[test]
    fn effective_resolves_duplicate_min_tokens_default_and_override() {
        assert_eq!(MetricThresholds::default().effective().duplicate_min_tokens, 50);

        let t = MetricThresholds {
            duplicate_min_tokens: Some(5),
            ..Default::default()
        }
        .effective();
        assert_eq!(t.duplicate_min_tokens, 5);
        assert_eq!(
            t.clone_min_tokens, 50,
            "tuning the exact-duplicate floor leaves the near-clone floor alone"
        );
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
        assert_eq!(r.max_cycles, Some(CountBudget::Absolute(0)));
        assert_eq!(r.max_cc, Some(Budget::Absolute(15)));
        assert_eq!(r.max_fn_lines, Some(Budget::Absolute(80)));
        assert_eq!(r.no_god_files, Some(Budget::Absolute(40)));
        assert_eq!(r.max_fan_in, Some(Budget::Absolute(30)));
        assert_eq!(r.max_fan_out, Some(Budget::Absolute(30)));
        assert_eq!(r.max_dead, Some(MaxDead::Absolute(0)));
        assert_eq!(r.max_duplicates, Some(Budget::Absolute(0)));
        assert_eq!(r.max_nesting_depth, Some(Budget::Absolute(4)));
        assert_eq!(r.max_brain_methods, Some(Budget::Absolute(0)));
        assert_eq!(r.max_clone_ratio, Some(RatioBudget::Absolute(0.0)));
        assert_eq!(r.no_god_containers, Some(GodContainersBudget::Flag(true)));
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

    // ── FR-GV-23 / S-507: the ratchet and count-delta forms ────────────────────

    fn constraints(toml_text: &str) -> Constraints {
        let rules: Rules = toml::from_str(&format!("[constraints]\n{toml_text}\n"))
            .unwrap_or_else(|e| panic!("{toml_text}: {e}"));
        rules.validate().unwrap_or_else(|e| panic!("{toml_text}: {e}"));
        rules.constraints
    }

    /// Every symbol-naming budget accepts `{ max = N, ratchet = true }` beside
    /// its scalar, and `ratchet_rules` names exactly the ratcheted ones.
    #[test]
    fn every_symbol_naming_budget_accepts_the_ratchet_table() {
        let c = constraints(
            "max_cc = { max = 15, ratchet = true }\n\
             max_fn_lines = { max = 80, ratchet = true }\n\
             no_god_files = { max = 40, ratchet = true }\n\
             max_fan_in = { max = 30, ratchet = true }\n\
             max_fan_out = { max = 30, ratchet = true }\n\
             max_dead = { max = 0, ratchet = true }\n\
             max_duplicates = { max = 0, ratchet = true }\n\
             max_nesting_depth = { max = 4, ratchet = true }\n\
             max_brain_methods = { max = 0, ratchet = true }\n\
             no_god_containers = { ratchet = true }",
        );
        let ratchet = |max| Some(Budget::Ratchet(RatchetBudget { max, ratchet: true }));
        assert_eq!(c.max_cc, ratchet(15));
        assert_eq!(c.max_dead, Some(MaxDead::Ratchet(RatchetBudget { max: 0, ratchet: true })));
        assert_eq!(
            c.no_god_containers,
            Some(GodContainersBudget::Ratchet(RatchetFlag { ratchet: true }))
        );
        assert_eq!(
            c.ratchet_rules(),
            [
                "max_cc",
                "max_fn_lines",
                "no_god_files",
                "max_fan_in",
                "max_fan_out",
                "max_dead",
                "max_duplicates",
                "max_nesting_depth",
                "max_brain_methods",
                "no_god_containers",
            ]
        );
    }

    /// The scalar forms parse and serialise exactly as before, so an existing
    /// contract — and every read-model that renders it — is unchanged; and a
    /// table with `ratchet = false` (or omitted) is not under ratchet.
    #[test]
    fn scalar_forms_are_unchanged_and_ratchet_false_is_not_a_ratchet() {
        let c = constraints("max_cc = 15\nmax_cycles = 2\nmax_clone_ratio = 0.1\nno_god_containers = true");
        assert_eq!(c.max_cc, Some(Budget::Absolute(15)));
        assert_eq!(c.max_cycles, Some(CountBudget::Absolute(2)));
        assert_eq!(c.max_clone_ratio, Some(RatioBudget::Absolute(0.1)));
        assert_eq!(c.no_god_containers, Some(GodContainersBudget::Flag(true)));
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["max_cc"], 15);
        assert_eq!(json["max_cycles"], 2);
        assert_eq!(json["max_clone_ratio"], 0.1);
        assert_eq!(json["no_god_containers"], true);
        assert!(c.ratchet_rules().is_empty());

        let c = constraints("max_cc = { max = 15 }\nmax_fn_lines = { max = 80, ratchet = false }");
        assert_eq!(c.max_cc.map(|b| b.max()), Some(15));
        assert!(c.ratchet_rules().is_empty(), "ratchet defaults to false");

        // A cached parse round-trips through JSON (the rules cache, FR-GV-01).
        let ratcheted = constraints("max_cc = { max = 15, ratchet = true }");
        let back: Constraints =
            serde_json::from_str(&serde_json::to_string(&ratcheted).unwrap()).unwrap();
        assert_eq!(back, ratcheted);
    }

    /// `max_cycles` and `max_clone_ratio` take the count-delta form; the delta
    /// defaults to 0. `max_dead`'s inline delta form is unchanged.
    #[test]
    fn count_delta_form_parses_for_cycles_and_clone_ratio() {
        let c = constraints(
            "max_cycles = { baseline = 2 }\nmax_clone_ratio = { baseline = 0.12, delta = 0 }\n\
             max_dead = { baseline = 42 }",
        );
        assert_eq!(
            c.max_cycles,
            Some(CountBudget::Baseline(CountBaseline { baseline: 2, delta: 0 }))
        );
        assert_eq!(
            c.max_clone_ratio,
            Some(RatioBudget::Baseline(RatioBaseline { baseline: 0.12, delta: 0.0 }))
        );
        assert_eq!(
            c.max_dead,
            Some(MaxDead::Baseline(MaxDeadBaseline { baseline: 42, delta: 0 }))
        );
        assert_eq!(c.max_clone_ratio.unwrap().ceiling(), 0.12);
    }

    /// A value matching no form names the forms the key accepts, rather than
    /// serde's bare "did not match any variant" (NFR-UX-02).
    #[test]
    fn a_value_matching_no_form_names_the_accepted_forms() {
        for (bad, expected) in [
            ("max_cc = -1", "a non-negative integer, or `{ max = N, ratchet = true }`"),
            ("max_cc = { max = 15, ratchett = true }", "`{ max = N, ratchet = true }`"),
            ("max_cycles = { baseline = 2, delt = 1 }", "`{ baseline = N, delta = D }`"),
            ("max_clone_ratio = \"x\"", "a ratio in [0.0, 1.0]"),
            ("no_god_containers = 1", "`{ ratchet = true }`"),
            ("max_dead = \"5\"", "`{ max = 0, ratchet = true }`"),
        ] {
            let err = toml::from_str::<Rules>(&format!("[constraints]\n{bad}\n"))
                .expect_err(bad)
                .to_string();
            assert!(err.contains(expected), "{bad}: {err}");
            assert!(!err.contains("did not match any variant"), "{bad}: {err}");
        }
    }

    /// Misdeclared ratchet and delta tables fail loud at load (exit 2): an
    /// unknown key, a ratchet table on a no-identity budget, a symbol-set
    /// ratchet with `max > 0`, and an out-of-range clone-ratio baseline.
    #[test]
    fn malformed_ratchet_and_delta_tables_fail_loud() {
        for bad in [
            "max_cc = { max = 15, ratchet = true, bogus = 1 }",
            "max_cc = { ratchet = true }",
            "max_cycles = { max = 2, ratchet = true }",
            "max_clone_ratio = { max = 0, ratchet = true }",
            "no_god_containers = { max = 0, ratchet = true }",
        ] {
            assert!(
                toml::from_str::<Rules>(&format!("[constraints]\n{bad}\n")).is_err(),
                "{bad} must not parse"
            );
        }
        for (bad, key) in [
            ("max_dead = { max = 3, ratchet = true }", "constraints.max_dead"),
            ("max_duplicates = { max = 1, ratchet = true }", "constraints.max_duplicates"),
            ("max_brain_methods = { max = 2, ratchet = true }", "constraints.max_brain_methods"),
            ("max_clone_ratio = { baseline = 1.5 }", "constraints.max_clone_ratio"),
            ("max_clone_ratio = { baseline = 0.1, delta = -0.1 }", "constraints.max_clone_ratio"),
        ] {
            let rules: Rules = toml::from_str(&format!("[constraints]\n{bad}\n")).unwrap();
            let err = rules.validate().expect_err(bad).to_string();
            assert!(err.contains(key), "{bad}: {err}");
        }
        // The near miss: a symbol-set budget with `max > 0` NOT under ratchet is
        // the ordinary count ceiling and validates.
        constraints("max_dead = { max = 3, ratchet = false }\nmax_duplicates = { max = 1 }");
    }

    // ── FR-SC-10 (S-668): the `[security]` table ──────────────────────────────

    /// Every key is optional: an empty table requires nothing, ratchets at
    /// `high` and sets no cap; a full one parses each key as written.
    #[test]
    fn the_security_table_parses_with_its_defaults() {
        let rules: Rules = toml::from_str("[security]\n").unwrap();
        let table = rules.security.expect("an empty table is still present");
        assert_eq!(table, SecurityRules::default());
        assert_eq!(table.ratchet_min_severity, crate::model::Severity::High);
        assert!(table.required_scanners.is_empty() && table.max_severity.is_none());

        let rules: Rules = toml::from_str(
            "[security]\nrequired_scanners = [\"semgrep\", \"Gitleaks\"]\nratchet_min_severity = \"medium\"\n\
             max_severity = \"critical\"\nfail_on_owasp = [\"A05\", \"A01:2021\"]\n",
        )
        .unwrap();
        rules.validate().unwrap();
        let table = rules.security.unwrap();
        assert_eq!(table.required_scanners, ["semgrep", "Gitleaks"]);
        assert_eq!(table.ratchet_min_severity, crate::model::Severity::Medium);
        assert_eq!(table.max_severity, Some(crate::model::Severity::Critical));
        assert_eq!(table.owasp_categories(), ["A05:2025", "A01:2021"], "a bare id is the 2025 edition");

        // Padding is not part of the id, and one category named three ways is
        // evaluated — and counted — once.
        let rules: Rules =
            toml::from_str("[security]\nfail_on_owasp = [\" A05 \", \"A03:2021\", \"A05:2025\", \"A05\"]\n").unwrap();
        rules.validate().unwrap();
        assert_eq!(rules.security.unwrap().owasp_categories(), ["A05:2025", "A03:2021"]);
    }

    /// No table is `None`, and the serialised contract — the rules cache's JSON,
    /// the config view's payload — carries no `security` key, so it is unchanged.
    /// A present table round-trips through that JSON.
    #[test]
    fn an_absent_security_table_leaves_the_serialised_contract_unchanged() {
        let rules: Rules = toml::from_str("[constraints]\nmax_cc = 15\n").unwrap();
        assert!(rules.security.is_none());
        assert!(!serde_json::to_string(&rules).unwrap().contains("security"));

        let rules: Rules =
            toml::from_str("[security]\nrequired_scanners = [\"semgrep\"]\nmax_severity = \"high\"\n").unwrap();
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<Rules>(&json).unwrap(), rules);
    }

    /// A misconfigured table fails loud at load: an unknown key or severity at
    /// parse, a blank scanner or a non-category OWASP entry at validation — each
    /// beside its one-character near miss, which passes.
    #[test]
    fn a_misconfigured_security_table_fails_loud() {
        for bad in [
            "required = [\"semgrep\"]",
            "max_severity = \"severe\"",
            "ratchet_min_severity = \"High\"",
        ] {
            assert!(toml::from_str::<Rules>(&format!("[security]\n{bad}\n")).is_err(), "{bad} must not parse");
        }
        for (bad, key) in [
            ("required_scanners = [\" \"]", "security.required_scanners[0]"),
            ("fail_on_owasp = [\"A11\"]", "security.fail_on_owasp[0]"),
            ("fail_on_owasp = [\"A00\"]", "security.fail_on_owasp[0]"),
            ("fail_on_owasp = [\"A5\"]", "security.fail_on_owasp[0]"),
            ("fail_on_owasp = [\"A05:2017\"]", "security.fail_on_owasp[0]"),
            ("fail_on_owasp = [\"A05:\"]", "security.fail_on_owasp[0]"),
            ("fail_on_owasp = [\"A10\", \"B01\"]", "security.fail_on_owasp[1]"),
        ] {
            let rules: Rules = toml::from_str(&format!("[security]\n{bad}\n")).unwrap();
            let err = rules.validate().expect_err(bad).to_string();
            assert!(err.contains(key), "{bad}: {err}");
        }
        for ok in ["required_scanners = [\"s\"]", "fail_on_owasp = [\"A10\", \"A01:2025\", \"A09:2021\"]"] {
            let rules: Rules = toml::from_str(&format!("[security]\n{ok}\n")).unwrap();
            rules.validate().unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
    }
}
