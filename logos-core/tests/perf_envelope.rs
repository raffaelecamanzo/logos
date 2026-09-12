//! The performance envelope as measured, release-gating fitness functions
//! (S-024, [performance NFRs], [navigation-service], [execution-runtime]).
//!
//! Each budget Logos promises is asserted here against a synthetic ~100k-LOC
//! Rust fixture — the small/medium [`ENVELOPE_LOC`] target the budgets are tuned
//! for ([NFR-PE-02]):
//!
//! (The [`configuration_envelope`] cases at the foot of this file are the one
//! exception: they measure the configuration-corpus and property-binding phases
//! against a *different* fixture — a Spring-shaped member — and are described in
//! their own module docs.)
//!
//! - **cold index ≤ 30 s** ([NFR-PE-02]);
//! - **single-file sync ≤ 250 ms** on the big graph ([NFR-PE-03]);
//! - **warm aggregate scan ≤ 2 s** when only a few files are dirty ([NFR-PE-04]);
//! - **point-query p95 < 100 ms / p99 < 250 ms under concurrent navigation load,
//!   telemetry on** ([NFR-PE-01], [UAT-NV-07], retiring the dual-scheduler risk
//!   [AR-02] — reads never block under the single writer-actor runtime);
//! - **peak RSS ≤ 1 GB** ([NFR-PE-06]);
//! - **hydration cache hit on a repeat run** ([NFR-PE-07], [AA-04]);
//! - **graceful degradation + advisory beyond the envelope** ([NFR-PE-09]) — the
//!   system stays correct and emits a one-line advisory, never a crash.
//!
//! These also exercise the concurrency assumptions [AA-01]/[AA-02] (the runtime
//! serves concurrent reads off the RO pool while a writer commits) on a
//! realistic graph.
//!
//! ## Running them
//!
//! Every budget test **over the ~100k-LOC fixture** is `#[ignore]` — building and
//! indexing that much source is far too heavy for the default `cargo test` run,
//! and (per the sprint risk register) timing assertions flake under host
//! oversubscription. Run them deliberately, **throttled**, and re-run a breach in
//! isolation before treating it as a regression:
//!
//! ```text
//! cargo test -p logos-core --features lang-rust --test perf_envelope \
//!     -- --ignored --test-threads=1
//! ```
//!
//! The [`configuration_envelope`] cases are **not** `#[ignore]`d: their fixture is
//! ~250 small files, the whole module runs in a few seconds, and a guard built
//! before the work it bounds (S-391) is worth nothing if the default run skips
//! it. The two of them that assert a wall clock carry `pe03_budget` /
//! `pe05_budget` in their names, which is how `scripts/gate.sh` skips exactly the
//! timing-sensitive guards at its `fast` tier and runs them at `full` — the same
//! treatment `cold_start_to_ready_engine_is_within_pe05_budget` and
//! `single_file_sync_meets_the_pe03_budget` already get.
//!
//! Two env knobs keep the budgets honest without editing them:
//! - `LOGOS_PERF_TOLERANCE` (f64 ≥ 1.0, default 1.0) widens every wall-clock band
//!   for a slow/loaded CI host — the tolerance-banding the sprint mandates;
//! - `LOGOS_PERF_LOC` (u64, default [`ENVELOPE_LOC`]) shrinks the fixture for a
//!   fast harness smoke-test; the *budgets are unchanged*, so a reduced run only
//!   proves the harness wiring, not the full-scale gate.
//!
//! Gated on `lang-rust`: the fixtures here are Rust sources. The
//! [`configuration_envelope`] module additionally needs `lang-yaml` (its
//! configuration sources) and `lang-java` (its `@ConfigurationProperties`
//! classes); all three are in the default feature set.
//!
//! [performance NFRs]: ../../docs/specs/requirements/NFR-PE-01.md
//! [navigation-service]: ../../docs/specs/architecture/components/navigation-service.md
//! [execution-runtime]: ../../docs/specs/architecture/components/execution-runtime.md
//! [NFR-PE-01]: ../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-PE-02]: ../../docs/specs/requirements/NFR-PE-02.md
//! [NFR-PE-03]: ../../docs/specs/requirements/NFR-PE-03.md
//! [NFR-PE-04]: ../../docs/specs/requirements/NFR-PE-04.md
//! [NFR-PE-06]: ../../docs/specs/requirements/NFR-PE-06.md
//! [NFR-PE-07]: ../../docs/specs/requirements/NFR-PE-07.md
//! [NFR-PE-09]: ../../docs/specs/requirements/NFR-PE-09.md
//! [UAT-NV-07]: ../../docs/specs/requirements/UAT-NV-07.md
//! [AA-01]: ../../docs/specs/architecture.md#24-assumptions
//! [AA-02]: ../../docs/specs/architecture.md#24-assumptions
//! [AA-04]: ../../docs/specs/architecture.md#24-assumptions
//! [AR-02]: ../../docs/specs/architecture.md#13-risk-register
#![cfg(feature = "lang-rust")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use logos_core::observability::{self, ProcessSurface};
use logos_core::{Engine, Granularity};

// ── Tolerance & sizing knobs ────────────────────────────────────────────────

