//! Build manifests persist as member-local artifact facts (S-462, CR-148 §3.2 A,
//! ADR-69 decision point 1) — end to end through the public [`Engine`].
//!
//! The reader's parse and resolution rules are pinned beside it
//! (`extract/build_manifest_tests.rs`); the schema in `graph_store::migrate`.
//! What is pinned here is the pipeline half: an index records the facts, a sync
//! re-derives them when — and only when — a manifest it names changed, a
//! full-walk reconcile notices an added or deleted manifest, a member with no
//! build manifest is left exactly as it was apart from the one extraction
//! marker, and only a full walk ever writes that marker — so an upgraded store
//! reads as "not yet extracted" until one runs (S-462 task 2, FR-WS-33).
//!
//! Fixtures carry a Rust source file so the member has a graph beside its
//! manifests, as every real member does.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::graph_store::{
    BuildArtifactRow, BuildManifestRow, BUILD_FACTS_EXTRACTED_KEY, DECLARED_TYPES_EXTRACTED_KEY,
};
use logos_core::{Engine, Runtime};

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

fn manifests(rt: &Runtime) -> Vec<BuildManifestRow> {
    rt.submit_read(|store| store.build_manifests()).expect("read runs")
}

fn manifest<'a>(rows: &'a [BuildManifestRow], path: &str) -> &'a BuildManifestRow {
    rows.iter()
        .find(|m| m.path == path)
        .unwrap_or_else(|| panic!("no manifest {path} recorded; have {:?}", paths(rows)))
}

fn paths(rows: &[BuildManifestRow]) -> Vec<&str> {
    rows.iter().map(|m| m.path.as_str()).collect()
}

fn produced(m: &BuildManifestRow) -> &BuildArtifactRow {
    m.artifacts
        .iter()
        .find(|a| a.role == "produced")
        .expect("a produced fact")
}

fn coordinate(a: &BuildArtifactRow) -> (Option<&str>, Option<&str>, Option<&str>) {
    (a.group_id.as_deref(), a.artifact_id.as_deref(), a.version.as_deref())
}

