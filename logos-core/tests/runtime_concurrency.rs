//! Black-box integration tests for the core execution runtime (S-008,
//! [execution-runtime], [ADR-02]/[ADR-03]/[ADR-04]).
//!
//! Where `runtime::tests` drives the `Runtime` directly, these exercise the
//! story's acceptance criteria through the public `Engine` façade exactly as the
//! CLI/MCP surfaces will: start a long-lived engine, submit reads and writes
//! through its runtime, and assert serialization, rollback, and the cold-start
//! budget.
//!
//! [execution-runtime]: ../../docs/specs/architecture/components/execution-runtime.md
//! [ADR-02]: ../../docs/specs/architecture/decisions/ADR-02.md
//! [ADR-03]: ../../docs/specs/architecture/decisions/ADR-03.md
//! [ADR-04]: ../../docs/specs/architecture/decisions/ADR-04.md

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tempfile::TempDir;

use logos_core::graph_store::{BatchWriter, NewNode};
use logos_core::model::{LogosSymbol, NodeKind};
use logos_core::Engine;

/// Insert one `function` node named `name` inside a write batch.
fn insert_function(w: &BatchWriter<'_>, symbol: &str, name: &str) -> Result<()> {
    let sym = LogosSymbol::parse(symbol)?;
    let symbol_id = w.upsert_symbol(&sym)?;
    w.insert_node(&NewNode::plain(symbol_id, NodeKind::Function, name))?;
    Ok(())
}

#[test]
fn start_brings_up_a_ready_long_lived_engine() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");

    // The canonical store was created under `.logos/`.
    let db = root.path().join(".logos").join("logos.db");
    assert!(db.exists(), "Engine::start creates .logos/logos.db");

    // The runtime is live and held for reuse across calls (ADR-04).
    assert!(
        engine.runtime().is_some(),
        "a started engine holds a runtime"
    );
    assert!(
        engine.runtime().unwrap().reader_pool_size() >= 1,
        "the read pool has at least one connection"
    );

    // The derived registry cache is built at startup and held — feature-agnostic
    // structural check (the Rust-grammar specifics are asserted separately under
    // `lang-rust`). With no grammar features the registry is an empty-but-present
    // cache, which is still the ADR-04 "built once, held" contract.
    assert!(
        engine.registry().is_some(),
        "a started engine caches the plugin registry as derived state (ADR-04)"
    );
}

#[cfg(feature = "lang-rust")]
#[test]
fn start_caches_the_plugin_registry_as_derived_state() {
    // ADR-04: the registry is built once at startup and held — a derived cache
    // reused across calls rather than rebuilt per operation.
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let registry = engine.registry().expect("registry cached at startup");
    assert!(
        registry.iter().any(|p| p.name() == "rust"),
        "the cached registry lists the compiled-in Rust grammar"
    );
}

#[test]
fn write_then_read_through_the_engine_runtime() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let runtime = engine.runtime().expect("runtime present");

    runtime
        .submit_write(|w| insert_function(w, "local e2e", "e2e_symbol"))
        .expect("write commits through the writer actor");

    let hits = runtime
        .submit_read(|store| Ok(store.search("e2e_symbol", None, 10)?.len()))
        .expect("read runs on the RO pool");
    assert_eq!(hits, 1, "the committed write is visible to the read pool");
}

#[test]
fn engine_survives_a_failed_write_batch_and_stays_consistent() {
    let root = TempDir::new().expect("temp root");
    let engine = Engine::start(root.path()).expect("engine starts");
    let runtime = engine.runtime().expect("runtime present");

    // A batch that errors after a partial write must roll back wholesale
    // (NFR-RA-07) and leave the engine usable.
    let failed: Result<()> = runtime.submit_write(|w| {
        insert_function(w, "local rollback", "rolled_back")?;
        Err(anyhow!("induced batch failure"))
    });
    assert!(failed.is_err());

    let leaked = runtime
        .submit_read(|store| Ok(store.search("rolled_back", None, 10)?.len()))
        .expect("read after rollback");
    assert_eq!(leaked, 0, "the rolled-back node never landed");

    // Engine remains consistent: the next write commits and reads back.
    runtime
        .submit_write(|w| insert_function(w, "local ok", "committed_ok"))
        .expect("the engine keeps serving writes after a rollback");
    let ok = runtime
        .submit_read(|store| Ok(store.search("committed_ok", None, 10)?.len()))
        .expect("read after recovery");
    assert_eq!(ok, 1);
}

