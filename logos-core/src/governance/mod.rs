//! The governance engine ([governance-engine component], S-020,
//! [FR-GV-01]..[FR-GV-09], [FR-RC-01]..[FR-RC-04], [ADR-11], [ADR-14]).
//!
//! Evaluates the `rules.toml` architecture contract, runs the session/CI
//! gate, and serves `evolution` and `dsm` — every aggregate
//! run wrapped in **reconcile-then-score** ([ADR-11]): the working tree is
//! reconciled into the graph (O(changed), via
//! [`pipeline::reconcile`](crate::pipeline::reconcile)) *before* anything is
//! scored, so a quality signal can never silently reflect stale code
//! ([NFR-RA-02]). Every result carries the [FR-RC-03] freshness line:
//!
//! ```text
//! reconciled N files · HEAD <sha> · M unresolved refs
//! ```
//!
//! # Degradation posture ([ADR-14])
//!
//! A **per-file** reconcile failure (unreadable / non-UTF-8 source) degrades:
//! the signal is still emitted but the freshness line is stamped
//! `INCOMPLETE` with the failed files ([NFR-RA-11]). A **structural** failure
//! (store fault, invalid `rules.toml`, broken config) is the returned error —
//! fail loud; the surfaces map it to exit 2/3 or a structured MCP error.
//!
//! # Rules cache ([FR-GV-01])
//!
//! The parsed contract is cached twice, keyed by the blake3 hash of the
//! `rules.toml` content: in-process ([`GovernanceState`], so a long-lived
//! `serve --mcp` engine compiles the `globset` matchers exactly once per
//! content change) and in the `rules_cache` singleton table (so a fresh
//! process skips the TOML parse + validation when the contract is
//! unchanged).
//!
//! # Severity ([FR-GV-03])
//!
//! Every `rules.toml` violation is `severity = "error"` — checked-in policy is
//! binding (ratified 2026-06-06) — with one exception: an offender of a
//! `ratchet = true` budget held in the checked-in blessed set at or below its
//! blessed value is a `warning` ([FR-GV-23], [`ratchet`]).
//!
//! [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
//!
//! [governance-engine component]: ../../../docs/specs/architecture/components/governance-engine.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
//! [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
//! [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
//! [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
//! [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
//! [FR-RC-04]: ../../../docs/specs/requirements/FR-RC-04.md
//! [NFR-RA-02]: ../../../docs/specs/requirements/NFR-RA-02.md
//! [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use globset::GlobSet;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::config::{AdmissionAuthority, Rules};
use crate::engine::Engine;
use crate::graph_store::{
    AnnotationNodeRow, EdgeRow, FunctionConstraintRow, FunctionMetricRow, GraphStore,
    LatestMetricSnapshot, MetricSnapshotRow, NewCheckRun, NewViolation, NodeRow,
    StructuralReport, CHECK_RUN_OP_CHECK, CHECK_RUN_OP_SCAN,
};
use crate::hydrate::{build_view, Granularity};
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::quality::{
    BaselineReset, BaselineStamp, CheckRun, DocGap, DocGapsReport, DoctorReport, DriftedEdge, DsmReport, DsmRow,
    EvaluatedSetAbsence, EvolutionPoint, EvolutionReport, GateResult, HealthInfo, LatestHealth, MetricDelta,
    GraphDimensionNotApplicable, MetricRegression, MetricSnapshot, MetricValue, QualityReadout,
    RatchetReport, RulesReport, ScanResult, SessionInfo, SignalAbsence, TemporalTier, VerifyCensus,
    VerifyReport, Violation, WhereToStart, WorstOffenders,
};
use crate::metrics::ContainerLinks;
use crate::runtime::Runtime;

/// The report tier's session-start payload rendering ([CR-095]) — a projection of
/// [`quality_readout`], kept beside it rather than beside the hook materializer
/// that installs the script consuming it (see the module docs for why).
///
/// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
pub mod readout;

pub mod ratchet;

/// The thin-graph coverage helper ([FR-QM-18], S-523): one warning, shared by
/// every scoring surface, naming each large language the graph cannot see
/// across files.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
pub mod thin_graph;

/// The parse-damage warning ([FR-IX-14], S-571): `scan` names each large
/// language a tenth or more of whose files were extracted from a damaged parse.
///
/// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
pub mod parse_damage;

/// The uncapped, per-symbol offender listing ([FR-QM-19], S-676).
///
/// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
pub mod offenders;
pub use offenders::OffendersFilter;
/// The branch delta ([FR-GV-24], S-677): every offender and violation
/// classified per symbol against the merge-base.
///
/// [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
pub(crate) mod review;
/// The attribution of an aggregate change ([FR-GV-26], S-755): per-dimension
/// contributions in log space, each with its driving symbols.
///
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
mod attribution;
/// The `rules.toml [security]` gate ([S-668](../../../docs/planning/journal.md#s-668-a-security-rule-table-gates-on-new-findings-and-caps-and-a-missing-scan-exits-5)).
pub(crate) mod security;

/// Go packages as DSM rows ([S-527](../../../docs/planning/journal.md#s-527-the-go-dsm-lists-packages)).
mod go_dsm;

#[cfg(test)]
mod tests;

/// The read-only verdict's trend over a store ([S-785](../../../docs/planning/journal.md#s-785-the-health-page-shows-the-trend-warning-beside-the-gate-verdict)).
#[cfg(all(test, feature = "lang-rust"))]
mod read_only_trend_tests;

/// The gate's float-noise tolerance on the 0–10000 integer signal (BR-10,
/// DL-04): fail iff `current < baseline − EPSILON`.
const EPSILON: f64 = 1.0;

/// The gate verdict when the signal, the baseline's signal, or both are absent
/// — the one spelling, for the two gate paths that reach it
/// ([`models::quality::absence`] R1, R5).
///
/// **It names no cause**, and that is the correction [S-434]'s audit made
/// rather than a wording preference. The condition here is `aggregate_signal`
/// being `None` on either side, which is [`SignalAbsence`]'s gate and has
/// **two** causes ([CR-138], [FR-EH-04] AC2): an empty graph, and a populated
/// graph whose production scope is empty. This message read *"n/a (empty
/// graph)"* for both — the same claim, one figure over, that [CR-138] filed
/// against `quality-report`, reproduced here on a store whose only source is a
/// test module.
///
/// Classifying it properly is possible and was **not** done: [`SignalAbsence`]
/// needs the store's own node and file counts, which the gate does not read and
/// which this sentence has no room to carry (R2 requires an arm to carry what
/// establishes it, so a named cause here would have to bring its figures). The
/// readout already publishes the classified form. Removing an unestablished
/// attribution loses nothing true; adding a store read to the gate to keep a
/// word is not what an audit is for.
///
/// Held as one constant because two gate paths render it — the read-only
/// verdict [`gate_from_snapshot`] (reached from [`Engine::latest_gate`]) and
/// the persisting [`gate`]. They carried byte-identical literals, which is a
/// pair that can drift and the exact shape R5 exists to stop.
///
/// [`models::quality::absence`]: crate::models::quality::absence
/// [`SignalAbsence`]: crate::models::quality::SignalAbsence
/// [`Engine::latest_gate`]: crate::Engine::latest_gate
/// [S-434]: ../../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
/// [FR-EH-04]: ../../../docs/specs/requirements/FR-EH-04.md
/// [CR-138]: ../../../docs/requests/CR-138-a-readout-names-the-cause-its-gating-condition-establishes.md
const SIGNAL_OR_BASELINE_ABSENT: &str = "signal or baseline is n/a — informational pass";

/// Per-metric noise floor for regression *detail* reporting ([FR-GV-05]):
/// movements smaller than this are float residue, not regressions.
///
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
const METRIC_NOISE: f64 = 1e-9;

/// The single baseline scope of v1 — one Engine per worktree root (ADR-15),
/// so the project *is* the scope ([FR-GV-04] "for the scope").
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
const SCOPE_PROJECT: &str = "project";

/// The default `evolution` window ([FR-GV-06]).
///
/// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
const DEFAULT_EVOLUTION_LIMIT: u32 = 30;

/// The default `doc_gaps` listing cap ([FR-GV-14]).
///
/// [FR-GV-14]: ../../../docs/specs/requirements/FR-GV-14.md
const DEFAULT_DOC_GAPS_LIMIT: u32 = 50;

/// The mandatory `doc_gaps` honesty caveat ([FR-GV-14]) — always emitted:
/// presence of a resolved reference, not documentation quality or completeness.
const DOC_GAPS_CAVEAT: &str =
    "reference presence, not documentation quality: a symbol is 'documented' if any \
     doc section resolves a reference to it in the documentation graph; the prose's \
     depth or accuracy is not judged";

/// The severity every violation is emitted at (ratified 2026-06-06);
/// [`ratchet::classify`] alone lowers blessed ratchet debt to `warning`.
const SEVERITY_ERROR: &str = "error";

/// Canonical metric order (ADR-08) for regression/delta reporting.
const METRIC_NAMES: [&str; 5] = [
    "modularity",
    "acyclicity",
    "depth",
    "equality",
    "redundancy",
];

// ── Engine-held state ───────────────────────────────────────────────────────

/// The governance engine's in-process state, held by [`Engine`] for its
/// lifetime: the compiled-rules cache ([FR-GV-01] "globs compiled once") and
/// the last `scan` parameters for `rescan`.
///
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
#[derive(Debug)]
pub(crate) struct GovernanceState {
    /// The last compiled `rules.toml`, keyed by its content hash.
    rules: Mutex<Option<Arc<CompiledRules>>>,
    /// Whether the last `scan` reconciled — replayed by `rescan` ("the same
    /// parameters as the last scan").
    last_scan_reconcile: AtomicBool,
}

impl Default for GovernanceState {
    fn default() -> Self {
        Self {
            rules: Mutex::new(None),
            // A rescan before any scan behaves like a default scan.
            last_scan_reconcile: AtomicBool::new(true),
        }
    }
}

impl GovernanceState {
    /// Record the parameters of a `scan` for `rescan` to replay.
    pub(crate) fn record_scan(&self, reconcile: bool) {
        self.last_scan_reconcile.store(reconcile, Ordering::Release);
    }

    /// The parameters of the last `scan` (defaults: reconcile).
    pub(crate) fn last_scan_reconcile(&self) -> bool {
        self.last_scan_reconcile.load(Ordering::Acquire)
    }
}

// ── Compiled rules (FR-GV-01) ───────────────────────────────────────────────

/// The [`CompiledRules::hash`] sentinel standing in for "no rules file exists"
/// — a missing default `<root>/.logos/rules.toml` compiles to the empty
/// contract under this hash, which `check_rules` reads as `rules_present =
/// false` (the honest "no contract authored yet" signal, NFR-CC-04).
pub(crate) const ABSENT_RULES_HASH: &str = "absent";

/// The `rules.toml` contract with its layer globs compiled to `globset`
/// matchers, cached by content hash ([FR-GV-01]).
///
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
#[derive(Debug)]
pub(crate) struct CompiledRules {
    /// The parsed contract.
    pub rules: Rules,
    /// blake3 hex hash of the file content (the cache key); the sentinel
    /// [`ABSENT_RULES_HASH`] when no rules file exists.
    pub hash: String,
    /// One compiled matcher per `[[layers]]` declaration, in declaration
    /// order (first-glob-wins, BR-15 / DL-05 tiebreak).
    layers: Vec<CompiledLayer>,
    /// One compiled matcher pair per `[[forbidden_imports]]` declaration, in
    /// declaration order ([FR-GV-12]: globs compiled once via `globset`).
    ///
    /// [FR-GV-12]: ../../../docs/specs/requirements/FR-GV-12.md
    forbidden_imports: Vec<CompiledForbiddenImport>,
    /// One compiled `paths` matcher per `[[require_tested]]` declaration, in
    /// declaration order ([FR-GV-13]: globs compiled once via `globset`).
    ///
    /// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
    require_tested: Vec<CompiledRequireTested>,
    /// One compiled `paths` matcher per `[[require_documented]]` declaration, in
    /// declaration order ([FR-GV-15]: globs compiled once via `globset`).
    ///
    /// [FR-GV-15]: ../../../docs/specs/requirements/FR-GV-15.md
    require_documented: Vec<CompiledRequireDocumented>,
}

/// One `[[layers]]` band with its compiled glob matcher.
#[derive(Debug)]
struct CompiledLayer {
    name: String,
    order: u32,
    matcher: GlobSet,
}

/// One `[[forbidden_imports]]` ban with its `from`/`to` globs compiled once
/// ([FR-GV-12]). The original glob strings are retained for the violation's
/// `rule` key (`forbidden_import:<from>-><to>`).
///
/// [FR-GV-12]: ../../../docs/specs/requirements/FR-GV-12.md
#[derive(Debug)]
struct CompiledForbiddenImport {
    from_glob: String,
    to_glob: String,
    from: GlobSet,
    to: GlobSet,
    reason: Option<String>,
}

/// One `[[require_tested]]` coverage contract with its `paths` globs compiled
/// once into a single matcher ([FR-GV-13]). The original glob strings are
/// retained (comma-joined) for the violation's `rule` key
/// (`require_tested:<paths>`).
///
/// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
#[derive(Debug)]
struct CompiledRequireTested {
    paths_glob: String,
    paths: GlobSet,
    reason: Option<String>,
}

/// One `[[require_documented]]` documentation contract with its `paths` globs
/// compiled once into a single matcher ([FR-GV-15]). The original glob strings
/// are retained (comma-joined) for the violation's `rule` key
/// (`require_documented:<paths>`).
///
/// [FR-GV-15]: ../../../docs/specs/requirements/FR-GV-15.md
#[derive(Debug)]
struct CompiledRequireDocumented {
    paths_glob: String,
    paths: GlobSet,
    reason: Option<String>,
}

