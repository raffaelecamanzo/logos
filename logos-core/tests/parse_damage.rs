//! Parse damage is **recorded per file, reported per language, warned about
//! by `scan`, and flagged on the insight entries it touches** ([FR-IX-14],
//! [S-571], [CR-181]).
//!
//! These tests pin, over a C++ tree one file of which breaks the parse the way
//! ccache's `parse_umask` does (an unexpanded `TRY_ASSIGN(…)` macro):
//!
//! - `status --json` states `parse_damage` on every language row — the damaged
//!   files and their floored share — and a relational answer's rows state none;
//! - `scan` carries one warning naming the language, and a clean tree none;
//! - a worst-offender or "where to start" entry in the damaged file reads
//!   `partial_parse: true`, and an entry in a clean file carries no key at
//!   all; the snapshot persists the flag with the entry, so `latest_scan` and
//!   `offenders` read back exactly what `scan` returned, even after the file
//!   is repaired, until the next scan;
//! - sync ≡ reindex: repairing the file and syncing it reads exactly what a
//!   cold index of the repaired tree reads.
//!
//! [FR-IX-14]: ../../docs/specs/requirements/FR-IX-14.md
//! [S-571]: ../../docs/planning/journal.md#s-571-parse-damage-is-reported-per-language-and-damaged-insight-entries-are-flagged
//! [CR-181]: ../../docs/requests/CR-181-cpp-is-declared-as-limited-support-and-parse-damage-is-reported.md

#![cfg(all(feature = "lang-cpp", feature = "lang-rust"))]

use std::fs;
use std::path::Path;

use logos_core::governance::OffendersFilter;
use logos_core::Engine;
use serde_json::Value;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

/// A deeply nested function (a Nesting offender at the default `T_nest` 4),
/// then ccache's `parse_umask` shape: the macro call tree-sitter cannot read.
const DAMAGED: &str = "\
int
deep(int a, int b)
{
  if (a) {
    for (int i = 0; i < b; i++) {
      while (b) {
        if (i) {
          return i;
        }
      }
    }
  }
  return 0;
}

tl::expected<mode_t, std::string>
parse_umask(std::string_view value)
{
  TRY_ASSIGN(auto mode, parse_unsigned(value, 0, 0777, \"umask\", 8));
  return static_cast<mode_t>(mode);
}
";

/// [`DAMAGED`] with the macro call gone: the same functions, parsed clean.
const REPAIRED: &str = "\
int
deep(int a, int b)
{
  if (a) {
    for (int i = 0; i < b; i++) {
      while (b) {
        if (i) {
          return i;
        }
      }
    }
  }
  return 0;
}

tl::expected<mode_t, std::string>
parse_umask(std::string_view value)
{
  return static_cast<mode_t>(value.size());
}
";

/// A clean C++ file holding a Nesting offender of its own.
const CLEAN: &str = "\
int
deep_clean(int a, int b)
{
  if (a) {
    for (int i = 0; i < b; i++) {
      while (b) {
        if (i) {
          return i;
        }
      }
    }
  }
  return 0;
}
";

const DAMAGED_PATH: &str = "src/util/string.cpp";

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A tree of `(path, contents)` files, indexed.
fn indexed(files: &[(&str, &str)]) -> (TempDir, Engine) {
    let tmp = TempDir::new().unwrap();
    for (rel, contents) in files {
        write(tmp.path(), rel, contents);
    }
    coordinate::pin(tmp.path());
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    (tmp, engine)
}

fn damaged_tree() -> (TempDir, Engine) {
    indexed(&[
        (DAMAGED_PATH, DAMAGED),
        ("src/util/clean.cpp", CLEAN),
        ("src/lib.rs", "pub fn ok() {}\n"),
    ])
}

/// `status --json`'s row for `language`, as the surface serialises it.
fn status_row(engine: &Engine, language: &str) -> Value {
    let status = serde_json::to_value(engine.status()).expect("status serialises");
    status["resolution_by_language"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["language"] == language)
        .unwrap_or_else(|| panic!("a {language} row: {status}"))
        .clone()
}

/// The serialised entries of a list that name `name`.
fn named<'a>(list: &'a Value, name: &str) -> Vec<&'a Value> {
    list.as_array().expect("a list").iter().filter(|e| e["name"] == name).collect()
}

