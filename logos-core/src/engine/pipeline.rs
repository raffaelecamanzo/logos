//! [`Pipeline`] — the indexing pipeline's triggers (FR-IX-01..07, FR-SY-01..06)
//! and the machinery every write path shares, reached by [`Engine::pipeline`]
//! (S-829, [CR-259]): the pre-evaluation reconcile ([ADR-11]), the open-time
//! coordinate migration and `.h` routing, the seeded worktree's diff
//! reconcile and the navigation prologue's config-change purge.
//!
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [CR-259]: ../../../docs/requests/CR-259-engine-is-split-by-feature-area.md

use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{Engine, LOG_TARGET};
use crate::models::pipeline::{IndexResult, SyncResult};
use crate::observability::Tool;
use crate::runtime::Runtime;

/// The pipeline over one engine's graph, reached by [`Engine::pipeline`].
#[derive(Clone, Copy)]
pub struct Pipeline<'e> {
    pub(super) engine: &'e Engine,
}

impl<'e> Pipeline<'e> {
    /// Full re-index of the project (FR-IX-01..06).
    ///
    /// discover → extract → resolve → annotate → persist (ADR-10). Requires a
    /// long-lived [`Engine::start`] engine (it submits write batches to the
    /// writer actor). Infallible at the surface (ADR-14 defers typed errors): a
    /// failure is logged to stderr and returned as an [`IndexResult`] carrying
    /// the reason in `warnings` rather than panicking.
    pub fn index(&self) -> IndexResult {
        match crate::observability::traced(Tool::Index, || self.run_index()) {
            Ok(result) => result,
            Err(err) => degraded_index(&err),
        }
    }

    /// Incremental sync over the given changed `paths` (FR-SY-01..06).
    ///
    /// Capture-before-delete ensures cross-file links rebind (ADR-10). Same
    /// infallible-surface posture as [`index`](Self::index): a failure is logged
    /// and surfaced in the [`SyncResult`] `warnings`.
    pub fn sync(&self, paths: &[PathBuf]) -> SyncResult {
        match crate::observability::traced(Tool::Sync, || self.run_sync(paths)) {
            Ok(result) => result,
            Err(err) => degraded_sync(&err),
        }
    }

    /// Auto-index on first evaluation (FR-IX-07): if the graph has never been
    /// indexed, run a full [`index`](Self::index) before serving; otherwise a
    /// no-op.
    ///
    /// This is the prologue a navigation/query call runs so the very first
    /// evaluation against an un-indexed project transparently builds the graph.
    /// Navigation methods (S-013) call this before reading; it is exposed now so
    /// the auto-index contract is in place and testable. Returns the
    /// [`IndexResult`] of the index that ran, or a zero-valued result when the
    /// graph was already populated.
    pub fn ensure_indexed(&self) -> IndexResult {
        match crate::observability::traced(Tool::EnsureIndexed, || self.run_ensure_indexed()) {
            Ok(Some(result)) => result,
            Ok(None) => IndexResult::default(),
            Err(err) => degraded_index(&err),
        }
    }

    /// Run the [ADR-11] pre-evaluation reconcile (S-020): discover → blake3
    /// dirty-detect → sync the dirty set, O(changed) ([FR-RC-01], [FR-RC-02]),
    /// then advance the sync stamp so hydrated views reflect the reconciled
    /// graph.
    ///
    /// # Errors
    /// Returns an error for a transient engine or on a structural reconcile
    /// failure ([ADR-14] fail-loud); per-file failures degrade inside the
    /// outcome ([NFR-RA-11]).
    ///
    /// [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
    /// [FR-RC-02]: ../../../docs/specs/requirements/FR-RC-02.md
    /// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
    pub(crate) fn run_reconcile(&self) -> Result<crate::pipeline::ReconcileOutcome> {
        let (runtime, registry, config) = self.ctx()?;
        let outcome = crate::pipeline::reconcile(runtime, registry, &self.engine.root, &config)?;
        // The reconcile may have written; invalidate hydrated views so the
        // score sees the fresh graph (ADR-04). The FR-NV-07 last-full-index
        // stamp is not touched here: a full-index degrade reaches it through
        // `pipeline::index`, which writes the durable `project_metadata` row
        // itself (CR-130).
        self.engine.graph().advance_sync_stamp();
        Ok(outcome)
    }