impl CompiledRules {
    /// Compile a parsed contract's layer globs ([FR-GV-01] "compiled once").
    ///
    /// # Errors
    /// Returns an error if a layer glob fails compilation — `rules.toml` is
    /// validated at load, so this is defence in depth.
    ///
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    fn compile(rules: Rules, hash: String) -> Result<Self> {
        let layers = rules
            .layers
            .iter()
            .map(|layer| {
                Ok(CompiledLayer {
                    name: layer.name.clone(),
                    order: layer.order,
                    matcher: crate::config::compile_globs(&layer.paths)
                        .with_context(|| format!("compiling layer '{}' globs", layer.name))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // FR-GV-12: compile the `[[forbidden_imports]]` globs once, here, in
        // declaration order. `rules.toml` is validated at load, so a failure
        // is defence in depth (a single-glob set per side, like a layer band).
        let forbidden_imports = rules
            .forbidden_imports
            .iter()
            .map(|fi| {
                Ok(CompiledForbiddenImport {
                    from_glob: fi.from.clone(),
                    to_glob: fi.to.clone(),
                    from: crate::config::compile_globs(std::slice::from_ref(&fi.from))
                        .with_context(|| {
                            format!("compiling forbidden_imports from '{}'", fi.from)
                        })?,
                    to: crate::config::compile_globs(std::slice::from_ref(&fi.to))
                        .with_context(|| format!("compiling forbidden_imports to '{}'", fi.to))?,
                    reason: fi.reason.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // FR-GV-13: compile the `[[require_tested]]` `paths` globs once, here,
        // in declaration order — a single matcher per contract (any path glob
        // matching assigns the file to the contract). `rules.toml` is validated
        // at load, so a failure is defence in depth.
        let require_tested = rules
            .require_tested
            .iter()
            .map(|rt| {
                Ok(CompiledRequireTested {
                    paths_glob: rt.paths.join(","),
                    paths: crate::config::compile_globs(&rt.paths).with_context(|| {
                        format!("compiling require_tested paths {:?}", rt.paths)
                    })?,
                    reason: rt.reason.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // FR-GV-15: compile the `[[require_documented]]` `paths` globs once, here,
        // in declaration order — a single matcher per contract, exactly like
        // `[[require_tested]]`. `rules.toml` is validated at load, so a failure is
        // defence in depth.
        let require_documented = rules
            .require_documented
            .iter()
            .map(|rd| {
                Ok(CompiledRequireDocumented {
                    paths_glob: rd.paths.join(","),
                    paths: crate::config::compile_globs(&rd.paths).with_context(|| {
                        format!("compiling require_documented paths {:?}", rd.paths)
                    })?,
                    reason: rd.reason.clone(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            rules,
            hash,
            layers,
            forbidden_imports,
            require_tested,
            require_documented,
        })
    }

    /// The `[[layers]]` band of `path`: first matching layer in declaration
    /// order wins (BR-15, DL-05); `None` = unassigned (exempt, [FR-GV-02]).
    ///
    /// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
    fn layer_of(&self, path: &str) -> Option<(&str, u32)> {
        self.layers
            .iter()
            .find(|l| l.matcher.is_match(path))
            .map(|l| (l.name.as_str(), l.order))
    }
}

/// Load the rules contract for `engine`, through both cache tiers
/// ([FR-GV-01]): the in-process compiled cache (hash hit → no work at all),
/// then the `rules_cache` table (hash hit → JSON parse, no TOML + no
/// re-validation), then a full load + validate + cache write.
///
/// `explicit` overrides the default `<root>/.logos/rules.toml` (the CLI
/// `check --rules FILE`). A missing default file is the empty contract
/// ([`Rules::default`]); a missing explicit file is the caller's error.
///
/// # Errors
/// Returns a [`ConfigError`](crate::config::ConfigError) (usage, exit 2) for
/// an unreadable/invalid contract, or a store error from the cache tier.
///
/// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
fn load_rules_cached(engine: &Engine, explicit: Option<&Path>) -> Result<Arc<CompiledRules>> {
    let default_path = engine.root().join(".logos").join("rules.toml");
    let path = explicit.unwrap_or(&default_path);

    // Hash the content first: the cache key for both tiers.
    let (content, hash) = match std::fs::read_to_string(path) {
        Ok(text) => {
            let hash = blake3::hash(text.as_bytes()).to_hex().to_string();
            (Some(text), hash)
        }
        Err(_) if explicit.is_none() => (None, ABSENT_RULES_HASH.to_string()),
        Err(e) => {
            // An explicit --rules path that cannot be read is a usage fault.
            return Err(crate::config::ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            }
            .into());
        }
    };

    // Tier 1: the in-process compiled cache.
    {
        let cached = engine.governance().rules.lock().expect("rules cache lock");
        if let Some(compiled) = cached.as_ref() {
            if compiled.hash == hash {
                return Ok(Arc::clone(compiled));
            }
        }
    }

    let rules = match content {
        None => Rules::default(),
        Some(ref text) => {
            // Tier 2: the persisted parse cache (FR-GV-01 "cached by hash").
            let persisted = engine
                .runtime()
                .map(|rt| rt.submit_read(|store| store.rules_cache()))
                .transpose()?
                .flatten();
            match persisted {
                Some(row) if row.rules_hash == hash => serde_json::from_str(&row.parsed_json)
                    .context("deserialising the cached rules.toml parse")?,
                _ => {
                    // Full parse + validation (clear message, exit 2 on a bad
                    // contract — FR-CF-03), then refresh the persisted cache.
                    // Parsing the SAME string the hash was computed from —
                    // never a second disk read — so a concurrent rules.toml
                    // edit can never persist an incoherent (hash, parse)
                    // cache pair (FR-GV-01 coherence).
                    let rules = crate::config::parse_rules(text, path)?;
                    if let Some(rt) = engine.runtime() {
                        let json = serde_json::to_string(&rules)
                            .context("serialising the rules.toml parse for the cache")?;
                        let hash_owned = hash.clone();
                        rt.submit_write(move |w| {
                            w.set_rules_cache(&hash_owned, &json, unix_now())
                        })?;
                    }
                    rules
                }
            }
        }
    };

    let compiled = Arc::new(CompiledRules::compile(rules, hash)?);
    *engine.governance().rules.lock().expect("rules cache lock") = Some(Arc::clone(&compiled));
    Ok(compiled)
}

// ── Freshness (FR-RC-01..04, ADR-11) ────────────────────────────────────────

/// The outcome of the pre-evaluation reconcile, carrying everything the
/// [FR-RC-03] freshness line needs.
///
/// [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
#[derive(Debug, Default)]
pub(crate) struct Freshness {
    /// Files actually (re-)entered into or removed from the graph.
    reconciled: u64,
    /// Per-file reconcile failures — non-empty stamps `INCOMPLETE`
    /// ([NFR-RA-11]).
    ///
    /// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
    failed: Vec<String>,
    /// `git rev-parse HEAD`, when inside a git repo.
    head: Option<String>,
    /// Reference-ledger rows currently unbound.
    unresolved: u64,
    /// `true` under `--no-reconcile` ([FR-RC-04]).
    ///
    /// [FR-RC-04]: ../../../docs/specs/requirements/FR-RC-04.md
    assumed: bool,
    /// Degradations folded from the reconcile — surfaced on the read-model.
    warnings: Vec<String>,
    /// Advisory notes folded from the reconcile (the minified-JS exclusion,
    /// HF-1) — surfaced on the read-model's own `notes`, never its `warnings`.
    notes: Vec<String>,
}

impl Freshness {
    /// Render the [FR-RC-03] freshness line.
    ///
    /// [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
    fn line(&self) -> String {
        let head = self.head.as_deref().unwrap_or("no-git");
        let body = if self.assumed {
            format!(
                "assumed-fresh (--no-reconcile) · HEAD {head} · {} unresolved refs",
                self.unresolved
            )
        } else {
            format!(
                "reconciled {} files · HEAD {head} · {} unresolved refs",
                self.reconciled, self.unresolved
            )
        };
        if self.failed.is_empty() {
            return body;
        }
        // The NFR-RA-11 degradation: prominent, names the unsynced files.
        let mut shown = self.failed.iter().take(5).cloned().collect::<Vec<_>>();
        if self.failed.len() > shown.len() {
            shown.push(format!("… {} more", self.failed.len() - shown.len()));
        }
        format!(
            "INCOMPLETE — {body} · {} files failed to sync: {}",
            self.failed.len(),
            shown.join(", ")
        )
    }
}

/// Run the [ADR-11] reconcile prologue of an aggregate run.
///
/// With `reconcile`, the working tree is reconciled into the graph
/// (O(changed), [FR-RC-02]); without, only the cheap unresolved-count read
/// runs and the result is marked assumed-fresh ([FR-RC-04]). The `HEAD` sha
/// is captured either way.
///
/// # Errors
/// Returns an error for a transient engine (no runtime) or on a structural
/// reconcile failure ([ADR-14] fail-loud).
///
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
/// [FR-RC-02]: ../../../docs/specs/requirements/FR-RC-02.md
/// [FR-RC-04]: ../../../docs/specs/requirements/FR-RC-04.md
fn reconcile_step(engine: &Engine, reconcile: bool) -> Result<Freshness> {
    let head = git_head(engine.root());
    if !reconcile {
        let counts = quality_runtime(engine)?.submit_read(|store| store.counts())?;
        return Ok(Freshness {
            head,
            unresolved: counts.refs_total.saturating_sub(counts.refs_resolved),
            assumed: true,
            ..Freshness::default()
        });
    }

    let outcome = engine.run_reconcile()?;
    Ok(Freshness {
        reconciled: outcome.reconciled_files,
        failed: outcome.files_failed,
        head,
        unresolved: outcome.resolution.refs_unresolved,
        assumed: false,
        warnings: outcome.warnings,
        notes: outcome.notes,
    })
}

/// `git rev-parse HEAD` at `root`, or `None` outside a repo / without git.
///
/// A subprocess, never a library dependency (the [Git integration] is
/// optional by design — "no-git" is a first-class freshness state).
///
/// [Git integration]: ../../../docs/specs/architecture/integrations/git.md
fn git_head(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// The runtime every quality evaluation needs; a transient engine cannot
/// score ([ADR-14] fail-loud, structural).
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
fn quality_runtime(engine: &Engine) -> Result<&Runtime> {
    engine.runtime().ok_or_else(|| {
        anyhow!(
            "quality evaluation requires a long-lived engine (Engine::start); \
             a transient Engine::open engine has no graph runtime"
        )
    })
}

/// Unix-seconds now (bookkeeping timestamps, never part of a signal).
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── Rules evaluation (FR-GV-02) ─────────────────────────────────────────────

/// Everything [`evaluate`] reads — a pure-input bundle so the evaluator is
/// unit-testable with no I/O (the same posture as `metrics::compute`).
pub(crate) struct EvalInput<'a> {
    pub compiled: &'a CompiledRules,
    pub nodes: &'a [NodeRow],
    pub edges: &'a [EdgeRow],
    /// The container links ([`ContainerLinks`], S-557, S-510): the
    /// `no_god_containers` budget counts a partial type once and an `impl` or
    /// receiver method toward its struct, as Focus does.
    pub links: &'a ContainerLinks,
    pub functions: &'a [FunctionConstraintRow],
    /// Function/method annotation rows ([FR-AN-04]) carrying `is_dead` /
    /// `is_duplicate` — the SAME rows the Redundancy metric reads ([FR-QM-05]),
    /// so the `max_dead`/`max_duplicates` budgets ([FR-GV-11]) and the metric
    /// can never disagree about what is dead or duplicated.
    ///
    /// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
    /// [FR-QM-05]: ../../../docs/specs/requirements/FR-QM-05.md
    /// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
    pub function_metrics: &'a [FunctionMetricRow],
    /// Annotation rows ([FR-AN-05]) carrying the `exported` flag the
    /// [`[[require_tested]]`](crate::config::RequireTested) contract ([FR-GV-13])
    /// needs — [`NodeRow`] omits visibility, so the coverage gate reads these
    /// id-ordered rows instead ([NFR-RA-06]). Empty unless a contract is set.
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
    pub annotations: &'a [AnnotationNodeRow],
    /// The `is_test = 1` node ids ([FR-AN-05]) seeding the [FR-GV-13]
    /// reachability BFS — the SAME `test_node_ids` seam `test_reachable_set`
    /// uses, so the coverage contract and the reachability core can never
    /// disagree about what a test reaches (CR-001 CRA-01).
    ///
    /// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
    /// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
    pub test_node_ids: &'a [NodeId],
    /// The cycle count from the shared SCC set ([FR-QM-02] reuse: gate,
    /// metric, and rule can never disagree on the cycle set).
    ///
    /// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
    pub cycles: u64,
    /// The effective CR-005 detection thresholds the four new structural budgets
    /// read ([FR-GV-11] ext., [UAT-GV-08]): the documented defaults composed with
    /// the `rules.toml` `[metric_thresholds]` overrides ([BR-25]). The SAME set
    /// the metrics snapshot scored under, so a budget and the dimension it
    /// enforces agree by construction.
    ///
    /// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
    /// [UAT-GV-08]: ../../../docs/specs/requirements/UAT-GV-08.md
    /// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
    pub thresholds: crate::metrics::Thresholds,
}

/// The effective CR-005 detection-threshold set ([BR-25], [FR-QM-14]): the
/// documented [`Thresholds::default`](crate::metrics::Thresholds) values with the
/// `rules.toml` `[metric_thresholds]` overrides applied. An omitted key keeps its
/// documented default, so a partial table is honoured key-by-key ([UAT-QM-13]
/// "omitted threshold keys fall back to the documented defaults").
///
/// This is the single place config policy becomes the metrics seam: the result
/// is passed to [`metrics::snapshot`](crate::metrics::snapshot) (its hash gates
/// the baseline) and into [`EvalInput::thresholds`] (the four new budgets), so
/// the snapshot, the gate, and the budgets all score under one threshold set.
///
/// [FR-QM-14]: ../../../docs/specs/requirements/FR-QM-14.md
/// [BR-25]: ../../../docs/specs/software-spec.md#311-quality-metrics
/// [UAT-QM-13]: ../../../docs/specs/requirements/UAT-QM-13.md
fn effective_thresholds(rules: &Rules) -> crate::metrics::Thresholds {
    // The raw→effective composition lives on the config read-model itself
    // (`MetricThresholds::effective`), so the annotation engine reads the SAME
    // near-clone parameters this gate scores under ([CR-013]) — one seam, no
    // drift between the clustering pass and the hashed effective set.
    //
    // [CR-013]: ../../../docs/requests/CR-013-tunable-near-clone-thresholds.md
    rules.metric_thresholds.effective()
}

/// The file extensions whose plugin declares the `member_access` capability
/// ([CR-171]) — the set every metrics pass gates Cohesion on, so `scan`, `gate`,
/// `session_start` and the readout score Cohesion over one population.
///
/// The engine's cached registry when it holds one; otherwise a fresh load, as
/// [`Engine::languages`] falls back. A registry that will not load declares
/// nothing, so Cohesion drops out rather than being scored from calls alone
/// ([NFR-CC-04]) — and the failure is logged, never swallowed.
///
/// [CR-171]: ../../../docs/requests/CR-171-the-quality-signal-and-test-classification-stay-honest-on-a-thin-graph.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
fn member_access_extensions(engine: &Engine) -> HashSet<String> {
    if let Some(registry) = engine.registry() {
        return registry.member_access_extensions();
    }
    match crate::plugin::LanguageRegistry::load(engine.root()) {
        Ok(registry) => registry.member_access_extensions(),
        Err(err) => {
            tracing::warn!("could not load plugin registry; Cohesion scores no class: {err}");
            HashSet::new()
        }
    }
}

/// The [FR-QM-18] coverage warnings for the store behind `runtime` — the one
/// call `scan`, `gate`, `session_start` and the readout make, so they carry
/// the same warnings ([`thin_graph::ThinGraph::warnings`]).
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
fn coverage_warnings(runtime: &Runtime) -> Result<Vec<String>> {
    Ok(runtime
        .submit_read(|store| thin_graph::ThinGraph::read(store))?
        .warnings())
}

/// `scan`'s [`coverage_warnings`] and its [FR-IX-14] parse-damage warnings
/// ([`parse_damage::warnings`]), from one read of the `status` rows and the
/// production callables: the thin graph is the one `ThinGraph::read` builds,
/// and the parse-damage warning asks it which languages are large enough to
/// name.
///
/// [FR-IX-14]: ../../../docs/specs/requirements/FR-IX-14.md
fn scan_coverage_warnings(runtime: &Runtime) -> Result<Vec<String>> {
    runtime.submit_read(|store| {
        let (rows, _) = crate::resolve::coverage_and_shared_headers(store)?;
        let thin = thin_graph::ThinGraph::from_rows(&rows, &store.production_callables_by_language()?);
        let mut warnings = thin.warnings();
        warnings.extend(parse_damage::warnings(&rows, &thin));
        Ok(warnings)
    })
}

/// Evaluate the architecture contract ([FR-GV-02]): constraints (point
/// queries — including the [FR-GV-11] coupling/redundancy budgets), layer
/// ordering (BR-11; unassigned files exempt, BR-15), boundaries, the
/// [FR-GV-12] `[[forbidden_imports]]` glob-level import linter, the
/// [FR-GV-13] `[[require_tested]]` coverage contract, and the [FR-GV-15]
/// `[[require_documented]]` documentation contract. Pure and deterministic —
/// inputs arrive in store id order and every grouping iterates a `BTree*` or
/// the id-ordered node/edge/annotation slices ([NFR-RA-06]).
///
/// Returns the violations (constraints, then layer ordering, then boundaries,
/// then forbidden imports, then require-tested, then require-documented) and the
/// number of active rules checked. The [FR-GV-11]/[FR-GV-12]/[FR-GV-13]/[FR-GV-15]
/// families are evaluated here, inside `check_rules`, **orthogonally to the
/// metrics gate** ([CR-002], [CR-003]): adding or removing them never perturbs
/// the geometric-mean signal.
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [FR-GV-12]: ../../../docs/specs/requirements/FR-GV-12.md
/// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
/// [FR-GV-15]: ../../../docs/specs/requirements/FR-GV-15.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [CR-002]: ../../../docs/requests/CR-002-extended-architecture-contracts.md
/// [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
pub(crate) fn evaluate(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    // Each section is a pure accumulator (CR-023): it reads `input` and returns
    // its own `(violations, checked-count)`. The orchestrator concatenates them
    // in the fixed canonical order — constraints, coupling, redundancy, CR-005
    // structural budgets, layer ordering + boundaries, forbidden imports,
    // require-tested, require-documented — so the emitted violation list and the
    // `checked` total are byte-identical to the pre-decomposition monolith for
    // every input ([NFR-RA-06]).
    let mut violations = Vec::new();
    let mut checked = 0u32;
    for (section_violations, section_checked) in [
        check_constraints(input),
        check_coupling(input),
        check_redundancy(input),
        check_structural(input),
        check_layer_ordering(input),
        check_forbidden_imports(input),
        check_require_tested(input),
        check_require_documented(input),
    ] {
        violations.extend(section_violations);
        checked += section_checked;
    }
    (violations, checked)
}

/// The ratchet identity of node `id` ([FR-GV-23]): [`NodeRow::identity`], its
/// symbol id **without the repository coordinate** — the descriptor tail
/// ([ADR-85]), stable across re-indexing where the storage id is not. The
/// blessed set is checked in, so it must match in every checkout of the same
/// tree: the coordinate comes from `[project] id`, else the `origin` remote,
/// else the directory name, and a fork, a renamed repository or a remote-less
/// clone would otherwise read every blessed entry as new. A node the id-ordered
/// slice does not carry falls back to its storage id, spelled so it can never
/// collide with a symbol.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
/// [ADR-85]: ../../../docs/specs/architecture/decisions/ADR-85.md
fn symbol_of(node_of: &HashMap<NodeId, &NodeRow>, id: NodeId) -> String {
    node_of.get(&id).map_or_else(
        || format!("node-id:{}", id.get()),
        |n| n.identity().to_string(),
    )
}

/// Constraint point queries (the original "Constraints" section): `max_cycles`,
/// `max_cc`, `max_fn_lines`, `no_god_files`. Each active key counts once toward
/// `checked`; inputs arrive in id order so the per-file `no_god_files` walk is
/// deterministic ([NFR-RA-06]). A budget under ratchet ([FR-GV-23]) marks each
/// offender with its identity and value for [`ratchet::classify`].
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
fn check_constraints(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let constraints = &input.compiled.rules.constraints;
    let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();

    if let Some(budget) = &constraints.max_cycles {
        checked += 1;
        // Both forms through the one count-budget seam (FR-GV-23 generalises
        // ADR-39's delta form): the scalar message is the pre-ratchet one.
        if let Some(message) =
            budget.exceeded_message(input.cycles, "max_cycles", "dependency cycles")
        {
            violations.push(Violation {
                rule: "max_cycles".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message,
                ratchet: None,
            });
        }
    }
    if let Some(budget) = constraints.max_cc {
        checked += 1;
        let max = budget.max();
        for f in input.functions {
            if let Some(cc) = f.cyclomatic_complexity.filter(|cc| *cc > i64::from(max)) {
                violations.push(Violation {
                    rule: "max_cc".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: f.file_path.clone().unwrap_or_default(),
                    node_id: Some(f.id.get()),
                    message: format!(
                        "fn `{}` has cyclomatic complexity {cc} > max_cc = {max}",
                        f.name
                    ),
                    ratchet: ratchet::mark(
                        budget.is_ratchet(),
                        symbol_of(&node_of, f.id),
                        cc.unsigned_abs(),
                    ),
                });
            }
        }
    }
    if let Some(budget) = constraints.max_fn_lines {
        checked += 1;
        let max = budget.max();
        for f in input.functions {
            if let Some(lines) = f.line_count.filter(|lines| *lines > i64::from(max)) {
                violations.push(Violation {
                    rule: "max_fn_lines".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: f.file_path.clone().unwrap_or_default(),
                    node_id: Some(f.id.get()),
                    message: format!(
                        "fn `{}` spans {lines} lines > max_fn_lines = {max}",
                        f.name
                    ),
                    ratchet: ratchet::mark(
                        budget.is_ratchet(),
                        symbol_of(&node_of, f.id),
                        lines.unsigned_abs(),
                    ),
                });
            }
        }
    }
    if let Some(budget) = constraints.no_god_files {
        checked += 1;
        let max = budget.max();
        // Symbols per file, over extracted (non-policy, non-documentation) nodes only.
        // Documentation nodes are excluded here for the same reason they are excluded at
        // graph hydration ([FR-DG-06], [ADR-19]): a `.md` file with N sections would
        // otherwise count N+1 toward the per-file symbol budget, making the check
        // non-neutral w.r.t. documentation.
        let mut per_file: BTreeMap<&str, u64> = BTreeMap::new();
        for n in input.nodes {
            if matches!(n.kind, NodeKind::Layer | NodeKind::Boundary) || n.kind.is_documentation() {
                continue;
            }
            if let Some(path) = n.file_path.as_deref() {
                *per_file.entry(path).or_default() += 1;
            }
        }
        for (path, count) in per_file {
            if count > u64::from(max) {
                violations.push(Violation {
                    rule: "no_god_files".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: path.to_string(),
                    node_id: None,
                    message: format!(
                        "`{path}` declares {count} symbols > no_god_files = {max} (OQ-06)"
                    ),
                    ratchet: ratchet::mark(budget.is_ratchet(), path, count),
                });
            }
        }
    }

    (violations, checked)
}

/// Coupling budgets (module-grain fan-in / fan-out, [FR-GV-11] / [BR-19],
/// [CR-065]).
///
/// Counted at **module grain** — the classic Constantine/Yourdon,
/// Henry-Kafura coupling level — as the number of **distinct neighbouring
/// modules**, not raw edge multiplicity: a shared helper called from many
/// symbols in one module is one neighbour, not N. Production-scoped
/// ([FR-QM-08]): `is_test` nodes (and the policy vertices `Layer`/`Boundary`,
/// which are annotation-materialised, not lexical code) are dropped from the
/// node set **before** the rollup, so a test-only module and any
/// test↔production edge never contribute. Reuses the SAME module rollup the
/// `dsm` view starts from ([`build_view`] at [`Granularity::Module`]; the DSM then
/// folds Go file modules into package rows, which these budgets do not) — it already
/// applies the canonical-view edge set `BR-19` requires (`Contains`,
/// `Accesses`, documentation, and config-reference edges excluded) and drops
/// module self-loops, and it deduplicates rollup edges to one per `(src,
/// dst)` pair, so a rollup vertex's in/out-degree already IS its
/// distinct-neighbour count. The derived `ForbiddenDependency` edge is ALSO
/// excluded, same as the pre-CR-065 implementation: it is last run's
/// governance *output* (BR-12), re-materialised before this evaluator runs, so
/// counting it would make fan-in/out drift with the policy graph and break the
/// byte-identical guarantee ([NFR-RA-06]) — `build_view` has no reason to know
/// about this governance-specific self-reference fence, so it is applied here.
/// A dispatch edge is dropped beside it: a call through a trait couples the
/// caller to the trait, not to its implementations ([FR-QM-20]).
/// Computed from the hydrated edge set, never persisted ([CR-002]: no
/// migration).
///
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
/// [FR-QM-20]: ../../../docs/specs/requirements/FR-QM-20.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [CR-002]: ../../../docs/requests/CR-002-extended-architecture-contracts.md
/// [CR-065]: ../../../docs/requests/CR-065-module-grain-coupling-metric.md
fn check_coupling(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let constraints = &input.compiled.rules.constraints;
    if constraints.max_fan_in.is_none() && constraints.max_fan_out.is_none() {
        return (violations, checked);
    }

    // Production scope (FR-QM-08): exclude is_test nodes and the derived
    // Layer/Boundary policy vertices BEFORE the rollup. Any edge incident to an
    // excluded node is dropped too — its endpoint is absent from the rollup's
    // node index, so `build_view` skips it (the same missing-endpoint fence the
    // rollup already applies to non-code nodes).
    let test_ids: HashSet<NodeId> = input.test_node_ids.iter().copied().collect();
    let nodes: Vec<NodeRow> = input
        .nodes
        .iter()
        .filter(|n| {
            !test_ids.contains(&n.id) && !matches!(n.kind, NodeKind::Layer | NodeKind::Boundary)
        })
        .cloned()
        .collect();
    let edges: Vec<EdgeRow> = input
        .edges
        .iter()
        .copied()
        .filter(|e| e.kind != EdgeKind::ForbiddenDependency && !e.is_dispatch())
        .collect();
    let view = build_view(Granularity::Module, &nodes, &edges);

    // The rollup vertex itself carries no file (it is an aggregate — `kind:
    // None`, `node_id: None`; see `Vertex` in `crate::hydrate`), so a genuine
    // module key (`module:<symbol>`, the common case: every source file gets
    // a synthetic per-file `NodeKind::Module` node, S-011) needs its file
    // resolved from the underlying Module row's own `file_path`. The
    // `file:<path>` FALLBACK key (no `Contains`-reachable Module ancestor)
    // already carries its path in the key itself, via `dsm_key_path`.
    let module_file: HashMap<String, String> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Module)
        .filter_map(|n| {
            n.file_path
                .clone()
                .map(|f| (format!("module:{}", n.symbol.as_str()), f))
        })
        .collect();

    if let Some(budget) = constraints.max_fan_in {
        checked += 1;
        violations.extend(module_coupling_violations(
            &view,
            budget,
            "max_fan_in",
            "fan-in",
            Direction::Incoming,
            &module_file,
        ));
    }
    if let Some(budget) = constraints.max_fan_out {
        checked += 1;
        violations.extend(module_coupling_violations(
            &view,
            budget,
            "max_fan_out",
            "fan-out",
            Direction::Outgoing,
            &module_file,
        ));
    }

    (violations, checked)
}

/// One coupling budget's module-grain violations, ordered by stable module
/// key ([NFR-RA-06]) rather than rollup-vertex insertion order (which follows
/// first-seen node id and is therefore not itself the ordering contract). Under
/// ratchet ([FR-GV-23]) the module key is each offender's identity.
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
fn module_coupling_violations(
    view: &crate::hydrate::GraphView,
    budget: crate::config::Budget,
    rule: &str,
    metric_label: &str,
    direction: Direction,
    module_file: &HashMap<String, String>,
) -> Vec<Violation> {
    let graph = view.graph();
    let max = budget.max();
    let mut hits: Vec<(&str, &str, usize)> = graph
        .node_indices()
        .filter_map(|idx| {
            let count = graph.edges_directed(idx, direction).count();
            (count as u64 > u64::from(max)).then(|| {
                let vertex = &graph[idx];
                (vertex.key.as_str(), vertex.label.as_str(), count)
            })
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(b.0));

    hits.into_iter()
        .map(|(key, label, count)| Violation {
            rule: rule.to_string(),
            rule_type: "constraint".to_string(),
            severity: SEVERITY_ERROR.to_string(),
            file: dsm_key_path(key, DsmGranularity::Module)
                .map(str::to_string)
                .or_else(|| module_file.get(key).cloned())
                .unwrap_or_default(),
            node_id: None,
            message: format!("`{label}` has {metric_label} {count} > {rule} = {max}"),
            ratchet: ratchet::mark(budget.is_ratchet(), key, count as u64),
        })
        .collect()
}

/// One violation per member of a symbol set under ratchet ([FR-GV-23]):
/// `max_dead`, `max_duplicates` and `max_brain_methods` name each dead,
/// duplicated or brain-method production function, in node-id order, marked
/// with value `1` — membership is the whole measurement, so a blessed member
/// can only stay blessed or be fixed, never worsen.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
fn symbol_set_violations<'a>(
    rule: &str,
    what: &str,
    members: impl Iterator<Item = &'a FunctionMetricRow>,
    node_of: &HashMap<NodeId, &NodeRow>,
) -> Vec<Violation> {
    members
        .map(|f| {
            let (name, file) = node_of
                .get(&f.id)
                .map(|n| (n.name.as_str(), n.file_path.clone().unwrap_or_default()))
                .unwrap_or(("?", String::new()));
            Violation {
                rule: rule.to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file,
                node_id: Some(f.id.get()),
                message: format!("fn `{name}` is {what} — {rule} = 0 under ratchet"),
                ratchet: ratchet::mark(true, symbol_of(node_of, f.id), 1),
            }
        })
        .collect()
}

/// Redundancy budgets (production-scope is_dead / is_duplicate, FR-GV-11).
///
/// Counted over the SAME annotation rows the Redundancy metric reads
/// ([FR-QM-05]) — Function/Method nodes, `is_test` and derived excluded
/// ([FR-QM-08]) — so the budget and the metric agree by construction. One
/// project-wide violation per budget, except under ratchet ([FR-GV-23]), where
/// each dead or duplicated function is one offender of a symbol set.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
fn check_redundancy(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let constraints = &input.compiled.rules.constraints;
    let test_ids: HashSet<NodeId> = input.test_node_ids.iter().copied().collect();
    let production_fns = || {
        input
            .function_metrics
            .iter()
            .filter(|f| !test_ids.contains(&f.id))
    };
    let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();

    // NULL (`is_dead = None`) is excluded for free — only `Some(true)` counts
    // ([CR-043], [ADR-39]): a language without the reachability capability
    // renders NULL and never inflates the budget, nor joins the ratchet set.
    let dead = |f: &&FunctionMetricRow| f.is_dead == Some(true);
    match &constraints.max_dead {
        // FR-GV-23: under ratchet the dead set is named member by member.
        Some(budget) if budget.is_ratchet() => {
            checked += 1;
            violations.extend(symbol_set_violations(
                "max_dead",
                "dead code",
                production_fns().filter(dead),
                &node_of,
            ));
        }
        Some(budget) => {
            checked += 1;
            let dead_count = production_fns().filter(dead).count() as u64;
            // Both modes route through the one budget seam ([FR-GV-11], [ADR-39]):
            // absolute fails iff `dead > max`; delta fails iff `dead` rises above
            // the blessed `baseline (+delta)`. The message is mode-specific but
            // deterministic, so a re-run is byte-identical ([NFR-RA-06]).
            if let Some(message) = budget.exceeded_message(dead_count) {
                violations.push(Violation {
                    rule: "max_dead".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: String::new(),
                    node_id: None,
                    message,
                    ratchet: None,
                });
            }
        }
        None => {}
    }
    if let Some(budget) = constraints.max_duplicates {
        checked += 1;
        let duplicate = |f: &&FunctionMetricRow| f.is_duplicate == Some(true);
        if budget.is_ratchet() {
            violations.extend(symbol_set_violations(
                "max_duplicates",
                "an exact duplicate",
                production_fns().filter(duplicate),
                &node_of,
            ));
        } else {
            let max = budget.max();
            let duplicates = production_fns().filter(duplicate).count() as u64;
            if duplicates > u64::from(max) {
                violations.push(Violation {
                    rule: "max_duplicates".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: String::new(),
                    node_id: None,
                    message: format!(
                        "{duplicates} duplicate functions exceed max_duplicates = {max}"
                    ),
                    ratchet: None,
                });
            }
        }
    }

    (violations, checked)
}

/// CR-005 structural budgets (FR-GV-11 ext. / UAT-GV-08): the enforcement face
/// of the four new structural dimensions, in the fixed order `max_nesting_depth`,
/// `max_brain_methods`, `max_clone_ratio`, `no_god_containers`.
///
/// All four are PRODUCTION-SCOPED (is_test functions/containers excluded) under
/// the SAME effective thresholds the snapshot scored (`input.thresholds`), so a
/// budget and the dimension it enforces agree by construction — the posture the
/// existing max_dead/max_duplicates take with Redundancy. The `test_ids` set is
/// computed once and shared with each sub-check, exactly as the pre-decomposition
/// body did. Omitted keys are not enforced; ordering is deterministic
/// (function_metrics arrives ORDER BY id; god containers are returned id-ordered).
fn check_structural(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let test_ids: HashSet<NodeId> = input.test_node_ids.iter().copied().collect();
    let mut violations = Vec::new();
    let mut checked = 0u32;
    for (section_violations, section_checked) in [
        check_max_nesting_depth(input, &test_ids),
        check_brain_methods(input, &test_ids),
        check_clone_ratio(input, &test_ids),
        check_god_containers(input, &test_ids),
    ] {
        violations.extend(section_violations);
        checked += section_checked;
    }
    (violations, checked)
}

/// `max_nesting_depth` — per-function cap (FR-EX-07). A production function whose
/// max nesting depth exceeds the budget is one error, in node-id order. Reads the
/// structural fact from `function_metrics`; the name/file/line for the message
/// comes from the id-ordered node set (the metric row omits them).
fn check_max_nesting_depth(
    input: &EvalInput<'_>,
    test_ids: &HashSet<NodeId>,
) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let production_fns = || {
        input
            .function_metrics
            .iter()
            .filter(|f| !test_ids.contains(&f.id))
    };
    if let Some(budget) = input.compiled.rules.constraints.max_nesting_depth {
        checked += 1;
        let max = budget.max();
        let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();
        for f in production_fns() {
            if let Some(depth) = f.max_nesting_depth.filter(|d| *d > i64::from(max)) {
                let (name, file) = node_of
                    .get(&f.id)
                    .map(|n| (n.name.as_str(), n.file_path.clone().unwrap_or_default()))
                    .unwrap_or(("?", String::new()));
                violations.push(Violation {
                    rule: "max_nesting_depth".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file,
                    node_id: Some(f.id.get()),
                    message: format!(
                        "fn `{name}` nests {depth} levels deep > max_nesting_depth = {max}"
                    ),
                    ratchet: ratchet::mark(
                        budget.is_ratchet(),
                        symbol_of(&node_of, f.id),
                        depth.unsigned_abs(),
                    ),
                });
            }
        }
    }
    (violations, checked)
}

/// `max_brain_methods` — project-wide brain-method count (FR-QM-10). A brain
/// method meets all three thresholds (CC ∧ LOC ∧ nesting), exactly as the
/// Conciseness dimension defines it. One project-wide violation when the count
/// exceeds the budget.
fn check_brain_methods(input: &EvalInput<'_>, test_ids: &HashSet<NodeId>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let production_fns = || {
        input
            .function_metrics
            .iter()
            .filter(|f| !test_ids.contains(&f.id))
    };
    if let Some(budget) = input.compiled.rules.constraints.max_brain_methods {
        checked += 1;
        let t = &input.thresholds;
        let is_brain = |f: &&FunctionMetricRow| {
            f.cyclomatic_complexity.is_some_and(|c| c >= t.brain_cc)
                && f.line_count.is_some_and(|l| l >= t.brain_loc)
                && f.max_nesting_depth.is_some_and(|d| d >= t.brain_nest)
        };
        if budget.is_ratchet() {
            // FR-GV-23: under ratchet each brain method is a named offender.
            let node_of: HashMap<NodeId, &NodeRow> =
                input.nodes.iter().map(|n| (n.id, n)).collect();
            violations.extend(symbol_set_violations(
                "max_brain_methods",
                "a brain method",
                production_fns().filter(is_brain),
                &node_of,
            ));
        } else {
            let max = budget.max();
            let brain = production_fns().filter(is_brain).count() as u64;
            if brain > u64::from(max) {
                violations.push(Violation {
                    rule: "max_brain_methods".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: String::new(),
                    node_id: None,
                    message: format!("{brain} brain methods exceed max_brain_methods = {max}"),
                    ratchet: None,
                });
            }
        }
    }
    (violations, checked)
}

/// `max_clone_ratio` — project-wide near-clone production-function ratio
/// (FR-AN-06): production functions in a near-clone group over production
/// functions, exactly the Uniqueness numerator/denominator. One project-wide
/// violation when the ratio exceeds the budget. A clean (zero-function) project
/// has ratio 0 and never violates.
fn check_clone_ratio(input: &EvalInput<'_>, test_ids: &HashSet<NodeId>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let production_fns = || {
        input
            .function_metrics
            .iter()
            .filter(|f| !test_ids.contains(&f.id))
    };
    if let Some(budget) = input.compiled.rules.constraints.max_clone_ratio {
        checked += 1;
        let total = production_fns().count();
        let cloned = production_fns().filter(|f| f.clone_group.is_some()).count();
        let ratio = if total == 0 {
            0.0
        } else {
            cloned as f64 / total as f64
        };
        // FR-GV-23 generalises ADR-39's count-delta form to the ratio: the
        // scalar's ceiling and message are the pre-ratchet ones verbatim.
        if ratio > budget.ceiling() {
            violations.push(Violation {
                rule: "max_clone_ratio".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message: format!(
                    "{cloned}/{total} near-clone functions (ratio {ratio:.4}) exceed {}",
                    budget.describe()
                ),
                ratchet: None,
            });
        }
    }
    (violations, checked)
}

/// `no_god_containers` — no class-like container over the god thresholds
/// (FR-QM-12). Each god container is one error, in node-id order — the SAME set
/// Focus counts as god (shared `metrics::god_containers`, over the same bodied
/// methods since metric-semantics v7), so the budget and the dimension never
/// disagree. Only enforced when explicitly `true`.
fn check_god_containers(
    input: &EvalInput<'_>,
    test_ids: &HashSet<NodeId>,
) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    if let Some(budget) = input
        .compiled
        .rules
        .constraints
        .no_god_containers
        .filter(crate::config::GodContainersBudget::enforced)
    {
        checked += 1;
        let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();
        for god in crate::metrics::god_containers(
            input.nodes,
            input.edges,
            input.links,
            input.function_metrics,
            test_ids,
            input.thresholds,
        ) {
            let (name, file) = node_of
                .get(&god.id)
                .map(|n| (n.name.as_str(), n.file_path.clone().unwrap_or_default()))
                .unwrap_or(("?", String::new()));
            violations.push(Violation {
                rule: "no_god_containers".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file,
                node_id: Some(god.id.get()),
                message: format!(
                    "`{name}` is a god container ({} methods, span {}) — no_god_containers",
                    god.method_count, god.span
                ),
                // FR-GV-23: a god container worsens by gaining member methods.
                ratchet: ratchet::mark(
                    budget.is_ratchet(),
                    symbol_of(&node_of, god.id),
                    god.method_count,
                ),
            });
        }
    }
    (violations, checked)
}

/// Layer ordering and boundaries (per-edge checks, BR-11 / BR-15). The BR-11
/// ordering rule is active once any layer exists (counts once); each
/// `[[boundaries]]` rule counts once regardless. One violation per offending
/// file pair, not per edge ([NFR-RA-06]).
fn check_layer_ordering(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let rules = &input.compiled.rules;
    let has_layers = !rules.layers.is_empty();
    if has_layers {
        checked += 1; // the BR-11 ordering rule, active once layers exist
    }
    checked += rules.boundaries.len() as u32;
    if !has_layers {
        return (violations, checked);
    }

    // Documentation nodes are excluded so that doc↔code DocReference/TracesTo
    // edges cannot appear to violate layer ordering ([FR-DG-06], [ADR-19]).
    let file_of: HashMap<_, &str> = input
        .nodes
        .iter()
        .filter(|n| !n.kind.is_documentation())
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();
    // One violation per offending file pair, not per edge (a single bad
    // dependency direction shows once, deterministically).
    let mut ordering_seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut boundary_seen: BTreeSet<(usize, &str, &str)> = BTreeSet::new();

    for edge in input.edges {
        // Lexical containment is not a dependency, and derived
        // forbidden_dependency edges are last run's *output* (BR-12) —
        // neither is an input here. Documentation edges (DocReference /
        // TracesTo) are also excluded: they are cross-kind links from docs
        // to code and must never trigger layer-ordering violations ([FR-DG-06]).
        // A dispatch edge reaches an implementation through its trait: the
        // caller depends on the trait, so it crosses no layer (FR-QM-20).
        if matches!(
            edge.kind,
            EdgeKind::Contains | EdgeKind::ForbiddenDependency
        ) || edge.kind.is_documentation()
            || edge.is_dispatch()
        {
            continue;
        }
        let (Some(&src_file), Some(&dst_file)) =
            (file_of.get(&edge.source), file_of.get(&edge.target))
        else {
            continue; // a policy vertex or an unbound node — exempt
        };
        if src_file == dst_file {
            continue; // intra-file edges violate no layering
        }
        // BR-15: only edges between two *assigned* layers are checked.
        let (Some((src_layer, src_order)), Some((dst_layer, dst_order))) = (
            input.compiled.layer_of(src_file),
            input.compiled.layer_of(dst_file),
        ) else {
            continue;
        };

        // BR-11: a dep edge from order i to order j violates iff j > i.
        if dst_order > src_order && ordering_seen.insert((src_file, dst_file)) {
            violations.push(Violation {
                rule: "layer-ordering".to_string(),
                rule_type: "layer".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: src_file.to_string(),
                node_id: None,
                message: format!(
                    "`{src_file}` (layer `{src_layer}`, order {src_order}) depends \
                     upward on `{dst_file}` (layer `{dst_layer}`, order {dst_order})"
                ),
                ratchet: None,
            });
        }

        for (i, boundary) in rules.boundaries.iter().enumerate() {
            if boundary.from == src_layer
                && boundary.to == dst_layer
                && boundary_seen.insert((i, src_file, dst_file))
            {
                let reason = boundary
                    .reason
                    .as_deref()
                    .map(|r| format!(" — {r}"))
                    .unwrap_or_default();
                violations.push(Violation {
                    rule: format!("boundary:{}->{}", boundary.from, boundary.to),
                    rule_type: "boundary".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: src_file.to_string(),
                    node_id: None,
                    message: format!(
                        "`{src_file}` ({src_layer}) depends on `{dst_file}` ({dst_layer}), \
                         a forbidden boundary{reason}"
                    ),
                    ratchet: None,
                });
            }
        }
    }

    (violations, checked)
}

/// Forbidden imports (glob-level import linter, FR-GV-12 / CR-002).
///
/// Distinct from `[[boundaries]]`: matches path globs (not layer names) and
/// acts only on `Imports`/`References` edges (not every dependency kind), so it
/// can fence a dependency to a region of the tree. The matching
/// `forbidden_dependency` edge is materialised by the annotation pass
/// (`annotate::run`) — the same idempotent machinery boundaries use; here we
/// only produce the report. v1 covers resolved intra-workspace edges (both
/// endpoints are bound nodes carrying a file path); an external-package target
/// is an unresolved-reference-ledger row, not a graph edge — deferred
/// (CR-002 CRA-01).
fn check_forbidden_imports(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let forbidden_imports = &input.compiled.forbidden_imports;
    let checked = forbidden_imports.len() as u32;
    if forbidden_imports.is_empty() {
        return (violations, checked);
    }

    let file_of: HashMap<_, &str> = input
        .nodes
        .iter()
        .filter_map(|n| n.file_path.as_deref().map(|p| (n.id, p)))
        .collect();
    // One violation per (rule, source file, target file): a single banned
    // import shows once. Edges arrive ORDER BY (source, target, kind) and
    // the BTreeSet keeps the emitted order stable ([NFR-RA-06]).
    let mut seen: BTreeSet<(usize, &str, &str)> = BTreeSet::new();
    for edge in input.edges {
        if !matches!(edge.kind, EdgeKind::Imports | EdgeKind::References) {
            continue;
        }
        let (Some(&src_file), Some(&dst_file)) =
            (file_of.get(&edge.source), file_of.get(&edge.target))
        else {
            continue; // an unbound endpoint — not a resolved intra-workspace edge
        };
        for (i, fi) in forbidden_imports.iter().enumerate() {
            if fi.from.is_match(src_file)
                && fi.to.is_match(dst_file)
                && seen.insert((i, src_file, dst_file))
            {
                let reason = fi
                    .reason
                    .as_deref()
                    .map(|r| format!(" — {r}"))
                    .unwrap_or_default();
                // A forbidden import is a forbidden *dependency*, so it
                // reuses the `boundary` rule_type (and the same
                // `forbidden_dependency` edge) — no schema migration is
                // needed ([CR-002] "no migration"; the `violations` CHECK is
                // unchanged). The distinct `rule` key keeps it
                // unambiguous from a layer `[[boundaries]]` crossing.
                violations.push(Violation {
                    rule: format!("forbidden_import:{}->{}", fi.from_glob, fi.to_glob),
                    rule_type: "boundary".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: src_file.to_string(),
                    node_id: None,
                    message: format!(
                        "`{src_file}` imports `{dst_file}`, a forbidden import{reason}"
                    ),
                    ratchet: None,
                });
            }
        }
    }

    (violations, checked)
}

/// Require-tested coverage contract (`[[require_tested]]`, FR-GV-13 / CR-002).
///
/// A coverage gate over the PUBLIC API: every exported Function/Method whose
/// defining file matches a `paths` glob must be reachable by transitive `calls`
/// BFS from an `is_test` node. It reuses the shared `test_reachable_set`
/// reachability seeded from the SAME persisted `is_test` column
/// ([FR-AN-05], `test_node_ids`), so the contract and the reachability core can
/// never disagree. Non-exported symbols are exempt (a public-API test path, not total
/// coverage); an unbound symbol carries no path to glob-match. Orthogonal to the
/// metrics gate ([CR-002]) and — reading node reachability rather than edges — it
/// materialises no derived edge and needs no migration.
fn check_require_tested(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let require_tested = &input.compiled.require_tested;
    let checked = require_tested.len() as u32;
    if require_tested.is_empty() {
        return (violations, checked);
    }

    let test_ids: HashSet<NodeId> = input.test_node_ids.iter().copied().collect();
    let reachable = test_reachable_set(input.edges, &test_ids);
    // Rules in declaration order (outer), exported symbols in node-id order
    // (inner: `annotations` arrives ORDER BY id) — a deterministic list
    // grouped per contract ([NFR-RA-06]).
    for rt in require_tested {
        for ann in input.annotations {
            if ann.derived
                || !ann.exported
                || !matches!(ann.kind, NodeKind::Function | NodeKind::Method)
            {
                continue; // non-exported / non-callable / policy node — exempt
            }
            let Some(file) = ann.file_path.as_deref() else {
                continue; // an unbound symbol carries no path to glob-match
            };
            if !rt.paths.is_match(file) || reachable.contains(&ann.id) {
                continue; // off-glob, or covered by a transitive test path
            }
            let reason = rt
                .reason
                .as_deref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default();
            violations.push(Violation {
                rule: format!("require_tested:{}", rt.paths_glob),
                // A require-tested gap is a per-symbol *requirement*, not a
                // dependency boundary — it reuses the `constraint` rule_type
                // so the `violations` CHECK is unchanged ([CR-002] "no
                // migration"; allowed: constraint/layer/boundary). The
                // distinct `require_tested:` rule-key prefix keeps it
                // unambiguous from a numeric `[constraints]` budget.
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: file.to_string(),
                node_id: Some(ann.id.get()),
                // The BR-16 honesty caveat, inline: this is static call-graph
                // reachability, NOT execution coverage — a symbol reached only
                // through dynamic dispatch reads as untested here ([FR-GV-08]).
                message: format!(
                    "exported `{}` in `{file}` is not reachable from any test over the \
                     static `calls` graph (reachability, not execution coverage; \
                     dynamic-dispatch call paths are not seen){reason}",
                    ann.name
                ),
                ratchet: None,
            });
        }
    }

    (violations, checked)
}

/// Require-documented contract (`[[require_documented]]`, FR-GV-15 / CR-003).
///
/// The documentation analog of `[[require_tested]]`: every exported
/// Function/Method whose defining file matches a `paths` glob must be referenced
/// by some `DocSection` over a `DocReference` edge. It reuses the SAME
/// `documented_set` core `doc_gaps` builds ([FR-GV-14]), so the contract and
/// `doc_gaps` can never disagree about what is documented — exactly as
/// `[[require_tested]]` shares `test_reachable_set`. Non-exported
/// symbols are exempt (a public-API documentation gate, not total documentation);
/// an unbound symbol carries no path to glob-match. Orthogonal to the metrics
/// gate ([CR-002]/[CR-003]) and — reading direct references rather than the code
/// dependency graph — it materialises no derived edge and needs no migration.
fn check_require_documented(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let require_documented = &input.compiled.require_documented;
    let checked = require_documented.len() as u32;
    if require_documented.is_empty() {
        return (violations, checked);
    }

    let documented = documented_set(input.nodes, input.edges);
    // Rules in declaration order (outer), exported symbols in node-id order
    // (inner: `annotations` arrives ORDER BY id) — a deterministic list
    // grouped per contract ([NFR-RA-06]).
    for rd in require_documented {
        for ann in input.annotations {
            if ann.derived
                || !ann.exported
                || !matches!(ann.kind, NodeKind::Function | NodeKind::Method)
            {
                continue; // non-exported / non-callable / policy node — exempt
            }
            let Some(file) = ann.file_path.as_deref() else {
                continue; // an unbound symbol carries no path to glob-match
            };
            if !rd.paths.is_match(file) || documented.contains(&ann.id) {
                continue; // off-glob, or referenced by some doc section
            }
            let reason = rd
                .reason
                .as_deref()
                .map(|r| format!(" — {r}"))
                .unwrap_or_default();
            violations.push(Violation {
                rule: format!("require_documented:{}", rd.paths_glob),
                // A require-documented gap is a per-symbol *requirement*, not a
                // dependency boundary — it reuses the `constraint` rule_type so
                // the `violations` CHECK is unchanged ([CR-003] rides the
                // existing schema; allowed: constraint/layer/boundary). The
                // distinct `require_documented:` rule-key prefix keeps it
                // unambiguous from a numeric `[constraints]` budget and from a
                // `require_tested:` gap.
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: file.to_string(),
                node_id: Some(ann.id.get()),
                // The FR-GV-14 honesty caveat, inline: this is reference
                // presence in the doc graph, NOT documentation quality.
                message: format!(
                    "exported `{}` in `{file}` is not referenced by any documentation \
                     section (reference presence, not documentation quality){reason}",
                    ann.name
                ),
                ratchet: None,
            });
        }
    }

    (violations, checked)
}

/// The set of nodes reachable from any test node over `Calls` edges — the
/// shared static-reachability core of the `[[require_tested]]` coverage
/// contract ([FR-GV-13], BR-16). The seed `test_ids` are
/// themselves included in the returned set (a test is trivially "reached").
///
/// Pure and order-independent: the result is a *set*, so the internal
/// `HashMap`/`HashSet` iteration order never reaches a serialized output — both
/// callers derive deterministic, id-ordered reports from it ([NFR-RA-06]).
///
/// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
/// [FR-GV-13]: ../../../docs/specs/requirements/FR-GV-13.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn test_reachable_set(edges: &[EdgeRow], test_ids: &HashSet<NodeId>) -> HashSet<NodeId> {
    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for edge in edges {
        if edge.kind == EdgeKind::Calls {
            adjacency.entry(edge.source).or_default().push(edge.target);
        }
    }
    let mut reachable: HashSet<NodeId> = test_ids.clone();
    let mut queue: VecDeque<NodeId> = test_ids.iter().copied().collect();
    while let Some(current) = queue.pop_front() {
        if let Some(callees) = adjacency.get(&current) {
            for &callee in callees {
                if reachable.insert(callee) {
                    queue.push_back(callee);
                }
            }
        }
    }
    reachable
}

/// The set of nodes referenced by at least one [`NodeKind::DocSection`] over a
/// [`EdgeKind::DocReference`] edge — the shared documentation-reference core of
/// `doc_gaps` ([FR-GV-14]) and the `[[require_documented]]` contract
/// ([FR-GV-15]), the documentation analog of [`test_reachable_set`].
///
/// Only `DocSection`-sourced references count, per [FR-GV-14]/[FR-GV-15]: a
/// reference from a file-level preamble is sourced from the enclosing `DocFile`,
/// not a section, so it does not document a symbol here. The reference is direct,
/// not transitive — documentation has no analog of the `calls` BFS.
///
/// Pure and order-independent: the result is a *set*, so the internal
/// `HashMap`/`HashSet` iteration order never reaches a serialized output — both
/// callers derive deterministic, id-ordered reports from it ([NFR-RA-06]).
///
/// [FR-GV-14]: ../../../docs/specs/requirements/FR-GV-14.md
/// [FR-GV-15]: ../../../docs/specs/requirements/FR-GV-15.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn documented_set(nodes: &[NodeRow], edges: &[EdgeRow]) -> HashSet<NodeId> {
    let doc_sections: HashSet<NodeId> = nodes
        .iter()
        .filter(|n| n.kind == NodeKind::DocSection)
        .map(|n| n.id)
        .collect();
    let mut documented: HashSet<NodeId> = HashSet::new();
    for edge in edges {
        if edge.kind == EdgeKind::DocReference && doc_sections.contains(&edge.source) {
            documented.insert(edge.target);
        }
    }
    documented
}

/// Re-run the annotation pass over the store behind `runtime` under `rules`
/// ([FR-GV-02], BR-12) — the step that makes an evaluation follow the contract
/// as it stands now, shared by `check`/`scan` ([`materialise_and_evaluate`])
/// and the branch review, which applies it to both its measurements.
///
/// BR-12: clear + re-materialise the derived policy graph each run. The
/// annotation pass owns that algorithm (FR-AN-03); re-running it here is what
/// makes check_rules idempotent AND correct after a rules.toml edit that
/// touched no source file — the layer and forbidden verdicts, and the
/// near-clone and duplicate verdicts its `[metric_thresholds]` tune. A full
/// re-materialisation (a rules.toml edit can flip verdicts across the whole
/// graph with no source change), so it writes every verdict —
/// `incremental = false` (S-024-HF); this is the governance path, not the
/// NFR-PE-03 sync hot path. CR-043 / ADR-39: the reachability-capability set
/// gates dead-code so the re-materialised `is_dead` column matches what
/// index/sync wrote. The policy nodes it re-materialises carry the coordinate
/// the store's symbols already carry (S-703), whatever the project resolves to
/// now. The caller advances the engine's sync stamp when `runtime` is its own.
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
pub(crate) fn rematerialise(
    runtime: &Runtime,
    registry: &crate::plugin::LanguageRegistry,
    config: &crate::config::Config,
    rules: &Rules,
) -> Result<()> {
    let ctx = crate::pipeline::store_symbol_context(runtime)?;
    crate::annotate::run(
        runtime,
        rules,
        &config.semantics.entry_points,
        &config.semantics.test_markers,
        &crate::annotate::Reachability::of(registry),
        &ctx,
        false,
    )?;
    Ok(())
}

/// Re-materialise the derived policy graph from the (possibly overridden)
/// contract and run the evaluator over the post-materialisation snapshot
/// ([FR-GV-02]: layers assigned, `forbidden_dependency` edges cleared at
/// start and rebuilt — delegated to the S-014 annotation pass, BR-12).
///
/// `cycles` must come from the same SCC set the metrics scored (the
/// [FR-QM-02] reuse contract); pass `None` to have it computed here (the
/// bare `check_rules` path).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-QM-02]: ../../../docs/specs/requirements/FR-QM-02.md
fn materialise_and_evaluate(
    engine: &Engine,
    compiled: &CompiledRules,
    cycles: Option<u64>,
) -> Result<(Vec<Violation>, u32)> {
    let (runtime, registry, config) = engine.pipeline_ctx()?;
    rematerialise(runtime, registry, &config, &compiled.rules)?;
    engine.advance_sync_stamp();

    let facts = StoreFacts::read(runtime)?;
    let cycles = match cycles {
        Some(c) => c,
        // Both the bare check_rules path and scan land here (each calls
        // materialise_and_evaluate with no precomputed cycle count), and only
        // when the contract actually budgets cycles.
        None if compiled.rules.constraints.max_cycles.is_some() => {
            facts.cycle_count(&*engine.hydrate(Granularity::ExcludeContains)?)
        }
        None => 0,
    };
    Ok(facts.evaluate(compiled, cycles))
}

/// The store facts [`evaluate`] reads, in one `submit_read` — shared by the
/// `check`/`scan` path ([`materialise_and_evaluate`]) and the branch review
/// ([`review`]), so the two evaluate one contract over one read shape and can
/// never disagree about what a violation is.
pub(crate) struct StoreFacts {
    pub nodes: Vec<NodeRow>,
    pub edges: Vec<EdgeRow>,
    /// The container links, as the metrics read them (S-557, S-510).
    pub links: ContainerLinks,
    pub functions: Vec<FunctionConstraintRow>,
    /// The is_dead/is_duplicate annotation rows for the FR-GV-11 redundancy
    /// budgets — the SAME query the Redundancy metric reads, so the budget and
    /// the metric agree by construction.
    pub function_metrics: Vec<FunctionMetricRow>,
    /// The annotation rows carry the `exported` flag the FR-GV-13
    /// [[require_tested]] contract needs (NodeRow omits visibility).
    pub annotations: Vec<AnnotationNodeRow>,
    /// The persisted is_test set: shared with the [[require_tested]] contract
    /// (FR-AN-05 / FR-GV-13) and used for production-scope metric filtering
    /// (FR-QM-08) — the gate, coverage contract, and Acyclicity rule can never
    /// disagree about the same SCC set.
    pub test_node_ids: Vec<NodeId>,
}

impl StoreFacts {
    /// Read every fact in one checkout of `runtime`'s store.
    pub(crate) fn read(runtime: &Runtime) -> Result<Self> {
        runtime.submit_read(|store| {
            let nodes = store.all_nodes()?;
            let links = ContainerLinks::new(
                store.partial_type_links()?,
                &nodes,
                &store.node_self_types()?,
                &store.impl_blocks()?,
            );
            Ok(Self {
                nodes,
                edges: store.all_edges()?,
                links,
                functions: store.function_constraint_rows()?,
                function_metrics: store.function_metrics()?,
                annotations: store.annotation_nodes()?,
                test_node_ids: store.test_node_ids()?,
            })
        })
    }

    /// The production `is_test` set, as the metrics read it.
    pub(crate) fn test_ids(&self) -> HashSet<NodeId> {
        self.test_node_ids.iter().copied().collect()
    }

    /// The Acyclicity cycle count over `view`. Only that count is read — the
    /// CR-005 thresholds and the Cohesion capability set do not affect it, so
    /// the documented defaults and an empty set suffice. It is the computed
    /// count whether or not Acyclicity applies (FR-QM-18): the budget is the
    /// contract's, not the signal's.
    pub(crate) fn cycle_count(&self, view: &crate::hydrate::GraphView) -> u64 {
        crate::metrics::compute(
            view,
            &self.nodes,
            &self.edges,
            &self.links,
            &self.function_metrics,
            &self.test_ids(),
            crate::metrics::Thresholds::default(),
            &HashSet::new(),
        )
        .acyclicity
        .raw as u64
    }

    /// [`evaluate`] `compiled` over these facts, with `cycles` as the
    /// `max_cycles` input and the contract's effective CR-005 thresholds — the
    /// SAME set the snapshot scores under (BR-25).
    pub(crate) fn evaluate(&self, compiled: &CompiledRules, cycles: u64) -> (Vec<Violation>, u32) {
        evaluate(&EvalInput {
            compiled,
            nodes: &self.nodes,
            edges: &self.edges,
            links: &self.links,
            functions: &self.functions,
            function_metrics: &self.function_metrics,
            annotations: &self.annotations,
            test_node_ids: &self.test_node_ids,
            cycles,
            thresholds: effective_thresholds(&compiled.rules),
        })
    }
}

/// What a governance run evaluated, carried from the run to its marker
/// (S-437, [CR-140] §3.1, [FR-GV-21]).
///
/// The two figures are **not** one fact. `checked_rules` is
/// `violation_count`'s denominator; `rules_present` says whether a contract
/// was authored at all, which the count alone cannot — a present contract
/// authoring zero rules is [`logos init`](crate::init)'s default template, and
/// it is a different state from an unconfigured project even though both
/// evaluate nothing.
///
/// `operation` is which run is writing: [`CHECK_RUN_OP_CHECK`] or
/// [`CHECK_RUN_OP_SCAN`]. Both call [`persist_violations`], so the marker
/// could not previously say which one it described.
///
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
#[derive(Debug, Clone, Copy)]
struct EvaluatedSet {
    /// How many rules the run evaluated.
    checked_rules: u32,
    /// Whether a rules contract was authored at all —
    /// `compiled.hash != ABSENT_RULES_HASH`, the same honest signal the report
    /// carries on [`RulesReport::rules_present`].
    rules_present: bool,
    /// [`CHECK_RUN_OP_CHECK`] or [`CHECK_RUN_OP_SCAN`]; any other spelling is
    /// refused by migration 21's `CHECK`.
    operation: &'static str,
}

/// Persist the violation set and the run marker that describes it
/// ([FR-GV-02], SRS §5.1: written per run, replaced wholesale — idempotent
/// like the derived policy graph; [FR-GV-21]).
///
/// Returns the `ran_at` it stamped, so the caller can report the run it just
/// recorded without re-reading the store. Both writes go through one
/// `submit_write`, and a write job is one transaction ([NFR-RA-07]), so the
/// marker and its rows can never describe different runs.
///
/// `head` is `HEAD` as the run's freshness step resolved it — `None` outside a
/// repo, stored as NULL rather than a placeholder ([FR-GV-21]).
///
/// `evaluated` is what the run scored **against** — the denominator the marker
/// lacked until S-437 ([CR-140] §3.1). Both callers already compute it; this
/// parameter exists because it was being dropped here, at the persistence
/// boundary, and a `violation_count` with no denominator read back as a clean
/// check over an empty evaluated set.
///
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
fn persist_violations(
    runtime: &Runtime,
    snapshot_id: Option<i64>,
    violations: &[Violation],
    head: Option<&str>,
    evaluated: EvaluatedSet,
) -> Result<i64> {
    let owned: Vec<Violation> = violations.to_vec();
    let head_owned: Option<String> = head.map(str::to_owned);
    let ran_at = unix_now();
    runtime.submit_write(move |w| {
        let rows: Vec<NewViolation<'_>> = owned
            .iter()
            .map(|v| NewViolation {
                snapshot_id,
                rule_type: &v.rule_type,
                rule_key: &v.rule,
                node_id: v.node_id,
                file: (!v.file.is_empty()).then_some(v.file.as_str()),
                message: &v.message,
                severity: &v.severity,
            })
            .collect();
        w.replace_violations(
            &rows,
            NewCheckRun {
                ran_at,
                commit_sha: head_owned.as_deref(),
                checked_rules: evaluated.checked_rules,
                rules_present: evaluated.rules_present,
                operation: evaluated.operation,
            },
        )
    })?;
    Ok(ran_at)
}

// ── The aggregate runs (Engine method bodies) ───────────────────────────────

/// `scan` — reconcile-then-score ([FR-RC-01], [ADR-11]): the full
/// architecture-quality scan, persisting a snapshot ([FR-GV-09]) and the
/// violation set.
///
/// [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
pub(crate) fn scan(engine: &Engine, reconcile: bool) -> Result<ScanResult> {
    let fresh = reconcile_step(engine, reconcile)?;
    let compiled = load_rules_cached(engine, None)?;
    let runtime = quality_runtime(engine)?;

    // Score on the freshly reconciled + re-materialised graph. The rule count
    // was discarded here until S-437; it is the marker's denominator (CR-140
    // §3.1), and `scan` evaluates exactly the same set `check` does.
    let (violations, checked_rules) = materialise_and_evaluate(engine, &compiled, None)?;
    // FR-GV-23: scored against the blessed set exactly as `check` is — read
    // only; `scan` never writes it.
    let (violations, _) = classify_against_blessed(engine, &compiled, violations)?;
    let rules_present = compiled.hash != ABSENT_RULES_HASH;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: the snapshot scores under the effective rules.toml thresholds, so
    // its persisted hash gates the baseline (FR-GV-10) and the budgets agree.
    let thresholds = effective_thresholds(&compiled.rules);
    // The per-dimension worst-offender lists (CR-005 §3.2) come back from the
    // snapshot that persisted them (FR-QM-15): computed from the same read as
    // the metrics, under the same thresholds, and written in the same batch —
    // so what `scan` reports is exactly what the Health bundle later projects.
    // The "where to start" ranking (FR-QM-17) rides the same snapshot, computed
    // after the metrics and never into them (BR-61).
    let crate::metrics::RecordedSnapshot {
        id: snapshot_id,
        metrics,
        worst_offenders,
        where_to_start,
    } = crate::metrics::snapshot(
        runtime,
        &view,
        fresh.head.as_deref(),
        thresholds,
        &member_access_extensions(engine),
    )?;
    persist_violations(
        runtime,
        Some(snapshot_id),
        &violations,
        fresh.head.as_deref(),
        EvaluatedSet {
            checked_rules,
            rules_present,
            // `scan` writes the marker because it replaces the violation set
            // wholesale (FR-GV-09 + FR-GV-02 idempotence). Recording WHICH
            // operation wrote it is what stops a reader attributing a `scan`'s
            // marker to a `logos check` that never ran (CR-140 §2.2).
            operation: CHECK_RUN_OP_SCAN,
        },
    )?;

    // The non-gated temporal tier (CR-006, FR-GH-07): computed independently of
    // every gated column above and attached as advisory detail. Fail-soft — a
    // history error (e.g. a non-compiling [history] defect_patterns) degrades to
    // an n/a tier + warning, never failing the gated scan (BR-26 two-tier rule).
    let freshness = fresh.line();
    let mut warnings = fresh.warnings;
    let notes = fresh.notes;
    let temporal = scan_temporal_tier(engine, &mut warnings);
    // The files whose facts could not be persisted (FR-EH-05, S-513), read after
    // this run's reconcile retried them, when one ran (`--no-reconcile` reads
    // the record as it stands) — the readout `status` carries too.
    let persistence = crate::models::PersistenceHealth::from_rows(
        &runtime.submit_read(|store| store.persist_failures())?,
    );
    if let Some(warning) = persistence.warning() {
        warnings.push(warning);
    }
    // FR-QM-18: the languages the graph cannot see across files, and FR-IX-14:
    // the large languages a tenth of whose files parsed damaged — advisory,
    // never a verdict, both from one reading of the per-language rows and the
    // production callables.
    warnings.extend(scan_coverage_warnings(runtime)?);
    // FR-GV-27: under `[review]`, a distributional dimension in decline warns
    // over the series this scan just extended.
    if let Some(policy) = compiled.rules.review.as_ref() {
        warnings.extend(review::trend_warnings(runtime, policy)?);
    }

    Ok(ScanResult {
        signal: metrics.aggregate_signal,
        freshness,
        violations,
        metrics,
        worst_offenders,
        where_to_start,
        temporal,
        persistence,
        warnings,
        notes,
    })
}

/// Build the scan's non-gated temporal tier ([FR-GH-07]) from the lazy
/// [`temporal_report`](Engine::temporal_report) read. Fail-soft: a history
/// error never breaks the gated scan — it degrades to an empty, `n/a` tier
/// carrying the reason as a warning ([BR-26], [NFR-RA-05]).
///
/// [FR-GH-07]: ../../../docs/specs/requirements/FR-GH-07.md
/// [BR-26]: ../../../docs/specs/software-spec.md#322-git-history-analytics
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn scan_temporal_tier(engine: &Engine, warnings: &mut Vec<String>) -> TemporalTier {
    match engine.temporal_report() {
        Ok(report) => temporal_tier_from_report(report),
        Err(err) => {
            // History never gates: a temporal failure is advisory noise here.
            warnings.push(format!("temporal tier unavailable: {err:#}"));
            temporal_tier_unavailable()
        }
    }
}

/// The read-only twin of [`scan_temporal_tier`]: the non-gated temporal tier
/// built from the **read-only** [`latest_temporal_report`](Engine::latest_temporal_report)
/// — the last-mined facts, never a fresh mine-and-persist ([CR-018], [ADR-28]).
/// Same fail-soft posture: a history error degrades to an `n/a` tier, never an
/// error, since the dashboard's Health view leads with the gated signal.
///
/// [CR-018]: ../../../docs/requests/CR-018-web-dashboard-write-on-read.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
fn latest_temporal_tier(engine: &Engine, warnings: &mut Vec<String>) -> TemporalTier {
    match engine.latest_temporal_report() {
        Ok(report) => temporal_tier_from_report(report),
        Err(err) => {
            warnings.push(format!("temporal tier unavailable: {err:#}"));
            temporal_tier_unavailable()
        }
    }
}

/// The empty `n/a` tier rendered when the temporal read failed — advisory, never
/// gated ([BR-26], [NFR-RA-05]).
fn temporal_tier_unavailable() -> TemporalTier {
    TemporalTier {
        tier: crate::history::TIER_LABEL,
        gated: false,
        defect_label: crate::history::DEFECT_LABEL,
        notice: Some("temporal tier unavailable".to_string()),
        ..TemporalTier::default()
    }
}

/// Project a [`TemporalReport`](crate::history::TemporalReport) into the scan
/// read-model's non-gated [`TemporalTier`] — pure, shared by the persisting
/// (`scan`) and read-only (`latest_scan`) paths so the two stay byte-identical.
fn temporal_tier_from_report(report: crate::history::TemporalReport) -> TemporalTier {
    // Degraded reason takes precedence in the notice; otherwise a first mine.
    let notice = report
        .degraded
        .map(|r| r.message().to_string())
        .or_else(|| {
            report
                .first_mine
                .then(|| crate::history::FIRST_MINE_NOTICE.to_string())
        });

    TemporalTier {
        tier: crate::history::TIER_LABEL,
        gated: false,
        defect_label: crate::history::DEFECT_LABEL,
        head_sha: report.head_sha,
        config_hash: Some(report.config_hash),
        degraded: report.degraded,
        notice,
        files: report.files,
    }
}

// ── Read-only read-model accessors (S-082, CR-018, ADR-28) ──────────────────
//
// The non-persisting twins of the evaluate-and-persist `scan`/`gate` paths the
// web dashboard's Health/Overview/Metrics views read through, so a page GET
// reflects the **last persisted** snapshot and never writes a `metric_snapshots`
// row ([FR-UI-03]). The CLI/MCP `scan`/`gate` paths are unchanged. None of these
// reconcile, score, or persist — they read the last row the store already holds.

/// The last persisted snapshot as the read path sees it: its metric breakdown
/// and the worst-offender lists persisted with it ([FR-QM-15]) — two
/// projections of the one row [`latest_metrics`] read.
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
pub(crate) struct PersistedSnapshot {
    /// The row's id: everything else of this snapshot is read by it.
    pub(crate) id: i64,
    /// The reconstructed [`MetricSnapshot`] breakdown.
    pub(crate) metrics: MetricSnapshot,
    /// The lists persisted with the row; `recorded: false` on a snapshot written
    /// before they were persisted.
    pub(crate) worst_offenders: WorstOffenders,
    /// The "where to start" ranking persisted with the row ([FR-QM-17]);
    /// `recorded: false` on a snapshot written before migration 36.
    ///
    /// [FR-QM-17]: ../../../docs/specs/requirements/FR-QM-17.md
    pub(crate) where_to_start: WhereToStart,
}

/// The most-recent persisted metric snapshot as a full read-model, or `None` on
/// a never-`scan`-ned store ([ADR-28] `Engine::latest_metrics`). A pure read —
/// no compute, no persist. Reconstructs the [`MetricSnapshot`] breakdown from the
/// last `metric_snapshots` row, preserving the Cohesion/Focus applicability
/// drop-out as `None` (the [NFR-CC-04] n/a sentinel), never a fabricated zero,
/// and carries the offender lists persisted with that same row ([FR-QM-15]) —
/// naming in `unrecorded` any CR-209 list the row predates
/// ([`unrecorded_lists`](crate::metrics::unrecorded_lists)).
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
pub(crate) fn latest_metrics(engine: &Engine) -> Result<Option<PersistedSnapshot>> {
    let row = quality_runtime(engine)?.submit_read(|store| store.latest_metric_snapshot())?;
    Ok(row.map(|mut row| {
        let mut worst_offenders = std::mem::take(&mut row.worst_offenders);
        let where_to_start = std::mem::take(&mut row.where_to_start);
        let id = row.id;
        let metrics = metric_snapshot_from_row(row);
        // A snapshot written before CR-209 holds no row for the four lists it
        // added; name each one its own score says is not empty (NFR-CC-04).
        worst_offenders.unrecorded = crate::metrics::unrecorded_lists(&metrics, &worst_offenders);
        PersistedSnapshot {
            id,
            metrics,
            worst_offenders,
            where_to_start,
        }
    }))
}

/// Read a persisted graph-dimension applicability flag back as the drop-out it
/// records ([FR-QM-18]; Modularity's since [CR-156]) — the one mapping every
/// snapshot reader shares (the dashboard read-back, the gate's regression
/// detail, the evolution series).
///
/// Only an explicit `0` drops a dimension out; a `NULL` flag is a row scored
/// before the dimension could drop out, when it always applied. A v8 row
/// carries the cross-module edge count the flag was decided on, and reads back
/// with it; a v6/v7 Modularity drop-out has none and was decided on every edge
/// of its graph, so it reads back with that row's `edge_count` and the reason
/// that rule gave.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
/// [CR-156]: ../../../docs/requests/CR-156-modularity-drops-out-of-a-too-small-graph.md
fn graph_drop_out(
    applicable: Option<bool>,
    cross_module_edges: Option<i64>,
    edge_count: i64,
) -> Option<GraphDimensionNotApplicable> {
    match (applicable, cross_module_edges) {
        (Some(false), Some(cross)) => GraphDimensionNotApplicable::for_cross_module_edges(cross as u64),
        (Some(false), None) => GraphDimensionNotApplicable::for_edges(edge_count as u64),
        (Some(true) | None, _) => None,
    }
}

/// The three graph dimensions' drop-outs read back from one persisted row's
/// flags, in canonical order (modularity, acyclicity, depth).
fn graph_drop_outs(
    flags: [Option<bool>; 3],
    cross_module_edges: Option<i64>,
    edge_count: i64,
) -> [Option<GraphDimensionNotApplicable>; 3] {
    flags.map(|flag| graph_drop_out(flag, cross_module_edges, edge_count))
}

/// [`graph_drop_outs`] of one gate/evolution series row.
fn row_drop_outs(row: &MetricSnapshotRow) -> [Option<GraphDimensionNotApplicable>; 3] {
    graph_drop_outs(
        [row.modularity_applicable, row.acyclicity_applicable, row.depth_applicable],
        row.cross_module_edges,
        row.edge_count,
    )
}

/// Map the persisted [`LatestMetricSnapshot`] columns back into the
/// [`MetricSnapshot`] read-model. The original five are always present; the
/// CR-005 structural pairs are `None` only on a pre-v3 snapshot (defaulted to
/// `0.0` — those legacy rows never reach the dashboard, which reads the latest);
/// Cohesion/Focus stay `None` when their applicability flag says they dropped
/// out, so the view renders the honest `n/a` ([NFR-CC-04]), never a zero.
fn metric_snapshot_from_row(row: LatestMetricSnapshot) -> MetricSnapshot {
    let mv = |raw: f64, normalized: f64| MetricValue { raw, normalized };
    let req = |raw: Option<f64>, normalized: Option<f64>| {
        mv(raw.unwrap_or(0.0), normalized.unwrap_or(0.0))
    };
    // An applicability drop-out (flag `Some(false)`) or a missing value → `None`;
    // a present value with the flag absent (pre-flag snapshot) still surfaces.
    let opt = |applicable: Option<bool>, raw: Option<f64>, normalized: Option<f64>| {
        match (applicable, raw, normalized) {
            (Some(false), _, _) => None,
            (_, Some(raw), Some(normalized)) => Some(mv(raw, normalized)),
            _ => None,
        }
    };
    let [modularity_not_applicable, acyclicity_not_applicable, depth_not_applicable] = graph_drop_outs(
        [row.modularity_applicable, row.acyclicity_applicable, row.depth_applicable],
        row.cross_module_edges,
        row.edge_count,
    );
    MetricSnapshot {
        modularity: mv(row.modularity_raw, row.modularity_normalized),
        // CR-156, FR-QM-18: only an explicit `0` drops a graph dimension out; a
        // `NULL` flag is a row scored when it always applied.
        modularity_not_applicable,
        acyclicity: mv(row.acyclicity_raw, row.acyclicity_normalized),
        acyclicity_not_applicable,
        depth: mv(row.depth_raw, row.depth_normalized),
        depth_not_applicable,
        cross_module_edge_count: row.cross_module_edges.map(|n| n as u64),
        equality: mv(row.equality_raw, row.equality_normalized),
        redundancy: mv(row.redundancy_raw, row.redundancy_normalized),
        nesting: req(row.nesting_raw, row.nesting_normalized),
        conciseness: req(row.conciseness_raw, row.conciseness_normalized),
        cohesion: opt(row.cohesion_applicable, row.cohesion_raw, row.cohesion_normalized),
        // The row records that Cohesion dropped out, not why (CR-252): a
        // too-few-classes drop-out reads back as the plain n/a.
        cohesion_not_applicable: None,
        focus: opt(row.focus_applicable, row.focus_raw, row.focus_normalized),
        // The row records Focus, not how many methods it left unattributed
        // (S-510): a stored snapshot reads back without the figure.
        focus_attribution: None,
        uniqueness: req(row.uniqueness_raw, row.uniqueness_normalized),
        thresholds_hash: row.thresholds_hash.unwrap_or_default(),
        node_count: row.node_count as u64,
        edge_count: row.edge_count as u64,
        function_count: row.function_count as u64,
        test_function_count: row.test_function_count as u64,
        empty: row.empty,
        aggregate_signal: row.aggregate_signal.map(|s| s as u32),
    }
}

/// The read-only twin of [`scan`]: the last persisted snapshot's metric
/// breakdown and the worst-offender lists persisted with it ([FR-QM-15]),
/// joined with the read-only temporal tier, or `None` on a never-`scan`-ned
/// store ([ADR-28], [CR-018], S-082). Backs the web dashboard's Metrics and
/// Health views. Carries **no** rule violations — those are not persisted on
/// the snapshot — and recomputes nothing, so a GET stays a pure read. A
/// snapshot written before the lists were persisted projects them as
/// [`recorded: false`](WorstOffenders::recorded), never as an empty result.
///
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
///
/// A never-`scan`-ned store yields a `ScanResult` whose `metrics.empty` is
/// `true` — the same honest sentinel `scan` returns for an empty production graph,
/// so the flag alone does **not** say which of the two happened. A surface that
/// names a cause or a next command must separate them first ([FR-EH-04], [CR-130]):
/// whether any `metric_snapshots` row exists is the distinguishing fact, and the
/// evolution series carries it. Either way the view renders an honest empty state,
/// never zeros ([NFR-CC-04]).
pub(crate) fn latest_scan(engine: &Engine) -> Result<ScanResult> {
    Ok(scan_from_snapshot(engine, latest_metrics(engine)?))
}

/// Project an **already-read** snapshot into the read-only [`ScanResult`].
///
/// Split out of [`latest_scan`] so [`latest_health`] can derive the Health
/// bundle's scan result from the *same* value its gate verdict was derived from
/// ([CR-135] §3.2). [`latest_scan`] keeps its own signature and behaviour for
/// its other callers by taking the read and handing it straight here — it does
/// not retain a second read of its own, so there is exactly one place the last
/// persisted snapshot becomes a `ScanResult`.
///
/// `None` (a never-`scan`-ned store) becomes the honest empty sentinel, never a
/// fabricated zero ([NFR-CC-04]) — see [`latest_scan`] for why the flag alone
/// does not say which of the two empties happened. Its offender lists and its
/// "where to start" ranking are "not recorded", like a pre-[FR-QM-15]
/// snapshot's: there is nothing to show.
///
/// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
/// [FR-QM-15]: ../../../docs/specs/requirements/FR-QM-15.md
fn scan_from_snapshot(engine: &Engine, snapshot: Option<PersistedSnapshot>) -> ScanResult {
    let PersistedSnapshot {
        metrics,
        worst_offenders,
        where_to_start,
        ..
    } = snapshot.unwrap_or(PersistedSnapshot {
        id: 0,
        metrics: MetricSnapshot {
            empty: true,
            ..MetricSnapshot::default()
        },
        worst_offenders: WorstOffenders::default(),
        where_to_start: WhereToStart::default(),
    });
    let mut warnings = Vec::new();
    let temporal = latest_temporal_tier(engine, &mut warnings);
    ScanResult {
        signal: metrics.aggregate_signal,
        freshness: String::new(),
        violations: Vec::new(),
        metrics,
        worst_offenders,
        where_to_start,
        temporal,
        persistence: crate::models::PersistenceHealth::default(),
        warnings,
        notes: Vec::new(),
    }
}

/// The read-only gate **verdict**: compare the last persisted snapshot's signal
/// to the saved baseline **without computing or persisting** a snapshot ([ADR-28]
/// — the dashboard's Health verdict). Mirrors [`gate`]'s comparison for the
/// comparable case (`current < baseline − ε` ⇒ FAIL, BR-10, or the
/// `[review]` drift budget, FR-GV-25) but never
/// re-baselines or writes: an incomparable baseline (different metric semantics
/// or threshold hash) is reported as an informational pass, not an auto-save.
/// A never-`scan`-ned store returns an `n/a` verdict naming the producing command.
pub(crate) fn latest_gate(engine: &Engine) -> Result<GateResult> {
    let snapshot = latest_metrics(engine)?;
    gate_from_snapshot(engine, snapshot.as_ref())
}

/// Project an **already-read** snapshot into the read-only gate verdict.
///
/// The gate-side twin of [`scan_from_snapshot`], split out for the same reason:
/// [`latest_health`] derives both Health fields from one snapshot value, and
/// [`latest_gate`] keeps working for its other callers by handing its own read
/// straight here rather than retaining a second one ([CR-135] §3.2).
///
/// The baseline is read **here** and not at the seam, deliberately. It is the
/// gate's own operand, it is compared against whatever snapshot it is given, and
/// it carries no cross-field invariant with the scan result — joining it to the
/// snapshot read would buy no atomicity, only a wider read.
///
/// Under a `rules.toml [review]` table the verdict carries the attribution the
/// persisting gate reports ([FR-GV-26]), so the dashboard shows what `gate
/// --json` and the MCP `session_end` do. The table is read without the parse
/// cache, which writes; an unreadable contract attributes nothing here and
/// leaves the verdict standing (the gate itself reports the contract fault).
/// Its drivers are that snapshot's own offenders, read by its id — never by
/// "latest" again, which a `scan` committing in between would move.
///
/// Under the same table, on every arm, the verdict carries the trend the
/// persisting gate reports ([FR-GV-27]), read off the snapshots up to this
/// one by id for the same reason. Without the table, or with an unreadable
/// contract, it carries none and the verdict is what it was.
///
/// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
/// [FR-GV-27]: ../../../docs/specs/requirements/FR-GV-27.md
fn gate_from_snapshot(engine: &Engine, snapshot: Option<&PersistedSnapshot>) -> Result<GateResult> {
    let runtime = quality_runtime(engine)?;
    let Some(PersistedSnapshot { id: snapshot_id, metrics, .. }) = snapshot else {
        return Ok(GateResult {
            passed: true,
            epsilon: EPSILON,
            message: "no snapshot yet — run `logos scan` to record one".to_string(),
            ..GateResult::default()
        });
    };

    let mut result = GateResult {
        passed: true,
        saved: false,
        signal: metrics.aggregate_signal,
        baseline_signal: None,
        test_function_count: metrics.test_function_count,
        threshold: None,
        epsilon: EPSILON,
        regressions: Vec::new(),
        structural_faults: Vec::new(),
        freshness: String::new(),
        message: String::new(),
        warnings: Vec::new(),
        notes: Vec::new(),
        security_violations: Vec::new(),
        missing_scans: Vec::new(),
        gating_entries: Vec::new(),
        drift: None,
        attribution: None,
        baseline_reason: None,
        baseline_reset: None,
        trend: Vec::new(),
    };

    let (baseline, record) = runtime.submit_read(|store| {
        Ok((store.baseline_snapshot(SCOPE_PROJECT)?, store.baseline_record(SCOPE_PROJECT)?))
    })?;
    // Read once, for every arm: the trend below reads it whatever the
    // baseline, and only the comparable arm judges by it.
    let review_table = crate::config::load_rules_from_root(engine.root())
        .ok()
        .and_then(|rules| rules.review);
    match baseline {
        None => {
            result.message =
                "no baseline saved — informational pass (save one with `gate --save`)".to_string();
        }
        // FR-GV-10: an incomparable baseline (semantics or thresholds changed
        // since it was saved) cannot gate this snapshot. The persisting `gate`
        // auto-re-baselines here; the read-only verdict must not write, so it
        // reports the mismatch as an informational pass instead.
        Some(base) if base.metric_version != crate::metrics::METRIC_SEMANTICS_VERSION => {
            result.baseline_signal = base.aggregate_signal.map(|s| s as u32);
            result.message =
                "baseline recorded under different metric semantics — informational pass \
                 (re-save with `gate --save`)"
                    .to_string();
        }
        Some(base) if base.thresholds_hash.as_deref() != Some(metrics.thresholds_hash.as_str()) => {
            result.baseline_signal = base.aggregate_signal.map(|s| s as u32);
            result.message =
                "baseline thresholds differ — informational pass (re-save with `gate --save`)"
                    .to_string();
        }
        Some(base) => {
            result.baseline_signal = base.aggregate_signal.map(|s| s as u32);
            result.regressions = metric_regressions(&base, metrics);
            // FR-GV-16: the reason of the baseline compared against, as the
            // persisting gate reports it — never from one it cannot compare.
            result.baseline_reason = record.and_then(|r| r.reason);
            if review_table.is_some() {
                result.attribution = baseline_attribution(runtime, &base, *snapshot_id, metrics)?;
            }
            match (metrics.aggregate_signal, base.aggregate_signal, review_table.as_ref()) {
                (Some(current), Some(baseline_signal), Some(policy)) => {
                    // FR-GV-25, as `gate` judges it: the aggregate is a drift
                    // budget under `[review]`. Per-symbol entries need a branch
                    // delta, which a read-only verdict does not compute.
                    let drift = review::signal_drift(baseline_signal as u32, current, policy.drift_budget);
                    result.passed = !drift.exceeded;
                    result.message = review::drift_message(&drift);
                    result.drift = Some(drift);
                    result.notes.push(
                        "per-symbol `[review]` entries are judged by `gate` and `session_end`, not by this \
                         read-only verdict"
                            .to_string(),
                    );
                }
                (Some(current), Some(baseline_signal), None) => {
                    // BR-10: fail iff current < baseline − epsilon.
                    let regressed = f64::from(current) < baseline_signal as f64 - EPSILON;
                    result.passed = !regressed;
                    result.message = if regressed {
                        format!(
                            "signal regressed: {current} < baseline {baseline_signal} − ε ({EPSILON})"
                        )
                    } else {
                        format!("signal {current} holds the baseline {baseline_signal} (ε {EPSILON})")
                    };
                }
                _ => {
                    result.message = SIGNAL_OR_BASELINE_ABSENT.to_string();
                }
            }
        }
    }
    // FR-GV-27: the trend `gate` reports, over the snapshots up to this one —
    // a `scan` committing after it never enters. Read, never computed into
    // the store; it moves no verdict.
    if let Some(table) = &review_table {
        set_trend(&mut result, review::trend(runtime, table, Some(*snapshot_id))?);
    }

    Ok(result)
}

/// **One** read of the last persisted snapshot, projected into both fields of
/// the Health bundle ([FR-UI-04], [CR-135] §3.2).
///
/// # Why this exists rather than calling the two accessors
///
/// [`latest_gate`] and [`latest_scan`] each read the last persisted snapshot.
/// The Health handler called both, so one page load opened **two** reads of the
/// same logical row over an append-only ledger, in two transactions. A `scan`
/// committing between them left the verdict describing the older row and the
/// metric grid the newer one — reproduced during [S-406]'s review as a no-signal
/// callout beside a fully populated quality grid — and nothing in the payload
/// let a reader tell the two generations apart.
///
/// Taking the read **once** and projecting its value twice removes that window
/// instead of narrowing it: the two fields are projections of one value, so the
/// interleaving has nowhere left to happen. Wrapping the two reads in one
/// transaction would have narrowed it for this one caller and left the next
/// caller that pairs the two accessors free to reopen the bug.
///
/// This is the treatment `navigate::status` already gives the [FR-IX-12] LOC
/// roll-up's two keys, and the reasoning is recorded at that read for the same
/// reason it is recorded here.
///
/// A pure read: no compute, no reconcile, no persist ([ADR-28]).
///
/// [S-406]: ../../../docs/planning/journal.md#s-406-a-readout-names-a-step-that-can-change-what-it-reports
/// [FR-UI-04]: ../../../docs/specs/requirements/FR-UI-04.md
/// [FR-IX-12]: ../../../docs/specs/requirements/FR-IX-12.md
/// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
/// [CR-135]: ../../../docs/requests/CR-135-the-health-readout-is-internally-consistent-and-never-stale.md
pub(crate) fn latest_health(engine: &Engine) -> Result<LatestHealth> {
    // THE read. Everything below is a projection of this one value — do not add
    // a second `latest_metrics` call here, in either projection or at a caller
    // that wants both fields ([CR-135] §3.2).
    let snapshot = latest_metrics(engine)?;
    let gate = gate_from_snapshot(engine, snapshot.as_ref())?;
    // Offenders and the "where to start" ranking ride inside the same value, so
    // they describe the same row as the signal and the verdict (FR-QM-15,
    // FR-QM-17).
    let scan = scan_from_snapshot(engine, snapshot);
    Ok(LatestHealth { gate, scan })
}

/// Every offender the last snapshot recorded, narrowed by `filter`
/// ([FR-QM-19], S-676) — the engine's entry point to
/// [`offenders::offenders`], named here so the engine reaches the listing
/// through this module as it does [`latest_health`] and not through a second
/// one (its module fan-out is budgeted, `max_fan_out`).
///
/// [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
pub(crate) fn offender_listing(
    engine: &Engine,
    filter: &OffendersFilter,
) -> Result<crate::models::quality::OffenderListing> {
    offenders::offenders(engine, filter)
}

/// The branch delta against the merge-base of `HEAD` and `base` ([FR-GV-24],
/// S-677) — the engine's entry point to [`review::branch_review`], named here
/// so the engine reaches it through this module as it does
/// [`offender_listing`] and not through a second one (its module fan-out is
/// budgeted, `max_fan_out`).
///
/// [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
///
/// `security` reads the security evidence at the merge-base it is handed
/// ([FR-SC-18], S-686): the engine supplies it, so governance never opens
/// `history.db`.
///
/// [FR-SC-18]: ../../../docs/specs/requirements/FR-SC-18.md
pub(crate) fn branch_review(
    engine: &Engine,
    base: Option<&str>,
    security: impl FnOnce(&str) -> Result<SecurityEvidence>,
) -> Result<crate::models::quality::BranchReview> {
    review::branch_review(engine, base, security)
}

/// The summed branch deltas projected onto the shared limits ([FR-GV-28],
/// S-757) — the engine's entry point to [`review::branches_review`], named
/// here beside [`branch_review`] for the same fan-out reason.
///
/// [FR-GV-28]: ../../../docs/specs/requirements/FR-GV-28.md
pub(crate) fn branches_review(
    engine: &Engine,
    base: Option<&str>,
    branches: &[String],
) -> Result<crate::models::quality::BranchesProjection> {
    review::branches_review(engine, base, branches)
}

/// The room left under every projected budget on the base commit ([FR-GV-29],
/// S-798) — the engine's entry point to [`review::headroom::headroom_review`],
/// named here beside [`branches_review`] for the same fan-out reason.
///
/// [FR-GV-29]: ../../../docs/specs/requirements/FR-GV-29.md
pub(crate) fn headroom_review(
    engine: &Engine,
    base: Option<&str>,
    subjects: &[String],
    near: Option<u64>,
) -> Result<crate::models::quality::HeadroomReport> {
    review::headroom::headroom_review(engine, base, subjects, near)
}

/// What the branch security delta classifies ([FR-SC-18], S-686), read by the
/// engine and handed in: governance does not open `history.db`. Defined here,
/// beside [`branch_review`], so the engine names it through the module it
/// already calls.
///
/// [FR-SC-18]: ../../../docs/specs/requirements/FR-SC-18.md
pub(crate) struct SecurityEvidence {
    /// Every stored finding of each scanner's latest report — unfiltered,
    /// labelled against the ignore file, OWASP-classified and in graph context.
    pub current: crate::security::SecurityFindings,
    /// Each scanner's latest snapshot: its id and ingest time.
    pub latest: Vec<crate::history::ScannerSnapshot>,
    /// The reports recorded at the merge-base commit — never a scan of its tree.
    pub base: crate::history::SecurityView,
    /// Each merge-base finding's OWASP categories, in `base.findings` order.
    pub base_owasp: Vec<Vec<&'static str>>,
    /// Each merge-base finding's graph context, in `base.findings` order.
    pub base_contexts: Vec<crate::security::context::FindingContext>,
}

/// The **non-persisting** quality readout for the report tier ([FR-IN-07],
/// [CR-095]) — the signal an agent-host session-start hook shows.
///
/// Writes nothing, by construction, so a hook firing at every session boundary
/// (including every `/clear`) never takes the graph-DB write lock and never
/// appends to the [FR-GV-06] `evolution` series. That is the whole reason this
/// exists next to [`gate`]: `gate` is the [FR-GV-09] recording path ("every gate
/// writes a snapshot, saved or compared"), which is correct for `scan`/`gate` and
/// wrong for a readout that only wants to display.
///
/// Three deliberate consequences, each reported rather than hidden:
/// - **No reconcile.** Reconciling is a write, so `freshness` is always stamped
///   assumed-fresh ([FR-RC-04]).
/// - **Signal computed fresh.** The deterministic `compute` half of the snapshot
///   path is shared verbatim with [`gate`], so the two can never disagree.
/// - **Violations read from the persisted table**, not re-evaluated: [FR-GV-02]'s
///   evaluator re-materialises the whole derived policy graph on every run
///   (BR-12), which is a write. The table alone cannot say whether a run
///   happened — `check_rules` clears and rewrites it, so a clean check and no
///   check at all leave it identical — so the [FR-GV-21] marker is read
///   alongside it ([CR-096]). The marker resolves the ambiguity as a recorded
///   fact rather than a guess: its presence dates and attributes the findings,
///   and its absence means no run has happened, which is reported as such and
///   never as a pass. Stating a *recorded clean* check ([BR-41]) needs the
///   marker **and**, since [CR-140], the **evaluated set** it recorded: a `0`
///   over a contract that authored no rules — or over a marker written before
///   the evaluated set was kept — is a vacuous run, not a pass ([FR-GV-03]).
///   That is what [`EvaluatedSetAbsence::classify`] separates below.
///
/// [BR-41]: ../../../docs/specs/software-spec.md#4-cross-cutting-non-functional-requirements
/// [FR-GV-21]: ../../../docs/specs/requirements/FR-GV-21.md
/// [FR-GV-03]: ../../../docs/specs/requirements/FR-GV-03.md
/// [CR-096]: ../../../docs/requests/CR-096-recorded-check-marker.md
/// [CR-140]: ../../../docs/requests/CR-140-the-recorded-check-marker-carries-what-it-evaluated.md
///
/// `message_cap` bounds the returned message list; `violation_count` always
/// carries the true total so a truncated list can say what it dropped.
///
/// # Errors
/// Returns an error on a structural failure (transient engine, store fault,
/// invalid `rules.toml`) — the same contract as the other quality reads.
///
/// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
/// [FR-IN-07]: ../../../docs/specs/requirements/FR-IN-07.md
/// [FR-RC-04]: ../../../docs/specs/requirements/FR-RC-04.md
/// [CR-095]: ../../../docs/requests/CR-095-session-start-quality-readout.md
pub(crate) fn quality_readout(engine: &Engine, message_cap: usize) -> Result<QualityReadout> {
    // `false`: reconciling writes, and this path must not.
    let fresh = reconcile_step(engine, false)?;
    let runtime = quality_runtime(engine)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: score under the effective rules.toml thresholds, exactly as `gate`
    // does, so the readout's signal is the one the gate would report.
    let thresholds = effective_thresholds(&load_rules_cached(engine, None)?.rules);
    let metrics =
        crate::metrics::compute_snapshot(runtime, &view, thresholds, &member_access_extensions(engine))?;

    // FR-QM-18: the session-start readout names what the session-start gate does.
    let mut warnings = coverage_warnings(runtime)?;
    let baseline = runtime.submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?;
    // Only a *comparable* baseline yields a delta. An incomparable one (metric
    // semantics or thresholds changed since it was saved, FR-GV-10) is reported
    // with its signal but no delta, and says why — subtracting across a
    // semantics change would produce a confident, meaningless number.
    let (baseline_signal, comparable) = match &baseline {
        None => (None, false),
        Some(base) if base.metric_version != crate::metrics::METRIC_SEMANTICS_VERSION => {
            warnings.push(
                "baseline recorded under different metric semantics — delta omitted \
                 (re-save with `gate --save`)"
                    .to_string(),
            );
            (base.aggregate_signal.map(|s| s as u32), false)
        }
        Some(base) if base.thresholds_hash.as_deref() != Some(metrics.thresholds_hash.as_str()) => {
            warnings.push(
                "baseline thresholds differ — delta omitted (re-save with `gate --save`)"
                    .to_string(),
            );
            (base.aggregate_signal.map(|s| s as u32), false)
        }
        Some(base) => (base.aggregate_signal.map(|s| s as u32), true),
    };
    let delta = match (comparable, metrics.aggregate_signal, baseline_signal) {
        (true, Some(current), Some(base)) => Some(i64::from(current) - i64::from(base)),
        _ => None,
    };

    // The persisted findings of the last `check_rules` run, and — since
    // [CR-096] — the marker recording that the run happened at all. The table
    // alone is irreducibly ambiguous (`check_rules` clears and rewrites it, so
    // a clean run and no run leave it identical); the marker is the only place
    // that distinction lives, which is why both are read here.
    //
    // `counts()` rides along as a third query in the *same* pooled read rather
    // than a second `submit_read`, so the discriminant below costs no extra
    // round-trip through the read pool. The rest of the cost, stated rather
    // than hidden ([CR-138] AC3), because the report tier fires at every
    // session boundary and pays it on every one:
    //
    // - Five `COUNT(*)` aggregates returned as one row. They materialise no
    //   rows, but each is an index *walk*, linear in table size — not the
    //   constant-time lookup "indexed aggregate" might suggest.
    // - It is the **second** execution of this query per readout.
    //   `reconcile_step` above already called `counts()` (see `Freshness`),
    //   and keeps only the unresolved-ref delta from it. Threading the whole
    //   `StoreCounts` out of there would make this read genuinely free, but
    //   `reconcile_step` is shared with the write-side paths that another
    //   story owns this iteration, so the duplication is recorded here rather
    //   than removed under a review fix. Measured at well under 1 ms on a
    //   13k-node store, and dwarfed by the `hydrate` + `all_nodes` reads this
    //   same function already makes.
    //
    // It is also the *same* query `logos status` derives its counts from, so
    // the two surfaces can never disagree about the **figure** — that
    // disagreement is what [CR-138] reproduced. They still answer different
    // *questions* about "indexed" (`status` asks `files > 0 || nodes > 0`,
    // this asks `files > 0 && nodes > 0`), which `SignalAbsence`'s own doc
    // sets out; agreeing on the number is not agreeing on the predicate.
    let (rows, marker, counts) = runtime
        .submit_read(|store| Ok((store.violations()?, store.check_run()?, store.counts()?)))?;
    let row_count = rows.len();
    let (violations, violation_count) = if rows.is_empty() {
        (None, None)
    } else {
        let messages = rows
            .iter()
            .take(message_cap)
            .map(|row| row.message.clone())
            .collect();
        (Some(messages), Some(row_count))
    };

    // Two sources, deliberately not equally authoritative (see `CheckRun`): the
    // marker when one exists, otherwise the rows' own `created_at`, which a
    // store written before the marker migration still carries. The fallback is
    // what lets such a store date its existing findings immediately, with no
    // re-index and nothing invented.
    let now = unix_now();
    let check = match &marker {
        Some(row) => {
            // The denominator and the reason it is missing, from ONE call, so a
            // recorded rule count and a cause for its absence can never both be
            // present ([CR-140] §3.2) — the treatment `signal` /
            // `signal_absence` already gets two fields up.
            let (checked_rules, evaluated_absence) =
                EvaluatedSetAbsence::classify(row.checked_rules, row.rules_present);
            Some(CheckRun {
                ran_at: row.ran_at,
                // Saturating, never raw: a corrupted or hand-edited `ran_at`
                // would otherwise overflow and PANIC in a debug build — a crash
                // on read, in the one tier whose whole contract is that it
                // reports and never blocks ([FR-GV-05]). An implausible result
                // is named by the rendering rather than crashed on.
                age_seconds: now.saturating_sub(row.ran_at),
                commit_sha: row.commit_sha.clone(),
                head_sha: fresh.head.clone(),
                // An unresolvable HEAD on either side is never *treated* as a
                // moved tree — the comparison is omitted, never guessed
                // ([NFR-RA-05]), mirroring the coverage artifact's staleness
                // rule.
                tree_moved: match (&row.commit_sha, &fresh.head) {
                    (Some(recorded), Some(head)) => recorded != head,
                    _ => false,
                },
                recorded_count: Some(row.violation_count),
                checked_rules,
                evaluated_absence,
            })
        }
        // Every row of one run carries that run's time, so any of them dates
        // it; the first is taken for determinism. No marker means no recorded
        // HEAD and no recorded total, so neither a tree comparison nor a clean
        // assertion can be drawn from this — and `recorded_count: None` is what
        // says so.
        None => rows.first().map(|row| CheckRun {
            ran_at: row.created_at,
            age_seconds: now.saturating_sub(row.created_at),
            commit_sha: None,
            head_sha: fresh.head.clone(),
            tree_moved: false,
            recorded_count: None,
            // No marker at all, so nothing recorded an evaluated set either —
            // the same *unknown* a pre-migration-21 marker carries, and for the
            // same reason ([CR-140] CRA-05). Rendering it as zero would be the
            // favourable reading of an absent fact this story removes.
            checked_rules: None,
            evaluated_absence: Some(EvaluatedSetAbsence::Unrecorded),
        }),
    };

    // One transaction writes the marker and the rows, so these can only
    // disagree on a store that has been edited underneath Logos or has lost a
    // write. Report it rather than resolving it: silently preferring either
    // figure would let a marker claiming a clean run over a table holding
    // findings render as "clean" — the precise fabrication [BR-41] forbids.
    if let Some(recorded) = marker.as_ref().map(|m| m.violation_count) {
        if recorded != row_count as i64 {
            warnings.push(format!(
                "the check-run marker records {recorded} violation(s) but {row_count} row(s) are \
                 stored — the store disagrees with itself; re-run `logos check`"
            ));
        }
    }

    Ok(QualityReadout {
        signal: metrics.aggregate_signal,
        // Derived from the snapshot that just withheld the signal, never from a
        // second opinion about what "empty" means — see `SignalAbsence::classify`.
        // The two fields are set from one expression pair here so a signal and a
        // cause for its absence can never both be present ([CR-138]).
        signal_absence: SignalAbsence::classify(&metrics, counts.nodes, counts.files),
        baseline_signal,
        delta,
        freshness: fresh.line(),
        violations,
        // The drop-out of the very snapshot the signal came from (CR-156), never
        // re-derived from a second reading of the edge count.
        modularity_not_applicable: metrics.modularity_not_applicable.clone(),
        acyclicity_not_applicable: metrics.acyclicity_not_applicable.clone(),
        depth_not_applicable: metrics.depth_not_applicable.clone(),
        cohesion_not_applicable: metrics.cohesion_not_applicable.clone(),
        violation_count,
        check,
        warnings,
    })
}

/// Classify an evaluated violation set against the checked-in blessed set
/// ([FR-GV-23]) — **read-only**: `check`, `scan` and the bless command's own
/// verdict all come through here, and none of them writes the file here.
///
/// With no constraint under ratchet the file is not even read, so a contract
/// that never opted in cannot be broken by a malformed blessed set.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
fn classify_against_blessed(
    engine: &Engine,
    compiled: &CompiledRules,
    violations: Vec<Violation>,
) -> Result<(Vec<Violation>, Option<RatchetReport>)> {
    let rules = compiled.rules.constraints.ratchet_rules();
    if rules.is_empty() {
        return Ok((violations, None));
    }
    let blessed = ratchet::BlessedSet::read(&ratchet::blessed_path(engine.root()))?;
    Ok(ratchet::classify(violations, &rules, blessed.as_ref()))
}

/// `check_rules` — the compliance report ([FR-GV-02]); no metric snapshot
/// (that is scan/gate's job, [FR-GV-09]). Never writes the blessed set
/// ([FR-GV-23]).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
pub(crate) fn check_rules(
    engine: &Engine,
    rules_path: Option<&Path>,
    reconcile: bool,
    evidence: &dyn security::SecurityEvidence,
) -> Result<RulesReport> {
    check_rules_run(engine, rules_path, reconcile, false, evidence)
}

/// `check --bless` — record every current offender of a ratchet constraint
/// as blessed debt in `.logos/rules-baseline.json` ([FR-GV-23], [ADR-73]), then
/// report exactly as `check` would against the set just written. The only
/// writer of the blessed set ([BR-62]): sorted and deterministic, so a re-bless
/// of an unchanged tree is byte-identical and leaves the file untouched.
///
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
/// [ADR-73]: ../../../docs/specs/architecture/decisions/ADR-73.md
/// [BR-62]: ../../../docs/specs/software-spec.md#318-configuration
pub(crate) fn bless_rules(
    engine: &Engine,
    rules_path: Option<&Path>,
    reconcile: bool,
    evidence: &dyn security::SecurityEvidence,
) -> Result<RulesReport> {
    check_rules_run(engine, rules_path, reconcile, true, evidence)
}

/// The shared body of [`check_rules`] and [`bless_rules`]: `bless` is the one
/// switch that writes the blessed set, before classification reads it.
/// `evidence` is read only when the contract carries a `[security]` table
/// ([FR-SC-10]).
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
fn check_rules_run(
    engine: &Engine,
    rules_path: Option<&Path>,
    reconcile: bool,
    bless: bool,
    evidence: &dyn security::SecurityEvidence,
) -> Result<RulesReport> {
    let mut fresh = reconcile_step(engine, reconcile)?;
    let compiled = load_rules_cached(engine, rules_path)?;
    let runtime = quality_runtime(engine)?;

    let (violations, checked_rules) = materialise_and_evaluate(engine, &compiled, None)?;
    let mut notes = std::mem::take(&mut fresh.notes);
    let rules = compiled.rules.constraints.ratchet_rules();
    let written = if bless && !rules.is_empty() {
        let path = ratchet::blessed_path(engine.root());
        let current = ratchet::BlessedSet::from_violations(&violations, &rules);
        let count = current.len();
        // Only the evaluated rules' entries are replaced: another contract's
        // blessed debt in the same file survives (`--rules FILE`). A malformed
        // existing file is rewritten rather than blocking the one command its
        // own error tells the user to run.
        let existing = ratchet::BlessedSet::read(&path).ok().flatten().unwrap_or_default();
        existing.reblessed(current, &rules).write(&path)?;
        Some(u32::try_from(count).unwrap_or(u32::MAX))
    } else {
        if bless {
            // Nothing is under ratchet, so there is no debt to bless — and no
            // reason to create a file the contract never reads.
            notes.push(format!(
                "no constraint declares `ratchet = true`: nothing to bless, {} left as it is",
                ratchet::BLESSED_FILE_REL
            ));
        }
        None
    };
    // FR-GV-23: blessed debt is a warning, new or worsened debt an error —
    // persisted as classified, so the recorded run matches the report.
    let (mut violations, mut ratchet_report) =
        classify_against_blessed(engine, &compiled, violations)?;
    // FR-SC-10: the `[security]` table's verdict, over the evidence the engine
    // injects — appended before persistence, so the recorded run carries it. No
    // table, no read: the report is byte-identical whatever evidence exists.
    let mut checked_rules = checked_rules;
    let mut missing_scans = Vec::new();
    if let Some(table) = compiled.rules.security.as_ref() {
        let verdict = security::check_security(table, &evidence.summary()?);
        violations.extend(verdict.violations);
        checked_rules += verdict.checked;
        notes.extend(verdict.notes);
        missing_scans = verdict.missing;
    }
    if let Some(report) = ratchet_report.as_mut() {
        report.written = written;
        if !report.blessed_file_present {
            notes.push(format!(
                "no blessed set at {}: every ratchet offender is an error — run `{}` to \
                 accept the current debt",
                ratchet::BLESSED_FILE_REL,
                ratchet::BLESS_COMMAND
            ));
        }
    }
    // Honest "is a contract authored?" signal (NFR-CC-04): the empty contract
    // that a missing default file compiles to carries the `ABSENT_RULES_HASH`
    // sentinel; any loaded file hashes to its content. Computed here rather
    // than at the report, because since S-437 the marker carries it too and
    // both must be the same fact about the same run.
    let rules_present = compiled.hash != ABSENT_RULES_HASH;
    // FR-GV-21: the marker records HEAD at write time, which is the HEAD the
    // freshness step already resolved for this run — and, since S-437, the
    // evaluated set the run scored against (CR-140 §3.1).
    let ran_at = persist_violations(
        runtime,
        None,
        &violations,
        fresh.head.as_deref(),
        EvaluatedSet {
            checked_rules,
            rules_present,
            operation: CHECK_RUN_OP_CHECK,
        },
    )?;

    // CR-052 / FR-GV-18: fold the fast structural-integrity verdict in as an
    // error-severity finding, so a drifted graph fails `check_rules` (exit 1)
    // independent of any rules.toml contract — the always-on structural guard.
    // Appended AFTER persistence: it is a live invariant check, not a
    // rules.toml violation, so it never enters the `violations` table (keeping
    // FR-GV-02 idempotence and the persisted-violations read model clean). It is
    // itself deterministic, so re-running still yields identical `violations`.
    for fault in structural_check(engine)?.faults() {
        violations.push(Violation {
            rule: "graph-structural-integrity".to_string(),
            rule_type: "constraint".to_string(),
            severity: SEVERITY_ERROR.to_string(),
            file: String::new(),
            node_id: None,
            message: fault,
            ratchet: None,
        });
    }
    // S-215 / FR-GV-20: the admission-tripwire analog, folded in the same way —
    // a distinct rule id so the two invariant classes (structural vs
    // admission) can be told apart in the read-model.
    for fault in admission_tripwire(engine)?.faults() {
        violations.push(Violation {
            rule: "graph-admission-drift".to_string(),
            rule_type: "constraint".to_string(),
            severity: SEVERITY_ERROR.to_string(),
            file: String::new(),
            node_id: None,
            message: fault,
            ratchet: None,
        });
    }

    let has_error = violations.iter().any(|v| v.severity == SEVERITY_ERROR);
    // FR-GV-22 / NFR-CC-04: a verdict over an empty evaluated set is not a
    // verdict. `rules_present` alone would misreport "nothing was evaluated"
    // when an always-on fold-in (FR-GV-18 structural / FR-GV-20 admission)
    // fired anyway, so a real violation still yields `Some(false)` even with
    // no contract loaded — only the truly empty case (no contract, nothing
    // fired) reports `None`.
    let passed = if rules_present || has_error {
        // FR-SC-10: a run with a required scan missing has no passing verdict.
        Some(!has_error && missing_scans.is_empty())
    } else {
        None
    };

    Ok(RulesReport {
        passed,
        checked_rules,
        rules_present,
        violations,
        ran_at: Some(ran_at),
        freshness: fresh.line(),
        warnings: fresh.warnings,
        notes,
        ratchet: ratchet_report,
        missing_scans,
    })
}

/// `gate` / `session_end` — fresh snapshot, baseline comparison within
/// epsilon ([FR-GV-04], [FR-GV-05], BR-10); `save` upserts the baseline
/// instead of comparing.
///
/// Under a `rules.toml [security]` table ([FR-SC-10]) the security verdict is
/// folded in after the metric one: `save` also records every unsuppressed
/// stored finding as the ratchet's baseline, and a compare fails on a new
/// finding or a cap. Without the table `evidence` is never called.
///
/// Under a `rules.toml [review]` table ([FR-GV-25]) a compare judges the
/// branch delta against the merge-base of `HEAD` and the default branch: a
/// gating entry fails it, and the aggregate fails only past `drift_budget`.
/// Under the table, saved or compared, a distributional dimension in decline
/// adds a `trend` entry and its warning ([FR-GV-27]) and moves no verdict. Without the table
/// nothing of it runs.
///
/// `save` records `reason` and the attribution of the change it blesses with
/// the baseline ([FR-GV-16], [FR-GV-26]); under the table, a drop past a
/// numeric `drift_budget` with no `reason` is a usage error (exit 2) and moves
/// no baseline.
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
/// [FR-GV-27]: ../../../docs/specs/requirements/FR-GV-27.md
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
pub(crate) fn gate(
    engine: &Engine,
    threshold: Option<u32>,
    save: bool,
    reconcile: bool,
    reason: Option<&str>,
    evidence: &dyn security::SecurityEvidence,
) -> Result<GateResult> {
    // The evidence is read before anything is written: an invalid ignore file
    // (exit 2) must leave the metric baseline as it was.
    let compiled = load_rules_cached(engine, None)?;
    let security = security_evidence(&compiled, evidence)?;
    // FR-GV-25: under a `[review]` table a compare also judges the branch
    // delta, on the graph the metric half just reconciled.
    let table = compiled.rules.review.as_ref();
    let policy = table.filter(|_| !save);
    let bless = save.then(|| Bless {
        reason: reason.map(str::trim).filter(|r| !r.is_empty()),
        policy: table,
    });
    let mut result = gate_metrics(engine, threshold, bless, reconcile, policy)?;
    if policy.is_some() {
        review::fold_into_gate(&mut result, review::branch_delta(engine, None)?);
    }
    if let Some((table, summary)) = security {
        fold_security_into_gate(&mut result, table, &summary, save, evidence)?;
    }
    // FR-GV-27: a warning beside the verdict, over the series including the
    // snapshot this gate recorded; it never moves `passed`.
    if let Some(table) = table {
        set_trend(&mut result, review::trend(quality_runtime(engine)?, table, None)?);
    }
    Ok(result)
}

/// Put `trend` on `result` ([FR-GV-27]): the structured list, and each entry's
/// warning in `warnings` — one source for both, on every gate surface.
///
/// [FR-GV-27]: ../../../docs/specs/requirements/FR-GV-27.md
fn set_trend(result: &mut GateResult, trend: Vec<crate::models::DimensionTrend>) {
    result.warnings.extend(trend.iter().map(crate::models::DimensionTrend::warning));
    result.trend = trend;
}

/// What `gate --save` blesses under ([FR-GV-16]): the `--reason` given, and
/// the `[review]` table whose `drift_budget` decides whether one is required.
///
/// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
struct Bless<'a> {
    reason: Option<&'a str>,
    policy: Option<&'a crate::config::ReviewRules>,
}

/// The attribution of the change from the `baseline` snapshot to the one
/// persisted as `current_id` ([FR-GV-26]): every dimension of the baseline's
/// row, and both snapshots' uncapped offenders, each read by its own id, as
/// its drivers. `None` when the baseline row is gone.
///
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
fn baseline_attribution(
    runtime: &Runtime,
    baseline: &MetricSnapshotRow,
    current_id: i64,
    current: &MetricSnapshot,
) -> Result<Option<crate::models::quality::SignalAttribution>> {
    let base_id = baseline.id;
    let (row, before, after) = runtime.submit_read(move |store| {
        Ok((
            store.metric_snapshot(base_id)?,
            store.uncapped_offenders_of(base_id)?,
            store.uncapped_offenders_of(current_id)?,
        ))
    })?;
    Ok(row.map(|row| review::snapshot_attribution(&metric_snapshot_from_row(row), current, before.zip(after))))
}

/// `true` when `baseline` was scored under the semantics and thresholds
/// `current` was, so the two signals compare ([FR-GV-10]).
///
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
fn comparable(baseline: &MetricSnapshotRow, current: &MetricSnapshot) -> bool {
    incomparable_because(baseline, current).is_none()
}

/// The announced re-baseline `current` takes over `baseline` ([FR-GV-10],
/// [S-762]): what made the two incomparable, and both ends as the notice names
/// them — the metric-semantics version and the signal each was scored as. `None`
/// when the two signals compare.
///
/// The one definition of "incomparable" and of the notice: `gate`,
/// `session_end` and `session_start` each reset through it.
///
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
/// [S-762]: ../../../docs/planning/journal.md#s-762-the-re-baseline-notice-shows-what-it-replaced
fn baseline_reset(baseline: &MetricSnapshotRow, current: &MetricSnapshot) -> Option<BaselineReset> {
    let cause = incomparable_because(baseline, current)?;
    Some(BaselineReset {
        cause: cause.to_string(),
        from: BaselineStamp {
            metric_version: baseline.metric_version,
            signal: baseline.aggregate_signal.map(|s| s as u32),
        },
        to: BaselineStamp {
            metric_version: crate::metrics::METRIC_SEMANTICS_VERSION,
            signal: current.aggregate_signal,
        },
    })
}

/// What makes `baseline` incomparable to `current` ([FR-GV-10]), as the
/// announced re-baseline names it, or `None` when the two signals compare.
///
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
fn incomparable_because(baseline: &MetricSnapshotRow, current: &MetricSnapshot) -> Option<&'static str> {
    if baseline.metric_version != crate::metrics::METRIC_SEMANTICS_VERSION {
        Some("metric semantics changed")
    } else if baseline.thresholds_hash.as_deref() != Some(current.thresholds_hash.as_str()) {
        Some("metric thresholds changed")
    } else {
        None
    }
}

/// Save `snapshot_id` as the baseline for `gate --save` ([FR-GV-04]) with what
/// the re-bless records ([FR-GV-16]): the attribution of the change from the
/// baseline it replaces, when that one compares, and the reason. Under a
/// `[review]` table a drop past a numeric `drift_budget` requires a reason —
/// without one this returns a [`ConfigError`](crate::config::ConfigError)
/// (exit 2) and the baseline stays as it was.
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
/// [FR-GV-16]: ../../../docs/specs/requirements/FR-GV-16.md
fn save_baseline(
    runtime: &Runtime,
    snapshot_id: i64,
    metrics: &MetricSnapshot,
    bless: &Bless<'_>,
    result: &mut GateResult,
) -> Result<()> {
    let baseline = runtime
        .submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?
        .filter(|base| comparable(base, metrics));
    let attribution = match &baseline {
        Some(base) => baseline_attribution(runtime, base, snapshot_id, metrics)?,
        None => None,
    };
    let drift = match (&baseline, bless.policy) {
        (Some(base), Some(policy)) => base
            .aggregate_signal
            .zip(metrics.aggregate_signal)
            .map(|(before, now)| review::signal_drift(before as u32, now, policy.drift_budget)),
        _ => None,
    };
    if let (Some(drift), None) = (drift.filter(|d| d.exceeded), bless.reason) {
        return Err(crate::config::ConfigError::InvalidValue {
            key: "gate --reason".to_string(),
            message: format!(
                "{}: a re-bless past the budget must say why — re-run with --reason \"<why>\"",
                review::drift_message(&drift)
            ),
        }
        .into());
    }
    let record = crate::graph_store::BaselineRecord {
        reason: bless.reason.map(str::to_string),
        attribution: attribution.as_ref().map(serde_json::to_string).transpose()?,
    };
    let created_at = unix_now();
    runtime.submit_write(move |w| w.record_baseline(SCOPE_PROJECT, snapshot_id, created_at, &record))?;
    result.baseline_signal = metrics.aggregate_signal;
    result.message = format!("baseline saved (snapshot #{snapshot_id})");
    result.baseline_reason = bless.reason.map(str::to_string);
    // The output carries the attribution under the table only, so a gate
    // without one reads as it always has; the record keeps it either way.
    result.attribution = attribution.filter(|_| bless.policy.is_some());
    Ok(())
}

/// The `[security]` table and the evidence summary it is judged over, or `None`
/// without the table — in which case `evidence` is never called ([FR-SC-10]).
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
fn security_evidence<'a>(
    compiled: &'a CompiledRules,
    evidence: &dyn security::SecurityEvidence,
) -> Result<Option<(&'a crate::config::SecurityRules, security::SecuritySummary)>> {
    compiled
        .rules
        .security
        .as_ref()
        .map(|table| Ok((table, evidence.summary()?)))
        .transpose()
}

/// Record the ratchet's baseline for `gate --save` and a `session_start` that
/// finds none ([FR-SC-10]) — unless a required scan is `missing`, since a baseline recorded
/// over missing evidence would accept whatever that scan would have found.
/// Returns the message fragment either way.
///
/// With a checked-in `.logos/security-baseline.json` nothing is recorded: the
/// file is the ratchet's baseline, a set recorded in `history.db` would never
/// be read, and only `logos security bless` rewrites the file ([FR-SC-25]).
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
/// [FR-SC-25]: ../../../docs/specs/requirements/FR-SC-25.md
fn record_security_baseline(
    table: &crate::config::SecurityRules,
    summary: &security::SecuritySummary,
    missing: &[crate::models::quality::MissingScan],
    evidence: &dyn security::SecurityEvidence,
) -> Result<String> {
    let missing_note = (!missing.is_empty()).then(|| {
        let named: Vec<&str> = missing.iter().map(|m| m.scanner.as_str()).collect();
        format!("required scan missing or stale at HEAD: {}", named.join(", "))
    });
    if summary.baseline_source == security::BaselineSource::CheckedIn {
        let mut message = format!(
            "security baseline not saved: the checked-in {} is the ratchet's baseline — `{}` \
             rewrites it",
            security::CHECKED_IN_FILE_REL,
            security::BLESS_COMMAND
        );
        if let Some(note) = missing_note {
            message.push_str("; ");
            message.push_str(&note);
        }
        return Ok(message);
    }
    match missing_note {
        None => {
            let recorded = evidence.save_baseline(table, &security::baseline_fingerprints(summary))?;
            Ok(format!("security baseline saved ({recorded} finding fingerprints)"))
        }
        Some(note) => Ok(format!("security baseline not saved: {note}")),
    }
}

/// The security half of [`gate`] ([FR-SC-10]). A required scan missing is exit
/// 5 and no pass; on `save` it also leaves the ratchet's baseline as it was,
/// since a baseline recorded over missing evidence would accept whatever the
/// missing scan would have found. The metric baseline is saved regardless.
///
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
fn fold_security_into_gate(
    result: &mut GateResult,
    table: &crate::config::SecurityRules,
    summary: &security::SecuritySummary,
    save: bool,
    evidence: &dyn security::SecurityEvidence,
) -> Result<()> {
    let missing = security::missing_scans(table, summary);
    if save {
        append_detail(&mut result.message, record_security_baseline(table, summary, &missing, evidence)?);
    } else {
        let verdict = security::check_security(table, summary);
        if !verdict.violations.is_empty() {
            result.passed = false;
            append_detail(
                &mut result.message,
                format!("{} security violation(s)", verdict.violations.len()),
            );
        }
        result.security_violations = verdict.violations;
        result.notes.extend(verdict.notes);
    }
    if !missing.is_empty() {
        result.passed = false;
        if !save {
            let named: Vec<&str> = missing.iter().map(|m| m.scanner.as_str()).collect();
            append_detail(
                &mut result.message,
                format!("required scan missing or stale at HEAD: {}", named.join(", ")),
            );
        }
    }
    result.missing_scans = missing;
    Ok(())
}

/// `detail` appended to a verdict `message`, `; `-separated.
fn append_detail(message: &mut String, detail: String) {
    *message = if message.is_empty() { detail } else { format!("{message}; {detail}") };
}

/// The metric half of [`gate`]: everything it did before the `[security]`
/// table existed. Under a `[review]` `policy` the aggregate is judged by its
/// drift budget instead of `baseline − ε` ([FR-GV-25]), and its change is
/// attributed ([FR-GV-26]). `save` is `gate --save` ([`save_baseline`]).
///
/// [FR-GV-25]: ../../../docs/specs/requirements/FR-GV-25.md
/// [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
fn gate_metrics(
    engine: &Engine,
    threshold: Option<u32>,
    save: Option<Bless<'_>>,
    reconcile: bool,
    policy: Option<&crate::config::ReviewRules>,
) -> Result<GateResult> {
    let fresh = reconcile_step(engine, reconcile)?;
    let runtime = quality_runtime(engine)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: the gate scores under the effective rules.toml thresholds, so the
    // snapshot's persisted hash is what the baseline comparison gates on.
    let thresholds = effective_thresholds(&load_rules_cached(engine, None)?.rules);
    // FR-GV-09: every gate writes a snapshot, saved or compared — with the
    // offender lists it computed (FR-QM-15), which the verdict does not report.
    let crate::metrics::RecordedSnapshot {
        id: snapshot_id,
        metrics,
        ..
    } = crate::metrics::snapshot(
        runtime,
        &view,
        fresh.head.as_deref(),
        thresholds,
        &member_access_extensions(engine),
    )?;
    // FR-QM-18: advisory beside the verdict, never part of it.
    let freshness = fresh.line();
    let mut warnings = fresh.warnings;
    warnings.extend(coverage_warnings(runtime)?);

    let mut result = GateResult {
        passed: true,
        saved: save.is_some(),
        signal: metrics.aggregate_signal,
        baseline_signal: None,
        test_function_count: metrics.test_function_count,
        threshold,
        epsilon: EPSILON,
        regressions: Vec::new(),
        structural_faults: Vec::new(),
        freshness,
        message: String::new(),
        warnings,
        notes: fresh.notes,
        security_violations: Vec::new(),
        missing_scans: Vec::new(),
        gating_entries: Vec::new(),
        drift: None,
        attribution: None,
        baseline_reason: None,
        baseline_reset: None,
        trend: Vec::new(),
    };

    if let Some(bless) = save {
        save_baseline(runtime, snapshot_id, &metrics, &bless, &mut result)?;
        return Ok(result);
    }

    let (baseline, record) = runtime.submit_read(|store| {
        Ok((store.baseline_snapshot(SCOPE_PROJECT)?, store.baseline_record(SCOPE_PROJECT)?))
    })?;
    // FR-GV-10 / BR-25 (CR-005): a baseline recorded under a different
    // metrics-semantics version (e.g. the pre-CR-001 test-inclusive scope), or
    // under thresholds since tuned in rules.toml, is incomparable to this run.
    // A tuned threshold moves what several dimensions measure, so both take the
    // SAME announced path: auto-re-baseline against the fresh snapshot, pass
    // informationally — never a regression failure against an incomparable
    // anchor (UAT-GV-06) — and name both ends (S-762). The next gate finds a
    // matching version and hash and compares normally (UAT-QM-13 step 3).
    if let Some(reset) = baseline.as_ref().and_then(|base| baseline_reset(base, &metrics)) {
        let created_at = unix_now();
        runtime.submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, created_at))?;
        result.saved = true;
        result.baseline_signal = metrics.aggregate_signal;
        result.message = reset.notice();
        result.baseline_reset = Some(reset);
        return Ok(result);
    }
    match baseline {
        None => {
            // FR-GV-05: no baseline → informational pass. `gate --save` saves
            // one, and so does the next `session_start`, which saves a missing
            // gate baseline whatever the ratchet's source (S-763).
            result.message =
                "no baseline saved — informational pass (save one with `gate --save`)".to_string();
        }
        Some(base) => {
            result.baseline_signal = base.aggregate_signal.map(|s| s as u32);
            result.regressions = metric_regressions(&base, &metrics);
            // FR-GV-16: the reason the compared baseline was blessed with. An
            // auto-re-baseline above replaced it, and its reason with it.
            result.baseline_reason = record.and_then(|r| r.reason);
            // FR-GV-26: under `[review]` the change is attributed.
            if policy.is_some() {
                result.attribution = baseline_attribution(runtime, &base, snapshot_id, &metrics)?;
            }
            match (metrics.aggregate_signal, base.aggregate_signal, policy) {
                (Some(current), Some(baseline_signal), Some(policy)) => {
                    // FR-GV-25: the aggregate is a drift budget under `[review]`.
                    let drift = review::signal_drift(baseline_signal as u32, current, policy.drift_budget);
                    result.passed = !drift.exceeded;
                    result.message = review::drift_message(&drift);
                    result.drift = Some(drift);
                }
                (Some(current), Some(baseline_signal), None) => {
                    // BR-10: fail iff current < baseline − epsilon; per-metric
                    // movement is detail, never an independent failure.
                    let regressed = f64::from(current) < baseline_signal as f64 - EPSILON;
                    result.passed = !regressed;
                    result.message = if regressed {
                        format!(
                            "signal regressed: {current} < baseline {baseline_signal} − ε ({EPSILON})"
                        )
                    } else {
                        format!(
                            "signal {current} holds the baseline {baseline_signal} (ε {EPSILON})"
                        )
                    };
                }
                _ => {
                    // An "n/a" on either side cannot regress — honesty over
                    // a fabricated comparison (ADR-12 posture).
                    result.message = SIGNAL_OR_BASELINE_ABSENT.to_string();
                }
            }
        }
    }

    // The optional explicit floor also gates (kept from the S-016 CLI
    // contract: `gate --threshold`).
    if let Some(floor) = threshold {
        match metrics.aggregate_signal {
            Some(current) if current >= floor => {}
            Some(current) => {
                result.passed = false;
                result.message = format!(
                    "{}{}signal {current} is under the required threshold {floor}",
                    result.message,
                    if result.message.is_empty() { "" } else { "; " }
                );
            }
            None => {
                result.passed = false;
                // Names no cause, for the reason recorded on
                // `SIGNAL_OR_BASELINE_ABSENT`: `aggregate_signal == None` has two
                // causes and this condition separates neither ([CR-138],
                // `models::quality::absence` R1).
                result.message = format!(
                    "{}{}signal is n/a — cannot satisfy threshold {floor}",
                    result.message,
                    if result.message.is_empty() { "" } else { "; " }
                );
            }
        }
    }

    // CR-052 / FR-GV-05 / FR-GV-18: fold in the fast structural-integrity
    // verdict as a HARD failure, independent of the metric signal — a corrupted
    // graph blocks the session even when the signal holds the baseline. This is
    // the last word so no earlier informational-pass message can mask it. (The
    // `--save` and auto-re-baseline paths return earlier — establishing a
    // baseline is not a gate; the bare `gate` / `session_end` comparison always
    // reaches here.)
    // S-215 / FR-GV-20: the admission tripwire hard-fails alongside the
    // structural guard, folded into the SAME `structural_faults` bucket — both
    // are invariants of the persisted graph the gate must never silently pass,
    // independent of the metric signal.
    let structural = structural_check(engine)?;
    let admission = admission_tripwire(engine)?;
    let mut faults = structural.faults();
    faults.extend(admission.faults());
    if !faults.is_empty() {
        result.structural_faults = faults;
        result.passed = false;
        let detail = format!(
            "graph structural drift (NFR-RA-13/FR-GV-20): {}",
            result.structural_faults.join("; ")
        );
        result.message = if result.message.is_empty() {
            detail
        } else {
            format!("{}; {detail}", result.message)
        };
    }

    Ok(result)
}

/// Per-metric regressions vs the baseline ([FR-GV-05] detail): canonical
/// metric order, noise-floored.
fn metric_regressions(base: &MetricSnapshotRow, current: &MetricSnapshot) -> Vec<MetricRegression> {
    // CR-156, FR-QM-18: a graph dimension that is not applicable on either side
    // is outside the signal, so its movement is no regression of it (NFR-CC-04).
    let base_drop_outs = row_drop_outs(base);
    let current_drop_outs = [
        &current.modularity_not_applicable,
        &current.acyclicity_not_applicable,
        &current.depth_not_applicable,
    ];
    let applies = |metric: &str| {
        METRIC_NAMES[..3]
            .iter()
            .position(|name| *name == metric)
            .is_none_or(|i| base_drop_outs[i].is_none() && current_drop_outs[i].is_none())
    };
    let pairs = [
        (
            METRIC_NAMES[0],
            base.modularity_normalized,
            current.modularity.normalized,
        ),
        (
            METRIC_NAMES[1],
            base.acyclicity_normalized,
            current.acyclicity.normalized,
        ),
        (
            METRIC_NAMES[2],
            base.depth_normalized,
            current.depth.normalized,
        ),
        (
            METRIC_NAMES[3],
            base.equality_normalized,
            current.equality.normalized,
        ),
        (
            METRIC_NAMES[4],
            base.redundancy_normalized,
            current.redundancy.normalized,
        ),
    ];
    pairs
        .into_iter()
        .filter(|(metric, _, _)| applies(metric))
        .filter(|(_, baseline, current)| *current < *baseline - METRIC_NOISE)
        .map(|(metric, baseline, current)| MetricRegression {
            metric: metric.to_string(),
            baseline,
            current,
            delta: current - baseline,
        })
        .collect()
}

/// `session_start` — records the session's snapshot, and each missing
/// baseline `session_end` compares against ([FR-GV-04]).
///
/// Each baseline is decided on its own, before the edits. A missing gate
/// baseline is saved onto this snapshot. Under a `rules.toml [security]`
/// table, a missing ratchet baseline — none checked in and none in
/// `history.db` — is recorded as `gate --save` records it ([FR-SC-10]); the
/// message says when a missing required scan left it unrecorded. An existing
/// baseline is never overwritten ([CR-238]): an agent's session must not bless
/// its own regressions or findings, so only the guarded `gate --save` moves
/// them. The one exception is a gate baseline that no longer compares
/// ([FR-GV-10]): it is reset onto this snapshot with the notice `gate` gives —
/// kept, `session_end` would reset it onto the post-edit snapshot instead.
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
/// [FR-GV-10]: ../../../docs/specs/requirements/FR-GV-10.md
/// [FR-SC-10]: ../../../docs/specs/requirements/FR-SC-10.md
/// [CR-238]: ../../../docs/requests/CR-238-sprint-106-review-follow-ups.md
pub(crate) fn session_start(
    engine: &Engine,
    evidence: &dyn security::SecurityEvidence,
) -> Result<SessionInfo> {
    let fresh = reconcile_step(engine, true)?;
    let runtime = quality_runtime(engine)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: baseline under the effective rules.toml thresholds (its hash gates
    // the later session_end comparison, FR-GV-10).
    let compiled = load_rules_cached(engine, None)?;
    // Read before the snapshot and the baseline write, as `gate` does: an
    // invalid ignore file (exit 2) leaves the metric baseline as it was.
    let security = security_evidence(&compiled, evidence)?;
    let thresholds = effective_thresholds(&compiled.rules);
    // The baseline snapshot persists its offender lists too (FR-QM-15).
    let crate::metrics::RecordedSnapshot {
        id: snapshot_id,
        metrics,
        ..
    } = crate::metrics::snapshot(
        runtime,
        &view,
        fresh.head.as_deref(),
        thresholds,
        &member_access_extensions(engine),
    )?;

    let started_at = unix_now();
    let baseline = runtime.submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?;
    // FR-GV-10: an incomparable baseline is reset now, before the edits, with
    // the notice `gate` gives. Kept, `session_end` would reset it onto the
    // post-edit snapshot and bless what the session did.
    let reset = baseline.as_ref().and_then(|base| baseline_reset(base, &metrics));
    let mut message = match baseline {
        Some(base) if reset.is_none() => {
            format!("session started — snapshot #{snapshot_id} recorded; {}", kept_baseline(base.id))
        }
        _ => {
            runtime.submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, started_at))?;
            let saved = reset.as_ref().map_or("baseline saved".to_string(), BaselineReset::notice);
            format!(
                "session started — {saved} (snapshot #{snapshot_id}); \
                 `session_end` compares against it"
            )
        }
    };
    // The ratchet's baseline is its one source's ([FR-SC-25]): the checked-in
    // file when there is one, which this never writes, else `history.db`.
    if let Some((table, summary)) = security {
        message.push_str("; ");
        message.push_str(&match &summary.baseline {
            Some(fingerprints) => kept_security_baseline(fingerprints, summary.baseline_source),
            None => {
                let missing = security::missing_scans(table, &summary);
                record_security_baseline(table, &summary, &missing, evidence)?
            }
        });
    }

    Ok(SessionInfo {
        session_id: snapshot_id.to_string(),
        started_at,
        signal: metrics.aggregate_signal,
        freshness: fresh.line(),
        message,
        // FR-QM-18: advisory; session_start always records its snapshot.
        warnings: coverage_warnings(runtime)?,
        baseline_reset: reset,
    })
}

/// The gate-baseline fragment of a `session_start` that keeps the baseline
/// with id `kept` ([CR-238]).
///
/// [CR-238]: ../../../docs/requests/CR-238-sprint-106-review-follow-ups.md
fn kept_baseline(kept: i64) -> String {
    format!(
        "baseline kept (snapshot #{kept}), `session_end` compares against it — only \
         `gate --save` moves it"
    )
}

/// The ratchet fragment of a `session_start` that keeps the baseline holding
/// `fingerprints`, read from `source` ([CR-238]).
///
/// [CR-238]: ../../../docs/requests/CR-238-sprint-106-review-follow-ups.md
fn kept_security_baseline(fingerprints: &BTreeSet<String>, source: security::BaselineSource) -> String {
    match source {
        security::BaselineSource::CheckedIn => format!(
            "security baseline kept ({} finding fingerprints, checked in at {})",
            fingerprints.len(),
            security::CHECKED_IN_FILE_REL
        ),
        security::BaselineSource::Store => {
            format!("security baseline kept ({} finding fingerprints)", fingerprints.len())
        }
    }
}

/// `evolution` — the windowed snapshot series with per-metric deltas
/// ([FR-GV-06]). Reports history; not an aggregate run (BR-03), so no
/// reconcile and no freshness stamp.
///
/// [FR-GV-06]: ../../../docs/specs/requirements/FR-GV-06.md
pub(crate) fn evolution(engine: &Engine, limit: Option<u32>) -> Result<EvolutionReport> {
    let limit = limit.unwrap_or(DEFAULT_EVOLUTION_LIMIT).max(1);
    let rows = quality_runtime(engine)?.submit_read(|store| store.metric_snapshots())?;

    let mut warnings = Vec::new();
    if rows.is_empty() {
        warnings.push("no snapshots recorded yet — run `scan` or `gate` first".to_string());
    }

    // Deltas are computed over the FULL series (so the first point in the
    // window still shows its movement), then the window is cut.
    let points: Vec<EvolutionPoint> = rows
        .iter()
        .enumerate()
        .map(|(i, row)| evolution_point(row, i.checked_sub(1).map(|p| &rows[p])))
        .collect();
    let start = points.len().saturating_sub(limit as usize);

    Ok(EvolutionReport {
        limit,
        snapshots: points[start..].to_vec(),
        warnings,
    })
}

/// Map one snapshot row (and its predecessor) to an evolution point.
fn evolution_point(row: &MetricSnapshotRow, prev: Option<&MetricSnapshotRow>) -> EvolutionPoint {
    let normalized = |r: &MetricSnapshotRow| {
        [
            r.modularity_normalized,
            r.acyclicity_normalized,
            r.depth_normalized,
            r.equality_normalized,
            r.redundancy_normalized,
        ]
    };
    let current = normalized(row);
    let previous = prev.map(normalized);
    // CR-156, FR-QM-18: a not-applicable graph dimension is reported with its
    // reason, and no delta is reported across a point where it was outside the
    // signal. The drop-outs index the first three METRIC_NAMES; Equality and
    // Redundancy, past them, always apply.
    let drop_outs = row_drop_outs(row);
    let previous_drop_outs = prev.map(row_drop_outs);
    let applies = |drop_outs: &[Option<GraphDimensionNotApplicable>; 3], i: usize| {
        drop_outs.get(i).is_none_or(Option::is_none)
    };

    EvolutionPoint {
        snapshot_id: row.id,
        created_at: row.created_at,
        commit_sha: row.commit_sha.clone(),
        signal: row.aggregate_signal.map(|s| s as u32),
        signal_delta: match (row.aggregate_signal, prev.and_then(|p| p.aggregate_signal)) {
            (Some(cur), Some(prev_sig)) => Some(cur - prev_sig),
            _ => None,
        },
        metric_deltas: METRIC_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| MetricDelta {
                metric: (*name).to_string(),
                normalized: current[i],
                delta: previous
                    .filter(|_| {
                        applies(&drop_outs, i)
                            && previous_drop_outs.as_ref().is_none_or(|p| applies(p, i))
                    })
                    .map(|p| current[i] - p[i]),
                not_applicable: drop_outs.get(i).cloned().flatten(),
            })
            .collect(),
    }
}

