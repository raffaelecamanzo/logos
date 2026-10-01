//! S-473 ([FR-WS-35], [ADR-70], [BR-60]) end to end: members really indexed
//! with Java sources, an Avro schema and Maven poms, entered through
//! [`discover`] over a written manifest — the path `logos workspace status`
//! takes — bind an import of another member's type in an advisory tier, apart
//! from every runtime and build figure.
//!
//! The fixture is the reference estate's shapes in miniature:
//!
//! - `lib` (`com.acme:lib`) declares `com.acme.lib.Dto`; `models`
//!   (`com.acme:models`) declares the Avro record `com.acme.events.Evt`;
//! - `app` depends on both and imports both — **bound** — and its own
//!   `Helper`, which binds inside `app` and so is never considered;
//! - `stray` builds against nothing and imports `Dto` — **type-only**;
//! - `fork-a` and `fork-b` both produce `com.acme:dup` (a build collision) and
//!   both declare `com.acme.dup.Thing`; only `fork-a` declares
//!   `com.acme.dup.Only`; `user` depends on `com.acme:dup` and imports both —
//!   `Thing` stays **ambiguous-owner**, `Only` binds **collision-backed**;
//! - beside them, `api` holds an OpenAPI document `web` serves in Rust, so the
//!   runtime figures the overlay must not move are non-trivial.
//!
//! [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
//! [ADR-70]: ../../docs/specs/architecture/decisions/ADR-70.md
//! [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#![cfg(all(feature = "lang-yaml", feature = "lang-rust", feature = "lang-java"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use logos_core::federation::{
    discover, workspace_status, BuildDependencies, ContractBridge, EngineRegistry, PairEvidence,
    RegistryMode, TypeOrigin, TypeReferences,
};
use logos_core::graph_store::DECLARED_TYPES_EXTRACTED_KEY;
use logos_core::Engine;

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

const EVT_AVSC: &str = r#"{"type": "record", "name": "Evt", "namespace": "com.acme.events", "fields": []}"#;

fn pom(artifact: &str, dependencies: &[&str]) -> String {
    let deps: String = dependencies
        .iter()
        .map(|d| format!("    <dependency><groupId>com.acme</groupId><artifactId>{d}</artifactId></dependency>\n"))
        .collect();
    format!(
        "<project>\n  <groupId>com.acme</groupId>\n  <artifactId>{artifact}</artifactId>\n  \
         <version>1</version>\n  <dependencies>\n{deps}  </dependencies>\n</project>\n"
    )
}

/// A main-tree Java file declaring `class` in `package`, importing `imports`.
fn java(package: &str, class: &str, imports: &[&str]) -> (String, String) {
    let path = format!("src/main/java/{}/{class}.java", package.replace('.', "/"));
    let imports: String = imports.iter().map(|i| format!("import {i};\n")).collect();
    (path, format!("package {package};\n\n{imports}\npublic class {class} {{}}\n"))
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

const MEMBERS: [&str; 9] = ["lib", "models", "app", "stray", "fork-a", "fork-b", "user", "api", "web"];

/// The JVM members' files: `(member, [(path, contents)])`.
fn jvm_files() -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        ("lib", vec![java("com.acme.lib", "Dto", &[]), ("pom.xml".into(), pom("lib", &[]))]),
        (
            "models",
            vec![("src/main/avro/evt.avsc".into(), EVT_AVSC.into()), ("pom.xml".into(), pom("models", &[]))],
        ),
        (
            "app",
            vec![
                java(
                    "com.acme.app",
                    "App",
                    &["com.acme.lib.Dto", "com.acme.events.Evt", "java.util.List", "com.acme.app.Helper"],
                ),
                java("com.acme.app", "Helper", &[]),
                ("pom.xml".into(), pom("app", &["lib", "models"])),
            ],
        ),
        (
            "stray",
            vec![java("com.acme.stray", "Stray", &["com.acme.lib.Dto"]), ("pom.xml".into(), pom("stray", &[]))],
        ),
        (
            "fork-a",
            vec![
                java("com.acme.dup", "Thing", &[]),
                java("com.acme.dup", "Only", &[]),
                ("pom.xml".into(), pom("dup", &[])),
            ],
        ),
        ("fork-b", vec![java("com.acme.dup", "Thing", &[]), ("pom.xml".into(), pom("dup", &[]))]),
        (
            "user",
            vec![
                java("com.acme.user", "User", &["com.acme.dup.Thing", "com.acme.dup.Only"]),
                ("pom.xml".into(), pom("user", &["dup"])),
            ],
        ),
    ]
}

