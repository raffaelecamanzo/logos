//! S-463 ([FR-WS-33], [ADR-69] points 2–3) end to end: members really indexed
//! with Maven poms, entered through [`discover`] over a written manifest — the
//! path `logos workspace status` takes — build against each other in a relation
//! counted apart from every runtime figure ([BR-58]).
//!
//! The fixture is the reference estate's dominant shape in miniature: a parent
//! POM (`starter`) every other member inherits and a shared library (`lib`) two
//! members depend on, beside real runtime coupling (`api` holds an OpenAPI
//! document that `web` serves in Rust), so the runtime figures the relation must
//! not move are non-trivial.
//!
//! [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
//! [ADR-69]: ../../docs/specs/architecture/decisions/ADR-69.md
//! [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{
    discover, workspace_status, BuildDependencies, ContractBridge, EngineRegistry, RegistryMode,
};
use logos_core::graph_store::{BUILD_FACTS_EXTRACTED_KEY, DECLARED_TYPES_EXTRACTED_KEY};
use logos_core::Engine;

#[path = "support/bridge_reads.rs"]
mod bridge_reads;

const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
";

const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{id}", get(get_user))
}
"#;

const G: &str = "com.acme";

fn pom(artifact: &str, parent: Option<&str>, dependencies: &[&str]) -> String {
    let parent = parent.map_or(String::new(), |p| {
        format!("  <parent><groupId>{G}</groupId><artifactId>{p}</artifactId><version>1</version></parent>\n")
    });
    let deps: String = dependencies
        .iter()
        .map(|d| format!("    <dependency><groupId>{G}</groupId><artifactId>{d}</artifactId></dependency>\n"))
        .collect();
    format!(
        "<project>\n{parent}  <groupId>{G}</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>1</version>\n  <dependencies>\n{deps}  </dependencies>\n</project>\n"
    )
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

fn git_init(dir: &Path) {
    fs::create_dir_all(dir).expect("mkdir member");
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .expect("git runs");
    assert!(status.success(), "git init {}", dir.display());
}

fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// The four members, with or without their poms. Every member carries its
/// source either way, so the only difference between the two is the manifests.
fn workspace(root: &Path, with_poms: bool) {
    let members: [(&str, Option<String>); 4] = [
        ("starter", with_poms.then(|| pom("starter", None, &[]))),
        ("lib", with_poms.then(|| pom("lib", Some("starter"), &[]))),
        ("api", with_poms.then(|| pom("api", Some("starter"), &["lib"]))),
        ("web", with_poms.then(|| pom("web", Some("starter"), &["lib", "commons-lang3"]))),
    ];
    for (name, pom) in &members {
        let dir = root.join(name);
        git_init(&dir);
        match *name {
            "api" => write(&dir, "api/openapi.yaml", OPENAPI_YAML),
            "web" => write(&dir, "src/main.rs", AXUM_MAIN),
            _ => write(&dir, "src/lib.rs", "pub fn f() {}\n"),
        }
        if let Some(pom) = pom {
            write(&dir, "pom.xml", pom);
        }
        index_member(&dir);
    }
}

const MANIFEST: &str = "[workspace]\nname = \"acme\"\nmembers = [\"starter\", \"lib\", \"api\", \"web\"]\n";

fn registry_over(root: &Path, manifest: &str) -> EngineRegistry<Engine> {
    fs::write(root.join("logos.workspace.toml"), manifest).expect("write manifest");
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

fn status_over(root: &Path, manifest: &str) -> serde_json::Value {
    serde_json::to_value(workspace_status(&registry_over(root, manifest))).expect("serializes")
}

/// **The relation on the real read-model.** `workspace status` carries the
/// headline by kind beside its denominator; the lazily-built relation over a
/// fresh registry agrees with it — one join, two entry points.
#[test]
fn status_carries_the_build_dependency_headline_joined_from_indexed_poms() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), true);
    let status = status_over(tmp.path(), MANIFEST);
    let headline = &status["build_dependency"];

    assert_eq!(
        headline["build_dependency_pairs"],
        serde_json::json!({"pairs": 5, "parent": 3, "dependency": 2, "managed": 0, "bom-import": 0}),
        "{headline:#}"
    );
    let references = &headline["references"];
    assert_eq!(references["references"], 6);
    assert_eq!(references["to_member"], 5);
    assert_eq!(references["external"], 1, "commons-lang3 is produced by no member");
    assert_eq!(headline["members"]["read"], 4);
    assert_eq!(headline["members"]["members"], 4);
    assert_eq!(headline["members"]["manifests_read"], 4);
    assert!(headline.get("platform_apart").is_none());
    assert_eq!(
        headline["platform_candidates"],
        serde_json::json!([
            {"member": "starter", "in_degree": 3, "of": 3},
            {"member": "lib", "in_degree": 2, "of": 3},
        ])
    );

    let relation = BuildDependencies::new().relation(&registry_over(tmp.path(), MANIFEST));
    assert_eq!(
        serde_json::to_value(&relation.headline).unwrap(),
        *headline,
        "the lazy relation and the status headline are one join"
    );
    let lib = relation.member("lib").expect("read");
    assert_eq!(
        lib.built_against_by.iter().map(|e| e.from.as_str()).collect::<Vec<_>>(),
        ["api", "web"]
    );
}

