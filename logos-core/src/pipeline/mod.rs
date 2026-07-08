//! The discover → extract → resolve → annotate → persist pipeline
//! ([pipeline-orchestrator], S-010, [ADR-10]).
//!
//! This module is the integrator behind [`Engine::index`](crate::Engine::index)
//! and [`Engine::sync`](crate::Engine::sync). It owns no new algorithm of its
//! own — it wires three already-built subsystems together:
//!
//! 1. **Discovery** ([config], S-006) — [`config::discover`](crate::config::discover)
//!    does the gitignore-aware, root-contained walk ([FR-IX-02], [NFR-SE-04]);
//!    here we keep only files whose extension resolves to a loaded grammar.
//! 2. **Extraction** ([extraction-engine], S-007) — [`extract_files`] runs Pass 1
//!    on the [execution-runtime]'s shared worker pool ([FR-IX-03], [AQ-04]).
//! 3. **Persistence** ([graph-store], S-005) — each file's facts are written in
//!    one transaction submitted to the single-writer actor ([ADR-02],
//!    [NFR-RA-07]).
//!
//! # Incremental sync & capture-before-delete ([ADR-10])
//!
//! [`sync`] re-extracts only the files whose content actually changed, detected
//! by a per-file **blake3** hash compared against the hash stored at the last
//! index ([FR-SY-03]). Re-extracting a file means deleting its old nodes — which
//! *cascades* to every incident edge, including cross-file edges pointing **into**
//! it from callers in other files. To stop those links from silently rotting
//! ([NFR-RA-04], the [AR-05] trap), [`persist_file`] **captures the inbound
//! cross-file edges before the delete** and persists them as exact-symbol
//! `unresolved_refs` rows — the full [ADR-10] design as of [S-011]: the
//! resolution pass rebinds them in the same sync (the captured symbol of an
//! unchanged declaration is invariant, so the edge re-binds to the fresh node
//! id), and a capture whose target was renamed away simply stays unresolved —
//! never invented.
//!
//! # Passes 2 & 3
//!
//! [`resolve_pass`] (resolution, [S-011]) runs [`crate::resolve::run`] over the
//! whole `unresolved_refs` ledger after extraction persists — both for a full
//! [`index`] and for every [`sync`] (the [FR-RS-03] retry contract) — and its
//! coverage stats are surfaced on the results ([FR-RS-04]).
//! [`annotate_pass`] (annotation, [S-014]) runs [`crate::annotate::run`] over
//! the resolved graph: dead-code, duplicates, and `rules.toml` layer policy,
//! written to native node columns ([FR-AN-01]..[FR-AN-04]); its counts are
//! surfaced on the results.
//!
//! [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
//! [FR-RS-04]: ../../../docs/specs/requirements/FR-RS-04.md
//! [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
//! [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
//!
//! [pipeline-orchestrator]: ../../../docs/specs/architecture/components/pipeline-orchestrator.md
//! [config]: ../../../docs/specs/architecture/components/config.md
//! [extraction-engine]: ../../../docs/specs/architecture/components/extraction-engine.md
//! [execution-runtime]: ../../../docs/specs/architecture/components/execution-runtime.md
//! [graph-store]: ../../../docs/specs/architecture/components/graph-store.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
//! [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
//! [FR-IX-03]: ../../../docs/specs/requirements/FR-IX-03.md
//! [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
//! [NFR-RA-04]: ../../../docs/specs/requirements/NFR-RA-04.md
//! [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
//! [NFR-SE-04]: ../../../docs/specs/requirements/NFR-SE-04.md
//! [AQ-04]: ../../../docs/specs/architecture.md#14-open-questions
//! [S-011]: ../../../docs/planning/journal.md#s-011-resolution-engine
//! [S-014]: ../../../docs/planning/journal.md#s-014-annotation-engine

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;

use crate::config::{self, BindingPolicy, Config, ConfigGlobs, DocGlobs};
use crate::extract::{extract_files, Facts, FileInput, SymbolContext};
use crate::graph_store::{BatchWriter, NewNode, NewUnresolvedRef, CONFIG_FINGERPRINT_KEY};
use crate::model::{EdgeKind, NodeId, RefForm};
use crate::models::pipeline::{
    AnnotationStats, DispatchStats, FrameworkStats, IndexResult, PhaseDurations, ResolutionStats,
    SyncResult,
};
use crate::plugin::LanguageRegistry;
use crate::runtime::Runtime;

/// Debug-only structural-integrity assertion (CR-052, [NFR-RA-13], [FR-GV-18],
/// [ADR-46]): after every completed `index`/`sync` the node store must hold one
/// node per `symbol_id` and no orphan rows. A release build never pays for it;
/// a debug build (tests, dev) panics loudly on drift so a regression is caught
/// at its source — the write path — rather than later at the gate.
///
/// [NFR-RA-13]: ../../../docs/specs/requirements/NFR-RA-13.md
/// [FR-GV-18]: ../../../docs/specs/requirements/FR-GV-18.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
#[cfg(debug_assertions)]
fn debug_assert_structural_integrity(runtime: &Runtime, op: &str) {
    match runtime.submit_read(|store| store.structural_check()) {
        Ok(report) => debug_assert!(
            report.is_ok(),
            "structural drift after {op} (NFR-RA-13): {}",
            report.faults().join("; ")
        ),
        Err(err) => debug_assert!(false, "structural check after {op} failed: {err:#}"),
    }
}

/// A file that has been read and hashed, ready to extract and persist.
struct LoadedFile {
    /// Project-relative path (the `files.path` key and symbol namespace prefix).
    rel: String,
    /// Full UTF-8 source text.
    source: String,
    /// blake3 content hash, stored for dirty detection on the next sync.
    hash: String,
}

/// Per-file persistence tally returned by [`persist_file`].
struct PersistCounts {
    nodes: u64,
    edges: u64,
}

/// Run a **full index**: discover every supported file, extract, and persist
/// ([FR-IX-01]).
///
/// A full index re-extracts every discovered file regardless of its stored hash
/// (it is the authoritative rebuild), so capture-before-delete is unnecessary —
/// every file's links are rebuilt from scratch. On a fresh database every file is
/// an insert; on a re-index an existing file's nodes are replaced in place.
///
/// # Errors
/// Returns an error if discovery fails (bad config glob, missing root) or a write
/// batch cannot commit.
///
/// [FR-IX-01]: ../../../docs/specs/requirements/FR-IX-01.md
pub fn index(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
) -> Result<IndexResult> {
    let start = Instant::now();
    let mut warnings = Vec::new();
    let mut files_failed = Vec::new();

    // Discovery and file-load are timed through the single `tracing` seam
    // (FR-OB-01, CR-057) so they join the per-phase index breakdown (FR-OB-06)
    // — the same measurement that reaches telemetry is handed back here, never
    // a parallel timing path (NFR-OO-01).
    let (candidates, discover_ms) = {
        let (res, ms) = crate::observability::traced_timed("discover", || {
            discover_candidates(root, config, registry, &mut warnings)
        });
        (res?, ms)
    };
    let (loaded, load_ms) = crate::observability::traced_infallible_timed("load", || {
        load_files(runtime, &candidates, &mut warnings, &mut files_failed)
    });

    // The LOC this index ingests, for the beyond-envelope advisory (NFR-PE-09).
    // Counted over the admitted, successfully-loaded set — the same denominator
    // the latency budgets are tuned against — and recorded under
    // `INDEXED_LOC_KEY` below so `status` can repeat the advisory cheaply.
    let indexed_loc: u64 = loaded
        .iter()
        .map(|l| l.source.lines().count() as u64)
        .sum();

    // swe-skills typed-node enrichment (S-039, FR-DG-07): auto-detected from the
    // discovered convention files (overridable in config). A full index sees the
    // whole file set, so detection is exact here.
    let enrich =
        config
            .documentation
            .enrichment_active(crate::extract::doc::enrich::conventions_present(
                loaded.iter().map(|l| l.rel.as_str()),
            ));

    let (outcome, extract_ms, persist_ms) =
        extract_and_persist(runtime, registry, &loaded, false, enrich, &mut warnings)?;

    // CR-004 / FR-SY-07: a full index reconciles the stored set to *exactly* the
    // freshly-discovered candidates — index upserts the discovered files and
    // `purge_unadmitted` removes any stored file the current walk no longer admits
    // or that has disappeared from disk. This runs **unconditionally** so a "fresh
    // index" matches the working tree: previously it was gated on a config-change
    // fingerprint ([ADR-20]) and left disk-deletion reconciliation to `sync`, so a
    // plain re-index over a populated DB retained ghost nodes for files that had
    // been deleted (e.g. a removed git worktree). The purge runs before Pass 2 so
    // the inbound cross-file edges it cascades away re-enter `unresolved_refs` in
    // this same index (NFR-RA-05). The admission fingerprint is still computed and
    // recorded after every pass completes (mirroring `reconcile`) so the
    // narrowing-config bookkeeping stays intact; the purge no longer depends on it.
    let pending_fingerprint = admission_change(runtime, config)?;
    let admitted: HashSet<&str> = candidates.iter().map(|c| c.rel.as_str()).collect();
    purge_unadmitted(runtime, &admitted)?;

    // Pass 2 binds the freshly persisted reference ledger (S-011); the
    // framework pass promotes route/component matches against the resolved
    // graph (S-012); Pass 3 annotates the resolved graph (S-014).
    let (mut resolution, resolve_ms) = resolve_pass(runtime, config.resolution.policy, None)?;
    let (framework, promoted_nodes) =
        framework_pass(runtime, registry, root, config.resolution.policy, None)?;
    // CR-017 / S-080: bind any deferred reference (an OpenAPI operation's route
    // reference) to a route/component this pass just promoted — see
    // [`rebind_for_promotions`]. The focused re-resolve it runs is timed through
    // the same seam and folded into the resolve-phase total (FR-OB-06).
    let rebind_ms = rebind_for_promotions(
        runtime,
        config.resolution.policy,
        &promoted_nodes,
        &mut resolution,
    )?;
    // Framework-dispatch live-rooting (CR-043, ADR-39): live-root the methods
    // dispatched only through an external framework so Pass 3 below stops
    // mis-reporting them dead — a cold index reconciles every `.rs` file.
    let dispatch = dispatch_pass(runtime, registry, root, None)?;
    // Cold index re-annotates the whole graph (whole-graph compute + commit).
    let (annotation, annotate_ms) = annotate_pass(runtime, registry, root, config, false)?;

    // Record the fingerprint last, after a complete index, so the purge above
    // runs at most once per config change ([ADR-20]).
    if let Some(fingerprint) = pending_fingerprint {
        record_admission_fingerprint(runtime, &fingerprint)?;
    }

    // Persist the ingested LOC and, if the repo materially exceeds the
    // performance envelope, surface the one-line advisory on this result
    // (NFR-PE-09) — degradation channel only, never a failure (ADR-14).
    record_indexed_loc(runtime, indexed_loc)?;
    if let Some(advisory) = crate::perf::envelope_advisory(indexed_loc) {
        warnings.push(advisory);
    }

    // FR-SY-09 / ADR-32: a completed index always rebuilds the graph, so it
    // always advances the persisted revision — done last (after every pass
    // committed) so a partial index leaves the revision un-advanced and the
    // freshness signal honest.
    advance_graph_revision(runtime)?;

    // CR-052 / NFR-RA-13: a completed index must leave the graph structurally
    // sound (one node per symbol_id, no orphan rows). Asserted in debug builds.
    #[cfg(debug_assertions)]
    debug_assert_structural_integrity(runtime, "index");

    Ok(IndexResult {
        files_indexed: outcome.files as u64,
        nodes_created: outcome.nodes,
        edges_created: outcome.edges,
        resolution,
        framework,
        dispatch,
        annotation,
        duration_ms: start.elapsed().as_millis() as u64,
        // Per-phase breakdown (FR-OB-06, CR-057): each phase timed exactly once
        // through the single seam. `framework`/`dispatch` reuse the wall-clock
        // their own stats already carry; `resolve` folds in the CR-017 rebind
        // re-resolve. The sum reconciles with `duration_ms` within noise — the
        // small remainder is the inter-pass reconcile/bookkeeping (purge,
        // fingerprint, revision advance) that sits between the timed phases.
        phases: PhaseDurations {
            discover_ms,
            load_ms,
            extract_ms,
            persist_ms,
            resolve_ms: resolve_ms + rebind_ms,
            framework_ms: framework.duration_ms,
            dispatch_ms: dispatch.duration_ms,
            annotate_ms,
        },
        warnings,
        files_failed,
    })
}