#[test]
fn cold_start_to_ready_engine_is_within_pe05_budget() {
    // NFR-PE-05: cold start completes in ≤ 600 ms in total before serving the
    // first request. This measures the *whole* ready-engine path through the
    // public façade — all six phases the requirement enumerates (plugin.toml
    // parse, registry construction, query compilation, store open, schema
    // migration, pool startup) plus the incidental root/`.logos`/seed work
    // around them, which the requirement's total bound also covers. That is
    // the point: requirement and guard enumerate the SAME phases, so neither
    // can be satisfied by a number the other did not produce (CR-116 §3.2).
    //
    // Budget history — each value stands, none is deleted (CR-084 §6 record
    // discipline):
    //   200 → 500 ms, 2026-06-14 (CR-009): the grammar set grew 5 → 12
    //     compiled-in code languages and cold start scales ≈ linearly with it.
    //   500 → 600 ms, 2026-09-08 (CR-116 §9, S-369): the ENUMERATION grew, not
    //     the cost. S-368 measured eight fresh-process cold starts on the
    //     reference machine: the three phases NFR-PE-05 then enumerated came in
    //     at mean 440.1 / max 457.5 ms — inside 500 ms in every sample, so the
    //     old target was never breached on its own terms. This guard, though,
    //     always timed all of Engine::start: mean 506.9 / p90 528.4 / max
    //     539.8 ms. The ~67 ms difference is store open + schema migration +
    //     pool startup + incidental work — real wait the requirement had never
    //     claimed. A user waiting for a ready engine waits for the store too,
    //     so the requirement was amended to bound the whole wait and the
    //     budget re-derived from the MEASURED full total: 600 ms leaves 71.6 ms
    //     over the p90 of 528.4 and 60.2 ms over the observed max of 539.8 —
    //     11.9% and 10.0% of the budget respectively. (CR-116 §9 quotes "~11%
    //     headroom"; that is the same margin stated as a fraction of the
    //     measured figure rather than of the budget. Both denominators appear
    //     in the record, so this comment names the one it uses.) This guard
    //     needed no rescoping — it already measured exactly the amended
    //     enumeration; only its literal moved.
    //
    // The bound is tolerance-banded via LOGOS_PERF_TOLERANCE so a loaded CI
    // host can widen it without editing the budget (a breach is re-run in
    // isolation first). S-369 set the default at 1.0 and recorded that widening
    // it was explicitly not an available outcome *then* — the headroom above was
    // measured at 1.0 and that derivation still stands.
    //
    // DEFAULT WIDENED TO 1.15 on 2026-09-21 (Sprint 74 human review), which is a
    // later decision about the guard, not a revision of S-369's derivation. The
    // budget itself is UNCHANGED at 600 ms. Evidence: this assertion failed at
    // 632.6 ms during Sprint 74's full gate, and at 604.3 / 640.9 ms on re-runs,
    // while an 8-fresh-process phase attribution on the same machine gave
    // min 560.4 / median 563.7 / p90 578.4 / max 598.6 / mean 569.1 ms — every
    // sample passing at 1.0, with query_compilation 85% of the cost and no phase
    // regressed. The machine ran ~12% slower than the recorded S-368 baseline
    // under memory pressure, leaving ~5% headroom, so a single-sample assertion
    // sat inside the noise band and tipped on an unlucky draw.
    //
    // 1.15 scales the band to 690 ms: it covers the worst observed sample
    // (640.9 ms = 1.068x) with ~7% margin, and still fails anything approaching a
    // real regression — the S-368 baseline mean is 506.9 ms, so a doubling lands
    // far outside. Set LOGOS_PERF_TOLERANCE=1.0 to measure against the raw budget.
    //
    // The per-phase distribution behind those figures is re-runnable:
    //   cargo test -p logos-core --test cold_start_phase_attribution \
    //       cold_start_phase_attribution -- --exact --nocapture
    //
    // MEASURED IN A FRESH CHILD PROCESS since 2026-09-25 (HF-3). Compiled
    // queries are now shared process-wide, so an `Engine::start` timed after a
    // sibling test in this binary had loaded a registry would time cache hits
    // and could no longer see a query-compilation regression. A fresh process
    // is the cold start NFR-PE-05 bounds — every CLI invocation is one. The
    // budget and the tolerance are unchanged.
    //
    // EMBEDDED QUERIES LEFT THE COLD PATH on 2026-10-05 (CR-197, S-600): each
    // language now compiles on its first extraction, and a start compiles only
    // override queries. Eight fresh-process attributions per arm, run
    // before/after/after/before on one debug build (host load ~13-14 on 12
    // cores), gave query_compilation medians of 622.7-623.2 ms before and
    // 0.2 ms after, and uninstrumented total medians of 710.2-710.4 ms before
    // and 78.7-79.5 ms after. On the same runs this guard measured 720.7 and
    // 727.7 ms before (failing) and passed both times after. The budget and the
    // tolerance are unchanged. The deferred cost, every language's first-use
    // compile summed (~622-626 ms debug), is reported by
    // `cold_start_phase_attribution` as `first_use_compile_ms`, where the user
    // now pays it.
    let elapsed = cold_start_in_a_fresh_process();

    let budget = Duration::from_millis(600).mul_f64(perf_tolerance());
    assert!(
        elapsed < budget,
        "cold start to a ready Engine took {elapsed:?}, over the NFR-PE-05 ≤600ms total budget \
         (tolerance-scaled to {budget:?}); re-run cold_start_phase_attribution to see which \
         phase moved"
    );
}

