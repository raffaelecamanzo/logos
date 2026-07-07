//! Black-box integration tests for the debounced filesystem watcher (S-022,
//! [FR-SY-04](../../docs/specs/requirements/FR-SY-04.md),
//! [filesystem-watcher](../../docs/specs/architecture/integrations/filesystem-watcher.md))
//! against real OS notifications (FSEvents/inotify) on a temp project:
//!
//! - an edit with the watcher running lands in navigation without a manual
//!   `sync` ([FR-SY-04](../../docs/specs/requirements/FR-SY-04.md));
//! - an edit *storm* debounces into **one** batched sync, and the sync's own
//!   `.logos` writes never re-trigger the watcher (feedback containment,
//!   [AQ-01](../../docs/specs/architecture.md#14-open-questions) drop-and-coalesce);
//! - a deletion syncs as a removal;
//! - dropping the handle stops the watcher with no orphaned worker, and an
//!   out-of-band edit made with NO watcher running is reflected by the next
//!   explicit sync — the reconcile backstop
//!   ([FR-SY-06](../../docs/specs/requirements/FR-SY-06.md),
//!   [UAT-SY-03](../../docs/specs/requirements/UAT-SY-03.md));
//! - a single-file sync stays within the ≤250 ms budget
//!   ([NFR-PE-03](../../docs/specs/requirements/NFR-PE-03.md)).
//!
//! Timing posture: OS event *delivery* latency is outside our contract, so
//! tests poll with generous deadlines rather than sleeping fixed amounts;
//! the assertions are on the watcher's *counters* (deterministic) plus the
//! navigation read-model, never on raw wall-clock except the NFR-PE-03
//! budget, which mirrors the NFR-PE-05 cold-start test's posture.
//!
//! Gated on `lang-rust`: the fixtures are Rust sources.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use logos_core::watch::WatchHandle;
use logos_core::Engine;

/// The debounce window used by these fixtures (config `[watcher]`): short
/// enough to keep tests fast, long enough that a burst of writes lands in
/// one window.
const DEBOUNCE_MS: u64 = 250;

/// Generous deadline for OS event delivery + debounce + sync to complete.
const DEADLINE: Duration = Duration::from_secs(15);

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, contents).expect("write fixture file");
}

/// A started, indexed engine over a temp project with a fast watcher window.
///
/// The returned root is canonicalized (macOS tempdirs live behind the
/// `/var → /private/var` symlink) so explicit `sync` calls and the watcher
/// agree on real paths, exactly as a CLI invocation from inside the project
/// would.
fn indexed_project() -> (TempDir, PathBuf, Arc<Engine>) {
    indexed_project_with_debounce(DEBOUNCE_MS)
}

/// [`indexed_project`] with an explicit debounce window (the storm test needs
/// one wide enough to absorb the OS's own delivery jitter).
fn indexed_project_with_debounce(debounce_ms: u64) -> (TempDir, PathBuf, Arc<Engine>) {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().canonicalize().expect("canonical temp root");
    // Policy lives at <root>/.logos/config.toml (FR-CF-01).
    write(
        &root,
        ".logos/config.toml",
        &format!("[watcher]\ndebounce_ms = {debounce_ms}\n"),
    );
    write(&root, "src/lib.rs", "pub fn seed_alpha() -> u32 { 1 }\n");
    let engine = Arc::new(Engine::start(&root).expect("engine starts"));
    let result = engine.index();
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert_eq!(result.files_indexed, 1);
    (tmp, root, engine)
}

/// Poll `probe` until it returns true or the deadline expires.
fn wait_for(what: &str, probe: impl Fn() -> bool) {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        if probe() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out after {DEADLINE:?} waiting for: {what}");
}

/// `true` once a function named `name` is searchable.
fn searchable(engine: &Engine, name: &str) -> bool {
    engine
        .search(name, None, Some(10))
        .hits
        .iter()
        .any(|h| h.name == name)
}

/// FR-SY-04 acceptance: editing a file with the watcher running updates
/// navigation within the debounce window — no manual `sync`.
#[test]
fn watcher_syncs_an_edit_into_navigation() {
    let (_tmp, root, engine) = indexed_project();
    let handle = engine.watch().expect("watcher starts");
    assert_eq!(handle.debounce(), Duration::from_millis(DEBOUNCE_MS));

    write(
        &root,
        "src/fresh.rs",
        "pub fn fresh_function_from_watcher() -> u32 { 42 }\n",
    );

    wait_for("the watcher to sync the new file into navigation", || {
        searchable(&engine, "fresh_function_from_watcher")
    });
    // Counters are incremented by the worker *after* the sync the probe just
    // observed — poll them rather than racing them.
    wait_for("the watcher counters to record the sync", || {
        let stats = handle.stats();
        stats.syncs_run >= 1 && stats.files_synced >= 1
    });
}

