//! Every snapshot persists the worst offenders it computed (S-498, [FR-QM-15],
//! [CR-162]) and its "where to start" ranking and outlier labels (S-504,
//! [FR-QM-17], [CR-164]).
//!
//! Engine-level, over real indexed fixtures: each snapshot-appending entry point
//! (`scan`, `gate`, `gate --save`, `session_start`, `session_end`) writes the
//! per-dimension lists with its row, and the read-only Health bundle projects
//! exactly those lists from the snapshot it reads the signal from — computing
//! nothing and writing nothing. A snapshot written before the lists were
//! persisted reads "not recorded", which the JSON tells apart from a
//! recorded-empty result.
//!
//! [FR-QM-15]: ../../docs/specs/requirements/FR-QM-15.md
//! [CR-162]: ../../docs/requests/CR-162-health-shows-the-worst-offenders-its-snapshot-computed.md
//! [FR-QM-17]: ../../docs/specs/requirements/FR-QM-17.md
//! [CR-164]: ../../docs/requests/CR-164-insight-layer-ranks-what-to-fix-first.md
//!
//! Since S-676 ([FR-QM-19], [CR-218]) every entry also carries its symbol id,
//! measure, value and threshold, and every snapshot persists its uncapped
//! lists, which `Engine::offenders` reads back by path and dimension.
//!
//! [FR-QM-19]: ../../docs/specs/requirements/FR-QM-19.md
//! [CR-218]: ../../docs/requests/CR-218-agent-dev-kit-plugin-and-review.md

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::governance::OffendersFilter;
use logos_core::models::quality::{FixUnitKind, OffenderListing, WhereToStart, WorstOffenders};
use logos_core::Engine;
use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Three production functions nested 6, 5 and 4 levels deep — all at or over
/// the default `T_nest = 4` — declared shallowest first, so the persisted list
/// (deepest first) differs from declaration order and from name order.
fn nested_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
pub fn alpha_depth_four(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            if x > 2 {
                if x > 3 {
                    return x;
                }
            }
        }
    }
    0
}

pub fn beta_depth_six(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            if x > 2 {
                if x > 3 {
                    if x > 4 {
                        if x > 5 {
                            return x;
                        }
                    }
                }
            }
        }
    }
    0
}

pub fn gamma_depth_five(x: i32) -> i32 {
    if x > 0 {
        if x > 1 {
            if x > 2 {
                if x > 3 {
                    if x > 4 {
                        return x;
                    }
                }
            }
        }
    }
    0
}
",
    );
    tmp
}

/// A two-function fixture that trips no structural dimension: every list empty.
fn clean_project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "pub fn api() {\n    helper();\n}\nfn helper() {}\n",
    );
    tmp
}

