//! Per-phase cold-start attribution for `Engine::start` ([CR-116], [S-368],
//! [NFR-PE-05]).
//!
//! [NFR-PE-05]'s ≤500 ms budget and `cold_start_to_ready_engine_is_within_pe05_budget`
//! (`runtime_concurrency.rs`) do not enumerate the same phases: the requirement
//! bounds `plugin.toml` parse, `LanguageRegistry` construction and query
//! compilation (plus, under `serve`, watcher registration); the test measures
//! the whole `Engine::start`, which additionally opens/migrates the store and
//! brings up the pools, and does not register a watcher. Neither list is a
//! subset of the other, so a number produced by one and judged against the
//! other settles nothing (CR-116 §3.2).
//!
//! This test attributes the cost instead of guessing at it. It reports:
//!
//! - the **distribution** (min/median/p90/max/mean) of every named phase,
//!   plus the full measured total and the [NFR-PE-05]-enumerated subtotal —
//!   not just a mean, so a bimodal cause is not averaged away (CR-116 CRA-02);
//! - the [NFR-PE-05]-enumerated subtotal **explicitly and separately** — the
//!   number that decides CR-116 §3.2 branch (a) (a genuine breach) versus
//!   branch (b) (the guard bounds more than the requirement does);
//! - the **instrumentation's own cost**: the mean of the per-sample delta
//!   between the instrumented and uninstrumented totals (CR-116 R2).
//!
//! ## Why every sample is a fresh process
//!
//! An early exploratory run of this measurement, driven in a loop *inside one
//! process*, reported a steady ~487 ms mean — comfortably under budget. But
//! running the **existing** `cold_start_to_ready_engine_is_within_pe05_budget`
//! test binary five times in a row (five separate process launches) gave
//! 1703 / 1333 / 636 / 581 / 525 ms: a clear descending trend, not noise
//! scattered around a mean. The cause is external to the phases themselves —
//! most plausibly CPU frequency/power-state ramp-up after the process (and the
//! host) have been idle, compounded by page-cache warmth for the freshly built
//! binary. A same-process loop cannot see this: once *that* process has paid
//! the ramp-up cost on its first sample, every later sample in the same loop
//! inherits the warm state and reports a falsely tight distribution.
//!
//! So each sample here is measured in its **own freshly spawned process**
//! ([`cold_start_phase_attribution_child_sample`], invoked as a subprocess of
//! this binary) — the only way to let cross-process effects like CPU
//! ramp-up show up in the reported distribution instead of being silently
//! averaged away by a warm loop.
//!
//! This story changes no budget and widens no tolerance (CR-116 §3.3 scope,
//! [S-368] AC6) — it reports, it does not gate. Nothing here asserts against
//! the 500 ms figure; that reconciliation is [S-369]'s job.
//!
//! ## Running it
//!
//! ```text
//! cargo test -p logos-core --test cold_start_phase_attribution \
//!     cold_start_phase_attribution -- --exact --nocapture
//! ```
//!
//! `LOGOS_COLD_START_SAMPLES` (default 8) sets how many fresh-process cold
//! starts are measured — raise it for a tighter distribution at the cost of
//! wall time (each sample is a real process launch plus a real ~500ms+ cold
//! start, so the default costs roughly `8 * 1s` end to end, more if the host
//! is exhibiting the ramp-up effect above).
//!
//! [CR-116]: ../../docs/requests/CR-116-cold-start-budget-and-its-guard-disagree.md
//! [S-368]: ../../docs/planning/journal.md#s-368-attribute-the-cold-start-cost-across-its-phases
//! [S-369]: ../../docs/planning/journal.md#s-369-reconcile-the-cold-start-budget-with-what-it-actually-bounds
//! [NFR-PE-05]: ../../docs/specs/requirements/NFR-PE-05.md

use std::process::Command;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use logos_core::Engine;

const DEFAULT_SAMPLES: usize = 8;
/// Prefix marking the one line of stdout
/// [`cold_start_phase_attribution_child_sample`] emits, so its parent can find
/// that line amid the test harness's own banner output.
const CHILD_MARKER: &str = "COLD_START_CHILD_SAMPLE: ";

