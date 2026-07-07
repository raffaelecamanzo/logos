//! The consolidated, **release-gating** correctness golden suite (S-025,
//! [ADR-17], [NFR-RA-06], [NFR-MA-03]).
//!
//! ADR-17 ratifies behavior-first golden tests as the *executable fitness
//! functions* for Logos's correctness invariants — symbol stability, the
//! cross-target byte-identical signal, FTS integrity, and cross-file rebind —
//! run on every target so a divergence blocks the release ([AR-03],
//! [AA-03]). This file is the single place those invariants are consolidated
//! into one canonical fixture + one committed golden, so that:
//!
//! 1. the **cross-target byte-identical signal** ([NFR-RA-06], [UAT-EX-02])
//!    is a real gate: every CI target re-derives the projection below and
//!    compares it to the *same* committed golden
//!    (`tests/golden/cross_target_signal.json`). If any target diverges by a
//!    single byte its leg fails, and the `golden-signal` CI matrix
//!    (`.github/workflows/ci.yml`) is `fail-fast: false` so a divergence on
//!    one target never masks another — a red leg fails the build, and a
//!    release is only ever cut from a green `main` ([AR-03]);
//! 2. **symbol stability across edit+sync** ([UAT-EX-02], [ADR-07]),
//!    **cross-file rebind** ([UAT-SY-02], [ADR-10]), and **FTS integrity**
//!    ([NFR-RA-09]) are asserted behaviorally against that same fixture.
//!
//! The remaining named fitness functions in the S-025 charter already live as
//! behavior-first tests and run on the same cross-target matrix, so they are
//! referenced rather than duplicated here:
//! - **stdout-safety at trace level** — `cli/tests/stdout_safety.rs` and the
//!   full `serve --mcp` proof in `mcp/tests/stdout_safety.rs` ([UAT-MC-02]);
//! - **the agent-session loop** — `mcp/tests/protocol.rs` ([UAT-CF-01]);
//! - **the dogfood (index Logos with Logos) loop** —
//!   `logos-core/tests/navigation.rs::dogfood_runs_all_eight_tools_on_logos_own_source`
//!   and `logos-core/tests/resolution.rs::dogfood_measures_resolution_accuracy_on_logos_own_source`
//!   ([UAT-CF-02]).
//!
//! Tests stay **behavior-first with no coverage floor** ([NFR-MA-03]): each
//! `#[test]` pins a behavior, not a line count.
//!
//! ## Re-blessing the golden
//!
//! When behavior legitimately changes, regenerate the committed golden with:
//!
//! ```text
//! UPDATE_GOLDEN=1 cargo test -p logos-core --test golden_signal
//! ```
//!
//! and review the diff deliberately — an un-reviewed golden churn is exactly
//! the silent regression this gate exists to catch ([ADR-17]).
//!
//! [ADR-17]: ../../docs/specs/architecture/decisions/ADR-17.md
//! [ADR-07]: ../../docs/specs/architecture/decisions/ADR-07.md
//! [ADR-10]: ../../docs/specs/architecture/decisions/ADR-10.md
//! [NFR-RA-06]: ../../docs/specs/requirements/NFR-RA-06.md
//! [NFR-RA-09]: ../../docs/specs/requirements/NFR-RA-09.md
//! [NFR-MA-03]: ../../docs/specs/requirements/NFR-MA-03.md
//! [UAT-EX-02]: ../../docs/specs/requirements/UAT-EX-02.md
//! [UAT-SY-02]: ../../docs/specs/requirements/UAT-SY-02.md
//! [UAT-MC-02]: ../../docs/specs/requirements/UAT-MC-02.md
//! [UAT-CF-01]: ../../docs/specs/requirements/UAT-CF-01.md
//! [UAT-CF-02]: ../../docs/specs/requirements/UAT-CF-02.md

#![cfg(feature = "lang-rust")]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use logos_core::metrics;
use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::{Engine, Granularity, Runtime};
use serde::Serialize;
use tempfile::TempDir;

