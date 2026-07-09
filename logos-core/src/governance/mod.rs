//! The governance engine ([governance-engine component], S-020,
//! [FR-GV-01]..[FR-GV-09], [FR-RC-01]..[FR-RC-04], [ADR-11], [ADR-14]).
//!
//! Evaluates the `rules.toml` architecture contract, runs the session/CI
//! gate, and serves `evolution`, `dsm`, and `test_gaps` — every aggregate
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
//! Every `rules.toml` violation is `severity = "error"` in v1 — checked-in
//! policy is binding (ratified 2026-06-06). The `warning` tier exists in the
//! schema for future advisory rules.
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
    LatestMetricSnapshot, MetricSnapshotRow, NewViolation, NodeRow, StructuralReport,
};
use crate::hydrate::{build_view, Granularity};
use crate::model::{EdgeKind, NodeId, NodeKind};
use crate::models::quality::{
    DocGap, DocGapsReport, DoctorReport, DsmReport, DsmRow, EvolutionPoint, EvolutionReport,
    GateResult, HealthInfo, MetricDelta, MetricRegression, MetricSnapshot, MetricValue,
    RulesReport, ScanResult, SessionInfo, TemporalTier, TestGap, TestGapsReport, VerifyCensus,
    VerifyReport, Violation,
};
use crate::runtime::Runtime;

mod smells;

#[cfg(test)]
mod tests;

/// The gate's float-noise tolerance on the 0–10000 integer signal (BR-10,
/// DL-04): fail iff `current < baseline − EPSILON`.
const EPSILON: f64 = 1.0;

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

/// The default `test_gaps` listing cap ([FR-GV-08]).
///
/// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
const DEFAULT_TEST_GAPS_LIMIT: u32 = 50;

/// The default `doc_gaps` listing cap ([FR-GV-14]) — the documentation analog
/// of [`DEFAULT_TEST_GAPS_LIMIT`].
///
/// [FR-GV-14]: ../../../docs/specs/requirements/FR-GV-14.md
const DEFAULT_DOC_GAPS_LIMIT: u32 = 50;

/// The mandatory `test_gaps` honesty caveat (BR-16) — always emitted.
const TEST_GAPS_CAVEAT: &str =
    "static reachability, not execution coverage: a function is 'covered' if any \
     test node transitively calls it in the code graph; no test was executed";

/// The mandatory `doc_gaps` honesty caveat ([FR-GV-14]) — always emitted. The
/// documentation analog of [`TEST_GAPS_CAVEAT`]: presence of a resolved
/// reference, not documentation quality or completeness.
const DOC_GAPS_CAVEAT: &str =
    "reference presence, not documentation quality: a symbol is 'documented' if any \
     doc section resolves a reference to it in the documentation graph; the prose's \
     depth or accuracy is not judged";

/// The only violation severity v1 emits (ratified 2026-06-06).
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
    /// reachability BFS — the SAME `test_node_ids` seam `test_gaps` uses
    /// ([FR-GV-08]), so the coverage contract and `test_gaps` can never
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