/// FR-SY-04 + AQ-01 acceptance: a rapid edit storm (many files in quick
/// succession) debounces into ONE batched sync — and the sync's own
/// `.logos/logos.db` writes never re-trigger the watcher (the feedback-loop
/// containment the module promises).
#[test]
fn edit_storm_debounces_into_one_batched_sync() {
    // A window wide enough that every FSEvents/inotify delivery callback for
    // the burst lands inside it — the assertion is about *our* coalescing,
    // not the OS's delivery sharding.
    const STORM_DEBOUNCE_MS: u64 = 1_000;
    let (_tmp, root, engine) = indexed_project_with_debounce(STORM_DEBOUNCE_MS);
    let handle = engine.watch().expect("watcher starts");

    const STORM: usize = 20;
    for i in 0..STORM {
        write(
            &root,
            &format!("src/storm_{i}.rs"),
            &format!("pub fn storm_fn_{i}() -> usize {{ {i} }}\n"),
        );
    }

    wait_for("the storm to be fully synced", || {
        handle.stats().files_synced >= STORM as u64
    });
    let stats = handle.stats();
    // The steady-state outcome is ONE batched sync (the deterministic
    // single-wake coalescing proof lives in the module's unit tests, which
    // drive the callback directly). End-to-end, the OS may shard delivery of
    // one burst across a couple of callbacks under load — what must NEVER
    // happen is the per-file cascade the debouncer+coalescer exist to
    // prevent: 20 edits ⇒ at most a couple of batches, not 20 syncs.
    assert!(
        stats.syncs_run <= 3,
        "an edit storm must coalesce into very few batched syncs: {stats:?}"
    );
    assert_eq!(stats.files_synced, STORM as u64, "{stats:?}");

    // Feedback containment: the sync above wrote logos.db (and telemetry)
    // under .logos/ while the watcher was live. A feedback loop would keep
    // producing sync-after-sync indefinitely — require total quiescence over
    // a multi-window pause.
    std::thread::sleep(Duration::from_millis(STORM_DEBOUNCE_MS * 5 / 2));
    let after = handle.stats();
    assert_eq!(
        after.syncs_run, stats.syncs_run,
        "the sync's own .logos writes re-triggered the watcher: {after:?}"
    );
}

/// A deletion observed by the watcher syncs as a removal: the symbol leaves
/// navigation without a manual `sync`.
#[test]
fn watcher_syncs_a_deletion_as_removal() {
    let (_tmp, root, engine) = indexed_project();
    write(
        &root,
        "src/doomed.rs",
        "pub fn doomed_function() -> u32 { 0 }\n",
    );
    let sync = engine.sync(&[root.join("src/doomed.rs")]);
    assert_eq!(sync.files_added, 1, "{sync:?}");
    assert!(searchable(&engine, "doomed_function"));

    let handle = engine.watch().expect("watcher starts");
    fs::remove_file(root.join("src/doomed.rs")).expect("delete fixture");

    wait_for("the watcher to sync the deletion", || {
        !searchable(&engine, "doomed_function")
    });
    wait_for("the watcher counters to record the removal sync", || {
        handle.stats().syncs_run >= 1
    });
}

/// FR-SY-06 / UAT-SY-03: dropping the handle stops the watcher cleanly (no
/// orphaned worker), an out-of-band edit made with NO watcher running is NOT
/// picked up spontaneously — and the next explicit sync (the reconcile
/// backstop's ingredient) reflects it. Correctness never depended on the
/// watcher.
#[test]
fn out_of_band_edit_without_watcher_is_reflected_by_the_next_sync() {
    let (_tmp, root, engine) = indexed_project();

    // Start and immediately stop a watcher: the drop joins the worker.
    let handle = engine.watch().expect("watcher starts");
    drop(handle);

    // Out-of-band edit with no watcher running.
    write(
        &root,
        "src/out_of_band.rs",
        "pub fn out_of_band_function() -> u32 { 7 }\n",
    );

    // No spontaneous pickup: give it two would-be windows.
    std::thread::sleep(Duration::from_millis(DEBOUNCE_MS * 3));
    assert!(
        !searchable(&engine, "out_of_band_function"),
        "no watcher is running — nothing should have synced"
    );

    // The backstop: the next explicit sync reflects the edit (FR-SY-06's
    // guarantee that evaluation correctness never depends on a watcher; the
    // scan-level reconcile that calls this lands with S-020).
    let sync = engine.sync(&[root.join("src/out_of_band.rs")]);
    assert_eq!(sync.files_added, 1, "{sync:?}");
    assert!(searchable(&engine, "out_of_band_function"));
}

