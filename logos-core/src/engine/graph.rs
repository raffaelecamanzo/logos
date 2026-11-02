//! [`Graph`] — the code graph's state, reached by [`Engine::graph`] (S-829,
//! [CR-259]): the hydrated petgraph views and the `last_sync_at` clock they
//! are keyed by ([ADR-04], [ADR-05], S-009), the index's status and the call
//! residue it reports ([FR-NV-07]), the graph canvas's elements, the language
//! composition, the registered languages and the usage stats.
//!
//! [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
//! [ADR-04]: ../../../docs/specs/architecture/decisions/ADR-04.md
//! [ADR-05]: ../../../docs/specs/architecture/decisions/ADR-05.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use super::{Engine, ResidueByLanguage, ResidueKey, LOG_TARGET};
use crate::hydrate::{Granularity, GraphView, HydrationStats, Scope, SyncStamp};
use crate::model::EdgeKind;
use crate::models::navigation::{GraphElements, GraphGranularity, GraphLayer, LanguageComposition, StatusInfo};
use crate::models::quality::{LanguageDescriptor, LanguageReach, LanguagesInfo, SkippedLanguage, StatsInfo};
use crate::observability::Tool;

/// The code graph's state and inventory over one engine, reached by
/// [`Engine::graph`].
#[derive(Clone, Copy)]
pub struct Graph<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Graph<'e> {
    /// Hydrate (or reuse the cached) petgraph view of the whole project at
    /// `granularity` ([FR-DB-05], [FR-DB-06], [ADR-05]).
    ///
    /// The first call at a given `last_sync_at` reads the graph from the RO pool
    /// and builds the petgraph; subsequent calls at the same stamp return the
    /// cached `Arc<GraphView>` without touching SQLite ([NFR-PE-07]). Advancing
    /// the sync stamp ([`Self::advance_sync_stamp`]) invalidates the cache, so
    /// the next call re-hydrates.
    ///
    /// # Errors
    /// Returns an error if this is a transient [`Engine::open`] engine (hydration
    /// needs the runtime's RO pool), or if the read fails.
    ///
    /// [FR-DB-05]: ../../../docs/specs/requirements/FR-DB-05.md
    /// [FR-DB-06]: ../../../docs/specs/requirements/FR-DB-06.md
    /// [NFR-PE-07]: ../../../docs/specs/requirements/NFR-PE-07.md
    pub fn hydrate(&self, granularity: Granularity) -> Result<Arc<GraphView>> {
        let runtime = self.engine.runtime.as_ref().ok_or_else(|| {
            anyhow!(
                "graph hydration requires a long-lived engine (Engine::start); \
                 a transient Engine::open engine has no read-only pool to hydrate from"
            )
        })?;
        self.engine.hydration
            .get_or_build(runtime, Scope::Project, granularity, self.sync_stamp())
    }

    /// The current in-memory `last_sync_at` ([ADR-04]). Starts at
    /// [`SyncStamp::INITIAL`].
    pub fn sync_stamp(&self) -> SyncStamp {
        SyncStamp(self.engine.sync_stamp.load(Ordering::Acquire))
    }

    /// Advance `last_sync_at`, invalidating the hydration cache on the next
    /// [`Self::hydrate`] ([ADR-04]).
    ///
    /// The indexing pipeline ([S-010]) calls this after committing a write batch
    /// so a re-hydration reflects the new graph state. Returns the new stamp.
    ///
    /// [S-010]: ../../../docs/planning/journal.md#s-010-indexing-and-incremental-sync-pipeline
    pub fn advance_sync_stamp(&self) -> SyncStamp {
        // fetch_add returns the previous value; the new stamp is +1.
        SyncStamp(self.engine.sync_stamp.fetch_add(1, Ordering::AcqRel) + 1)
    }

    /// A snapshot of hydration-cache effectiveness for `stats` ([NFR-PE-07]).
    pub fn hydration_stats(&self) -> HydrationStats {
        self.engine.hydration.stats()
    }

    /// Current index and sync health of the code graph (FR-NV-07): counts,
    /// store size, resolution coverage, and the freshness statement.
    ///
    /// Infallible at the surface like the rest of navigation: a failed read
    /// degrades to a **defaulted** read-model carrying the reason in
    /// `warnings` (ADR-14). A caller that must tell that degradation apart from
    /// a genuinely empty graph — both report `indexed: false` — needs
    /// [`try_status`](Self::try_status) instead.
    pub fn status(&self) -> StatusInfo {
        self.try_status().unwrap_or_else(|err| {
            tracing::warn!(target: LOG_TARGET, "status failed: {err:#}");
            StatusInfo {
                warnings: vec![format!("status failed: {err}")],
                ..StatusInfo::default()
            }
        })
    }

    /// [`status`](Self::status) without the infallible degradation — the same
    /// traced read, returning its `Err` ([FR-NV-07], [NFR-CC-04]).
    ///
    /// `status`'s defaulted fallback is right for a single-root surface, where
    /// the warning is displayed beside the numbers it explains. It is wrong for
    /// the workspace fan-out: a defaulted `StatusInfo` reports `indexed: false`,
    /// which is indistinguishable from an honestly empty graph, so a member
    /// whose freshness read *failed* would be labeled `deferred` — "never
    /// attempted" — by [`federation::warm_state`](crate::federation::warm_state)
    /// ([BR-44]). Fanning this instead folds that failure into the per-member
    /// `error` channel, where it reads `degraded` like any other failed attempt.
    ///
    /// # Errors
    /// Propagates the store/runtime fault the read hit.
    ///
    /// [BR-44]: ../../../docs/specs/software-spec.md#327-workspace-federation
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn try_status(&self) -> Result<StatusInfo> {
        crate::observability::traced(Tool::Status, || crate::navigate::status(self.engine))
    }

    /// [`try_status`](Self::try_status), with the facts a workspace re-reads
    /// this member's reference classes by (S-637) — from the same read, so a
    /// workspace `status` adds no walk for them.
    pub(crate) fn try_status_with_reference_facts(
        &self,
    ) -> Result<(StatusInfo, crate::resolve::reference_classes::MemberReferenceFacts)> {
        crate::observability::traced(Tool::Status, || crate::navigate::status_with_reference_facts(self.engine))
    }

    /// Read-only graph-elements accessor feeding the web surface's interactive
    /// graph canvas ([FR-UI-08], [ADR-29]): a presentation-shaped nodes+edges
    /// snapshot of the hydrated graph, whole-graph (`seed = None`) or seed-scoped
    /// (the seed's connected neighbourhood), bounded by a visible-element `cap`
    /// (default 250) with the elided remainder reported, never silently dropped
    /// ([NFR-CC-04]).
    ///
    /// `layers` / `edge_types` are the optional **server-side re-budgeting
    /// filters** (S-122, [FR-UI-15]): when present they narrow the candidate set
    /// *before* the degree-rank+truncate, so deselecting a layer or edge type
    /// backfills the freed visible budget with previously-elided nodes of the
    /// remaining scope (the graph stays full, not merely smaller). `None` ⇒ no
    /// filter (all layers / all edge types); an empty slice ⇒ filter everything
    /// out (the honest empty graph). See [`navigate::graph_elements`] for the exact
    /// re-budgeting semantics.
    ///
    /// `granularity` is the optional **semantic cluster-zoom tier** (S-124,
    /// [FR-UI-15], [ADR-36]): `module`/`file`/`symbol` select the existing
    /// module-rollup / file-rollup / visualization hydration view ([ADR-34]). `None`
    /// is the symbol tier — the pre-S-124 behaviour. The rollup tiers are the code
    /// subgraph (docs/artifacts excluded, [FR-DG-06]) and feed no metric path, so
    /// every tier is metric-neutral ([FR-QM-08]).
    ///
    /// `intent_overlay` is the **bounded documentation-intent overlay** (S-128,
    /// [FR-UI-16], [ADR-37]): `false` (the default) is byte-identical to the
    /// pre-S-128 snapshot; `true` admits the governing-doc nodes adjacent (via the
    /// existing `DocReference`/`TracesTo` edges) to the kept code nodes up to a
    /// separate reserved budget computed outside the structural ranking, so the
    /// overlay can never starve the code anchors (the [CR-014] doc-flooding guard).
    /// Presentation-only over the visualization view — metric-neutral and read-only.
    ///
    /// A **pure reader** over the cached hydrated view — no compute-and-persist;
    /// calling it once or repeatedly mutates no store, the read-only discipline
    /// [ADR-28] established for `latest_*` and [ADR-29] extends to the graph
    /// ([FR-UI-03]). Consumed only by the [web-surface]; CLI/MCP are unaffected.
    ///
    /// [FR-UI-08]: ../../../docs/specs/requirements/FR-UI-08.md
    /// [FR-UI-15]: ../../../docs/specs/requirements/FR-UI-15.md
    /// [FR-UI-16]: ../../../docs/specs/requirements/FR-UI-16.md
    /// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    /// [FR-DG-06]: ../../../docs/specs/requirements/FR-DG-06.md
    /// [FR-QM-08]: ../../../docs/specs/requirements/FR-QM-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [ADR-29]: ../../../docs/specs/architecture/decisions/ADR-29.md
    /// [ADR-34]: ../../../docs/specs/architecture/decisions/ADR-34.md
    /// [ADR-36]: ../../../docs/specs/architecture/decisions/ADR-36.md
    /// [ADR-37]: ../../../docs/specs/architecture/decisions/ADR-37.md
    /// [CR-014]: ../../../docs/requests/CR-014-context-seed-doc-flooding.md
    /// [web-surface]: ../../../docs/specs/architecture/components/web-surface.md
    /// [`navigate::graph_elements`]: crate::navigate::graph_elements
    pub fn elements(
        &self,
        seed: Option<&str>,
        cap: Option<usize>,
        layers: Option<&[GraphLayer]>,
        edge_types: Option<&[EdgeKind]>,
        granularity: Option<GraphGranularity>,
        intent_overlay: bool,
    ) -> GraphElements {
        crate::observability::traced(Tool::GraphElements, || {
            crate::navigate::graph_elements(
                self.engine,
                seed,
                cap,
                layers,
                edge_types,
                granularity,
                intent_overlay,
            )
        })
        .unwrap_or_else(|err| {
            tracing::warn!(target: LOG_TARGET, "graph_elements failed: {err:#}");
            GraphElements {
                seed: seed.map(str::to_string),
                granularity: granularity.unwrap_or_default(),
                warnings: vec![format!("graph_elements failed: {err}")],
                ..GraphElements::default()
            }
        })
    }

    /// Aggregated usage/perf stats from `telemetry.db` over a trailing window
    /// of `window_days` (default 7, [FR-OB-04]): per-tool/surface call counts,
    /// latency p50/p95/p99, and the estimated reads + tokens saved by
    /// navigation — the dogfood metric ([NFR-OO-03]), honestly an estimate.
    ///
    /// Works on both engine flavours (it reads `telemetry.db` directly, not
    /// the graph runtime). A project with no telemetry yet degrades to a
    /// zeroed read-model carrying the reason in `warnings` (ADR-14 posture).
    ///
    /// [FR-OB-04]: ../../../docs/specs/requirements/FR-OB-04.md
    /// [NFR-OO-03]: ../../../docs/specs/requirements/NFR-OO-03.md
    pub fn stats(&self, window_days: Option<u32>) -> StatsInfo {
        let mut info = crate::observability::traced(Tool::Stats, || {
            crate::observability::stats(&self.engine.root, window_days)
        })
        .unwrap_or_else(|err| {
            tracing::warn!(target: LOG_TARGET, "stats failed: {err:#}");
            // An unreadable store degrades like a missing one ([FR-OB-11]): the
            // window the caller asked for is echoed, and the attribution coverage
            // limits are still stated. They are structural — properties of
            // `daily_rollup`'s schema, not of the data — so they hold whether or
            // not the store could be read, and a surface rendering them must not
            // fall silent on exactly the payload least worth trusting
            // ([NFR-CC-04]).
            let window_days =
                window_days.unwrap_or(crate::observability::DEFAULT_STATS_WINDOW_DAYS);
            StatsInfo {
                window_days,
                attribution_coverage: crate::observability::attribution_coverage(window_days),
                warnings: vec![format!("stats failed: {err}")],
                ..StatsInfo::default()
            }
        });
        // Per-relation-class cross-artifact binding counts (CR-011, FR-OB-04,
        // FR-CG-11) are a live-graph property, not telemetry: read the ledger
        // best-effort from `logos.db` and merge. A missing or unreadable graph
        // leaves the map empty — the stats surface stays infallible (ADR-14).
        match self.artifact_binding_counts() {
            Ok(by_relation) => info.artifact_bindings = by_relation,
            Err(err) => tracing::debug!(target: LOG_TARGET, "artifact binding counts unavailable: {err:#}"),
        }
        info
    }

    /// Per-relation-class cross-artifact binding counts read from the live graph
    /// ledger (CR-011, [FR-CG-11]) — the `stats` surface's artifact half.
    ///
    /// Works on both engine flavours by opening `logos.db` read-only (like
    /// [`stats`](Self::stats) reads `telemetry.db`), independent of the graph
    /// runtime. An un-indexed project (no `logos.db`) yields an empty map, not an
    /// error.
    ///
    /// # Errors
    /// Returns an error only if an existing store is unreadable/corrupt; the
    /// caller degrades that to an empty map.
    ///
    /// [FR-CG-11]: ../../../docs/specs/requirements/FR-CG-11.md
    fn artifact_binding_counts(
        &self,
    ) -> Result<std::collections::BTreeMap<String, crate::models::pipeline::RelationCoverage>> {
        let db_path = self.engine.root.join(".logos").join("logos.db");
        if !db_path.is_file() {
            return Ok(std::collections::BTreeMap::new());
        }
        let store = crate::graph_store::SqliteGraphStore::open_readonly(&db_path)?;
        Ok(crate::resolve::coverage(&store)?.by_relation)
    }

    /// Registered languages from the plugin substrate (FR-PL-06).
    ///
    /// A long-lived [`Engine::start`] engine maps from the
    /// [`LanguageRegistry`](crate::plugin::LanguageRegistry) cached at startup
    /// (ADR-04, NFR-PE-05) — no per-call descriptor parse / query recompile. A
    /// transient [`Engine::open`] engine has no cached registry, so it loads one
    /// on demand (resolving any on-disk query overrides under
    /// `<root>/.logos/plugins/`).
    ///
    /// A load failure (a malformed descriptor, or a user-supplied override query
    /// that fails to compile) is reported to stderr naming the
    /// file (FR-PL-02) and surfaced in [`LanguagesInfo::load_error`] rather than
    /// a panic, since the Engine surface is infallible until error types land
    /// (ADR-14). The failure is named in the read-model itself (S-340,
    /// [NFR-CC-04]) rather than degraded to an empty-but-healthy-looking
    /// listing — the failure mode that hid the [FR-WS-08] capture gap: a
    /// registry-wide load failure and a project with no plugins at all were
    /// otherwise indistinguishable.
    ///
    /// [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn languages(&self) -> LanguagesInfo {
        use crate::plugin::LanguageRegistry;

        crate::observability::traced_infallible(Tool::Languages, || {
            // Prefer the cached registry (the started-engine path); fall back
            // to a fresh load for a transient engine that holds none.
            if let Some(registry) = self.engine.registry.as_ref() {
                return languages_from(registry);
            }

            match LanguageRegistry::load(&self.engine.root) {
                Ok(registry) => languages_from(&registry),
                Err(err) => {
                    tracing::warn!(target: LOG_TARGET, "could not load plugin registry: {err}");
                    LanguagesInfo {
                        load_error: Some(err.to_string()),
                        ..LanguagesInfo::default()
                    }
                }
            }
        })
    }

    /// The per-project **language composition** ([FR-UI-10], [CR-021]): the
    /// languages actually present in the indexed graph, each with its graph
    /// node/symbol count and contributing-file count, in deterministic order
    /// (node count descending, then language ascending — [NFR-RA-06]). Distinct
    /// from [`languages`](Self::languages), which lists every *registered*
    /// grammar regardless of project use — here a registered-but-unused grammar
    /// is absent. Backs the Dashboard's project-only Languages card ([FR-UI-09]).
    ///
    /// A pure reader that computes-and-persists nothing ([ADR-28], [FR-UI-03]):
    /// it opens `logos.db` read-only — so, like [`latest_*`](super::Readouts::latest_metrics)
    /// it works on both engine flavours and never touches the writer — and an
    /// un-indexed root (no `logos.db`) returns an empty composition the view
    /// renders as its honest empty state ([NFR-CC-04]).
    ///
    /// # Errors
    /// Returns an error only when an existing `logos.db` is unreadable or at the
    /// wrong schema version; an absent store is the empty (un-indexed) case, not
    /// an error.
    ///
    /// [FR-UI-10]: ../../../docs/specs/requirements/FR-UI-10.md
    /// [FR-UI-09]: ../../../docs/specs/requirements/FR-UI-09.md
    /// [FR-UI-03]: ../../../docs/specs/requirements/FR-UI-03.md
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-28]: ../../../docs/specs/architecture/decisions/ADR-28.md
    /// [CR-021]: ../../../docs/requests/CR-021-dashboard-redesign-quality-coverage-rollups.md
    pub fn language_composition(&self) -> Result<LanguageComposition> {
        crate::observability::traced(Tool::LanguageComposition, || {
            let db_path = self.engine.root.join(".logos").join("logos.db");
            if !db_path.is_file() {
                // Un-indexed root: an empty composition, not an error ([FR-UI-10]).
                return Ok(LanguageComposition::default());
            }
            let store = crate::graph_store::SqliteGraphStore::open_readonly(&db_path)?;
            use crate::graph_store::GraphStore;
            Ok(LanguageComposition {
                languages: store.language_composition()?,
            })
        })
    }

    /// The per-language call residue `status` reports ([FR-RS-09]), walked at
    /// most once per graph revision, schema version and `[resolution]` section
    /// (CR-201, [`ResidueKey`]).
    ///
    /// The revision-keyed memo shape the wiki's native tier also follows (now
    /// in the wiki extension's crate): read the key, answer a hit
    /// from the memo, and on a miss run
    /// [`call_residue_by_language`](crate::resolve::call_residue_by_language) on
    /// the read-only pool and keep its answer. The key is read **before** the
    /// walk, and every writer advances the revision only after its rows commit,
    /// so a walk that overlaps a write is cached under the older revision and the
    /// next call misses — never the reverse. A failed read caches nothing.
    ///
    /// # Errors
    /// Returns an error for a transient or registry-less engine, or when the
    /// graph cannot be read.
    ///
    /// [FR-RS-09]: ../../../docs/specs/requirements/FR-RS-09.md
    pub(crate) fn call_residue(
        &self,
        resolution: &crate::config::Resolution,
    ) -> Result<ResidueByLanguage> {
        let runtime = self.engine.navigation().runtime_no_prologue()?;
        let registry = self
            .engine
            .registry()
            .context("an engine without its language registry states no call residue")?;
        let key = runtime.submit_read(|store| {
            Ok(ResidueKey {
                graph_revision: store.graph_revision()?,
                schema_version: store.store_health()?.schema_version,
                resolution: resolution.clone(),
            })
        })?;

        {
            let cache = self.engine.call_residue.lock().expect("call-residue cache lock");
            if let Some((cached_key, residues)) = cache.as_ref() {
                if *cached_key == key {
                    return Ok(residues.clone());
                }
            }
        }

        self.engine.call_residue_walks.fetch_add(1, Ordering::Relaxed);
        let policy = key.resolution.policy;
        let residues = runtime.submit_read(move |store| {
            crate::resolve::call_residue_by_language(store, registry, policy)
        })?;
        let mut cache = self.engine.call_residue.lock().expect("call-residue cache lock");
        *cache = Some((key, residues.clone()));
        Ok(residues)
    }

    /// How many residue walks [`call_residue`](Self::call_residue) has run.
    #[cfg(test)]
    pub(crate) fn call_residue_walks(&self) -> u64 {
        self.engine.call_residue_walks.load(Ordering::Relaxed)
    }

    /// The zero-admission diagnostic for this engine's root ([FR-IX-13]) — the
    /// single seam `status` ([FR-NV-07]) and `doctor` ([FR-GV-18]) both go
    /// through, so the two persisted-graph surfaces and the `index` that produced
    /// the graph cannot classify the same root differently.
    ///
    /// `admitted` is the indexed file count the calling surface reports — the same
    /// post-admission quantity `index` derives from — so at a parent folder of
    /// sibling repositories all three read `0` and render the identical line.
    ///
    /// **An empty store is not the same claim as an empty walk.** `index` knows it
    /// admitted nothing because it just walked; `status` and `doctor` deliberately
    /// do not walk (`status` uses the no-prologue runtime so it *reports* an
    /// unindexed graph rather than silently building one, and `doctor` is a pure
    /// read). So `admitted == 0` alone is also true of every root that has simply
    /// never been indexed, where an ordinary repository with one top-level
    /// submodule would be told it "looks like a parent folder of sibling
    /// repositories" — the confidently-wrong instruction [`derive`]'s own contract
    /// calls worse than the silence [FR-IX-13] exists to fix ([NFR-CC-04]), and
    /// contradicting the `run `logos index`` remedy `status` prints in the same
    /// payload. The persisted graph revision closes that gap: [FR-SY-09]/[ADR-32]
    /// advance it once at the end of every **completed** index — including a
    /// zero-admission one — and it reads `0` before the first, so `> 0` is exactly
    /// "an index has run and this is what it produced". With it, the two surfaces
    /// answer `index`'s question rather than a different one, which is what makes
    /// the parity real instead of coincidental.
    ///
    /// **Infallible and advisory** ([ADR-14]): a failed revision read, a
    /// missing/invalid `config.toml`, or an unreadable root yields `None` and a
    /// logged warning rather than an error. That is what keeps the promise the
    /// diagnostic makes — it never changes a command's exit code, so it must not
    /// be able to fail one. The guards are ordered by cost: the free
    /// [`admits_diagnosis`](crate::config::ZeroAdmissionDiagnostic::admits_diagnosis)
    /// test, then one point query, then the config load, then the depth-1 prune
    /// walk, and last — inside [`derive`] itself, reached only by a root about to
    /// be diagnosed — the workspace-manifest probe that suppresses advice the
    /// operator has already taken. An ordinary indexed root pays nothing beyond
    /// the first ([NFR-PE-08]).
    ///
    /// [FR-IX-13]: ../../../docs/specs/requirements/FR-IX-13.md
    /// [FR-NV-07]: ../../../docs/specs/requirements/FR-NV-07.md
    /// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [NFR-PE-08]: ../../../docs/specs/requirements/NFR-PE-08.md
    /// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    /// [`derive`]: crate::config::ZeroAdmissionDiagnostic::derive
    pub(crate) fn zero_admission_diagnostic(
        &self,
        admitted: u64,
    ) -> Option<crate::config::ZeroAdmissionDiagnostic> {
        // A count beyond `usize` cannot be zero, so saturating is exact here:
        // `admits_diagnosis` rejects it either way.
        let admitted = usize::try_from(admitted).unwrap_or(usize::MAX);
        if !crate::config::admits_zero_admission_diagnosis(admitted) {
            return None;
        }
        if !self.has_completed_an_index() {
            return None;
        }
        let config = crate::config::load_config_from_root(&self.engine.root)
            .inspect_err(|err| tracing::warn!(target: LOG_TARGET, "zero-admission diagnostic skipped: {err:#}"))
            .ok()?;
        crate::config::zero_admission_diagnostic(&self.engine.root, &config, admitted)
            .inspect_err(|err| tracing::warn!(target: LOG_TARGET, "zero-admission diagnostic skipped: {err:#}"))
            .ok()
            .flatten()
    }

    /// Whether a **completed** index has ever run against this store, read from
    /// the persisted graph revision ([FR-SY-09], [ADR-32]): `0` before the first
    /// index, advanced once at the end of every completed one.
    ///
    /// Distinguishes "the walk admitted nothing" from "nothing has walked yet" —
    /// two states an empty store renders identically. A transient engine or a
    /// failed read answers `false`: this only ever gates an advisory, so the safe
    /// direction is silence.
    ///
    /// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
    /// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
    fn has_completed_an_index(&self) -> bool {
        self.engine.runtime
            .as_ref()
            .and_then(|runtime| {
                runtime
                    .submit_read(|store| store.graph_revision())
                    .inspect_err(|err| {
                        tracing::warn!(target: LOG_TARGET, "zero-admission diagnostic skipped: {err:#}");
                    })
                    .ok()
            })
            .is_some_and(|revision| revision > 0)
    }
}

