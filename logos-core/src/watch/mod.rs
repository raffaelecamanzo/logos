//! The debounced filesystem watcher hosted under `serve --mcp` ([S-022],
//! [FR-SY-04], [filesystem-watcher]).
//!
//! While the MCP server runs, `notify` + `notify-debouncer-full` deliver
//! OS file-change notifications (FSEvents on macOS, inotify on Linux),
//! debounced over the configurable `[watcher] debounce_ms` window (default
//! 300 ms, the resolved SRS OQ-04 value), then further coalesced by the
//! sync-worker **cadence** (CR-015, `[watcher] settle_ms`/`min_sync_interval_ms`/
//! `max_staleness_ms`): a burst of debounced batches collapses into one
//! incremental [`Engine::sync`] once edits settle — submitted to the
//! single-writer actor ([ADR-02]) — so navigation stays current as the
//! developer/agent edits without paying a sync per keystroke.
//!
//! # Non-load-bearing for correctness ([FR-SY-06], [ADR-11])
//!
//! The watcher is an optimization for navigation *latency*, never a
//! correctness dependency: a dropped, late, or never-delivered event is
//! repaired by the next evaluation's reconcile backstop. Every failure path
//! in this module therefore degrades (log + drop) rather than propagates —
//! losing an event costs at most a slightly stale navigation answer.
//!
//! # Backpressure: drop-and-coalesce (the [AQ-01] resolution)
//!
//! Write-queue backpressure is **hybrid by source**: correctness-bearing
//! index/explicit writes keep the writer queue's bounded-*block* submit
//! (their caller asked for them, so they must not be lost), while watcher
//! events — mere freshness hints — use **drop-and-coalesce**, realized here
//! as two self-bounding pieces:
//!
//! 1. a **pending path-set** the debouncer callback merges into — bounded by
//!    the number of *distinct* dirty files, not the number of events: a
//!    10,000-event storm on 5 files occupies 5 entries, and
//! 2. a **1-slot wake channel** to the sync worker — a full slot means a wake
//!    is already pending, so the send is *dropped* (the wake it would have
//!    delivered is already guaranteed; nothing is lost, hence "coalesce").
//!
//! # Writer-actor starvation guard ([AR-01])
//!
//! The sync worker drains the pending set and runs **at most one** sync at a
//! time, so the watcher can never contribute more than one in-flight write
//! batch to the writer queue regardless of storm intensity — an edit storm
//! cannot crowd out an agent-triggered reconcile or index. Navigation reads
//! are immune by construction: they run on the read-only WAL pool and are
//! never blocked by any writer ([ADR-02], [NFR-PE-01]).
//!
//! # Feedback-loop containment
//!
//! Events under `.logos/` (the store the sync itself writes!) and `.git/`
//! are filtered *before* coalescing — without this, every sync's own
//! `logos.db` write would re-trigger the watcher forever.
//!
//! [S-022]: ../../../docs/planning/journal.md#s-022-incremental-sync-hardening-with-watcher-and-git-hooks
//! [FR-SY-04]: ../../../docs/specs/requirements/FR-SY-04.md
//! [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
//! [AQ-01]: ../../../docs/specs/architecture.md#14-open-questions
//! [AR-01]: ../../../docs/specs/architecture.md#13-risk-register
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [filesystem-watcher]: ../../../docs/specs/architecture/integrations/filesystem-watcher.md

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use notify::RecursiveMode;
use notify_debouncer_full::file_id::{get_file_id, FileId};
use notify_debouncer_full::{new_debouncer_opt, DebounceEventResult, FileIdCache};

use crate::config::AdmissionAuthority;
use crate::engine::Engine;
use crate::history::coverage::ArtifactMatcher;

#[cfg(test)]
mod tests;

/// Directory names whose subtrees never feed the watcher: `.logos` holds the
/// store the sync itself writes (feedback loop) and `.git` churns on every
/// git operation without ever being source code.
///
/// These are the **feedback-loop** filter: unlike the indexer's `ignored_dirs`,
/// no allow-list exception (the coverage-artifact hook, [FR-CV-10]) ever re-admits
/// a path under them — checked first in [`classify`] so a stray artifact glob can
/// never re-open the self-trigger melt ([ADR-38], [ADR-11]).
const INTERNAL_DIRS: &[&str] = &[".logos", ".git"];

/// How the watcher classifies a changed path: drive an incremental [`Engine::sync`]
/// ([`Source`](Admission::Source)), auto-ingest it as a coverage artifact
/// ([`Coverage`](Admission::Coverage), [FR-CV-10]), or drop it
/// ([`Ignored`](Admission::Ignored)).
///
/// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admission {
    /// An indexed source path → coalesce into the next `Engine::sync`.
    Source,
    /// A coverage artifact (convention/configured) → auto-ingest it ([FR-CV-10]).
    Coverage,
    /// Outside the root, under a feedback-loop/ignored dir → dropped.
    Ignored,
}