    /// Resolve the live runtime, the cached plugin registry, and the on-disk
    /// config — the three inputs every pipeline operation needs.
    ///
    /// # Errors
    /// Returns an error if this is a transient [`Engine::open`] engine (no
    /// runtime), if the plugin registry failed to load, or if `config.toml` is
    /// present but invalid.
    pub(crate) fn ctx(
        &self,
    ) -> Result<(
        &'e Runtime,
        &'e crate::plugin::LanguageRegistry,
        crate::config::Config,
    )> {
        let runtime = self.engine.runtime.as_ref().context(
            "graph operations require a started engine (Engine::start), not a transient one",
        )?;
        let registry = self
            .engine
            .registry
            .as_ref()
            .context("the plugin registry failed to load; cannot extract any files")?;
        let config = crate::config::load_config_from_root(&self.engine.root)?;
        Ok((runtime, registry, config))
    }

    /// Record this project's resolved identity coordinate in the store
    /// ([FR-CF-10], S-702) — the step `init` runs after completing `config.toml`.
    ///
    /// # Errors
    /// A transient engine, or a failed write.
    ///
    /// [FR-CF-10]: ../../../docs/specs/requirements/FR-CF-10.md
    pub(super) fn record_project_id(&self, config: &crate::config::Config) -> Result<()> {
        let runtime = self.engine.runtime.as_ref().context(
            "recording the project id requires a started engine (Engine::start)",
        )?;
        crate::pipeline::record_project_id(runtime, &self.engine.root, config)
    }

    /// Fallible body of [`index`](Self::index).
    fn run_index(&self) -> Result<IndexResult> {
        let (runtime, registry, config) = self.ctx()?;
        let mut result = crate::pipeline::index(runtime, registry, &self.engine.root, &config)?;
        // Invalidate the hydration cache so the next hydrate() reflects the new
        // graph state ([ADR-04], [ADR-05], [S-009]).
        self.engine.graph().advance_sync_stamp();
        // FR-WS-02 root-scope note (CR-119): a root carrying a workspace
        // manifest is one project among N — `index` here builds only the root,
        // never the members (BR-44, NFR-PE-06 forbid a member fan-out from this
        // command). Advisory only: pushed to `notes`, never `warnings`, so it
        // cannot trip a CI parser or move the FR-CL-03 exit code, and a root
        // without a manifest is byte-identical to today (`notes` stays empty
        // and is elided from the serialized report).
        if self.engine.root.join(crate::federation::MANIFEST_FILENAME).is_file() {
            result.notes.push(format!(
                "note: this root carries a workspace manifest ({}) — `index` built the \
                 root project only ({} files); members are indexed independently by the \
                 background warm started by `logos init --workspace` (see `logos workspace \
                 status`).",
                crate::federation::MANIFEST_FILENAME,
                result.files_indexed,
            ));
        }
        Ok(result)
    }

    /// Fallible body of [`sync`](Self::sync).
    fn run_sync(&self, paths: &[PathBuf]) -> Result<SyncResult> {
        let (runtime, registry, config) = self.ctx()?;
        // A public `Self::sync` carries a *partial* changed-file set (the
        // debounced watcher batch, a git hook, or an explicit CLI `sync`), so it
        // reconciles only those paths and never sweeps files outside the set
        // (CR-052 / FR-SY-10). The full-stored-set disk-deletion sweep is reserved
        // for the full-walk `reconcile` path (`run_reconcile`).
        let result = crate::pipeline::sync(
            runtime,
            registry,
            &self.engine.root,
            &config,
            paths,
            crate::pipeline::SyncScope::Partial,
        )?;
        self.engine.graph().advance_sync_stamp();
        Ok(result)
    }