/// Monotonic counter making each [`ShadowStore`] directory unique within the
/// process; combined with the pid it keeps concurrent verifies (and parallel
/// tests) from colliding on one temp path without needing a clock or RNG.
static SHADOW_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A **throwaway on-disk graph store** for the deep [`verify`](crate::Engine::verify)
/// reindex (CR-052, [FR-GV-19], [ADR-46]).
///
/// It owns its own directory under the OS temp dir, so the shadow reindex writes
/// nowhere near the live `<root>/.logos/logos.db` and — being outside the project
/// root — never enters the discovery walk. Dropping it removes the db **and its
/// `-wal`/`-shm` sidecars** (then the enclosing directory): the teardown
/// [FR-GV-19] mandates on completion. Drop the shadow [`Runtime`] first so no
/// writer connection still holds the files when they are removed.
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
pub struct ShadowStore {
    dir: PathBuf,
    db_path: PathBuf,
}

impl ShadowStore {
    /// Create a fresh, empty shadow-store directory under the OS temp dir.
    ///
    /// The name is unique per run (`logos-verify-<pid>-<seq>`); any stale
    /// directory from a crashed prior run is cleared first so a leftover db
    /// can never pollute this reindex.
    ///
    /// # Errors
    /// Returns an error if the temp directory cannot be created.
    pub fn create() -> Result<Self> {
        let seq = SHADOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("logos-verify-{}-{seq}", std::process::id()));
        // A crashed prior run with the same pid+seq (pid reuse across a reboot)
        // could strand a directory; start from a clean slate.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating the shadow-store directory {}", dir.display()))?;
        let db_path = dir.join("logos.db");
        Ok(Self { dir, db_path })
    }

    /// The db path the shadow [`Runtime`] opens.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

impl Drop for ShadowStore {
    fn drop(&mut self) {
        // Remove the db and both SQLite sidecars explicitly (the FR-GV-19
        // teardown contract names `-wal`/`-shm`), then the enclosing dir.
        for suffix in ["", "-wal", "-shm"] {
            let mut os = self.db_path.as_os_str().to_os_string();
            os.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(os));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Reindex `root` into the throwaway `shadow` store via the always-purge full
/// [`index`] path ([FR-GV-19]), returning the opened [`Runtime`] for the caller
/// to census read-only and then drop.
///
/// This is the pipeline's **transient shadow-index entry point** ([ADR-46] §4.4):
/// it reuses the exact `index` used to build the canonical store — same discovery
/// of `root`, same always-purge reconcile — so the shadow graph is ground truth
/// for the equivalence diff ([NFR-RA-06]). It reads the project tree and writes
/// only the shadow store; the live store is untouched.
///
/// The caller owns [`ShadowStore`] teardown and **must drop the returned
/// [`Runtime`] before the [`ShadowStore`]**, so the writer connection releases
/// the db (and checkpoints its WAL) before the files are removed.
///
/// # Errors
/// Returns an error if the shadow runtime cannot be opened/migrated or the index
/// fails (a bad config glob, an unreadable root, or a failed write batch).
///
/// [FR-GV-19]: ../../../docs/specs/requirements/FR-GV-19.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
pub fn shadow_reindex(
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
    shadow: &ShadowStore,
) -> Result<Runtime> {
    let runtime = Runtime::open(shadow.db_path()).with_context(|| {
        format!(
            "opening the throwaway shadow store at {}",
            shadow.db_path().display()
        )
    })?;
    // The `IndexResult` is discarded: `verify` diffs the resulting *store* (via a
    // read-only census), not the index summary — `index` is run for its effect.
    index(&runtime, registry, root, config)?;
    Ok(runtime)
}

/// Whether a [`sync`] call's `paths` are the **complete** current candidate set
/// (a full-walk reconcile) or only a **partial** changed-file batch (the
/// filesystem-watcher's debounced set, a git hook, an explicit CLI `sync`).
///
/// The distinction gates the Channel-B disk-deletion sweep ([FR-SY-10],
/// [ADR-46]): only a full walk carries the evidence that a stored file absent
/// from both the path-set and the disk is an orphan to purge. A partial call
/// says nothing about the files outside its set — one is simply untouched, not
/// deleted — so it must never sweep, or a single-file watcher event could wrongly
/// purge the rest of the graph (the [filesystem-watcher] path-set contract).
///
/// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
/// [filesystem-watcher]: ../../../docs/specs/architecture/integrations/filesystem-watcher.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    /// `paths` is the entire freshly-discovered candidate set; a stored file that
    /// is neither in it nor on disk has been deleted and is reconciled out over
    /// the removal path a full [`index`] uses ([FR-RC-01]).
    FullWalk,
    /// `paths` is a partial changed-file batch; only those paths are reconciled
    /// and no stored file outside the set is ever purged.
    Partial,
}

/// Run an **incremental sync** over the requested `paths` ([FR-SY-01]).
///
/// Each path is normalised to a project-relative key and classified by comparing
/// its current blake3 hash against the stored one: *unchanged* files are skipped
/// without re-extraction ([FR-SY-03]), *added*/*modified* files are re-extracted
/// and persisted with capture-before-delete ([ADR-10]), and a path that no longer
/// exists on disk but is present in the index is *removed*.
///
/// `scope` gates the Channel-B disk-deletion reconciliation ([FR-SY-10],
/// [ADR-46]): a [`SyncScope::FullWalk`] caller (whose `paths` are the entire
/// current candidate set) additionally sweeps the **full stored set** for files
/// that have disappeared from disk — routing each to the same removal path a full
/// [`index`] uses, independent of the admission fingerprint — while a
/// [`SyncScope::Partial`] caller (the watcher/hooks/CLI) reconciles only its own
/// path-set and never purges a file outside it.
///
/// # Errors
/// Returns an error if the stored file list cannot be read or a write batch
/// cannot commit.
///
/// [FR-SY-01]: ../../../docs/specs/requirements/FR-SY-01.md
/// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
/// [FR-SY-10]: ../../../docs/specs/requirements/FR-SY-10.md
/// [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
/// [ADR-46]: ../../../docs/specs/architecture/decisions/ADR-46.md
pub fn sync(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
    paths: &[PathBuf],
    scope: SyncScope,
) -> Result<SyncResult> {
    // Discovery globs do not gate an explicit changed-file set; config feeds
    // the resolution policy below.
    let start = Instant::now();
    let mut warnings = Vec::new();

    let canon_root = root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", root.display()))?;

    // The doc-admission matcher (S-034): a changed `.md` is processed only if the
    // doc globs admit it, so a doc edit rides the same blake3 sync as code while
    // an out-of-glob or disabled-config markdown change is ignored. `None` when
    // documentation is disabled.
    let doc_globs = config.documentation.compile()?;
    // The config-artifact admission matcher (S-062, CR-010): a changed artifact
    // file rides the same blake3 sync as code; `None` when the layer is disabled.
    let config_globs = config.config_artifacts.compile()?;
    // CR-017 / S-081: the code-language admission allowlist (`None` = all
    // compiled-in code grammars; a non-empty `languages` restricts to those).
    let langs = config.language_allowlist();

    // CR-054 / FR-SY-11: the walk-level admission authority (nested-`.git`
    // boundary + gitignore matcher + `ignored_dirs` + include/exclude globs +
    // size), built ONCE per sync so an edit to `.gitignore` self-heals on the
    // next sync (the matcher re-reads the root ignore sources here). This is the
    // load-bearing gate that finally aligns the incremental path with the full
    // walk ([FR-IX-02]): it covers the watcher, the git hook, an explicit CLI
    // `sync`, and the worktree seed-diff — every partial entry point flows
    // through here. It composes with `admits_file` at the gate below ([ADR-48]):
    // `admits_path` answers "would the discovery walk yield P?", `admits_file`
    // answers "is P code/doc/config?". A bad include/exclude glob fails loud
    // here exactly as it does in `discover`/`discover_candidates` ([ADR-14]).
    let authority = crate::config::AdmissionAuthority::from_config(&canon_root, config)
        .context("building the sync admission authority")?;

    // The hash recorded at the last index, per project-relative path.
    let stored: HashMap<String, Option<String>> = runtime
        .submit_read(|store| store.indexed_files())?
        .into_iter()
        .map(|r| (r.path, r.content_hash))
        .collect();

    let mut loaded: Vec<LoadedFile> = Vec::new();
    let mut added: HashSet<String> = HashSet::new();
    let mut removals: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut files_failed: Vec<String> = Vec::new();

    for path in paths {
        let Some(rel) = relativize(&canon_root, path) else {
            warnings.push(format!(
                "{}: outside the project root, skipped",
                path.display()
            ));
            continue;
        };
        if !seen.insert(rel.clone()) {
            continue; // the same file requested twice
        }

        let abs = canon_root.join(&rel);
        if !abs.is_file() {
            // Gone from disk: a removal if it was previously indexed, otherwise
            // nothing to do. This check precedes the admission gate so a file
            // that was indexed and is now gone is always reconcilable — even a
            // doc the config no longer admits (documentation disabled, or the
            // doc globs narrowed). Without this, deleting a now-unadmitted doc
            // would orphan its nodes until a full re-index. `stored` only holds
            // previously-admitted files, so this never removes anything that was
            // not indexed.
            if stored.contains_key(&rel) {
                removals.push(rel);
            }
            continue;
        }

        // The file still exists: gate it on admission. Two composed predicates
        // ([ADR-48]): the walk-level `authority.admits_path` (CR-054 / FR-SY-11 —
        // a gitignored path, a nested-`.git`-boundary path, an `ignored_dirs`
        // name, a glob-excluded or oversize file), and `admits_file` (a non-source
        // file, a markdown path the doc globs/toggle do not admit, or a config
        // artifact the config globs/toggle do not admit). Either rejecting means
        // the file is not (re-)extracted.
        if !authority.admits_path(&abs)
            || !admits_file(registry, doc_globs.as_ref(), config_globs.as_ref(), langs.as_ref(), &rel)
        {
            // CR-004 / FR-SY-07 + CR-054 / FR-RC-06: a file that *is* stored but is
            // no longer admitted — by a config narrowing (`[documentation] enabled
            // = false`), a new `.gitignore` rule, or a glob/size change since its
            // index — is a narrowing removal, not a skip. Route it to the removal
            // path so its nodes are purged and inbound cross-file edges return to
            // `unresolved_refs` via this sync's resolve pass. This gate is
            // self-limiting: admission is deterministic in the config + the root
            // ignore sources, so a stored, on-disk, now-unadmitted file can only
            // exist *after* one of those changed — no fingerprint check is needed
            // here, and a `.gitignore` edit (which does not move the config
            // fingerprint) is now caught too. A never-admitted file (not in
            // `stored`) is simply skipped, exactly as before.
            if stored.contains_key(&rel) {
                removals.push(rel);
            }
            continue;
        }

        let source = match fs::read_to_string(&abs) {
            Ok(s) => s,
            Err(e) => {
                warnings.push(format!("{rel}: unreadable or non-UTF-8, skipped ({e})"));
                // A per-file reconcile failure — the INCOMPLETE input
                // (NFR-RA-11, ADR-11): the file exists but could not enter
                // the graph, so a score over this state is degraded.
                files_failed.push(rel);
                continue;
            }
        };
        let hash = hash_source(&source);

        match stored.get(&rel) {
            // Unchanged: the stored hash matches — skip without re-extracting.
            Some(Some(prev)) if *prev == hash => continue,
            // Present but changed (or never hashed): a modification.
            Some(_) => {}
            // Absent from the index: a new file.
            None => {
                added.insert(rel.clone());
            }
        }
        loaded.push(LoadedFile { rel, source, hash });
    }

    // CR-052 / FR-SY-10 + CR-054 / FR-RC-06 (Channel B): on a FULL-WALK sync,
    // converge the stored set to the freshly-discovered admitted candidate set.
    // `paths` here is that complete candidate set (`discover_candidates` = the walk
    // ∪ `admits_file`), so every path it names is already in `seen`. Any stored
    // file NOT in `seen` is therefore absent from the current admission — gone from
    // disk *or* still on disk but now unadmitted (gitignored, under a nested-`.git`
    // boundary, glob-excluded, or oversize) — and is routed to the same removal
    // path a full `index` uses ([FR-RC-01]). This makes a full-walk reconcile
    // byte-identical to a fresh index ([NFR-RA-06]): the [CR-052] version escaped
    // any still-on-disk file (`|| …is_file()`), so a now-gitignored file that
    // never left disk survived — the exact CR-054 drift. `.gitignore` edits do not
    // move the config fingerprint, so the fingerprint-gated purge ([FR-SY-07]) in
    // `reconcile` could not catch them; this convergence does.
    //
    // Gated to full walks by construction: a PARTIAL watcher/hook/CLI batch carries
    // no evidence about files outside its set, so sweeping there could purge live
    // files (the filesystem-watcher path-set contract, [ADR-46] open question).
    // Every path processed above is already in `seen` (its own admission handled
    // inline), so the sweep scans only stored-minus-seen.
    //
    // [FR-RC-06]: ../../../docs/specs/requirements/FR-RC-06.md
    // [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    if scope == SyncScope::FullWalk {
        for rel in stored.keys() {
            if seen.contains(rel) {
                continue;
            }
            removals.push(rel.clone());
        }
    }