/// Live counters for the watcher's behaviour, shared between the debouncer
/// callback, the sync worker, and [`WatchHandle::stats`].
///
/// These are the observable face of the drop-and-coalesce policy: tests (and
/// a future `stats` surface) assert storm behaviour against them instead of
/// guessing from timing.
#[derive(Default)]
struct Counters {
    /// Debounced batches delivered by the OS watcher (post-debounce).
    batches_delivered: AtomicU64,
    /// Paths accepted into the pending set (post-filter, pre-coalesce).
    paths_accepted: AtomicU64,
    /// Wakes dropped because one was already pending — each is a debounced
    /// batch coalesced into an earlier one (the "drop" of drop-and-coalesce).
    wakes_coalesced: AtomicU64,
    /// Syncs the worker actually ran (each is one write-batch sequence).
    syncs_run: AtomicU64,
    /// Files the syncs reported touched (added + modified + removed).
    files_synced: AtomicU64,
    /// Coverage-artifact paths accepted into the pending artifact set
    /// (post-filter, the allow-list exception, [FR-CV-10]).
    artifacts_accepted: AtomicU64,
    /// Auto-ingests the worker actually attempted (one per changed artifact);
    /// failures are counted here too — they degrade to a warning, [ADR-38].
    coverage_ingests_run: AtomicU64,
}

/// A point-in-time snapshot of the watcher's counters ([`WatchHandle::stats`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchStats {
    /// Debounced batches delivered by the OS watcher.
    pub batches_delivered: u64,
    /// Paths accepted into the pending set (post-filter).
    pub paths_accepted: u64,
    /// Debounced batches coalesced into an already-pending wake.
    pub wakes_coalesced: u64,
    /// Syncs run by the worker.
    pub syncs_run: u64,
    /// Files reported touched across all syncs.
    pub files_synced: u64,
    /// Coverage-artifact paths accepted (the [FR-CV-10] allow-list exception).
    pub artifacts_accepted: u64,
    /// Auto-ingests the worker attempted (success or degraded-to-warning).
    pub coverage_ingests_run: u64,
}

impl Counters {
    fn snapshot(&self) -> WatchStats {
        WatchStats {
            batches_delivered: self.batches_delivered.load(Ordering::Acquire),
            paths_accepted: self.paths_accepted.load(Ordering::Acquire),
            wakes_coalesced: self.wakes_coalesced.load(Ordering::Acquire),
            syncs_run: self.syncs_run.load(Ordering::Acquire),
            files_synced: self.files_synced.load(Ordering::Acquire),
            artifacts_accepted: self.artifacts_accepted.load(Ordering::Acquire),
            coverage_ingests_run: self.coverage_ingests_run.load(Ordering::Acquire),
        }
    }
}

/// Owns the live watcher: the OS debouncer and the sync worker thread.
///
/// Hold it for the lifetime of the serve loop; dropping it tears the watcher
/// down deterministically — the debouncer stops delivering events, the worker
/// drains any final pending wake (so edits made just before shutdown still
/// sync), and the thread is joined. No orphaned watcher survives a host
/// disconnect ([NFR-RA-12]).
///
/// [NFR-RA-12]: ../../../docs/specs/requirements/NFR-RA-12.md
pub struct WatchHandle {
    /// `Some` until [`Drop`] takes it to stop event delivery first.
    ///
    /// The cache is the pruning [`PrunedFileIdCache`] (not the crate's
    /// `RecommendedCache`): its registration-time seed walk skips ignored
    /// directories so `serve` cold start never pays the `target/`-class pre-walk
    /// ([CR-077], [NFR-PE-05]).
    debouncer: Option<
        notify_debouncer_full::Debouncer<notify::RecommendedWatcher, PrunedFileIdCache>,
    >,
    /// `Some` until [`Drop`] takes it to close the wake channel.
    wake_tx: Option<Sender<()>>,
    /// `Some` until [`Drop`] joins the sync worker.
    worker: Option<JoinHandle<()>>,
    counters: Arc<Counters>,
    /// The debounce window in effect (config `[watcher] debounce_ms`).
    debounce: Duration,
}

impl WatchHandle {
    /// A snapshot of the watcher's live counters.
    pub fn stats(&self) -> WatchStats {
        self.counters.snapshot()
    }