/// Each file's recorded parse damage, by path — read off the store.
fn recorded(root: &Path) -> Vec<(String, Option<i64>)> {
    let db = rusqlite::Connection::open(root.join(".logos/logos.db")).expect("the store opens");
    let mut stmt = db.prepare("SELECT path, parse_damaged FROM files ORDER BY path").expect("prepare");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("rows")
}

#[test]
fn status_states_each_languages_parse_damage_with_its_share() {
    let (tmp, engine) = damaged_tree();
    assert_eq!(
        recorded(tmp.path()),
        [
            ("src/lib.rs".to_string(), Some(0)),
            ("src/util/clean.cpp".to_string(), Some(0)),
            (DAMAGED_PATH.to_string(), Some(1)),
        ],
        "every file records whether its parse was damaged"
    );
    assert_eq!(
        status_row(&engine, "cpp")["parse_damage"],
        serde_json::json!({ "files": 1, "unrecorded": 0, "share_percent": 50 })
    );
    // A clean language states its zero: the figure is on every row.
    assert_eq!(
        status_row(&engine, "rust")["parse_damage"],
        serde_json::json!({ "files": 0, "unrecorded": 0, "share_percent": 0 })
    );
}

#[test]
fn a_relational_answer_states_its_denominator_and_not_the_parse() {
    let (_tmp, engine) = damaged_tree();
    let callers = serde_json::to_value(engine.callers("deep", None)).expect("callers serialises");
    let rows = callers["resolution_denominator"]["languages"].as_array().expect("denominator rows");
    assert!(!rows.is_empty(), "the answer attaches its language's row: {callers}");
    assert!(rows.iter().all(|row| row.get("parse_damage").is_none()), "{rows:?}");
}

#[test]
fn scan_warns_naming_the_language_and_flags_the_entries_of_the_damaged_file() {
    let (_tmp, engine) = damaged_tree();
    let scan = engine.scan(true).expect("scan");
    let warned: Vec<&String> = scan.warnings.iter().filter(|w| w.starts_with("parse damage:")).collect();
    assert_eq!(warned.len(), 1, "{:?}", scan.warnings);
    assert!(
        warned[0].starts_with("parse damage: 1 of 2 cpp files (50%, warned at 10% or more)"),
        "{}",
        warned[0]
    );

    let offenders = serde_json::to_value(&scan.worst_offenders).expect("offenders serialise");
    let deep = named(&offenders["nesting"], "deep");
    assert_eq!(deep.len(), 1, "`deep` is a Nesting offender: {offenders}");
    assert_eq!(deep[0]["partial_parse"], true, "{}", deep[0]);
    let clean = named(&offenders["nesting"], "deep_clean");
    assert_eq!(clean.len(), 1, "`deep_clean` is a Nesting offender: {offenders}");
    assert!(clean[0].get("partial_parse").is_none(), "a clean file's entry grows no key: {}", clean[0]);

    // Every "where to start" unit in the damaged file is flagged, and only those.
    let units = &scan.where_to_start.units;
    assert!(units.iter().any(|u| u.file == DAMAGED_PATH), "the damaged file ranks a unit: {units:?}");
    for unit in units {
        assert_eq!(unit.partial_parse, unit.file == DAMAGED_PATH, "{unit:?}");
    }

    // The read-backs return, entry for entry, what the scan returned (FR-QM-15):
    // the capped lists and the ranking from the snapshot, and the uncapped list.
    let latest = engine.latest_scan().expect("latest scan");
    assert_eq!(latest.worst_offenders, scan.worst_offenders, "the snapshot's own lists");
    assert_eq!(latest.where_to_start, scan.where_to_start, "the snapshot's own ranking");
    assert_eq!(nesting_flags(&engine), [("deep".to_string(), true), ("deep_clean".to_string(), false)]);
}

/// The uncapped Nesting entries `offenders` reads back, with their flags, by
/// name.
fn nesting_flags(engine: &Engine) -> Vec<(String, bool)> {
    let filter = OffendersFilter::parse(None, Some("nesting")).expect("filter");
    let mut flags: Vec<(String, bool)> = engine
        .offenders(&filter)
        .expect("offenders")
        .offenders
        .iter()
        .map(|entry| (entry.offender.name.clone(), entry.offender.partial_parse))
        .collect();
    flags.sort();
    flags
}