/// Write and index the workspace's `members`; `jvm` adds the Java/Avro
/// members' files (without it they hold one Rust file).
fn workspace(root: &Path, members: &[&str], jvm: bool) {
    let files = jvm_files();
    for name in members {
        let dir = root.join(name);
        git_init(&dir);
        match *name {
            "api" => write(&dir, "api/openapi.yaml", OPENAPI_YAML),
            "web" => write(&dir, "src/main.rs", AXUM_MAIN),
            _ if jvm => {
                for (path, contents) in &files.iter().find(|(m, _)| m == name).expect("a JVM member").1 {
                    write(&dir, path, contents);
                }
            }
            _ => write(&dir, "src/lib.rs", "pub fn f() {}\n"),
        }
        index_member(&dir);
    }
}

fn manifest(members: &[&str]) -> String {
    let list: Vec<String> = members.iter().map(|m| format!("\"{m}\"")).collect();
    format!("[workspace]\nname = \"acme\"\nmembers = [{}]\n", list.join(", "))
}

fn registry_over(root: &Path, members: &[&str]) -> EngineRegistry<Engine> {
    fs::write(root.join("logos.workspace.toml"), manifest(members)).expect("write manifest");
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

fn status_over(root: &Path, members: &[&str]) -> serde_json::Value {
    serde_json::to_value(workspace_status(&registry_over(root, members))).expect("serializes")
}

/// Withhold every member's declared-type facts — the overlay's only input
/// beyond the ledger — so it has nothing to bind: the "overlay off" state.
fn withhold_declared_types(member: &Path) {
    let conn = rusqlite::Connection::open(member.join(".logos").join("logos.db")).unwrap();
    conn.execute_batch(&format!(
        "DELETE FROM declared_types; DELETE FROM avro_schemas; \
         DELETE FROM project_metadata WHERE key = '{DECLARED_TYPES_EXTRACTED_KEY}';"
    ))
    .expect("withhold the declared types");
}

/// **The section on the real read-model** ([FR-WS-35]): bound, type-only,
/// ambiguous-owner and collision-backed, each from indexed source, beside the
/// rows considered and the members read; the lazily-built index over a fresh
/// registry agrees with it — one builder, two entry points.
///
/// [FR-WS-35]: ../../docs/specs/requirements/FR-WS-35.md
#[test]
fn status_carries_the_type_reference_section_built_from_indexed_members() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), &MEMBERS, true);
    let status = status_over(tmp.path(), &MEMBERS);
    let section = &status["type_reference"];

    assert_eq!(section["type_reference_pairs"], 3, "app→lib, app→models, user→fork-a: {section:#}");
    assert_eq!((&section["build_pairs"], &section["collision_backed_pairs"]), (&2.into(), &1.into()));
    let rows = &section["rows"];
    assert_eq!(rows["considered"], 6, "app's import of its own Helper binds in-member, so is not considered: {rows:#}");
    assert_eq!(rows["self_owned"], 0, "{rows:#}");
    assert_eq!(rows["bound"], 3, "{rows:#}");
    assert_eq!(rows["type_only"], 1);
    assert_eq!(rows["ambiguous_owner"], 1);
    assert_eq!(rows["pair_unread"], 0);
    assert_eq!(rows["no_owner"], 1, "java.util.List");
    assert_eq!(section["members"]["read"], 9);
    assert_eq!(section["members"]["members"], 9);
    assert_eq!(section["members"]["java_kotlin_avro"], 7, "api and web hold none");
    assert_eq!(
        section["collision_backed"],
        serde_json::json!([{"from": "user", "to": "fork-a", "artifacts": ["com.acme:dup"], "references": 1}])
    );
    assert_eq!(
        section["type_only"],
        serde_json::json!([{"from": "stray", "to": "lib", "types": ["com.acme.lib.Dto"], "references": 1}])
    );
    assert_eq!(
        section["ambiguous_owner"],
        serde_json::json!([{"fqn": "com.acme.dup.Thing", "owners": ["fork-a", "fork-b"], "references": 1}])
    );

    let registry = registry_over(tmp.path(), &MEMBERS);
    let index = TypeReferences::new().index(&registry, &BuildDependencies::new());
    assert_eq!(
        serde_json::to_value(&index.headline).unwrap(),
        *section,
        "the lazy index and the status section are one builder"
    );
}

/// **A bound reference names both ends' provenance**: the importing file and
/// line, and the declaring file and node — or, for an Avro type, its schema.
#[test]
fn a_bound_reference_names_the_importing_line_and_the_declaring_file_or_schema() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), &MEMBERS, true);
    let registry = registry_over(tmp.path(), &MEMBERS);
    let index = TypeReferences::new().index(&registry, &BuildDependencies::new());

    let dto = index.importers("com.acme.lib.Dto").next().expect("app's import binds");
    assert_eq!(dto.importer.member, "app");
    assert_eq!(dto.importer.file, "src/main/java/com/acme/app/App.java");
    assert_eq!(dto.importer.line, Some(3), "the import's own line");
    assert_eq!(dto.owner.member, "lib");
    assert_eq!(dto.owner.declared_in, "src/main/java/com/acme/lib/Dto.java");
    let symbol = dto.owner.symbol.as_deref().expect("a source type has a node");
    assert_eq!(index.references_to("lib", symbol).count(), 1);
    assert_eq!(dto.evidence, PairEvidence::Build { platform: false });

    let evt = index.importers("com.acme.events.Evt").next().expect("app's import binds");
    assert_eq!((evt.owner.member.as_str(), evt.owner.origin), ("models", TypeOrigin::Avro));
    assert_eq!(evt.owner.declared_in, "src/main/avro/evt.avsc");
    assert_eq!(evt.owner.symbol, None);
}