/// The `dsm` granularity argument ([FR-GV-07]): module rollup by default,
/// `file` on request.
///
/// [FR-GV-07]: ../../../docs/specs/requirements/FR-GV-07.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DsmGranularity {
    /// Module rollup (the default).
    #[default]
    Module,
    /// File rollup (`--granularity file`).
    File,
}

impl FromStr for DsmGranularity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "module" => Ok(Self::Module),
            "file" => Ok(Self::File),
            other => Err(format!(
                "unknown dsm granularity {other:?} (one of: module, file)"
            )),
        }
    }
}

/// `dsm` — the dependency structure matrix ([FR-GV-07]): cell `(i, j)`
/// counts dep edges `i → j` — never a dispatch edge, which reaches an
/// implementation through its trait ([FR-QM-20]) — rows ordered by layer order then name (so
/// forward deps sit below the diagonal). At module granularity a Go package —
/// a directory plus its `package` clause — is one row, not one row per file
/// ([`go_dsm`]); every other language keeps the module rollup's rows.
///
/// [FR-GV-07]: ../../../docs/specs/requirements/FR-GV-07.md
/// [FR-QM-20]: ../../../docs/specs/requirements/FR-QM-20.md
pub(crate) fn dsm(
    engine: &Engine,
    granularity: Option<DsmGranularity>,
    reconcile: bool,
) -> Result<DsmReport> {
    let fresh = reconcile_step(engine, reconcile)?;
    let granularity = granularity.unwrap_or_default();
    let compiled = load_rules_cached(engine, None)?;

    let view = engine.hydrate(match granularity {
        DsmGranularity::Module => Granularity::Module,
        DsmGranularity::File => Granularity::File,
    })?;
    let graph = view.graph();

    // One row per aggregate vertex, with its layer when the key is
    // file-backed (a module rollup key can fall back to `file:<path>`). A Go
    // file module is not a Go module: its vertices roll up again, one row per
    // directory plus `package` clause ([S-527]). Every other vertex is its own
    // row, so a project without Go files is untouched.
    //
    // [S-527]: ../../../docs/planning/journal.md#s-527-the-go-dsm-lists-packages
    let go_packages = match granularity {
        DsmGranularity::Module => go_dsm::package_rows(
            engine,
            graph.node_weights().map(|vertex| vertex.key.as_str()),
        )?,
        DsmGranularity::File => go_dsm::GoPackages::default(),
    };
    let mut row_of_name: HashMap<&str, usize> = HashMap::new();
    let mut row_of_vertex: Vec<usize> = Vec::with_capacity(graph.node_count());
    let mut rows_unsorted: Vec<DsmRow> = Vec::new();
    for idx in graph.node_indices() {
        let key = graph[idx].key.as_str();
        let name = go_packages.row_of.get(key).map_or(key, String::as_str);
        let row = *row_of_name.entry(name).or_insert_with(|| {
            let layer = dsm_key_path(name, granularity)
                .and_then(|path| compiled.layer_of(path))
                .map(|(layer, _)| layer.to_string());
            rows_unsorted.push(DsmRow {
                name: name.to_string(),
                layer,
            });
            rows_unsorted.len() - 1
        });
        row_of_vertex.push(row);
    }
    let layer_order = |i: usize| -> u32 {
        rows_unsorted[i]
            .layer
            .as_deref()
            .and_then(|name| {
                compiled
                    .rules
                    .layers
                    .iter()
                    .find(|l| l.name == name)
                    .map(|l| l.order)
            })
            .unwrap_or(u32::MAX) // unassigned rows sort after every layer
    };
    let mut order: Vec<usize> = (0..rows_unsorted.len()).collect();
    order.sort_by(|&a, &b| {
        layer_order(a)
            .cmp(&layer_order(b))
            .then_with(|| rows_unsorted[a].name.cmp(&rows_unsorted[b].name))
    });
    let position: HashMap<usize, usize> = order
        .iter()
        .enumerate()
        .map(|(pos, &original)| (original, pos))
        .collect();

    let n = order.len();
    let mut matrix = vec![vec![0u32; n]; n];
    for edge in graph.edge_references() {
        let (src, dst) = (
            position[&row_of_vertex[edge.source().index()]],
            position[&row_of_vertex[edge.target().index()]],
        );
        // Two files of one package depend on each other inside the package;
        // that is not a coupling between rows (the rollup drops the same
        // self-loop one level down). A pair reached only through a trait is
        // no dependency either (FR-QM-20).
        if src != dst && !edge.weight().is_dispatch() {
            matrix[src][dst] += edge.weight().weight;
        }
    }

    let mut report = DsmReport {
        granularity: match granularity {
            DsmGranularity::Module => "module".to_string(),
            DsmGranularity::File => "file".to_string(),
        },
        rows: order
            .into_iter()
            .map(|i| rows_unsorted[i].clone())
            .collect(),
        matrix,
        freshness: fresh.line(),
        warnings: fresh.warnings,
        notes: fresh.notes,
    };
    if go_packages.directory_only > 0 {
        report.notes.push(format!(
            "{} Go file(s) had no readable `package` clause and are grouped by directory alone; \
             their package may show as a second row",
            go_packages.directory_only
        ));
    }
    Ok(report)
}