    /// The debounce window in effect.
    pub fn debounce(&self) -> Duration {
        self.debounce
    }
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        // 1. Stop the OS watcher first: no new events reach the callback.
        self.debouncer.take();
        // 2. Close the wake channel: the worker drains any final pending wake
        //    (crossbeam `recv` errs only once the channel is empty AND
        //    disconnected), then breaks out of its loop.
        self.wake_tx.take();
        // 3. Join the worker so the last sync (if any) completes before the
        //    engine it borrows is torn down.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Spawn the debounced watcher over `engine`'s project root.
///
/// Reads the debounce window from the project's `config.toml`
/// (`[watcher] debounce_ms`, default 300 ms — the resolved SRS OQ-04 value),
/// watches the root recursively, and submits one coalesced [`Engine::sync`]
/// per debounce window through the engine's single-writer actor.
///
/// The returned [`WatchHandle`] owns the watcher; drop it to stop watching.
///
/// # Errors
/// Returns an error if `config.toml` is present but invalid, or if the OS
/// watcher cannot be created or attached to the root. Callers on the serve
/// path should treat a failure as a degraded start, not a fatal one — the
/// watcher is non-load-bearing ([FR-SY-06]).
///
/// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
pub fn spawn(engine: Arc<Engine>) -> Result<WatchHandle> {
    // Canonicalize so the internal-path filter agrees with the OS's event
    // paths: notify reports *real* paths (e.g. `/private/var/…` on macOS),
    // and a symlinked root would otherwise fail every `strip_prefix` and
    // filter the whole project away. Same normalization the sync pipeline
    // applies to the root.
    let root = engine.root().to_path_buf();
    let root = root
        .canonicalize()
        .with_context(|| format!("resolving project root {}", root.display()))?;
    let config = crate::config::load_config_from_root(&root)?;
    let debounce = Duration::from_millis(config.watcher.debounce_ms);
    // The sync-worker cadence (CR-015): coalesce edit storms into a few syncs
    // instead of one-per-debounce-window. Pure scheduling — the graph a sync
    // produces is unchanged, only *when* it runs.
    let cadence = Cadence {
        settle: Duration::from_millis(config.watcher.settle_ms),
        min_interval: Duration::from_millis(config.watcher.min_sync_interval_ms),
        max_staleness: Duration::from_millis(config.watcher.max_staleness_ms),
    };

    // Directory names whose subtrees never feed the watcher. The feedback-loop
    // internal dirs (`.logos`, `.git`) are ALWAYS excluded — that containment
    // must not be configurable away — unioned with the indexer's configured
    // `ignored_dirs` (`target`, `node_modules`, `dist`, `build`, `vendor`, …).
    // Without the union the watcher reacts to build-output churn: a single
    // `cargo build` writes thousands of files under `target/`, each an event
    // the watcher would accept and feed to `Engine::sync` — a storm that pegs
    // every core (the sync fans out across the rayon pool) for paths the
    // indexer would never ingest anyway (FR-IX-02). The watched set MUST match
    // what indexing admits, so freshness work tracks real source, not artifacts.
    let ignored_dirs: Arc<HashSet<String>> = Arc::new(
        INTERNAL_DIRS
            .iter()
            .map(|s| (*s).to_string())
            .chain(config.semantics.ignored_dirs.iter().cloned())
            .collect(),
    );

    // The coverage-artifact matcher ([FR-CV-10], [ADR-38]): the built-in
    // conventions ∪ the optional `[coverage_ingest].artifact_glob`. A changed path
    // it matches is auto-ingested as an allow-list EXCEPTION over the
    // `target/`-class ignore filter above — but never over the `.logos/`/`.git/`
    // feedback-loop filter ([`classify`] checks those first). The configured globs
    // were containment-validated at config load, so this compile does not fail for
    // a user typo.
    let artifacts = Arc::new(
        ArtifactMatcher::compile(&config.coverage_ingest.effective())
            .context("compiling the coverage-artifact watch globs")?,
    );

    // CR-054 / FR-SY-11: the walk-level admission authority, `Arc`-cached so
    // [`classify`] can drop paths the full walk would exclude — gitignored files
    // and nested-`.git` boundaries (dev-session worktrees under `.worktrees/**`,
    // browser scratch under `.playwright-mcp/**`) — before they ever reach the
    // pending set. This is a **best-effort pre-filter**: it is built once at
    // watcher start (so a mid-session `.gitignore` edit is not reflected until the
    // watcher restarts), and `Engine::sync` rebuilds the authority per batch and
    // is the load-bearing gate that actually enforces admission ([ADR-48]). If the
    // authority cannot be built here (e.g. a bad include/exclude glob), degrade to
    // no pre-filter and lean entirely on `sync` — the watcher is never a
    // correctness dependency ([FR-SY-06], [ADR-11]).
    let authority: Arc<Option<AdmissionAuthority>> = Arc::new(
        AdmissionAuthority::from_config(&root, &config)
            .map_err(|e| {
                tracing::warn!(
                    target: "logos::watch",
                    "admission pre-filter disabled ({e}); sync remains the load-bearing gate",
                );
                e
            })
            .ok(),
    );

    let counters = Arc::new(Counters::default());
    // The coalescing pending set (see the module docs): bounded by distinct
    // dirty files, never by event count.
    let pending: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
    // A sibling pending set for coverage artifacts: the worker drains it through
    // the non-load-bearing auto-ingest path, never `Engine::sync` ([FR-CV-10]).
    let pending_artifacts: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
    // The 1-slot wake channel: a full slot means a wake is already pending —
    // `try_send` drops, nothing blocks the debouncer's delivery thread.
    let (wake_tx, wake_rx) = bounded::<()>(1);

    let worker = spawn_sync_worker(
        Arc::clone(&engine),
        Arc::clone(&pending),
        Arc::clone(&pending_artifacts),
        wake_rx,
        Arc::clone(&counters),
        cadence,
    )?;

    let callback = {
        let root = root.clone();
        let pending = Arc::clone(&pending);
        let pending_artifacts = Arc::clone(&pending_artifacts);
        let counters = Arc::clone(&counters);
        let wake_tx = wake_tx.clone();
        let ignored_dirs = Arc::clone(&ignored_dirs);
        let artifacts = Arc::clone(&artifacts);
        let authority = Arc::clone(&authority);
        move |result: DebounceEventResult| {
            let sink = DebounceSink {
                root: &root,
                ignored_dirs: &ignored_dirs,
                artifacts: &artifacts,
                authority: &authority,
                pending_sources: &pending,
                pending_artifacts: &pending_artifacts,
                wake_tx: &wake_tx,
                counters: &counters,
            };
            on_debounced(&sink, result);
        }
    };

    // CR-077 / NFR-PE-05: the pruning file-ID cache. `notify-debouncer-full`'s
    // default `RecommendedCache` seeds itself on `.watch()` by walking the ENTIRE
    // watched subtree with `walkdir` — on the repo root that is the whole physical
    // tree, `target/` included (measured ~1.2M entries), stalling the first MCP
    // request ~58–93 s. [`PrunedFileIdCache`] routes that registration-time seed
    // walk through the same `ignored_dirs` name-prune and `Arc<AdmissionAuthority>`
    // the event-time `classify` and the full `index` walk already use, so the
    // pre-walk visits only admitted source directories and cold start returns to
    // sub-second — while preserving full rename tracking for admitted paths.
    let cache =
        PrunedFileIdCache::new(root.clone(), Arc::clone(&ignored_dirs), Arc::clone(&authority));

    // `None` tick rate lets the debouncer pick a sensible poll cadence for
    // the window; events are delivered on the debouncer's own thread.
    let mut debouncer = new_debouncer_opt::<_, notify::RecommendedWatcher, PrunedFileIdCache>(
        debounce,
        None,
        callback,
        cache,
        notify::Config::default(),
    )
    .context("creating the debounced filesystem watcher")?;
    debouncer
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watching the project root {}", root.display()))?;

    tracing::info!(
        target: "logos::watch",
        root = %root.display(),
        debounce_ms = debounce.as_millis() as u64,
        "filesystem watcher started",
    );

    Ok(WatchHandle {
        debouncer: Some(debouncer),
        wake_tx: Some(wake_tx),
        worker: Some(worker),
        counters,
        debounce,
    })
}

/// The shared sinks the debouncer callback routes a classified path into. Bundled
/// so [`on_debounced`] stays a two-argument seam the unit tests can drive directly.
struct DebounceSink<'a> {
    /// The canonicalized project root.
    root: &'a Path,
    /// The feedback-loop ∪ indexer-ignored directory-name set.
    ignored_dirs: &'a HashSet<String>,
    /// The coverage-artifact allow-list matcher ([FR-CV-10]).
    artifacts: &'a ArtifactMatcher,
    /// The best-effort walk-level admission pre-filter (CR-054 / FR-SY-11);
    /// `None` when it could not be built at watcher start. `sync` is the gate.
    authority: &'a Option<AdmissionAuthority>,
    /// Coalesced source paths bound for `Engine::sync`.
    pending_sources: &'a Mutex<HashSet<PathBuf>>,
    /// Coalesced coverage-artifact paths bound for non-load-bearing auto-ingest.
    pending_artifacts: &'a Mutex<HashSet<PathBuf>>,
    /// The 1-slot wake channel into the worker.
    wake_tx: &'a Sender<()>,
    /// Live behaviour counters.
    counters: &'a Counters,
}

