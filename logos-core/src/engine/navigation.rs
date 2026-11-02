//! [`Navigation`] — the navigation tools ([FR-NV-01]..[FR-NV-13], S-013) and
//! the runtime they read through, reached by [`Engine::navigation`] (S-829,
//! [CR-259]).
//!
//! Every tool is a best-effort-fresh point query ([ADR-11], NFR-DM-02): it reads
//! exclusively from the runtime's read-only pool and NEVER reconciles the
//! working tree per call ([FR-RC-05]). The surface is infallible ([ADR-14]): a
//! failed read degrades to an empty read-model carrying the reason in
//! `warnings`; an unknown symbol yields an empty result plus "did you mean"
//! suggestions (FR-NV-09) — never an error.
//!
//! [FR-NV-01]: ../../../docs/specs/requirements/FR-NV-01.md
//! [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md
//! [FR-RC-05]: ../../../docs/specs/requirements/FR-RC-05.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::sync::atomic::Ordering;

use anyhow::{anyhow, Result};

use super::{Engine, Pipeline, LOG_TARGET};
use crate::model::NodeKind;
use crate::models::navigation::{
    AffectedResult, BranchOverlapResult, CalleesResult, CallersResult, ContextBundle, ExploreResult,
    ImpactIntersectionResult, ImpactResult, ImplementorsResult, NodeInfo, PrecedentResult,
    ReferencingDocsResult, SearchResult,
};
use crate::navigate;
use crate::observability::{traced, traced_with, CallOutcome, Tool};
use crate::runtime::Runtime;