/// The file path behind a DSM vertex key, when one exists: a file-view key
/// IS a path; a module-view key is layer-mappable only through its
/// `file:<path>` fallback form. Pure module keys (and the `<unbound>`
/// sentinel) have no path → unassigned.
fn dsm_key_path(key: &str, granularity: DsmGranularity) -> Option<&str> {
    match granularity {
        DsmGranularity::File => (key != "<unbound>").then_some(key),
        DsmGranularity::Module => key.strip_prefix("file:"),
    }
}

/// `doc_gaps` ([FR-GV-14]): exported Function/Method symbols referenced by no
/// `DocSection` over `DocReference` edges, with the mandatory honesty caveat.
///
/// The scope is the public API — the `exported` flag is read from the persisted
/// annotation rows ([FR-AN-05], the SAME source `[[require_tested]]` reads), and
/// the documented set is built by [`documented_set`], the same core the
/// `[[require_documented]]` contract reuses ([FR-GV-15]). Deterministically
/// ordered: `annotations` arrive id-ordered and gaps sort by file then name
/// ([NFR-RA-06]).
///
/// [FR-GV-14]: ../../../docs/specs/requirements/FR-GV-14.md
/// [FR-GV-15]: ../../../docs/specs/requirements/FR-GV-15.md
/// [FR-AN-05]: ../../../docs/specs/requirements/FR-AN-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(crate) fn doc_gaps(
    engine: &Engine,
    limit: Option<u32>,
    reconcile: bool,
) -> Result<DocGapsReport> {
    let fresh = reconcile_step(engine, reconcile)?;
    let runtime = quality_runtime(engine)?;
    let limit = limit.unwrap_or(DEFAULT_DOC_GAPS_LIMIT).max(1);

    let (nodes, edges, annotations) = runtime.submit_read(|store| {
        Ok((
            store.all_nodes()?,
            store.all_edges()?,
            // The annotation rows carry the `exported` flag the FR-GV-14 scope
            // needs (NodeRow omits visibility) — the SAME seam `[[require_tested]]`
            // and `[[require_documented]]` read ([FR-AN-05]).
            store.annotation_nodes()?,
        ))
    })?;

    // The set of nodes referenced by some DocSection — shared with the
    // `[[require_documented]]` contract ([FR-GV-15]), so the report and the gate
    // can never disagree about what is documented.
    let documented = documented_set(&nodes, &edges);
    // Declaration start lines are not on the annotation projection; index the
    // id-ordered node rows so each gap can carry its line ([FR-GV-14]).
    let line_of: HashMap<NodeId, Option<i64>> =
        nodes.iter().map(|n| (n.id, n.start_line)).collect();

    // Gaps = exported Function/Method symbols no DocSection references. Iterating
    // the id-ordered annotation rows mirrors the contract's scope exactly.
    let mut total = 0u64;
    let mut gaps: Vec<DocGap> = Vec::new();
    for ann in &annotations {
        if ann.derived
            || !ann.exported
            || !matches!(ann.kind, NodeKind::Function | NodeKind::Method)
        {
            continue;
        }
        total += 1;
        if !documented.contains(&ann.id) {
            gaps.push(DocGap {
                name: ann.name.clone(),
                file: ann.file_path.clone().unwrap_or_default(),
                line: line_of.get(&ann.id).copied().flatten(),
            });
        }
    }
    gaps.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.name.cmp(&b.name)));

    let documented_functions = total - gaps.len() as u64;
    let truncated = gaps.len() > limit as usize;
    gaps.truncate(limit as usize);

    Ok(DocGapsReport {
        undocumented: gaps,
        total_functions: total,
        documented_functions,
        // Integer signal posture (ADR-08/AR-03); n/a when nothing to document.
        documentation_ratio: (total > 0)
            .then(|| ((documented_functions as f64 / total as f64) * 10_000.0).round() as u32),
        limit,
        truncated,
        caveat: DOC_GAPS_CAVEAT.to_string(),
        freshness: fresh.line(),
        warnings: fresh.warnings,
        notes: fresh.notes,
    })
}

