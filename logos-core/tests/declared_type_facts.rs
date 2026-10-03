//! Declared types persist as member-local facts (S-472, CR-152 §3.2 B, ADR-70
//! decision point 1) — end to end through the public [`Engine`].
//!
//! The reader's naming and refusal rules are pinned beside it
//! (`extract/declared_types_tests.rs`); the schema in `graph_store::migrate`.
//! What is pinned here is the pipeline half: an index records every source and
//! Avro fact with its provenance; a one-file change resyncs only that file's
//! facts, and the synced store's facts equal a fresh index's (sync ≡ reindex);
//! a member with no Java/Kotlin/Avro file writes nothing but the one extraction
//! marker; and an upgraded store reads "not yet extracted" until its first full
//! walk re-extracts its package-shaped source and marks the facts complete.
#![cfg(all(feature = "lang-rust", feature = "lang-java", feature = "lang-kotlin"))]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::graph_store::{AvroSchemaRow, DeclaredTypeRow, DECLARED_TYPES_EXTRACTED_KEY};
use logos_core::{Engine, Runtime};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

fn types(rt: &Runtime) -> Vec<DeclaredTypeRow> {
    rt.submit_read(|store| store.declared_types()).expect("read runs")
}

fn schemas(rt: &Runtime) -> Vec<AvroSchemaRow> {
    rt.submit_read(|store| store.avro_schemas()).expect("read runs")
}

fn extracted(rt: &Runtime) -> bool {
    rt.submit_read(|store| store.declared_types_extracted()).expect("read runs")
}

/// `(path, fqn or reason, kind, tree)` per fact — the projection most
/// assertions compare.
fn summary(rows: &[DeclaredTypeRow]) -> Vec<(String, String, String, Option<String>)> {
    rows.iter()
        .map(|r| {
            let name = r.fqn.clone().unwrap_or_else(|| format!("refused: {}", r.name));
            (r.path.clone(), name, r.kind.clone(), r.tree.clone())
        })
        .collect()
}

/// Every row of `table`, rowid-ordered, from the member store opened read-only.
fn table(root: &Path, table: &str) -> Vec<Vec<String>> {
    dump(root, &[]).into_iter().find(|(t, _)| t == table).map(|(_, rows)| rows).unwrap_or_default()
}