/// Every row of every table, as text, from the member store opened read-only —
/// the byte-for-byte projection. Clock-stamped bookkeeping is excluded by name:
/// `schema_versions.applied_at` and the `project_metadata` timestamps are
/// written by the clock, not by the facts under test.
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
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    conn.query_row(
        "SELECT value FROM project_metadata WHERE key = 'graph_revision'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// The member store's build-facts extraction marker, if recorded.
fn extraction_marker(root: &Path) -> Option<String> {
    let db = root.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    conn.query_row(
        "SELECT value FROM project_metadata WHERE key = ?1",
        [BUILD_FACTS_EXTRACTED_KEY],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Take a member store back to what the release before migration 22 left on
/// disk: both build tables absent (their one index goes with them), migration
/// 22 unrecorded, `user_version` 21 — and so no extraction marker. The exact
/// inverse of migration 22, which is two `CREATE TABLE`s and one index, of
/// migration 23 (S-487), which only adds `metric_snapshots.modularity_applicable`,
/// and of migration 24 (S-472), two declared-type tables with their indexes and
/// marker; the next [`Engine::start`] re-applies all three, as it does on a real
/// upgrade.
fn downgrade_to_v21(root: &Path) {
    let conn = rusqlite::Connection::open(root.join(".logos").join("logos.db")).unwrap();
    conn.execute_batch(&format!(
        "DROP TABLE declared_types; DROP TABLE avro_schemas; \
         DELETE FROM schema_versions WHERE version = 24; \
         DELETE FROM project_metadata WHERE key = '{DECLARED_TYPES_EXTRACTED_KEY}'; \
         ALTER TABLE metric_snapshots DROP COLUMN modularity_applicable; \
         DELETE FROM schema_versions WHERE version = 23; \
         DROP TABLE build_artifacts; DROP TABLE build_manifests; \
         DELETE FROM schema_versions WHERE version = 22; \
         DELETE FROM project_metadata WHERE key = '{BUILD_FACTS_EXTRACTED_KEY}'; \
         PRAGMA user_version = 21;"
    ))
    .expect("downgrade the store to v21");
}

const ROOT_POM: &str = r#"<project>
    <groupId>com.example.james</groupId>
    <artifactId>parent</artifactId>
    <version>2.1.0</version>
    <packaging>pom</packaging>
    <parent>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-starter-parent</artifactId>
        <version>2.7.18</version>
    </parent>
    <properties>
        <james.groupId>org.apache.james</james.groupId>
    </properties>
    <dependencyManagement><dependencies>
        <dependency>
            <groupId>${james.groupId}</groupId>
            <artifactId>james-server-cli</artifactId>
            <version>3.7.4</version>
        </dependency>
        <dependency>
            <groupId>com.example</groupId>
            <artifactId>bom</artifactId>
            <version>1</version>
            <type>pom</type>
            <scope>import</scope>
        </dependency>
    </dependencies></dependencyManagement>
</project>"#;

const MODULE_POM: &str = r#"<project>
    <parent>
        <groupId>com.example.james</groupId>
        <artifactId>parent</artifactId>
        <version>2.1.0</version>
    </parent>
    <artifactId>lifecycle-module</artifactId>
    <dependencies>
        <dependency>
            <groupId>${james.groupId}</groupId>
            <artifactId>james-server-cli</artifactId>
            <scope>runtime</scope>
        </dependency>
        <dependency>
            <groupId>org.projectlombok</groupId>
            <artifactId>lombok</artifactId>
            <version>${lombok.version}</version>
        </dependency>
    </dependencies>
</project>"#;

fn member(root: &Path) {
    write(root, "src/lib.rs", "pub fn hello() {}\n");
    write(root, "pom.xml", ROOT_POM);
    write(root, "lifecycle-module/pom.xml", MODULE_POM);
}

// ── an index records the facts ───────────────────────────────────────────────

#[test]
fn indexing_a_member_persists_what_it_produces_and_references_with_kind_and_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);

    let engine = Engine::start(root).expect("engine starts");
    let result = engine.index();
    assert_eq!(result.files_indexed, 1, "a pom is never a graph file: {:?}", result.warnings);
    let rt = engine.runtime().expect("runtime");
    let rows = manifests(rt);
    assert_eq!(paths(&rows), vec!["lifecycle-module/pom.xml", "pom.xml"]);
    assert!(rows.iter().all(|m| m.status == "read" && m.format == "maven" && m.content_hash.is_some()));

    // The module inherits its group from the in-member parent, and the
    // parent's ${james.groupId} resolves through that chain.
    let module = manifest(&rows, "lifecycle-module/pom.xml");
    assert_eq!(
        coordinate(produced(module)),
        (Some("com.example.james"), Some("lifecycle-module"), Some("2.1.0"))
    );
    type Row<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>, &'a str);
    let facts: Vec<Row<'_>> = module
        .artifacts
        .iter()
        .map(|a| {
            (
                a.role.as_str(),
                a.kind.as_deref(),
                a.group_id.as_deref(),
                a.scope.as_deref(),
                a.resolution.as_str(),
            )
        })
        .collect();
    assert_eq!(
        facts,
        vec![
            ("produced", None, Some("com.example.james"), None, "resolved"),
            ("referenced", Some("parent"), Some("com.example.james"), None, "resolved"),
            ("referenced", Some("dependency"), Some("org.apache.james"), Some("runtime"), "resolved"),
            ("referenced", Some("dependency"), Some("org.projectlombok"), None, "version-refused"),
        ]
    );
    let lombok = &module.artifacts[3];
    assert!(
        lombok.reason.as_deref().unwrap().contains(
            "parent `org.springframework.boot:spring-boot-starter-parent` is not a pom of this member"
        ),
        "the refusal names the out-of-member end of the chain: {:?}",
        lombok.reason
    );

    let kinds: Vec<Option<&str>> = manifest(&rows, "pom.xml").artifacts.iter().map(|a| a.kind.as_deref()).collect();
    assert_eq!(kinds, vec![None, Some("parent"), Some("managed"), Some("bom-import")]);
}

#[test]
fn a_gitignored_manifest_is_not_read_and_an_unreadable_one_stays_in_the_denominator() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "src/lib.rs", "pub fn hello() {}\n");
    write(root, ".gitignore", "generated/\n");
    write(root, "generated/pom.xml", ROOT_POM);
    write(root, "pom.xml", ROOT_POM);
    fs::create_dir_all(root.join("latin1")).unwrap();
    fs::write(root.join("latin1/pom.xml"), b"<project><artifactId>caf\xe9</artifactId></project>").unwrap();

    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rows = manifests(engine.runtime().unwrap());
    assert_eq!(paths(&rows), vec!["latin1/pom.xml", "pom.xml"], "the gitignored pom is not a manifest found");
    let latin1 = manifest(&rows, "latin1/pom.xml");
    assert_eq!(latin1.status, "unreadable");
    assert!(latin1.detail.is_some() && latin1.content_hash.is_none() && latin1.artifacts.is_empty());
}

