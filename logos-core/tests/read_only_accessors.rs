//! Read-only read-model accessor contract (S-082, CR-018, ADR-28, FR-UI-03).
//!
//! The web dashboard's Health/Overview/Metrics/Hotspots/Commits views read the
//! engine through the `latest_*` accessors so a page GET reflects the **last
//! persisted** snapshot/mine and never triggers an evaluate-and-persist write.
//! These tests prove that no-write invariant end-to-end through the [`Engine`]
//! façade over a real indexed + scanned + mined git fixture: repeated `latest_*`
//! calls leave the `metric_snapshots` and `temporal_snapshots` row counts
//! byte-for-byte unchanged, the read-only figures match the persisting paths',
//! and the CLI/MCP `scan`/`hotspots`/`temporal_report` paths still persist
//! (byte-unchanged behaviour, [ADR-28] consequence note).

#![cfg(feature = "lang-rust")]

use std::path::Path;
use std::process::Command;

use logos_core::models::quality::TestGapsReport;
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

// ── git fixture helpers (mirroring tests/hotspots.rs conventions) ────────────

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

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A small **indexed** (not yet scanned/mined) git repo: one branchy function
/// and one churny file with a defect-matching commit subject, so the metric and
/// temporal tiers both have something honest to report.
fn indexed_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(
        repo,
        "src/a.rs",
        "pub fn a(x: i64) -> i64 { if x > 0 { 1 } else { 0 } }\n",
        "add a",
    );
    for n in 0..3 {
        commit(
            repo,
            "src/b.rs",
            &format!("pub fn b() -> i64 {{ {n} }}\n"),
            &format!("fix: b v{n}"),
        );
    }
    Engine::start(repo).expect("engine starts").index();
    tmp
}

/// Count `metric_snapshots` rows by opening `logos.db` read-only — the
/// FR-UI-03/CR-018 snapshot-count invariant the read-only accessors must hold.
fn metric_snapshot_count(repo: &Path) -> i64 {
    let conn = Connection::open_with_flags(
        repo.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only");
    conn.query_row("SELECT count(*) FROM metric_snapshots", [], |r| r.get(0))
        .expect("count metric_snapshots")
}

/// Count `temporal_snapshots` rows; `0` when `history.db` has not been created.
fn temporal_snapshot_count(repo: &Path) -> i64 {
    let db = repo.join(".logos/history.db");
    if !db.exists() {
        return 0;
    }
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open history.db read-only");
    conn.query_row("SELECT count(*) FROM temporal_snapshots", [], |r| r.get(0))
        .expect("count temporal_snapshots")
}

// ── metric side: latest_metrics / latest_scan / latest_gate ──────────────────

/// On a never-`scan`-ned store the read-only accessors honestly report "no
/// snapshot" — `latest_metrics` is `None`, `latest_scan` carries the empty
/// sentinel, `latest_gate` is an informational pass naming the producing command
/// — and **reading them writes no snapshot** ([NFR-CC-04], [ADR-28]).
#[test]
fn never_scanned_store_reads_empty_and_writes_nothing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    assert!(
        engine.latest_metrics().unwrap().is_none(),
        "a never-scanned store has no persisted snapshot"
    );
    let scan = engine.latest_scan().unwrap();
    assert!(scan.metrics.empty, "no snapshot → the empty sentinel, never zeros");
    assert!(scan.signal.is_none(), "no fabricated signal");
    let gate = engine.latest_gate().unwrap();
    assert!(gate.signal.is_none());
    assert!(gate.passed, "no snapshot cannot regress — informational pass");
    assert!(
        gate.message.contains("scan"),
        "the verdict names the producing command: {}",
        gate.message
    );

    // The temporal read-only twins on a never-mined store: a headless `n/a`
    // report and an empty board, computed without mining or persisting
    // ([NFR-RA-05], [ADR-28]).
    let temporal = engine.latest_temporal_report().unwrap();
    assert!(temporal.head_sha.is_none(), "never-mined → headless n/a report");
    assert!(temporal.files.is_empty(), "no in-window facts → no files, never fabricated");
    assert!(
        engine.latest_hotspots(Some(50), false).unwrap().files.is_empty(),
        "never-mined → empty hotspot board"
    );

    assert_eq!(
        metric_snapshot_count(tmp.path()),
        0,
        "reading the read-only metric accessors created no snapshot"
    );
    assert_eq!(
        temporal_snapshot_count(tmp.path()),
        0,
        "reading the read-only temporal accessors mined and appended nothing"
    );
}