/// Every row of every table, as text — the byte-for-byte projection.
/// Clock-stamped bookkeeping is excluded by name by the caller.
fn dump(root: &Path, skip: &[&str]) -> Vec<(String, Vec<Vec<String>>)> {
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open the member store read-only");
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'nodes_fts%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    tables
        .into_iter()
        .filter(|t| !skip.contains(&t.as_str()))
        .map(|table| {
            let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid")).unwrap();
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |r| {
                    (0..columns)
                        .map(|i| {
                            Ok(match r.get_ref(i)? {
                                rusqlite::types::ValueRef::Null => "NULL".to_string(),
                                rusqlite::types::ValueRef::Integer(v) => format!("i:{v}"),
                                rusqlite::types::ValueRef::Real(v) => format!("r:{v}"),
                                rusqlite::types::ValueRef::Text(v) => {
                                    format!("t:{}", String::from_utf8_lossy(v))
                                }
                                rusqlite::types::ValueRef::Blob(v) => format!("b:{v:?}"),
                            })
                        })
                        .collect::<rusqlite::Result<Vec<String>>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (table, rows)
        })
        .collect()
}

/// The persisted graph revision (advanced only by a write that changed the graph).
fn revision(root: &Path) -> Option<String> {
    table(root, "project_metadata")
        .into_iter()
        .find(|row| row[0] == "t:graph_revision")
        .map(|row| row[1].clone())
}

/// The graph by content, id-free: every node as `(symbol, kind, name)`, every
/// edge and every reference-ledger row by its endpoints' symbols — what a
/// re-extract of an unchanged file must reproduce although it reissues ids.
fn graph(root: &Path) -> Vec<Vec<String>> {
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let mut out = Vec::new();
    for sql in [
        "SELECT 'node', s.symbol, n.kind, n.name, COALESCE(f.path, '') FROM nodes n \
         JOIN symbols s ON s.id = n.symbol_id LEFT JOIN files f ON f.id = n.file_id",
        "SELECT 'edge', ss.symbol, ts.symbol, e.kind, COALESCE(e.payload, '') FROM edges e \
         JOIN nodes sn ON sn.id = e.source JOIN symbols ss ON ss.id = sn.symbol_id \
         JOIN nodes tn ON tn.id = e.target JOIN symbols ts ON ts.id = tn.symbol_id",
        "SELECT 'ref', r.source_symbol, r.target, r.form || '/' || r.kind, r.resolved FROM unresolved_refs r",
        "SELECT 'file', path, COALESCE(content_hash, ''), COALESCE(language, ''), '' FROM files",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt
            .query_map([], |r| {
                (0..5)
                    .map(|i| r.get_ref(i).map(|v| format!("{v:?}")))
                    .collect::<rusqlite::Result<Vec<String>>>()
            })
            .unwrap();
        out.extend(rows.map(Result::unwrap));
    }
    out.sort();
    out
}

/// Take a member store back to what the release before migration 24 left on
/// disk: both declared-type tables absent (their indexes go with them),
/// migration 24 unrecorded, `user_version` 23 — and so no extraction marker. The
/// exact inverse of migration 24, and of migrations 25 (S-500, two `nodes`
/// columns), 26 (S-498, the snapshot offender table and its flag column) and
/// 27 (S-513, the persist-failure record) after it; the next [`Engine::start`]
/// re-applies all four, as a real upgrade does.
fn downgrade_to_v23(root: &Path) {
    let conn = rusqlite::Connection::open(root.join(".logos").join("logos.db")).unwrap();
    conn.execute_batch(&format!(
        "DROP TABLE persist_failures; DELETE FROM schema_versions WHERE version = 27; \
         DROP TABLE metric_snapshot_offenders; ALTER TABLE metric_snapshots DROP COLUMN offenders_recorded; \
         DELETE FROM schema_versions WHERE version = 26; \
         ALTER TABLE nodes DROP COLUMN body_tokens; ALTER TABLE nodes DROP COLUMN has_body; \
         DELETE FROM schema_versions WHERE version = 25; \
         DROP TABLE declared_types; DROP TABLE avro_schemas; \
         DELETE FROM schema_versions WHERE version = 24; \
         DELETE FROM project_metadata WHERE key = '{DECLARED_TYPES_EXTRACTED_KEY}'; \
         PRAGMA user_version = 23;"
    ))
    .expect("downgrade the store to v23");
}

const SVC: &str = "package com.x.mail;\n\npublic class MailService {\n    static class Nested {}\n}\n";
const PORT: &str = "package com.x.mail;\n\npublic interface MailPort {}\n";
const SVC_TEST: &str = "package com.x.mail;\n\nclass MailServiceTest {}\n";
const KOTLIN: &str = "package com.x.mail\n\nenum class Channel { PEC, MAIL }\nobject Defaults\n";
const EVENTS: &str = r#"{"type": "record", "name": "MailSent", "namespace": "com.x.events",
    "fields": [{"name": "status", "type": {"type": "enum", "name": "Status", "symbols": ["OK"]}}]}"#;
const AUDIT: &str = r#"{"type": "enum", "name": "AuditKind", "namespace": "com.x.audit", "symbols": ["A"]}"#;

/// A Java/Kotlin/Avro member with a Rust file beside it, so the graph has a
/// non-declaring file too.
fn member(root: &Path) {
    write(root, "tools/src/lib.rs", "pub fn hello() {}\n");
    write(root, "svc/src/main/java/com/x/mail/MailService.java", SVC);
    write(root, "svc/src/main/java/com/x/mail/MailPort.java", PORT);
    write(root, "svc/src/test/java/com/x/mail/MailServiceTest.java", SVC_TEST);
    write(root, "svc/src/main/kotlin/com/x/mail/Channels.kt", KOTLIN);
    write(root, "svc/src/main/avro/events.avsc", EVENTS);
    write(root, "svc/src/main/avro/audit.avsc", AUDIT);
}