/// Classify a changed path ([FR-CV-10], [ADR-38]). Order is load-bearing:
///
/// 1. **Feedback-loop dirs first** — anything under `.logos/`/`.git/` (or outside
///    the root) is [`Ignored`](Admission::Ignored) with **no** exception, so a
///    stray artifact glob can never re-open the self-trigger melt ([ADR-11]).
/// 2. **Coverage-artifact allow-list** — a path the [`ArtifactMatcher`] recognizes
///    is [`Coverage`](Admission::Coverage) even under a `target/`-class ignored dir
///    (the bounded hole [ADR-38] sanctions).
/// 3. **Indexer-ignored dirs** — otherwise the normal `target/node_modules/…`
///    filter applies ([`is_ignored`]).
/// 4. **Walk-level admission** (CR-054 / FR-SY-11) — a best-effort pre-filter that
///    drops a gitignored or nested-`.git`-boundary path the name-based filters
///    miss, so a dev-session worktree (`.worktrees/**`) or browser scratch
///    (`.playwright-mcp/**`) never feeds a sync. Guarded on existence so a
///    deletion still reaches `sync`'s removal arm; `sync` is the load-bearing
///    gate ([ADR-48]).
/// 5. Everything else is an indexed [`Source`](Admission::Source) path.
///
/// [FR-CV-10]: ../../../docs/specs/requirements/FR-CV-10.md
/// [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
/// [FR-SY-11]: ../../../docs/specs/requirements/FR-SY-11.md
fn classify(
    root: &Path,
    path: &Path,
    ignored_dirs: &HashSet<String>,
    artifacts: &ArtifactMatcher,
    authority: Option<&AdmissionAuthority>,
) -> Admission {
    let Ok(relative) = path.strip_prefix(root) else {
        return Admission::Ignored;
    };
    // (1) The feedback-loop filter is absolute — checked before the allow-list.
    let under_internal = relative.components().any(|component| {
        matches!(component, Component::Normal(name) if INTERNAL_DIRS.iter().any(|d| name == *d))
    });
    if under_internal {
        return Admission::Ignored;
    }
    // (2) The coverage-artifact allow-list exception (re-admits under target/…).
    // Checked before the admission authority so a sanctioned artifact under a
    // `target/`-class dir ([ADR-38]) is still routed to the non-load-bearing
    // auto-ingest path, not dropped by the walk-level gate.
    if artifacts.matches_relative(relative) {
        return Admission::Coverage;
    }
    // (3) The ordinary indexer-ignored-dir filter (cheap, name-based).
    if is_ignored(root, path, ignored_dirs) {
        return Admission::Ignored;
    }
    // (4) The walk-level admission pre-filter (CR-054 / FR-SY-11): drop a path the
    // full walk would exclude but the name-based filters above miss — a gitignored
    // file or a nested-`.git`-boundary path (a dev-session worktree copy, a
    // vendored repo). Guarded on existence: `admits_path` cannot stat a path that
    // is gone, so an unguarded check would swallow a *deletion* event and leave the
    // removed file's nodes resident until the next reconcile. A deletion must reach
    // `Engine::sync`'s removal arm, so a non-existent path falls through to
    // `Source` and `sync` (the load-bearing gate, which re-checks admission)
    // decides — a false *admit* here costs only a wasted hash, never a stale graph.
    if let Some(authority) = authority {
        if path.exists() && !authority.admits_path(path) {
            return Admission::Ignored;
        }
    }
    // (5) An indexed source path.
    Admission::Source
}