    /// Apply the main↔branch diff over a just-seeded store ([FR-WT-03],
    /// [ADR-15]): compute the paths differing from the primary checkout's HEAD
    /// (committed branch work + dirty working tree + untracked files) and run
    /// one incremental sync over exactly that set.
    ///
    /// Fail-soft: a failed diff or sync is logged and surfaced to telemetry
    /// (the `worktree_seed` tool record, [FR-WT-03]'s observability
    /// criterion), never fatal — the seeded graph is at worst stale by the
    /// diff, and the next evaluation's reconcile backstop ([ADR-11]) catches
    /// up. A registry-less engine degrades the same way.
    ///
    /// [FR-WT-03]: ../../../docs/specs/requirements/FR-WT-03.md
    pub(super) fn reconcile_seed_diff(&self, seed: &crate::workspace::SeedSource) {
        let outcome = crate::observability::traced(Tool::WorktreeSeed, || {
            let paths = crate::workspace::diff_from_primary(&self.engine.root, &seed.head)?;
            if paths.is_empty() {
                return Ok(SyncResult::default());
            }
            self.run_sync(&paths)
        });
        match outcome {
            Ok(result) => tracing::info!(target: LOG_TARGET,
                files_added = result.files_added,
                files_modified = result.files_modified,
                files_removed = result.files_removed,
                "diff-reconciled the seeded store against the worktree (FR-WT-03)"
            ),
            Err(err) => tracing::warn!(target: LOG_TARGET,
                "diff-reconcile after seeding failed; the next evaluation's \
                 reconcile backstop (ADR-11) will catch up: {err:#}"
            ),
        }
    }

    /// Migrate the store to the project's coordinate when its symbols carry
    /// another ([FR-EX-35], S-704) — the open-time step
    /// [`Engine::start_with`] runs after the runtime
    /// is up and before any sync. A static schema migration could not: the
    /// coordinate comes from the configuration and the repository, not the
    /// store.
    ///
    /// Fail-soft, like [`reconcile_seed_diff`](Self::reconcile_seed_diff): a
    /// configuration that does not load or a migration that fails is logged
    /// and the store stays as it was, consistent under its old coordinate —
    /// every sync renders the coordinate the store records — and the next open
    /// tries again. The configuration error is `index`'s to report.
    ///
    /// [FR-EX-35]: ../../../docs/specs/requirements/FR-EX-35.md
    pub(super) fn migrate_symbol_coordinate(&self) {
        let Some(runtime) = self.engine.runtime.as_ref() else {
            return;
        };
        let migrated = crate::config::load_config_from_root(&self.engine.root)
            .map_err(anyhow::Error::from)
            .and_then(|config| crate::pipeline::migrate_symbol_coordinate(runtime, &self.engine.root, &config));
        match migrated {
            Ok(None) => {}
            Ok(Some(migrated)) => tracing::info!(target: LOG_TARGET,
                from = migrated.from.as_deref().unwrap_or("."),
                to = %migrated.to,
                symbols = migrated.changes.symbols_rewritten,
                orphans_removed = migrated.changes.orphan_symbols_removed,
                files_to_re_extract = migrated.changes.files_invalidated,
                "migrated the store's symbols to the project coordinate (FR-EX-35)"
            ),
            Err(err) => tracing::warn!(target: LOG_TARGET,
                "the store's symbols were not migrated to the project coordinate; it keeps \
                 its recorded one until the next open: {err:#}"
            ),
        }
    }

    /// Read `.h` by the plugin the store's graph was built with (S-582,
    /// [FR-PL-07]) — the decision the last index or sync recorded — so a
    /// process that runs neither, a `callers` or a `status`, routes a header
    /// as the graph does. A store recording none keeps the default, C++.
    ///
    /// Fail-soft, like [`migrate_symbol_coordinate`](Self::migrate_symbol_coordinate):
    /// an unreadable record keeps the default, and the next index or sync
    /// decides again from the files it reads.
    ///
    /// [FR-PL-07]: ../../../docs/specs/requirements/FR-PL-07.md
    pub(super) fn route_shared_extensions(&self) {
        let (Some(runtime), Some(registry)) = (self.engine.runtime.as_ref(), self.engine.registry.as_ref()) else {
            return;
        };
        if let Err(err) = crate::pipeline::route_shared_extensions(runtime, registry) {
            tracing::warn!(target: LOG_TARGET, "the recorded `.h` reader could not be read; C++ reads `.h` until the next sync: {err:#}");
        }
    }