// ── a member with no build manifest is unaffected ────────────────────────────

/// Byte-for-byte apart from the two empty tables **and one `project_metadata`
/// row**: the extraction marker a full index records for every member, a
/// manifest-less one included, so "no manifests" is never confused with "never
/// extracted" (S-462 task 2). The graph tables are compared directly; the
/// marker is asserted on its own because `project_metadata` also holds clocked
/// rows.
#[test]
fn a_member_with_no_build_manifest_is_byte_for_byte_unaffected_apart_from_the_empty_tables() {
    let bare = tempfile::tempdir().unwrap();
    let with_pom = tempfile::tempdir().unwrap();
    for root in [bare.path(), with_pom.path()] {
        write(root, "src/lib.rs", "pub fn hello() {}\npub fn world() { hello(); }\n");
    }
    write(with_pom.path(), "pom.xml", ROOT_POM);

    for root in [bare.path(), with_pom.path()] {
        let engine = Engine::start(root).expect("engine starts");
        engine.index();
    }
    // Clock-stamped bookkeeping aside, the two stores differ in the build
    // tables and nowhere else: the manifest reaches no graph table.
    let clocked = ["schema_versions", "project_metadata"];
    let build = ["build_manifests", "build_artifacts"];
    let skip: Vec<&str> = clocked.iter().chain(&build).copied().collect();
    assert_eq!(
        dump(bare.path(), &skip),
        dump(with_pom.path(), &skip),
        "a pom changes no table outside build_manifests / build_artifacts"
    );
    let bare_build = dump(bare.path(), &[])
        .into_iter()
        .filter(|(t, _)| build.contains(&t.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        bare_build,
        vec![("build_artifacts".to_string(), vec![]), ("build_manifests".to_string(), vec![])],
        "a member with no build manifest has the two tables, empty"
    );
    for root in [bare.path(), with_pom.path()] {
        assert_eq!(
            extraction_marker(root).as_deref(),
            Some("1"),
            "a full index marks the facts extracted, with or without a manifest"
        );
    }

    // Neither a partial sync nor a full-walk reconcile writes anything for it:
    // the whole store — graph revision included — is unchanged by a no-op.
    let before = dump(bare.path(), &["schema_versions"]);
    let engine = Engine::start(bare.path()).expect("engine restarts");
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    engine.health(true).expect("a full-walk reconcile runs");
    drop(engine);
    assert_eq!(
        dump(bare.path(), &["schema_versions"]),
        before,
        "a no-op sync and reconcile on a manifest-less member write nothing"
    );
}

// ── only a full walk marks the facts extracted ───────────────────────────────

/// **An upgraded store reads "not yet extracted" until a full walk.** A store
/// written before migration 22 has no build facts and no marker. A partial sync
/// — even one that names the manifest and so records its facts — never writes
/// the marker: it saw a subset of the member. The first full-walk reconcile
/// does, for a member with a manifest and for one without, advancing the graph
/// revision; a second writes nothing at all.
#[test]
fn only_a_full_walk_marks_an_upgraded_members_build_facts_extracted() {
    for with_pom in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib.rs", "pub fn hello() {}\n");
        if with_pom {
            write(root, "pom.xml", ROOT_POM);
        }
        Engine::start(root).expect("engine starts").index();
        downgrade_to_v21(root);

        let engine = Engine::start(root).expect("the upgraded store opens at the latest version");
        let rt = engine.runtime().unwrap();
        assert!(manifests(rt).is_empty(), "migration 22 creates the tables empty");
        assert_eq!(extraction_marker(root), None, "an upgraded store records no extraction");

        write(root, "src/lib.rs", "pub fn hello() {}\npub fn more() {}\n");
        engine.sync(&[PathBuf::from("src/lib.rs"), PathBuf::from("pom.xml")]);
        assert_eq!(manifests(rt).len(), usize::from(with_pom), "the partial sync read what it named");
        assert_eq!(
            extraction_marker(root),
            None,
            "a partial sync never marks the facts extracted (with_pom = {with_pom})"
        );

        let before = revision(root);
        engine.health(true).expect("a full-walk reconcile runs");
        assert_eq!(
            extraction_marker(root).as_deref(),
            Some("1"),
            "the first full walk marks the facts extracted (with_pom = {with_pom})"
        );
        assert_ne!(
            revision(root),
            before,
            "marking the facts extracted moves what a reader of them sees, so it advances the \
             graph revision as a manifest change does (with_pom = {with_pom})"
        );
        assert_eq!(manifests(rt).len(), usize::from(with_pom));

        let before = dump(root, &["schema_versions"]);
        engine.health(true).expect("a second full-walk reconcile runs");
        assert_eq!(
            dump(root, &["schema_versions"]),
            before,
            "once marked, a no-op full walk writes nothing (with_pom = {with_pom})"
        );
    }
}