fn row<'a>(rows: &'a [(String, String, String, Option<String>)], fqn: &str) -> &'a (String, String, String, Option<String>) {
    rows.iter().find(|r| r.1 == fqn).unwrap_or_else(|| panic!("no fact {fqn}; have {rows:?}"))
}

// ── an index records the facts ───────────────────────────────────────────────

#[test]
fn indexing_a_member_records_its_source_and_avro_types_with_their_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    let rows = types(rt);
    assert_eq!(
        summary(&rows),
        vec![
            ("svc/src/main/avro/audit.avsc".into(), "com.x.audit.AuditKind".into(), "enum".into(), None),
            ("svc/src/main/avro/events.avsc".into(), "com.x.events.MailSent".into(), "record".into(), None),
            ("svc/src/main/avro/events.avsc".into(), "com.x.events.Status".into(), "enum".into(), None),
            ("svc/src/main/java/com/x/mail/MailPort.java".into(), "com.x.mail.MailPort".into(), "interface".into(), Some("main".into())),
            ("svc/src/main/java/com/x/mail/MailService.java".into(), "com.x.mail.MailService".into(), "class".into(), Some("main".into())),
            ("svc/src/main/kotlin/com/x/mail/Channels.kt".into(), "com.x.mail.Channel".into(), "class".into(), Some("main".into())),
            ("svc/src/main/kotlin/com/x/mail/Channels.kt".into(), "com.x.mail.Defaults".into(), "class".into(), Some("main".into())),
            ("svc/src/test/java/com/x/mail/MailServiceTest.java".into(), "com.x.mail.MailServiceTest".into(), "class".into(), Some("test".into())),
        ],
        "ordered by provenance path; a nested type and the Rust file declare nothing"
    );
    for r in &rows {
        assert_eq!(r.resolution, "resolved");
        assert_eq!(r.origin, if r.path.ends_with(".avsc") { "avro" } else { "source" });
        assert_eq!(r.symbol.is_some(), r.origin == "source", "a source fact names its node");
    }

    // The symbol is the graph's own node for the type.
    let svc = rows.iter().find(|r| r.name == "MailService").unwrap();
    let node = rt
        .submit_read(|store| store.search("MailService", None, 10))
        .unwrap()
        .into_iter()
        .find(|n| n.kind.as_str() == "class")
        .expect("the class node");
    assert_eq!(svc.symbol.as_deref(), Some(node.symbol.as_str()));

    assert_eq!(
        schemas(rt).iter().map(|s| (s.path.as_str(), s.status.as_str())).collect::<Vec<_>>(),
        vec![("svc/src/main/avro/audit.avsc", "read"), ("svc/src/main/avro/events.avsc", "read")]
    );
    assert!(extracted(rt), "a full index marks the facts extracted");
}

#[test]
fn a_refused_package_and_a_malformed_schema_stay_in_the_denominator_with_their_reasons() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/main/java/com/x/Moved.java", "package com.y;\npublic class Moved {}\n");
    write(root, "src/main/avro/broken.avsc", "{ \"type\": \"record\", ");
    write(root, "src/main/avro/nameless.avsc", r#"{"type": "record", "fields": []}"#);
    write(root, "ignored/skip.avsc", AUDIT);
    write(root, ".gitignore", "ignored/\n");
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    let rows = types(rt);
    assert_eq!(rows.len(), 1, "no Avro type is guessed from a malformed schema: {rows:?}");
    assert_eq!((rows[0].name.as_str(), rows[0].fqn.as_deref()), ("Moved", None));
    assert_eq!(rows[0].resolution, "refused");
    assert!(rows[0].reason.as_deref().unwrap().contains("package `com.y`"), "{:?}", rows[0].reason);

    let schemas = schemas(rt);
    assert_eq!(
        schemas.iter().map(|s| (s.path.as_str(), s.status.as_str())).collect::<Vec<_>>(),
        vec![("src/main/avro/broken.avsc", "malformed"), ("src/main/avro/nameless.avsc", "malformed")],
        "found and refused schemas are both recorded; a gitignored one is never found"
    );
    assert!(schemas[0].detail.as_deref().unwrap().contains("not valid JSON"));
    assert!(schemas[1].detail.as_deref().unwrap().contains("no `name`"));
}