    // CR-015 incremental resolution change-set (part 1 of 2): the project-relative
    // files this sync re-extracts or removes, and the node names they carry *now*.
    // Read before persist replaces them — the re-extract deletes these very nodes
    // — so that, unioned with the freshly extracted names below, they form the set
    // of binding-bucket keys this sync moves. The resolve pass then re-binds only
    // the rows those keys (and the changed files) can affect, not the whole ledger.
    let changed_paths: HashSet<String> = loaded
        .iter()
        .map(|l| l.rel.clone())
        .chain(removals.iter().cloned())
        .collect();
    let old_names: Vec<String> = if changed_paths.is_empty() {
        Vec::new()
    } else {
        runtime.submit_read(|store| {
            let mut names = Vec::new();
            for path in &changed_paths {
                names.extend(store.node_names_for_path(path)?);
            }
            Ok(names)
        })?
    };

    let mut result = SyncResult::default();

    // Re-extract and persist the dirty set with capture-before-delete.
    let inputs: Vec<FileInput> = loaded
        .iter()
        .map(|l| FileInput::new(l.rel.clone(), l.source.clone()))
        .collect();
    let hash_by_rel: HashMap<&str, &str> = loaded
        .iter()
        .map(|l| (l.rel.as_str(), l.hash.as_str()))
        .collect();
    let ctx = SymbolContext::default();
    // Pass 1 over the dirty set — instrumented like the full-index path
    // (FR-OB-01: the three pipeline passes emit through the single seam).
    let mut facts = crate::observability::traced_infallible("extract", || {
        runtime
            .worker_pool()
            .install(|| extract_files(&inputs, registry, &ctx))
    });

    // swe-skills typed-node enrichment (S-039, FR-DG-07). Auto-detection is over
    // the *whole post-sync* repo — the stored file list, minus this sync's
    // removals and unioned with its dirty set — not just the dirty files, so
    // editing one prose doc on a swe-skills repo still promotes (and a sync that
    // touches a convention file is detected even before it is persisted). Files
    // being removed are excluded so deleting the last convention file deactivates
    // enrichment in the same sync, not one sync late. The config override wins.
    let removed: HashSet<&str> = removals.iter().map(String::as_str).collect();
    let enrich =
        config
            .documentation
            .enrichment_active(crate::extract::doc::enrich::conventions_present(
                stored
                    .keys()
                    .map(String::as_str)
                    .filter(|p| !removed.contains(p))
                    .chain(loaded.iter().map(|l| l.rel.as_str())),
            ));
    crate::extract::doc::enrich::promote_facts(&mut facts, enrich);

    for f in &facts {
        let hash = hash_by_rel.get(f.path.as_str()).copied().unwrap_or("");
        persist_one(runtime, f, hash, true, &mut warnings)?;
        if added.contains(&f.path) {
            result.files_added += 1;
        } else {
            result.files_modified += 1;
        }
    }

    // Removals: a file gone from disk, or a stored/on-disk file the current config
    // no longer admits (FR-SY-07). Both route through the shared removal path.
    for rel in removals {
        remove_file(runtime, rel)?;
        result.files_removed += 1;
    }

    // CR-015 incremental resolution change-set (part 2 of 2): union the names that
    // entered the changed files (this sync's freshly extracted facts) with those
    // that left them (`old_names`) and the changed paths, tokenized. The resolve
    // pass re-binds exactly the rows these can move and skips the rest — the same
    // result as retrying the whole ledger (FR-RS-03), a fraction of the cost.
    let mut dirty_tokens: HashSet<String> = HashSet::new();
    for name in &old_names {
        dirty_tokens.extend(crate::resolve::tokens(name));
    }
    for f in &facts {
        for n in &f.nodes {
            dirty_tokens.extend(crate::resolve::tokens(&n.name));
        }
    }
    for path in &changed_paths {
        dirty_tokens.extend(crate::resolve::tokens(path));
    }
    let delta = crate::resolve::Delta {
        changed_paths,
        dirty_tokens,
    };

