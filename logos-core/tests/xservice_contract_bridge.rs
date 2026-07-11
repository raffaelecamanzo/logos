//! End-to-end test for the in-memory cross-service contract bridge (S-245,
//! [CR-061], [FR-WS-04], [ADR-52], [NFR-RA-05]).
//!
//! Where the unit tests drive the matcher with fake engines, this drives the
//! **real** path: two (or three) member repositories, each indexed by its own
//! [`Engine`] over the real `tree-sitter-yaml` / `tree-sitter-rust` grammars,
//! multiplexed behind an [`EngineRegistry`], with the bridge reading each
//! member's contract surface through its read pool. It proves an OpenAPI
//! operation in one member binds a framework route in **another** via
//! `route_key`, that two providers of one key are ambiguous (no edge), and that
//! computing the bridge mutates **no** member database file ([ADR-52]).
//!
//! Gated on both grammars so a build excluding either does not run it.
#![cfg(all(feature = "lang-yaml", feature = "lang-rust"))]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::federation::{
    ContractBridge, EngineRegistry, Federation, Member, RegistryMode,
};
use logos_core::Engine;

/// An OpenAPI spec whose `/users/{user_id}` path drifts in parameter name from
/// the axum route's `/users/{id}` — the {id}-vs-{user_id} drift `route_key`
/// erases. Its `get` operation matches; its `delete` has no provider anywhere.
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

/// Index a member repo's fixtures into its own `.logos/logos.db`, then drop the
/// engine so the store is closed before the registry re-opens it. A no-op sync
/// re-runs resolution so a promoted route rebinds intra-repo (harmless here; it
/// just exercises the same path the unit test cannot).
fn index_member(root: &Path) {
    let engine = Engine::start(root).expect("engine starts");
    engine.index();
    let _ = engine.sync(&[] as &[PathBuf]);
    // `engine` drops here, releasing the store lock.
}

/// A member pointing at an indexed repo root.
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
        governance: Default::default(),
    }
}

/// The bytes of a member's persisted graph database — the file "no member DB is
/// mutated" is about ([ADR-52]).
fn db_bytes(root: &Path) -> Vec<u8> {
    fs::read(root.join(".logos").join("logos.db")).expect("member db exists")
}

/// FR-WS-04 acceptance: an OpenAPI operation in member `api` binds the matching
/// framework route in member `web` via `route_key`, across the param-name drift;
/// the endpoints are `(member, symbol)`; and computing the bridge mutates no
/// member DB.
#[test]
fn an_openapi_operation_binds_a_route_in_another_member() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let api = root.join("api");
    let web = root.join("web");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&web, "src/main.rs", AXUM_MAIN);
    index_member(&api);
    index_member(&web);

    let registry = EngineRegistry::<Engine>::new(
        federation(root, vec![member("api", &api), member("web", &web)]),
        RegistryMode::Lazy,
    );
    let bridge = ContractBridge::new();

    // Warm the engines (opening a store can checkpoint its WAL into the main db
    // file) BEFORE snapshotting, so the checksum isolates the bridge's own reads
    // from the one-time open — then prove those reads mutate nothing.
    let _ = bridge.edges(&registry);
    let api_before = db_bytes(&api);
    let web_before = db_bytes(&web);

    let edges = bridge.edges(&registry);

    // Exactly one cross-service edge: the GET operation in `api` → the route in
    // `web`. The DELETE operation has no provider, so it stays unbound.
    assert_eq!(
        edges.len(),
        1,
        "exactly one operation binds its cross-member route: {edges:?}"
    );
    let edge = &edges[0];
    assert_eq!(edge.relation, "route");
    assert_eq!(edge.from.member, "api", "the operation is in member `api`");
    assert_eq!(edge.to.member, "web", "the route is in member `web`");
    assert!(
        !edge.from.symbol.as_str().is_empty() && !edge.to.symbol.as_str().is_empty(),
        "both endpoints carry a portable LogosSymbol"
    );

    // Nothing was written to any member DB by the bridge computation ([ADR-52]).
    assert_eq!(db_bytes(&api), api_before, "member `api` DB unchanged by the bridge");
    assert_eq!(db_bytes(&web), web_before, "member `web` DB unchanged by the bridge");
}

/// FR-WS-04 acceptance: two members providing the same `route_key` make the
/// operation's match ambiguous — no edge is fabricated ([NFR-RA-05]).
#[test]
fn two_members_providing_the_same_route_are_ambiguous() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();

    let api = root.join("api");
    let web = root.join("web");
    let admin = root.join("admin");
    write(&api, "api/openapi.yaml", OPENAPI_YAML);
    write(&web, "src/main.rs", AXUM_MAIN);
    // `admin` registers the same GET /users/{...} route (param name drifts, but
    // the positional key is identical) — a second provider of the one key.
    write(&admin, "src/main.rs", &AXUM_MAIN.replace("{id}", "{uid}"));
    index_member(&api);
    index_member(&web);
    index_member(&admin);

    let registry = EngineRegistry::<Engine>::new(
        federation(
            root,
            vec![
                member("api", &api),
                member("web", &web),
                member("admin", &admin),
            ],
        ),
        RegistryMode::Lazy,
    );

    let edges = ContractBridge::new().edges(&registry);
    assert!(
        edges.is_empty(),
        "two providers of one key are ambiguous — no edge fabricated: {edges:?}"
    );
}