/// NFR-PE-03: re-syncing a single changed file completes in ≤ 250 ms. Same
/// wall-clock posture as the NFR-PE-05 cold-start test (the dev profile pins
/// tree-sitter opt-levels, keeping the budget honest in both profiles).
#[test]
fn single_file_sync_meets_the_pe03_budget() {
    let (_tmp, root, engine) = indexed_project();
    write(
        &root,
        "src/lib.rs",
        "pub fn seed_alpha() -> u32 { 2 }\npub fn seed_beta() -> u32 { 3 }\n",
    );

    let start = Instant::now();
    let sync = engine.sync(&[root.join("src/lib.rs")]);
    let elapsed = start.elapsed();

    assert_eq!(sync.files_modified, 1, "{sync:?}");
    assert!(
        elapsed <= Duration::from_millis(250),
        "single-file sync took {elapsed:?}, over the NFR-PE-03 ≤250ms budget"
    );
}

/// AR-01 starvation guard: under a *continuous* write-event stream, the
/// watcher never holds more than one in-flight sync, and concurrent
/// navigation reads on the RO pool keep answering throughout (reads are
/// never blocked by the writer, ADR-02/NFR-PE-01).
#[test]
fn navigation_reads_survive_a_continuous_edit_stream() {
    let (_tmp, root, engine) = indexed_project();
    let handle: WatchHandle = engine.watch().expect("watcher starts");

    // Feed a continuous stream for ~6 windows while hammering navigation.
    let stream_until = Instant::now() + Duration::from_millis(DEBOUNCE_MS * 6);
    let mut i = 0usize;
    let mut slowest_read = Duration::ZERO;
    while Instant::now() < stream_until {
        write(
            &root,
            &format!("src/stream_{}.rs", i % 5),
            &format!("pub fn stream_fn_{i}() -> usize {{ {i} }}\n"),
        );
        i += 1;
        let t = Instant::now();
        let result = engine.search("seed_alpha", None, Some(5));
        slowest_read = slowest_read.max(t.elapsed());
        assert!(
            result.hits.iter().any(|h| h.name == "seed_alpha"),
            "navigation starved mid-stream: {result:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    // The stream produced far more events than syncs: drop-and-coalesce kept
    // the writer queue at ≤1 watcher batch at a time.
    wait_for("the tail of the stream to drain", || {
        handle.stats().syncs_run >= 1
    });
    let stats = handle.stats();
    assert!(
        stats.syncs_run <= stats.batches_delivered.max(1),
        "more syncs than delivered batches can never happen: {stats:?}"
    );
    // Reads stayed live (NFR-PE-01 posture; the rigorous p95 lands in S-024).
    assert!(
        slowest_read < Duration::from_secs(2),
        "a navigation read stalled for {slowest_read:?} during the stream"
    );
}

/// CR-015 cadence: the worker DEFERS during active editing and coalesces a burst
/// into one sync only after a quiet `settle` window — it does not sync per edit.
/// With a generous settle window the burst is observably un-synced shortly after
/// it lands, then becomes one batched sync once edits go quiet.
#[test]
fn edits_defer_until_the_settle_window_then_coalesce() {
    // Fast debounce (prompt batch delivery), a large settle window (the deferral
    // we observe), no rate-limit floor, and a staleness cap well beyond the test
    // so nothing forces an early sync. Drives the cadence knobs end-to-end
    // through `[watcher]` config.
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().canonicalize().expect("canonical temp root");
    write(
        &root,
        ".logos/config.toml",
        "[watcher]\ndebounce_ms = 100\nsettle_ms = 2000\nmin_sync_interval_ms = 0\nmax_staleness_ms = 60000\n",
    );
    write(&root, "src/lib.rs", "pub fn seed_alpha() -> u32 { 1 }\n");
    let engine = Arc::new(Engine::start(&root).expect("engine starts"));
    assert_eq!(engine.index().files_indexed, 1);
    let handle = engine.watch().expect("watcher starts");

    const BURST: usize = 8;
    for i in 0..BURST {
        write(
            &root,
            &format!("src/burst_{i}.rs"),
            &format!("pub fn burst_fn_{i}() -> usize {{ {i} }}\n"),
        );
    }

    // Well inside the 2 s settle window, the burst must NOT have synced — the
    // worker is deferring, not syncing per edit (the whole point of CR-015).
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        handle.stats().syncs_run,
        0,
        "edits synced inside the settle window instead of deferring: {:?}",
        handle.stats()
    );

    // Once edits go quiet, the settle elapses and the whole burst lands as one
    // coalesced sync.
    wait_for("the deferred burst to settle into one sync", || {
        handle.stats().files_synced >= BURST as u64
    });
    let stats = handle.stats();
    assert!(
        stats.syncs_run <= 2,
        "the burst must coalesce into ~one sync, got {stats:?}"
    );
    assert_eq!(stats.files_synced, BURST as u64, "{stats:?}");
}