/// `latest_metrics` returns the last persisted snapshot's full breakdown, its
/// figures trace to that snapshot, and calling the read-only accessors
/// repeatedly leaves the `metric_snapshots` count unchanged ([FR-UI-03] AC,
/// [ADR-28]).
#[test]
fn latest_metrics_reflects_last_snapshot_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let scanned = engine.scan(false).expect("scan persists one snapshot");
    let before = metric_snapshot_count(tmp.path());
    assert_eq!(before, 1, "scan wrote exactly one snapshot");

    let latest = engine
        .latest_metrics()
        .unwrap()
        .expect("a snapshot now exists");
    // Every figure traces to the persisted snapshot ([NFR-RA-05]): the read-only
    // breakdown is byte-for-byte the fresh-computed one the scan persisted. This
    // also guards the 29-column reader against any SELECT column-offset bug — a
    // swapped dimension would diverge here.
    assert_eq!(
        serde_json::to_string(&latest).unwrap(),
        serde_json::to_string(&scanned.metrics).unwrap(),
        "latest_metrics reconstructs the persisted snapshot exactly"
    );
    // The Cohesion/Focus applicability drop-out is preserved, never fabricated
    // as a zero ([ADR-21], [NFR-CC-04]): a class-less Rust repo drops both.
    assert_eq!(
        latest.cohesion.is_none(),
        scanned.metrics.cohesion.is_none(),
        "cohesion drop-out preserved across the read-only round-trip"
    );

    // latest_scan composes the same metrics; its signal matches.
    assert_eq!(engine.latest_scan().unwrap().signal, scanned.metrics.aggregate_signal);

    // Repeated read-only calls write nothing.
    for _ in 0..3 {
        engine.latest_metrics().unwrap();
        engine.latest_scan().unwrap();
        engine.latest_gate().unwrap();
    }
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        before,
        "no read-only accessor ever appended a snapshot"
    );
}

/// The read-only verdict mirrors a non-saving `gate` comparison: with a saved
/// baseline at the same tree it is a PASS holding the baseline, and reading it
/// persists nothing extra ([ADR-28]).
#[test]
fn latest_gate_compares_to_baseline_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    engine.gate(None, true, true).expect("gate --save sets a baseline");
    let after_save = metric_snapshot_count(tmp.path());

    let verdict = engine.latest_gate().expect("read-only verdict");
    assert!(verdict.passed, "the snapshot holds its own freshly-saved baseline");
    assert_eq!(verdict.signal, verdict.baseline_signal, "current == baseline");
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        after_save,
        "the read-only verdict appended no snapshot"
    );
}

/// A scanned store with **no saved baseline** (the common new-user state) yields
/// an informational pass naming the producing command, never a fabricated FAIL —
/// the distinct no-baseline branch of the read-only verdict ([ADR-28]).
#[test]
fn latest_gate_without_saved_baseline_is_informational_pass() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    engine.scan(false).expect("scan persists a snapshot but saves no baseline");
    let after_scan = metric_snapshot_count(tmp.path());

    let verdict = engine.latest_gate().expect("read-only verdict");
    assert!(verdict.passed, "no baseline cannot regress — informational pass");
    assert!(verdict.baseline_signal.is_none(), "there is no baseline to compare against");
    assert!(
        verdict.message.contains("no baseline"),
        "the verdict names the missing baseline: {}",
        verdict.message
    );
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        after_scan,
        "the read-only verdict appended no snapshot"
    );
}

/// CLI/MCP `scan` keeps persisting on every call — the read-only seam is
/// additive and leaves the evaluate-and-persist path byte-unchanged ([ADR-28]).
#[test]
fn cli_scan_still_persists_each_call() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.scan(false).unwrap();
    engine.scan(false).unwrap();
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        2,
        "scan persists a snapshot every call, as before"
    );
}

// ── temporal side: latest_temporal_report / latest_hotspots ──────────────────