    // Pass 2: a deferred reference binds once its target is indexed (UAT-RS-01)
    // and captured cross-file edges rebind (ADR-10) — now over just the
    // change-affected rows (CR-015) rather than the whole ledger, preserving the
    // retry contract (FR-RS-03). The framework pass then reconciles route/component
    // promotions (S-012) — incrementally gated so a framework-free sync skips its
    // whole-graph snapshot (S-024-HF). Pass 3 re-annotates: the dead-code/
    // duplicate/layer verdicts are graph-global, so a one-file change can flip a
    // verdict anywhere — the compute stays whole-graph, but the commit writes only
    // the changed verdicts (S-024-HF), keeping the change-proportional sync within
    // the NFR-PE-03 budget.
    // Sync does not surface a per-phase breakdown (FR-OB-06 is index-scoped), so
    // the seam-measured durations these passes return are discarded here — the
    // `tracing` events still reach telemetry unchanged.
    let (resolution, _resolve_ms) = resolve_pass(runtime, config.resolution.policy, Some(&delta))?;
    result.resolution = resolution;
    let (framework, promoted_nodes) =
        framework_pass(runtime, registry, &canon_root, config.resolution.policy, Some(&delta))?;
    result.framework = framework;
    // CR-017 / S-080: the framework pass may have promoted a route/component after
    // the resolve above; rebind deferred cross-artifact references that target it.
    rebind_for_promotions(
        runtime,
        config.resolution.policy,
        &promoted_nodes,
        &mut result.resolution,
    )?;
    // Framework-dispatch live-rooting (CR-043, ADR-39), incrementally gated:
    // only the changed `.rs` files are rescanned and only their markers
    // reconciled, so a sync that touched no Rust file does no work — keeping the
    // change-proportional sync within the NFR-PE-03 budget. Its markers feed the
    // re-annotate below transparently (no Pass-3 edit beyond reading the marker).
    result.dispatch = dispatch_pass(runtime, registry, &canon_root, Some(&delta))?;
    let (annotation, _annotate_ms) = annotate_pass(runtime, registry, root, config, true)?;
    result.annotation = annotation;

    // FR-SY-09 / ADR-32: advance the persisted revision only when this sync
    // mutated the graph — at least one file added, modified, or removed. A no-op
    // sync (every requested path unchanged or skipped) leaves the graph, and so
    // the revision, untouched. Done after every pass committed, mirroring index.
    if result.files_added + result.files_modified + result.files_removed > 0 {
        advance_graph_revision(runtime)?;
    }

    // CR-052 / NFR-RA-13: a completed sync must leave the graph structurally
    // sound (one node per symbol_id, no orphan rows). Asserted in debug builds.
    #[cfg(debug_assertions)]
    debug_assert_structural_integrity(runtime, "sync");

    result.duration_ms = start.elapsed().as_millis() as u64;
    result.warnings = warnings;
    result.files_failed = files_failed;
    Ok(result)
}

/// Reconcile the working tree into the graph before an aggregate run
/// ([FR-RC-01], [ADR-11], S-020): discover the current candidate set and
/// [`sync`] it as a [`SyncScope::FullWalk`] — blake3 dirty-detection makes the
/// re-parse `O(changed)` ([FR-RC-02]), and the full-walk sweep reconciles out any
/// file gone from disk (Channel B, [FR-SY-10], [ADR-46]) so deletions are noticed
/// without pre-unioning the stored file list. A never-indexed tree degrades to a
/// full [`index`].
///
/// Per-file failures ride in [`ReconcileOutcome::files_failed`] for the
/// governance engine's `INCOMPLETE` stamping ([NFR-RA-11]); a structural
/// failure (store fault, bad config) is the returned error — fail loud
/// ([ADR-14]).
///
/// # Errors
/// Returns an error if discovery fails, the stored file list cannot be read,
/// or a write batch cannot commit.
///
/// [FR-RC-01]: ../../../docs/specs/requirements/FR-RC-01.md
/// [FR-RC-02]: ../../../docs/specs/requirements/FR-RC-02.md
/// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-14]: ../../../docs/specs/architecture/decisions/ADR-14.md
pub fn reconcile(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
) -> Result<ReconcileOutcome> {
    let stored = runtime.submit_read(|store| store.indexed_files())?;

    // FR-RC-02: a never-indexed tree degrades to a full index.
    if stored.is_empty() {
        let result = index(runtime, registry, root, config)?;
        return Ok(ReconcileOutcome {
            full_index: true,
            reconciled_files: result.files_indexed,
            files_failed: result.files_failed,
            resolution: result.resolution,
            warnings: result.warnings,
        });
    }

    let mut warnings = Vec::new();
    let candidates = discover_candidates(root, config, registry, &mut warnings)?;
    let candidate_keys: HashSet<&str> = candidates.iter().map(|c| c.rel.as_str()).collect();

    // CR-004 / FR-SY-07: a config-narrowing change reconciles on the next
    // aggregate run too, gated on the durable fingerprint ([ADR-20]). When the
    // fingerprint moved, purge every stored file the new config no longer admits
    // — config-excluded *and* disk-deleted alike — through the removal path, then
    // sync only the still-admitted candidates. The purged files must NOT be
    // re-fed to sync: an on-disk code file dropped by a new `exclude` glob still
    // satisfies `admits_file` (which only knows extensions/doc/config claims, not
    // discovery's include/exclude/size filters), so sync would re-admit it. When
    // the fingerprint is unchanged, disk-deletions are noticed by the full-walk
    // `sync` below — its Channel-B sweep removes any stored file gone from disk
    // (CR-052 / FR-SY-10, [FR-RC-01]) — so no stored-set pre-union is needed here.
    let pending_fingerprint = admission_change(runtime, config)?;
    let paths: Vec<PathBuf> = candidates.iter().map(|c| PathBuf::from(&c.rel)).collect();
    let purge = if pending_fingerprint.is_some() {
        purge_unadmitted(runtime, &candidate_keys)?
    } else {
        // Config unchanged: disk-deletions are reconciled by the full-walk `sync`
        // below, which sweeps the full stored set for files gone from disk
        // (CR-052 / FR-SY-10). We no longer pre-union the stored-minus-candidate
        // set into `paths` for that purpose — the removal arm is generalised into
        // `sync` itself, keyed on disk presence rather than on the caller naming
        // the file. The config-narrowing purge stays fingerprint-gated (the `if`
        // arm above), unchanged.
        PurgeOutcome::default()
    };

    let result = sync(runtime, registry, root, config, &paths, SyncScope::FullWalk)?;
    warnings.extend(result.warnings);

    // CR-017 / S-079: the config-narrowing purge above removed files *outside*
    // the incremental sync — they are dropped by a discovery glob `sync` cannot
    // see, so they cannot ride its removal path — and so the CR-015 sync delta
    // never covered the inbound cross-file references that pointed into them. A
    // still-admitted caller's ledger row whose target was just purged would stay
    // stale `resolved = 1`. Re-resolve exactly those references, keyed on the
    // purged files' now-gone node names, so each returns to `unresolved_refs`
    // ([NFR-RA-05], [FR-SY-07]) — the write-path counterpart of the demotion the
    // [FR-SY-08] navigation prologue defers. Gated on a purge having happened (a
    // config change), so it runs at most once per change and the CR-015
    // incremental-resolution budget holds; the focused delta keeps it cheap.
    let mut resolution = result.resolution;
    if !purge.names.is_empty() {
        let mut dirty_tokens: HashSet<String> = HashSet::new();
        for name in &purge.names {
            dirty_tokens.extend(crate::resolve::tokens(name));
        }
        for path in &purge.paths {
            dirty_tokens.extend(crate::resolve::tokens(path));
        }
        if !dirty_tokens.is_empty() {
            let delta = crate::resolve::Delta {
                changed_paths: purge.paths.iter().cloned().collect(),
                dirty_tokens,
            };
            let (res, _resolve_ms) = resolve_pass(runtime, config.resolution.policy, Some(&delta))?;
            resolution = res;
        }
    }

    // Record the new fingerprint only after a complete reconciliation (the purge
    // above covered the full stored set), so the purge runs at most once per
    // change and a partial direct `sync` can never disarm it prematurely.
    if let Some(fingerprint) = pending_fingerprint {
        record_admission_fingerprint(runtime, &fingerprint)?;
    }
    Ok(ReconcileOutcome {
        full_index: false,
        // Purged files count toward the reconciled total the freshness line
        // reports (FR-OB-01 / FR-RC-03), the way disk removals already do.
        reconciled_files: result.files_added
            + result.files_modified
            + result.files_removed
            + purge.count,
        files_failed: result.files_failed,
        resolution,
        warnings,
    })
}

/// The outcome of a pre-evaluation [`reconcile`] (S-020, [FR-RC-01..03]).
///
/// [FR-RC-01..03]: ../../../docs/specs/requirements/FR-RC-01.md
#[derive(Debug, Default)]
pub struct ReconcileOutcome {
    /// `true` when a never-indexed tree degraded to a full index
    /// ([FR-RC-02]).
    ///
    /// [FR-RC-02]: ../../../docs/specs/requirements/FR-RC-02.md
    pub full_index: bool,
    /// Files this reconcile actually (re-)entered into or removed from the
    /// graph — the `reconciled N files` count of the freshness line
    /// ([FR-RC-03]).
    ///
    /// [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
    pub reconciled_files: u64,
    /// Files that could not be read/extracted — the `INCOMPLETE` input
    /// ([NFR-RA-11]).
    ///
    /// [NFR-RA-11]: ../../../docs/specs/requirements/NFR-RA-11.md
    pub files_failed: Vec<String>,
    /// Post-reconcile resolution coverage — supplies the `M unresolved refs`
    /// half of the freshness line ([FR-RC-03]).
    ///
    /// [FR-RC-03]: ../../../docs/specs/requirements/FR-RC-03.md
    pub resolution: crate::models::pipeline::ResolutionStats,
    /// Degradations folded from discovery and the sync — never an error.
    pub warnings: Vec<String>,
}

/// Auto-index on first use ([FR-IX-07]): index only if the graph is empty.
///
/// Returns the [`IndexResult`] when an index ran, or `None` if the graph already
/// holds at least one file. This is the prologue a navigation/query call runs so
/// the very first evaluation against an un-indexed project transparently builds
/// the graph before serving.
///
/// # Errors
/// Returns an error if the stored file list cannot be read or the index fails.
///
/// [FR-IX-07]: ../../../docs/specs/requirements/FR-IX-07.md
pub fn ensure_indexed(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
) -> Result<Option<IndexResult>> {
    let already_indexed = !runtime
        .submit_read(|store| store.indexed_files())?
        .is_empty();
    if already_indexed {
        Ok(None)
    } else {
        Ok(Some(index(runtime, registry, root, config)?))
    }
}

// ── Config-change reconciliation (CR-004, FR-SY-07, ADR-20) ───────────────────