/// A snapshot's flag is what it computed, persisted with the entry: a repair
/// synced after the scan changes the file's record, not the snapshot's
/// (FR-QM-15: offender rows are never mutated). The next scan records the
/// repair.
#[test]
fn a_snapshot_keeps_the_flag_it_computed_until_the_next_scan() {
    let (tmp, engine) = damaged_tree();
    let scan = engine.scan(true).expect("scan");
    write(tmp.path(), DAMAGED_PATH, REPAIRED);
    engine.sync(&[DAMAGED_PATH.into()]);
    assert_eq!(status_row(&engine, "cpp")["parse_damage"]["files"], 0, "the file's record moved");

    let latest = engine.latest_scan().expect("latest scan");
    assert_eq!(latest.worst_offenders, scan.worst_offenders, "the snapshot's lists did not");
    assert_eq!(latest.where_to_start, scan.where_to_start);
    assert_eq!(nesting_flags(&engine), [("deep".to_string(), true), ("deep_clean".to_string(), false)]);

    engine.scan(true).expect("rescan");
    assert_eq!(nesting_flags(&engine), [("deep".to_string(), false), ("deep_clean".to_string(), false)]);
}

/// A store upgraded to migration 44 records no file's parse until it is
/// re-extracted: `status` states those files as unrecorded — never as a clean
/// zero (NFR-CC-04) — and the next scan records them.
#[test]
fn an_upgraded_store_states_its_files_unrecorded_until_a_scan_records_them() {
    let (tmp, engine) = damaged_tree();
    drop(engine);
    let db = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).expect("the store opens");
    db.execute_batch(
        "ALTER TABLE files DROP COLUMN parse_damaged; ALTER TABLE metric_snapshot_offenders DROP COLUMN partial_parse; \
         ALTER TABLE metric_snapshot_offenders_uncapped DROP COLUMN partial_parse; \
         ALTER TABLE metric_snapshot_fix_units DROP COLUMN partial_parse; \
         DELETE FROM schema_versions WHERE version = 44; PRAGMA user_version = 43;",
    )
    .expect("back to what the release before S-571 left on disk");
    drop(db);

    let engine = Engine::start(tmp.path()).expect("engine reopens, applying migration 44");
    assert_eq!(
        status_row(&engine, "cpp")["parse_damage"],
        serde_json::json!({ "files": 0, "unrecorded": 2, "share_percent": 0 })
    );
    let scan = engine.scan(true).expect("scan");
    assert_eq!(
        status_row(&engine, "cpp")["parse_damage"],
        serde_json::json!({ "files": 1, "unrecorded": 0, "share_percent": 50 })
    );
    assert!(scan.warnings.iter().any(|w| w.starts_with("parse damage: 1 of 2 cpp files")), "{:?}", scan.warnings);
}

#[test]
fn a_clean_tree_carries_no_parse_damage_warning() {
    let (_tmp, engine) = indexed(&[("src/util/clean.cpp", CLEAN), ("src/lib.rs", "pub fn ok() {}\n")]);
    let scan = engine.scan(true).expect("scan");
    assert!(!scan.warnings.iter().any(|w| w.starts_with("parse damage:")), "{:?}", scan.warnings);
}

#[test]
fn repairing_the_file_and_syncing_it_reads_as_a_cold_index_does() {
    let (tmp, engine) = damaged_tree();
    write(tmp.path(), DAMAGED_PATH, REPAIRED);
    engine.sync(&[DAMAGED_PATH.into()]);
    let warm = (recorded(tmp.path()), status_row(&engine, "cpp")["parse_damage"].clone());
    assert_eq!(warm.1, serde_json::json!({ "files": 0, "unrecorded": 0, "share_percent": 0 }), "the repair is read");

    let (cold_tmp, cold_engine) = indexed(&[
        (DAMAGED_PATH, REPAIRED),
        ("src/util/clean.cpp", CLEAN),
        ("src/lib.rs", "pub fn ok() {}\n"),
    ]);
    let cold = (recorded(cold_tmp.path()), status_row(&cold_engine, "cpp")["parse_damage"].clone());
    assert_eq!(warm, cold, "sync ≡ reindex");

    // And the damage comes back when the macro does.
    write(tmp.path(), DAMAGED_PATH, DAMAGED);
    engine.sync(&[DAMAGED_PATH.into()]);
    assert_eq!(
        status_row(&engine, "cpp")["parse_damage"],
        serde_json::json!({ "files": 1, "unrecorded": 0, "share_percent": 50 })
    );
}