/// Constraint point queries (the original "Constraints" section): `max_cycles`,
/// `max_cc`, `max_fn_lines`, `no_god_files`. Each active key counts once toward
/// `checked`; inputs arrive in id order so the per-file `no_god_files` walk is
/// deterministic ([NFR-RA-06]).
fn check_constraints(input: &EvalInput<'_>) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    let constraints = &input.compiled.rules.constraints;

    if let Some(max) = constraints.max_cycles {
        checked += 1;
        if input.cycles > u64::from(max) {
            violations.push(Violation {
                rule: "max_cycles".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message: format!(
                    "{} dependency cycles exceed max_cycles = {max}",
                    input.cycles
                ),
            });
        }
    }
    if let Some(max) = constraints.max_cc {
        checked += 1;
        for f in input.functions {
            if f.cyclomatic_complexity
                .is_some_and(|cc| cc > i64::from(max))
            {
                violations.push(Violation {
                    rule: "max_cc".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: f.file_path.clone().unwrap_or_default(),
                    node_id: Some(f.id.get()),
                    message: format!(
                        "fn `{}` has cyclomatic complexity {} > max_cc = {max}",
                        f.name,
                        f.cyclomatic_complexity.unwrap_or_default()
                    ),
                });
            }
        }
    }
    if let Some(max) = constraints.max_fn_lines {
        checked += 1;
        for f in input.functions {
            if f.line_count.is_some_and(|lines| lines > i64::from(max)) {
                violations.push(Violation {
                    rule: "max_fn_lines".to_string(),
                    rule_type: "constraint".to_string(),
                    severity: SEVERITY_ERROR.to_string(),
                    file: f.file_path.clone().unwrap_or_default(),
                    node_id: Some(f.id.get()),
                    message: format!(
                        "fn `{}` spans {} lines > max_fn_lines = {max}",
                        f.name,
                        f.line_count.unwrap_or_default()
                    ),
                });
            }
        }
    }
    if let Some(max) = constraints.no_god_files {
        checked += 1;
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
/// `dsm` view builds ([`build_view`] at [`Granularity::Module`]) — it already
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
/// Computed from the hydrated edge set, never persisted ([CR-002]: no
/// migration).
///
/// [FR-GV-11]: ../../../docs/specs/requirements/FR-GV-11.md
/// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
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
        .filter(|e| e.kind != EdgeKind::ForbiddenDependency)
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

    if let Some(max) = constraints.max_fan_in {
        checked += 1;
        violations.extend(module_coupling_violations(
            &view,
            max,
            "max_fan_in",
            "fan-in",
            Direction::Incoming,
            &module_file,
        ));
    }
    if let Some(max) = constraints.max_fan_out {
        checked += 1;
        violations.extend(module_coupling_violations(
            &view,
            max,
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
/// first-seen node id and is therefore not itself the ordering contract).
///
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn module_coupling_violations(
    view: &crate::hydrate::GraphView,
    max: u32,
    rule: &str,
    metric_label: &str,
    direction: Direction,
    module_file: &HashMap<String, String>,
) -> Vec<Violation> {
    let graph = view.graph();
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
        })
        .collect()
}

/// Redundancy budgets (production-scope is_dead / is_duplicate, FR-GV-11).
///
/// Counted over the SAME annotation rows the Redundancy metric reads
/// ([FR-QM-05]) — Function/Method nodes, `is_test` and derived excluded
/// ([FR-QM-08]) — so the budget and the metric agree by construction. One
/// project-wide violation per budget (no per-symbol enumeration).
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

    if let Some(budget) = &constraints.max_dead {
        checked += 1;
        // NULL (`is_dead = None`) is excluded for free — only `Some(true)` counts
        // ([CR-043], [ADR-39]): a language without the reachability capability
        // renders NULL and never inflates the budget.
        let dead = production_fns()
            .filter(|f| f.is_dead == Some(true))
            .count() as u64;
        // Both modes route through the one budget seam ([FR-GV-11], [ADR-39]):
        // absolute fails iff `dead > max`; delta fails iff `dead` rises above
        // the blessed `baseline (+delta)`. The message is mode-specific but
        // deterministic, so a re-run is byte-identical ([NFR-RA-06]).
        if let Some(message) = budget.exceeded_message(dead) {
            violations.push(Violation {
                rule: "max_dead".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message,
            });
        }
    }
    if let Some(max) = constraints.max_duplicates {
        checked += 1;
        let duplicates = production_fns()
            .filter(|f| f.is_duplicate == Some(true))
            .count() as u64;
        if duplicates > u64::from(max) {
            violations.push(Violation {
                rule: "max_duplicates".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message: format!("{duplicates} duplicate functions exceed max_duplicates = {max}"),
            });
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
    if let Some(max) = input.compiled.rules.constraints.max_nesting_depth {
        checked += 1;
        let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();
        for f in production_fns() {
            if f.max_nesting_depth.is_some_and(|d| d > i64::from(max)) {
                let depth = f.max_nesting_depth.unwrap_or_default();
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
    if let Some(max) = input.compiled.rules.constraints.max_brain_methods {
        checked += 1;
        let t = &input.thresholds;
        let brain = production_fns()
            .filter(|f| {
                f.cyclomatic_complexity.is_some_and(|c| c >= t.brain_cc)
                    && f.line_count.is_some_and(|l| l >= t.brain_loc)
                    && f.max_nesting_depth.is_some_and(|d| d >= t.brain_nest)
            })
            .count() as u64;
        if brain > u64::from(max) {
            violations.push(Violation {
                rule: "max_brain_methods".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message: format!("{brain} brain methods exceed max_brain_methods = {max}"),
            });
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
    if let Some(max) = input.compiled.rules.constraints.max_clone_ratio {
        checked += 1;
        let total = production_fns().count();
        let cloned = production_fns().filter(|f| f.clone_group.is_some()).count();
        let ratio = if total == 0 {
            0.0
        } else {
            cloned as f64 / total as f64
        };
        if ratio > max {
            violations.push(Violation {
                rule: "max_clone_ratio".to_string(),
                rule_type: "constraint".to_string(),
                severity: SEVERITY_ERROR.to_string(),
                file: String::new(),
                node_id: None,
                message: format!(
                    "{cloned}/{total} near-clone functions (ratio {ratio:.4}) exceed \
                     max_clone_ratio = {max}"
                ),
            });
        }
    }
    (violations, checked)
}

/// `no_god_containers` — no class-like container over the god thresholds
/// (FR-QM-12). Each god container is one error, in node-id order — the SAME set
/// Focus counts as god (shared `metrics::god_containers`), so the budget and the
/// dimension never disagree. Only enforced when explicitly `true`.
fn check_god_containers(
    input: &EvalInput<'_>,
    test_ids: &HashSet<NodeId>,
) -> (Vec<Violation>, u32) {
    let mut violations = Vec::new();
    let mut checked = 0u32;
    if input.compiled.rules.constraints.no_god_containers == Some(true) {
        checked += 1;
        let node_of: HashMap<NodeId, &NodeRow> = input.nodes.iter().map(|n| (n.id, n)).collect();
        for god in
            crate::metrics::god_containers(input.nodes, input.edges, test_ids, input.thresholds)
        {
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
        if matches!(
            edge.kind,
            EdgeKind::Contains | EdgeKind::ForbiddenDependency
        ) || edge.kind.is_documentation()
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
/// BFS from an `is_test` node. It reuses `test_gaps`'s reachability ([FR-GV-08],
/// `test_reachable_set`) seeded from the SAME persisted `is_test` column
/// ([FR-AN-05], `test_node_ids`), so the contract and `test_gaps` can never
/// disagree. Non-exported symbols are exempt (a public-API test path, not total
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
/// `[[require_tested]]` and `test_gaps` share `test_reachable_set`. Non-exported
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
            });
        }
    }

    (violations, checked)
}

/// The set of nodes reachable from any test node over `Calls` edges — the
/// shared static-reachability core of `test_gaps` ([FR-GV-08], BR-16) and the
/// `[[require_tested]]` coverage contract ([FR-GV-13]). The seed `test_ids` are
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

    // BR-12: clear + re-materialise the derived policy graph each run. The
    // annotation pass owns that algorithm (FR-AN-03); re-running it here is
    // what makes check_rules idempotent AND correct after a rules.toml edit
    // that touched no source file.
    // A full re-materialisation (a rules.toml edit can flip layer/forbidden
    // verdicts across the whole graph with no source change), so write every
    // verdict — `incremental = false` (S-024-HF); this is the governance path,
    // not the NFR-PE-03 sync hot path.
    // CR-043 / ADR-39: the reachability-capability set gates dead-code so the
    // re-materialised `is_dead` column matches what index/sync wrote — the same
    // registry-derived set keeps `check_rules` and the pipeline in agreement.
    let reachable_exts = registry.reachability_extensions();
    crate::annotate::run(
        runtime,
        &compiled.rules,
        &config.semantics.entry_points,
        &config.semantics.test_markers,
        &reachable_exts,
        false,
    )?;
    engine.advance_sync_stamp();

    let (nodes, edges, functions, function_metrics, annotations, test_node_ids) = runtime
        .submit_read(|store| {
            Ok((
                store.all_nodes()?,
                store.all_edges()?,
                store.function_constraint_rows()?,
                // The is_dead/is_duplicate annotation rows for the FR-GV-11
                // redundancy budgets — the SAME query the Redundancy metric reads,
                // so the budget and the metric agree by construction.
                store.function_metrics()?,
                // The annotation rows carry the `exported` flag the FR-GV-13
                // [[require_tested]] contract needs (NodeRow omits visibility).
                store.annotation_nodes()?,
                // The persisted is_test set: shared with test_gaps (FR-AN-05 /
                // FR-GV-08) and used for production-scope metric filtering
                // (FR-QM-08) — the gate, coverage contract, and Acyclicity rule
                // can never disagree about the same SCC set.
                store.test_node_ids()?,
            ))
        })?;

    let cycles = match cycles {
        Some(c) => c,
        // Both the bare check_rules path and scan land here (each calls
        // materialise_and_evaluate with no precomputed cycle count), and only
        // when the contract actually budgets cycles.
        None if compiled.rules.constraints.max_cycles.is_some() => {
            let view = engine.hydrate(Granularity::ExcludeContains)?;
            let test_ids: std::collections::HashSet<crate::model::NodeId> =
                test_node_ids.iter().copied().collect();
            // Only the Acyclicity cycle count is read here — the CR-005 thresholds
            // do not affect it, so the documented defaults suffice.
            crate::metrics::compute(
                &view,
                &nodes,
                &edges,
                &function_metrics,
                &test_ids,
                crate::metrics::Thresholds::default(),
            )
            .acyclicity
            .raw as u64
        }
        None => 0,
    };

    let input = EvalInput {
        compiled,
        nodes: &nodes,
        edges: &edges,
        functions: &functions,
        function_metrics: &function_metrics,
        annotations: &annotations,
        test_node_ids: &test_node_ids,
        cycles,
        // The effective CR-005 thresholds the four new budgets read (BR-25) —
        // the SAME set the snapshot scores under (composed from the same rules).
        thresholds: effective_thresholds(&compiled.rules),
    };
    Ok(evaluate(&input))
}

/// Persist the violation set ([FR-GV-02], SRS §5.1: written per run,
/// replaced wholesale — idempotent like the derived policy graph).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
fn persist_violations(
    runtime: &Runtime,
    snapshot_id: Option<i64>,
    violations: &[Violation],
) -> Result<()> {
    let owned: Vec<Violation> = violations.to_vec();
    let created_at = unix_now();
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
                created_at,
            })
            .collect();
        w.replace_violations(&rows)
    })
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

    // Score on the freshly reconciled + re-materialised graph.
    let (violations, _checked) = materialise_and_evaluate(engine, &compiled, None)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: the snapshot scores under the effective rules.toml thresholds, so
    // its persisted hash gates the baseline (FR-GV-10) and the budgets agree.
    let thresholds = effective_thresholds(&compiled.rules);
    let (snapshot_id, metrics) =
        crate::metrics::snapshot(runtime, &view, fresh.head.as_deref(), thresholds)?;
    persist_violations(runtime, Some(snapshot_id), &violations)?;

    // Per-dimension worst-offender detail (CR-005 §3.2): the top-N offenders per
    // new dimension, deterministically ordered and capped — review-phase
    // visibility on the metric-bearing `scan` surface. Read fresh under the same
    // thresholds the snapshot used; report detail only, never gated.
    let (nodes, edges, functions, test_ids) = runtime.submit_read(|store| {
        Ok((
            store.all_nodes()?,
            store.all_edges()?,
            store.function_metrics()?,
            store.test_node_ids()?,
        ))
    })?;
    let test_ids: HashSet<NodeId> = test_ids.into_iter().collect();
    let worst_offenders = crate::metrics::worst_offenders(
        &nodes,
        &edges,
        &functions,
        &test_ids,
        thresholds,
        crate::metrics::WORST_OFFENDER_CAP,
    );

    // The non-gated temporal tier (CR-006, FR-GH-07): computed independently of
    // every gated column above and attached as advisory detail. Fail-soft — a
    // history error (e.g. a non-compiling [history] defect_patterns) degrades to
    // an n/a tier + warning, never failing the gated scan (BR-26 two-tier rule).
    let freshness = fresh.line();
    let mut warnings = fresh.warnings;
    let temporal = scan_temporal_tier(engine, &mut warnings);

    Ok(ScanResult {
        signal: metrics.aggregate_signal,
        freshness,
        violations,
        metrics,
        worst_offenders,
        temporal,
        warnings,
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

/// The most-recent persisted metric snapshot as a full read-model, or `None` on
/// a never-`scan`-ned store ([ADR-28] `Engine::latest_metrics`). A pure read —
/// no compute, no persist. Reconstructs the [`MetricSnapshot`] breakdown from the
/// last `metric_snapshots` row, preserving the Cohesion/Focus applicability
/// drop-out as `None` (the [NFR-CC-04] n/a sentinel), never a fabricated zero.
pub(crate) fn latest_metrics(engine: &Engine) -> Result<Option<MetricSnapshot>> {
    let row = quality_runtime(engine)?.submit_read(|store| store.latest_metric_snapshot())?;
    Ok(row.map(metric_snapshot_from_row))
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
    MetricSnapshot {
        modularity: mv(row.modularity_raw, row.modularity_normalized),
        acyclicity: mv(row.acyclicity_raw, row.acyclicity_normalized),
        depth: mv(row.depth_raw, row.depth_normalized),
        equality: mv(row.equality_raw, row.equality_normalized),
        redundancy: mv(row.redundancy_raw, row.redundancy_normalized),
        nesting: req(row.nesting_raw, row.nesting_normalized),
        conciseness: req(row.conciseness_raw, row.conciseness_normalized),
        cohesion: opt(row.cohesion_applicable, row.cohesion_raw, row.cohesion_normalized),
        focus: opt(row.focus_applicable, row.focus_raw, row.focus_normalized),
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
/// breakdown joined with the read-only temporal tier, or `None` on a
/// never-`scan`-ned store ([ADR-28], [CR-018], S-082). Backs the web dashboard's
/// Metrics and Health views. Carries **no** worst-offenders or rule violations
/// (those are not persisted on the snapshot — they are review-phase detail the
/// metric-bearing `scan` surface computes fresh, deliberately absent from the
/// read-only dashboard so a GET stays a pure read).
///
/// A never-`scan`-ned store yields a `ScanResult` whose `metrics.empty` is
/// `true` — the same honest sentinel `scan` returns for an empty graph — so the
/// view renders the "run `logos scan`" empty state ([NFR-CC-04]), never zeros.
pub(crate) fn latest_scan(engine: &Engine) -> Result<ScanResult> {
    let metrics = latest_metrics(engine)?.unwrap_or(MetricSnapshot {
        empty: true,
        ..MetricSnapshot::default()
    });
    let mut warnings = Vec::new();
    let temporal = latest_temporal_tier(engine, &mut warnings);
    Ok(ScanResult {
        signal: metrics.aggregate_signal,
        freshness: String::new(),
        violations: Vec::new(),
        metrics,
        worst_offenders: Default::default(),
        temporal,
        warnings,
    })
}

/// The read-only gate **verdict**: compare the last persisted snapshot's signal
/// to the saved baseline **without computing or persisting** a snapshot ([ADR-28]
/// — the dashboard's Health verdict). Mirrors [`gate`]'s comparison for the
/// comparable case (`current < baseline − ε` ⇒ FAIL, BR-10) but never
/// re-baselines or writes: an incomparable baseline (different metric semantics
/// or threshold hash) is reported as an informational pass, not an auto-save.
/// A never-`scan`-ned store returns an `n/a` verdict naming the producing command.
pub(crate) fn latest_gate(engine: &Engine) -> Result<GateResult> {
    let runtime = quality_runtime(engine)?;
    let Some(metrics) = latest_metrics(engine)? else {
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
    };

    let baseline = runtime.submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?;
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
            result.regressions = metric_regressions(&base, &metrics);
            match (metrics.aggregate_signal, base.aggregate_signal) {
                (Some(current), Some(baseline_signal)) => {
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
                    result.message =
                        "signal or baseline is n/a (empty graph) — informational pass".to_string();
                }
            }
        }
    }

    Ok(result)
}

/// `check_rules` — the compliance report ([FR-GV-02]); no metric snapshot
/// (that is scan/gate's job, [FR-GV-09]).
///
/// [FR-GV-02]: ../../../docs/specs/requirements/FR-GV-02.md
/// [FR-GV-09]: ../../../docs/specs/requirements/FR-GV-09.md
pub(crate) fn check_rules(
    engine: &Engine,
    rules_path: Option<&Path>,
    reconcile: bool,
) -> Result<RulesReport> {
    let fresh = reconcile_step(engine, reconcile)?;
    let compiled = load_rules_cached(engine, rules_path)?;
    let runtime = quality_runtime(engine)?;

    let (mut violations, checked_rules) = materialise_and_evaluate(engine, &compiled, None)?;
    persist_violations(runtime, None, &violations)?;

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
        });
    }

    Ok(RulesReport {
        passed: !violations.iter().any(|v| v.severity == SEVERITY_ERROR),
        checked_rules,
        // Honest "is a contract authored?" signal (NFR-CC-04): the empty
        // contract that a missing default file compiles to carries the
        // `ABSENT_RULES_HASH` sentinel; any loaded file hashes to its content.
        rules_present: compiled.hash != ABSENT_RULES_HASH,
        violations,
        freshness: fresh.line(),
        warnings: fresh.warnings,
    })
}

/// `gate` / `session_end` — fresh snapshot, baseline comparison within
/// epsilon ([FR-GV-04], [FR-GV-05], BR-10); `save` upserts the baseline
/// instead of comparing.
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
/// [FR-GV-05]: ../../../docs/specs/requirements/FR-GV-05.md
pub(crate) fn gate(
    engine: &Engine,
    threshold: Option<u32>,
    save: bool,
    reconcile: bool,
) -> Result<GateResult> {
    let fresh = reconcile_step(engine, reconcile)?;
    let runtime = quality_runtime(engine)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: the gate scores under the effective rules.toml thresholds, so the
    // snapshot's persisted hash is what the baseline comparison gates on.
    let thresholds = effective_thresholds(&load_rules_cached(engine, None)?.rules);
    // FR-GV-09: every gate writes a snapshot, saved or compared.
    let (snapshot_id, metrics) =
        crate::metrics::snapshot(runtime, &view, fresh.head.as_deref(), thresholds)?;

    let mut result = GateResult {
        passed: true,
        saved: save,
        signal: metrics.aggregate_signal,
        baseline_signal: None,
        test_function_count: metrics.test_function_count,
        threshold,
        epsilon: EPSILON,
        regressions: Vec::new(),
        structural_faults: Vec::new(),
        freshness: fresh.line(),
        message: String::new(),
        warnings: fresh.warnings,
    };

    if save {
        let created_at = unix_now();
        runtime.submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, created_at))?;
        result.baseline_signal = metrics.aggregate_signal;
        result.message = format!("baseline saved (snapshot #{snapshot_id})");
        return Ok(result);
    }

    let baseline = runtime.submit_read(|store| store.baseline_snapshot(SCOPE_PROJECT))?;
    match baseline {
        None => {
            // FR-GV-05: no baseline → informational pass.
            result.message =
                "no baseline saved — informational pass (save one with `gate --save` or \
                 `session_start`)"
                    .to_string();
        }
        Some(base) if base.metric_version != crate::metrics::METRIC_SEMANTICS_VERSION => {
            // FR-GV-10: the baseline was recorded under a different
            // metrics-semantics version (e.g. the pre-CR-001 test-inclusive
            // scope) and is incomparable to this production-scope run. Auto-
            // re-baseline against the fresh snapshot, announce it, and pass
            // informationally — never a regression failure against an
            // incomparable anchor (UAT-GV-06). The next gate finds a matching
            // version and compares normally.
            let created_at = unix_now();
            runtime
                .submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, created_at))?;
            result.saved = true;
            result.baseline_signal = metrics.aggregate_signal;
            result.message = "baseline reset: metric semantics changed".to_string();
            return Ok(result);
        }
        Some(base) if base.thresholds_hash.as_deref() != Some(metrics.thresholds_hash.as_str()) => {
            // FR-GV-10 / BR-25 (CR-005): the effective detection thresholds were
            // tuned in rules.toml since the baseline was saved — its hash no
            // longer matches this run's. A tuned threshold moves what several
            // dimensions measure, so the baseline is incomparable: take the SAME
            // announced re-baseline path as a semantics-version change (auto-save,
            // informational pass, distinct notice). The next gate finds a matching
            // hash and compares normally (UAT-QM-13 step 3) — threshold tuning is
            // always possible and never silently moves a gated comparison.
            let created_at = unix_now();
            runtime
                .submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, created_at))?;
            result.saved = true;
            result.baseline_signal = metrics.aggregate_signal;
            result.message = "baseline reset: metric thresholds changed".to_string();
            return Ok(result);
        }
        Some(base) => {
            result.baseline_signal = base.aggregate_signal.map(|s| s as u32);
            result.regressions = metric_regressions(&base, &metrics);
            match (metrics.aggregate_signal, base.aggregate_signal) {
                (Some(current), Some(baseline_signal)) => {
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
                    result.message =
                        "signal or baseline is n/a (empty graph) — informational pass".to_string();
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
                result.message = format!(
                    "{}{}signal is n/a (empty graph) — cannot satisfy threshold {floor}",
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
        .filter(|(_, baseline, current)| *current < *baseline - METRIC_NOISE)
        .map(|(metric, baseline, current)| MetricRegression {
            metric: metric.to_string(),
            baseline,
            current,
            delta: current - baseline,
        })
        .collect()
}

/// `session_start` — the MCP spelling of `gate --save` ([FR-GV-04]).
///
/// [FR-GV-04]: ../../../docs/specs/requirements/FR-GV-04.md
pub(crate) fn session_start(engine: &Engine) -> Result<SessionInfo> {
    let fresh = reconcile_step(engine, true)?;
    let runtime = quality_runtime(engine)?;
    let view = engine.hydrate(Granularity::ExcludeContains)?;
    // BR-25: baseline under the effective rules.toml thresholds (its hash gates
    // the later session_end comparison, FR-GV-10).
    let thresholds = effective_thresholds(&load_rules_cached(engine, None)?.rules);
    let (snapshot_id, metrics) =
        crate::metrics::snapshot(runtime, &view, fresh.head.as_deref(), thresholds)?;

    let started_at = unix_now();
    runtime.submit_write(move |w| w.upsert_baseline(SCOPE_PROJECT, snapshot_id, started_at))?;

    Ok(SessionInfo {
        session_id: snapshot_id.to_string(),
        started_at,
        signal: metrics.aggregate_signal,
        freshness: fresh.line(),
        message: format!(
            "session started — baseline saved (snapshot #{snapshot_id}); \
             `session_end` compares against it"
        ),
    })
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
                delta: previous.map(|p| current[i] - p[i]),
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
/// counts dep edges `i → j`, rows ordered by layer order then name (so
/// forward deps sit below the diagonal).
///
/// [FR-GV-07]: ../../../docs/specs/requirements/FR-GV-07.md
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
    // file-backed (a module rollup key can fall back to `file:<path>`).
    let mut order: Vec<usize> = (0..graph.node_count()).collect();
    let rows_unsorted: Vec<DsmRow> = graph
        .node_indices()
        .map(|idx| {
            let key = graph[idx].key.as_str();
            let layer = dsm_key_path(key, granularity)
                .and_then(|path| compiled.layer_of(path))
                .map(|(name, _)| name.to_string());
            DsmRow {
                name: key.to_string(),
                layer,
            }
        })
        .collect();
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
            position[&edge.source().index()],
            position[&edge.target().index()],
        );
        matrix[src][dst] += edge.weight().weight;
    }

    Ok(DsmReport {
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
    })
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

/// `test_gaps` — static test coverage over `calls` BFS from test nodes
/// ([FR-GV-08], BR-16), with the mandatory honesty caveat.
///
/// When `hotspot_ranks` is supplied by the façade — a read-only file → hotspot
/// score map — the untested set is ordered by **blast radius** ([FR-GV-17]):
/// caller fan-in ([FR-NV-02]) × the containing file's hotspot score
/// ([FR-GH-06]), most-urgent first. The ranking is *supplied* here, never linked
/// — the governance engine contributes only the graph-native fan-in and never
/// reads history ([CR-038], [ADR-28]). `None` (no history/hotspot store) degrades
/// to the FR-GV-08 file/name order, the caveat still emitted, never a fabricated
/// ranking ([NFR-CC-04]).
///
/// [FR-GV-08]: ../../../docs/specs/requirements/FR-GV-08.md
/// [FR-GV-17]: ../../../docs/specs/requirements/FR-GV-17.md
/// [FR-NV-02]: ../../../docs/specs/requirements/FR-NV-02.md
/// [FR-GH-06]: ../../../docs/specs/requirements/FR-GH-06.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
pub(crate) fn test_gaps(
    engine: &Engine,
    limit: Option<u32>,
    reconcile: bool,
    hotspot_ranks: Option<&HashMap<String, i64>>,
) -> Result<TestGapsReport> {
    let fresh = reconcile_step(engine, reconcile)?;
    let (runtime, registry, config) = engine.pipeline_ctx()?;
    let limit = limit.unwrap_or(DEFAULT_TEST_GAPS_LIMIT).max(1);

    let (nodes, edges, test_node_ids) = runtime.submit_read(|store| {
        Ok((
            store.all_nodes()?,
            store.all_edges()?,
            store.test_node_ids()?,
        ))
    })?;
    let entry_points: HashSet<&str> = config
        .semantics
        .entry_points
        .iter()
        .map(String::as_str)
        .collect();

    // Test nodes are read from the persisted `is_test` annotation ([FR-AN-05],
    // CR-001) — the single source of truth the annotation pass computed
    // (`test_evidence` ∨ path convention ∨ `[semantics].test_markers`). Reading
    // the column rather than re-deriving it is what guarantees `test_gaps` and
    // the annotation classify the identical set (CR-001 CRA-01).
    let test_ids: HashSet<_> = test_node_ids.into_iter().collect();

    // The advisory test-quality-smells appendix ([FR-CV-08], CR-007): the
    // current tree's source files re-parsed on demand via the plugins' optional
    // smell query. Computed entirely off the gate path — `test_gaps` itself
    // never feeds the gate, so this appendix cannot move it ([BR-28]) — and each
    // flagged candidate is re-confirmed against the canonical test-marker logic
    // ([FR-AN-05]) inside the detector.
    let smells = smells::detect_test_smells(engine.root(), registry, &config);

    // BFS over `calls` from every test node (BR-16) — the shared reachability
    // core the `[[require_tested]]` contract reuses ([FR-GV-13]).
    let reachable = test_reachable_set(&edges, &test_ids);

    // Caller fan-in ([FR-NV-02]): inbound `calls` edges per node — the
    // graph-native blast-radius axis ([FR-GV-17]). Computed only when the façade
    // supplied a hotspot ranking to weight it by; the degraded path keeps the
    // FR-GV-08 file/name order, so fan-in is never needed there.
    let fan_in: HashMap<NodeId, u64> = if hotspot_ranks.is_some() {
        let mut counts: HashMap<NodeId, u64> = HashMap::new();
        for edge in &edges {
            if edge.kind == EdgeKind::Calls {
                *counts.entry(edge.target).or_default() += 1;
            }
        }
        counts
    } else {
        HashMap::new()
    };

    // Gaps = non-test, non-entry-point functions not reachable from any test
    // node (BR-16), each scored by blast radius for ordering ([FR-GV-17]).
    let mut total = 0u64;
    let mut scored: Vec<ScoredGap> = Vec::new();
    for node in &nodes {
        if !matches!(node.kind, NodeKind::Function | NodeKind::Method)
            || test_ids.contains(&node.id)
            || entry_points.contains(node.name.as_str())
        {
            continue;
        }
        total += 1;
        if !reachable.contains(&node.id) {
            let file = node.file_path.clone().unwrap_or_default();
            let fan = fan_in.get(&node.id).copied().unwrap_or(0);
            let blast = blast_radius(fan, &file, hotspot_ranks);
            scored.push(ScoredGap {
                blast,
                gap: TestGap {
                    name: node.name.clone(),
                    file,
                    line: node.start_line,
                },
            });
        }
    }

    // Order by blast radius when a ranking was supplied ([FR-GV-17]), else the
    // FR-GV-08 file/name order; both deterministic ([NFR-RA-06]). Ordering
    // precedes truncation so raising `limit` reveals more gaps in the SAME
    // ranked order.
    let gap_count = scored.len();
    let mut gaps = order_untested(scored, hotspot_ranks.is_some());

    let covered = total - gap_count as u64;
    let truncated = gap_count > limit as usize;
    gaps.truncate(limit as usize);

    Ok(TestGapsReport {
        untested: gaps,
        total_functions: total,
        covered_functions: covered,
        // Integer signal posture (ADR-08/AR-03); n/a when nothing to cover.
        coverage_ratio: (total > 0)
            .then(|| ((covered as f64 / total as f64) * 10_000.0).round() as u32),
        limit,
        truncated,
        caveat: TEST_GAPS_CAVEAT.to_string(),
        freshness: fresh.line(),
        warnings: fresh.warnings,
        smells,
    })
}

/// One untested function paired with its blast-radius score ([FR-GV-17]) for
/// ordering. The score is an ordering key only — it is never serialized; the
/// wire form stays the FR-GV-08 [`TestGap`].
struct ScoredGap {
    /// Caller fan-in × the containing file's hotspot score; `0` when no ranking
    /// was supplied or the file carries no hotspot signal.
    blast: i64,
    gap: TestGap,
}

/// The blast-radius score of one gap ([FR-GV-17]): caller fan-in × the
/// containing file's hotspot score. A file absent from `hotspot_ranks` (or no
/// ranking supplied at all) contributes weight `0` — honest absence, never a
/// fabricated number ([NFR-CC-04]); such gaps sink to the file/name-ordered
/// tail. Saturating so a pathological graph can never overflow the product.
fn blast_radius(fan_in: u64, file: &str, hotspot_ranks: Option<&HashMap<String, i64>>) -> i64 {
    let weight = hotspot_ranks
        .and_then(|ranks| ranks.get(file))
        .copied()
        .unwrap_or(0);
    i64::try_from(fan_in)
        .unwrap_or(i64::MAX)
        .saturating_mul(weight)
}

/// Order the untested set: by blast radius descending (most-urgent first) when a
/// hotspot ranking was supplied by the façade (`ranked`, [FR-GV-17]), else by the
/// FR-GV-08 file/name order. Both paths tie-break on file then name, so the order
/// is deterministic ([NFR-RA-06]) and the degraded path is byte-identical to the
/// historical file/name order. When `ranked` is false every `blast` is `0`, so
/// the file/name order is authoritative.
fn order_untested(mut scored: Vec<ScoredGap>, ranked: bool) -> Vec<TestGap> {
    if ranked {
        scored.sort_by(|a, b| {
            b.blast
                .cmp(&a.blast)
                .then_with(|| a.gap.file.cmp(&b.gap.file))
                .then_with(|| a.gap.name.cmp(&b.gap.name))
        });
    } else {
        scored.sort_by(|a, b| {
            a.gap
                .file
                .cmp(&b.gap.file)
                .then_with(|| a.gap.name.cmp(&b.gap.name))
        });
    }
    scored.into_iter().map(|s| s.gap).collect()
}

/// `doc_gaps` — the read-only analog of `test_gaps` ([FR-GV-14]): exported
/// Function/Method symbols referenced by no `DocSection` over `DocReference`
/// edges, with the mandatory honesty caveat.
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
    let mut report = doctor_report(structural_check(engine)?, admission_tripwire(engine)?);
    // FR-IX-11: warn (path + reason) when a documentation directory-symlink exists
    // under the doc-include set but ended up unindexed. Diagnostic only — it does
    // not touch `report.ok`, so it never changes `doctor`'s exit status.
    report.doc_symlink_warnings = doc_symlink_warnings(engine)?;
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
        message,
    }
}