/// Has the admission-relevant configuration changed since the last
/// reconciliation? Returns `Some(current_fingerprint)` when a config-narrowing
/// purge is **armed** — the stored fingerprint is absent (a fresh DB, or one
/// upgraded across migration 15) or differs from the current one — and `None`
/// when the config is unchanged.
///
/// The single read that gates every purge below: an unchanged config costs only
/// this one comparison and writes nothing ([FR-SY-07] "no purge work, no node
/// churn").
fn admission_change(runtime: &Runtime, config: &Config) -> Result<Option<String>> {
    let current = config.admission_fingerprint();
    let stored = runtime.submit_read(|store| store.project_metadata(CONFIG_FINGERPRINT_KEY))?;
    Ok((stored.as_deref() != Some(current.as_str())).then_some(current))
}

/// What a config-narrowing purge removed: the count for the freshness line, the
/// purged files' project-relative paths, and the node **names** they carried —
/// the latter two captured *before* deletion. They let a write-path caller
/// re-resolve the inbound cross-file references that pointed into the purged
/// files, so a caller's now-dangling resolved ledger row returns to
/// `unresolved_refs` ([NFR-RA-05], CR-017/S-079). The navigation-prologue caller
/// ignores them by design — [`purge_on_config_change`] defers that demotion
/// ([FR-SY-08]).
#[derive(Default)]
struct PurgeOutcome {
    count: u64,
    paths: Vec<String>,
    names: Vec<String>,
}

/// Purge every stored file the current config no longer admits — the
/// config-narrowing reconciliation ([FR-SY-07]).
///
/// `admitted` is the set of currently-admitted project-relative keys (the
/// freshly-discovered candidate set); any stored file absent from it is removed
/// through [`remove_file`]. Callers gate this on [`admission_change`] so it only
/// runs when the fingerprint moved. Before deleting, it snapshots the purged
/// files' node names (the binding keys their inbound references resolved through)
/// so a write-path caller can re-bind exactly those references and demote the
/// ones whose target just vanished — the same change-set the `sync` removal path
/// feeds its resolve pass. Returns the [`PurgeOutcome`].
fn purge_unadmitted(runtime: &Runtime, admitted: &HashSet<&str>) -> Result<PurgeOutcome> {
    let stored = runtime.submit_read(|store| store.indexed_files())?;
    let paths: Vec<String> = stored
        .into_iter()
        .filter(|f| !admitted.contains(f.path.as_str()))
        .map(|f| f.path)
        .collect();
    if paths.is_empty() {
        return Ok(PurgeOutcome::default());
    }
    // Capture the node names of the files about to be purged BEFORE the delete
    // cascades them away, so the caller can re-resolve the inbound cross-file
    // references that targeted them ([NFR-RA-05]).
    let names = {
        let paths = paths.clone();
        runtime.submit_read(move |store| {
            let mut names = Vec::new();
            for p in &paths {
                names.extend(store.node_names_for_path(p)?);
            }
            Ok(names)
        })?
    };
    for path in &paths {
        remove_file(runtime, path.clone())?;
    }
    Ok(PurgeOutcome {
        count: paths.len() as u64,
        paths,
        names,
    })
}

/// Record the admission-config fingerprint in `project_metadata` ([ADR-20]).
///
/// Written after a complete reconciliation so the purge runs at most once per
/// config change.
fn record_admission_fingerprint(runtime: &Runtime, fingerprint: &str) -> Result<()> {
    let fingerprint = fingerprint.to_string();
    runtime.submit_write(move |w| w.set_project_metadata(CONFIG_FINGERPRINT_KEY, &fingerprint))
}

/// Record the LOC ingested by this index in `project_metadata`, so `status`
/// can emit the [NFR-PE-09] beyond-envelope advisory without re-reading the
/// tree ([`crate::perf::INDEXED_LOC_KEY`]).
///
/// [NFR-PE-09]: ../../../docs/specs/requirements/NFR-PE-09.md
fn record_indexed_loc(runtime: &Runtime, indexed_loc: u64) -> Result<()> {
    let value = indexed_loc.to_string();
    runtime.submit_write(move |w| w.set_project_metadata(crate::perf::INDEXED_LOC_KEY, &value))
}

/// Advance the persisted monotonic graph revision after a completed `index` or a
/// `sync` that mutated the graph (CR-027, [ADR-32], [FR-SY-09]).
///
/// The durable "the graph changed" signal: a single-row write on the existing
/// post-`sync` write path ([NFR-PE-03]), submitted through the single-writer
/// actor so the read-modify-write is atomic with respect to every other write
/// ([NFR-RA-10]). Called once per completed `index` and once per mutating
/// `sync`; a no-op `sync` (no dirty files) and a read-only navigation never
/// reach it, so the revision stays a pure function of completed, graph-changing
/// pipeline runs ([NFR-RA-06]).
///
/// [ADR-32]: ../../../docs/specs/architecture/decisions/ADR-32.md
/// [FR-SY-09]: ../../../docs/specs/requirements/FR-SY-09.md
/// [NFR-PE-03]: ../../../docs/specs/requirements/NFR-PE-03.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
fn advance_graph_revision(runtime: &Runtime) -> Result<()> {
    runtime.submit_write(|w| w.advance_graph_revision())?;
    Ok(())
}

/// The navigation-prologue config-change purge ([FR-SY-08], [ADR-20], S-041).
///
/// A **scoped exception** to [FR-RC-05]'s "navigation never reconciles per call":
/// when the admission-relevant configuration has narrowed since the last
/// reconciliation, purge the now-unadmitted stored nodes *before* the first
/// navigation read serves, so reads never surface no-longer-admitted symbols
/// ([FR-SY-08]). Unlike [`reconcile`], it does **not** sync the working tree —
/// it only removes the config-narrowing set, never a per-call disk reconcile.
///
/// Gated on the durable fingerprint exactly like the write-path purge: an
/// unchanged config costs one comparison and issues **no write and no discovery
/// walk** (the `else` early-returns before [`discover_candidates`]); a changed
/// config purges, then records the fingerprint *last* so the purge runs at most
/// once per change ([FR-SY-08] AC, [ADR-20]). The removals and the fingerprint
/// write both submit through the single-writer actor ([NFR-RA-10]), never the
/// read-only pool.
///
/// Returns the number of files purged (`0` when the config is unchanged), so the
/// caller can decide whether to invalidate hydrated views.
///
/// [FR-SY-08]: ../../../docs/specs/requirements/FR-SY-08.md
/// [FR-RC-05]: ../../../docs/specs/requirements/FR-RC-05.md
/// [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md
pub fn purge_on_config_change(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
) -> Result<u64> {
    // The single fingerprint comparison that gates everything below: unchanged
    // config → no discovery walk, no write ([FR-SY-08] AC).
    let Some(fingerprint) = admission_change(runtime, config)? else {
        return Ok(0);
    };

    // Armed: discover the currently-admitted candidate set and purge every stored
    // file absent from it — the same "stored file ∉ current candidate set" key the
    // `index`/`reconcile` write-path purge uses, so a code `exclude` narrowing is
    // caught (not just `!admits_file` layer toggles). Discovery warnings are
    // immaterial to the prologue (best-effort, [ADR-11]) and dropped.
    let mut warnings = Vec::new();
    let candidates = discover_candidates(root, config, registry, &mut warnings)?;
    let admitted: HashSet<&str> = candidates.iter().map(|c| c.rel.as_str()).collect();
    // The navigation prologue deliberately defers inbound-ref demotion to the
    // next governance reconcile ([FR-SY-08], [NFR-PE-01]); take only the count
    // and discard the captured node names that the write path uses to re-resolve.
    let purged = purge_unadmitted(runtime, &admitted)?.count;

    // Record last, after the complete purge over the full stored set, so a second
    // prologue (a later process) sees a matching fingerprint and does no further
    // purge work ([FR-SY-08] at-most-once AC, [ADR-20]). Deliberately no resolve
    // pass: node deletion already cascades the inbound edges away, so reads cannot
    // surface unadmitted symbols; re-flipping the producing ledger rows back to
    // `unresolved_refs` is the next governance reconcile's job, kept out of the
    // point-query path to honour the [NFR-PE-01] budget and the no-per-call-
    // reconcile rule.
    record_admission_fingerprint(runtime, &fingerprint)?;
    Ok(purged)
}

/// Remove a stored file's entire graph footprint — the removal path shared by a
/// disk-deletion sync and a config-narrowing purge ([ADR-10], [FR-SY-07]).
///
/// Deletes the file's nodes (cascading their incident edges, including the
/// inbound cross-file edges from still-admitted callers in other files) and then
/// the file row (whose `ON DELETE CASCADE` clears the file's own ledger rows).
/// The *producing* ledger rows of those inbound edges live on the other files
/// and survive; the next resolution pass re-evaluates them, finds the target
/// gone, and flips them back to unresolved — inbound references return to
/// `unresolved_refs`, never fabricated ([NFR-RA-05]). A path with no file row is
/// a no-op, so a double removal (e.g. a purge then a sync of the same path) is
/// idempotent.
fn remove_file(runtime: &Runtime, rel: String) -> Result<()> {
    runtime.submit_write(move |w| {
        if let Some(file_id) = w.file_id(&rel)? {
            w.delete_nodes_for_file(file_id)?;
            w.delete_file(file_id)?;
        }
        Ok(())
    })
}

// ── Internals ────────────────────────────────────────────────────────────────

/// A discovered source file: its absolute path and project-relative key.
struct Candidate {
    abs: PathBuf,
    rel: String,
}

