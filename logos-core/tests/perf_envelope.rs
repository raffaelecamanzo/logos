//! The performance envelope as measured, release-gating fitness functions
//! (S-024, [performance NFRs], [navigation-service], [execution-runtime]).
//!
//! Each budget Logos promises is asserted here against a synthetic ~100k-LOC
//! Rust fixture — the small/medium [`ENVELOPE_LOC`] target the budgets are tuned
//! for ([NFR-PE-02]):
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
//! Every budget test is `#[ignore]` — building and indexing ~100k LOC is far too
//! heavy for the default `cargo test` run, and (per the sprint risk register)
//! timing assertions flake under host oversubscription. Run them deliberately,
//! **throttled**, and re-run a breach in isolation before treating it as a
//! regression:
//!
//! ```text
//! cargo test -p logos-core --features lang-rust --test perf_envelope \
//!     -- --ignored --test-threads=1
//! ```
//!
//! Two env knobs keep the budgets honest without editing them:
//! - `LOGOS_PERF_TOLERANCE` (f64 ≥ 1.0, default 1.0) widens every wall-clock band
//!   for a slow/loaded CI host — the tolerance-banding the sprint mandates;
//! - `LOGOS_PERF_LOC` (u64, default [`ENVELOPE_LOC`]) shrinks the fixture for a
//!   fast harness smoke-test; the *budgets are unchanged*, so a reduced run only
//!   proves the harness wiring, not the full-scale gate.
//!
//! Gated on `lang-rust`: the fixtures are Rust sources.
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

use logos_core::observability::{self, Surface};
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
    let guard = observability::init(Surface::Cli, &root);
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
    let guard = observability::init(Surface::Cli, &root);
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