fn samples() -> usize {
    std::env::var("LOGOS_COLD_START_SAMPLES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_SAMPLES)
}

/// A human-identifiable reference machine label (CR-116: "the reference
/// machine is named"). Best-effort — `uname -srm` on unix, plus the core
/// count every wall-clock budget in this suite is implicitly measured
/// against; falls back to the compiled OS/arch pair if `uname` is
/// unavailable. Never fails the measurement.
fn reference_machine() -> String {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);

    #[cfg(unix)]
    {
        if let Ok(out) = Command::new("uname").arg("-srm").output() {
            if out.status.success() {
                let uname = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !uname.is_empty() {
                    return format!("{uname} ({cores} cores)");
                }
            }
        }
    }
    format!(
        "{}-{} ({cores} cores)",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// min / median / p90 / max / mean over a sample set, in ms — the shape that
/// makes a bimodal cause visible (CR-116 CRA-02) rather than averaged away.
fn stats(mut xs: Vec<f64>) -> serde_json::Value {
    xs.sort_by(|a, b| a.partial_cmp(b).expect("no NaNs in a wall-clock sample"));
    let n = xs.len();
    let at = |q: f64| -> f64 {
        let idx = (((n - 1) as f64) * q).round() as usize;
        xs[idx.min(n - 1)]
    };
    let mean = xs.iter().sum::<f64>() / n as f64;
    serde_json::json!({
        "n": n,
        "min_ms": xs[0],
        "median_ms": at(0.5),
        "p90_ms": at(0.9),
        "max_ms": xs[n - 1],
        "mean_ms": mean,
        "samples_ms": xs,
    })
}

fn mean_of(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// One sample, measured in its own process: an uninstrumented [`Engine::start`]
/// (byte-for-byte what `cold_start_to_ready_engine_is_within_pe05_budget`
/// measures) and its phase-timed twin
/// [`Engine::start_with_phase_report`](logos_core::Engine::start_with_phase_report),
/// each over a fresh root. Printed as one JSON line so the parent
/// [`cold_start_phase_attribution`] test — which spawns this as a subprocess,
/// once per sample — can parse it back out.
///
/// Run directly (`cargo test cold_start_phase_attribution_child_sample --
/// --exact --nocapture`) it is just another passing test: a fresh process
/// launched by the test harness itself is exactly one (uncontrolled) sample.
#[test]
fn cold_start_phase_attribution_child_sample() {
    let root = TempDir::new().expect("temp root");
    let t = Instant::now();
    let engine = Engine::start(root.path()).expect("engine starts");
    let uninstrumented = t.elapsed();
    assert!(engine.runtime().is_some(), "engine is ready to serve");
    drop(engine);
    drop(root);

    let root = TempDir::new().expect("temp root");
    let t = Instant::now();
    let (engine, phases) =
        Engine::start_with_phase_report(root.path()).expect("instrumented engine starts");
    let instrumented = t.elapsed();
    assert!(
        engine.runtime().is_some(),
        "instrumented engine is ready to serve"
    );

    // AC1: the per-phase breakdown reconciles with the externally measured
    // total, within a stated margin. The margin absorbs the noise of the
    // handful of `Instant::now()` calls plus OS scheduling jitter, not a real
    // gap in the attribution — 2 ms or 2% of the measured total, whichever is
    // larger.
    let phase_sum = phases.sum();
    let margin = Duration::from_millis(2).max(instrumented.mul_f64(0.02));
    let diff = phase_sum.max(instrumented) - phase_sum.min(instrumented);
    assert!(
        diff <= margin,
        "phase sum {phase_sum:?} does not reconcile with the measured total {instrumented:?} \
         (diff {diff:?} exceeds the {margin:?} margin): {phases:?}"
    );

    let record = serde_json::json!({
        "uninstrumented_ms": ms(uninstrumented),
        "instrumented_ms": ms(instrumented),
        "phase_sum_ms": ms(phase_sum),
        "nfr_pe05_enumerated_total_ms": ms(phases.nfr_pe05_enumerated_total()),
        "phases_ms": {
            "plugin_toml_parse": ms(phases.plugin_toml_parse),
            "registry_construction": ms(phases.registry_construction),
            "query_compilation": ms(phases.query_compilation),
            "store_open": ms(phases.store_open),
            "schema_migration": ms(phases.schema_migration),
            "pool_startup": ms(phases.pool_startup),
            "other": ms(phases.other),
        },
    });
    println!(
        "{CHILD_MARKER}{}",
        serde_json::to_string(&record).expect("child sample record serialises")
    );
}

/// One process-per-sample cold-start measurement (see module docs for why).
/// Spawns [`cold_start_phase_attribution_child_sample`] as a subprocess of
/// this same test binary, once per sample, and aggregates the JSON line each
/// child prints.
#[test]
fn cold_start_phase_attribution() {
    let n = samples();
    let machine = reference_machine();
    let exe = std::env::current_exe().expect("this test binary's own path");
    eprintln!("cold-start-phase-attribution: reference machine = {machine}");
    eprintln!("cold-start-phase-attribution: samples (fresh processes) = {n}");

    let phase_names = [
        "plugin_toml_parse",
        "registry_construction",
        "query_compilation",
        "store_open",
        "schema_migration",
        "pool_startup",
        "other",
    ];

    let mut uninstrumented_ms: Vec<f64> = Vec::with_capacity(n);
    let mut instrumented_ms: Vec<f64> = Vec::with_capacity(n);
    let mut phase_sum_ms: Vec<f64> = Vec::with_capacity(n);
    let mut enumerated_ms: Vec<f64> = Vec::with_capacity(n);
    let mut per_phase_ms: Vec<Vec<f64>> = phase_names.iter().map(|_| Vec::new()).collect();
    let mut paired_overhead_ms: Vec<f64> = Vec::with_capacity(n);

    for i in 0..n {
        let output = Command::new(&exe)
            .args([
                "--exact",
                "--nocapture",
                "cold_start_phase_attribution_child_sample",
            ])
            .output()
            .unwrap_or_else(|err| panic!("spawning cold-start child sample {i}: {err}"));
        assert!(
            output.status.success(),
            "cold-start child sample {i} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| l.starts_with(CHILD_MARKER))
            .unwrap_or_else(|| panic!("child sample {i} printed no {CHILD_MARKER} line: {stdout}"));
        let record: serde_json::Value = serde_json::from_str(&line[CHILD_MARKER.len()..])
            .unwrap_or_else(|err| panic!("child sample {i} record did not parse: {err}"));

        let get = |key: &str| record[key].as_f64().expect("child record field is a number");
        let un = get("uninstrumented_ms");
        let ins = get("instrumented_ms");
        uninstrumented_ms.push(un);
        instrumented_ms.push(ins);
        phase_sum_ms.push(get("phase_sum_ms"));
        enumerated_ms.push(get("nfr_pe05_enumerated_total_ms"));
        // Paired per-child delta: both arms ran in the same process, so they
        // share whatever warm/cold state that process happened to launch
        // into. Averaging *this* isolates the instrumentation's own cost from
        // the much larger cross-process variance (CR-116 R2, R3).
        paired_overhead_ms.push(ins - un);
        for (bucket, name) in per_phase_ms.iter_mut().zip(phase_names) {
            bucket.push(record["phases_ms"][name].as_f64().expect("phase field is a number"));
        }
    }

    let overhead_mean_ms = mean_of(&paired_overhead_ms);
    let uninstrumented_mean = mean_of(&uninstrumented_ms);
    let overhead_pct = overhead_mean_ms / uninstrumented_mean * 100.0;

    let phases_json: serde_json::Map<String, serde_json::Value> = phase_names
        .iter()
        .zip(per_phase_ms)
        .map(|(name, xs)| (name.to_string(), stats(xs)))
        .collect();

    let record = serde_json::json!({
        "reference_machine": machine,
        "samples": n,
        "each_sample_is_a_fresh_process": true,
        "uninstrumented_total_ms": stats(uninstrumented_ms),
        "instrumented_total_ms": stats(instrumented_ms),
        "phase_sum_ms": stats(phase_sum_ms),
        "phases_ms": phases_json,
        // The number that decides CR-116 §3.2 branch (a) vs (b): the total
        // for ONLY the phases NFR-PE-05 currently enumerates.
        "nfr_pe05_enumerated_total_ms": stats(enumerated_ms),
        "instrumentation_overhead_ms": {
            "mean_of_paired_per_sample_deltas": overhead_mean_ms,
            "pct_of_uninstrumented_mean": overhead_pct,
        },
    });
    let line = serde_json::to_string(&record).expect("attribution record serialises");
    eprintln!("cold-start-phase-attribution: {line}");
    if let Some(out) = std::env::var_os("LOGOS_BENCH_OUT") {
        std::fs::write(&out, format!("{line}\n")).expect("write attribution record");
        eprintln!(
            "cold-start-phase-attribution: recorded to {}",
            std::path::PathBuf::from(&out).display()
        );
    }

    // A sanity floor, not a budget: each child already asserts its own phase
    // sum reconciles with its own measured total (AC1). This only catches the
    // instrumentation itself becoming pathologically expensive on average
    // (e.g. an accidental syscall per phase) — loose, because the paired
    // per-sample delta still carries some of each process's own scheduling
    // noise even though it cancels the cross-process ramp-up effect.
    assert!(
        overhead_mean_ms.abs() < 100.0,
        "the mean paired instrumented-vs-uninstrumented delta is {overhead_mean_ms:.1} ms \
         ({overhead_pct:.1}% of the {uninstrumented_mean:.1} ms uninstrumented mean) — the \
         instrumentation itself looks expensive, not just measured"
    );
}