/// A schema that cannot be read as UTF-8 stays in the denominator —
/// `unreadable`, with no hash and its reason — and once fixed, a sync that names
/// it reads its types (the no-hash → hash transition).
#[test]
fn an_unreadable_schema_is_recorded_and_a_fixed_one_is_read_by_the_next_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/lib.rs", "pub fn hello() {}\n");
    let bin = "src/main/avro/bin.avsc";
    fs::create_dir_all(root.join("src/main/avro")).unwrap();
    fs::write(root.join(bin), [0xff, 0xfe, 0x00]).unwrap();
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    let rows = schemas(rt);
    assert_eq!(rows.len(), 1, "an unreadable schema is still found: {rows:?}");
    assert_eq!((rows[0].path.as_str(), rows[0].status.as_str()), (bin, "unreadable"));
    assert_eq!(rows[0].content_hash, None);
    assert!(rows[0].detail.as_deref().unwrap().contains("non-UTF-8"), "{:?}", rows[0].detail);
    assert!(types(rt).is_empty());

    write(root, bin, AUDIT);
    engine.sync(&[PathBuf::from(bin)]);
    let rows = schemas(rt);
    assert_eq!((rows[0].status.as_str(), rows[0].content_hash.is_some()), ("read", true));
    assert_eq!(summary(&types(rt))[0].1, "com.x.audit.AuditKind");
}

// ── a member with no Java/Kotlin/Avro file is unaffected ─────────────────────

#[test]
fn a_member_with_no_java_kotlin_or_avro_file_writes_nothing_but_the_marker() {
    let bare = tempfile::tempdir().unwrap();
    let with_avro = tempfile::tempdir().unwrap();
    for root in [bare.path(), with_avro.path()] {
        write(root, "src/lib.rs", "pub fn hello() {}\npub fn world() { hello(); }\n");
    }
    write(with_avro.path(), "src/main/avro/audit.avsc", AUDIT);
    for root in [bare.path(), with_avro.path()] {
        Engine::start(root).expect("engine starts").index();
    }

    // Clock-stamped bookkeeping aside, a schema changes the two declared-type
    // tables and nothing else: it reaches no graph table.
    let clocked = ["schema_versions", "project_metadata"];
    let declared = ["avro_schemas", "declared_types"];
    let skip: Vec<&str> = clocked.iter().chain(&declared).copied().collect();
    assert_eq!(
        dump(bare.path(), &skip),
        dump(with_avro.path(), &skip),
        "an .avsc changes no table outside avro_schemas / declared_types"
    );
    for t in declared {
        assert!(table(bare.path(), t).is_empty(), "{t} is empty for a member with nothing to declare");
    }
    let marker: Vec<_> = table(bare.path(), "project_metadata")
        .into_iter()
        .filter(|r| r[0] == format!("t:{DECLARED_TYPES_EXTRACTED_KEY}"))
        .collect();
    assert_eq!(marker.len(), 1, "the one row S-472 adds to such a member is its marker");

    // Neither a partial sync nor a full-walk reconcile writes anything for it.
    let before = dump(bare.path(), &["schema_versions"]);
    let engine = Engine::start(bare.path()).expect("engine restarts");
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    engine.health(true).expect("a full-walk reconcile runs");
    drop(engine);
    assert_eq!(
        dump(bare.path(), &["schema_versions"]),
        before,
        "a no-op sync and reconcile on such a member write nothing"
    );
}