// ── a changed manifest resyncs only that member's facts ──────────────────────

#[test]
fn a_sync_re_derives_the_facts_only_when_it_names_a_changed_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    // The parent redefines the property its module inherits.
    write(root, "pom.xml", &ROOT_POM.replace("org.apache.james", "org.forked.james"));

    // A sync that names only source leaves the build facts as they were — the
    // pass costs an ordinary edit nothing.
    write(root, "src/lib.rs", "pub fn hello() {}\npub fn more() {}\n");
    let before = revision(root);
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    let module = manifest(&manifests(rt), "lifecycle-module/pom.xml").clone();
    assert_eq!(
        module.artifacts[2].group_id.as_deref(),
        Some("org.apache.james"),
        "a sync naming no manifest does not re-read one"
    );
    assert_ne!(revision(root), before, "the source edit itself advanced the revision");

    // Naming the changed PARENT re-derives the whole member: the unchanged
    // module's inherited group moves with it.
    let before = revision(root);
    engine.sync(&[PathBuf::from("pom.xml")]);
    let rows = manifests(rt);
    assert_eq!(
        manifest(&rows, "lifecycle-module/pom.xml").artifacts[2].group_id.as_deref(),
        Some("org.forked.james"),
        "a parent change re-derives its in-member children"
    );
    assert_ne!(revision(root), before, "a manifest change advances the graph revision");

    // Naming an UNCHANGED manifest rewrites nothing and advances nothing.
    let before = (revision(root), manifests(rt));
    engine.sync(&[PathBuf::from("pom.xml")]);
    assert_eq!((revision(root), manifests(rt)), before, "an unchanged manifest is a no-op");
}

#[test]
fn an_added_or_deleted_manifest_is_reconciled_by_a_partial_sync_and_by_a_full_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    // Partial: a deleted manifest named by the sync leaves the facts.
    fs::remove_file(root.join("lifecycle-module/pom.xml")).unwrap();
    engine.sync(&[PathBuf::from("lifecycle-module/pom.xml")]);
    assert_eq!(paths(&manifests(rt)), vec!["pom.xml"]);

    // Full walk: a manifest added without being named is found by the walk…
    write(root, "gradle-svc/build.gradle", "group = 'com.example.g'\ndependencies { api 'com.example.james:parent:2.1.0' }\n");
    engine.health(true).expect("reconcile runs");
    let rows = manifests(rt);
    assert_eq!(paths(&rows), vec!["gradle-svc/build.gradle", "pom.xml"]);
    let gradle = manifest(&rows, "gradle-svc/build.gradle");
    assert_eq!((gradle.format.as_str(), produced(gradle).group_id.as_deref()), ("gradle", Some("com.example.g")));

    // …and one deleted without being named is dropped by it.
    fs::remove_file(root.join("pom.xml")).unwrap();
    engine.health(true).expect("reconcile runs");
    assert_eq!(paths(&manifests(rt)), vec!["gradle-svc/build.gradle"]);
}

#[test]
fn a_gitignored_manifest_named_by_a_sync_is_treated_as_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let rt = engine.runtime().unwrap();

    write(root, ".gitignore", "lifecycle-module/\n");
    engine.sync(&[PathBuf::from("lifecycle-module/pom.xml")]);
    assert_eq!(
        paths(&manifests(rt)),
        vec!["pom.xml"],
        "a manifest the walk would no longer admit leaves the facts, as its source would"
    );
}

/// A full index is authoritative: when the walk finds no manifest any more, the
/// facts an earlier index recorded are cleared, not left standing.
#[test]
fn a_re_index_after_every_manifest_is_deleted_clears_the_facts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    member(root);
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    assert_eq!(manifests(engine.runtime().unwrap()).len(), 2);

    fs::remove_file(root.join("pom.xml")).unwrap();
    fs::remove_file(root.join("lifecycle-module/pom.xml")).unwrap();
    engine.index();
    assert!(
        manifests(engine.runtime().unwrap()).is_empty(),
        "a re-index that finds no manifest leaves none recorded"
    );
}