/// `health` — ARCHITECTURE health ([governance-engine] operational sheet):
/// DB presence/size, schema version, FTS coherence, and counts, behind the
/// same reconcile prologue as every aggregate run (BR-03).
///
/// [governance-engine]: ../../../docs/specs/architecture/components/governance-engine.md
/// The fast structural-integrity census ([FR-GV-18], [NFR-RA-13], [ADR-46]),
/// read from the RO pool. The single point where the gate paths, `health`, and
/// `doctor` all read the same verdict.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
fn structural_check(engine: &Engine) -> Result<StructuralReport> {
    quality_runtime(engine)?.submit_read(|store| store.structural_check())
}

/// The structural census **and** the indexed file count in ONE store access —
/// the census `doctor` reports and the `admitted` input its [FR-IX-13]
/// zero-admission diagnostic keys on ([FR-GV-18], [NFR-RA-13]).
///
/// Read together deliberately, the same discipline (and for the same reason) as
/// [`navigate::status`](crate::navigate)'s paired LOC keys: `submit_read` checks
/// out a pooled connection per call, so two calls can straddle a concurrent
/// index's commit — and a report that pairs `node_count: 1500` with "discovery
/// admitted no files" is worse than either number alone. One connection closes
/// that window.
///
/// [FR-IX-13]: ../../../docs/specs/requirements/FR-IX-13.md
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
fn structural_check_and_file_count(engine: &Engine) -> Result<(StructuralReport, u64)> {
    quality_runtime(engine)?.submit_read(|store| {
        let structural = store.structural_check()?;
        let files = store.counts()?.files;
        Ok((structural, files))
    })
}