// ── a one-file change resyncs only that file's facts ─────────────────────────

/// The declared-type rows with their row ids — "only that file's facts moved"
/// is asserted on ids, since a rewrite of an unchanged file would reissue them.
fn with_ids(root: &Path) -> Vec<Vec<String>> {
    table(root, "declared_types")
}

#[test]
fn changing_one_source_file_and_one_schema_resyncs_only_their_facts_and_sync_equals_reindex() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();
    let before = with_ids(root);

    // One source file moves package out from under its directory; one schema
    // gains a type.
    write(root, "svc/src/main/java/com/x/mail/MailPort.java", "package com.x.other;\npublic interface MailPort {}\n");
    write(
        root,
        "svc/src/main/avro/audit.avsc",
        r#"[{"type": "enum", "name": "AuditKind", "namespace": "com.x.audit", "symbols": ["A"]},
            {"type": "enum", "name": "AuditLevel", "namespace": "com.x.audit", "symbols": ["L"]}]"#,
    );
    let rev = revision(root);
    engine.sync(&[PathBuf::from("svc/src/main/avro/audit.avsc")]);
    assert_ne!(revision(root), rev, "a changed schema alone advances the graph revision");
    let rev = revision(root);
    engine.sync(&[PathBuf::from("svc/src/main/java/com/x/mail/MailPort.java")]);
    assert_ne!(revision(root), rev, "a changed source file advances the graph revision");

    let after = with_ids(root);
    let untouched = |rows: &[Vec<String>]| -> Vec<Vec<String>> {
        rows.iter()
            .filter(|r| !r.iter().any(|c| c.contains("MailPort") || c.contains("AuditKind") || c.contains("AuditLevel")))
            .cloned()
            .collect()
    };
    assert_eq!(untouched(&after), untouched(&before), "every other file's facts keep their rows, ids included");
    let rows = summary(&types(rt));
    assert_eq!(row(&rows, "refused: MailPort").0, "svc/src/main/java/com/x/mail/MailPort.java");
    row(&rows, "com.x.audit.AuditLevel");
    assert_eq!(rows.len(), 9);

    // An unchanged schema named again rewrites nothing and advances nothing.
    let before = (revision(root), with_ids(root));
    engine.sync(&[PathBuf::from("svc/src/main/avro/audit.avsc")]);
    assert_eq!((revision(root), with_ids(root)), before, "an unchanged schema is a no-op");

    // sync ≡ reindex: the synced facts are what a fresh index of the same tree
    // records.
    let fresh = tempfile::tempdir().unwrap();
    copy_tree(root, fresh.path());
    let fresh_engine = Engine::start(fresh.path()).expect("engine starts");
    fresh_engine.index();
    assert_eq!(types(rt), types(fresh_engine.runtime().unwrap()), "sync ≡ reindex over the declared types");
    assert_eq!(schemas(rt), schemas(fresh_engine.runtime().unwrap()), "and over the schemas");
}

/// A file whose new content declares no type at all loses every fact it had —
/// the per-file replace runs on an empty set too — so no stale, resolved name
/// outlives the type it named.
#[test]
fn a_file_that_stops_declaring_any_type_loses_its_facts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    let kotlin = "svc/src/main/kotlin/com/x/mail/Channels.kt";
    let java = "svc/src/main/java/com/x/mail/MailPort.java";
    write(root, kotlin, "package com.x.mail\n\nfun helper() {}\n");
    write(root, java, "package com.x.mail;\n\n// nothing declared here any more\n");
    engine.sync(&[PathBuf::from(kotlin), PathBuf::from(java)]);

    let rows = types(rt);
    assert!(
        !rows.iter().any(|t| t.path == kotlin || t.path == java),
        "no fact outlives the types its file stopped declaring: {:?}",
        summary(&rows)
    );
    let fresh = tempfile::tempdir().unwrap();
    copy_tree(root, fresh.path());
    let fresh_engine = Engine::start(fresh.path()).expect("engine starts");
    fresh_engine.index();
    assert_eq!(rows, types(fresh_engine.runtime().unwrap()), "sync ≡ reindex");
}

