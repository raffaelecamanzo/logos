//! The quality readout names the scope its signal was computed over
//! (S-432, [CR-138], [FR-EH-04], [FR-QM-08]).
//!
//! `quality-report` rendered `signal n/a (empty graph)` for **both** reasons a
//! signal can be absent. A crate whose only source is `tests/only_tests.rs` was
//! told its graph was empty while `logos status` reported it populated in the
//! same breath — the reproduction in [CR-138] §2, and exactly the attribution
//! [FR-EH-04] AC2 forbids. These tests pin both arms and, above all, pin that
//! the discriminant and the metric computation classify one store the same way.
//!
//! **No exact node count is quoted here on purpose.** [CR-138] §5.1 recorded
//! `node_count: 8, edge_count: 7` from the reporter's own crate and the manual
//! quotes `9/9` from a fixture whose `Cargo.toml` carries one key more; this
//! file's fixture yields its own figure again. All are real readings of
//! different trees, so `a_test_only_crate_…` asserts `node_count > 0` and
//! compares against whatever `status` reports, rather than pinning a number
//! that is only true of one of them.
//!
//! # A new file rather than an addition to `read_only_accessors.rs`
//!
//! That file owns the [S-312] no-write invariant and is re-run here unchanged
//! rather than extended — a guard that grows a new concern each sprint stops
//! being a guard. The no-write half of this story is *its* assertion, still, and
//! `quality_readout_computes_fresh_and_writes_nothing` covers the extra read
//! this story adds without a line changing.
//!
//! [S-312]: ../../docs/planning/journal.md#s-312-session-start-quality-readout-replacing-the-cancelled-sessionend-hook

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::models::quality::{MetricSnapshot, SignalAbsence};
use logos_core::Engine;
use tempfile::TempDir;

// ── fixtures ────────────────────────────────────────────────────────────────

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// An indexed repo holding `files`, each `(relative path, contents)`.
fn repo_of(files: &[(&str, &str)]) -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let root = tmp.path();
    sh_git(root, &["init", "-q", "-b", "main"]);
    for (rel, contents) in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        sh_git(root, &["add", rel]);
    }
    sh_git(root, &["commit", "-q", "-m", "fixture"]);
    Engine::start(root).expect("engine starts").index();
    tmp
}

/// A store with **no node**: one file the indexer does not extract from, so the
/// repo is a real repo and the graph is genuinely empty.
fn empty_store() -> TempDir {
    repo_of(&[("blob.dat", "x\n")])
}

/// The [CR-138] §2 reproduction: a crate whose **only** source is a test module.
/// Every node it holds is test scope, so the production metric graph comes out
/// empty while the store is plainly populated — `indexed: true, node_count > 0`.
fn test_only_store() -> TempDir {
    repo_of(&[
        ("Cargo.toml", "[package]\nname = \"onlytests\"\nversion = \"0.1.0\"\n"),
        (
            "tests/only_tests.rs",
            "#[test]\nfn alpha() {\n    helper();\n}\n\n#[test]\nfn beta() {\n    helper();\n}\n\nfn helper() {\n    assert!(true);\n}\n",
        ),
    ])
}

/// An ordinary store with production code — the control: a signal is present and
/// no absence is claimed at all.
fn production_store() -> TempDir {
    repo_of(&[(
        "src/a.rs",
        "pub fn a(x: i64) -> i64 { if x > 0 { b() } else { 0 } }\npub fn b() -> i64 { 1 }\n",
    )])
}

/// Production **types** and no production function: `function_count == 0` while
/// the graph is not empty and does score.
///
/// The one real-store shape on which the `function_count` lookalike diverges
/// from `empty`. Without it the agreement test below cannot tell the two apart
/// on any live store — the constructed-snapshot test pins the predicate, this
/// pins that a real store reaches the divergent shape at all.
fn types_only_store() -> TempDir {
    repo_of(&[("src/t.rs", "pub struct A;\npub struct B;\npub struct C;\n")])
}

// ── the two arms ────────────────────────────────────────────────────────────