/// **Byte-identity, overlay on and off** ([BR-60]). The same workspace with
/// and without its declared-type facts: the coverage payload (which carries
/// `resolved_cross_service_edges` and `egress_resolution`), the topic
/// inventory, the build headline and the bridge edge set serialize to the same
/// bytes, and so does every member's `scan` and `gate`; only the
/// `type_reference` section moves.
///
/// [BR-60]: ../../docs/specs/software-spec.md#327-workspace-federation
#[test]
fn runtime_build_and_member_figures_are_byte_identical_with_the_overlay_on_and_off() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    workspace(root, &MEMBERS, true);

    let figures = |root: &Path| {
        let status = status_over(root, &MEMBERS);
        let registry = registry_over(root, &MEMBERS);
        let edges = serde_json::to_string(&*ContractBridge::new().edges(&registry)).unwrap();
        let members: Vec<String> = MEMBERS
            .iter()
            .map(|m| {
                let engine = Engine::start(root.join(m)).expect("member opens");
                let scan = serde_json::to_string(&engine.scan(false).expect("scan runs")).unwrap();
                let gate = serde_json::to_string(&engine.gate(None, false, false).expect("gate runs")).unwrap();
                format!("{m}: {scan} {gate}")
            })
            .collect();
        (status, edges, members)
    };

    let (on, edges_on, members_on) = figures(root);
    for member in MEMBERS {
        withhold_declared_types(&root.join(member));
    }
    let (off, edges_off, members_off) = figures(root);

    assert_eq!(on["type_reference"]["rows"]["bound"], 3, "the overlay is on: {:#}", on["type_reference"]);
    assert_eq!(off["type_reference"]["members"]["read"], 0, "…and off: {:#}", off["type_reference"]);
    assert!(on["coverage"]["resolved_cross_service_edges"].as_u64().is_some());
    assert!(on["build_dependency"]["build_dependency_pairs"]["pairs"].as_u64().unwrap() > 0);
    for key in ["coverage", "topics", "build_dependency", "kind_candidates", "warm_rollup", "degraded_rollup"] {
        assert_eq!(
            serde_json::to_string(&on[key]).unwrap(),
            serde_json::to_string(&off[key]).unwrap(),
            "{key} moved with the overlay"
        );
    }
    assert!(edges_on.contains("GET /users/{id}"), "the bridge binds the route: {edges_on}");
    assert_eq!(edges_on, edges_off, "the bridge edge set moved with the overlay");
    assert_eq!(members_on, members_off, "a member's scan or gate moved with the overlay");
}

/// **A workspace with no Java/Kotlin/Avro member** prints `workspace status`
/// with no `type_reference` key — the pre-overlay top-level keys only.
#[test]
fn a_workspace_with_no_java_kotlin_or_avro_member_carries_no_type_reference_section() {
    let tmp = tempfile::tempdir().unwrap();
    let members = ["lib", "api", "web"];
    workspace(tmp.path(), &members, false);
    let status = status_over(tmp.path(), &members);
    let mut keys: Vec<&str> = status.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["coverage", "degraded_rollup", "kind_candidates", "members", "topics", "warm_rollup", "workspace"],
        "{status:#}"
    );
}

/// **An upgraded member is unread, never "declares nothing"**: with one
/// member's facts withheld, the section names it unread with its reason and
/// its type binds nobody — `app`'s import of `Dto` is then no owner.
#[test]
fn a_member_whose_declared_types_are_not_extracted_is_named_unread() {
    let tmp = tempfile::tempdir().unwrap();
    workspace(tmp.path(), &MEMBERS, true);
    withhold_declared_types(&tmp.path().join("lib"));
    let status = status_over(tmp.path(), &MEMBERS);
    let members = &status["type_reference"]["members"];
    assert_eq!(members["unread"], serde_json::json!(["lib"]), "{members:#}");
    assert_eq!(members["unread_reasons"]["lib"], "declared types not yet extracted");
    assert!(
        status["type_reference"]["summary"].as_str().unwrap().contains("over 8 of 9 members read"),
        "{:#}",
        status["type_reference"]
    );
    assert_eq!(status["type_reference"]["type_reference_pairs"], 2, "app→models, user→fork-a");
}