/// The debouncer-thread half: classify, coalesce, wake. Never blocks, never
/// errs — a failed batch is logged and dropped (reconcile is the backstop).
fn on_debounced(sink: &DebounceSink, result: DebounceEventResult) {
    let events = match result {
        Ok(events) => events,
        Err(errors) => {
            // Dropped/failed OS events are tolerated by contract (FR-SY-06):
            // the next evaluation's reconcile reflects whatever was missed.
            tracing::warn!(
                target: "logos::watch",
                surface = "watcher",
                "watch error ({} event(s) dropped; reconcile will catch up): {errors:?}",
                errors.len(),
            );
            return;
        }
    };
    sink.counters.batches_delivered.fetch_add(1, Ordering::AcqRel);

    let mut sources_accepted = 0u64;
    let mut artifacts_accepted = 0u64;
    {
        // A poisoned lock means a *previous* callback panicked mid-insert; the
        // sets are plain HashSets with no invariant to corrupt, so keep serving
        // rather than killing freshness for the rest of the session.
        let mut sources = sink.pending_sources.lock().unwrap_or_else(|p| p.into_inner());
        let mut artifacts = sink
            .pending_artifacts
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for path in events.iter().flat_map(|event| event.paths.iter()) {
            match classify(sink.root, path, sink.ignored_dirs, sink.artifacts, sink.authority.as_ref()) {
                Admission::Source => {
                    if sources.insert(path.clone()) {
                        sources_accepted += 1;
                    }
                }
                Admission::Coverage => {
                    if artifacts.insert(path.clone()) {
                        artifacts_accepted += 1;
                    }
                }
                Admission::Ignored => {}
            }
        }
    }
    if sources_accepted == 0 && artifacts_accepted == 0 {
        return;
    }
    sink.counters
        .paths_accepted
        .fetch_add(sources_accepted, Ordering::AcqRel);
    sink.counters
        .artifacts_accepted
        .fetch_add(artifacts_accepted, Ordering::AcqRel);

    match sink.wake_tx.try_send(()) {
        Ok(()) => {}
        // Slot already holds a wake: this batch coalesces into it — the pending
        // sets already carry our paths (drop-and-coalesce, AQ-01).
        Err(TrySendError::Full(())) => {
            sink.counters.wakes_coalesced.fetch_add(1, Ordering::AcqRel);
        }
        // Worker gone (shutdown race): nothing to wake; the handle is being
        // dropped and the debouncer will stop momentarily.
        Err(TrySendError::Disconnected(())) => {}
    }
}

/// The sync-worker scheduling cadence (CR-015): settle window, rate-limit floor,
/// and staleness cap, resolved from `[watcher]` config. Decouples sync frequency
/// from edit frequency so a machine-speed edit storm coalesces into a few syncs
/// instead of one-per-debounce-window. Freshness work nobody is querying mid-
/// storm is deferred until edits settle (or the staleness cap), never lost — the
/// reconcile backstop ([FR-SY-06]) and best-effort-fresh navigation ([ADR-11])
/// both tolerate the extra latency.
///
/// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
#[derive(Debug, Clone, Copy)]
struct Cadence {
    /// Quiet window before syncing: each new debounced batch resets it, so a
    /// burst collapses into one sync once editing pauses.
    settle: Duration,
    /// Rate-limit floor: never start two syncs closer together than this.
    min_interval: Duration,
    /// Staleness cap: sync once the oldest pending change is this old even if
    /// edits never pause, so a continuous stream still refreshes.
    max_staleness: Duration,
}