/// A genuinely empty store still says `empty graph` — this story narrows an
/// over-broad cause, it does not retire the one honest use of it.
#[test]
fn an_empty_store_still_names_an_empty_graph() {
    let tmp = empty_store();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let readout = engine.quality_readout().expect("readout");

    assert!(readout.signal.is_none(), "nothing to score: {:?}", readout.signal);
    assert_eq!(
        readout.signal_absence,
        Some(SignalAbsence::EmptyGraph),
        "no node in the store is an empty graph, and nothing else"
    );

    let payload = engine.quality_report_hook_payload().expect("payload");
    let json = serde_json::to_value(&payload).expect("serialise");
    let summary = json["systemMessage"].as_str().expect("a summary line");
    assert!(
        summary.contains("signal n/a (empty graph)"),
        "the one honest use of the phrase is unchanged: {summary}"
    );
}

/// The reproduction. A populated store whose production scope is empty names
/// **that scope** as the cause and carries the figures establishing it — and
/// never calls itself an empty graph, which is the assertion [CR-138] filed.
#[test]
fn a_test_only_crate_names_its_empty_production_scope_and_carries_the_figure() {
    let tmp = test_only_store();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // The contradiction as the user meets it: `status` says the store is
    // populated in the same breath the readout used to say it was empty.
    let status = engine.status();
    assert!(status.indexed, "the reproduction's store is indexed");
    assert!(
        status.node_count > 0,
        "and holds nodes — {} of them",
        status.node_count
    );

    let readout = engine.quality_readout().expect("readout");
    assert!(readout.signal.is_none(), "the production scope scored nothing");
    let Some(SignalAbsence::NoProductionScope {
        indexed_nodes,
        test_functions,
    }) = readout.signal_absence
    else {
        panic!(
            "a populated store with an empty production scope is not an empty graph: {:?}",
            readout.signal_absence
        );
    };
    assert_eq!(
        indexed_nodes, status.node_count,
        "the figure carried is the store's own node count — the one `logos status` prints"
    );
    assert!(
        test_functions > 0,
        "the test functions the production filter excluded are counted: {test_functions}"
    );

    let payload = engine.quality_report_hook_payload().expect("payload");
    let json = serde_json::to_value(&payload).expect("serialise");
    let summary = json["systemMessage"].as_str().expect("a summary line");
    let context = json["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("a full readout");
    for rendered in [summary, context] {
        assert!(
            !rendered.contains("empty graph"),
            "the cause this store's gating condition does NOT establish (FR-EH-04 AC2): {rendered}"
        );
        assert!(
            rendered.contains("no production code"),
            "the cause it does establish: {rendered}"
        );
        assert!(
            rendered.contains(&format!("{indexed_nodes} node(s) indexed")),
            "carrying the figure that establishes it: {rendered}"
        );
        assert!(
            rendered.contains(&format!("{test_functions} test function(s) excluded")),
            "and the figure that explains it: {rendered}"
        );
    }
}

// ── the agreement test — the one that matters ───────────────────────────────

/// **The discriminant and the metric computation classify every store the same
/// way.**
///
/// This is the test the story exists for. A discriminant that merely *happens*
/// to agree with the production scope today is the defect class being closed, so
/// agreement is asserted rather than inspected, over four stores spanning the
/// three outcomes — a signal, an empty graph, an empty production scope — plus
/// the types-only shape on which the `function_count` lookalike diverges.
///
/// `scan` is the persisting path and `quality_readout` the non-persisting one,
/// and they reach the production scope through different code — so this compares
/// the readout's verdict with one derived from the metric snapshot `scan`
/// produced over the same store, and catches **wiring** drift between them
/// (a caller passing the wrong count, the two paths disagreeing about a store).
///
/// **What it deliberately does not do**, stated because an earlier draft of
/// these notes claimed it did: the second assertion calls `classify` on both
/// sides, so it compares `classify` against itself and is structurally blind to
/// any change *inside* it — a review sweep proved this by swapping the two arms
/// and watching this test stay green while three others failed. The predicate
/// itself is pinned by `the_discriminant_gates_on_the_production_scope_not_a_lookalike`
/// below, which is why both tests exist and neither subsumes the other.
#[test]
fn the_discriminant_agrees_with_the_metrics_own_production_scope() {
    for (name, tmp) in [
        ("empty store", empty_store()),
        ("test-only store", test_only_store()),
        ("production store", production_store()),
        ("types-only store", types_only_store()),
    ] {
        let engine = Engine::start(tmp.path()).expect("engine starts");
        let scan = engine.scan(true).expect("scan");
        let status = engine.status();
        let readout = engine.quality_readout().expect("readout");

        assert_eq!(
            readout.signal.is_none(),
            readout.signal_absence.is_some(),
            "{name}: a cause is stated exactly when a signal is absent, never both and never neither"
        );
        assert_eq!(
            readout.signal_absence,
            SignalAbsence::classify(&scan.metrics, status.node_count, status.file_count),
            "{name}: the readout's discriminant and the metric snapshot's own production \
             scope classify this store differently"
        );
        assert_eq!(
            readout.signal.is_none(),
            scan.metrics.aggregate_signal.is_none(),
            "{name}: the readout withholds a signal exactly when the metric computation does"
        );
    }
}

/// The discriminant gates on the metric snapshot's **own** emptiness predicate,
/// not on a lookalike that happens to agree on ordinary stores.
///
/// Four lookalikes were available and all four are wrong: the snapshot's
/// `function_count`, its `test_function_count`, its post-filter `node_count`,
/// and — the subtlest, because it is welded to `empty` for every snapshot
/// `compute` produces — the presence of `aggregate_signal` itself. Each
/// assertion below is a snapshot shape on which one of them diverges from
/// `empty`, so an implementation reaching for it fails here rather than three
/// iterations later on someone's repo.
///
/// The last two are why this test builds `MetricSnapshot` values by hand
/// instead of indexing another fixture repo: `classify` is `pub` over an
/// arbitrary snapshot, including one rehydrated from a persisted row, so the
/// contract has to hold for shapes `compute` would never emit. A review sweep
/// confirmed the cost of omitting them — gating on `aggregate_signal.is_some()`
/// or on `node_count` passed every one of the other 23 tests.
#[test]
fn the_discriminant_gates_on_the_production_scope_not_a_lookalike() {
    // `function_count == 0`: a graph of production *types* with no production
    // function is not empty and does have a signal.
    let types_only = MetricSnapshot {
        empty: false,
        aggregate_signal: Some(8000),
        node_count: 5,
        function_count: 0,
        ..MetricSnapshot::default()
    };
    assert_eq!(
        SignalAbsence::classify(&types_only, 5, 1),
        None,
        "a scored graph has no absence to explain, whatever its function count"
    );

    // `test_function_count > 0`: an empty production scope is an empty production
    // scope even when nothing was excluded as a test. A tree of files none of
    // which parsed reaches this — the scope was emptied by something other than
    // tests, and the arm must still say so rather than claiming every symbol was
    // a test.
    let no_tests_counted = MetricSnapshot {
        empty: true,
        aggregate_signal: None,
        test_function_count: 0,
        ..MetricSnapshot::default()
    };
    assert_eq!(
        SignalAbsence::classify(&no_tests_counted, 4, 2),
        Some(SignalAbsence::NoProductionScope {
            indexed_nodes: 4,
            test_functions: 0,
        }),
        "nodes built from ingested files rule out an empty graph regardless of \
         why the scope is empty"
    );

    // `aggregate_signal`'s presence: welded to `empty` for every snapshot
    // `compute` emits, which is exactly what makes it the lookalike inspection
    // cannot separate. `classify` is public over hand-built and rehydrated
    // snapshots, so the two are pulled apart here deliberately.
    let signal_without_scope = MetricSnapshot {
        empty: true,
        aggregate_signal: Some(8000),
        node_count: 5,
        ..MetricSnapshot::default()
    };
    assert_eq!(
        SignalAbsence::classify(&signal_without_scope, 7, 1),
        Some(SignalAbsence::NoProductionScope {
            indexed_nodes: 7,
            test_functions: 0,
        }),
        "the gate is `empty`, not the presence of a signal"
    );
    let scope_without_signal = MetricSnapshot {
        empty: false,
        aggregate_signal: None,
        node_count: 0,
        ..MetricSnapshot::default()
    };
    assert_eq!(
        SignalAbsence::classify(&scope_without_signal, 7, 1),
        None,
        "the gate is `empty`, not a missing signal and not the snapshot's own node_count"
    );

    // The store's own counts are the *only* thing separating the two arms; the
    // snapshot's `node_count` is the post-filter figure and reads zero on both.
    let scope_empty = MetricSnapshot {
        empty: true,
        aggregate_signal: None,
        node_count: 0,
        test_function_count: 3,
        ..MetricSnapshot::default()
    };
    assert_eq!(
        SignalAbsence::classify(&scope_empty, 0, 1),
        Some(SignalAbsence::EmptyGraph),
        "no node in the store: an empty graph"
    );
    assert_eq!(
        SignalAbsence::classify(&scope_empty, 9, 1),
        Some(SignalAbsence::NoProductionScope {
            indexed_nodes: 9,
            test_functions: 3,
        }),
        "the same snapshot over a populated store is the other arm entirely"
    );

    // No ingested file: whatever nodes the store holds, Logos manufactured them.
    // Reproduced against a project that declares two layers and a boundary and
    // has never been indexed — `file_count: 0, node_count: 3`, every one a
    // derived `Layer`/`Boundary` vertex. Calling that "3 node(s) indexed" states
    // a value about indexing that nothing established (FR-EH-04 AC3), and routes
    // the reader to the arm that deliberately names no command, when `logos
    // index` is the step they need.
    assert_eq!(
        SignalAbsence::classify(&scope_empty, 3, 0),
        Some(SignalAbsence::EmptyGraph),
        "derived policy vertices are not evidence that any code was indexed"
    );
}

// ── the same claim, one renderer over ───────────────────────────────────────

/// **The gate verdict names no cause it cannot establish** ([S-434]'s audit,
/// [CR-138], [FR-EH-04] AC2, `models::quality::absence` R1).
///
/// `governance::gate` renders the absent signal on the *same* CLI surface as the
/// readout and reached it by a different path, so [S-432] left it behind: both
/// gate messages read *"n/a (empty graph)"* for a condition
/// (`aggregate_signal == None` on either side) that has the two causes
/// [`SignalAbsence`] separates. On the [CR-138] §2 reproduction store that is
/// the false attribution, word for word, one renderer over.
///
/// The gate cannot classify it — [`SignalAbsence`] needs the store counts the
/// gate does not read, and R2 forbids naming a cause without the figures that
/// establish it — so the correction drops the attribution rather than inventing
/// a read. The contrast below is the whole point: the readout, which *can*
/// establish the cause, still names it on a genuinely empty store.
///
/// [S-432]: ../../docs/planning/journal.md#s-432-the-quality-readout-names-the-scope-its-signal-was-computed-over
/// [S-434]: ../../docs/planning/journal.md#s-434-one-absence-taxonomy-audited-across-the-three-reporting-surfaces
#[test]
fn the_gate_verdict_names_no_cause_for_an_absent_signal() {
    for (name, tmp) in [
        ("test-only store", test_only_store()),
        ("empty store", empty_store()),
    ] {
        let engine = Engine::start(tmp.path()).expect("engine starts");

        // The threshold floor: `aggregate_signal == None` cannot satisfy one.
        let floored = engine.gate(Some(10_000), false, true).expect("gate runs");
        assert!(
            !floored.passed && floored.message.contains("cannot satisfy threshold"),
            "{name}: an absent signal cannot satisfy a floor: {}",
            floored.message
        );
        assert!(
            floored.message.contains("signal is n/a"),
            "{name}: the absence is named: {}",
            floored.message
        );
        assert!(
            !floored.message.contains("empty graph"),
            "{name}: the floor message states a cause this condition does not \
             establish (FR-EH-04 AC2): {}",
            floored.message
        );

        // The comparison verdict: a baseline exists and one side has no signal.
        //
        // BOTH renderers, because the correction's whole R5 claim is that they
        // share one constant. The persisting `gate` was pinned here first and
        // the read-only `latest_gate` was not, so a mutation replacing
        // `gate_from_snapshot`'s arm with an entirely different sentence passed
        // 43 of 43 tests — the "call site you never edited" blind spot, inside
        // the one pair this correction exists to hold together.
        engine.gate(None, true, true).expect("gate --save runs");
        let persisting = engine.gate(None, false, true).expect("gate runs");
        let read_only = engine.latest_gate().expect("latest_gate runs");
        assert_eq!(
            (persisting.message.as_str(), read_only.message.as_str()),
            (
                "signal or baseline is n/a — informational pass",
                "signal or baseline is n/a — informational pass"
            ),
            "{name}: one spelling, no cause, and BOTH gate paths render the \
             constant that holds it"
        );
    }

    // The contrast, and the reason this is a narrowing rather than a retirement:
    // the readout reads the store counts, so on a genuinely empty store it
    // *does* establish the cause and still names it.
    let tmp = empty_store();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let readout = engine.quality_readout().expect("readout");
    assert_eq!(
        readout.signal_absence,
        Some(SignalAbsence::EmptyGraph),
        "the surface that can establish the cause is untouched by this correction"
    );
}