/// Discover the supported source files under `root`, honouring config and
/// gitignore, and fold any oversize-skip notices into `warnings`.
fn discover_candidates(
    root: &Path,
    config: &Config,
    registry: &LanguageRegistry,
    warnings: &mut Vec<String>,
) -> Result<Vec<Candidate>> {
    let report = config::discover(root, config)?;
    for notice in report.notices() {
        warnings.push(notice);
    }
    // Surface any documentation directory-symlink that exists under the doc-
    // include set but ended up unindexed ([FR-IX-11]) — a git-ignored symlink with
    // no sanctioned bypass, or one whose target escapes containment — so the
    // silent doc-drop [CR-071] closes becomes a loud `index`/`sync` warning.
    for drop in &report.unindexed_doc_symlinks {
        warnings.push(drop.to_string());
    }
    // `discover` walks the canonicalised root and yields paths beneath it.
    let canon_root = root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", root.display()))?;

    // The doc-admission matcher, compiled once for the whole walk (S-034); `None`
    // when documentation is disabled. A malformed doc glob fails here exactly as
    // a bad include/exclude does (already validated at config load).
    let doc_globs = config.documentation.compile()?;
    // The config-artifact admission matcher (S-062, CR-010), same shape; `None`
    // when the config layer is disabled.
    let config_globs = config.config_artifacts.compile()?;
    // CR-017 / S-081: the code-language admission allowlist (`None` = all
    // compiled-in code grammars; a non-empty `languages` restricts to those).
    let langs = config.language_allowlist();

    let mut candidates = Vec::new();
    for abs in report.files {
        let Ok(rel_path) = abs.strip_prefix(&canon_root) else {
            continue; // defence in depth — discovery already contains the walk
        };
        let rel = to_forward_slash(rel_path);
        if admits_file(registry, doc_globs.as_ref(), config_globs.as_ref(), langs.as_ref(), &rel) {
            candidates.push(Candidate { abs, rel });
        }
    }
    Ok(candidates)
}

/// Read and hash each candidate, skipping (with a warning) any that cannot be
/// read as UTF-8; the skipped paths are also recorded in `files_failed` — the
/// `INCOMPLETE` input the governance engine stamps from (NFR-RA-11, S-020).
///
/// Each file's read + blake3 hash is independent, so the work runs on the shared
/// core-owned worker pool ([FR-IX-09], [FR-IX-03], [NFR-PE-08]) — the same pool
/// extraction uses ([AQ-04]) — rather than the global rayon pool. Output is
/// order-deterministic regardless of the worker count ([NFR-RA-06]): the
/// parallel map preserves candidate order, and the serial fold below drains that
/// ordered result so `loaded`, `warnings`, and `files_failed` are byte-identical
/// to the single-threaded path.
fn load_files(
    runtime: &Runtime,
    candidates: &[Candidate],
    warnings: &mut Vec<String>,
    files_failed: &mut Vec<String>,
) -> Vec<LoadedFile> {
    // Read + hash in parallel on the worker pool. `map` over a `par_iter`
    // preserves input order through `collect` (NFR-RA-06), so the results line up
    // one-to-one with `candidates`. Each item is either a loaded file or the
    // (warning, failed-rel) pair the serial path would have pushed.
    let outcomes: Vec<Result<LoadedFile, (String, String)>> = runtime.worker_pool().install(|| {
        candidates
            .par_iter()
            .map(|c| match fs::read_to_string(&c.abs) {
                Ok(source) => {
                    let hash = hash_source(&source);
                    Ok(LoadedFile {
                        rel: c.rel.clone(),
                        source,
                        hash,
                    })
                }
                Err(e) => Err((
                    format!("{}: unreadable or non-UTF-8, skipped ({e})", c.rel),
                    c.rel.clone(),
                )),
            })
            .collect()
    });

    // Fold the ordered results back into the caller's accumulators on this
    // thread — this reproduces the serial push order exactly, so warnings and
    // `files_failed` keep the same sequence a single-threaded load produced.
    let mut loaded = Vec::with_capacity(candidates.len());
    for outcome in outcomes {
        match outcome {
            Ok(file) => loaded.push(file),
            Err((warning, failed)) => {
                warnings.push(warning);
                files_failed.push(failed);
            }
        }
    }
    loaded
}

/// The aggregate result of extracting and persisting a set of loaded files.
struct ExtractOutcome {
    files: usize,
    nodes: u64,
    edges: u64,
}

/// Extract `loaded` on the shared worker pool and persist each file's facts in
/// its own write batch.
///
/// Returns the outcome plus the wall-clock of the two timed phases it owns —
/// `extract_ms` and `persist_ms` — each measured once through the single
/// `tracing` seam (FR-OB-01, CR-057) so they feed the per-phase index
/// breakdown (FR-OB-06) without a parallel timing path (NFR-OO-01). The persist
/// loop is now timed distinctly from extraction so the cold-index profiler can
/// attribute the serial write cost CR-057 targets.
fn extract_and_persist(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    loaded: &[LoadedFile],
    capture: bool,
    enrich: bool,
    warnings: &mut Vec<String>,
) -> Result<(ExtractOutcome, u64, u64)> {
    let inputs: Vec<FileInput> = loaded
        .iter()
        .map(|l| FileInput::new(l.rel.clone(), l.source.clone()))
        .collect();
    let hash_by_rel: HashMap<&str, &str> = loaded
        .iter()
        .map(|l| (l.rel.as_str(), l.hash.as_str()))
        .collect();

    let ctx = SymbolContext::default();
    // Run the rayon-parallel extraction on the core-owned worker pool (AQ-04),
    // not the global rayon pool, so all CPU parallelism shares one pool.
    // Pass 1 is one of the instrumented pipeline passes (FR-OB-01).
    let (mut facts, extract_ms) = crate::observability::traced_infallible_timed("extract", || {
        runtime
            .worker_pool()
            .install(|| extract_files(&inputs, registry, &ctx))
    });

    // swe-skills typed-node enrichment (S-039, FR-DG-07): a pure post-extraction
    // relabel of the convention artifacts' doc nodes to their typed kinds. A
    // no-op when enrichment is inactive (plain repo, or config-disabled), so the
    // generic doc layer is unaffected.
    crate::extract::doc::enrich::promote_facts(&mut facts, enrich);

    // Persistence — CR-057 / FR-IX-08. The old loop cloned each file's `Facts`
    // and blocked on its *own* writer round-trip (958 commits on the Logos
    // repo). It is replaced by **bounded chunked write batches**: each chunk is
    // one transaction through the *same* single writer (ADR-02, NFR-RA-07) —
    // far fewer, larger commits, the owned facts move straight into the writer
    // closure (no per-file clone), and one round-trip per chunk instead of one
    // per file. Timed distinctly from extraction through the same seam
    // (FR-OB-01) so the cold-index profiler still attributes the write cost.
    let (outcome, persist_ms) = crate::observability::traced_timed("persist", || {
        persist_facts_chunked(
            runtime,
            &mut facts,
            &hash_by_rel,
            capture,
            PERSIST_CHUNK_FILES,
            warnings,
        )
    });
    Ok((outcome?, extract_ms, persist_ms))
}

/// Files per Pass-1 write transaction on a full index (CR-057, [FR-IX-08]).
///
/// Chunking collapses the per-file commit storm into a handful of larger
/// transactions while keeping each transaction — and its rollback granularity
/// (NFR-RA-07) — *bounded* regardless of repo size. A full index is an
/// authoritative rebuild, so per-chunk (rather than per-file) atomicity is
/// acceptable ([FR-IX-08]); incremental `sync` keeps its per-file
/// capture-before-delete path ([ADR-10]).
///
/// [FR-IX-08]: ../../../docs/specs/requirements/FR-IX-08.md
/// [ADR-10]: ../../../docs/specs/architecture/decisions/ADR-10.md
const PERSIST_CHUNK_FILES: usize = 256;

/// Persist every file's facts on a **full index** as bounded chunked write
/// batches (CR-057, [FR-IX-08]).
///
/// Owned `Facts` are drained in extraction (= input) order and moved straight
/// into the writer closure, so a full index pays **no per-file `Facts` clone**
/// and issues **one writer round-trip per chunk** instead of one per file. File
/// order is preserved across chunks, so node/file ids are assigned exactly as
/// the per-file baseline would — the graph output stays byte-identical and
/// batch-size-independent ([NFR-RA-06]). The single-writer invariant is intact:
/// every chunk still funnels through the one [`Runtime::submit_write`] actor
/// ([ADR-02]).
///
/// [FR-IX-08]: ../../../docs/specs/requirements/FR-IX-08.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
fn persist_facts_chunked(
    runtime: &Runtime,
    facts: &mut Vec<Facts>,
    hash_by_rel: &HashMap<&str, &str>,
    capture: bool,
    chunk_files: usize,
    warnings: &mut Vec<String>,
) -> Result<ExtractOutcome> {
    // A zero bound would never flush; clamp to 1 (degenerate per-file behavior)
    // so "every file is persisted" holds for any caller-provided bound — and so
    // chunk_files == 1 reproduces the per-file-transaction baseline exactly.
    let chunk_files = chunk_files.max(1);

    let mut outcome = ExtractOutcome {
        files: 0,
        nodes: 0,
        edges: 0,
    };
    let mut chunk: Vec<(Facts, String)> = Vec::with_capacity(chunk_files);

    for f in facts.drain(..) {
        // Non-fatal extraction diagnostics are surfaced on the calling thread —
        // the writer closure runs on another thread and cannot borrow
        // `warnings` — before the facts move into the chunk.
        for w in &f.warnings {
            warnings.push(format!("{}: {w}", f.path));
        }
        let hash = hash_by_rel
            .get(f.path.as_str())
            .copied()
            .unwrap_or("")
            .to_string();
        chunk.push((f, hash));

        if chunk.len() == chunk_files {
            let batch = std::mem::replace(&mut chunk, Vec::with_capacity(chunk_files));
            persist_chunk(runtime, batch, capture, &mut outcome)?;
        }
    }
    // Flush the final partial chunk.
    if !chunk.is_empty() {
        persist_chunk(runtime, chunk, capture, &mut outcome)?;
    }

    Ok(outcome)
}

/// Persist one bounded chunk of file facts as a **single write transaction**
/// through the shared single writer ([ADR-02], [NFR-RA-07]).
///
/// Every file in the chunk runs against the *same* [`BatchWriter`], so the
/// chunk commits as a unit — or, on any file's error, rolls back **wholesale**
/// with no partial rows ([NFR-RA-07]). The chunk moves into the closure (the
/// writer runs it on its own thread), so there is no per-file clone and exactly
/// one round-trip for the whole chunk. Counts are accumulated into `outcome`.
///
/// [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
/// [NFR-RA-07]: ../../../docs/specs/requirements/NFR-RA-07.md
fn persist_chunk(
    runtime: &Runtime,
    chunk: Vec<(Facts, String)>,
    capture: bool,
    outcome: &mut ExtractOutcome,
) -> Result<()> {
    let (files, nodes, edges) = runtime.submit_write(move |w| {
        let mut files = 0usize;
        let mut nodes = 0u64;
        let mut edges = 0u64;
        for (facts, hash) in &chunk {
            let counts = persist_file(w, facts, hash, capture)?;
            files += 1;
            nodes += counts.nodes;
            edges += counts.edges;
        }
        Ok((files, nodes, edges))
    })?;
    outcome.files += files;
    outcome.nodes += nodes;
    outcome.edges += edges;
    Ok(())
}