/// Spawn the sync worker: one thread that coalesces edit activity into syncs on
/// the [`Cadence`] schedule and runs at most one [`Engine::sync`] at a time (the
/// AR-01 starvation guard).
///
/// The worker parks on the wake channel when idle (~0 % CPU). On a wake it defers
/// the sync until edits **settle** (a quiet [`Cadence::settle`] window), bounded
/// by [`Cadence::max_staleness`] so a never-quiet storm still syncs, then honours
/// the [`Cadence::min_interval`] rate-limit floor — turning a per-edit sync
/// cascade into one coalesced sync per burst (CR-015). Freshness latency rises by
/// at most the settle/staleness windows; correctness is unaffected (the watcher
/// is non-load-bearing, [FR-SY-06], and navigation is best-effort-fresh,
/// [ADR-11]).
///
/// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
fn spawn_sync_worker(
    engine: Arc<Engine>,
    pending: Arc<Mutex<HashSet<PathBuf>>>,
    pending_artifacts: Arc<Mutex<HashSet<PathBuf>>>,
    wake_rx: Receiver<()>,
    counters: Arc<Counters>,
    cadence: Cadence,
) -> Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("logos-watch-sync".to_owned())
        .spawn(move || {
            let mut last_sync: Option<Instant> = None;
            loop {
                // Park until the first wake of a new batch — idle is truly parked.
                // An `Err` means the channel is empty AND disconnected: the handle
                // is being dropped, so flush any final pending set and exit (edits
                // made just before shutdown still sync).
                let mut shutting_down = wake_rx.recv().is_err();

                // Coalesce: defer until edits settle (a quiet `settle` window),
                // capped at `max_staleness` from the first pending change so a
                // never-quiet storm still syncs. Each new wake resets the settle.
                if !shutting_down {
                    let first_pending = Instant::now();
                    loop {
                        let waited = first_pending.elapsed();
                        if waited >= cadence.max_staleness {
                            break; // staleness cap reached — sync now
                        }
                        let budget = cadence.settle.min(cadence.max_staleness - waited);
                        match wake_rx.recv_timeout(budget) {
                            Ok(()) => continue, // a new batch arrived → keep settling
                            Err(RecvTimeoutError::Timeout) => break, // quiet → sync now
                            Err(RecvTimeoutError::Disconnected) => {
                                shutting_down = true; // shutdown mid-settle: flush & exit
                                break;
                            }
                        }
                    }
                }

                // Rate-limit floor: never start two syncs closer than `min_interval`
                // (skipped while shutting down so the final flush stays prompt).
                if !shutting_down {
                    if let Some(prev) = last_sync {
                        let since = prev.elapsed();
                        if since < cadence.min_interval {
                            if let Err(RecvTimeoutError::Disconnected) =
                                wake_rx.recv_timeout(cadence.min_interval - since)
                            {
                                shutting_down = true;
                            }
                        }
                    }
                }

                // Drain the coalesced coverage artifacts and auto-ingest each
                // through the non-load-bearing path ([FR-CV-10], [ADR-38]): a local
                // read + parse + store, NEVER a test run, and a failure (a
                // half-written report mid-CI-run, a malformed artifact) degrades to
                // a warning so it can never block the source sync below. Drained in
                // the same worker turn (including the shutdown flush), so an
                // artifact written just before shutdown still ingests.
                let artifacts: Vec<PathBuf> = {
                    let mut pending = pending_artifacts.lock().unwrap_or_else(|p| p.into_inner());
                    pending.drain().collect()
                };
                for artifact in artifacts {
                    counters.coverage_ingests_run.fetch_add(1, Ordering::AcqRel);
                    match engine.coverage_ingest_auto(&artifact) {
                        Ok(summary) => tracing::info!(
                            target: crate::observability::TELEMETRY_TARGET,
                            tool = "watch_coverage_ingest",
                            surface = "watcher",
                            artifact = %artifact.display(),
                            matched_files = summary.matched_files,
                            "watcher auto-ingested a coverage artifact",
                        ),
                        Err(e) => tracing::warn!(
                            target: "logos::watch",
                            surface = "watcher",
                            artifact = %artifact.display(),
                            "auto coverage ingest failed (degraded to a warning; sync unaffected): {e:#}",
                        ),
                    }
                }

                // Drain the coalesced set and run at most one sync (AR-01).
                let paths: Vec<PathBuf> = {
                    let mut pending = pending.lock().unwrap_or_else(|p| p.into_inner());
                    pending.drain().collect()
                };
                if !paths.is_empty() {
                    let started = Instant::now();
                    // Infallible surface (ADR-14): a failed sync degrades to a
                    // result carrying warnings; freshness self-heals on the next
                    // window or the reconcile backstop.
                    let result = engine.sync(&paths);
                    let duration_ms = started.elapsed().as_millis() as u64;
                    let files = result.files_added + result.files_modified + result.files_removed;
                    counters.syncs_run.fetch_add(1, Ordering::AcqRel);
                    counters.files_synced.fetch_add(files, Ordering::AcqRel);
                    last_sync = Some(Instant::now());
                    // The telemetry event for this window (filesystem-watcher
                    // integration: surface=watcher, files synced per window).
                    // Distinct tool name: the inner Engine::sync already emitted
                    // its own `tool=sync` event under the process surface —
                    // `watch_sync` attributes the *trigger* without double-
                    // counting the operation.
                    tracing::info!(
                        target: crate::observability::TELEMETRY_TARGET,
                        tool = "watch_sync",
                        surface = "watcher",
                        duration_ms,
                        ok = result.warnings.is_empty(),
                        files,
                        "watcher sync",
                    );
                }

                if shutting_down {
                    break;
                }
            }
        })
        .context("spawning the logos-watch-sync worker thread")
}

/// `true` if `path` lives under any ignored directory relative to `root` —
/// those subtrees never feed the watcher. `ignored_dirs` is the union the
/// caller builds in [`spawn`]: the feedback-loop internal dirs (`.logos`,
/// `.git`) plus the indexer's configured `ignored_dirs` (`target`,
/// `node_modules`, …), so the watched set matches what indexing admits and
/// build-output churn never reaches `Engine::sync` (see the module docs).
///
/// A path outside `root` entirely is also ignored-by-default: the watcher
/// only watches the root, so such a path is an OS-event oddity (rename
/// endpoint, mount quirk) the sync pipeline would reject anyway.
fn is_ignored(root: &Path, path: &Path, ignored_dirs: &HashSet<String>) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative.components().any(|component| match component {
        Component::Normal(name) => is_pruned_dir_name(name, ignored_dirs),
        _ => false,
    })
}