/// Multiplier applied to every wall-clock budget so a loaded CI host can widen
/// the bands without editing the budget. Defaults to `1.0`; values below `1.0`
/// are ignored (a budget is never tightened by accident).
fn tolerance() -> f64 {
    std::env::var("LOGOS_PERF_TOLERANCE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v >= 1.0)
        .unwrap_or(1.0)
}

/// The target LOC for the in-envelope fixture: [`logos_core::perf::ENVELOPE_LOC`]
/// by default, overridable down via `LOGOS_PERF_LOC` for a fast harness smoke
/// test (budgets unchanged).
fn target_loc() -> u64 {
    std::env::var("LOGOS_PERF_LOC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(logos_core::perf::ENVELOPE_LOC)
}

/// A wall-clock budget scaled by [`tolerance`].
fn budget(ms: u64) -> Duration {
    Duration::from_millis(ms).mul_f64(tolerance())
}

// ── Fixture generation ──────────────────────────────────────────────────────

/// Functions emitted per generated file.
const FUNCS_PER_FILE: usize = 40;

/// Write `contents` to `<root>/<rel>`, creating parent directories.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dirs");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

/// The 16 shared hub functions every node calls.
const HUBS: usize = 16;

/// Generate a synthetic Rust repo of roughly `goal_loc` lines under `root`,
/// returning `(exact_loc, top_node_id)`.
///
/// The shape is deliberately *navigable* **and** *resolvable*: each file
/// `use`s the hub module, so every node's `hub_*` call binds cross-file — the 16
/// hubs accumulate ~1k callers each (a genuine high-degree `callers` target),
/// while same-file `node_*` calls give dense intra-file edges. Crucially, almost
/// every reference resolves, so the `unresolved_refs` retry ledger stays small —
/// the realistic shape NFR-PE-03 budgets (sync cost ∝ the dirty set, not a repo
/// drowning in permanently-unresolvable refs).
fn generate_repo(root: &Path, goal_loc: u64) -> (u64, usize) {
    // Discovery is gitignore-aware and root-contained (the `.logos/` config dir
    // is created by `Engine::start`); a bare repo with source files is enough.
    let mut loc = 0u64;
    let mut file_idx = 0usize;

    // The hub file: 16 common-named sinks every node calls. Defined first.
    let mut hub = String::new();
    for h in 0..HUBS {
        hub.push_str(&format!("pub fn hub_{h}() -> u64 {{ {h} }}\n"));
    }
    write(root, "src/hub.rs", &hub);
    loc += hub.lines().count() as u64;

    // Explicit import of every hub so `hub_N()` calls bind cross-file.
    let imports: Vec<String> = (0..HUBS).map(|h| format!("hub_{h}")).collect();
    let use_line = format!("use crate::hub::{{{}}};\n", imports.join(", "));

    let mut top_id = 0usize;
    while loc < goal_loc {
        let mut s = use_line.clone();
        for j in 0..FUNCS_PER_FILE {
            let id = file_idx * FUNCS_PER_FILE + j;
            top_id = id;
            // A same-file predecessor call (first-in-file self-calls) + one hub
            // call: ~5 LOC/function, both edges resolvable.
            let a = if j == 0 { id } else { id - 1 };
            s.push_str(&format!(
                "pub fn node_{id}() -> u64 {{\n    \
                 let x = node_{a}();\n    \
                 x.wrapping_add(hub_{}())\n\
                 }}\n",
                id % HUBS,
            ));
        }
        write(root, &format!("src/mod_{file_idx}.rs"), &s);
        loc += s.lines().count() as u64;
        file_idx += 1;
    }
    (loc, top_id)
}

// ── Peak RSS (NFR-PE-06) ────────────────────────────────────────────────────

/// Peak resident-set size of this process in bytes, via `getrusage`.
///
/// `ru_maxrss` is bytes on macOS and kilobytes on Linux — normalised here.
fn peak_rss_bytes() -> u64 {
    // SAFETY: `getrusage` writes a fully-initialised `rusage` into the out-param
    // and returns 0 on success; the struct is zeroed first so a partial write is
    // still defined.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    assert_eq!(rc, 0, "getrusage(RUSAGE_SELF) failed");
    let maxrss = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        maxrss
    } else {
        maxrss * 1024
    }
}

// ── Percentiles ─────────────────────────────────────────────────────────────

/// The `q`-quantile (0.0..=1.0) of `sorted` durations (must be pre-sorted,
/// non-empty), nearest-rank.
fn percentile(sorted: &[Duration], q: f64) -> Duration {
    debug_assert!(!sorted.is_empty());
    let rank = (q * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

// ── The full-envelope fitness function ──────────────────────────────────────

/// One budget's outcome, accumulated for the end-of-run envelope report.
struct Check {
    label: &'static str,
    detail: String,
    pass: bool,
    /// `true` = a breach fails the release gate; `false` = measured-and-reported
    /// only (a documented gap whose fix is scoped to a follow-up — the
    /// "gated-vs-reported" posture the sprint review weighs).
    gated: bool,
}

#[test]
#[ignore = "heavy ~100k-LOC perf gate; run explicitly, throttled (see module docs)"]
fn perf_envelope_full_budget() {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().canonicalize().expect("canonical temp root");
    let goal = target_loc();
    let (loc, top_id) = generate_repo(&root, goal);
    eprintln!("perf_envelope: generated ~{loc} LOC (top node_{top_id})");

    // Every budget records a `Check`; the gate is evaluated once at the end so a
    // single breach does not mask the rest of the envelope (a fitness function
    // reports the *whole* picture each run).
    let mut report: Vec<Check> = Vec::new();

    // Telemetry ON for the whole run (NFR-PE-01 / UAT-NV-07: the budget holds
    // *with* instrumentation). `init` installs the global subscriber + telemetry
    // writer exactly as the CLI adapter does — and needs `.logos/` to exist for
    // the telemetry store, which `Engine::start` would otherwise create after.
    std::fs::create_dir_all(root.join(".logos")).expect("pre-create .logos");
    let guard = observability::init(ProcessSurface::Cli, &root);
    let engine = Arc::new(Engine::start(&root).expect("engine starts"));

    // ── NFR-PE-02: cold index ≤ 30 s ────────────────────────────────────────
    let t = Instant::now();
    let indexed = engine.index();
    let index_elapsed = t.elapsed();
    assert!(
        !indexed
            .warnings
            .iter()
            .any(|w| w.contains("performance envelope")),
        "an at-envelope (~{loc} LOC) index must NOT trip the NFR-PE-09 advisory: {:?}",
        indexed.warnings
    );
    assert!(
        indexed.files_indexed > 0 && indexed.nodes_created > 0,
        "the fixture indexed: {indexed:?}"
    );
    report.push(Check {
        label: "NFR-PE-02 cold index ≤30s",
        detail: format!(
            "{index_elapsed:?} ({} files, {} nodes)",
            indexed.files_indexed, indexed.nodes_created
        ),
        pass: index_elapsed <= budget(30_000),
        gated: true,
    });

    // ── NFR-PE-06: peak RSS ≤ 1 GB (after the heaviest phase) ────────────────
    let rss = peak_rss_bytes();
    let rss_budget = (1024.0 * 1024.0 * 1024.0 * tolerance()) as u64;
    report.push(Check {
        label: "NFR-PE-06 peak RSS ≤1GB",
        detail: format!("{} MiB", rss / (1024 * 1024)),
        pass: rss <= rss_budget,
        gated: true,
    });

    // ── NFR-PE-07 / AA-04: hydration cache hit on a repeat run ───────────────
    // Two hydrations with no intervening change must serve the second from the
    // resident view — a hit, no rebuild.
    let before = engine.hydration_stats();
    let _v1 = engine.hydrate(Granularity::File).expect("first hydrate");
    let _v2 = engine.hydrate(Granularity::File).expect("second hydrate (no change)");
    let after = engine.hydration_stats();
    report.push(Check {
        label: "NFR-PE-07 hydration cache hit on repeat",
        detail: format!("hits {} → {}", before.hits, after.hits),
        pass: after.hits > before.hits,
        gated: true,
    });

    // ── NFR-PE-03: single-file sync ≤ 250 ms on the big graph ────────────────
    // Sync ONE small, self-contained leaf file (a developer adding a helper): the
    // navigation-bearing reconcile work scales with the dirty set, not the repo.
    // All three reconcile passes are now change-proportional on the sync hot path
    // (S-024-HF): resolve re-binds only the change-affected ledger rows (CR-015),
    // the framework-promotion pass skips its whole-graph snapshot on a
    // framework-free sync (a cheap footprint probe gates it), and the annotation
    // pass still recomputes whole-graph verdicts — so a cross-file dead-code /
    // duplicate flip is never missed — but commits only the verdicts that actually
    // changed instead of re-writing every node. A GATED budget: a regression that
    // re-introduces a whole-graph pass on the watcher hot path fails the release.
    write(
        &root,
        "src/perf_leaf.rs",
        "pub fn perf_edit_marker() -> u64 {\n    perf_edit_helper()\n}\npub fn perf_edit_helper() -> u64 { 7 }\n",
    );
    let t = Instant::now();
    let sync = engine.sync(&[root.join("src/perf_leaf.rs")]);
    let sync_elapsed = t.elapsed();
    assert_eq!(sync.files_added, 1, "the one new leaf file synced: {sync:?}");
    report.push(Check {
        label: "NFR-PE-03 single-file sync ≤250ms",
        detail: format!("{sync_elapsed:?}"),
        pass: sync_elapsed <= budget(250),
        gated: true,
    });

    // ── NFR-PE-04: warm aggregate scan ≤ 2 s (few dirty files) ───────────────
    // Warm the reconcile once, then time a scan whose reconcile sees nothing
    // dirty — proportional to the (empty) dirty set, not the repo.
    let _ = engine.scan(true).expect("warm-up scan");
    let t = Instant::now();
    let scan = engine.scan(true).expect("warm scan");
    let scan_elapsed = t.elapsed();
    assert!(
        scan.signal.is_some(),
        "scan produced a quality signal over the populated graph"
    );
    report.push(Check {
        label: "NFR-PE-04 warm scan ≤2s",
        detail: format!("{scan_elapsed:?}"),
        pass: scan_elapsed <= budget(2_000),
        gated: true,
    });

    // ── NFR-PE-01 / UAT-NV-07 / AR-02: point-query p95<100ms / p99<250ms under
    // concurrent navigation load, telemetry on ──────────────────────────────
    let latencies = run_concurrent_navigation_load(&engine, top_id);
    let mut sorted = latencies;
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    let p99 = percentile(&sorted, 0.99);
    report.push(Check {
        label: "NFR-PE-01 point-query p95<100ms (concurrent, telemetry on)",
        detail: format!("p50 {p50:?} · p95 {p95:?} ({} queries)", sorted.len()),
        pass: p95 < budget(100),
        gated: true,
    });
    report.push(Check {
        label: "NFR-PE-01 point-query p99<250ms (concurrent, telemetry on)",
        detail: format!("p99 {p99:?}"),
        pass: p99 < budget(250),
        gated: true,
    });

    // Telemetry was genuinely on throughout (the budget was paid with
    // instrumentation): flush and confirm usage was recorded.
    drop(guard);
    let stats = Engine::open(&root).stats(None);
    report.push(Check {
        label: "NFR-PE-01 telemetry recorded under load",
        detail: format!("{} calls", stats.calls_total),
        pass: stats.calls_total > 0,
        gated: true,
    });

    // ── Envelope report + gate ───────────────────────────────────────────────
    eprintln!(
        "\n── performance envelope @ ~{loc} LOC (tolerance ×{}) ──",
        tolerance()
    );
    for c in &report {
        let tag = match (c.pass, c.gated) {
            (true, _) => "PASS  ",
            (false, true) => "FAIL  ",
            (false, false) => "OVER* ",
        };
        eprintln!("  [{tag}] {} — {}", c.label, c.detail);
    }
    eprintln!("  (* OVER = reported-only budget exceeded; not a gate failure)\n");

    let breaches: Vec<&str> = report
        .iter()
        .filter(|c| c.gated && !c.pass)
        .map(|c| c.label)
        .collect();
    assert!(
        breaches.is_empty(),
        "release-gating performance budgets breached at ~{loc} LOC: {breaches:?} \
         (tolerance ×{}; re-run in isolation before treating as a regression)",
        tolerance()
    );
}

/// Hammer the engine with point queries from several threads at once and return
/// every per-call latency. Bounded thread count keeps the harness from
/// oversubscribing the host (the reads still run concurrently — the AA-01/AA-02
/// posture under the single writer-actor runtime).
fn run_concurrent_navigation_load(engine: &Arc<Engine>, top_id: usize) -> Vec<Duration> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8);
    let per_worker = 250usize;

    let handles: Vec<_> = (0..workers)
        .map(|w| {
            let engine = Arc::clone(engine);
            std::thread::spawn(move || {
                let mut local = Vec::with_capacity(per_worker * 4);
                // A cheap deterministic-per-thread spread over the id space — no
                // rand dependency; just a stride that visits varied targets.
                let mut id = (w * 37) % (top_id + 1);
                let stride = 101usize;
                for _ in 0..per_worker {
                    id = (id + stride) % (top_id + 1);
                    let name = format!("node_{id}");

                    // The four point queries NFR-PE-01 budgets: search/node/
                    // callers/callees. Each timed individually.
                    for query in 0..4u8 {
                        let t = Instant::now();
                        match query {
                            0 => {
                                let _ = engine.search(&name, None, Some(10));
                            }
                            1 => {
                                let _ = engine.node(&name, false);
                            }
                            2 => {
                                // A hub is a genuine high-degree target (~1k
                                // resolved callers) — the worst-case point query.
                                let _ = engine.callers(&format!("hub_{}", id % 16), Some(50));
                            }
                            _ => {
                                let _ = engine.callees(&name, Some(50));
                            }
                        }
                        local.push(t.elapsed());
                    }
                }
                local
            })
        })
        .collect();

    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().expect("navigation worker panicked"));
    }
    all
}