/// Map a loaded [`LanguageRegistry`](crate::plugin::LanguageRegistry) into the
/// `languages` read-model — the loaded grammars plus any ABI-skipped ones.
pub(super) fn languages_from(registry: &crate::plugin::LanguageRegistry) -> LanguagesInfo {
    let languages = registry
        .iter()
        .map(|plugin| {
            let semantics = plugin.semantics();
            LanguageDescriptor {
                name: plugin.name().to_string(),
                extensions: plugin.extensions().to_vec(),
                // Artifact plugins (CR-010) claim basenames and are flagged as
                // the third class, so `logos languages` lists their filename
                // claims alongside extensions ([FR-PL-06], [FR-CG-04]).
                filenames: plugin.filenames().to_vec(),
                artifact: plugin.is_artifact(),
                module_separator: semantics.module_separator.clone(),
                capabilities: plugin.capabilities().to_vec(),
                abi_version: semantics.abi_version as u32,
                overridden_capabilities: plugin.overridden_capabilities().to_vec(),
                reach: plugin.reach().map(|r| LanguageReach {
                    level: r.level.as_str().to_string(),
                    cross_file: r
                        .cross_file
                        .iter()
                        .map(|c| c.as_str().to_string())
                        .collect(),
                }),
                note: plugin.note().map(str::to_string),
            }
        })
        .collect();

    let skipped = registry
        .skipped()
        .iter()
        .map(|s| SkippedLanguage {
            name: s.name.clone(),
            reason: s.reason.to_string(),
        })
        .collect();

    LanguagesInfo {
        languages,
        skipped,
        load_error: None,
    }
}