/// `doctor` — the fast structural-integrity check ([FR-GV-18], [NFR-RA-13],
/// [ADR-46]): asserts one node per `symbol_id` and zero orphan rows in O(a
/// handful of indexed queries), reporting the verdict. Exits 1 on drift (the
/// CLI maps `!report.ok`), and the same verdict hard-fails `session_end`
/// ([FR-GV-05]) and `check_rules` ([FR-GV-02]).
///
/// A pure read of the persisted graph — it does not reconcile: the invariant is
/// about the node store's internal consistency, not its freshness vs the working
/// tree, and the check must stay cheap enough to run always-on.
///
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
pub(crate) fn doctor(engine: &Engine) -> Result<DoctorReport> {
    let (structural, indexed_files) = structural_check_and_file_count(engine)?;
    let mut report = doctor_report(structural, admission_tripwire(engine)?);
    // FR-IX-11: warn (path + reason) when a documentation directory-symlink exists
    // under the doc-include set but ended up unindexed. Diagnostic only — it does
    // not touch `report.ok`, so it never changes `doctor`'s exit status.
    report.doc_symlink_warnings = doc_symlink_warnings(engine)?;
    // FR-IX-13: explain a root that admitted nothing because every immediate child
    // was pruned as a nested git boundary — the parent-of-sibling-repositories
    // shape, which is otherwise structurally *consistent* and so passes every check
    // above while telling the user nothing. Diagnostic only, like the line before
    // it: `report.ok` is untouched, so `doctor` still exits on structural drift
    // alone. Derived through the engine seam `status` also uses, from the indexed
    // file count — read on the SAME connection as the census above, so the two
    // cannot describe different graph generations — so the two persisted-graph
    // surfaces and `index` agree by construction rather than by convention. The
    // seam is infallible, so this advisory adds no way for `doctor` to fail.
    report.zero_admission_warning =
        engine.zero_admission_diagnostic(indexed_files).map(|d| d.to_string());
    // CR-106 / FR-IN-03: an installed hook configuration that cannot fire from
    // THIS working tree. Diagnostic only, like the two lines above — `ok` and
    // the exit status still move on structural drift alone — but not optional:
    // the every-working-tree defect survived precisely because "installed" was
    // never checked against "fires", and every candidate reachability mechanism
    // has its own way of silently ceasing to work, so the check outlives the
    // choice of mechanism.
    report.hook_warnings = crate::hooks::reachability_findings(engine.root());
    // FR-IN-12: the Claude Code plugin registered twice, or newer than this
    // binary. Diagnostic only, like the lines above. The user's Claude Code
    // directory is read from the environment here, at the `doctor` entry point,
    // and passed down: `findings` itself reads no environment.
    report.plugin_warnings = crate::init::plugin::findings(
        engine.root(),
        crate::init::plugin::claude_config_dir().as_deref(),
        env!("CARGO_PKG_VERSION"),
    );
    Ok(report)
}