/// Marker on the one stdout line [`pe05_budget_cold_start_child_sample`]
/// prints, so its parent can find it amid the harness's own output. Matched
/// anywhere in a line, not as a prefix: a single-threaded harness
/// (`RUST_TEST_THREADS=1`, which the child inherits) prints `test <name> ... `
/// on the same line before the test body runs.
const COLD_START_MARKER: &str = "PE05_COLD_START_NANOS: ";

/// Spawn [`pe05_budget_cold_start_child_sample`] as a subprocess of this test
/// binary and return the cold start it measured — the same shape as
/// `cold_start_phase_attribution`'s per-sample child.
fn cold_start_in_a_fresh_process() -> Duration {
    let exe = std::env::current_exe().expect("this test binary's own path");
    let output = std::process::Command::new(exe)
        .args([
            "--exact",
            "--nocapture",
            "pe05_budget_cold_start_child_sample",
        ])
        .output()
        .expect("spawning the cold-start child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the cold-start child failed: stdout={stdout} stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let nanos = stdout
        .lines()
        .find_map(|l| l.split_once(COLD_START_MARKER).map(|(_, nanos)| nanos))
        .unwrap_or_else(|| {
            panic!("the cold-start child printed no {COLD_START_MARKER} line: {stdout}")
        })
        .trim()
        .parse::<u64>()
        .expect("the child's measurement is a whole number of nanoseconds");
    Duration::from_nanos(nanos)
}

/// The measured half of [`cold_start_to_ready_engine_is_within_pe05_budget`]:
/// one `Engine::start` to a ready engine, timed in whatever process runs it.
/// The guard spawns it so that process is fresh; run directly it is just
/// another passing test. The budget is the parent's to assert, not this one's.
#[test]
fn pe05_budget_cold_start_child_sample() {
    let root = TempDir::new().expect("temp root");

    let start = Instant::now();
    let engine = Engine::start(root.path()).expect("engine starts");
    let elapsed = start.elapsed();

    assert!(engine.runtime().is_some(), "engine is ready to serve");
    println!("{COLD_START_MARKER}{}", elapsed.as_nanos());
}

/// Multiplier applied to every wall-clock budget so a loaded CI host can widen
/// the bands without editing the budget itself (S-024). `LOGOS_PERF_TOLERANCE`
/// defaults to `1.15` since 2026-09-21 (see the call site for the measurement
/// that set it); a breach is re-run in isolation before being treated as a
/// regression. Values below `1.0` are ignored so a budget is never tightened by
/// accident; set `LOGOS_PERF_TOLERANCE=1.0` to measure against the raw budget.
fn perf_tolerance() -> f64 {
    std::env::var("LOGOS_PERF_TOLERANCE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v >= 1.0)
        .unwrap_or(1.15)
}

/// Per language, a few files that call across one another — several files per
/// language so the extraction workers reach each language's first use at once.
const MULTI_LANGUAGE_FIXTURE: &[(&str, &str)] = &[
    ("src/lib.rs", "mod a;\nmod b;\npub fn root() -> u32 { a::one() + b::two() }\n"),
    ("src/a.rs", "pub fn one() -> u32 { crate::b::two() - 1 }\n"),
    ("src/b.rs", "pub fn two() -> u32 { 2 }\npub struct Pair { pub left: u32 }\n"),
    ("src/c.rs", "use crate::b::Pair;\npub fn make() -> Pair { Pair { left: 1 } }\n"),
    ("py/app.py", "from py.util import helper\n\ndef run(x):\n    return helper(x)\n"),
    ("py/util.py", "def helper(x):\n    return x + 1\n\nclass Box:\n    def get(self):\n        return helper(1)\n"),
    ("py/more.py", "from py.util import Box\n\ndef open_box():\n    return Box().get()\n"),
    ("py/last.py", "def last():\n    return 3\n"),
    ("ts/main.ts", "import { greet } from './greet';\nexport function main(): string { return greet('a'); }\n"),
    ("ts/greet.ts", "export function greet(n: string): string { return `hi ${n}`; }\n"),
    ("ts/shape.ts", "export class Shape { area(): number { return 1; } }\n"),
    ("ts/use.ts", "import { Shape } from './shape';\nexport const a = new Shape().area();\n"),
    ("go/main.go", "package main\n\nfunc main() { helper() }\n"),
    ("go/helper.go", "package main\n\nfunc helper() int { return other() }\n"),
    ("go/other.go", "package main\n\nfunc other() int { return 1 }\n"),
    ("java/App.java", "package app;\n\npublic class App { int run() { return new Util().one(); } }\n"),
    ("java/Util.java", "package app;\n\npublic class Util { int one() { return 1; } }\n"),
    ("java/More.java", "package app;\n\npublic class More extends Util { int two() { return one() + 1; } }\n"),
];

/// Every node, edge and ledger row of `runtime`'s graph as sorted lines, edge
/// endpoints by symbol rather than store-local rowid — the same sections
/// `indexing.rs`'s `graph_fingerprint` compares for sync ≡ reindex.
fn graph_lines(runtime: &logos_core::Runtime) -> String {
    runtime
        .submit_read(|store| {
            let nodes = store.all_nodes()?;
            let symbol_of: std::collections::BTreeMap<i64, String> = nodes
                .iter()
                .map(|n| (n.id.0, n.symbol.as_str().to_string()))
                .collect();
            let key = |id: i64| symbol_of.get(&id).cloned().unwrap_or_default();
            let mut lines: Vec<String> = nodes
                .iter()
                .map(|n| {
                    format!(
                        "N {}|{:?}|{}|{}|{:?}|{:?}",
                        n.symbol.as_str(),
                        n.kind,
                        n.name,
                        n.file_path.as_deref().unwrap_or(""),
                        n.start_line,
                        n.end_line
                    )
                })
                .collect();
            lines.extend(store.all_edges()?.iter().map(|e| {
                format!(
                    "E {} -> {} [{:?}]",
                    key(e.source.0),
                    key(e.target.0),
                    e.kind
                )
            }));
            lines.extend(store.unresolved_refs()?.iter().map(|r| {
                format!(
                    "R {}|{}|{:?}|{:?}|{}|{:?}",
                    r.source_symbol, r.target, r.form, r.kind, r.resolved, r.payload
                )
            }));
            lines.sort();
            Ok(lines.join("\n"))
        })
        .expect("graph read runs")
}

/// Index [`MULTI_LANGUAGE_FIXTURE`] in a fresh root from a cold process cache,
/// compiling every language's queries before the index when `eager` — what
/// engine start did before CR-197 — or leaving each to its first use.
fn index_the_fixture(eager: bool) -> String {
    let root = TempDir::new().expect("temp root");
    for (rel, body) in MULTI_LANGUAGE_FIXTURE {
        let path = root.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }
    logos_core::plugin::queries::clear_compiled_cache();
    let engine = Engine::start(root.path()).expect("engine starts");
    if eager {
        let registry = engine.registry().expect("the registry loads");
        registry
            .compile_all_queries()
            .expect("every query compiles");
    }
    let indexed = engine.index();
    assert_eq!(
        indexed.files_indexed,
        MULTI_LANGUAGE_FIXTURE.len() as u64,
        "the whole fixture indexed: {indexed:?}"
    );
    graph_lines(engine.runtime().expect("a started engine"))
}

/// CR-197: queries compiled on each language's first use — reached by several
/// extraction workers at once — extract the very graph that compiling every
/// query up front does, and each language used reports its compile once.
///
/// Clears the process-wide query cache, so the lazy arm genuinely compiles;
/// no other test in this binary depends on the cache's contents.
#[test]
fn queries_compiled_on_first_use_extract_the_same_graph_as_compiling_them_up_front() {
    let eager = index_the_fixture(true);
    let lazy = index_the_fixture(false);
    let reports = logos_core::plugin::queries::first_use_compiles();
    assert!(
        reports.iter().all(|r| r.elapsed > Duration::ZERO),
        "every first-use report carries its compile time: {reports:?}"
    );
    let reported: Vec<String> = reports.into_iter().map(|r| r.language).collect();

    assert!(
        eager.lines().filter(|l| l.starts_with("E ")).count() > 0,
        "the fixture has edges"
    );
    let first_difference = eager.lines().zip(lazy.lines()).find(|(e, l)| e != l);
    assert!(
        eager == lazy,
        "first-use compilation changed the extracted graph; first difference \
         (eager, lazy): {first_difference:?}"
    );
    for language in ["rust", "python", "typescript", "go", "java"] {
        assert_eq!(
            reported.iter().filter(|r| *r == language).count(),
            1,
            "{language} reports its first-use compile once: {reported:?}"
        );
    }
}