/// The navigation tools over one engine's graph, reached by
/// [`Engine::navigation`].
#[derive(Clone, Copy)]
pub struct Navigation<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Navigation<'e> {
    /// FTS5 full-text search over the code graph, optionally filtered by node
    /// `kind`, at most `limit` (default 20) hits (FR-NV-01).
    ///
    /// Sub-100 ms p95 budget (NFR-PE-01); best-effort-fresh (NFR-DM-02).
    pub fn search(&self, query: &str, kind: Option<NodeKind>, limit: Option<usize>) -> SearchResult {
        served(Tool::Search, query, || navigate::search(self.engine, query, kind, limit))
    }

    /// Deterministic context bundle for a `task` description (FR-NV-02).
    ///
    /// One call replaces several ad-hoc reads — the token-saving thesis
    /// (AS-02): FTS5-seed → 1-hop expand (OQ-05) → centrality rank → cap at
    /// `max_nodes` (default 25). Code only when `include_code` is `true`.
    pub fn context(&self, task: &str, max_nodes: Option<usize>, include_code: bool) -> ContextBundle {
        served(Tool::Context, task, || navigate::context(self.engine, task, max_nodes, include_code))
    }

    /// Neighbourhood exploration around `query`: source grouped by file, at
    /// most `max_files` (default 10) groups (FR-NV-03).
    pub fn explore(&self, query: &str, max_files: Option<usize>) -> ExploreResult {
        served(Tool::Explore, query, || navigate::explore(self.engine, query, max_files))
    }

    /// Full node info for a single `symbol`: metadata, immediate edges, and
    /// code opt-in (FR-NV-04).
    pub fn node(&self, symbol: &str, include_code: bool) -> NodeInfo {
        served(Tool::Node, symbol, || navigate::node(self.engine, symbol, include_code))
    }

    /// Direct callers of `symbol`, at most `limit` (default 50) (FR-NV-05).
    pub fn callers(&self, symbol: &str, limit: Option<usize>) -> CallersResult {
        answer(Tool::Callers, symbol, traced_with(Tool::Callers, || navigate::callers(self.engine, symbol, limit), CallOutcome::outcome))
    }

    /// Direct callees of `symbol`, at most `limit` (default 50) (FR-NV-05).
    pub fn callees(&self, symbol: &str, limit: Option<usize>) -> CalleesResult {
        served(Tool::Callees, symbol, || navigate::callees(self.engine, symbol, limit))
    }

    /// Transitive impact of changing `symbol`, both directions labeled
    /// (FR-NV-06, DL-03): upstream "breaks if changed", downstream
    /// "depends on", bounded by `depth` (default 3).
    pub fn impact(&self, symbol: &str, depth: Option<usize>) -> ImpactResult {
        answer(Tool::Impact, symbol, traced_with(Tool::Impact, || navigate::impact(self.engine, symbol, depth), CallOutcome::outcome))
    }

    /// Which planned work items collide, and on what ([FR-NV-11], CR-114).
    ///
    /// Given work items each naming the symbols it intends to change, reports
    /// every pair whose transitive impact sets intersect (naming the shared
    /// symbols) and every pair that is safely parallel. The payload carries its
    /// own coverage limits ([NFR-CC-04]): impact is computed over the indexed
    /// graph, so an unindexed surface cannot contribute an intersection.
    ///
    /// [FR-NV-11]: ../../../docs/specs/requirements/FR-NV-11.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// Each spec is `<id>=<symbol>[,<symbol>…]` — parsed in the core, so every
    /// surface accepts the identical spelling and none of them parses anything
    /// of its own ([ADR-01]). A malformed spec becomes a warning on the payload,
    /// never an error.
    pub fn impact_intersection(&self, items: &[String], depth: Option<usize>) -> ImpactIntersectionResult {
        let outcome = traced(Tool::ImpactIntersection, || navigate::impact_intersection(self.engine, items, depth));
        or_degraded(Tool::ImpactIntersection, outcome, |warning| navigate::intersection_degraded(depth, warning))
    }

    /// Nodes structurally analogous to `target` — a symbol or a
    /// project-relative file ([FR-NV-12], CR-114).
    ///
    /// The precedent question a plan-driven workflow asks once scope is
    /// settled: *which existing code plays the same structural role as the
    /// thing I am about to write*. Analogy is three named graph facts — a
    /// shared supertype, a shared registration, a matching call shape — and
    /// every result names which of them it matched and through which nodes.
    /// There is no score ([FR-NV-12] AC 1/AC 2).
    ///
    /// An empty answer always states its reason rather than relaxing the
    /// notion into a guess ([FR-NV-12] AC 4, [NFR-CC-04]). `limit` defaults to
    /// 20 and is clamped to 100.
    ///
    /// [FR-NV-12]: ../../../docs/specs/requirements/FR-NV-12.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn precedent(&self, target: &str, limit: Option<usize>) -> PrecedentResult {
        let outcome =
            traced_with(Tool::Precedent, || navigate::precedent(self.engine, target, limit), CallOutcome::outcome);
        or_degraded(Tool::Precedent, outcome, |warning| navigate::precedent_degraded(target, warning))
    }

    /// Which git refs collide, and what a merge did not carry ([FR-NV-13],
    /// CR-114).
    ///
    /// Given a set of refs, reports the symbols more than one of them modifies,
    /// naming the refs. When `merge` names a stated merge result, also reports
    /// the symbols and files a ref changed that the result does not carry — a
    /// clean merge is not a complete merge. The payload carries its own coverage
    /// limits ([NFR-CC-04]): hunks are attributed to the spans of the indexed
    /// snapshot, so a symbol outside the indexed set cannot be reported.
    ///
    /// Refs are passed to git verbatim and are never interpreted here, so every
    /// surface accepts whatever `git rev-parse` accepts ([ADR-01]). An
    /// unresolvable ref becomes a warning on the payload, never an error.
    ///
    /// [FR-NV-13]: ../../../docs/specs/requirements/FR-NV-13.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-01]: ../../../docs/specs/architecture/decisions/ADR-01.md
    pub fn branch_overlap(&self, refs: &[String], base: Option<&str>, merge: Option<&str>) -> BranchOverlapResult {
        let outcome = traced(Tool::BranchOverlap, || navigate::branch::branch_overlap(self.engine, refs, base, merge));
        or_degraded(Tool::BranchOverlap, outcome, navigate::branch::overlap_degraded)
    }

    /// Which code implements a documentation/requirement node (FR-NV-10,
    /// S-037): the code symbols a doc node points at over doc→code edges.
    /// Empty (never an error) when no such edge exists; "did you mean"
    /// suggestions when the doc node is unknown (FR-NV-09).
    pub fn implements(&self, doc: &str) -> ImplementorsResult {
        served(Tool::Implements, doc, || navigate::implements(self.engine, doc))
    }

    /// Which documents reference a code symbol (FR-NV-10, S-037): the doc
    /// sections pointing at the symbol over doc→code edges. Empty (never an
    /// error) when no doc references it; suggestions when the symbol is
    /// unknown (FR-NV-09).
    pub fn referencing_docs(&self, symbol: &str) -> ReferencingDocsResult {
        served(Tool::ReferencingDocs, symbol, || navigate::referencing_docs(self.engine, symbol))
    }

    /// Whole reverse-transitive closure of files affected by a changed set
    /// (FR-CL-04, DL-08): every file depending — directly or transitively, over
    /// calls/imports/references — on any of `files`. `tests_only` narrows the
    /// closure to test-marked files.
    pub fn affected(&self, files: &[String], tests_only: bool) -> AffectedResult {
        let outcome =
            traced_with(Tool::Affected, || navigate::affected(self.engine, files, tests_only), CallOutcome::outcome);
        or_degraded(Tool::Affected, outcome, |warning| AffectedResult {
            changed: files.to_vec(),
            tests_only,
            warnings: vec![warning],
            ..AffectedResult::default()
        })
    }

    /// The runtime a navigation tool reads through, after the **one-time**
    /// auto-index prologue ([FR-IX-07]): the very first navigation call on a
    /// never-indexed graph transparently builds it; every later call skips the
    /// check entirely (memoised), so the steady-state freshness contract is
    /// exactly [ADR-11]'s "never reconcile per call" ([FR-RC-05]).
    ///
    /// A failed prologue (e.g. no plugin registry) is logged by
    /// [`ensure_indexed`](super::Pipeline::ensure_indexed)'s degradation path
    /// and **not** retried — navigation still serves whatever graph exists,
    /// best-effort.
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no RO pool).
    pub(crate) fn runtime(&self) -> Result<&'e Runtime> {
        let runtime = self.runtime_no_prologue()?;
        if !self.engine.nav_prologue_done.swap(true, Ordering::AcqRel) {
            let pipeline: Pipeline<'_> = self.engine.pipeline();
            // Degrades internally (warnings + stderr), never panics; an
            // actual index advances the sync stamp itself. A racing second
            // caller sees `true` and proceeds WITHOUT waiting for the index
            // to finish — deliberate: best-effort-fresh ([ADR-11]) allows
            // serving whatever graph exists at read time.
            let _ = pipeline.ensure_indexed();
            // [FR-SY-08]/[ADR-20] (S-041): the one-time prologue also runs the
            // fingerprint-gated config-change purge before serving, so a read
            // after a config narrowing never surfaces now-unadmitted symbols.
            // A scoped exception to [FR-RC-05] — it purges on a *detected*
            // config change at most once per change, not a per-call working-
            // tree reconcile. When `ensure_indexed` above ran a full index it
            // already recorded the fingerprint, so this gates to a no-op (one
            // comparison, no discovery walk). Degrades like the auto-index
            // prologue: logged, never fatal — navigation still serves.
            pipeline.run_prologue_purge();
        }
        Ok(runtime)
    }

    /// The navigation runtime *without* the auto-index prologue — for `status`
    /// (which must report an unindexed graph, not silently build one) and for
    /// follow-up reads inside a tool that already ran the prologue. Public for
    /// the extensions that read the graph over `&Engine` without forcing an
    /// index — the wiki's anchor resolution ([FR-DS-04]).
    ///
    /// # Errors
    /// Returns an error for a transient [`Engine::open`] engine (no RO pool).
    ///
    /// [FR-DS-04]: ../../../docs/specs/requirements/FR-DS-04.md
    pub fn runtime_no_prologue(&self) -> Result<&'e Runtime> {
        self.engine.runtime.as_ref().ok_or_else(|| {
            anyhow!(
                "navigation requires a long-lived engine (Engine::start); a transient \
                 Engine::open engine has no read-only pool to serve reads (ADR-02)"
            )
        })
    }
}

