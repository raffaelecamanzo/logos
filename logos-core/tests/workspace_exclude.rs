//! End-to-end test for `[workspace] exclude` (S-707, [CR-223], [FR-WS-40]).
//!
//! The scenario the key exists for: a fork re-exposes its original's route, so
//! the bridge's exactly-one rule sees two providers and drops the edge without
//! anything visibly failing. Excluding the fork — durably, in the manifest —
//! turns the ambiguous reference back into an exactly-one binding, and
//! `workspace status` says what it excluded.
//!
//! The members are real indexed repositories behind the real [`discover`] and
//! [`EngineRegistry`]; nothing here builds a [`Federation`] by hand.
//!
//! Gated on both grammars so a build excluding either does not run it.
//!
//! [CR-223]: ../../docs/requests/CR-223-workspace-manifest-excludes-members.md
//! [FR-WS-40]: ../../docs/specs/requirements/FR-WS-40.md
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    discover, workspace_status, ContractBridge, EngineRegistry, RegistryMode, MANIFEST_FILENAME,
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

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member into its own `.logos/logos.db` (which also makes the plain
/// directory an autodiscovery candidate: a child carrying a store is a member).
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
}

/// `api` declares the operation; `web` provides the route; `web-fork` is a
/// copy of `web` — a second provider of the same key.
fn fork_workspace(root: &Path) {
    write(&root.join("api"), "api/openapi.yaml", OPENAPI_YAML);
    write(&root.join("web"), "src/main.rs", AXUM_MAIN);
    write(&root.join("web-fork"), "src/main.rs", AXUM_MAIN);
    for member in ["api", "web", "web-fork"] {
        index_member(&root.join(member));
    }
}

fn manifest(root: &Path, exclude_line: &str) {
    fs::write(
        root.join(MANIFEST_FILENAME),
        format!("[workspace]\nname = \"shop\"\n{exclude_line}\n[workspace.autodiscover]\n"),
    )
    .expect("write manifest");
}

fn registry(root: &Path) -> EngineRegistry<Engine> {
    let federation = discover(root).expect("manifest parses").expect("a workspace");
    EngineRegistry::<Engine>::new(federation, RegistryMode::Lazy)
}

#[test]
fn excluding_the_fork_turns_an_ambiguous_reference_into_an_exactly_one_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fork_workspace(root);

    // BEFORE: the fork is autodiscovered, so the route has two providers and
    // the operation binds to neither.
    manifest(root, "");
    let before = registry(root);
    let names: Vec<_> = before.federation().members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["api", "web", "web-fork"], "the fork joined by autodiscovery");
    let edges = ContractBridge::new().edges(&before);
    assert!(edges.is_empty(), "two providers of one key are ambiguous: {edges:?}");
    let status = serde_json::to_value(workspace_status(&before)).unwrap();
    assert!(status.get("exclusion").is_none(), "no key, no exclusion block: {status}");

    // AFTER: `exclude = ["*-fork"]` — the fork is not a member, the reference
    // binds to exactly one provider, and status lists what was excluded.
    manifest(root, "exclude = [\"*-fork\"]\n");
    let after = registry(root);
    let names: Vec<_> = after.federation().members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["api", "web"], "the fork is absent from the member set");
    let edges = ContractBridge::new().edges(&after);
    assert_eq!(edges.len(), 1, "exactly-one binding: {edges:?}");
    assert_eq!(edges[0].from.member, "api");
    assert_eq!(edges[0].to.member, "web", "bound to the original, not the fork");

    let status = serde_json::to_value(workspace_status(&after)).unwrap();
    let roster: Vec<&str> = status["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["member"].as_str().unwrap())
        .collect();
    assert_eq!(roster, ["api", "web"], "absent from status fan-out and coverage");
    assert_eq!(
        status["exclusion"],
        serde_json::json!({ "globs": ["*-fork"], "excluded": ["web-fork"], "conflicts": [] }),
        "status lists the directory it excluded"
    );
    // The fork's rows are in no coverage reference either.
    let coverage = status["coverage"].to_string();
    assert!(!coverage.contains("web-fork"), "coverage never read the fork: {coverage}");
}