/// The [FR-IX-11] unindexed-doc-symlink warnings for `doctor`, computed from the
/// project root + on-disk config (the same source the admission tripwire reads)
/// without a full index walk, so `doctor` stays a cheap diagnostic.
fn doc_symlink_warnings(engine: &Engine) -> Result<Vec<String>> {
    let (_runtime, _registry, config) = engine.pipeline_ctx()?;
    let drops = crate::config::unindexed_doc_symlinks(engine.root(), &config)?;
    Ok(drops.iter().map(ToString::to_string).collect())
}

/// An admission-tripwire census (S-215, [FR-GV-20], [ADR-48]): every indexed
/// `files.path` row the *current* [`AdmissionAuthority`] would reject if it
/// were freshly discovered — the admission-divergence drift [CR-054] closes
/// (gitignored/boundary scratch, e.g. a dev worktree, that slipped past
/// `sync`/the watcher into the store).
///
/// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
/// [CR-054]: ../../../docs/requests/CR-054-graph-update-admission-unification.md
#[derive(Debug, Default)]
struct AdmissionCensus {
    /// Total unadmitted rows — the exact count, never truncated.
    unadmitted_files: u64,
    /// A capped, lexically-ordered sample of unadmitted paths ([NFR-RA-06]);
    /// `unadmitted_files` stays exact even when this is truncated.
    unadmitted_sample: Vec<String>,
}