// ── Cold-index per-phase baseline (FR-OB-06, CR-057 S-225 gate) ─────────────

/// The **cold-index baseline** benchmark — the CR-057 gate ([S-225]).
///
/// Cold-indexes a repo and records a repeatable baseline: total wall-clock, the
/// per-phase breakdown ([FR-OB-06] — discover/load/extract/persist/resolve/
/// framework/dispatch/annotate), and peak RSS ([NFR-PE-06]). No optimization in
/// [S-226]..[S-229] may merge without a before/after number from here.
///
/// The per-phase numbers are read straight off [`logos_core`]'s
/// `IndexResult::phases`, so they come from the single `tracing` seam
/// ([FR-OB-01]) — this benchmark records that breakdown, it does not re-time the
/// pipeline itself (no parallel timing path).
///
/// ## Target
/// - **Default:** a synthetic ~[`ENVELOPE_LOC`]-LOC Rust fixture (portable and
///   repeatable — runs anywhere the fitness suite runs). Size it down with
///   `LOGOS_PERF_LOC` for a fast harness smoke test.
/// - **`LOGOS_BENCH_REPO=<path>`:** cold-index that checkout in place (this is
///   how the "on the Logos repo" baseline is taken — point it at a *clean*
///   checkout, or `rm -rf <path>/.logos` first, for a genuine cold number; build
///   with every `lang-*` feature so a multi-language repo is fully indexed).
///
/// ## Output
/// A single-line JSON baseline record is printed to stderr (prefixed
/// `cold-index-baseline: `) and, when `LOGOS_BENCH_OUT=<file>` is set, written
/// there — so a run's baseline can be diffed against a later run for the
/// before/after gate. Run it twice and confirm the record shape is stable:
///
/// ```text
/// cargo test -p logos-core --features lang-rust --test perf_envelope \
///     cold_index_phase_baseline -- --ignored --nocapture
/// ```
///
/// [S-225]: ../../docs/planning/journal.md#s-225-per-phase-index-instrumentation-and-repeatable-cold-index-benchmark
/// [S-226]: ../../docs/planning/journal.md#s-226-chunked-pass-1-persistence-into-bounded-write-batches
/// [S-229]: ../../docs/planning/journal.md#s-229-parallelize-the-annotation-compute-gated-stretch
/// [FR-OB-06]: ../../docs/specs/requirements/FR-OB-06.md
/// [FR-OB-01]: ../../docs/specs/requirements/FR-OB-01.md
/// [NFR-PE-06]: ../../docs/specs/requirements/NFR-PE-06.md
#[test]
#[ignore = "cold-index baseline benchmark (CR-057 gate); run explicitly (see fn docs)"]
fn cold_index_phase_baseline() {
    // Resolve the target: a real checkout via LOGOS_BENCH_REPO, else a synthetic
    // fixture generated under a throwaway root. The `_tmp` guard keeps the temp
    // dir alive for the synthetic case (dropping it would delete the fixture).
    let (root, loc, _tmp): (PathBuf, u64, Option<TempDir>) =
        match std::env::var_os("LOGOS_BENCH_REPO") {
            Some(path) => {
                let root = PathBuf::from(path)
                    .canonicalize()
                    .expect("LOGOS_BENCH_REPO path resolves");
                eprintln!("cold-index-baseline: target = real repo {}", root.display());
                (root, 0, None)
            }
            None => {
                let tmp = TempDir::new().expect("temp root");
                let root = tmp.path().canonicalize().expect("canonical temp root");
                let (loc, _top) = generate_repo(&root, target_loc());
                eprintln!("cold-index-baseline: target = synthetic ~{loc} LOC fixture");
                (root, loc, Some(tmp))
            }
        };

    // Telemetry ON for the whole run — the baseline must be measured *with*
    // instrumentation (NFR-OO-02: it stays off the hot path, so a paid-with-
    // telemetry number is the honest one). `.logos/` must exist before `init`
    // installs the telemetry writer.
    std::fs::create_dir_all(root.join(".logos")).expect("pre-create .logos");
    let guard = observability::init(ProcessSurface::Cli, &root);
    let engine = Engine::start(&root).expect("engine starts");

    // The cold index — a fresh store, first index() is the cold path.
    let t = Instant::now();
    let indexed = engine.index();
    let wall = t.elapsed();
    let rss_mib = peak_rss_bytes() / (1024 * 1024);
    drop(guard);

    assert!(
        indexed.files_indexed > 0 && indexed.nodes_created > 0,
        "the cold index populated the graph: {indexed:?}"
    );

    let p = indexed.phases;
    let phases_sum = p.discover_ms
        + p.load_ms
        + p.extract_ms
        + p.persist_ms
        + p.resolve_ms
        + p.framework_ms
        + p.dispatch_ms
        + p.annotate_ms;

    // The per-phase breakdown reconciles with the total (FR-OB-06): each phase
    // timed once, none double-counted.
    assert!(
        phases_sum <= indexed.duration_ms,
        "per-phase breakdown exceeds the reported total (double-count?): \
         sum {phases_sum}ms > {}ms — {p:?}",
        indexed.duration_ms
    );

    // Peak RSS stays within the ≤1 GB indexing ceiling (NFR-PE-06), tolerance-
    // banded like the rest of the suite.
    let rss_budget_mib = (1024.0 * tolerance()) as u64;
    assert!(
        rss_mib <= rss_budget_mib,
        "cold-index peak RSS {rss_mib} MiB exceeds the ≤1 GB ceiling (NFR-PE-06)"
    );

    // The repeatable baseline record — stable JSON shape, one line, to stderr
    // and (optionally) a file for the before/after gate.
    let record = serde_json::json!({
        "target_loc": loc,
        "files_indexed": indexed.files_indexed,
        "nodes_created": indexed.nodes_created,
        "edges_created": indexed.edges_created,
        "total_ms": indexed.duration_ms,
        "wall_ms": wall.as_millis() as u64,
        "peak_rss_mib": rss_mib,
        "phases_ms": {
            "discover": p.discover_ms,
            "load": p.load_ms,
            "extract": p.extract_ms,
            "persist": p.persist_ms,
            "resolve": p.resolve_ms,
            "framework": p.framework_ms,
            "dispatch": p.dispatch_ms,
            "annotate": p.annotate_ms,
        },
        "phases_sum_ms": phases_sum,
    });
    let line = serde_json::to_string(&record).expect("baseline record serialises");
    eprintln!("cold-index-baseline: {line}");
    if let Some(out) = std::env::var_os("LOGOS_BENCH_OUT") {
        std::fs::write(&out, format!("{line}\n")).expect("write baseline record");
        eprintln!(
            "cold-index-baseline: recorded to {}",
            PathBuf::from(&out).display()
        );
    }
}