/// The read-only temporal accessors recompute from the last-mined facts and
/// append **no** `temporal_snapshots` row, while the persisting
/// `temporal_report`/`hotspots` still append one — and the read-only figures
/// match the persisting board ([ADR-28], [NFR-RA-06]).
#[test]
fn latest_temporal_reads_never_append_a_snapshot() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Prime the mine the way the CLI does — this is allowed to persist.
    engine
        .hotspots(None, false)
        .expect("hotspots mines + appends one temporal snapshot");
    let primed = temporal_snapshot_count(tmp.path());
    assert!(primed >= 1, "the CLI hotspots read appended a temporal snapshot");

    // The read-only twin reflects the same per-file figures …
    let read_only = engine.latest_temporal_report().unwrap();
    let persisting = engine.temporal_report().unwrap(); // appends another, on purpose
    assert_eq!(
        serde_json::to_string(&read_only.files).unwrap(),
        serde_json::to_string(&persisting.files).unwrap(),
        "read-only temporal figures match the persisting report"
    );
    let after_persisting = temporal_snapshot_count(tmp.path());
    assert_eq!(
        after_persisting,
        primed + 1,
        "the persisting temporal_report appended exactly one"
    );

    // … and repeated read-only temporal/hotspot reads append nothing.
    for _ in 0..3 {
        engine.latest_temporal_report().unwrap();
        engine.latest_hotspots(Some(50), false).unwrap();
        engine.latest_hotspots(Some(20), true).unwrap();
    }
    assert_eq!(
        temporal_snapshot_count(tmp.path()),
        after_persisting,
        "no read-only temporal accessor ever appended a snapshot"
    );
}

/// The read-only hotspot board ranks identically to the persisting one at the
/// same HEAD — the dashboard reflects the last mine without re-mining ([ADR-28]).
#[test]
fn latest_hotspots_matches_the_persisting_board() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let persisting = engine.hotspots(Some(50), false).unwrap();
    let read_only = engine.latest_hotspots(Some(50), false).unwrap();
    assert_eq!(
        serde_json::to_string(&persisting.files).unwrap(),
        serde_json::to_string(&read_only.files).unwrap(),
        "the read-only board ranks identically to the mined board"
    );
    assert_eq!(persisting.ranked_files, read_only.ranked_files);
}

// ── language composition: FR-UI-10 / CR-021 / ADR-28 ─────────────────────────

/// Read the canonical store's bytes — the strongest no-write invariant: a
/// read-only accessor must leave `logos.db` byte-for-byte identical.
fn logos_db_bytes(repo: &Path) -> Vec<u8> {
    std::fs::read(repo.join(".logos/logos.db")).expect("read logos.db")
}

/// An un-indexed root (no `logos.db`) returns an empty composition — the
/// Dashboard's honest empty state, never an error ([FR-UI-10], [NFR-CC-04]).
#[test]
fn language_composition_on_unindexed_root_is_empty() {
    let tmp = TempDir::new().expect("temp root");
    sh_git(tmp.path(), &["init", "-q", "-b", "main"]);
    commit(tmp.path(), "src/a.rs", "pub fn a() {}\n", "add a, never indexed");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(
        engine.language_composition().unwrap().languages.is_empty(),
        "a never-indexed root has no indexed nodes → empty composition"
    );
}

/// On a Rust-only indexed graph the composition reports exactly `rust` with its
/// node/file counts; a registered-but-unused grammar (e.g. `python`) is absent,
/// and repeated reads leave `logos.db` byte-for-byte unchanged ([FR-UI-10],
/// [FR-UI-03], [ADR-28]).
#[test]
fn language_composition_reflects_the_indexed_graph_without_writing() {
    let tmp = indexed_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let comp = engine.language_composition().unwrap();
    assert_eq!(comp.languages.len(), 1, "the fixture indexes only rust files");
    let rust = &comp.languages[0];
    assert_eq!(rust.language, "rust");
    assert_eq!(rust.files, 2, "src/a.rs and src/b.rs both carry nodes");
    assert!(rust.nodes > 0, "every count is a graph fact, not fabricated");
    assert!(
        !comp.languages.iter().any(|e| e.language == "python"),
        "a registered-but-unused grammar never appears (distinct from `languages`)"
    );

    // The composition lists only languages the project actually uses, unlike the
    // registry listing which surfaces every loaded grammar (FR-PL-06).
    let registered = engine.languages();
    assert!(
        registered.languages.len() > comp.languages.len(),
        "more grammars are registered than the project uses"
    );

    // Repeated reads mutate no store: logos.db is byte-identical and no metric
    // snapshot is appended ([FR-UI-03] AC, [ADR-28]).
    let before = logos_db_bytes(tmp.path());
    let snapshots_before = metric_snapshot_count(tmp.path());
    for _ in 0..3 {
        engine.language_composition().unwrap();
    }
    assert_eq!(
        logos_db_bytes(tmp.path()),
        before,
        "reading the composition left logos.db byte-for-byte unchanged"
    );
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        snapshots_before,
        "reading the composition appended no snapshot"
    );
}