    /// Fallible body of [`ensure_indexed`](Self::ensure_indexed).
    fn run_ensure_indexed(&self) -> Result<Option<IndexResult>> {
        let (runtime, registry, config) = self.ctx()?;
        let result = crate::pipeline::ensure_indexed(runtime, registry, &self.engine.root, &config)?;
        if result.is_some() {
            self.engine.graph().advance_sync_stamp();
        }
        Ok(result)
    }

    /// Run the navigation-prologue config-change purge ([FR-SY-08], [ADR-20],
    /// S-041) and degrade fail-soft, mirroring [`ensure_indexed`].
    ///
    /// Gated on the durable admission fingerprint: an unchanged config is one
    /// read and no discovery walk; a narrowed config purges the now-unadmitted
    /// stored nodes through the single-writer actor and records the new
    /// fingerprint so the purge runs at most once per change. When something was
    /// purged, the hydration cache is invalidated so the read it precedes
    /// reflects the smaller graph ([ADR-04]). A failure is logged and swallowed
    /// — navigation is best-effort ([ADR-11]); the next governance reconcile
    /// catches up.
    ///
    /// [FR-SY-08]: ../../../docs/specs/requirements/FR-SY-08.md
    pub(super) fn run_prologue_purge(&self) {
        let outcome = crate::observability::traced(Tool::NavProloguePurge, || {
            let (runtime, registry, config) = self.ctx()?;
            crate::pipeline::purge_on_config_change(runtime, registry, &self.engine.root, &config)
        });
        match outcome {
            Ok(0) => {}
            Ok(purged) => {
                // Nodes were removed; invalidate hydrated views so the read this
                // prologue precedes reflects the purge ([ADR-04]).
                self.engine.graph().advance_sync_stamp();
                tracing::info!(target: LOG_TARGET,
                    purged,
                    "navigation prologue purged now-unadmitted nodes after a config change \
                     (FR-SY-08, ADR-20)"
                );
            }
            Err(err) => tracing::warn!(target: LOG_TARGET,
                "navigation-prologue config-change purge failed; the next governance \
                 reconcile (ADR-11) will catch up: {err:#}"
            ),
        }
    }
}

/// Log a pipeline failure and degrade to an [`IndexResult`] carrying the reason,
/// honouring the infallible-surface posture (ADR-14 defers typed errors).
///
/// Reached only when the pipeline **itself** failed: since S-513 a file whose
/// facts cannot be persisted fails alone inside the pipeline and never lands
/// here ([FR-EH-05]). A failed pipeline is a correctness failure, not an empty
/// success, so the result is marked [`failed`](IndexResult::failed) — the CLI
/// exits 1 on it ([FR-CL-03]) — rather than reading as a run that indexed
/// nothing.
///
/// [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
/// [FR-CL-03]: ../../../docs/specs/requirements/FR-CL-03.md
fn degraded_index(err: &anyhow::Error) -> IndexResult {
    IndexResult { warnings: failure(Tool::Index, err), failed: true, ..IndexResult::default() }
}

/// Log a sync failure and degrade to a [`SyncResult`] carrying the reason —
/// marked [`failed`](SyncResult::failed), as [`degraded_index`] explains.
fn degraded_sync(err: &anyhow::Error) -> SyncResult {
    SyncResult { warnings: failure(Tool::Sync, err), failed: true, ..SyncResult::default() }
}

/// Log that `tool`'s pipeline failed with `err`, and return the warning a
/// degraded result carries.
fn failure(tool: Tool, err: &anyhow::Error) -> Vec<String> {
    let name = tool.as_str();
    tracing::warn!(target: LOG_TARGET, "{name} failed: {err:#}");
    vec![format!("{name} failed: {err}")]
}