impl AdmissionCensus {
    fn is_ok(&self) -> bool {
        self.unadmitted_files == 0
    }

    /// One fault line naming the drift (empty when [`is_ok`](Self::is_ok)) —
    /// the [`StructuralReport::faults`] analog for the admission dimension.
    fn faults(&self) -> Vec<String> {
        if self.unadmitted_files == 0 {
            return Vec::new();
        }
        vec![format!(
            "{} indexed file(s) violate the current AdmissionAuthority (admission drift, \
             FR-GV-20; run `logos index` to purge): {}",
            self.unadmitted_files,
            self.unadmitted_sample.join(", ")
        )]
    }
}

/// `admission_tripwire` — the always-on admission guard (S-215, [FR-GV-20],
/// [ADR-48]): builds the current [`AdmissionAuthority`] from
/// [`Engine::pipeline_ctx`] (project root + on-disk config) and flags every
/// [`indexed_files`](crate::graph_store::GraphStore::indexed_files) row it
/// rejects. Read-only, O(files) matcher evaluations — no reindex, no parse —
/// so it stays cheap enough to fold into every gate. Deliberately free-standing
/// from [`structural_report`](crate::graph_store::structural_report), which
/// stays a pure `&Connection` census ([NFR-RA-13]); this predicate additionally
/// needs the root + config the DB alone cannot supply.
///
/// [FR-GV-20]: ../../../docs/specs/requirements/FR-GV-20.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
fn admission_tripwire(engine: &Engine) -> Result<AdmissionCensus> {
    let (runtime, _registry, config) = engine.pipeline_ctx()?;
    let authority = AdmissionAuthority::from_config(engine.root(), &config)?;
    let files = runtime.submit_read(|store| store.indexed_files())?;
    // `indexed_files` arrives `ORDER BY path` (NFR-RA-06), so the unadmitted
    // subset is already lexically ordered before it is capped.
    let unadmitted: Vec<String> = files
        .into_iter()
        .filter(|f| !authority.admits_path(Path::new(&f.path)))
        .map(|f| f.path)
        .collect();
    Ok(AdmissionCensus {
        unadmitted_files: unadmitted.len() as u64,
        unadmitted_sample: capped_sample(unadmitted),
    })
}

/// Build the `doctor` read-model verdict from a raw structural census and the
/// [FR-GV-20] admission census — shared by [`doctor`] (the fast standalone
/// check) and [`verify`], which embeds the same verdict as `structural` so the
/// deep check reports the fast one's findings too.
fn doctor_report(report: StructuralReport, admission: AdmissionCensus) -> DoctorReport {
    let mut faults = report.faults();
    faults.extend(admission.faults());
    let ok = report.is_ok() && admission.is_ok();
    let message = if ok {
        format!(
            "graph structurally sound: {} nodes, one per symbol_id, no orphan rows, \
             no admission drift",
            report.node_count
        )
    } else {
        format!("graph structural drift (NFR-RA-13/FR-GV-20): {}", faults.join("; "))
    };
    DoctorReport {
        ok,
        node_count: report.node_count,
        distinct_symbol_ids: report.distinct_symbol_ids,
        duplicate_symbol_nodes: report.duplicate_symbol_nodes,
        dangling_file_refs: report.dangling_file_refs,
        dangling_edge_endpoints: report.dangling_edge_endpoints,
        orphan_shingles: report.orphan_shingles,
        unadmitted_files: admission.unadmitted_files,
        unadmitted_sample: admission.unadmitted_sample,
        faults,
        // Populated by `doctor` (the fast standalone check); `verify`, which reuses
        // this builder for its embedded `structural` field, leaves it empty — the
        // unindexed-doc-symlink diagnostic is an `index`/`doctor` surface ([FR-IX-11]).
        doc_symlink_warnings: Vec::new(),
        // Populated by `doctor` for the same reason: `verify` embeds this builder's
        // output as its `structural` field, and the zero-admission explanation is an
        // `index`/`status`/`doctor` surface ([FR-IX-13]).
        zero_admission_warning: None,
        // Populated by `doctor` alone, for the same reason: hook reachability
        // is a property of the working tree the command ran in, not of the
        // graph `verify` re-derives (CR-106).
        hook_warnings: Vec::new(),
        // Populated by `doctor` alone: the plugin's registration is a property
        // of the host settings, not of the graph (FR-IN-12).
        plugin_warnings: Vec::new(),
        message,
    }
}

/// The lexical cap on the leaked/orphaned symbol samples a [`VerifyReport`]
/// lists ([FR-GV-19], [NFR-RA-06]): the `*_total` counts stay exact; only the
/// listed sample is bounded so the CLI/MCP/web read-model is a fixed size even on
/// a badly drifted graph. Matches the `doc_gaps` default cap.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
const VERIFY_SAMPLE_CAP: usize = 50;

/// An edge's identity across two stores — `(kind discriminant, source symbol,
/// target symbol)`; the storage rowids differ between the live store and a
/// shadow reindex, the symbols do not. Ordered, so the drift lists are
/// deterministic ([NFR-RA-06]).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
type EdgeKey = (i32, String, String);

/// What an [`EdgeKey`] maps to: the kind and the files of its two endpoints,
/// carried so a drifted edge can name them ([FR-GV-19], [CR-236]).
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [CR-236]: ../../../docs/requests/CR-236-incremental-sync-loses-inbound-edges.md
type EdgeEnds = (EdgeKind, Option<String>, Option<String>);

/// One store's whole-graph read for [`verify`]: the row counts, the set of
/// every node's canonical symbol, and every edge keyed by symbol identity — read
/// in one pass over a read-only connection.
///
/// The counts drive the numeric deltas (they catch a Channel-A duplicate — two
/// nodes for one `symbol_id` — that the deduplicated symbol *set* would hide),
/// the symbol set drives the leaked/orphaned diff (Channel B), and the edge map
/// drives the lost/extra edge lists ([CR-236]).
///
/// [CR-236]: ../../../docs/requests/CR-236-incremental-sync-loses-inbound-edges.md
struct VerifyRead {
    census: VerifyCensus,
    symbols: BTreeSet<String>,
    edges: BTreeMap<EdgeKey, EdgeEnds>,
}

fn census(store: &dyn GraphStore) -> Result<VerifyRead> {
    let counts = store.counts()?;
    let symbols = store
        .all_nodes()?
        .into_iter()
        .map(|n| n.symbol.as_str().to_string())
        .collect::<BTreeSet<String>>();
    let edges = store
        .symbol_edges()?
        .into_iter()
        .map(|e| {
            (
                (e.kind.as_i32(), e.source, e.target),
                (e.kind, e.source_file, e.target_file),
            )
        })
        .collect::<BTreeMap<EdgeKey, EdgeEnds>>();
    Ok(VerifyRead {
        census: VerifyCensus {
            files: counts.files,
            nodes: counts.nodes,
            edges: counts.edges,
        },
        symbols,
        edges,
    })
}

/// The edges in `from` that `against` lacks: the exact total, and the first
/// [`VERIFY_SAMPLE_CAP`] of them in key order as [`DriftedEdge`]s. Only the
/// capped sample is cloned out of the map.
fn edge_diff(
    from: &BTreeMap<EdgeKey, EdgeEnds>,
    against: &BTreeMap<EdgeKey, EdgeEnds>,
) -> (u64, Vec<DriftedEdge>) {
    let mut missing = from.iter().filter(|(key, _)| !against.contains_key(*key));
    let sample = missing
        .by_ref()
        .take(VERIFY_SAMPLE_CAP)
        .map(|((_, source, target), (kind, source_file, target_file))| DriftedEdge {
            kind: *kind,
            source_symbol: source.clone(),
            target_symbol: target.clone(),
            source_file: source_file.clone(),
            target_file: target_file.clone(),
        })
        .collect::<Vec<_>>();
    let total = (sample.len() + missing.count()) as u64;
    (total, sample)
}

/// `verify` — the on-demand **deep** consistency check (CR-052, [FR-GV-19],
/// [NFR-RA-06], [ADR-46]): reindex the project into a throwaway shadow store via
/// the always-purge [`index`](../../../docs/specs/requirements/FR-IX-01.md) path,
/// census both stores, and diff node/edge/file counts + symbol sets against the
/// live graph — reporting any drift with a capped sample of leaked (live-only)
/// and orphaned (reindex-only) symbols, and of the lost (reindex-only) and extra
/// (live-only) edges ([CR-236]). Embeds the fast structural check ([FR-GV-18])
/// as `structural`.
///
/// It is the only check that catches **Channel-B orphans** — files the live store
/// retains but a fresh index would drop — which the fast `doctor` census cannot
/// see (a stale-but-internally-consistent graph passes `doctor`).
///
/// # Invariants
/// - The live store is opened **read-only** for the census (the reader pool's
///   connections carry `query_only`), so `verify` never mutates it ([FR-GV-19]).
/// - `verify` does **not** reconcile the live graph first: comparing the *current
///   persisted* graph to a fresh reindex is the whole point — a reconcile would
///   heal the very drift `verify` exists to surface.
/// - The shadow store lives at a distinct temp path outside the discovery walk
///   and is torn down (db + `-wal`/`-shm`) on completion by the [`ShadowStore`]
///   guard; the shadow [`Runtime`] is dropped before that teardown.
///
/// # Errors
/// Returns an error on a transient engine (no runtime), an unloadable registry,
/// or a failed shadow reindex / read.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
/// [CR-236]: ../../../docs/requests/CR-236-incremental-sync-loses-inbound-edges.md
pub(crate) fn verify(engine: &Engine) -> Result<VerifyReport> {
    let (runtime, registry, config) = engine.pipeline_ctx()?;

    // 1. Live census + fast structural check + the FR-GV-20 admission read, in
    //    ONE read-only checkout so the counts, symbol set, structural verdict,
    //    and indexed-file list are a consistent snapshot. The reader pool never
    //    mutates the live store (FR-GV-19).
    let (live_read, structural, indexed_files) = runtime.submit_read(|store| {
        Ok((census(store)?, store.structural_check()?, store.indexed_files()?))
    })?;
    let authority = AdmissionAuthority::from_config(engine.root(), &config)?;
    let unadmitted: Vec<String> = indexed_files
        .into_iter()
        .filter(|f| !authority.admits_path(Path::new(&f.path)))
        .map(|f| f.path)
        .collect();
    let admission = AdmissionCensus {
        unadmitted_files: unadmitted.len() as u64,
        unadmitted_sample: capped_sample(unadmitted),
    };

    // 2. Reindex the live tree into a throwaway shadow store (a distinct temp
    //    path, torn down by the guard) and census it. The shadow Runtime is
    //    dropped before the guard so the writer releases the db first.
    let shadow = crate::pipeline::ShadowStore::create()?;
    let reindex_read = {
        let shadow_rt =
            crate::pipeline::shadow_reindex(registry, engine.root(), &config, &shadow)?;
        let read = shadow_rt.submit_read(census)?;
        drop(shadow_rt);
        read
    };
    let (live, reindex) = (live_read.census, reindex_read.census);
    // `shadow` guard drops at end of function → db + `-wal`/`-shm` removed.

    // 3. Diff. Counts drive the numeric deltas; the symbol-set difference names
    //    the leaked (live-only) and orphaned (reindex-only) symbols. A BTreeSet
    //    difference is already lexically ordered (NFR-RA-06), so the sample is
    //    deterministic before it is capped. The edge maps are diffed the same
    //    way: `lost` is what a fresh reindex has and the live graph does not,
    //    `extra` the reverse (CR-236).
    let node_delta = live.nodes as i64 - reindex.nodes as i64;
    let edge_delta = live.edges as i64 - reindex.edges as i64;
    let file_delta = live.files as i64 - reindex.files as i64;

    let leaked: Vec<String> = live_read
        .symbols
        .difference(&reindex_read.symbols)
        .cloned()
        .collect();
    let orphaned: Vec<String> = reindex_read
        .symbols
        .difference(&live_read.symbols)
        .cloned()
        .collect();
    let leaked_total = leaked.len() as u64;
    let orphaned_total = orphaned.len() as u64;
    let (lost_edges_total, lost_edges) = edge_diff(&reindex_read.edges, &live_read.edges);
    let (extra_edges_total, extra_edges) = edge_diff(&live_read.edges, &reindex_read.edges);

    let ok = node_delta == 0
        && edge_delta == 0
        && file_delta == 0
        && leaked_total == 0
        && orphaned_total == 0
        && lost_edges_total == 0
        && extra_edges_total == 0
        && structural.is_ok()
        && admission.is_ok();

    let message = if ok {
        format!(
            "graph consistent: live matches a fresh reindex \
             ({} nodes, {} edges, {} files), structurally sound",
            live.nodes, live.edges, live.files
        )
    } else {
        let mut parts = Vec::new();
        if node_delta != 0 || edge_delta != 0 || file_delta != 0 {
            parts.push(format!(
                "count drift vs fresh reindex (live−reindex): \
                 nodes {node_delta:+}, edges {edge_delta:+}, files {file_delta:+}"
            ));
        }
        if leaked_total > 0 {
            parts.push(format!("{leaked_total} leaked symbol(s) (live-only)"));
        }
        if orphaned_total > 0 {
            parts.push(format!("{orphaned_total} orphaned symbol(s) (reindex-only)"));
        }
        if lost_edges_total > 0 {
            parts.push(format!("{lost_edges_total} lost edge(s) (reindex-only)"));
        }
        if extra_edges_total > 0 {
            parts.push(format!("{extra_edges_total} extra edge(s) (live-only)"));
        }
        if !structural.is_ok() {
            parts.push(format!(
                "structural drift: {}",
                structural.faults().join("; ")
            ));
        }
        if !admission.is_ok() {
            parts.extend(admission.faults());
        }
        format!("graph drift (NFR-RA-06/NFR-RA-13/FR-GV-20): {}", parts.join("; "))
    };

    Ok(VerifyReport {
        ok,
        live,
        reindex,
        node_delta,
        edge_delta,
        file_delta,
        leaked_total,
        leaked_symbols: capped_sample(leaked),
        orphaned_total,
        orphaned_symbols: capped_sample(orphaned),
        lost_edges_total,
        lost_edges,
        extra_edges_total,
        extra_edges,
        structural: doctor_report(structural, admission),
        message,
    })
}

/// Truncate an already-ordered symbol list to [`VERIFY_SAMPLE_CAP`] for the
/// bounded read-model sample ([NFR-RA-06]).
fn capped_sample(mut symbols: Vec<String>) -> Vec<String> {
    symbols.truncate(VERIFY_SAMPLE_CAP);
    symbols
}

pub(crate) fn health(engine: &Engine, reconcile: bool) -> Result<HealthInfo> {
    let fresh = reconcile_step(engine, reconcile)?;
    let runtime = quality_runtime(engine)?;
    let (counts, store_health, structural) = runtime.submit_read(|store| {
        Ok((
            store.counts()?,
            store.store_health()?,
            store.structural_check()?,
        ))
    })?;
    // FTS5's 'integrity-check' command is an INSERT — it must run on the
    // writer connection. The desync is *reported*, not propagated: `health`
    // exists to diagnose exactly this Correctness fault (ADR-14).
    let fts_error: Option<String> =
        runtime.submit_write(|w| Ok(w.fts_integrity_check().err().map(|e| format!("{e:#}"))))?;

    let db_path: PathBuf = engine.root().join(".logos").join("logos.db");
    let db_size_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    // S-215 / FR-GV-20: the admission tripwire — read outside the counts/
    // store_health/structural checkout (it needs the on-disk config, not just
    // the store), then folded into the SAME structural_ok/structural_faults
    // pair `health` already exposes (CR-052) — admission drift is a graph
    // structural-integrity dimension too.
    let admission = admission_tripwire(engine)?;

    // ARCHITECTURE health is sound only when the FTS index is coherent
    // (NFR-RA-09), the node store is structurally sound (NFR-RA-13, CR-052),
    // AND no indexed file violates the current admission rules (FR-GV-20).
    let fts_ok = fts_error.is_none();
    let mut structural_faults = structural.faults();
    structural_faults.extend(admission.faults());
    let structural_ok = structural.is_ok() && admission.is_ok();
    let ok = fts_ok && structural_ok;
    let message = match (&fts_error, structural_ok) {
        // An FTS desync is a Correctness fault (ADR-14) — reported loud, with
        // the remedy; it takes precedence in the summary line.
        (Some(reason), _) => format!("FTS index desync: {reason}; run `logos index` to rebuild"),
        // Structural/admission drift is the CR-052/FR-GV-20 Correctness fault —
        // name it and the remedy (a full reindex rebuilds ground truth).
        (None, false) => format!(
            "graph structural drift (NFR-RA-13/FR-GV-20): {}; run `logos index` to rebuild",
            structural_faults.join("; ")
        ),
        (None, true) => "store healthy: schema current, FTS coherent, graph sound".to_string(),
    };

    Ok(HealthInfo {
        ok,
        db_path: db_path.display().to_string(),
        db_size_bytes,
        schema_version: store_health.schema_version,
        fts_ok,
        structural_ok,
        structural_faults,
        files: counts.files,
        nodes: counts.nodes,
        edges: counts.edges,
        unresolved_refs: counts.refs_total.saturating_sub(counts.refs_resolved),
        freshness: fresh.line(),
        message,
    })
}