// ── FR-GV-17: the façade's blast-radius-ranked test-gaps read-model ───────────

/// An indexed git fixture with two untested targets of equal caller fan-in (3)
/// in files of very different hotspot heat: `src/hot.rs` churns over six commits
/// and carries branchy (high-complexity) callers → the top hotspot; `src/cool.rs`
/// is a single trivial commit → a low hotspot score. With fan-in held equal,
/// only the containing file's hotspot weight can separate `target_hot` from
/// `target_cool` ([FR-GV-17]).
fn blast_radius_repo() -> TempDir {
    let tmp = TempDir::new().expect("temp root");
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);

    // cool.rs: one commit, trivial complexity → a low hotspot score. Three
    // callers give `target_cool` fan-in 3.
    commit(
        repo,
        "src/cool.rs",
        "pub fn target_cool() -> i64 { 0 }\n\
         pub fn c1() -> i64 { target_cool() }\n\
         pub fn c2() -> i64 { target_cool() }\n\
         pub fn c3() -> i64 { target_cool() }\n",
        "add cool",
    );

    // hot.rs: six commits (high churn) + branchy callers (high complexity) → the
    // top hotspot. The final indexed revision wires three callers into
    // `target_hot` (fan-in 3, the SAME as `target_cool`).
    let hot = "pub fn target_hot() -> i64 { 0 }\n\
         pub fn h1(x: i64) -> i64 { if x > 0 { target_hot() } else { 1 } }\n\
         pub fn h2(x: i64) -> i64 { if x > 1 { target_hot() } else { 2 } }\n\
         pub fn h3(x: i64) -> i64 { if x > 2 { target_hot() } else { 3 } }\n";
    for n in 0..6 {
        commit(repo, "src/hot.rs", &format!("{hot}// rev {n}\n"), &format!("hot v{n}"));
    }

    Engine::start(repo).expect("engine starts").index();
    tmp
}

/// FR-GV-17 AC1+AC2: the façade orders the untested set by blast radius (caller
/// fan-in × the containing file's hotspot score). `target_hot` and `target_cool`
/// share fan-in 3, so the hotspot weight is the only thing that can order them —
/// and only AFTER history has been mined. The ranking is read-only, so an
/// un-mined store degrades to the FR-GV-08 file/name order with the caveat
/// intact, never a fabricated rank ([NFR-CC-04], [ADR-28]).
#[test]
fn test_gaps_orders_by_blast_radius_only_after_history_is_mined() {
    let tmp = blast_radius_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    let pos = |report: &TestGapsReport, name: &str| {
        report.untested.iter().position(|g| g.name == name)
    };

    // Both targets carry real caller fan-in, so the hotspot weight is the only
    // discriminator (FR-NV-02).
    assert_eq!(engine.callers("target_hot", None).total, 3, "target_hot fan-in");
    assert_eq!(engine.callers("target_cool", None).total, 3, "target_cool fan-in");

    // (1) Un-mined: no hotspot store → the FR-GV-08 file/name order. "src/cool.rs"
    // sorts before "src/hot.rs", so `target_cool` precedes `target_hot`. The
    // mandatory caveat is present (BR-16).
    let degraded = engine.test_gaps(None, true).expect("test_gaps runs");
    assert!(
        degraded.caveat.contains("static reachability"),
        "the honesty caveat is always emitted (BR-16): {}",
        degraded.caveat
    );
    let (cool_d, hot_d) = (pos(&degraded, "target_cool"), pos(&degraded, "target_hot"));
    assert!(
        cool_d.is_some() && hot_d.is_some(),
        "both targets are gaps: {:?}",
        degraded.untested
    );
    assert!(cool_d < hot_d, "degraded → file/name order: cool.rs before hot.rs");

    // (2) Mine the temporal tier (this WRITE is the explicit mining call, not the
    // test_gaps read). hot.rs must rank first.
    let board = engine.hotspots(None, false).expect("hotspots mines history.db");
    assert!(board.degraded.is_none(), "a real git repo is not degraded");
    assert_eq!(
        board.files.first().map(|h| h.path.as_str()),
        Some("src/hot.rs"),
        "hot.rs is the top hotspot ({:?})",
        board.files
    );

    // (3) Mined: with fan-in equal, the hotspot weight flips `target_hot` above
    // `target_cool` — the blast-radius order (FR-GV-17 AC1).
    let ranked = engine.test_gaps(None, true).expect("test_gaps runs");
    let (cool_r, hot_r) = (pos(&ranked, "target_cool"), pos(&ranked, "target_hot"));
    assert!(
        hot_r < cool_r,
        "ranked → blast radius: the hot-file target outranks the cool-file target ({:?})",
        ranked.untested
    );
}