/// Copy the member's source tree — everything but its `.logos/` store.
fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == ".logos" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_tree(&src, &dst);
        } else {
            fs::copy(&src, &dst).unwrap();
        }
    }
}

#[test]
fn a_deleted_source_file_or_schema_takes_its_facts_by_a_partial_sync_and_by_a_full_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    // Partial: named deletions.
    fs::remove_file(root.join("svc/src/main/kotlin/com/x/mail/Channels.kt")).unwrap();
    fs::remove_file(root.join("svc/src/main/avro/audit.avsc")).unwrap();
    engine.sync(&[
        PathBuf::from("svc/src/main/kotlin/com/x/mail/Channels.kt"),
        PathBuf::from("svc/src/main/avro/audit.avsc"),
    ]);
    let rows = summary(&types(rt));
    assert!(!rows.iter().any(|r| r.0.ends_with("Channels.kt") || r.0.ends_with("audit.avsc")), "{rows:?}");
    assert_eq!(rows.len(), 5);

    // Full walk: an unnamed deletion is found by the walk, and an unnamed
    // addition too.
    fs::remove_file(root.join("svc/src/main/avro/events.avsc")).unwrap();
    write(root, "lib/src/main/avro/shared.avsc", AUDIT);
    engine.health(true).expect("reconcile runs");
    assert_eq!(
        schemas(rt).iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
        vec!["lib/src/main/avro/shared.avsc"]
    );
    let rows = summary(&types(rt));
    assert_eq!(rows.len(), 4, "{rows:?}");
    row(&rows, "com.x.audit.AuditKind");
}

/// A full index is authoritative: a schema an earlier index recorded and the
/// walk no longer finds is forgotten with its types, not left standing.
#[test]
fn a_re_index_forgets_a_schema_the_walk_no_longer_finds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    fs::remove_file(root.join("svc/src/main/avro/events.avsc")).unwrap();
    engine.index();
    let rt = engine.runtime().unwrap();
    assert_eq!(
        schemas(rt).iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
        vec!["svc/src/main/avro/audit.avsc"]
    );
    assert!(!types(rt).iter().any(|t| t.path.ends_with("events.avsc")));
}

#[test]
fn a_gitignored_schema_named_by_a_sync_is_treated_as_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    write(root, ".gitignore", "*.avsc\n");
    engine.sync(&[PathBuf::from("svc/src/main/avro/events.avsc")]);
    assert_eq!(
        schemas(rt).iter().map(|s| s.path.as_str()).collect::<Vec<_>>(),
        vec!["svc/src/main/avro/audit.avsc"],
        "a schema the walk would no longer admit leaves the facts"
    );
}

// ── an upgraded store is backfilled by its first full walk ───────────────────