// ── Beyond-envelope graceful degradation (NFR-PE-09) ────────────────────────

#[test]
#[ignore = "heavy >110k-LOC perf gate; run explicitly, throttled (see module docs)"]
fn beyond_envelope_degrades_with_advisory_not_crash() {
    // Materially beyond the envelope (>110k LOC trigger): index + status must
    // emit the one-line advisory, and navigation must still serve correct
    // results — degradation, never a crash (NFR-PE-09, ADR-14).
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path().canonicalize().expect("canonical temp root");
    let goal = (logos_core::perf::ENVELOPE_LOC * 13) / 10; // ~130k LOC, well past trigger
    let (loc, top_id) = generate_repo(&root, goal);

    let engine = Engine::start(&root).expect("engine starts");
    let indexed = engine.index();
    assert!(
        indexed.files_indexed > 0,
        "a beyond-envelope repo still indexes correctly: {indexed:?}"
    );
    assert!(
        indexed
            .warnings
            .iter()
            .any(|w| w.contains("performance envelope")),
        "index past the envelope (~{loc} LOC) emits the NFR-PE-09 advisory: {:?}",
        indexed.warnings
    );

    // `status` repeats the same advisory from the recorded LOC.
    let status = engine.status();
    assert!(
        status.indexed,
        "status reports the beyond-envelope graph as indexed"
    );
    assert!(
        status
            .warnings
            .iter()
            .any(|w| w.contains("performance envelope")),
        "status past the envelope emits the NFR-PE-09 advisory: {:?}",
        status.warnings
    );

    // Correctness is retained beyond the envelope (no wrong results, no crash):
    // a known symbol still resolves and its same-file call edge is navigable.
    let node = engine.node(&format!("node_{top_id}"), false);
    assert!(
        node.warnings.is_empty() && node.node.is_some(),
        "a real symbol still resolves beyond the envelope: {node:?}"
    );
    let callees = engine.callees(&format!("node_{top_id}"), Some(10));
    let want = format!("node_{}", top_id - 1);
    assert!(
        callees.callees.iter().any(|c| c.name == want),
        "same-file call edges are still navigable beyond the envelope (want {want}): {callees:?}"
    );
}

// ── S-391: the configuration and binding phases inside the envelope ─────────

/// The configuration-corpus and property-binding phases, guarded against the
/// three `Must` budgets they will be asked to fit inside (S-391, [CR-121],
/// [NFR-PE-02], [NFR-PE-03], [NFR-PE-05], [ADR-10]).
///
/// # The guard is built before the work it bounds
///
/// [CR-121]'s remaining stories add resolution work to the index and to the
/// sync hot path, and whole-graph re-resolution was already rejected on the
/// per-file budget. These cases exist so that any budget amendment those
/// stories need is requested with a **measured** figure and a phase
/// attribution, rather than to unblock work that is already written.
///
/// # The egress phase is GATED on [S-387], not modelled
///
/// The journal's criterion names **three** phases — config, binding and
/// **egress**. Egress is delivered by [S-387], which is not in this sprint and
/// is now blocked: [S-384]'s gate falsified [CR-121] CRA-02 (12 net-new edges
/// against a declared floor of 16). The clause is therefore marked **GATED in
/// place and never deleted**, following the procedure [FR-WS-22] uses for its
/// own gated marker.
///
/// **The egress phase is unbuilt and therefore unbounded.** It contributes no
/// number to any assertion here, and none is synthesised for it: a stand-in for
/// an unbuilt phase is a number with no referent, and the whole point of
/// measuring before building is to refuse exactly that. When [S-387] lands, its
/// phase joins [`PHASE_BUDGET_MS`] here and the marker comes off.
///
/// # The corpus shape
///
/// One synthetic **member**, sized at the *whole reference estate's* corpus
/// ([CR-121] CRA-10, measured 2026-09-07): [`SOURCES`] configuration sources
/// ([`UNPROFILED_SOURCES`] unprofiled + [`PROFILED_SOURCES`] profiled across
/// [`PROFILES`]), [`KEYS`] distinct canonical keys, and [`BINDING_CLASSES`]
/// `@ConfigurationProperties` classes. Sizing one member at the estate's total
/// is deliberately conservative: the real per-member share is ~1/84th of it.
/// The fixture asserts its own census before any budget is read, because a
/// fixture that quietly generated 170 sources would make every figure below
/// wrong while every assertion still passed.
///
/// [S-384]: ../../docs/planning/journal.md#s-384-measure-service-identity-resolvability-across-the-deploy-corpus
/// [S-387]: ../../docs/planning/journal.md#s-387-outbound-calls-become-first-class-egress-facts
/// [CR-121]: ../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
/// [FR-WS-22]: ../../docs/specs/requirements/FR-WS-22.md
/// [NFR-PE-02]: ../../docs/specs/requirements/NFR-PE-02.md
/// [NFR-PE-03]: ../../docs/specs/requirements/NFR-PE-03.md
/// [NFR-PE-05]: ../../docs/specs/requirements/NFR-PE-05.md
/// [NFR-PE-06]: ../../docs/specs/requirements/NFR-PE-06.md
/// [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md
#[cfg(all(feature = "lang-yaml", feature = "lang-java"))]
mod configuration_envelope {
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use logos_core::extract::config::binding::PropertiesIndex;
    use logos_core::extract::config::corpus::{source_facts, ConfigCorpus, ConfigSourceFact};
    use logos_core::graph_store::ConfigDefinition;
    use logos_core::plugin::LanguageRegistry;
    use logos_core::resolve::binding::{ConfigLookup, KeySource, Resolver};
    use logos_core::{Engine, Runtime};

    use super::{budget, peak_rss_bytes, tolerance, write};

    // ── The reference-estate corpus shape (CR-121 CRA-10) ───────────────────

    /// Unprofiled `application.yml` sources — one per resource directory.
    const UNPROFILED_SOURCES: usize = 44;
    /// `application-<profile>.yml` overlay sources across [`PROFILES`].
    const PROFILED_SOURCES: usize = 130;
    /// Total configuration sources in the synthetic member.
    const SOURCES: usize = UNPROFILED_SOURCES + PROFILED_SOURCES;
    /// Distinct canonical keys the corpus commits.
    const KEYS: usize = 872;
    /// `@ConfigurationProperties` classes the member declares.
    const BINDING_CLASSES: usize = 69;
    /// The five profiles the reference estate declares.
    const PROFILES: [&str; 5] = ["dev", "test", "stage", "prod", "it-jenkins"];