// ── The canonical fixture ────────────────────────────────────────────────────
//
// A small, deterministic, Rust-only workspace that exercises every invariant
// the golden guards in one graph:
// - two `target` functions in sibling modules — the same-name overload case
//   ([UAT-EX-02]) that proves symbol identity disambiguates by scope, not name;
// - cross-file `Calls` edges (`caller` → `callee::target`, `caller` →
//   `twin::target`) bound by resolution itself — the rebind surface ([ADR-10]);
// - a `helper`/`unused` pair so the metric signal is a real, non-trivial value
//   and FTS has distinct names to find.
//
// No `Cargo.toml`: the index pipeline derives symbols under
// `SymbolContext::default()` (empty package/version), so the symbol strings are
// a pure function of the project-relative path + declaration — identical on
// every run and every target.

/// `(relative path, contents)` pairs for the canonical golden fixture.
const FIXTURE: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod caller;\npub mod callee;\npub mod twin;\n",
    ),
    (
        "src/caller.rs",
        "use crate::callee::target;\n\
         \n\
         pub fn caller() -> i64 {\n\
         \x20   target() + crate::twin::target()\n\
         }\n",
    ),
    (
        "src/callee.rs",
        "pub fn target() -> i64 {\n\
         \x20   helper()\n\
         }\n\
         \n\
         pub fn helper() -> i64 {\n\
         \x20   1\n\
         }\n\
         \n\
         pub fn unused() -> i64 {\n\
         \x20   0\n\
         }\n",
    ),
    ("src/twin.rs", "pub fn target() -> i64 {\n\x20   2\n}\n"),
];

/// Materialise the fixture under a fresh temp dir and return the guard.
fn build_fixture() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    for (rel, contents) in FIXTURE {
        write(dir.path(), rel, contents);
    }
    dir
}

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

// ── The deterministic signal projection ──────────────────────────────────────

/// The cross-target-stable correctness projection — everything that must be
/// byte-identical across the four CI targets ([NFR-RA-06]), and nothing that is
/// not. Excludes rowids (storage-local), timestamps, the HEAD sha, and absolute
/// paths; keys every structural fact off the content-derived [`LogosSymbol`].
#[derive(Debug, Serialize)]
struct GoldenSignal {
    /// A schema tag so a future projection change is an explicit, reviewable
    /// golden diff rather than a silent shape drift.
    schema: &'static str,
    /// The 0–10000 aggregate quality signal (ADR-08/ADR-12); `null` = the
    /// empty-graph "n/a" sentinel. The hardest cross-target invariant.
    aggregate_signal: Option<u32>,
    /// The five core metrics, raw + normalized, each rendered at the documented
    /// 1e-6 epsilon so a last-ULP libm difference between arm64/x86_64 cannot
    /// flake the gate while a real divergence still fails it ([NFR-RA-06]).
    metrics: Vec<MetricEntry>,
    /// The counts the metric run scored.
    counts: Counts,
    /// Every node, keyed by its canonical symbol — the symbol-stability and
    /// extraction signal. Sorted by symbol for a canonical byte order.
    symbols: Vec<SymbolEntry>,
    /// Every edge, both endpoints rendered as symbols — the structural / binding
    /// signal (cross-file `Calls`, `Contains`, …). Sorted for a canonical order.
    edges: Vec<EdgeEntry>,
}

#[derive(Debug, Serialize)]
struct MetricEntry {
    name: &'static str,
    raw: String,
    normalized: String,
}

#[derive(Debug, Serialize)]
struct Counts {
    node_count: u64,
    edge_count: u64,
    function_count: u64,
    test_function_count: u64,
}

#[derive(Debug, Serialize)]
struct SymbolEntry {
    symbol: String,
    kind: String,
    name: String,
    file: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
}

#[derive(Debug, Serialize)]
struct EdgeEntry {
    source: String,
    target: String,
    kind: String,
}

/// Render a float at the documented 1e-6 epsilon — a fixed-precision decimal so
/// the comparison is byte-exact yet tolerant of sub-epsilon platform noise.
fn eps(v: f64) -> String {
    format!("{v:.6}")
}

/// Compute the deterministic projection for the project rooted at `root`.
///
/// Indexes the project, hydrates the metric view, snapshots the five core
/// metrics, and reads the full node/edge sets back through the public read
/// seam — the same path `scan` takes, minus the non-deterministic freshness and
/// temporal tiers.
fn project(root: &Path) -> GoldenSignal {
    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert!(
        result.warnings.is_empty(),
        "the golden fixture must index cleanly: {:?}",
        result.warnings
    );
    project_from_engine(&engine)
}