/// FR-GV-17 AC4 / ADR-28: computing the blast-radius ranking on a read leaves
/// every store unchanged — repeated `test_gaps` reads append no metric or
/// temporal snapshot, never mine, and return a deterministic order ([NFR-RA-06])
/// — the same order every surface (web/MCP/CLI) gets from this one accessor.
#[test]
fn test_gaps_ranking_is_write_free_and_deterministic() {
    let tmp = blast_radius_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");

    // Mine once so the ranking is actually active (history.db now exists).
    engine.hotspots(None, false).expect("hotspots mines history.db");
    let metrics_before = metric_snapshot_count(tmp.path());
    let temporal_before = temporal_snapshot_count(tmp.path());
    let logos_before = logos_db_bytes(tmp.path());

    // Repeated read-only (reconcile = false, the web path) ranking reads.
    let first =
        serde_json::to_string(&engine.test_gaps(None, false).unwrap().untested).unwrap();
    for _ in 0..3 {
        let again =
            serde_json::to_string(&engine.test_gaps(None, false).unwrap().untested).unwrap();
        assert_eq!(again, first, "the ranked order is deterministic across reads (NFR-RA-06)");
    }

    assert_eq!(
        metric_snapshot_count(tmp.path()),
        metrics_before,
        "ranking appended no metric snapshot (ADR-28)"
    );
    assert_eq!(
        temporal_snapshot_count(tmp.path()),
        temporal_before,
        "ranking mined/appended no temporal snapshot (ADR-28)"
    );
    assert_eq!(
        logos_db_bytes(tmp.path()),
        logos_before,
        "ranking left logos.db byte-for-byte unchanged (ADR-28)"
    );
}

/// FR-GV-17 / S-149 AC: ordering precedes truncation, so raising `limit` reveals
/// more gaps in the SAME ranked order — a smaller limit is a strict prefix of the
/// full ranked list. Guards against a maintenance slip that truncated the
/// untested set *before* ranking it (which would silently re-order the head).
#[test]
fn test_gaps_limit_truncates_after_ranking_preserving_order() {
    let tmp = blast_radius_repo();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.hotspots(None, false).expect("hotspots mines history.db");

    // `TestGap` is not `PartialEq`/`Clone`, so compare the ranked name sequence.
    let names = |limit: Option<u32>| -> Vec<String> {
        engine
            .test_gaps(limit, false)
            .expect("test_gaps runs")
            .untested
            .into_iter()
            .map(|g| g.name)
            .collect()
    };

    let full = names(None);
    assert!(full.len() >= 4, "the fixture yields several ranked gaps: {full:?}");
    assert_eq!(names(Some(2)), full[..2].to_vec(), "limit=2 is the first 2 of the ranked order");
    assert_eq!(names(Some(4)), full[..4].to_vec(), "limit=4 is the first 4 of the ranked order");
}