    /// The config-and-binding budget this guard pins: **3 s** for the two phases
    /// together, over the corpus shape above.
    ///
    /// It is a *marginal* budget — the cost the corpus adds to an index that
    /// would happen anyway — which is why it is measured as a delta against an
    /// identical tree whose files are not configuration sources, rather than as
    /// a share of a single run.
    const PHASE_BUDGET_MS: u64 = 3_000;

    /// The egress clause of the journal's criterion, carried verbatim so the
    /// deferral is visible in every run's report rather than only in a document.
    ///
    /// **GATED on S-387 — not deleted.** See the module docs.
    const EGRESS_GATED: &str =
        "egress — GATED on S-387 (blocked: S-384's gate falsified CR-121 CRA-02, \
         12 net-new edges against a floor of 16); the phase is unbuilt and therefore \
         unbounded, and no number is synthesised for it";

    /// The fallback recorded against a breach of the config/binding budget, per
    /// the journal's fifth criterion: drop egress to a ledger-only fact rather
    /// than widen a `Must` budget to fit it.
    const EGRESS_LEDGER_ONLY_FALLBACK: &str =
        "fallback on breach: drop the egress phase to a ledger-only fact rather than \
         amend a Must budget to accommodate it";

    // ── Fixture ─────────────────────────────────────────────────────────────

    /// The YAML body one resource directory commits: its whole share of [`KEYS`]
    /// under a two-level `app: svc<d>:` mapping.
    ///
    /// Keys are dealt round-robin across the [`UNPROFILED_SOURCES`] directories,
    /// so every directory carries 19 or 20 of them as the estate's do, and the
    /// dotted key `app.svc<d>.key<i>` is already canonical — no two directories
    /// can collide on one.
    fn unprofiled_body(dir: usize, value_pad: usize) -> String {
        let mut s = format!("app:\n  svc{dir}:\n");
        for i in (dir..KEYS).step_by(UNPROFILED_SOURCES) {
            s.push_str(&format!("    key{i}: {}\n", value_for(i, value_pad)));
        }
        s
    }

    /// The value committed for key `i`, padded to `value_pad` characters.
    ///
    /// The padding exists for one case only — the 84-member memory guard, which
    /// needs a per-member footprint large enough to see above process noise. The
    /// timing cases leave it at zero and commit realistic short values.
    fn value_for(i: usize, value_pad: usize) -> String {
        let base = format!("value-{i}");
        if value_pad <= base.len() {
            base
        } else {
            format!("{base}{}", "x".repeat(value_pad - base.len()))
        }
    }

    /// A profiled overlay redefines its directory's first key and introduces no
    /// new one — exactly what an overlay does on the estate.
    fn profiled_body(dir: usize, profile: &str) -> String {
        format!("app:\n  svc{dir}:\n    key{dir}: value-{dir}-{profile}\n")
    }

    /// The `(dir, profile)` pairs the [`PROFILED_SOURCES`] overlays occupy.
    ///
    /// Dealt on **both** cycles at once — directory on a 44-cycle, profile on a
    /// 5-cycle — so all five profiles appear and every directory carries about
    /// three overlays, which is the estate's 130-over-44 shape. The two cycle
    /// lengths are coprime, so the pairs stay distinct for the first 220 slots
    /// and no file is written twice. A profile-major deal was tried first and
    /// was wrong for a reason worth keeping: 130 slots exhaust after the third
    /// profile, so the corpus declared three profiles while the constant said
    /// five, and only the census assertion showed it.
    fn profiled_slots() -> Vec<(usize, &'static str)> {
        (0..PROFILED_SOURCES)
            .map(|k| (k % UNPROFILED_SOURCES, PROFILES[k % PROFILES.len()]))
            .collect()
    }

    /// The class at index `n`: its simple name, the directory (and therefore the
    /// prefix) it binds, and the global key index its single property names.
    fn class_spec(n: usize) -> (String, usize, usize) {
        let dir = n % UNPROFILED_SOURCES;
        let key = dir + UNPROFILED_SOURCES * (n / UNPROFILED_SOURCES);
        (format!("Props{n}"), dir, key)
    }

    /// Write the synthetic member under `root`.
    ///
    /// `config_sources` selects the **only** difference between the two arms of
    /// the phase-delta measurement, and that difference is the **filename**:
    /// `true` names the files `application*.yml` (configuration sources, which
    /// the corpus ingests) and `false` names them `settings*.yml` (the same
    /// extension so the same plugin claims them, byte-identical contents so the
    /// same parse and the same artifact extraction — and ingested by nothing).
    /// Everything else about the two trees is identical, so their index-time
    /// difference is the configuration phase and nothing else.
    fn write_member(root: &Path, config_sources: bool) {
        // One module descriptor at the root: this is one member, so every source
        // and every class sits in the module `""`.
        write(root, "pom.xml", "<project><artifactId>member</artifactId></project>\n");

        let stem = if config_sources { "application" } else { "settings" };
        for dir in 0..UNPROFILED_SOURCES {
            write(
                root,
                &format!("svc{dir}/src/main/resources/{stem}.yml"),
                // Unpadded: the on-disk fixture commits realistic short values.
                // Padding exists only for the in-memory 84-member memory guard,
                // which reaches `unprofiled_body` through `ingest_member` and
                // never writes a file.
                &unprofiled_body(dir, 0),
            );
        }
        for (dir, profile) in profiled_slots() {
            write(
                root,
                &format!("svc{dir}/src/main/resources/{stem}-{profile}.yml"),
                &profiled_body(dir, profile),
            );
        }

        for n in 0..BINDING_CLASSES {
            let (name, dir, key) = class_spec(n);
            write(
                root,
                &format!("src/main/java/com/example/{name}.java"),
                &format!(
                    "package com.example;\n\n\
                     @ConfigurationProperties(prefix = \"app.svc{dir}\")\n\
                     public class {name} {{\n    \
                         private String key{key};\n    \
                         public String getKey{key}() {{ return key{key}; }}\n\
                     }}\n"
                ),
            );
        }
    }