/// The lexical cap on the leaked/orphaned symbol samples a [`VerifyReport`]
/// lists ([FR-GV-19], [NFR-RA-06]): the `*_total` counts stay exact; only the
/// listed sample is bounded so the CLI/MCP/web read-model is a fixed size even on
/// a badly drifted graph. Matches the `test_gaps`/`doc_gaps` default cap.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
const VERIFY_SAMPLE_CAP: usize = 50;

/// One store's whole-graph census for [`verify`]: the row counts plus the set of
/// every node's canonical symbol, read in one pass over a read-only connection.
///
/// The counts drive the numeric deltas (they catch a Channel-A duplicate — two
/// nodes for one `symbol_id` — that the deduplicated symbol *set* would hide),
/// while the symbol set drives the leaked/orphaned diff (Channel B).
fn census(store: &dyn GraphStore) -> Result<(VerifyCensus, BTreeSet<String>)> {
    let counts = store.counts()?;
    let symbols = store
        .all_nodes()?
        .into_iter()
        .map(|n| n.symbol.as_str().to_string())
        .collect::<BTreeSet<String>>();
    Ok((
        VerifyCensus {
            files: counts.files,
            nodes: counts.nodes,
            edges: counts.edges,
        },
        symbols,
    ))
}

/// `verify` — the on-demand **deep** consistency check (CR-052, [FR-GV-19],
/// [NFR-RA-06], [ADR-46]): reindex the project into a throwaway shadow store via
/// the always-purge [`index`](../../../docs/specs/requirements/FR-IX-01.md) path,
/// census both stores, and diff node/edge/file counts + symbol sets against the
/// live graph — reporting any drift with a capped sample of leaked (live-only)
/// and orphaned (reindex-only) symbols. Embeds the fast structural check
/// ([FR-GV-18]) as `structural`.
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
pub(crate) fn verify(engine: &Engine) -> Result<VerifyReport> {
    let (runtime, registry, config) = engine.pipeline_ctx()?;

    // 1. Live census + fast structural check + the FR-GV-20 admission read, in
    //    ONE read-only checkout so the counts, symbol set, structural verdict,
    //    and indexed-file list are a consistent snapshot. The reader pool never
    //    mutates the live store (FR-GV-19).
    let (live, live_symbols, structural, indexed_files) = runtime.submit_read(|store| {
        let (census, symbols) = census(store)?;
        Ok((census, symbols, store.structural_check()?, store.indexed_files()?))
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
    let (reindex, reindex_symbols) = {
        let shadow_rt =
            crate::pipeline::shadow_reindex(registry, engine.root(), &config, &shadow)?;
        let census = shadow_rt.submit_read(census)?;
        drop(shadow_rt);
        census
    };
    // `shadow` guard drops at end of function → db + `-wal`/`-shm` removed.

    // 3. Diff. Counts drive the numeric deltas; the symbol-set difference names
    //    the leaked (live-only) and orphaned (reindex-only) symbols. A BTreeSet
    //    difference is already lexically ordered (NFR-RA-06), so the sample is
    //    deterministic before it is capped.
    let node_delta = live.nodes as i64 - reindex.nodes as i64;
    let edge_delta = live.edges as i64 - reindex.edges as i64;
    let file_delta = live.files as i64 - reindex.files as i64;

    let leaked: Vec<String> = live_symbols.difference(&reindex_symbols).cloned().collect();
    let orphaned: Vec<String> = reindex_symbols
        .difference(&live_symbols)
        .cloned()
        .collect();
    let leaked_total = leaked.len() as u64;
    let orphaned_total = orphaned.len() as u64;

    let ok = node_delta == 0
        && edge_delta == 0
        && file_delta == 0
        && leaked_total == 0
        && orphaned_total == 0
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
