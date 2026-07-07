//! End-to-end test for the OpenAPI `ApiOperation` → framework `route` binding
//! (S-069, [CR-011], [FR-CG-09], [FR-CG-11], [UAT-CG-03], [NFR-RA-05]), driving
//! the whole discover → extract → resolve → framework-promote pipeline over the
//! real `tree-sitter-yaml` and `tree-sitter-rust` grammars.
//!
//! It exercises the load-bearing path the unit tests cannot: the operation
//! reference is captured at extraction, the route node is promoted by the
//! framework pass *after* resolution, and the binding therefore lands on the
//! **next sync** (the retry-on-sync contract, [FR-RS-03]). It then asserts the
//! binding through the coverage surface, `node`, and `context` ([FR-CG-11]).
//!
//! Gated on both grammars so a build excluding either does not run it.
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeKind};
use logos_core::{Engine, Runtime};

/// An OpenAPI spec whose `/users/{user_id}` path drifts in parameter name from
/// the axum route's `/users/{id}` — the {id}-vs-{user_id} drift the acceptance
/// calls out. Its `get` operation has a matching route; its `delete` does not.
const OPENAPI_YAML: &str = "\
openapi: 3.0.3
info:
  title: User API
  version: 1.0.0
paths:
  /users/{user_id}:
    get:
      summary: Get a user
    delete:
      summary: Delete a user
";

/// An axum app registering exactly one route, `GET /users/{id}`, to a handler —
/// the framework pass promotes it to a `route` node named `"GET /users/{id}"`.
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

/// Index the fixtures, then run a no-op sync so the operation reference — captured
/// at index but unbindable until the framework pass promoted the route — rebinds
/// against the now-present route node (the retry-on-sync contract).
fn index_then_rebind(root: &Path) -> Engine {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    // A sync over no changed paths re-extracts nothing but re-runs resolution over
    // the full ledger, which now sees the promoted route node.
    let _ = engine.sync(&[] as &[PathBuf]);
    engine
}

/// The symbol of the one node of `kind` whose name is `name`.
fn symbol_of(rt: &Runtime, name: &str, kind: NodeKind) -> String {
    let needle = name.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|n| n.kind == kind && n.name == needle)
            .map(|n| n.symbol.as_str().to_string()))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} named {name}"))
}

/// FR-CG-09 / UAT-CG-03: the operation binds to its route across the
/// parameter-name drift, and the unmatched operation stays honestly unresolved —
/// surfaced in the per-relation coverage of `route`.
#[test]
fn operation_binds_to_its_route_and_the_unmatched_one_stays_unresolved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "api/openapi.yaml", OPENAPI_YAML);
    write(root, "src/main.rs", AXUM_MAIN);

    let engine = index_then_rebind(root);
    let rt = engine.runtime().expect("runtime present");

    // The route node was promoted with the axum template verbatim.
    let route = symbol_of(rt, "GET /users/{id}", NodeKind::Route);

    // Exactly one `ArtifactBinding` edge lands on the route — from an
    // `ApiOperation` source — proving the operation→route bind (param-name drift
    // erased by the shared positional normalizer).
    let route_sym = route.clone();
    let bindings_into_route = rt
        .submit_read(move |store| {
            let id = store.node_by_symbol(&route_sym)?.expect("route node").id;
            Ok(store
                .neighbours_in(id)?
                .into_iter()
                .filter(|(kind, src)| {
                    *kind == EdgeKind::ArtifactBinding && src.kind == NodeKind::ApiOperation
                })
                .count())
        })
        .expect("read runs");
    assert_eq!(
        bindings_into_route, 1,
        "exactly one ApiOperation binds to the route (the GET; param-name drift erased)"
    );

    // The coverage surface reports the `route` relation: one bound (GET), one
    // unresolved (DELETE has no matching route) — honest, never guessed.
    let coverage = rt
        .submit_read(|store| logos_core::resolve::coverage(store))
        .expect("coverage read");
    let route_cov = coverage
        .by_relation
        .get("route")
        .expect("the route relation appears in coverage");
    assert_eq!(
        (route_cov.bound, route_cov.unresolved),
        (1, 1),
        "one operation binds, the method-mismatched one stays unresolved (NFR-RA-05)"
    );
}

/// FR-CG-11: `node` on the route lists its inbound artifact binding, and
/// `context` seeded at the handler pulls in the `ApiOperation` and its spec
/// section (the enclosing `ApiPath`).
#[test]
fn the_binding_surfaces_through_node_and_context() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "api/openapi.yaml", OPENAPI_YAML);
    write(root, "src/main.rs", AXUM_MAIN);

    let engine = index_then_rebind(root);
    let rt = engine.runtime().expect("runtime present");

    // `node` on the route lists the ArtifactBinding among its immediate edges.
    let route = symbol_of(rt, "GET /users/{id}", NodeKind::Route);
    let info = engine.node(&route, false);
    let detail = info.node.expect("the route node resolves");
    assert!(
        detail
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::ArtifactBinding),
        "node on the route lists its artifact binding: {:?}",
        detail.edges
    );

    // `context` seeded at the handler pulls in its ApiOperation and spec section:
    // the handler → route (RoutesTo) → ApiOperation (ArtifactBinding) → ApiPath
    // (Contains) chain is traversed through the store-read artifact expansion.
    let bundle = engine.context("get_user", None, false);
    let kinds: Vec<NodeKind> = bundle.nodes.iter().map(|n| n.symbol.kind).collect();
    assert!(
        kinds.contains(&NodeKind::ApiOperation),
        "context at the handler pulls in the ApiOperation: {kinds:?}"
    );
    assert!(
        kinds.contains(&NodeKind::ApiPath),
        "context at the handler pulls in the spec section (ApiPath): {kinds:?}"
    );
}