/// The projection from an already-indexed engine (so the edit+sync fitness
/// functions can re-project after a `sync` without re-`start`ing).
fn project_from_engine(engine: &Engine) -> GoldenSignal {
    let rt = engine.runtime().expect("runtime present");
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("view hydrates");
    let (_, snap) = metrics::snapshot(rt, &view, Some("golden"), metrics::Thresholds::default())
        .expect("snapshot runs");

    let metrics = vec![
        MetricEntry {
            name: "modularity",
            raw: eps(snap.modularity.raw),
            normalized: eps(snap.modularity.normalized),
        },
        MetricEntry {
            name: "acyclicity",
            raw: eps(snap.acyclicity.raw),
            normalized: eps(snap.acyclicity.normalized),
        },
        MetricEntry {
            name: "depth",
            raw: eps(snap.depth.raw),
            normalized: eps(snap.depth.normalized),
        },
        MetricEntry {
            name: "equality",
            raw: eps(snap.equality.raw),
            normalized: eps(snap.equality.normalized),
        },
        MetricEntry {
            name: "redundancy",
            raw: eps(snap.redundancy.raw),
            normalized: eps(snap.redundancy.normalized),
        },
    ];

    // Read every node and edge once, building the rowid→symbol map that lets us
    // render edges in the portable symbol space.
    let (mut symbols, symbol_of) = read_symbols(rt);
    let mut edges = read_edges(rt, &symbol_of);

    symbols.sort_by(|a, b| a.symbol.cmp(&b.symbol).then(a.kind.cmp(&b.kind)));
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then(a.target.cmp(&b.target))
            .then(a.kind.cmp(&b.kind))
    });

    GoldenSignal {
        schema: "logos.golden-signal/v1",
        aggregate_signal: snap.aggregate_signal,
        metrics,
        counts: Counts {
            node_count: snap.node_count,
            edge_count: snap.edge_count,
            function_count: snap.function_count,
            test_function_count: snap.test_function_count,
        },
        symbols,
        edges,
    }
}

/// Read every node as a [`SymbolEntry`] and the rowid→symbol map.
fn read_symbols(rt: &Runtime) -> (Vec<SymbolEntry>, HashMap<NodeId, String>) {
    let nodes = rt
        .submit_read(|store| store.all_nodes())
        .expect("read runs");
    let mut entries = Vec::with_capacity(nodes.len());
    let mut by_id = HashMap::with_capacity(nodes.len());
    for n in nodes {
        let sym = n.symbol.as_str().to_string();
        by_id.insert(n.id, sym.clone());
        entries.push(SymbolEntry {
            symbol: sym,
            kind: format!("{:?}", n.kind),
            name: n.name,
            file: n.file_path,
            start_line: n.start_line,
            end_line: n.end_line,
        });
    }
    (entries, by_id)
}

/// Read every edge, rendering both endpoints in the portable symbol space.
fn read_edges(rt: &Runtime, symbol_of: &HashMap<NodeId, String>) -> Vec<EdgeEntry> {
    let raw = rt
        .submit_read(|store| store.all_edges())
        .expect("read runs");
    raw.into_iter()
        .map(|e| EdgeEntry {
            source: symbol_of
                .get(&e.source)
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{}>", e.source)),
            target: symbol_of
                .get(&e.target)
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{}>", e.target)),
            kind: format!("{:?}", e.kind),
        })
        .collect()
}

/// Serialise a projection to canonical pretty JSON with a trailing newline.
fn to_golden_json(signal: &GoldenSignal) -> String {
    let mut s = serde_json::to_string_pretty(signal).expect("projection serialises");
    s.push('\n');
    s
}

/// The committed golden's path, relative to this crate.
fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cross_target_signal.json")
}

// ── Fitness function 1: the cross-target byte-identical signal ────────────────