    /// The loaded plugin registry, built once per test binary — [`Engine::start`]
    /// pays for its own, and rebuilding one per case would charge query
    /// compilation to a phase that does not do it.
    fn registry() -> &'static LanguageRegistry {
        static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            LanguageRegistry::load(std::env::temp_dir()).expect("plugin registry loads")
        })
    }

    /// Assert the fixture is the shape the budgets were sized for, before any of
    /// them is read.
    ///
    /// A census over the tree itself, not over the constants that wrote it: the
    /// failure this catches is a generator that emits a colliding key or a
    /// filename the admission rule declines, which no amount of re-reading the
    /// constants would show.
    fn assert_census(root: &Path) -> (ConfigCorpus, PropertiesIndex) {
        let corpus = ConfigCorpus::discover(root);
        let keys: BTreeSet<&str> = corpus
            .sources
            .iter()
            .flat_map(|s| s.values.keys().map(String::as_str))
            .collect();
        assert_eq!(
            corpus.sources.len(),
            SOURCES,
            "the fixture must commit exactly {SOURCES} configuration sources",
        );
        assert_eq!(keys.len(), KEYS, "…carrying exactly {KEYS} distinct canonical keys");
        assert_eq!(
            corpus.profiles().len(),
            PROFILES.len(),
            "…across exactly {} profiles",
            PROFILES.len(),
        );
        let properties = PropertiesIndex::build(root, &corpus, registry());
        assert_eq!(
            properties.len(),
            BINDING_CLASSES,
            "…and exactly {BINDING_CLASSES} @ConfigurationProperties classes",
        );
        (corpus, properties)
    }

    /// Every committed definition of one canonical key, as the member store
    /// answers it — the same read `config_corpus.rs` makes.
    fn definitions(rt: &Runtime, key: &str) -> Vec<ConfigDefinition> {
        let key = key.to_string();
        rt.submit_read(move |store| store.config_definitions(&key))
            .expect("read runs")
    }

    /// `(config_sources, config_values)` straight off the member store.
    ///
    /// Raw SQL, deliberately: "nothing was ingested" is a statement about the
    /// TABLES, and a key-by-key probe can only ever show that the keys it
    /// happened to name are absent.
    fn corpus_row_counts(root: &Path) -> (i64, i64) {
        let db = root.join(".logos").join("logos.db");
        assert!(db.is_file(), "no store at {}", db.display());
        let conn = rusqlite::Connection::open_with_flags(
            &db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("open the member store read-only");
        conn.query_row(
            "SELECT (SELECT count(*) FROM config_sources), (SELECT count(*) FROM config_values)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("the migration-19 tables exist")
    }

    /// Compose the amendment request a breached budget must carry (the journal's
    /// fifth criterion).
    ///
    /// Every breach message in this module is rendered through here, so a
    /// request cannot be made without the measured figure, the per-phase
    /// attribution behind it, the gated phase that contributed none, and the
    /// recorded fallback. `phases` is `(name, measured ms)` in the order they
    /// run.
    fn amendment_request(
        requirement: &str,
        budget_ms: u64,
        measured_ms: u128,
        phases: &[(&str, u128)],
    ) -> String {
        let attribution = phases
            .iter()
            .map(|(name, ms)| format!("{name} {ms} ms"))
            .collect::<Vec<_>>()
            .join(" + ");
        format!(
            "BUDGET AMENDMENT REQUEST ({requirement}): measured {measured_ms} ms against a \
             {budget_ms} ms budget (tolerance ×{tol}).\n  \
             phase attribution: {attribution}\n  \
             {EGRESS_GATED}\n  \
             {EGRESS_LEDGER_ONLY_FALLBACK}",
            tol = tolerance(),
        )
    }

    /// Wall-clock of one cold `index()` over `root`, engine start excluded — the
    /// start path is identical in both arms of the delta and timing it would put
    /// query compilation inside a phase that does not compile queries.
    fn index_wall_ms(root: &Path) -> u128 {
        let engine = Engine::start(root).expect("engine starts");
        let t = Instant::now();
        let indexed = engine.index();
        let elapsed = t.elapsed();
        assert!(
            indexed.files_indexed > 0 && indexed.nodes_created > 0,
            "the member indexed: {indexed:?}"
        );
        elapsed.as_millis()
    }

    /// The key `accessor` binds on `class`, through the S-381 index — the
    /// accessor → field → prefix → canonical key chain a real use site walks.
    fn bound_key(properties: &PropertiesIndex, class: &str, accessor: &str) -> String {
        let declared = properties.get(class, "").unwrap_or_else(|| panic!("{class} indexed"));
        properties
            .bind(declared, accessor)
            .unwrap_or_else(|e| panic!("{class}::{accessor} binds: {e:?}"))
            .key
    }

    /// [`ConfigLookup`] over a live member store — the production-shaped read
    /// path for configuration resolution, as [`Resolver`] consumes it.
    struct StoreCorpus<'a>(&'a Runtime);

    impl ConfigLookup for StoreCorpus<'_> {
        fn definitions(&self, key: &str, _module: &str) -> Vec<ConfigDefinition> {
            // A single member's store holds only its own corpus, so the module
            // filter is already applied by the store itself — the same reading
            // `MemberCorpus` makes in the federation coverage tier.
            definitions(self.0, key)
        }
    }

    /// One member's configuration corpus, ingested through the **production**
    /// entry point ([`source_facts`], which takes text the extract pass already
    /// holds and opens no file), and returned so the caller decides its lifetime.
    fn ingest_member(value_pad: usize) -> Vec<ConfigSourceFact> {
        let mut facts = Vec::with_capacity(SOURCES);
        for dir in 0..UNPROFILED_SOURCES {
            let text = unprofiled_body(dir, value_pad);
            let path = format!("svc{dir}/src/main/resources/application.yml");
            facts.push(source_facts(&path, &text).expect("a configuration source"));
        }
        for (dir, profile) in profiled_slots() {
            let text = profiled_body(dir, profile);
            let path = format!("svc{dir}/src/main/resources/application-{profile}.yml");
            facts.push(source_facts(&path, &text).expect("a configuration source"));
        }
        facts
    }

    // ── AC1: the config and binding phase delta, within 3 s ─────────────────

    /// **AC1.** The configuration and binding phases, measured over the member
    /// the estate's corpus shape sizes, stay inside a 3 s delta.
    ///
    /// # Why a delta, and against what
    ///
    /// The configuration phase has no timer of its own: flattening happens
    /// inside extraction, per file, on text the pass already read ([CR-121]
    /// CRA-07). Timing a single run would therefore charge it for the YAML
    /// artifact extraction those files pay anyway. So it is measured as the
    /// difference between two indexes of trees that are identical **except**
    /// that one arm's files are named `application*.yml` and the other's
    /// `settings*.yml`. The **filename is the whole difference**: same
    /// extension, so the same plugin claims them; byte-identical contents, so
    /// the same parse and the same artifact extraction; and only the first stem
    /// is a configuration source. What is left in the delta is the corpus
    /// flatten plus its persistence, and nothing else.
    ///
    /// The binding phase is timed at [`PropertiesIndex::build`], the only entry
    /// point with a corpus behind it. That entry point reads its candidate files
    /// itself, which production ingestion would not (`absorb_source` takes text
    /// the pass already holds), so this figure **over-states** the phase — the
    /// safe direction for a guard.
    ///
    /// The **egress** phase contributes nothing: see [`EGRESS_GATED`].
    ///
    /// [CR-121]: ../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    #[test]
    fn the_configuration_and_binding_phases_stay_inside_the_three_second_delta() {
        let bare_tmp = TempDir::new().expect("temp root");
        let bare = bare_tmp.path().canonicalize().expect("canonical temp root");
        write_member(&bare, false);

        let full_tmp = TempDir::new().expect("temp root");
        let full = full_tmp.path().canonicalize().expect("canonical temp root");
        write_member(&full, true);
        let (corpus, _) = assert_census(&full);

        let bare_ms = index_wall_ms(&bare);
        let full_ms = index_wall_ms(&full);

        // The arms differ in exactly one thing, and both halves of that are
        // checked: the corpus arm ingested, and the control arm did not. Without
        // the second check a control arm that quietly ingested would deflate the
        // delta to nothing and the guard would pass by measuring zero.
        let (sources, values) = corpus_row_counts(&full);
        assert_eq!(sources as usize, SOURCES, "the corpus arm ingested every source");
        assert!(values > 0, "…and its values: {values}");
        assert_eq!(
            corpus_row_counts(&bare),
            (0, 0),
            "the control arm is byte-identical work with no configuration source in it",
        );

        let t = Instant::now();
        let properties = PropertiesIndex::build(&full, &corpus, registry());
        let binding_ms = t.elapsed().as_millis();
        assert_eq!(properties.len(), BINDING_CLASSES, "the binding phase indexed the classes");

        let config_ms = full_ms.saturating_sub(bare_ms);
        let total_ms = config_ms + binding_ms;
        let budget = budget(PHASE_BUDGET_MS);

        eprintln!(
            "\n── S-391 configuration/binding envelope ({SOURCES} sources, {KEYS} keys, \
             {BINDING_CLASSES} classes) ──\n  \
             index with configuration sources: {full_ms} ms\n  \
             index without (control arm):      {bare_ms} ms\n  \
             config phase (delta):             {config_ms} ms\n  \
             binding phase (build):            {binding_ms} ms\n  \
             config + binding:                 {total_ms} ms / {} ms budget\n  \
             {EGRESS_GATED}\n",
            budget.as_millis(),
        );

        assert!(
            Duration::from_millis(total_ms as u64) <= budget,
            "{}",
            amendment_request(
                "NFR-PE-02 config+binding",
                PHASE_BUDGET_MS,
                total_ms,
                &[("config", config_ms), ("binding", binding_ms)],
            ),
        );
    }

    // ── AC2: one edited key re-resolves one key, inside the sync budget ─────

    /// **AC2.** Editing one configuration key re-resolves only the sites whose
    /// bound key changed — never the module — and completes inside the
    /// single-file sync budget ([NFR-PE-03]).
    ///
    /// Named with `pe03_budget` so the fast gate tier skips it for the same
    /// reason it skips every other wall-clock guard: a loaded host should not
    /// redden the iteration loop. It runs in the full tier and in CI.
    ///
    /// "Never the module" is read off two facts together: the sync touched
    /// exactly one of the member's files, and it finished inside a budget a
    /// module-wide re-resolution of 174 sources and 69 classes could not.
    ///
    /// [NFR-PE-03]: ../../docs/specs/requirements/NFR-PE-03.md
    #[test]
    fn editing_one_key_re_resolves_only_that_key_within_the_pe03_budget() {
        let tmp = TempDir::new().expect("temp root");
        let root = tmp.path().canonicalize().expect("canonical temp root");
        write_member(&root, true);
        let (_, properties) = assert_census(&root);

        let engine = Engine::start(&root).expect("engine starts");
        engine.index();
        let rt = engine.runtime().expect("runtime present");

        // Two accessor sites in the same module, bound through S-381 to two
        // keys committed by two DIFFERENT sources, plus a third key committed by
        // the same source as the first — so "only the sites whose bound key
        // changed" is tested against a same-file neighbour as well as a
        // different-file one.
        let (changed_class, changed_dir, changed_i) = class_spec(0);
        let (neighbour_class, neighbour_dir, neighbour_i) = class_spec(UNPROFILED_SOURCES);
        let (stable_class, stable_dir, stable_i) = class_spec(1);
        assert_eq!(changed_dir, neighbour_dir, "the neighbour shares the edited source");
        assert_ne!(changed_dir, stable_dir, "the stable key is committed elsewhere");

        let changed_key = bound_key(&properties, &changed_class, &format!("getKey{changed_i}"));
        let neighbour_key =
            bound_key(&properties, &neighbour_class, &format!("getKey{neighbour_i}"));
        let stable_key = bound_key(&properties, &stable_class, &format!("getKey{stable_i}"));

        let store = StoreCorpus(rt);
        let resolver = Resolver { corpus: &store, module: "" };
        let before_changed = resolver
            .resolve(&changed_key, KeySource::Properties)
            .expect("the edited key resolves before the edit");
        let before_neighbour = resolver
            .resolve(&neighbour_key, KeySource::Properties)
            .expect("its same-file neighbour resolves");
        let before_stable = resolver
            .resolve(&stable_key, KeySource::Properties)
            .expect("a key in another source resolves");

        // Edit exactly one key's value in exactly one source; every other line
        // of that file, and every other file, is byte-identical.
        let edited_rel = format!("svc{changed_dir}/src/main/resources/application.yml");
        let edited = unprofiled_body(changed_dir, 0).replace(
            &format!("key{changed_i}: value-{changed_i}\n"),
            &format!("key{changed_i}: value-{changed_i}-edited\n"),
        );
        assert!(
            edited.contains(&format!("key{changed_i}: value-{changed_i}-edited")),
            "the edit landed in the fixture text",
        );
        write(&root, &edited_rel, &edited);

        let t = Instant::now();
        let sync = engine.sync(&[root.join(&edited_rel)]);
        let elapsed = t.elapsed();

        assert_eq!(sync.files_modified, 1, "one file changed: {sync:?}");
        assert_eq!(sync.files_added, 0, "…and none was added: {sync:?}");
        assert_eq!(sync.files_removed, 0, "…and none removed: {sync:?}");

        let after_changed = resolver
            .resolve(&changed_key, KeySource::Properties)
            .expect("the edited key still resolves");
        let after_neighbour = resolver
            .resolve(&neighbour_key, KeySource::Properties)
            .expect("its neighbour still resolves");
        let after_stable = resolver
            .resolve(&stable_key, KeySource::Properties)
            .expect("the other source's key still resolves");

        assert_ne!(
            after_changed, before_changed,
            "the site bound to the edited key re-resolved",
        );
        assert!(
            after_changed
                .values
                .iter()
                .any(|v| v.value == format!("value-{changed_i}-edited")),
            "…to the value the edit committed: {after_changed:?}",
        );
        assert_eq!(
            after_neighbour, before_neighbour,
            "a key in the SAME edited source, untouched by the edit, did not move",
        );
        assert_eq!(
            after_stable, before_stable,
            "and a key in another source of the same module did not move either",
        );

        eprintln!(
            "\n── S-391 single-key re-resolution ──\n  \
             sync of 1 of {} member files: {elapsed:?} / {:?} budget\n",
            SOURCES + BINDING_CLASSES + 1,
            budget(250),
        );
        assert!(
            elapsed <= budget(250),
            "{}",
            amendment_request(
                "NFR-PE-03 single-file sync",
                250,
                elapsed.as_millis(),
                &[("single-file sync after a one-key edit", elapsed.as_millis())],
            ),
        );
    }

    // ── AC3: nothing is ingested at process start ───────────────────────────

    /// **AC3, the exact half.** Process start ingests nothing.
    ///
    /// An engine started over a member carrying [`SOURCES`] configuration
    /// sources — and never indexed — has written **no** corpus row. Read as raw
    /// SQL over the two migration-19 tables, so it is a statement about the
    /// tables rather than about the keys this test happened to name.
    ///
    /// Deliberately **not** named for a budget: it carries no wall clock, so it
    /// must keep running in the gate tier that skips the timing guards. The
    /// before/after figure the criterion also asks for is its sibling,
    /// [`cold_start_is_unchanged_by_a_configuration_corpus_within_the_pe05_budget`].
    #[test]
    fn nothing_is_ingested_at_process_start() {
        let tmp = TempDir::new().expect("temp root");
        let root = tmp.path().canonicalize().expect("canonical temp root");
        write_member(&root, true);

        let engine = Engine::start(&root).expect("engine starts");
        assert!(engine.runtime().is_some(), "engine is ready to serve");
        drop(engine);

        assert_eq!(
            corpus_row_counts(&root),
            (0, 0),
            "process start ingested a configuration corpus it was never asked to index",
        );
    }

    /// **AC3, the before/after figure.** The cold-start total is unchanged by
    /// the presence of a configuration corpus ([NFR-PE-05]).
    ///
    /// The criterion asks for a measured before/after rather than an assertion
    /// that lazy initialisation was written, so the same member is built twice —
    /// once with configuration sources, once with the same files under names the
    /// corpus does not admit — and the two cold-start totals are compared
    /// against each other and against the budget.
    ///
    /// The comparison is anchored on a **measured** figure, not a tolerance
    /// invented here: the corpus is ingested through the production entry point
    /// first, and the start-time difference must come in under that cost. An
    /// implementation that ingested at start would have to pay it.
    ///
    /// [NFR-PE-05]'s 600 ms bounds the **total** wall time of `Engine::start`
    /// over its six phases, and explicitly forbids gating any subset of them in
    /// its name — so this measures the whole public call, exactly as
    /// `cold_start_to_ready_engine_is_within_pe05_budget` does. Named with
    /// `pe05_budget` so the fast gate tier skips it alongside that guard.
    ///
    /// [NFR-PE-05]: ../../docs/specs/requirements/NFR-PE-05.md
    #[test]
    fn cold_start_is_unchanged_by_a_configuration_corpus_within_the_pe05_budget() {
        let full_tmp = TempDir::new().expect("temp root");
        let full = full_tmp.path().canonicalize().expect("canonical temp root");
        write_member(&full, true);

        let bare_tmp = TempDir::new().expect("temp root");
        let bare = bare_tmp.path().canonicalize().expect("canonical temp root");
        write_member(&bare, false);

        let t = Instant::now();
        let engine = Engine::start(&full).expect("engine starts");
        let full_cold = t.elapsed();
        assert!(engine.runtime().is_some(), "engine is ready to serve");
        drop(engine);

        let t = Instant::now();
        let engine = Engine::start(&bare).expect("engine starts");
        let bare_cold = t.elapsed();
        drop(engine);

        // What eager ingestion would cost, measured through the production
        // ingestion entry point over this very corpus.
        let t = Instant::now();
        let corpus = ConfigCorpus::discover(&full);
        let ingested: usize = corpus
            .sources
            .iter()
            .filter_map(|s| {
                let text = std::fs::read_to_string(full.join(&s.path)).ok()?;
                source_facts(&s.path, &text)
            })
            .map(|f| f.values.len())
            .sum();
        let ingest = t.elapsed();
        assert!(ingested > 0, "the ingestion probe read the corpus it is pricing");

        // Steady-state starts, three samples each, compared on the minimum — the
        // least noise-prone estimator of a fixed cost, and the arms are sampled
        // in alternation so a drifting host loads both equally.
        let mut full_min = full_cold;
        let mut bare_min = bare_cold;
        for _ in 0..2 {
            let t = Instant::now();
            drop(Engine::start(&full).expect("engine starts"));
            full_min = full_min.min(t.elapsed());
            let t = Instant::now();
            drop(Engine::start(&bare).expect("engine starts"));
            bare_min = bare_min.min(t.elapsed());
        }
        let delta = full_min.saturating_sub(bare_min);

        eprintln!(
            "\n── S-391 cold start, before/after a configuration corpus ──\n  \
             with {SOURCES} configuration sources: cold {full_cold:?}, min-of-3 {full_min:?}\n  \
             without (same files, unadmitted names): cold {bare_cold:?}, min-of-3 {bare_min:?}\n  \
             difference: {delta:?}\n  \
             cost of ingesting that corpus (what eager ingestion would add): {ingest:?}\n  \
             budget: {:?}\n",
            budget(600),
        );

        for (label, measured) in [("with a corpus", full_cold), ("without one", bare_cold)] {
            assert!(
                measured <= budget(600),
                "{}",
                amendment_request(
                    "NFR-PE-05 cold start to a ready engine",
                    600,
                    measured.as_millis(),
                    &[(label, measured.as_millis())],
                ),
            );
        }
        assert!(
            delta < ingest,
            "cold start grew by {delta:?} when a configuration corpus is present, which is at \
             least what ingesting it costs ({ingest:?}) — startup is no longer independent of \
             the corpus (with {full_min:?} vs without {bare_min:?})",
        );
    }

    // ── AC4: per-member corpora are dropped, not accumulated ────────────────

    /// **AC4.** A corpus is held for one member's ingestion and dropped after
    /// it, so an 84-member workspace's peak stays inside the memory budget
    /// ([NFR-PE-06]).
    ///
    /// # The control arm is the point
    ///
    /// A memory assertion that has never been shown to move is decoration. So
    /// the same 84-member loop runs twice: once in the production shape
    /// (ingest → use → drop) and once **retaining** every member's corpus, which
    /// is the failure being guarded against. The retaining arm must cost
    /// materially more, or the instrument is blind and this test says so instead
    /// of passing.
    ///
    /// # Two deliberate distortions, both stated
    ///
    /// - **Values are padded** to [`MEMORY_VALUE_PAD`] characters. Real
    ///   configuration values are short, and 84 realistic corpora would sit
    ///   under the noise floor of a process-wide RSS reading. The claim under
    ///   test is a *lifetime* claim, and padding is what makes a lifetime
    ///   observable. One member's **unpadded** corpus is summed exactly and
    ///   printed alongside, so the estate-scale footprint the padding stands in
    ///   for is on the record in the same report.
    /// - **`ru_maxrss` is process-wide.** Under `--test-threads` > 1 a sibling
    ///   case's allocations land in whichever arm is running. That can only push
    ///   the arms together, so it makes this test fail rather than pass — the
    ///   safe direction — and a failure here should be re-run in isolation
    ///   before it is read as a regression, exactly as the wall-clock guards are.
    ///
    /// [NFR-PE-06]: ../../docs/specs/requirements/NFR-PE-06.md
    #[test]
    fn per_member_corpora_are_dropped_inside_the_workspace_memory_budget() {
        /// The reference workspace's member count.
        const MEMBERS: usize = 84;
        /// See the distortions note above.
        const MEMORY_VALUE_PAD: usize = 4096;
        /// How much more the retaining arm must cost for the instrument to be
        /// credited with being able to see accumulation at all.
        const SENSITIVITY: u64 = 3;

        // Arm A — the production shape: one member's corpus at a time.
        let baseline = peak_rss_bytes();
        let mut dropped_values = 0usize;
        for _ in 0..MEMBERS {
            let facts = ingest_member(MEMORY_VALUE_PAD);
            dropped_values += facts.iter().map(|f| f.values.len()).sum::<usize>();
            drop(facts);
        }
        let dropping = peak_rss_bytes().saturating_sub(baseline);

        // Arm B — the control: the same work, every member retained.
        let before_retaining = peak_rss_bytes();
        let mut retained = Vec::with_capacity(MEMBERS);
        for _ in 0..MEMBERS {
            retained.push(ingest_member(MEMORY_VALUE_PAD));
        }
        let retained_values: usize =
            retained.iter().flatten().map(|f| f.values.len()).sum();
        let retaining = peak_rss_bytes().saturating_sub(before_retaining);
        let peak = peak_rss_bytes();
        drop(retained);

        // Both arms did the same work — otherwise a cheaper arm A would be
        // measuring less, not holding less.
        assert_eq!(
            dropped_values, retained_values,
            "the two arms must ingest the same corpus to be comparable",
        );

        // The estate-scale figure the padding hides: one member's corpus as it
        // is actually committed, summed exactly over the keys and values it
        // holds. Reported rather than asserted — it is the number a reader needs
        // to convert the padded arms above back to real terms, and it is far too
        // small to read off a process-wide RSS sample, which is why the arms are
        // padded in the first place.
        let unpadded: usize = ingest_member(0)
            .iter()
            .flat_map(|f| f.values.iter())
            .map(|v| v.key.len() + v.value.len())
            .sum();

        let mib = |b: u64| b / (1024 * 1024);
        eprintln!(
            "\n── S-391 per-member corpus lifetime over {MEMBERS} members ──\n  \
             values ingested per arm:      {dropped_values}\n  \
             dropping each member's corpus: +{} MiB peak\n  \
             retaining all {MEMBERS} (control):    +{} MiB peak\n  \
             process peak:                 {} MiB / {} MiB budget\n  \
             one member's corpus unpadded: {unpadded} bytes of keys and values\n",
            mib(dropping),
            mib(retaining),
            mib(peak),
            mib((1024.0 * 1024.0 * 1024.0 * tolerance()) as u64),
        );

        assert!(
            retaining > dropping.saturating_mul(SENSITIVITY),
            "the instrument cannot see accumulation: retaining all {MEMBERS} corpora cost \
             {} MiB against {} MiB for dropping each after its member, which is not the \
             ≥{SENSITIVITY}× separation this guard needs to mean anything — re-run in \
             isolation (ru_maxrss is process-wide) before reading it as a regression",
            mib(retaining),
            mib(dropping),
        );
        assert!(
            peak <= (1024.0 * 1024.0 * 1024.0 * tolerance()) as u64,
            "peak RSS {} MiB over an {MEMBERS}-member workspace exceeds the ≤1 GB ceiling \
             (NFR-PE-06)",
            mib(peak),
        );
    }

    // ── AC5: an amendment request carries its measured figure ───────────────

    /// **AC5.** A budget amendment cannot be requested from here without the
    /// measured figure, the phase it is attributed to, the gated phase that
    /// contributed none, and the recorded fallback.
    ///
    /// Every breach message above is rendered through [`amendment_request`], so
    /// this is the whole of the criterion: there is no second path by which a
    /// breach could be reported bare. No budget is breached today — that is what
    /// the cases above establish — so the composer is driven with a synthetic
    /// breach rather than left unexercised until the day one happens.
    #[test]
    fn a_budget_amendment_request_carries_its_measured_figure_and_attribution() {
        let request = amendment_request(
            "NFR-PE-02 config+binding",
            PHASE_BUDGET_MS,
            3_412,
            &[("config", 2_900), ("binding", 512)],
        );

        assert!(request.contains("3412 ms"), "the measured total: {request}");
        assert!(request.contains("3000 ms budget"), "the budget it breached: {request}");
        assert!(
            request.contains("config 2900 ms + binding 512 ms"),
            "the per-phase attribution behind the total: {request}",
        );
        assert!(
            request.contains("GATED on S-387"),
            "the phase that is deferred rather than measured: {request}",
        );
        assert!(
            request.contains("ledger-only fact"),
            "the recorded fallback: {request}",
        );
        assert!(
            request.contains("NFR-PE-02 config+binding"),
            "the requirement whose budget is at stake: {request}",
        );
    }
}