/// `true` if a directory whose last component is `name` must never be descended
/// into during the registration-time seed walk — the cheap, name-based prune
/// that captures the whole `target/`-class cost ([CR-077]). `ignored_dirs` is the
/// same union [`spawn`] builds: the feedback-loop internal dirs (`.logos`,
/// `.git`) ∪ the indexer's configured `ignored_dirs` (`target`, `node_modules`,
/// `dist`, `build`, `vendor`, …).
fn is_pruned_dir_name(name: &OsStr, ignored_dirs: &HashSet<String>) -> bool {
    name.to_str().is_some_and(|name| ignored_dirs.contains(name))
}

/// A [`notify_debouncer_full::FileIdCache`] whose registration-time seed walk is
/// pruned through the watcher's admission machinery ([CR-077], [NFR-PE-05]).
///
/// # Why it exists
/// `notify-debouncer-full` keeps a path→[`FileId`] map so it can stitch rename
/// `from`/`to` pairs back together when the OS back-end drops rename cookies. The
/// stock `RecommendedCache` seeds that map on `.watch()` by walking the **entire**
/// watched subtree ([`file_id::get_file_id`] per entry). Registered on the repo
/// root, that walk traverses the whole physical tree — `target/` and all — before
/// `.watch()` returns, so `serve_stdio` cannot answer the MCP `initialize`
/// handshake until it finishes (measured ~1.2M entries, ~58–93 s: an
/// [NFR-PE-05] violation by ~116×).
///
/// # What it prunes
/// The seed walk here (`add_path` in [`Recursive`](RecursiveMode::Recursive) mode)
/// applies the same two gates the event-time [`classify`] and the full `index`
/// walk apply, so the registration-time file-ID set matches admission
/// ([FR-SY-11], [ADR-48]):
///
/// 1. **Directory-name prune (always).** A directory whose name is in
///    `ignored_dirs` is never descended into — this captures the entire
///    `target/`-class cost cheaply, with no `stat`.
/// 2. **Leaf admission (best-effort).** When an [`AdmissionAuthority`] is
///    available, a leaf *file* the authority would reject (a gitignored path, a
///    nested-`.git`-boundary path) is not cached — matching full-walk parity for
///    the leaves the name prune alone would miss. With no authority (construction
///    failed) the walk degrades to the name prune only, mirroring how [`classify`]
///    degrades — the watcher is never a correctness dependency ([FR-SY-06],
///    [ADR-11]).
///
/// Descent is pruned **by name only** (per [CR-077] §3.2): a directory the
/// authority would reject wholesale but whose name is *not* in `ignored_dirs` —
/// e.g. a gitignored `datasets/` or an in-tree nested-`.git` boundary under an
/// ordinary name — is still descended, and its leaf *files* are rejected one by
/// one. The seeded **file** set therefore matches the full walk, but the
/// registration walk may still `stat` such a subtree's directory skeleton. The
/// headline `target/`-class cost is name-pruned, so this residual is bounded;
/// pruning gitignored-directory descent wholesale (a directory-level authority
/// predicate) is a tracked follow-up, not this CR's scope.
///
/// Symlinks are not followed (the seed walk classifies via `read_dir`'s
/// non-following `file_type`), matching the full walk's `follow_links(false)` and
/// keeping the walk cycle-safe.
///
/// # Rename tracking is preserved for admitted paths
/// [`cached_file_id`](FileIdCache::cached_file_id) and
/// [`remove_path`](FileIdCache::remove_path) behave exactly as the stock
/// `FileIdMap` — and every admitted source path is seeded with its real
/// [`FileId`] — so rename stitching is unchanged for the only paths a rename event
/// is ever acted upon ([CR-077] CRA-02). A rename into/out of a pruned directory
/// is already dropped by event-time `classify` and healed by reconcile
/// ([FR-SY-06]).
///
/// [CR-077]: ../../../docs/requests/CR-077-watcher-registration-prewalk-prune.md
/// [NFR-PE-05]: ../../../docs/specs/requirements/NFR-PE-05.md
/// [FR-SY-06]: ../../../docs/specs/requirements/FR-SY-06.md
/// [FR-SY-11]: ../../../docs/specs/requirements/FR-SY-11.md
/// [ADR-11]: ../../../docs/specs/architecture/decisions/ADR-11.md
/// [ADR-48]: ../../../docs/specs/architecture/decisions/ADR-48.md
#[derive(Debug)]
pub struct PrunedFileIdCache {
    /// path → file ID, the rename-stitching map (same role as `FileIdMap`).
    paths: HashMap<PathBuf, FileId>,
    /// The canonicalised watched root. The name prune below is skipped for the
    /// root itself so a project whose own directory name happens to be an ignored
    /// name (a repo checked out into `…/build`, `…/target`, `…/out`, …) is still
    /// seeded — only *descendant* and event-time-created directories are pruned by
    /// name.
    root: PathBuf,
    /// The feedback-loop ∪ indexer-ignored directory-name set: directories whose
    /// name is in here are never descended into during the seed walk.
    ignored_dirs: Arc<HashSet<String>>,
    /// The best-effort walk-level admission pre-filter (CR-054 / FR-SY-11),
    /// shared with the event-time [`classify`] path; `None` when it could not be
    /// built at watcher start, in which case the seed walk degrades to the
    /// name-prune only.
    authority: Arc<Option<AdmissionAuthority>>,
}