/// The release gate ([NFR-RA-06], [AR-03], [UAT-EX-02]): the deterministic
/// signal projection over the canonical fixture is **byte-identical** to the
/// committed golden. Run on every CI target, this asserts the four targets
/// agree on the signal to the byte — a divergence fails the leg and the build.
///
/// Re-bless deliberately with `UPDATE_GOLDEN=1` (see the module docs).
#[test]
fn cross_target_signal_is_byte_identical_to_the_committed_golden() {
    let dir = build_fixture();
    let signal = project(dir.path());
    let actual = to_golden_json(&signal);

    let path = golden_path();

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).expect("golden dir");
        fs::write(&path, &actual).expect("write golden");
        eprintln!("re-blessed golden at {}", path.display());
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "missing golden at {} ({e}); bless it with \
             UPDATE_GOLDEN=1 cargo test -p logos-core --test golden_signal",
            path.display()
        )
    });

    assert_eq!(
        actual, expected,
        "cross-target signal diverged from the committed golden ([NFR-RA-06], \
         [AR-03]). If this is an intended behavior change, re-bless with \
         UPDATE_GOLDEN=1 and review the diff; otherwise a target has produced a \
         non-identical signal and the release must not ship."
    );
}

/// The projection is itself stable run-to-run on a single target — the
/// determinism the cross-target golden relies on ([NFR-RA-06]). A flaky
/// projection would make the golden a coin-flip, so pin it independently.
#[test]
fn the_projection_is_deterministic_across_repeated_indexes() {
    let a = to_golden_json(&project(build_fixture().path()));
    let b = to_golden_json(&project(build_fixture().path()));
    assert_eq!(a, b, "two fresh indexes of the same fixture must agree");
}

// ── Fitness function 2: symbol stability across edit+sync ─────────────────────

/// Symbol IDs are content-derived, so an edit to one file must not churn the
/// symbols of *other* files, and the edited file's surviving declarations keep
/// their identities ([UAT-EX-02], [ADR-07], [NFR-RA-06]). Editing `twin.rs`
/// (a whitespace/line shift that reassigns rowids) leaves `caller.rs` and
/// `callee.rs` symbols byte-identical and keeps `twin::target`'s symbol stable.
#[test]
fn symbol_ids_are_stable_across_edit_and_sync() {
    let dir = build_fixture();
    let engine = Engine::start(dir.path()).expect("engine starts");
    engine.index();

    let before = symbols_by_file(&engine);

    // A leading comment shifts every line in twin.rs and forces a re-extract
    // (delete + reinsert → fresh rowids) without changing any declaration.
    write(
        dir.path(),
        "src/twin.rs",
        "// a comment shifts the lines below\npub fn target() -> i64 {\n\x20   2\n}\n",
    );
    let result = engine.sync(&[PathBuf::from("src/twin.rs")]);
    assert_eq!(result.files_modified, 1, "the edited file re-synced");

    let after = symbols_by_file(&engine);

    // Unrelated files are byte-identical in their symbol sets.
    for file in ["src/caller.rs", "src/callee.rs", "src/lib.rs"] {
        assert_eq!(
            before.get(file),
            after.get(file),
            "editing twin.rs churned the symbols of unrelated file {file} \
             (NFR-RA-06 / ADR-07 violated)"
        );
    }

    // twin::target's symbol survives its own file's re-extract (the rowid moved
    // but the content-derived identity did not).
    let twin_target = "`twin.rs`/target().";
    assert!(
        after
            .get("src/twin.rs")
            .expect("twin.rs has symbols")
            .iter()
            .any(|s| s.ends_with(twin_target)),
        "twin::target's symbol did not survive the edit+sync: {:?}",
        after.get("src/twin.rs")
    );
}

/// The set of canonical symbol strings grouped by defining file.
fn symbols_by_file(engine: &Engine) -> HashMap<String, Vec<String>> {
    let rt = engine.runtime().expect("runtime present");
    let (entries, _) = read_symbols(rt);
    let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
    for e in entries {
        if let Some(file) = e.file {
            by_file.entry(file).or_default().push(e.symbol);
        }
    }
    for v in by_file.values_mut() {
        v.sort();
    }
    by_file
}

// ── Fitness function 3: cross-file rebind through a sync ──────────────────────