/// **`kind = "platform"` through the manifest.** Declared, `starter`'s three
/// inbound parent pairs leave the headline for `platform_apart`, and it is no
/// longer listed as a candidate; the denominator does not move.
#[test]
fn a_manifest_declared_platform_has_its_inbound_pairs_counted_apart() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), true);
    let declared = format!("{MANIFEST}\n[workspace.member.starter]\nkind = \"platform\"\n");
    let status = status_over(tmp.path(), &declared);
    let headline = &status["build_dependency"];

    assert_eq!(
        headline["build_dependency_pairs"],
        serde_json::json!({"pairs": 2, "parent": 0, "dependency": 2, "managed": 0, "bom-import": 0})
    );
    assert_eq!(headline["platform_apart"]["members"], serde_json::json!(["starter"]));
    assert_eq!(headline["platform_apart"]["build_dependency_pairs"]["parent"], 3);
    assert_eq!(headline["references"]["references"], 6);
    assert_eq!(
        headline["platform_candidates"],
        serde_json::json!([{"member": "lib", "in_degree": 2, "of": 3}])
    );
    // The declaration moved no contract-surface row: platform stays in the headline.
    assert!(status["coverage"].get("declared_apart").is_none());
}

/// **Byte-identical runtime figures** ([BR-58], [ADR-26]). The same workspace
/// with and without its poms: the coverage payload (which carries
/// `resolved_cross_service_edges` and `egress_resolution`), the topic inventory
/// and the bridge edge set serialize to the same bytes, and the pom-less one
/// carries no `build_dependency` key at all.
///
/// [BR-58]: ../../docs/specs/software-spec.md#327-workspace-federation
/// [ADR-26]: ../../docs/specs/architecture/decisions/ADR-26.md
#[test]
fn runtime_figures_are_byte_identical_with_and_without_build_manifests() {
    let with = tempfile::tempdir().unwrap();
    let without = tempfile::tempdir().unwrap();
    workspace(with.path(), true);
    workspace(without.path(), false);

    let a = status_over(with.path(), MANIFEST);
    let b = status_over(without.path(), MANIFEST);
    assert!(a.get("build_dependency").is_some());
    assert!(b.get("build_dependency").is_none(), "no manifest, no key: {b:#}");
    assert!(
        a["coverage"]["resolved_cross_service_edges"].as_u64().is_some(),
        "the figure under test is present"
    );
    for key in ["coverage", "topics", "kind_candidates", "warm_rollup", "degraded_rollup"] {
        assert_eq!(
            serde_json::to_string(&a[key]).unwrap(),
            serde_json::to_string(&b[key]).unwrap(),
            "{key} moved with build manifests present"
        );
    }

    let edges = |root: &Path| {
        let registry = registry_over(root, MANIFEST);
        serde_json::to_string(&*ContractBridge::new().edges(&registry)).unwrap()
    };
    let (edges_with, edges_without) = (edges(with.path()), edges(without.path()));
    assert!(edges_with.contains("GET /users/{id}"), "the bridge binds the route: {edges_with}");
    assert_eq!(edges_with, edges_without);

    bridge_reads::assert_narrowed_read_changes_no_answer(&registry_over(with.path(), MANIFEST));
    bridge_reads::assert_narrowed_read_changes_no_answer(&registry_over(without.path(), MANIFEST));
}

