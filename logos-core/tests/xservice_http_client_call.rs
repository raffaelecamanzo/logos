//! End-to-end test for the HTTP client-call → route cross-service arm (S-252,
//! [CR-061], [FR-WS-08], [ADR-54], [NFR-RA-05]).
//!
//! Where the unit tests drive the bridge matcher and the arm's normalizer with
//! fixtures, this drives the **real** pipeline: two member repositories indexed
//! by their own [`Engine`] over the real `tree-sitter-rust` grammar, behind an
//! [`EngineRegistry`]. A Rust file in the `web` member makes an outbound
//! `client.get("/users/{id}")` call — captured by the `invocations` query into
//! the ledger under the `HttpClientCall` relation — and an axum route in the
//! `api` member is promoted to a `route` node. It proves:
//!
//! - a static client call binds the sole matching route in **another** member,
//!   across the `{id}`/`{user_id}` param-name drift (acceptance 1);
//! - two matching routes make the call ambiguous — no edge (acceptance 1);
//! - a runtime-composed (bare-variable) path never binds — no approximate edge is
//!   fabricated even when a matching route exists (acceptance 2/3);
//! - computing the bridge mutates no member database file ([ADR-52]).
//!
//! Gated on the Rust grammar so a build excluding it does not run it.
#![cfg(feature = "lang-rust")]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    cross_service_coverage, ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;

/// A client module making a static outbound call `GET /users/{id}` — captured as
/// an `HttpClientCall` reference `"GET /users/{id}"` sourced from `fetch_user`.
const CLIENT_STATIC: &str = r#"
pub async fn fetch_user(client: reqwest::Client) {
    let _ = client.get("/users/{id}").await;
}
"#;

/// A client module whose request path is a **bare variable** — the URL is
/// composed at runtime, so the arm refuses it (base-url-runtime): no reference,
/// no ledger entry, no bind.
const CLIENT_COMPOSED: &str = r#"
pub async fn fetch_user(client: reqwest::Client, url: String) {
    let _ = client.get(url).await;
}
"#;

/// An axum app registering `GET /users/{id}` — promoted to a `route` node named
/// `"GET /users/{id}"`, the provider the client call binds to. Its `{id}` drifts
/// from a consumer's `{user_id}`; the positional `route_key` erases the drift.
const AXUM_MAIN: &str = r#"
use axum::routing::get;
use axum::Router;

async fn get_user() {}

fn app() -> Router {
    Router::new().route("/users/{user_id}", get(get_user))
}
"#;

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
    fs::write(path, contents).expect("write fixture");
}

/// Index a member repo's fixtures into its own `.logos/logos.db`, then drop the
/// engine so the store is closed before the registry re-opens it.
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
    // `engine` drops here, releasing the store lock.
}

fn member(name: &str, root: &Path) -> Member {
    Member {
        name: name.to_string(),
        root: root.to_path_buf(),
    }
}

fn federation(root: &Path, members: Vec<Member>) -> Federation {
    Federation {
        name: "shop".to_string(),
        root: root.to_path_buf(),
        members,
        default: None,
        links: Vec::new(),
    }
}

fn db_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join(".logos").join("logos.db")).expect("member db exists")
}

/// Acceptance (1): a static client call in `web` binds the sole matching route in
/// `api` via `route_key` across the param-name drift; the edge starts at the call
/// site and points at the route; and the bridge mutates no member DB ([ADR-52]).
/// The coverage tier reports the same call `bound`.
#[test]
fn a_static_client_call_binds_a_route_in_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "src/client.rs", CLIENT_STATIC);
    write(&api, "src/main.rs", AXUM_MAIN);
    index_member(&web);
    index_member(&api);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("web", &web), member("api", &api)]),
        RegistryMode::Lazy,
    );
    let bridge = ContractBridge::new();

    // Warm the engines before snapshotting so the checksum isolates the bridge's
    // reads from the one-time store open.
    let _ = bridge.edges(&registry);
    let web_before = db_bytes(&web);
    let api_before = db_bytes(&api);

    let edges = bridge.edges(&registry);

    assert_eq!(
        edges.len(),
        1,
        "exactly one client call binds its cross-member route: {edges:?}"
    );
    let edge = &edges[0];
    assert_eq!(edge.relation, "route", "an HTTP arm edge speaks the `route` vocabulary");
    assert_eq!(edge.from.member, "web", "the call site is in member `web`");
    assert_eq!(edge.to.member, "api", "the route is in member `api`");
    assert!(
        !edge.from.symbol.as_str().is_empty() && !edge.to.symbol.as_str().is_empty(),
        "both endpoints carry a portable LogosSymbol"
    );

    // The coverage read-model reports the same call as `bound`.
    let coverage = cross_service_coverage(&registry);
    assert_eq!(coverage.bound, 1, "the client call is bound in the coverage tier");
    assert_eq!(coverage.ambiguous, 0);
    assert!(coverage
        .references
        .iter()
        .any(|r| r.relation == "route" && r.from.member == "web"));

    // The bridge computation wrote to no member DB ([ADR-52]).
    assert_eq!(db_bytes(&web), web_before, "member `web` DB unchanged by the bridge");
    assert_eq!(db_bytes(&api), api_before, "member `api` DB unchanged by the bridge");
}

/// Acceptance (1): two members providing the same route make the client call
/// ambiguous — no edge is fabricated ([NFR-RA-05]).
#[test]
fn two_matching_routes_make_the_client_call_ambiguous() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    let admin = root.join("admin");
    write(&web, "src/client.rs", CLIENT_STATIC);
    write(&api, "src/main.rs", AXUM_MAIN);
    // `admin` registers the same positional route (param name drifts again).
    write(&admin, "src/main.rs", &AXUM_MAIN.replace("{user_id}", "{uid}"));
    index_member(&web);
    index_member(&api);
    index_member(&admin);

    let registry = EngineRegistry::<Engine>::new(
        federation(
            root,
            vec![member("web", &web), member("api", &api), member("admin", &admin)],
        ),
        RegistryMode::Lazy,
    );

    let edges = ContractBridge::new().edges(&registry);
    assert!(
        edges.is_empty(),
        "two providers of one client-call key are ambiguous — no edge: {edges:?}"
    );

    // The coverage tier records it as ambiguous, not bound.
    let coverage = cross_service_coverage(&registry);
    assert_eq!(coverage.ambiguous, 1, "the ambiguous call is bucketed as such");
    assert_eq!(coverage.bound, 0);
}

/// Acceptance (2/3): a runtime-composed (bare-variable) client-call path never
/// enters the ledger, so it binds nothing even when a matching route exists —
/// no approximate edge is ever fabricated.
#[test]
fn a_runtime_composed_client_call_never_binds() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let web = root.join("web");
    let api = root.join("api");
    write(&web, "src/client.rs", CLIENT_COMPOSED);
    write(&api, "src/main.rs", AXUM_MAIN);
    index_member(&web);
    index_member(&api);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("web", &web), member("api", &api)]),
        RegistryMode::Lazy,
    );

    let edges = ContractBridge::new().edges(&registry);
    assert!(
        edges.is_empty(),
        "a base-url-runtime client call never binds — no approximate edge: {edges:?}"
    );
}