/// A *resolved* cross-file call edge rebinds to the callee's new rowid when the
/// callee's file is edited and synced ([UAT-SY-02], [ADR-10]). The edge is
/// rendered in the portable symbol space, so "the edge survives" means the
/// caller→callee::target `Calls` relationship is still present after the rowids
/// underneath it have moved.
#[test]
fn cross_file_call_edge_rebinds_through_sync() {
    let dir = build_fixture();
    let engine = Engine::start(dir.path()).expect("engine starts");
    engine.index();

    // The portable symbol carries a scheme/package prefix and backtick-escapes
    // each dotted path segment (``src/`caller.rs`/caller().``); match on the
    // path-and-descriptor suffix so the assertion is independent of the prefix.
    let caller = "`caller.rs`/caller().";
    let callee_target = "`callee.rs`/target().";

    assert!(
        has_calls_edge(&engine, caller, callee_target),
        "resolution bound the cross-file call at index time"
    );

    // Edit the callee's file (a comment shift → re-extract reassigns target a
    // fresh rowid) and sync it.
    write(
        dir.path(),
        "src/callee.rs",
        "// shift everything down a line\n\
         pub fn target() -> i64 {\n\
         \x20   helper()\n\
         }\n\
         \n\
         pub fn helper() -> i64 {\n\
         \x20   1\n\
         }\n\
         \n\
         pub fn unused() -> i64 {\n\
         \x20   0\n\
         }\n",
    );
    let result = engine.sync(&[PathBuf::from("src/callee.rs")]);
    assert_eq!(result.files_modified, 1, "the callee file re-synced");

    assert!(
        has_calls_edge(&engine, caller, callee_target),
        "the cross-file call edge rebound to the callee's new node through \
         resolution after the sync (ADR-10 / UAT-SY-02)"
    );
}

/// Whether a `Calls` edge exists whose endpoints' symbols end with the given
/// path-and-descriptor suffixes (prefix-independent — see the caller).
fn has_calls_edge(engine: &Engine, source_suffix: &str, target_suffix: &str) -> bool {
    let rt = engine.runtime().expect("runtime present");
    let (_, symbol_of) = read_symbols(rt);
    let calls = format!("{:?}", EdgeKind::Calls);
    read_edges(rt, &symbol_of).into_iter().any(|e| {
        e.kind == calls && e.source.ends_with(source_suffix) && e.target.ends_with(target_suffix)
    })
}

// ── Fitness function 4: FTS integrity across edit+sync ────────────────────────

/// The full-text index stays consistent with the node content table across an
/// edit+sync ([NFR-RA-09], [NFR-MA-03]): a rename must make the new name
/// findable and the old name un-findable — proof the inverted index was updated
/// in lockstep (no stale row from a missed delete trigger, no missing row from a
/// missed insert).
#[test]
fn fts_search_reflects_renames_after_edit_and_sync() {
    let dir = build_fixture();
    let engine = Engine::start(dir.path()).expect("engine starts");
    engine.index();

    assert!(fts_finds(&engine, "unused"), "the original name is indexed");
    assert!(
        !fts_finds(&engine, "retired"),
        "the post-rename name is not present before the edit"
    );

    // Rename `unused` → `retired` in callee.rs and sync.
    write(
        dir.path(),
        "src/callee.rs",
        "pub fn target() -> i64 {\n\
         \x20   helper()\n\
         }\n\
         \n\
         pub fn helper() -> i64 {\n\
         \x20   1\n\
         }\n\
         \n\
         pub fn retired() -> i64 {\n\
         \x20   0\n\
         }\n",
    );
    let result = engine.sync(&[PathBuf::from("src/callee.rs")]);
    assert_eq!(result.files_modified, 1, "the renamed file re-synced");

    assert!(
        fts_finds(&engine, "retired"),
        "the FTS index picked up the new name (NFR-RA-09)"
    );
    assert!(
        !fts_finds(&engine, "unused"),
        "the FTS index dropped the old name — no stale inverted-index row \
         (NFR-RA-09)"
    );
}

/// Whether an FTS search for `name` returns a function node of exactly that
/// name through the public read seam.
fn fts_finds(engine: &Engine, name: &str) -> bool {
    let rt = engine.runtime().expect("runtime present");
    let owned = name.to_string();
    rt.submit_read(move |store| {
        let rows = store.search(&owned, Some(NodeKind::Function), 16)?;
        Ok(rows.into_iter().any(|r| r.name == owned))
    })
    .expect("read runs")
}