fn read_only(root: &Path) -> Connection {
    Connection::open_with_flags(
        root.join(".logos/logos.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open logos.db read-only")
}

fn metric_snapshot_count(root: &Path) -> i64 {
    read_only(root)
        .query_row("SELECT count(*) FROM metric_snapshots", [], |r| r.get(0))
        .unwrap()
}

fn offender_row_count(root: &Path) -> i64 {
    read_only(root)
        .query_row("SELECT count(*) FROM metric_snapshot_offenders", [], |r| {
            r.get(0)
        })
        .unwrap()
}

/// One snapshot's offender rows, every column but the snapshot id, in key
/// order — rendered as text so "identical" means the stored values.
fn offender_rows(root: &Path, snapshot_id: i64) -> Vec<String> {
    read_only(root)
        .prepare(
            "SELECT dimension, rank, name, quote(file), quote(line), detail \
             FROM metric_snapshot_offenders WHERE snapshot_id = ?1 \
             ORDER BY dimension, rank",
        )
        .unwrap()
        .query_map([snapshot_id], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// One snapshot's where-to-start rows — fix units, measures and labels — every
/// column but the snapshot id, rendered as text in key order.
fn insight_rows(root: &Path, snapshot_id: i64) -> Vec<String> {
    let conn = read_only(root);
    let mut rows = Vec::new();
    for sql in [
        "SELECT rank, kind, node_id, name, quote(file), quote(line), dimensions, points, detail, outliers \
         FROM metric_snapshot_fix_units WHERE snapshot_id = ?1 ORDER BY rank",
        "SELECT measure, units, quote(fence), quote(threshold), labelled, '', '', '', '', '' \
         FROM metric_snapshot_outlier_measures WHERE snapshot_id = ?1 ORDER BY measure",
        "SELECT measure, rank, node_id, name, quote(file), quote(line), value, '', '', '' \
         FROM metric_snapshot_outlier_labels WHERE snapshot_id = ?1 ORDER BY measure, rank",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let found = stmt
            .query_map([snapshot_id], |r| {
                (0..10)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map(|cols| cols.join("|"))
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        rows.extend(found);
    }
    rows
}

fn nesting_names(w: &WorstOffenders) -> Vec<&str> {
    w.nesting.iter().map(|o| o.name.as_str()).collect()
}

/// [FR-QM-15] AC 1: after `scan`, `latest_health()` returns, entry for entry,
/// the lists the `scan` returned — not a recompute, and not the empty default
/// the Health bundle carried before.
#[test]
fn latest_health_returns_the_lists_scan_returned_entry_for_entry() {
    let tmp = nested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let scanned = engine.scan(true).expect("scan runs").worst_offenders;
    assert!(scanned.recorded, "a scan's lists are a recorded set");
    assert_eq!(
        nesting_names(&scanned),
        ["beta_depth_six", "gamma_depth_five", "alpha_depth_four"],
        "the fixture's known offenders, deepest first"
    );

    let health = engine.latest_health().expect("read-only health");
    assert_eq!(
        health.scan.worst_offenders, scanned,
        "the Health bundle projects exactly the lists the scan persisted"
    );
    assert_eq!(
        engine
            .latest_scan()
            .expect("read-only scan")
            .worst_offenders,
        scanned,
        "the standalone read-only scan projects the same lists"
    );
}

/// [FR-QM-15] AC 2: every snapshot-appending entry point persists its lists,
/// not only `scan`. Each runs alone on a fresh store, so the snapshot
/// `latest_health()` reads is the one that entry point wrote; the lists it
/// carries must equal the ones a `scan` of the same unchanged tree returns.
#[test]
fn every_snapshot_appending_entry_point_persists_the_lists_it_computed() {
    type EntryPoint = fn(&Engine);
    let entry_points: [(&str, EntryPoint); 4] = [
        ("gate", |e| {
            e.gate(None, false, true).expect("gate runs");
        }),
        ("gate --save", |e| {
            e.gate(None, true, true).expect("gate --save runs");
        }),
        ("session_start", |e| {
            e.session_start().expect("session_start runs");
        }),
        ("session_end", |e| {
            e.session_end().expect("session_end runs");
        }),
    ];
    for (name, run) in entry_points {
        let tmp = nested_project();
        let engine = Engine::start(tmp.path()).expect("engine starts");
        assert!(engine.index().warnings.is_empty());

        run(&engine);
        assert_eq!(
            metric_snapshot_count(tmp.path()),
            1,
            "`{name}` appended exactly the snapshot read below"
        );
        let health = engine.latest_health().expect("read-only health").scan;
        let (projected, projected_ranking) = (health.worst_offenders, health.where_to_start);

        let scan = engine.scan(true).expect("scan runs");
        let scanned = scan.worst_offenders;
        assert!(projected_ranking.recorded, "`{name}`'s snapshot recorded its ranking");
        assert!(!projected_ranking.units.is_empty(), "`{name}` ranked the fixture's units");
        assert_eq!(
            projected_ranking, scan.where_to_start,
            "`{name}` persisted, field for field, the ranking a scan of the same tree returns"
        );
        assert!(projected.recorded, "`{name}`'s snapshot recorded its lists");
        assert_eq!(
            nesting_names(&projected),
            ["beta_depth_six", "gamma_depth_five", "alpha_depth_four"],
            "`{name}` persisted the fixture's known offenders"
        );
        assert_eq!(
            projected, scanned,
            "`{name}` persisted, entry for entry, the lists a scan of the same tree returns"
        );
    }
}

/// [FR-QM-15] AC 5 / [NFR-RA-06]: a repeated scan of an unchanged tree writes
/// byte-identical offender rows — the same dimensions, ranks, names, files,
/// lines and details under each snapshot id.
#[test]
fn a_repeated_scan_of_an_unchanged_tree_writes_byte_identical_offender_rows() {
    let tmp = nested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    engine.scan(true).expect("first scan");
    engine.scan(true).expect("second scan");
    let first = offender_rows(tmp.path(), 1);
    let second = offender_rows(tmp.path(), 2);
    assert_eq!(
        first,
        [
            // CR-209: beta (CC 7) is the one function above the mean of 6.
            "equality|1|beta_depth_six|'src/lib.rs'|14|complexity 7",
            "nesting|1|beta_depth_six|'src/lib.rs'|14|nesting depth 6",
            "nesting|2|gamma_depth_five|'src/lib.rs'|31|nesting depth 5",
            "nesting|3|alpha_depth_four|'src/lib.rs'|1|nesting depth 4",
        ],
        "the first snapshot stored its lists in computed order"
    );
    assert_eq!(
        first, second,
        "an unchanged tree re-persists byte-identical rows"
    );
    let (first, second) = (insight_rows(tmp.path(), 1), insight_rows(tmp.path(), 2));
    assert!(!first.is_empty(), "the fixture ranks units");
    assert_eq!(first, second, "and byte-identical where-to-start rows (NFR-RA-06)");
}

/// [FR-QM-15] AC 4 / [NFR-CC-04]: a snapshot written before offenders were
/// persisted projects as **not recorded**, and the JSON tells that apart from
/// a recorded-empty result — the flag, not the empty arrays, carries it.
///
/// The pre-migration row is the exact state migration 26 leaves an old
/// snapshot in (`offenders_recorded` NULL, no offender rows — asserted in
/// `graph_store::migrate`), produced here on a real scanned store.
#[test]
fn a_pre_migration_snapshot_reads_not_recorded_and_never_as_recorded_empty() {
    // Not recorded: a scanned store whose snapshot is put back in the state an
    // upgraded pre-migration row is in.
    let old = nested_project();
    let engine = Engine::start(old.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    engine.scan(true).expect("scan runs");
    Connection::open(old.path().join(".logos/logos.db"))
        .unwrap()
        .execute_batch(
            "DELETE FROM metric_snapshot_offenders; \
             UPDATE metric_snapshots SET offenders_recorded = NULL;",
        )
        .unwrap();
    let not_recorded = engine.latest_health().expect("read-only health");
    assert!(
        !not_recorded.scan.worst_offenders.recorded,
        "a snapshot with no recorded lists reads not recorded"
    );
    assert_eq!(not_recorded.scan.worst_offenders, WorstOffenders::default());
    assert!(
        not_recorded.scan.signal.is_some(),
        "the signal is still projected from that snapshot — only its lists are absent"
    );

    // Recorded-empty: a scan that flagged nothing.
    let clean = clean_project();
    let engine = Engine::start(clean.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    engine.scan(true).expect("scan runs");
    let recorded_empty = engine.latest_health().expect("read-only health");
    let w = &recorded_empty.scan.worst_offenders;
    assert!(
        w.recorded,
        "a scan that flagged nothing recorded an empty result"
    );
    assert!(
        w.lists().iter().all(|(_, list)| list.is_empty()),
        "every list is empty on the clean fixture"
    );

    // The JSON shape S-499 mirrors: the two differ only in `recorded`.
    let json = |w: &WorstOffenders| serde_json::to_value(w).unwrap();
    let empty = serde_json::json!([]);
    for (recorded, value) in [
        (false, json(&not_recorded.scan.worst_offenders)),
        (true, json(w)),
    ] {
        assert_eq!(
            value,
            serde_json::json!({
                "recorded": recorded,
                "values_recorded": recorded,
                "nesting": empty,
                "conciseness": empty,
                "cohesion": empty,
                "focus": empty,
                "uniqueness": empty,
                "acyclicity": empty,
                "depth": empty,
                "equality": empty,
                "redundancy": empty,
            }),
            "the payload carries the recorded flag beside the nine lists"
        );
    }
}

/// A never-scanned store has no snapshot, so it has no lists to show: they
/// read not recorded, like a pre-migration snapshot's — never as "none flagged".
#[test]
fn a_never_scanned_store_reads_not_recorded() {
    let tmp = nested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    assert_eq!(metric_snapshot_count(tmp.path()), 0, "nothing scanned yet");
    let health = engine.latest_health().unwrap().scan;
    assert!(!health.worst_offenders.recorded);
    assert_eq!(health.where_to_start, WhereToStart::default(), "nor any ranking");
}

/// [FR-QM-15] AC 3: the Health GET recomputes nothing and writes nothing — no
/// snapshot and no offender row — and the non-persisting quality readout
/// appends no offender rows either.
#[test]
fn the_health_read_and_the_quality_readout_write_no_offender_rows() {
    let tmp = nested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    engine.scan(true).expect("scan runs");
    let snapshots = metric_snapshot_count(tmp.path());
    let rows = offender_row_count(tmp.path());
    assert_eq!(
        rows, 4,
        "the scan persisted the fixture's three Nesting and one Equality offenders"
    );

    for _ in 0..3 {
        engine.latest_health().expect("read-only health");
        engine.latest_scan().expect("read-only scan");
        engine.quality_readout().expect("readout");
    }
    assert_eq!(
        metric_snapshot_count(tmp.path()),
        snapshots,
        "no snapshot appended"
    );
    assert_eq!(
        offender_row_count(tmp.path()),
        rows,
        "no offender row appended"
    );
}

/// The persisted dimension names and the serialized field names are one
/// vocabulary: [`WorstOffenders::DIMENSIONS`] lists exactly the JSON keys
/// beside the `recorded` and `values_recorded` flags, in field order, and [`WorstOffenders::lists`] /
/// `list_mut` agree with it.
#[test]
fn the_dimension_vocabulary_is_the_serialized_field_set() {
    let mut w = WorstOffenders::default();
    let value = serde_json::to_value(&w).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = vec!["recorded", "values_recorded"];
    expected.extend(WorstOffenders::DIMENSIONS);
    let mut sorted_keys = keys.clone();
    sorted_keys.sort_unstable();
    expected.sort_unstable();
    assert_eq!(sorted_keys, expected);

    for (i, dimension) in WorstOffenders::DIMENSIONS.iter().enumerate() {
        w.list_mut(dimension)
            .unwrap_or_else(|| panic!("`{dimension}` has a list"))
            .push(logos_core::models::quality::Offender {
                name: format!("n{i}"),
                ..Default::default()
            });
    }
    for (i, (dimension, list)) in w.lists().into_iter().enumerate() {
        assert_eq!(dimension, WorstOffenders::DIMENSIONS[i]);
        assert_eq!(
            list.len(),
            1,
            "`{dimension}` received exactly its own entry"
        );
        assert_eq!(
            list[0].name,
            format!("n{i}"),
            "`{dimension}` maps to its own field"
        );
    }
    assert!(
        w.list_mut("modularity").is_none(),
        "Modularity — the one dimension outside the nine — has no list"
    );
}

/// A production function `name` whose body nests `depth` `if`s.
fn nested_fn(name: &str, depth: usize) -> String {
    let mut body = String::new();
    for level in 0..depth {
        body.push_str(&format!("{}if x > {level} {{\n", "    ".repeat(level + 1)));
    }
    body.push_str(&format!("{}return x;\n", "    ".repeat(depth + 1)));
    for level in (0..depth).rev() {
        body.push_str(&format!("{}}}\n", "    ".repeat(level + 1)));
    }
    format!("pub fn {name}(x: i32) -> i32 {{\n{body}    0\n}}\n")
}

/// [FR-QM-15]: every persisted list is capped at `WORST_OFFENDER_CAP`, and the
/// cap keeps the most severe entries. Two more offenders than the cap — the
/// last two declared the deepest — persist exactly `WORST_OFFENDER_CAP` rows,
/// led by those two, and the Health bundle projects the same capped list.
#[test]
fn persisted_lists_are_capped_at_the_worst_offender_cap_keeping_the_deepest() {
    let cap = logos_core::metrics::WORST_OFFENDER_CAP;
    let total = cap + 2;
    let tmp = TempDir::new().unwrap();
    let source: String = (0..total)
        .map(|i| nested_fn(&format!("f{i:02}"), if i >= cap { 5 } else { 4 }))
        .collect();
    write(tmp.path(), "src/lib.rs", &source);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let scanned = engine.scan(true).expect("scan runs").worst_offenders;
    let persisted: i64 = read_only(tmp.path())
        .query_row(
            "SELECT count(*) FROM metric_snapshot_offenders WHERE dimension = 'nesting'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        persisted, cap as i64,
        "{total} offenders persist exactly the cap's {cap} rows"
    );
    let projected = engine
        .latest_health()
        .expect("read-only health")
        .scan
        .worst_offenders;
    assert_eq!(
        projected.nesting.len(),
        cap,
        "the projection is the capped list"
    );
    assert_eq!(projected, scanned);
    let mut deepest: Vec<&str> = nesting_names(&projected)[..2].to_vec();
    deepest.sort_unstable();
    assert_eq!(
        deepest,
        [format!("f{:02}", cap), format!("f{:02}", cap + 1)],
        "the cap keeps the two depth-5 functions, ahead of every depth-4 one"
    );
}

/// [BR-25]: the persisted lists are computed under the **effective**
/// `rules.toml` thresholds — the ones the snapshot's hash records — not the
/// defaults. Raising `nesting_depth` to 5 drops the depth-4 function from the
/// list that `scan` persists and from the one a standalone `gate` persists.
///
/// [BR-25]: ../../docs/specs/software-spec.md#311-quality-metrics
#[test]
fn persisted_lists_follow_the_effective_rules_toml_thresholds() {
    type EntryPoint = fn(&Engine);
    let entry_points: [(&str, EntryPoint); 2] = [
        ("scan", |e| {
            e.scan(true).expect("scan runs");
        }),
        ("gate", |e| {
            e.gate(None, false, true).expect("gate runs");
        }),
    ];
    for (name, run) in entry_points {
        let tmp = nested_project();
        write(
            tmp.path(),
            ".logos/rules.toml",
            "[metric_thresholds]\nnesting_depth = 5\n",
        );
        let engine = Engine::start(tmp.path()).expect("engine starts");
        assert!(engine.index().warnings.is_empty());

        run(&engine);
        let projected = engine
            .latest_health()
            .expect("read-only health")
            .scan
            .worst_offenders;
        assert_eq!(
            nesting_names(&projected),
            ["beta_depth_six", "gamma_depth_five"],
            "`{name}` persisted the lists under nesting_depth = 5, not the default 4"
        );
    }
}

/// CR-209 end to end: a real two-directory cycle is recorded under Acyclicity,
/// read back by the Health bundle exactly as scanned, and — once its rows are
/// removed, as on a snapshot written before CR-209 — read back as unrecorded
/// rather than as an empty, clean list ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn a_two_directory_cycle_is_recorded_and_a_pre_cr209_snapshot_reads_unrecorded() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub mod alpha;\npub mod beta;\n");
    write(
        tmp.path(),
        "src/alpha/mod.rs",
        "pub fn ping(n: u32) -> u32 {\n    if n == 0 { 0 } else { crate::beta::pong(n - 1) }\n}\n",
    );
    write(
        tmp.path(),
        "src/beta/mod.rs",
        "pub fn pong(n: u32) -> u32 {\n    if n == 0 { 1 } else { crate::alpha::ping(n - 1) }\n}\n",
    );
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let scanned = engine.scan(true).expect("scan runs");
    assert_eq!(scanned.metrics.acyclicity.raw, 1.0, "one cross-directory cycle");
    let cycle: Vec<(&str, &str)> = scanned
        .worst_offenders
        .acyclicity
        .iter()
        .map(|o| (o.file.as_str(), o.detail.as_str()))
        .collect();
    assert_eq!(cycle.len(), 1, "{cycle:?}");
    assert_eq!(cycle[0].1, "2 symbols across 2 directories: src/alpha, src/beta");

    let read = engine.latest_health().expect("read-only health").scan.worst_offenders;
    assert_eq!(read, scanned.worst_offenders, "Health reads back what scan recorded");
    assert!(read.unrecorded.is_empty());

    // A snapshot written before CR-209 holds rows for the five lists only.
    Connection::open(tmp.path().join(".logos/logos.db"))
        .unwrap()
        .execute(
            "DELETE FROM metric_snapshot_offenders \
             WHERE dimension IN ('acyclicity', 'depth', 'equality', 'redundancy')",
            [],
        )
        .unwrap();
    let old = engine.latest_health().expect("read-only health").scan.worst_offenders;
    assert!(old.recorded && old.acyclicity.is_empty());
    assert!(
        old.unrecorded.contains(&"acyclicity"),
        "a cycle the score counts but no row names reads unrecorded: {:?}",
        old.unrecorded
    );
    let json = serde_json::to_value(&old).unwrap();
    assert!(json["unrecorded"].as_array().is_some_and(|a| a.contains(&"acyclicity".into())));
    assert!(
        serde_json::to_value(&read).unwrap().get("unrecorded").is_none(),
        "elided on a snapshot this version wrote"
    );
}

// ── S-504: the "where to start" ranking and outlier labels ───────────────────

/// Eight shallow production functions and one nested six deep: the deep one is
/// a Nesting offender, a ranked function unit, and — against this member's own
/// distribution of depth-1 functions — a nesting outlier.
fn insight_project(rules: Option<&str>) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let mut source: String = (0..8)
        .map(|i| format!("pub fn shallow_{i}(x: i32) -> i32 {{\n    if x > {i} {{\n        return x;\n    }}\n    0\n}}\n"))
        .collect();
    source.push_str(&nested_fn("deep_one", 6));
    write(tmp.path(), "src/lib.rs", &source);
    if let Some(rules) = rules {
        write(tmp.path(), ".logos/rules.toml", rules);
    }
    tmp
}

/// [FR-QM-17] AC 4: the ranking and labels a `scan` computed are persisted with
/// its snapshot and `latest_health()` reads them back unchanged — the deep
/// function ranked first, under Nesting, carrying its nesting outlier label.
#[test]
fn latest_health_reads_back_the_ranking_and_labels_scan_returned() {
    let tmp = insight_project(None);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let scanned = engine.scan(true).expect("scan runs").where_to_start;
    assert!(scanned.recorded);
    let top = &scanned.units[0];
    assert_eq!(
        (top.kind, top.name.as_str(), top.dimensions.as_slice(), top.outliers.as_slice()),
        (FixUnitKind::Function, "deep_one", &["nesting"][..], &["nesting"][..]),
        "{scanned:#?}"
    );
    assert!(top.points >= 1);
    let nesting = scanned.outliers.iter().find(|m| m.measure == "nesting").unwrap();
    assert_eq!(nesting.units, 9, "the member's nine production functions");
    assert_eq!(nesting.labels.len(), 1);
    assert_eq!(nesting.labels[0].name, "deep_one");
    assert_eq!(nesting.labels[0].value, 6);

    let health = engine.latest_health().expect("read-only health");
    assert_eq!(health.scan.where_to_start, scanned, "field for field");
    assert_eq!(
        serde_json::to_string(&health.scan.where_to_start).unwrap(),
        serde_json::to_string(&scanned).unwrap(),
        "and byte for byte in the payload"
    );
    assert_eq!(engine.latest_scan().unwrap().where_to_start, scanned);
    // FR-QM-17: the payload labels its figures an estimate.
    assert_eq!(
        serde_json::to_value(&health.scan.where_to_start).unwrap()["basis"],
        WhereToStart::BASIS
    );
}

/// [FR-QM-17] / [NFR-CC-04]: a snapshot written before migration 36 reads
/// "not recorded" — never a recorded-empty ranking — while its signal and its
/// offender lists still project.
#[test]
fn a_pre_migration_snapshot_reads_its_ranking_not_recorded() {
    let tmp = insight_project(None);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    engine.scan(true).expect("scan runs");
    Connection::open(tmp.path().join(".logos/logos.db"))
        .unwrap()
        .execute_batch(
            "DELETE FROM metric_snapshot_fix_units; DELETE FROM metric_snapshot_outlier_measures; \
             DELETE FROM metric_snapshot_outlier_labels; \
             UPDATE metric_snapshots SET insight_recorded = NULL;",
        )
        .unwrap();
    let health = engine.latest_health().expect("read-only health").scan;
    assert_eq!(health.where_to_start, WhereToStart::default());
    assert_eq!(
        serde_json::to_value(&health.where_to_start).unwrap(),
        serde_json::json!({"recorded": false, "units": [], "outliers": []})
    );
    assert!(health.signal.is_some() && health.worst_offenders.recorded);
}

/// [FR-QM-17] AC 5 / [BR-61]: emptying or changing the persisted ranking leaves
/// the signal, the gate verdict and every `check` exit code unchanged — over
/// the three `check` outcomes (no contract → exit 4, a passing contract → 0, a
/// failing one → 1).
///
/// [BR-61]: ../../docs/specs/software-spec.md#311-quality-metrics
#[test]
fn emptying_or_changing_the_ranking_moves_no_signal_gate_or_check_outcome() {
    type Tamper = (&'static str, &'static str);
    let tampers: [Tamper; 2] = [
        (
            "emptied",
            "DELETE FROM metric_snapshot_fix_units; DELETE FROM metric_snapshot_outlier_labels;",
        ),
        (
            "changed",
            "UPDATE metric_snapshot_fix_units SET points = points * 7 + 3, name = 'other'; \
             UPDATE metric_snapshot_outlier_labels SET value = value + 40;",
        ),
    ];
    let contracts: [(Option<&str>, Option<bool>); 3] = [
        (None, None),
        (Some("[constraints]\nmax_nesting_depth = 10\n"), Some(true)),
        (Some("[constraints]\nmax_nesting_depth = 4\n"), Some(false)),
    ];
    for (rules, check_outcome) in contracts {
        for (how, sql) in tampers {
            let tmp = insight_project(rules);
            let engine = Engine::start(tmp.path()).expect("engine starts");
            assert!(engine.index().warnings.is_empty());
            let saved = engine.gate(None, true, true).expect("gate --save runs");
            let check_before = engine.check_rules(None, false).expect("check runs");
            assert_eq!(check_before.passed, check_outcome, "the contract's own outcome");
            let health_before = engine.latest_health().expect("read-only health");
            assert!(!health_before.scan.where_to_start.units.is_empty());

            Connection::open(tmp.path().join(".logos/logos.db"))
                .unwrap()
                .execute_batch(sql)
                .unwrap();

            let health_after = engine.latest_health().expect("read-only health");
            assert_ne!(
                health_after.scan.where_to_start, health_before.scan.where_to_start,
                "the ranking was {how}"
            );
            assert_eq!(health_after.scan.signal, health_before.scan.signal, "{how}: signal");
            assert_eq!(
                serde_json::to_value(&health_after.gate).unwrap(),
                serde_json::to_value(&health_before.gate).unwrap(),
                "{how}: the read-only gate verdict"
            );
            let check_after = engine.check_rules(None, false).expect("check runs");
            assert_eq!(check_after.passed, check_before.passed, "{how}: the check exit discriminator");
            assert_eq!(
                serde_json::to_value(&check_after.violations).unwrap(),
                serde_json::to_value(&check_before.violations).unwrap(),
                "{how}: the check findings"
            );
            let gate = engine.gate(None, false, true).expect("gate runs");
            assert_eq!(
                (gate.passed, gate.signal, gate.baseline_signal),
                (saved.passed, saved.signal, saved.signal),
                "{how}: a fresh gate against the saved baseline"
            );
        }
    }
}

/// [FR-QM-17] AC 3 / [BR-56]: two members, each calibrated on its own
/// distribution. In member A the depth-6 function stands out among depth-1
/// functions and is labelled; in member B every function is that deep, so the
/// same depth labels nothing. Re-scanning A after B has been scanned — and
/// changed — reproduces A's rows byte for byte: B's distribution never reaches
/// A's labels.
///
/// [BR-56]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn one_members_distribution_never_changes_the_others_labels() {
    let a = insight_project(None);
    let b = TempDir::new().unwrap();
    let uniform: String = (0..9).map(|i| nested_fn(&format!("deep_{i}"), 6)).collect();
    write(b.path(), "src/lib.rs", &uniform);

    let engine_a = Engine::start(a.path()).expect("member A starts");
    let engine_b = Engine::start(b.path()).expect("member B starts");
    assert!(engine_a.index().warnings.is_empty() && engine_b.index().warnings.is_empty());

    let labels = |w: &WhereToStart| -> Vec<String> {
        let nesting = w.outliers.iter().find(|m| m.measure == "nesting").unwrap();
        nesting.labels.iter().map(|l| l.name.clone()).collect()
    };
    let first_a = engine_a.scan(true).expect("scan A").where_to_start;
    assert_eq!(labels(&first_a), ["deep_one"], "A labels its depth-6 function");
    let scanned_b = engine_b.scan(true).expect("scan B").where_to_start;
    assert!(labels(&scanned_b).is_empty(), "B, uniformly that deep, labels none");

    write(b.path(), "src/lib.rs", &format!("{uniform}{}", nested_fn("deeper", 12)));
    engine_b.scan(true).expect("rescan B");
    let second_a = engine_a.scan(true).expect("rescan A").where_to_start;
    assert_eq!(second_a, first_a, "A's ranking and labels are unchanged");
    assert_eq!(insight_rows(a.path(), 1), insight_rows(a.path(), 2), "byte for byte");
}

/// A body long enough to clear the default 50-token exact-duplicate floor.
fn duplicated_body(name: &str) -> String {
    format!(
        "pub fn {name}(items: &[i64], limit: i64) -> i64 {{\n    let mut total = 0;\n    \
         for item in items {{\n        if *item > limit {{\n            total += item * 2 - limit;\n        \
         }} else if *item < -limit {{\n            total -= item * 3 + limit;\n        }} else {{\n            \
         total += item;\n        }}\n    }}\n    if total > limit * 10 {{\n        total / 2\n    }} else {{\n        \
         total + limit\n    }}\n}}\n"
    )
}

/// [FR-QM-17]: a real exact-duplicate pair, read from the store by the
/// recording pass, is ranked as one duplicate-group unit and read back by
/// `latest_health()` — the store's duplicate groups reach the ranking.
#[test]
fn a_real_exact_duplicate_pair_is_ranked_as_one_group_and_read_back() {
    let tmp = TempDir::new().unwrap();
    let mut source = format!("{}{}", duplicated_body("copy_one"), duplicated_body("copy_two"));
    for i in 0..3 {
        source.push_str(&format!("pub fn plain_{i}(x: i64) -> i64 {{\n    x + {i}\n}}\n"));
    }
    write(tmp.path(), "src/lib.rs", &source);
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let scanned = engine.scan(true).expect("scan runs");
    assert!(
        scanned.worst_offenders.redundancy.iter().any(|o| o.detail.contains("duplicate")),
        "the fixture's pair is an exact duplicate: {:?}",
        scanned.worst_offenders.redundancy
    );
    let groups: Vec<_> = scanned
        .where_to_start
        .units
        .iter()
        .filter(|u| u.kind == FixUnitKind::DuplicateGroup)
        .collect();
    assert_eq!(groups.len(), 1, "{:#?}", scanned.where_to_start.units);
    assert_eq!(
        (groups[0].name.as_str(), groups[0].dimensions.as_slice()),
        ("copy_one", &["redundancy"][..])
    );
    assert!(groups[0].detail.starts_with("duplicate group · 2 members"), "{}", groups[0].detail);
    assert_eq!(engine.latest_health().unwrap().scan.where_to_start, scanned.where_to_start);
}

// ── S-676 / FR-QM-19: offenders carry a value, a threshold and a symbol id ──

/// The FR-QM-19 fixture: 14 deep-nesting production functions under `src/a`
/// — more than [`WORST_OFFENDER_CAP`](logos_core::metrics::WORST_OFFENDER_CAP),
/// spread over a file and a nested directory — and deep functions just
/// outside the glob: `src/ab` (one character from matching `src/a/**`), `src`
/// itself, and `lib`.
fn fourteen_under_src_a() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let fns = |prefix: &str, range: std::ops::Range<usize>| -> String {
        range.map(|i| nested_fn(&format!("{prefix}{i:02}"), 4 + i % 3)).collect()
    };
    write(tmp.path(), "src/lib.rs", &format!("pub mod a;\npub mod ab;\n{}", nested_fn("root_deep", 5)));
    write(tmp.path(), "src/a/mod.rs", &format!("pub mod inner;\n{}", fns("a_top_", 0..8)));
    write(tmp.path(), "src/a/inner/mod.rs", &fns("a_inner_", 0..6));
    write(tmp.path(), "src/ab/mod.rs", &fns("ab_near_", 0..2));
    write(tmp.path(), "lib/extra.rs", &nested_fn("lib_deep", 6));
    tmp
}

/// [FR-QM-19] AC 1: after `scan`, every capped entry carries its node id
/// (except a Depth row, which names directories), its measure and value, and
/// `value ≥ threshold` wherever the dimension has a threshold — persisted
/// under the snapshot's `offender_values_recorded` flag and read back by the
/// Health bundle field for field.
#[test]
fn after_scan_every_entry_carries_its_node_id_value_and_threshold() {
    let tmp = fourteen_under_src_a();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());

    let scanned = engine.scan(true).expect("scan runs").worst_offenders;
    assert!(scanned.recorded && scanned.values_recorded);
    let mut entries = 0;
    for (dimension, list) in scanned.lists() {
        for o in list {
            entries += 1;
            assert!(o.measure.is_some(), "{dimension}: {o:?}");
            let value = o.value.unwrap_or_else(|| panic!("{dimension} records a value: {o:?}"));
            if let Some(threshold) = o.threshold {
                assert!(value >= threshold, "{dimension}: value {value} < threshold {threshold}");
            }
            assert_eq!(o.node_id.is_none(), dimension == "depth", "{dimension}: {o:?}");
        }
    }
    assert!(entries > scanned.nesting.len(), "more than one dimension is exercised");
    assert_eq!(scanned.nesting.len(), logos_core::metrics::WORST_OFFENDER_CAP, "capped");
    let deep = &scanned.nesting[0];
    assert_eq!((deep.measure, deep.threshold), (Some("nesting_depth"), Some(4)));
    assert!(
        deep.node_id.as_deref().is_some_and(|id| id.ends_with(&format!("{}().", deep.name))
            && !id.contains("github.com")),
        "the id is the coordinate-free descriptor tail: {deep:?}"
    );

    let flag: Option<i64> = read_only(tmp.path())
        .query_row("SELECT offender_values_recorded FROM metric_snapshots", [], |r| r.get(0))
        .unwrap();
    assert_eq!(flag, Some(1));
    assert_eq!(engine.latest_health().unwrap().scan.worst_offenders, scanned);
}

/// [FR-QM-19] AC 2: `offenders --path 'src/a/**' --dimension nesting` returns
/// all 14 deep functions under `src/a` — past the display cap — and none
/// outside the glob, `src/ab` included; each is the full entry the capped list
/// carries, in its order. Without filters every offender of every dimension is
/// listed, the capped lists being the head of each.
#[test]
fn offenders_by_path_and_dimension_lists_all_fourteen_and_nothing_outside() {
    let tmp = fourteen_under_src_a();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let scanned = engine.scan(true).expect("scan runs").worst_offenders;

    let filter = OffendersFilter::parse(Some("src/a/**"), Some("nesting")).unwrap();
    let listing = engine.offenders(&filter).expect("offenders reads");
    assert!(listing.recorded && listing.not_recorded_because.is_none());
    assert_eq!(listing.offenders.len(), 14, "all 14, uncapped: {listing:#?}");
    let mut names: Vec<&str> = listing.offenders.iter().map(|o| o.offender.name.as_str()).collect();
    names.sort_unstable();
    let mut expected: Vec<String> = (0..8).map(|i| format!("a_top_{i:02}")).collect();
    expected.extend((0..6).map(|i| format!("a_inner_{i:02}")));
    expected.sort_unstable();
    assert_eq!(names, expected);
    for o in &listing.offenders {
        assert_eq!(o.dimension, "nesting");
        assert!(o.offender.file.starts_with("src/a/"), "inside the glob: {o:?}");
        assert!(o.offender.value >= o.offender.threshold && o.offender.threshold == Some(4));
    }

    let all_nesting = engine
        .offenders(&OffendersFilter::parse(None, Some("nesting")).unwrap())
        .unwrap()
        .offenders;
    assert_eq!(all_nesting.len(), 14 + 2 + 1 + 1, "src/a, src/ab, src and lib");
    let head: Vec<_> = all_nesting.iter().take(scanned.nesting.len()).map(|o| &o.offender).collect();
    assert_eq!(head, scanned.nesting.iter().collect::<Vec<_>>(), "the capped list is the head");
    let in_a: Vec<_> = all_nesting.iter().filter(|o| o.offender.file.starts_with("src/a/")).collect();
    assert_eq!(
        in_a,
        listing.offenders.iter().collect::<Vec<_>>(),
        "the glob keeps the full list's order"
    );

    let everything = engine.offenders(&OffendersFilter::default()).unwrap().offenders;
    let dimensions: Vec<&str> = everything.iter().map(|o| o.dimension).collect();
    let mut canonical = dimensions.clone();
    canonical.sort_by_key(|d| WorstOffenders::DIMENSIONS.iter().position(|x| x == d));
    assert_eq!(dimensions, canonical, "dimension order, then each list's own");
    assert!(everything.len() > all_nesting.len(), "every dimension is listed");
}

/// [FR-QM-19] AC 3 / [NFR-RA-06]: the listing is byte-stable — twice-read on
/// one snapshot, and re-read after a re-scan of the unchanged tree — and the
/// uncapped rows each scan persists are byte-identical. Reading writes nothing.
#[test]
fn the_offender_listing_is_byte_stable_and_writes_nothing() {
    let tmp = fourteen_under_src_a();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    engine.scan(true).expect("scan runs");
    let filter = OffendersFilter::default();
    let json = || serde_json::to_string(&engine.offenders(&filter).unwrap()).unwrap();

    let first = json();
    assert_eq!(json(), first, "twice-read, byte for byte");
    assert_eq!(metric_snapshot_count(tmp.path()), 1, "reading appended no snapshot");
    engine.scan(true).expect("re-scan");
    assert_eq!(json(), first, "a re-scan of the unchanged tree lists the same bytes");

    let uncapped = |id: i64| -> Vec<String> {
        read_only(tmp.path())
            .prepare(
                "SELECT dimension, rank, name, quote(file), quote(line), detail, quote(node_id), \
                        quote(measure), quote(value), quote(threshold) \
                 FROM metric_snapshot_offenders_uncapped WHERE snapshot_id = ?1 ORDER BY dimension, rank",
            )
            .unwrap()
            .query_map([id], |r| {
                (0..10)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map(|cols| cols.join("|"))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    let rows = uncapped(1);
    assert!(rows.len() > logos_core::metrics::WORST_OFFENDER_CAP, "uncapped rows were persisted");
    assert_eq!(uncapped(2), rows, "an unchanged tree re-persists byte-identical uncapped rows");
}

/// [FR-QM-19] AC 4 / [NFR-CC-04]: a snapshot written before migration 37 reads
/// its new fields as "not recorded" — `null` under `values_recorded: false`,
/// never `0` — and lists no uncapped offenders, saying why; a never-scanned
/// store says so too. The pre-migration state is the one migration 37 leaves
/// an old snapshot in (asserted in `graph_store::migrate`), on a real store.
#[test]
fn a_pre_migration_snapshot_reads_its_offender_values_not_recorded() {
    let tmp = nested_project();
    let engine = Engine::start(tmp.path()).expect("engine starts");
    assert!(engine.index().warnings.is_empty());
    let never = engine.offenders(&OffendersFilter::default()).unwrap();
    assert_eq!(
        (never.recorded, never.not_recorded_because),
        (false, Some(OffenderListing::NEVER_SCANNED))
    );

    engine.scan(true).expect("scan runs");
    Connection::open(tmp.path().join(".logos/logos.db"))
        .unwrap()
        .execute_batch(
            "DELETE FROM metric_snapshot_offenders_uncapped; \
             UPDATE metric_snapshot_offenders SET node_id = NULL, measure = NULL, value = NULL, threshold = NULL; \
             UPDATE metric_snapshots SET offender_values_recorded = NULL;",
        )
        .unwrap();

    let w = engine.latest_health().expect("read-only health").scan.worst_offenders;
    assert!(w.recorded && !w.values_recorded, "the lists are recorded, their values are not");
    assert_eq!(nesting_names(&w), ["beta_depth_six", "gamma_depth_five", "alpha_depth_four"]);
    let entry = serde_json::to_value(&w.nesting[0]).unwrap();
    for field in ["node_id", "measure", "value", "threshold"] {
        assert_eq!(entry[field], serde_json::Value::Null, "{field} reads not recorded, never 0: {entry}");
    }
    assert_eq!(serde_json::to_value(&w).unwrap()["values_recorded"], false);

    let old = engine.offenders(&OffendersFilter::default()).unwrap();
    assert_eq!(
        (old.recorded, old.not_recorded_because, old.offenders.len()),
        (false, Some(OffenderListing::PREDATES_UNCAPPED), 0)
    );
}

/// [FR-QM-19] AC 3 / [BR-61]: the offender values and the uncapped list never
/// feed the measurement. Emptying, nulling or changing them leaves the signal,
/// the read-only gate verdict and a fresh gate against the saved baseline
/// unchanged; every snapshot is written at the unchanged metric-semantics
/// version.
///
/// [BR-61]: ../../docs/specs/software-spec.md#311-quality-metrics
#[test]
fn offender_values_and_the_uncapped_list_move_no_signal_gate_or_semantics_version() {
    // S-676 did not move the semantics; S-523 moved them to v8 (FR-QM-18).
    assert_eq!(logos_core::metrics::METRIC_SEMANTICS_VERSION, 8, "S-676 does not move the semantics");
    let tampers: [(&str, &str); 3] = [
        ("emptied", "DELETE FROM metric_snapshot_offenders_uncapped;"),
        (
            "nulled",
            "UPDATE metric_snapshot_offenders SET node_id = NULL, value = NULL, threshold = NULL; \
             UPDATE metric_snapshots SET offender_values_recorded = NULL;",
        ),
        (
            "changed",
            "UPDATE metric_snapshot_offenders SET value = value * 9 + 1, threshold = 99, node_id = 'x'; \
             UPDATE metric_snapshot_offenders_uncapped SET value = 0;",
        ),
    ];
    for (how, sql) in tampers {
        let tmp = fourteen_under_src_a();
        let engine = Engine::start(tmp.path()).expect("engine starts");
        assert!(engine.index().warnings.is_empty());
        let saved = engine.gate(None, true, true).expect("gate --save runs");
        let before = engine.latest_health().expect("read-only health");
        let listing_before = engine.offenders(&OffendersFilter::default()).unwrap();

        Connection::open(tmp.path().join(".logos/logos.db"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();

        let after = engine.latest_health().expect("read-only health");
        assert!(
            after.scan.worst_offenders != before.scan.worst_offenders
                || engine.offenders(&OffendersFilter::default()).unwrap() != listing_before,
            "the offender values were {how}"
        );
        assert_eq!(after.scan.signal, before.scan.signal, "{how}: signal");
        assert_eq!(
            serde_json::to_value(&after.gate).unwrap(),
            serde_json::to_value(&before.gate).unwrap(),
            "{how}: the read-only gate verdict"
        );
        let gate = engine.gate(None, false, true).expect("gate runs");
        assert_eq!(
            (gate.passed, gate.signal, gate.baseline_signal),
            (saved.passed, saved.signal, saved.signal),
            "{how}: a fresh gate against the saved baseline"
        );
        let versions: Vec<i64> = read_only(tmp.path())
            .prepare("SELECT metric_version FROM metric_snapshots ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let current = logos_core::metrics::METRIC_SEMANTICS_VERSION;
        assert_eq!(versions, [current, current], "{how}: every snapshot at the current semantics version");
    }
}