/// Submit one file's facts to the writer actor and collect non-fatal warnings.
///
/// This is the **incremental `sync`** persist unit: one transaction per file so
/// each file's capture-before-delete is atomic ([ADR-10], [NFR-PE-03]). A full
/// index instead batches files into bounded chunks via [`persist_facts_chunked`]
/// (CR-057, [FR-IX-08]) — this per-file path is deliberately left unchanged for
/// `sync`.
fn persist_one(
    runtime: &Runtime,
    facts: &Facts,
    hash: &str,
    capture: bool,
    warnings: &mut Vec<String>,
) -> Result<PersistCounts> {
    for w in &facts.warnings {
        warnings.push(format!("{}: {w}", facts.path));
    }
    // One transaction per file (NFR-RA-07). The closure owns its inputs because
    // the writer runs it on its own thread; cloning `Facts` here is the simple,
    // correct choice for the dirty-set-sized sync loop.
    let facts = facts.clone();
    let hash = hash.to_string();
    runtime.submit_write(move |w| persist_file(w, &facts, &hash, capture))
}

/// Persist one file's nodes, edges, and reference-ledger rows inside an open
/// write batch, replacing any previously indexed version of the file and
/// (when `capture`) preserving the cross-file edges that point into it as
/// exact-symbol `unresolved_refs` for the resolution pass to rebind
/// ([ADR-10]).
fn persist_file(
    w: &BatchWriter<'_>,
    facts: &Facts,
    hash: &str,
    capture: bool,
) -> Result<PersistCounts> {
    // Replace-in-place: keep the file row's id stable across a re-extract so any
    // FK to it survives, but rebuild its nodes/edges/refs from the fresh facts.
    let file_id = match w.file_id(&facts.path)? {
        Some(file_id) => {
            // Capture-before-delete: snapshot inbound cross-file edges BEFORE the
            // node delete cascades them away ([ADR-10]). They are persisted as
            // exact-symbol ledger rows below, and Pass 2 rebinds them.
            let captured = if capture {
                w.inbound_cross_file_edges(file_id)?
            } else {
                Vec::new()
            };
            w.delete_nodes_for_file(file_id)?;
            w.update_file(file_id, Some(&facts.language), Some(hash))?;
            // Replace the file's ledger rows wholesale: stale refs from the
            // previous version never linger.
            w.delete_unresolved_refs_for_file(file_id)?;
            let counts = insert_facts(w, facts, file_id)?;
            insert_refs(w, facts, file_id)?;
            for cap in &captured {
                let kind = EdgeKind::try_from(cap.kind)
                    .with_context(|| format!("captured edge has an unknown kind {}", cap.kind))?;
                // An exact-symbol ref: resolution binds it by pure lookup —
                // or leaves it unresolved if the edit renamed the target away
                // (never invented, NFR-RA-05).
                w.insert_unresolved_ref(&NewUnresolvedRef {
                    file_id: Some(file_id),
                    source_symbol: &cap.source_symbol,
                    target: &cap.target_symbol,
                    alias: None,
                    form: RefForm::Symbol,
                    kind,
                    line: None,
                    // A captured cross-file edge is re-bound by exact symbol; its
                    // relation payload (if any) is re-derived when the owning file
                    // is re-extracted, so the capture row carries none.
                    payload: None,
                })?;
            }
            return Ok(PersistCounts {
                nodes: counts.nodes,
                edges: counts.edges,
            });
        }
        None => w.insert_file(&facts.path, Some(&facts.language), Some(hash))?,
    };

    let counts = insert_facts(w, facts, file_id)?;
    insert_refs(w, facts, file_id)?;
    Ok(PersistCounts {
        nodes: counts.nodes,
        edges: counts.edges,
    })
}

/// Persist a file's extracted references into the `unresolved_refs` ledger
/// (S-011). Insertion is idempotent over the ledger's uniqueness rule.
fn insert_refs(w: &BatchWriter<'_>, facts: &Facts, file_id: i64) -> Result<()> {
    for r in &facts.refs {
        w.insert_unresolved_ref(&NewUnresolvedRef {
            file_id: Some(file_id),
            source_symbol: r.source.as_str(),
            target: &r.target,
            alias: r.alias.as_deref(),
            form: r.form,
            kind: r.kind,
            line: Some(i64::from(r.line)),
            // The cross-artifact relation class rides into the ledger as the
            // payload token (CR-011); `None` for every code/doc/access ref.
            payload: r.relation.map(|rel| rel.as_str()),
        })?;
    }
    Ok(())
}

/// The per-file insert tally returned by [`insert_facts`].
struct InsertedNodes {
    nodes: u64,
    edges: u64,
}

/// Insert a file's nodes and its intra-file edges (the Pass-1 `Contains`
/// edges, both endpoints in this file). The symbol → node-id map lives only
/// for edge wiring; cross-file linking is the resolution pass's job (S-011).
fn insert_facts(w: &BatchWriter<'_>, facts: &Facts, file_id: i64) -> Result<InsertedNodes> {
    let mut symbol_to_node: HashMap<String, NodeId> = HashMap::with_capacity(facts.nodes.len());
    for n in &facts.nodes {
        let symbol_id = w.upsert_symbol(&n.symbol)?;
        let node_id = w.insert_node(&NewNode {
            file_id: Some(file_id),
            start_line: Some(i64::from(n.start_line)),
            end_line: Some(i64::from(n.end_line)),
            // The S-014 annotation inputs captured by Pass 1: visibility for
            // exported-is-live (FR-AN-01), per-function metrics (FR-AN-04),
            // and the AST-shape fingerprint (FR-AN-02). The S-027 test-marker
            // evidence (FR-EX-06) rides the same AST-in-hand pass — the
            // persisted input to the unified is_test annotation (FR-AN-05).
            exported: n.exported,
            cyclomatic_complexity: n.metrics.map(|m| i64::from(m.cyclomatic_complexity)),
            line_count: n.metrics.map(|m| i64::from(m.line_count)),
            fingerprint: n.fingerprint.as_deref(),
            test_evidence: n.test_evidence,
            // FTS-indexed body text: a DocSection's prose (FR-DG-05, S-037) or a
            // config typed-anchor's payload subtype (S-065, CR-010, FR-CG-03) —
            // NULL on every code node; the FTS triggers index it alongside name.
            body: n.body.as_deref(),
            // The CR-005 per-function max nesting depth (FR-EX-07) — NULL on
            // every non-callable node.
            max_nesting_depth: n.max_nesting_depth.map(i64::from),
            ..NewNode::plain(symbol_id, n.kind, &n.name)
        })?;
        // The CR-005 winnowed near-clone shingle set (FR-EX-09) — persisted into
        // the inverted index keyed on the node just inserted; empty for a
        // non-callable or a body below the token floor.
        w.insert_shingles(node_id, &n.shingles)?;
        symbol_to_node.insert(n.symbol.to_string(), node_id);
    }

    let mut edges = 0u64;
    for e in &facts.edges {
        if let (Some(&source), Some(&target)) = (
            symbol_to_node.get(e.source.as_str()),
            symbol_to_node.get(e.target.as_str()),
        ) {
            w.insert_edge(source, target, e.kind)?;
            edges += 1;
        }
    }

    Ok(InsertedNodes {
        nodes: facts.nodes.len() as u64,
        edges,
    })
}

/// Pass 2 — resolution ([resolution-engine], [ADR-10], [S-011]).
///
/// Delegates to [`crate::resolve::run`]: snapshot the graph, bind the whole
/// `unresolved_refs` ledger (parallel compute on the shared worker pool),
/// commit bound edges serially through the writer actor. Unbindable refs
/// survive in the ledger and are retried on the next sync ([FR-RS-03],
/// [NFR-RA-05]).
///
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
/// [S-011]: ../../../docs/planning/journal.md#s-011-resolution-engine
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn resolve_pass(
    runtime: &Runtime,
    policy: BindingPolicy,
    delta: Option<&crate::resolve::Delta>,
) -> Result<(ResolutionStats, u64)> {
    // Pass 2 — instrumented through the single emission seam (FR-OB-01). `delta`
    // is `None` on a full index (re-bind the whole ledger) and `Some` on a sync
    // (re-bind only the change-affected rows, CR-015). The measured wall-clock
    // rides back for the per-phase index breakdown (FR-OB-06, CR-057).
    let (res, ms) = crate::observability::traced_timed("resolve", || {
        crate::resolve::run(runtime, policy, delta)
    });
    Ok((res?, ms))
}

/// The framework-promotion pass ([resolution-engine], S-012, [FR-FW-01]..
/// [FR-FW-04]).
///
/// Runs after [`resolve_pass`] on both [`index`] and [`sync`]: detects
/// framework usage from the reference ledger, matches Axum/Actix route and
/// state patterns in the candidate files, and reconciles the promoted
/// `route`/`component` nodes to the current desired set — never fabricating a
/// handler link the binder cannot prove ([NFR-RA-05]).
///
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
/// [FR-FW-01]: ../../../docs/specs/requirements/FR-FW-01.md
/// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
///
/// Returns the [`FrameworkStats`] plus the **names of the route/component nodes
/// it newly promoted** this run, which [`rebind_for_promotions`] turns into a
/// focused re-resolve so deferred cross-artifact references bind to them
/// (CR-017 / S-080).
fn framework_pass(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    policy: BindingPolicy,
    delta: Option<&crate::resolve::Delta>,
) -> Result<(FrameworkStats, Vec<String>)> {
    crate::resolve::framework::run(runtime, registry, root, policy, delta)
}

