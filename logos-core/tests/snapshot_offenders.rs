//! Every snapshot persists the worst offenders it computed (S-498, [FR-QM-15],
//! [CR-162]).
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

#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::Path;

use logos_core::models::quality::WorstOffenders;
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
        let projected = engine
            .latest_health()
            .expect("read-only health")
            .scan
            .worst_offenders;

        let scanned = engine.scan(true).expect("scan runs").worst_offenders;
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
                "nesting": empty,
                "conciseness": empty,
                "cohesion": empty,
                "focus": empty,
                "uniqueness": empty,
            }),
            "the payload carries the recorded flag beside the five lists"
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
    assert!(
        !engine
            .latest_health()
            .unwrap()
            .scan
            .worst_offenders
            .recorded
    );
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
    assert_eq!(rows, 3, "the scan persisted the fixture's three offenders");

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
/// beside `recorded`, in field order, and [`WorstOffenders::lists`] /
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
    let mut expected = vec!["recorded"];
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
        w.list_mut("redundancy").is_none(),
        "a name outside the five has no list"
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