impl PrunedFileIdCache {
    /// Construct the cache over the canonicalised watched `root`, the shared
    /// `ignored_dirs` name set, and the shared best-effort admission `authority`
    /// (the latter two already `Arc`-built in [`spawn`]).
    fn new(
        root: PathBuf,
        ignored_dirs: Arc<HashSet<String>>,
        authority: Arc<Option<AdmissionAuthority>>,
    ) -> Self {
        Self {
            paths: HashMap::new(),
            root,
            ignored_dirs,
            authority,
        }
    }

    /// Whether a leaf *file* should be cached: admitted by the authority when one
    /// is available, else unconditionally (the degradation path).
    fn leaf_admitted(&self, path: &Path) -> bool {
        match self.authority.as_ref() {
            Some(authority) => authority.admits_path(path),
            None => true,
        }
    }

    /// Seed `path`'s file ID and (in [`Recursive`](RecursiveMode::Recursive) mode)
    /// those of its descendants, pruning ignored-directory descent by name and
    /// dropping authority-rejected leaves. `max_depth` mirrors `walkdir`'s
    /// semantics: depth 0 is `path` itself, depth 1 its immediate children.
    fn seed(&mut self, path: &Path, max_depth: usize) {
        let Ok(meta) = fs::symlink_metadata(path) else {
            return; // gone/unreadable — nothing to seed (parity with the walk).
        };
        let file_type = meta.file_type();
        if file_type.is_dir() {
            // A *non-root* ignored directory (by name) is never seeded or
            // descended — this is the event-time guard against a Create of a
            // `target/` dir. The watched root itself is always descended, even if
            // its own name collides with an ignored name (a repo checked out into
            // `…/build`), so registration seeds the whole project.
            if path != self.root
                && is_pruned_dir_name(path.file_name().unwrap_or_default(), &self.ignored_dirs)
            {
                return;
            }
            self.insert_id(path);
            if max_depth >= 1 {
                self.seed_children(path, max_depth);
            }
        } else if file_type.is_file() {
            // A file start (e.g. a Create event for a single file): apply the leaf
            // admission gate, then cache it.
            if self.leaf_admitted(path) {
                self.insert_id(path);
            }
        }
        // Symlinks / other non-regular entries are skipped (parity, cycle-safe).
    }

    /// Walk `root`'s subtree up to `max_depth`, seeding admitted entries and
    /// pruning ignored-directory descent by name. Iterative (an explicit stack)
    /// so a deep tree cannot overflow the call stack.
    fn seed_children(&mut self, root: &Path, max_depth: usize) {
        let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 1)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue; // unreadable dir — best-effort, skip it.
            };
            for entry in entries.flatten() {
                // `read_dir`'s `file_type` does not follow symlinks, so a
                // symlinked directory is classified as a symlink (neither dir nor
                // file below) and skipped — matching the full walk.
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let child = entry.path();
                if file_type.is_dir() {
                    if is_pruned_dir_name(&entry.file_name(), &self.ignored_dirs) {
                        continue; // never descend into an ignored dir (the cost).
                    }
                    self.insert_id(&child);
                    if depth < max_depth {
                        stack.push((child, depth + 1));
                    }
                } else if file_type.is_file() && self.leaf_admitted(&child) {
                    self.insert_id(&child);
                }
            }
        }
    }

    /// Read `path`'s real [`FileId`] and record it. A path whose ID cannot be read
    /// is skipped (best-effort — a missing ID only degrades rename stitching for
    /// that one path, never correctness).
    fn insert_id(&mut self, path: &Path) {
        if let Ok(file_id) = get_file_id(path) {
            self.paths.insert(path.to_path_buf(), file_id);
        }
    }

    /// The number of seeded file-ID entries — the deterministic, wall-clock- and
    /// machine-independent observable the CR-077 regression test asserts against.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.paths.len()
    }

    /// The set of seeded paths (test-only), for asserting exactly which entries
    /// the pruned seed walk visited.
    #[cfg(test)]
    fn cached_paths(&self) -> HashSet<PathBuf> {
        self.paths.keys().cloned().collect()
    }
}

impl FileIdCache for PrunedFileIdCache {
    fn cached_file_id(&self, path: &Path) -> Option<&FileId> {
        self.paths.get(path)
    }

    fn add_path(&mut self, path: &Path, recursive_mode: RecursiveMode) {
        // Mirror `walkdir`'s depth semantics: recursive = the whole subtree,
        // non-recursive = the path plus its immediate children.
        let max_depth = if recursive_mode == RecursiveMode::Recursive {
            usize::MAX
        } else {
            1
        };
        self.seed(path, max_depth);
    }

    fn remove_path(&mut self, path: &Path) {
        // Same subtree removal as the stock `FileIdMap`.
        self.paths.retain(|p, _| !p.starts_with(path));
    }
}