/// **An upgraded store reads "not yet extracted" until a full walk.** A store
/// written before migration 24 has no declared-type facts and no marker, and
/// its unchanged source files are never re-extracted by an ordinary sync. A
/// partial sync never writes the marker. The first full-walk reconcile
/// re-extracts the member's package-shaped source once — leaving the graph as
/// it was — reads its schemas, and marks the facts complete; they then equal a
/// fresh index's. A second full walk writes nothing at all.
#[test]
fn the_first_full_walk_backfills_an_upgraded_members_declared_types() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    Engine::start(root).expect("engine starts").index();
    let graph_before = graph(root);
    downgrade_to_v23(root);

    let engine = Engine::start(root).expect("the upgraded store opens at the latest version");
    let rt = engine.runtime().unwrap();
    assert!(types(rt).is_empty(), "migration 24 creates the tables empty");
    assert!(!extracted(rt), "an upgraded store records no extraction");

    write(root, "tools/src/lib.rs", "pub fn hello() {}\npub fn more() {}\n");
    engine.sync(&[PathBuf::from("tools/src/lib.rs"), PathBuf::from("svc/src/main/avro/audit.avsc")]);
    assert_eq!(schemas(rt).len(), 1, "the partial sync read the schema it named");
    assert!(!extracted(rt), "a partial sync never marks the facts extracted");

    let rev = revision(root);
    engine.health(true).expect("a full-walk reconcile runs");
    assert!(extracted(rt), "the first full walk marks the facts extracted");
    assert_ne!(revision(root), rev, "completing the facts advances the graph revision");
    write(root, "tools/src/lib.rs", "pub fn hello() {}\n");
    engine.sync(&[PathBuf::from("tools/src/lib.rs")]);
    assert_eq!(graph(root), graph_before, "the backfill re-extracts unchanged source into the same graph");

    let fresh = tempfile::tempdir().unwrap();
    copy_tree(root, fresh.path());
    let fresh_engine = Engine::start(fresh.path()).expect("engine starts");
    fresh_engine.index();
    assert_eq!(types(rt), types(fresh_engine.runtime().unwrap()), "backfilled ≡ freshly indexed");

    let before = dump(root, &["schema_versions"]);
    engine.health(true).expect("a second full-walk reconcile runs");
    assert_eq!(dump(root, &["schema_versions"]), before, "once marked, a no-op full walk writes nothing");
}

/// S-513 / FR-EH-05: a backfill file that fails to persist keeps the backfill
/// open. The marker is not written, so the next full walk re-extracts the file
/// — its hash never moved — and only then marks the facts complete. Marking
/// despite the failure would skip the file as unchanged forever and lose its
/// declared types.
#[cfg(debug_assertions)]
#[test]
fn a_backfill_file_that_fails_to_persist_keeps_the_backfill_open() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    Engine::start(root).expect("engine starts").index();
    downgrade_to_v23(root);

    let engine = Engine::start(root).expect("the upgraded store opens");
    let rt = engine.runtime().unwrap();
    let service = "svc/src/main/java/com/x/mail/MailService.java";
    rt.inject_persist_fault(service);
    engine.health(true).expect("a full-walk reconcile runs");
    assert!(!extracted(rt), "a failed backfill file leaves the facts unmarked");
    assert!(
        !types(rt).iter().any(|t| t.path == service),
        "the failed file contributed no types"
    );

    rt.clear_persist_faults();
    engine.health(true).expect("the next full walk retries it");
    assert!(extracted(rt), "the retried backfill completes and marks the facts");
    let fresh = tempfile::tempdir().unwrap();
    copy_tree(root, fresh.path());
    let fresh_engine = Engine::start(fresh.path()).expect("engine starts");
    fresh_engine.index();
    assert_eq!(types(rt), types(fresh_engine.runtime().unwrap()), "backfilled ≡ freshly indexed");
}

/// The marker alone: an upgraded member with no Java/Kotlin/Avro file has
/// nothing to backfill, and its first full walk still records that its facts —
/// none — are complete, advancing the revision once; a second writes nothing.
#[test]
fn the_first_full_walk_marks_an_upgraded_member_with_nothing_to_declare() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/lib.rs", "pub fn hello() {}\n");
    Engine::start(root).expect("engine starts").index();
    downgrade_to_v23(root);

    let engine = Engine::start(root).expect("the upgraded store opens");
    let rt = engine.runtime().unwrap();
    assert!(!extracted(rt));
    let rev = revision(root);
    engine.health(true).expect("a full-walk reconcile runs");
    assert!(extracted(rt), "the first full walk marks a member with nothing to declare");
    assert_ne!(revision(root), rev, "turning an unread member into a read one advances the revision");
    assert!(types(rt).is_empty() && schemas(rt).is_empty());

    let before = dump(root, &["schema_versions"]);
    engine.health(true).expect("a second full-walk reconcile runs");
    assert_eq!(dump(root, &["schema_versions"]), before, "once marked, a no-op full walk writes nothing");
}