/// Take a member store back to what the release before migration 22 left on
/// disk: both build tables absent (their one index goes with them), migration
/// 22 unrecorded, `user_version` 21 — and so no extraction marker. The exact
/// inverse of migrations 22, 23 (S-487, only adds
/// `metric_snapshots.modularity_applicable`), 24 (S-472, two declared-type
/// tables and their marker), 25 (S-500, two `nodes` columns), 26 (S-498,
/// the snapshot offender table and its flag column), 27 (S-513, the
/// persist-failure record) and 28 (S-493, the `nodes.self_type` column); the
/// next open re-applies all seven, as a real upgrade does. Duplicated from `build_manifest_facts.rs` (no shared test module).
fn downgrade_to_v21(member: &Path) {
    let conn = rusqlite::Connection::open(member.join(".logos").join("logos.db")).unwrap();
    conn.execute_batch(&format!(
        "ALTER TABLE nodes DROP COLUMN self_type; DELETE FROM schema_versions WHERE version = 28; \
         DROP TABLE persist_failures; DELETE FROM schema_versions WHERE version = 27; \
         DROP TABLE metric_snapshot_offenders; ALTER TABLE metric_snapshots DROP COLUMN offenders_recorded; \
         DELETE FROM schema_versions WHERE version = 26; \
         ALTER TABLE nodes DROP COLUMN body_tokens; ALTER TABLE nodes DROP COLUMN has_body; \
         DELETE FROM schema_versions WHERE version = 25; \
         DROP TABLE declared_types; DROP TABLE avro_schemas; \
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

fn user_version(member: &Path) -> i64 {
    let db = member.join(".logos").join("logos.db");
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap()
}

/// **An upgraded workspace never reads as "no manifests"** ([FR-WS-33],
/// [NFR-CC-04]; the Sprint 80 review's D1). Three members indexed by the
/// release before migration 22 — `lib`, `app` building against it, and a
/// manifest-less `docs`:
///
/// 1. opened at the latest version (v24), every member is **unread** with the reason "build facts
///    not yet extracted", never "read, 0 manifests" — and the section is
///    present, because an unread member could hold manifests nobody saw;
/// 2. a partial sync naming `app`'s pom records its facts but not the marker,
///    so `app` stays unread;
/// 3. after one full-walk reconcile per member the facts are there, every
///    member reads, and `docs` is read with 0 manifests.
///
/// [FR-WS-33]: ../../docs/specs/requirements/FR-WS-33.md
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
#[test]
fn an_upgraded_member_reads_unread_with_its_reason_until_a_full_walk_extracts_its_facts() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let members = [
        ("lib", Some(pom("lib", None, &[]))),
        ("app", Some(pom("app", None, &["lib"]))),
        ("docs", None),
    ];
    for (name, pom) in &members {
        let dir = root.join(name);
        git_init(&dir);
        write(&dir, "src/lib.rs", "pub fn f() {}\n");
        if let Some(pom) = pom {
            write(&dir, "pom.xml", pom);
        }
        index_member(&dir);
        downgrade_to_v21(&dir);
    }
    let manifest = "[workspace]\nname = \"acme\"\nmembers = [\"lib\", \"app\", \"docs\"]\n";
    let not_extracted = serde_json::json!("build facts not yet extracted");

    let status = status_over(root, manifest);
    for (name, _) in &members {
        assert_eq!(user_version(&root.join(name)), 28, "{name} was opened at the latest version (v28)");
    }
    let section = status.get("build_dependency").expect("an unread member keeps the section");
    assert_eq!(section["members"]["read"], 0, "{section:#}");
    assert_eq!(section["members"]["unread"], serde_json::json!(["lib", "app", "docs"]));
    for (name, _) in &members {
        assert_eq!(section["members"]["unread_reasons"][name], not_extracted, "{section:#}");
    }
    assert_eq!(section["members"]["with_manifests"], 0);
    assert!(section["summary"].as_str().unwrap().contains("over 0 of 3 members read"));

    // A partial sync sees a subset of the member: facts, but no marker.
    let app = root.join("app");
    Engine::start(&app).expect("app opens").sync(&[PathBuf::from("pom.xml")]);
    let status = status_over(root, manifest);
    let section = &status["build_dependency"];
    assert_eq!(section["members"]["unread"], serde_json::json!(["lib", "app", "docs"]));
    assert_eq!(section["members"]["unread_reasons"]["app"], not_extracted);

    for (name, _) in &members {
        Engine::start(root.join(name))
            .expect("member opens")
            .health(true)
            .expect("a full-walk reconcile runs");
    }
    let status = status_over(root, manifest);
    let section = &status["build_dependency"];
    let read = &section["members"];
    assert_eq!((&read["read"], &read["members"]), (&serde_json::json!(3), &serde_json::json!(3)));
    assert!(read.get("unread").is_none() && read.get("unread_reasons").is_none(), "{read:#}");
    assert_eq!((&read["with_manifests"], &read["manifests"]), (&serde_json::json!(2), &serde_json::json!(2)));
    assert_eq!(
        section["build_dependency_pairs"],
        serde_json::json!({"pairs": 1, "parent": 0, "dependency": 1, "managed": 0, "bom-import": 0}),
        "{section:#}"
    );
}

/// **A manifest-less workspace, fully indexed, reads every member with 0
/// manifests** and so carries no `build_dependency` key: a full index marks
/// the facts extracted whether or not it found a manifest.
#[test]
fn a_fully_indexed_manifest_less_workspace_reads_every_member_and_shows_no_section() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), false);
    let status = status_over(tmp.path(), MANIFEST);
    assert!(status.get("build_dependency").is_none(), "{status:#}");
    let relation = BuildDependencies::new().relation(&registry_over(tmp.path(), MANIFEST));
    let read = &relation.headline.members;
    assert_eq!((read.read, read.members, read.with_manifests), (4, 4, 0));
    assert!(read.unread.is_empty() && read.unread_reasons.is_empty(), "{read:?}");
}

/// **A never-indexed member is "not yet extracted" too** — nothing has read its
/// manifests, so it is unread with that reason and keeps the section present,
/// on a workspace whose only other member holds no manifest at all. One full
/// index and it reads, with 0 manifests, and the section goes.
#[test]
fn a_never_indexed_member_is_unread_not_yet_extracted_until_it_is_indexed() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for name in ["indexed", "cold"] {
        let dir = root.join(name);
        git_init(&dir);
        write(&dir, "src/lib.rs", "pub fn f() {}\n");
    }
    index_member(&root.join("indexed"));
    let manifest = "[workspace]\nname = \"acme\"\nmembers = [\"indexed\", \"cold\"]\n";

    let status = status_over(root, manifest);
    let read = &status["build_dependency"]["members"];
    assert_eq!(read["unread"], serde_json::json!(["cold"]), "{status:#}");
    assert_eq!(read["unread_reasons"]["cold"], "build facts not yet extracted");
    assert_eq!((&read["read"], &read["with_manifests"]), (&serde_json::json!(1), &serde_json::json!(0)));

    index_member(&root.join("cold"));
    let status = status_over(root, manifest);
    assert!(status.get("build_dependency").is_none(), "every member read, none holds a manifest: {status:#}");
}
