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
}