/// A navigation read-model that can carry its own failure ([ADR-14]): the
/// question it was asked and the warning saying why it is empty.
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
trait Degraded {
    /// This read-model for `query`, empty but for `warning`.
    fn degraded(query: &str, warning: String) -> Self;
}

/// [`Degraded`] for read-models that name what they were asked in one field.
macro_rules! degraded_by_query {
    ($($model:ty => $field:ident),+ $(,)?) => {$(
        impl Degraded for $model {
            fn degraded(query: &str, warning: String) -> Self {
                Self { $field: query.to_string(), warnings: vec![warning], ..Self::default() }
            }
        }
    )+};
}

degraded_by_query!(
    SearchResult => query,
    ContextBundle => task,
    ExploreResult => query,
    NodeInfo => query,
    CallersResult => query,
    CalleesResult => query,
    ImpactResult => query,
    ImplementorsResult => query,
    ReferencingDocsResult => query,
);

/// `f`'s answer under `tool`'s span, or — when the read failed — `query`'s
/// read-model carrying the reason.
fn served<T: Degraded>(tool: Tool, query: &str, f: impl FnOnce() -> Result<T>) -> T {
    answer(tool, query, traced(tool, f))
}

/// `outcome`, or `query`'s read-model carrying why it failed. A tool whose
/// answer records what it answered ([FR-OB-14]) is traced at its call site:
/// `traced_with` takes its `Tool` literally, so the classification census reads
/// which tools are classified.
///
/// [FR-OB-14]: ../../../docs/specs/requirements/FR-OB-14.md
fn answer<T: Degraded>(tool: Tool, query: &str, outcome: Result<T>) -> T {
    or_degraded(tool, outcome, |warning| T::degraded(query, warning))
}

/// `outcome`, or — when it failed — the read-model `fallback` builds around the
/// reason, which is logged first ([ADR-14] defers typed errors): the
/// navigation surface never fails a call.
///
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
fn or_degraded<T>(tool: Tool, outcome: Result<T>, fallback: impl FnOnce(String) -> T) -> T {
    outcome.unwrap_or_else(|err| {
        let name = tool.as_str();
        tracing::warn!(target: LOG_TARGET, "{name} failed: {err:#}");
        fallback(format!("{name} failed: {err}"))
    })
}