/// The framework-dispatch live-rooting pass ([resolution-engine], [CR-043],
/// [ADR-39], [FR-RS-03]).
///
/// Runs after [`framework_pass`] on both [`index`] and [`sync`]: recognises the
/// Rust methods dispatched only through an external framework (trait-impl
/// dispatch, `#[tool]` tool dispatch) and live-roots each with a
/// `RoutesTo` self-edge marker, so the dead-code reachability pass
/// ([`annotate_pass`]) no longer mis-reports them dead ([FR-AN-01]) — never
/// fabricating an edge between distinct nodes, biased toward false-live
/// ([NFR-RA-05], [AR-05]). The marker set is reconciled per run (whole-graph on
/// a full index, change-set-scoped on a sync), so a full index and an
/// incremental sync converge on a byte-identical set ([NFR-RA-06]).
///
/// [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md
/// [CR-043]: ../../../docs/requests/CR-043-dead-code-detector-precision.md
/// [ADR-39]: ../../../docs/specs/architecture/decisions/ADR-39.md
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
/// [AR-05]: ../../../docs/specs/architecture.md#13-risk-register
fn dispatch_pass(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    delta: Option<&crate::resolve::Delta>,
) -> Result<DispatchStats> {
    crate::resolve::dispatch::run(runtime, registry, root, delta)
}

/// Re-resolve the references that target the framework pass's **newly-promoted**
/// route/component nodes (CR-017 / S-080).
///
/// The framework pass creates `route`/`component` nodes *after* [`resolve_pass`]
/// has already run, so a cross-artifact reference that targets one — an OpenAPI
/// `ApiOperation`'s route reference ([FR-CG-09]), captured at extraction — could
/// not bind in that resolve, and the CR-015 incremental sync that follows never
/// re-attempts it (its delta is built from changed code files; a no-op sync has
/// none). This runs a focused resolve keyed on the promoted nodes' name tokens so
/// those deferred references bind on the same run that promotes their target,
/// honouring the retry-on-sync contract ([FR-RS-03]) without re-binding the whole
/// ledger. A no-op when nothing new was promoted, so a plain library and an
/// unchanged sync pay nothing.
///
/// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
/// [FR-RS-03]: ../../../docs/specs/requirements/FR-RS-03.md
///
/// Returns the wall-clock (ms) of the focused re-resolve it ran — `0` when it
/// was a no-op — so the caller folds it into the resolve-phase total for the
/// per-phase index breakdown (FR-OB-06, CR-057).
fn rebind_for_promotions(
    runtime: &Runtime,
    policy: BindingPolicy,
    promoted: &[String],
    resolution: &mut ResolutionStats,
) -> Result<u64> {
    if promoted.is_empty() {
        return Ok(0);
    }
    let mut dirty_tokens: HashSet<String> = HashSet::new();
    for name in promoted {
        dirty_tokens.extend(crate::resolve::tokens(name));
    }
    if dirty_tokens.is_empty() {
        return Ok(0);
    }
    let delta = crate::resolve::Delta {
        changed_paths: HashSet::new(),
        dirty_tokens,
    };
    let (res, ms) = resolve_pass(runtime, policy, Some(&delta))?;
    *resolution = res;
    Ok(ms)
}

/// Pass 3 — annotation ([annotation-engine], [S-014]).
///
/// Loads the `rules.toml` architecture contract fresh from `<root>/.logos/`
/// (defaulted when absent — checked-in policy may not exist in every worktree,
/// [NFR-DM-04]) and delegates to [`crate::annotate::run`]: dead-code with
/// exported-is-live, fingerprint duplicates, and layer policy materialisation,
/// all to native node columns ([FR-AN-01]..[FR-AN-04]).
///
/// [annotation-engine]: ../../../docs/specs/architecture/components/annotation-engine.md
/// [S-014]: ../../../docs/planning/journal.md#s-014-annotation-engine
/// [NFR-DM-04]: ../../../docs/specs/requirements/NFR-DM-04.md
/// [FR-AN-01]: ../../../docs/specs/requirements/FR-AN-01.md
/// [FR-AN-04]: ../../../docs/specs/requirements/FR-AN-04.md
fn annotate_pass(
    runtime: &Runtime,
    registry: &LanguageRegistry,
    root: &Path,
    config: &Config,
    incremental: bool,
) -> Result<(AnnotationStats, u64)> {
    // Pass 3 — instrumented through the single emission seam (FR-OB-01).
    // `incremental` (a `sync`/`reconcile`) writes only the verdicts that changed;
    // a full `index` writes every node — the compute stays whole-graph either way
    // (S-024-HF). The measured wall-clock rides back for the per-phase index
    // breakdown (FR-OB-06, CR-057).
    let (res, ms) = crate::observability::traced_timed("annotate", || {
        let rules = config::load_rules_from_root(root)?;
        // CR-043 / ADR-39: dead-code reachability is computed only for languages
        // that declare the reachability capability on their descriptor; the
        // others render `is_dead = NULL`. The capable-extension set is derived
        // fresh from the loaded registry each run, so it tracks the compiled-in
        // grammars deterministically (NFR-RA-06).
        let reachable_exts = registry.reachability_extensions();
        crate::annotate::run(
            runtime,
            &rules,
            &config.semantics.entry_points,
            &config.semantics.test_markers,
            &reachable_exts,
            incremental,
        )
    });
    Ok((res?, ms))
}

/// The blake3 content hash of a source file, hex-encoded ([FR-SY-03]).
fn hash_source(source: &str) -> String {
    blake3::hash(source.as_bytes()).to_hex().to_string()
}

/// Does any loaded **code** grammar claim this file's extension?
///
/// Documentation grammars (S-033, [ADR-19]) are excluded here: this is the
/// *code*-discovery gate. Whether a markdown file enters the pipeline is the
/// separate documentation gate ([`is_doc_admitted`]) — scoped by the doc globs
/// and the `[documentation]` toggle (S-034) — and the two compose in
/// [`admits_file`], the predicate discovery and sync actually use.
///
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
fn supported_extension(
    registry: &LanguageRegistry,
    langs: Option<&HashSet<String>>,
    rel: &str,
) -> bool {
    Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| registry.for_extension(ext))
        // Documentation (S-033) and config/artifact (S-062, CR-010) grammars are
        // excluded here: this is the *code*-discovery gate. Each has its own gate
        // ([`is_doc_admitted`]/[`is_config_admitted`]) so an artifact file with a
        // claimed extension (e.g. `.yaml`) is never (re-)extracted as code.
        .is_some_and(|plugin| {
            !plugin.is_documentation()
                && !plugin.is_artifact()
                // CR-017 / S-081 / FR-CF-01: the `languages` allowlist gates code
                // admission. `None` (empty/omitted list) admits every compiled-in
                // code grammar — the twelve-out-of-the-box default; a non-empty
                // list admits only its named grammars.
                && langs.is_none_or(|set| set.contains(plugin.name()))
        })
}

/// Is `rel` admitted as **documentation** (S-034, [FR-DG-01], [ADR-19])?
///
/// True only when its extension resolves to a *documentation* plugin **and** the
/// compiled doc globs admit the path. `docs` is `None` when documentation is
/// disabled (`[documentation] enabled = false`), so a disabled config admits no
/// markdown. This is the authoritative doc-glob gate even when the master walk's
/// `**` include surfaced the file — it rejects an `.md` outside the doc globs.
///
/// [FR-DG-01]: ../../../docs/specs/requirements/FR-DG-01.md
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
fn is_doc_admitted(registry: &LanguageRegistry, docs: Option<&DocGlobs>, rel: &str) -> bool {
    let Some(docs) = docs else {
        return false; // documentation disabled
    };
    Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| registry.for_extension(ext))
        .is_some_and(|plugin| plugin.is_documentation() && docs.admits(rel))
}

/// Is `rel` admitted as a **config artifact** (S-062, [CR-010], [FR-CG-01],
/// [FR-IX-02] as modified)?
///
/// True only when an *artifact* plugin claims the file — by **extension or
/// basename** ([`LanguageRegistry::for_path`]), so an extensionless `Dockerfile`
/// is admitted — **and** the compiled config globs admit the path. `config` is
/// `None` when the layer is disabled (`[config_artifacts] enabled = false`), so a
/// disabled config admits no artifact. The default lock-file excludes ([BR-30])
/// live in the glob set, so `package-lock.json` is rejected here by default.
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [FR-CG-01]: ../../../docs/specs/requirements/FR-CG-01.md
/// [FR-IX-02]: ../../../docs/specs/requirements/FR-IX-02.md
fn is_config_admitted(
    registry: &LanguageRegistry,
    config: Option<&ConfigGlobs>,
    rel: &str,
) -> bool {
    let Some(config) = config else {
        return false; // the config-artifact layer is disabled
    };
    registry
        .for_path(rel)
        .is_some_and(|plugin| plugin.is_artifact() && config.admits(rel))
}

/// Should `rel` enter the pipeline at all — as code, documentation, or a config
/// artifact?
///
/// The single admission predicate discovery ([`discover_candidates`]) and sync
/// ([`sync`]) share, so the two passes can never disagree on what is indexable.
/// A markdown doc (S-034) or a config artifact (S-062) rides the same
/// discover → extract → persist machinery as code once admitted ([ADR-19],
/// [ADR-25]).
///
/// [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
/// [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
fn admits_file(
    registry: &LanguageRegistry,
    docs: Option<&DocGlobs>,
    config: Option<&ConfigGlobs>,
    langs: Option<&HashSet<String>>,
    rel: &str,
) -> bool {
    supported_extension(registry, langs, rel)
        || is_doc_admitted(registry, docs, rel)
        || is_config_admitted(registry, config, rel)
}

/// Normalise a path's components to a forward-slash project-relative string.
fn to_forward_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Resolve an input path (absolute or root-relative) to a project-relative key,
/// rejecting anything that escapes the root ([NFR-SE-04]).
fn relativize(canon_root: &Path, path: &Path) -> Option<String> {
    let rel: &Path = if path.is_absolute() {
        path.strip_prefix(canon_root).ok()?
    } else {
        path
    };
    // Reject `..`/root components so a crafted changed-file set cannot escape
    // ([NFR-SE-04]); drop no-op `.` (CurDir) components so `./a.rs` and `a.rs`
    // normalise to the same project-relative key the index stored.
    let mut normalized = PathBuf::new();
    for component in rel.components() {
        match component {
            Component::CurDir => continue,
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    let s = to_forward_slash(&normalized);
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

#[cfg(all(test, feature = "lang-rust"))]
mod tests;
