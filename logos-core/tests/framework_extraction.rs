//! Integration tests for the framework-promotion pass (S-012 / FR-FW-01..04,
//! NFR-RA-05, NFR-PE-02), exercised end-to-end through
//! [`Engine::index`]/[`Engine::sync`] against real temp-directory fixtures.
//!
//! Coverage by acceptance criterion:
//! - Axum and Actix route registrations yield `route` nodes linked to their
//!   handler functions (FR-FW-01, FR-FW-03, UAT-FW-01);
//! - shared-state extractor types are promoted to `component` nodes
//!   (FR-FW-02 — the Rust face of component promotion; UAT-FW-02's Next.js
//!   case lands with the TS plugin, S-015);
//! - a plain library yields zero route/component nodes and is never even
//!   parsed by the pass (FR-FW-04, UAT-FW-03);
//! - an unprovable handler keeps its route node but gets no fabricated edge
//!   (NFR-RA-05);
//! - a sync reconciles promoted nodes to the current source (stale routes
//!   retired, surviving routes id-stable);
//! - the pass cost is measured and surfaced on every result (NFR-PE-02 /
//!   OQ-07 evidence).

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::model::{EdgeKind, NodeId, NodeKind};
use logos_core::Engine;
use logos_core::Runtime;
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// The id of the unique node with `name` and `kind`.
fn node_id(rt: &Runtime, name: &str, kind: NodeKind) -> NodeId {
    let wanted = name.to_string();
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .find(|r| r.kind == kind && r.name == wanted)
            .map(|r| r.id))
    })
    .expect("read runs")
    .unwrap_or_else(|| panic!("no {kind:?} node named {name:?}"))
}

/// Every node of `kind` as `(id, name)`.
fn nodes_of(rt: &Runtime, kind: NodeKind) -> Vec<(NodeId, String)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_nodes()?
            .into_iter()
            .filter(|n| n.kind == kind)
            .map(|n| (n.id, n.name))
            .collect())
    })
    .expect("read runs")
}

/// All `(source, target)` pairs of edges with `kind`.
fn edges_of(rt: &Runtime, kind: EdgeKind) -> Vec<(NodeId, NodeId)> {
    rt.submit_read(move |store| {
        Ok(store
            .all_edges()?
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| (e.source, e.target))
            .collect())
    })
    .expect("read runs")
}

// ── FR-FW-01 / FR-FW-03 / UAT-FW-01: Axum route promotion ────────────────────

#[test]
fn axum_routes_are_promoted_and_linked_to_handlers() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::{get, post};
use axum::Router;

fn app() -> Router {
    Router::new()
        .route(\"/users\", get(list_users).post(create_user))
        .route(\"/health\", get(health))
}

async fn list_users() {}
async fn create_user() {}
async fn health() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The minimal Axum router with two registrations (sprint Testing &
    // Verification): every (method, path) pair becomes a route node…
    let mut names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort();
    assert_eq!(names, ["GET /health", "GET /users", "POST /users"]);
    assert_eq!(result.framework.routes, 3);
    assert_eq!(result.framework.files_scanned, 1);

    // …linked to its handler function (FR-FW-01).
    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let get_users = node_id(rt, "GET /users", NodeKind::Route);
    let post_users = node_id(rt, "POST /users", NodeKind::Route);
    let get_health = node_id(rt, "GET /health", NodeKind::Route);
    let list_users = node_id(rt, "list_users", NodeKind::Function);
    let create_user = node_id(rt, "create_user", NodeKind::Function);
    let health = node_id(rt, "health", NodeKind::Function);
    assert!(routes_to.contains(&(get_users, list_users)));
    assert!(routes_to.contains(&(post_users, create_user)));
    assert!(routes_to.contains(&(get_health, health)));
    assert_eq!(routes_to.len(), 3);

    // Promoted nodes are scope-anchored under their file module so the
    // binder and module rollups see them like any declaration. (A `main.rs`
    // stem names its enclosing module, so the file module here is `crate`.)
    let contains = edges_of(rt, EdgeKind::Contains);
    let file_module = node_id(rt, "crate", NodeKind::Module);
    assert!(contains.contains(&(file_module, get_users)));
}

// ── FR-FW-01 / FR-FW-03 / UAT-FW-01: Actix route promotion ───────────────────

#[test]
fn actix_attribute_and_builder_routes_are_promoted() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use actix_web::{get, web, App};

#[get(\"/health\")]
async fn health() -> &'static str { \"ok\" }

fn app() {
    let _ = App::new().route(\"/index\", web::get().to(index));
}

async fn index() -> &'static str { \"hi\" }
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    let mut names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    names.sort();
    assert_eq!(names, ["GET /health", "GET /index"]);
    assert_eq!(result.framework.routes, 2);

    let routes_to = edges_of(rt, EdgeKind::RoutesTo);
    let health_route = node_id(rt, "GET /health", NodeKind::Route);
    let index_route = node_id(rt, "GET /index", NodeKind::Route);
    let health_fn = node_id(rt, "health", NodeKind::Function);
    let index_fn = node_id(rt, "index", NodeKind::Function);
    assert!(routes_to.contains(&(health_route, health_fn)));
    assert!(routes_to.contains(&(index_route, index_fn)));
}

// ── FR-FW-04 / UAT-FW-03: the plain-library regression ───────────────────────

#[test]
fn plain_rust_library_yields_no_spurious_routes_or_components() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/lib.rs",
        "\
use std::collections::HashMap;

pub struct Config { pub retries: u32 }

pub fn transform(input: &str, config: &Config) -> String {
    let map: HashMap<String, u32> = HashMap::new();
    // Strings that *look* like routes must not bait the pass:
    let path = \"/users\";
    format!(\"{}{}{}{}\", input, path, map.len(), config.retries)
}

pub fn route(s: &str) -> String { s.replace(\"/a\", \"/b\") }
",
    );
    write(tmp.path(), "src/util.rs", "pub fn helper() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert!(
        nodes_of(rt, NodeKind::Route).is_empty(),
        "no spurious routes"
    );
    assert!(
        nodes_of(rt, NodeKind::Component).is_empty(),
        "no spurious components"
    );
    assert!(edges_of(rt, EdgeKind::RoutesTo).is_empty());
    assert_eq!(result.framework.routes, 0);
    assert_eq!(result.framework.components, 0);
    // The ledger gate means the pass never parsed a single file (FR-FW-04).
    assert_eq!(result.framework.files_scanned, 0);
}

// ── FR-FW-02 / component promotion ───────────────────────────────────────────

#[test]
fn shared_state_types_are_promoted_to_components() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::extract::State;
use axum::routing::get;

pub struct AppState { pub hits: u64 }

fn app() {
    let _ = axum::Router::new().route(\"/\", get(home));
}

async fn home(State(state): State<AppState>) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The wired shared-state type is the Rust face of a framework component.
    let components = nodes_of(rt, NodeKind::Component);
    assert_eq!(
        components
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>(),
        ["AppState"]
    );
    assert_eq!(result.framework.components, 1);

    // The component references the *indexed* type node it promotes — proven
    // through the binder, never fabricated (NFR-RA-05).
    let component = node_id(rt, "AppState", NodeKind::Component);
    let the_struct = node_id(rt, "AppState", NodeKind::Struct);
    assert!(edges_of(rt, EdgeKind::References).contains(&(component, the_struct)));
}

#[test]
fn unknown_state_type_is_not_promoted() {
    // `State<ExternalState>` over a type the graph does not contain: the
    // binder cannot prove it, so nothing is promoted (NFR-RA-05).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::extract::State;
use external_crate::ExternalState;

async fn home(State(state): State<ExternalState>) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    assert!(nodes_of(rt, NodeKind::Component).is_empty());
}

// ── NFR-RA-05: never fabricate a handler link ────────────────────────────────

#[test]
fn unprovable_handler_keeps_route_node_without_edge() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app() {
    // `missing_handler` is declared nowhere in the indexed graph.
    let _ = axum::Router::new().route(\"/ghost\", get(missing_handler));
}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    // The registration itself is real — the route node is honest data…
    assert_eq!(result.framework.routes, 1);
    let _route = node_id(rt, "GET /ghost", NodeKind::Route);
    // …but no handler edge is invented for a target the binder cannot prove.
    assert!(edges_of(rt, EdgeKind::RoutesTo).is_empty());
}

// ── Sync reconciliation: stale promotions retire, survivors stay stable ─────

#[test]
fn sync_reconciles_routes_to_the_current_source() {
    let tmp = TempDir::new().unwrap();
    let main_rs = "\
use axum::routing::get;

fn app() {
    let _ = axum::Router::new()
        .route(\"/keep\", get(keep))
        .route(\"/drop\", get(drop_me));
}

async fn keep() {}
async fn drop_me() {}
";
    write(tmp.path(), "src/main.rs", main_rs);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    assert_eq!(first.framework.routes, 2);
    let keep_before = node_id(rt, "GET /keep", NodeKind::Route);

    // Drop one registration and sync the file.
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app() {
    let _ = axum::Router::new().route(\"/keep\", get(keep));
}

async fn keep() {}
",
    );
    let synced = engine.sync(&[PathBuf::from("src/main.rs")]);
    assert!(synced.warnings.is_empty(), "{:?}", synced.warnings);
    assert_eq!(synced.framework.routes, 1);

    let names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(names, ["GET /keep"], "the stale route is retired");

    // The surviving route is re-linked to the re-extracted handler.
    let keep_after = node_id(rt, "GET /keep", NodeKind::Route);
    let keep_fn = node_id(rt, "keep", NodeKind::Function);
    assert!(edges_of(rt, EdgeKind::RoutesTo).contains(&(keep_after, keep_fn)));
    // (The file was re-extracted, so its promoted nodes were rebuilt with it;
    // id stability across *unrelated* syncs is covered below.)
    let _ = keep_before;
}

#[test]
fn untouched_routes_keep_their_ids_across_unrelated_syncs() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app() {
    let _ = axum::Router::new().route(\"/stable\", get(handler));
}

async fn handler() {}
",
    );
    write(tmp.path(), "src/other.rs", "pub fn unrelated() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();
    let before = node_id(rt, "GET /stable", NodeKind::Route);

    write(
        tmp.path(),
        "src/other.rs",
        "pub fn unrelated_changed() {}\n",
    );
    let synced = engine.sync(&[PathBuf::from("src/other.rs")]);
    assert_eq!(synced.framework.routes, 1);

    let after = node_id(rt, "GET /stable", NodeKind::Route);
    assert_eq!(before, after, "an unrelated sync must not churn route ids");
}

#[test]
fn sync_removes_all_routes_when_framework_file_is_deleted() {
    // Review round 1, must-fix #1: the reconcile-to-empty branch — candidates
    // empty but promoted nodes present — must NOT take the fast path and must
    // retire every promotion (FR-FW-04 over time, not just at first index).
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app() {
    let _ = axum::Router::new().route(\"/gone\", get(handler));
}

async fn handler() {}
",
    );
    write(tmp.path(), "src/lib_part.rs", "pub fn stays() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    assert_eq!(first.framework.routes, 1);

    fs::remove_file(tmp.path().join("src/main.rs")).unwrap();
    let synced = engine.sync(&[PathBuf::from("src/main.rs")]);
    assert_eq!(synced.files_removed, 1);
    assert_eq!(synced.framework.routes, 0);
    assert!(
        nodes_of(rt, NodeKind::Route).is_empty(),
        "every promoted route is retired with its defining file"
    );
    assert!(edges_of(rt, EdgeKind::RoutesTo).is_empty());
}

#[test]
fn sync_retires_component_when_state_extractor_removed() {
    // Review round 1, must-fix #2: a component anchors at the *type's* file,
    // so removing the only State<T> usage from the *handler* file cannot
    // cascade it away — only the reconcile sweep retires it.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/state.rs", "pub struct AppState;\n");
    let with_state = "\
use axum::extract::State;
use axum::routing::get;
use crate::state::AppState;

fn app() {
    let _ = axum::Router::new().route(\"/\", get(home));
}

async fn home(State(s): State<AppState>) {}
";
    write(tmp.path(), "src/main.rs", with_state);

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let first = engine.index();
    assert_eq!(first.framework.components, 1);

    // Drop the extractor but keep the route (the file stays a candidate).
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app() {
    let _ = axum::Router::new().route(\"/\", get(home));
}

async fn home() {}
",
    );
    let synced = engine.sync(&[PathBuf::from("src/main.rs")]);
    assert_eq!(synced.framework.components, 0);
    assert!(
        nodes_of(rt, NodeKind::Component).is_empty(),
        "the stale component is retired by the reconcile sweep"
    );
    // The promoted marker is gone; the real type declaration survives.
    let _ = node_id(rt, "AppState", NodeKind::Struct);
}

#[test]
fn sync_retires_handler_edge_when_binding_becomes_ambiguous() {
    // Review round 1, must-fix #3: the stale-edge sweep (delete_edge) on a
    // *surviving* route node. The route file is untouched; a new same-name
    // handler elsewhere makes the glob-scope binding ambiguous, so the
    // exactly-one rule (NFR-RA-05) withdraws the previously proven edge.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;
use crate::a::*;
use crate::b::*;

fn app() {
    let _ = axum::Router::new().route(\"/x\", get(handler));
}
",
    );
    write(tmp.path(), "src/a.rs", "pub fn handler() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let route_before = node_id(rt, "GET /x", NodeKind::Route);
    assert_eq!(
        edges_of(rt, EdgeKind::RoutesTo).len(),
        1,
        "uniquely provable through the one resolvable glob"
    );

    // A second glob target with the same name appears; only IT is synced —
    // the router file (and its route node) stays in place.
    write(tmp.path(), "src/b.rs", "pub fn handler() {}\n");
    let synced = engine.sync(&[PathBuf::from("src/b.rs")]);
    assert_eq!(synced.framework.routes, 1, "the route itself survives");

    let route_after = node_id(rt, "GET /x", NodeKind::Route);
    assert_eq!(route_before, route_after, "no id churn for the survivor");
    assert!(
        edges_of(rt, EdgeKind::RoutesTo).is_empty(),
        "the now-ambiguous handler edge is withdrawn, never guessed"
    );
}

#[test]
fn duplicate_method_path_registrations_collapse_to_one_route() {
    // Review round 1, should-fix #3: one route node per (file, METHOD, path)
    // symbol — a duplicate registration deduplicates deterministically.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;

fn app_a() {
    let _ = axum::Router::new().route(\"/dup\", get(handler));
}

fn app_b() {
    let _ = axum::Router::new().route(\"/dup\", get(handler));
}

async fn handler() {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(result.framework.routes, 1);
    let names: Vec<String> = nodes_of(rt, NodeKind::Route)
        .into_iter()
        .map(|(_, n)| n)
        .collect();
    assert_eq!(names, ["GET /dup"]);
}

#[test]
fn multi_file_state_usage_yields_one_component() {
    // Review round 1, should-fix #4: two usage sites of the same state type
    // converge on ONE component node (the symbol derives from the type's
    // file, not the usage site).
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/state.rs", "pub struct AppState;\n");
    write(
        tmp.path(),
        "src/routes_a.rs",
        "\
use axum::extract::State;
use crate::state::AppState;

pub async fn list(State(s): State<AppState>) {}
",
    );
    write(
        tmp.path(),
        "src/routes_b.rs",
        "\
use axum::extract::State;
use crate::state::AppState;

pub async fn create(State(s): State<AppState>) {}
",
    );

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    let result = engine.index();

    assert_eq!(result.framework.components, 1);
    assert_eq!(nodes_of(rt, NodeKind::Component).len(), 1);
}

// ── Cross-file handlers resolve through the file's imports ───────────────────

#[test]
fn cross_file_handler_binds_through_imports() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "src/main.rs",
        "\
use axum::routing::get;
use crate::handlers::list;

fn app() {
    let _ = axum::Router::new().route(\"/list\", get(list));
}
",
    );
    write(tmp.path(), "src/handlers.rs", "pub async fn list() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let rt = engine.runtime().unwrap();
    engine.index();

    let route = node_id(rt, "GET /list", NodeKind::Route);
    let handler = node_id(rt, "list", NodeKind::Function);
    assert!(edges_of(rt, EdgeKind::RoutesTo).contains(&(route, handler)));
}

// ── NFR-PE-02 / OQ-07: the dogfood cost measurement ──────────────────────────

/// Recursively copy every `.rs` file under `dir` into `dst_root`, preserving
/// the path relative to `base` (the resolution-dogfood pattern, S-011).
fn copy_rs_tree(base: &Path, dir: &Path, dst_root: &Path) {
    for entry in fs::read_dir(dir).expect("readable dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            copy_rs_tree(base, &path, dst_root);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let rel = path.strip_prefix(base).unwrap();
            let dst = dst_root.join(rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(&path, &dst).unwrap();
        }
    }
}

#[test]
fn dogfood_measures_framework_cost_against_the_index_budget() {
    // OQ-07 / AQ-05: the *measured* answer to "does framework extraction fit
    // the ≤30s index budget?" (NFR-PE-02, CR-01). Two data points:
    //
    // 1. The Logos dogfood itself — a real ~25k-LOC Rust tree with no web
    //    framework: the pass must gate to zero parsed files and cost
    //    effectively nothing (the FR-FW-04 fast path at scale).
    // 2. The same tree + a realistic Axum/Actix surface (30 routes across
    //    two router files): the candidate-parse + promotion cost.
    //
    // The printed numbers are the measurement; the assertions are generous
    // ceilings so CI noise cannot flake them while a pathological regression
    // (whole-tree re-parse, quadratic reconcile) still fails loudly.
    let tmp = TempDir::new().unwrap();
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    copy_rs_tree(&crate_root, &crate_root.join("src"), tmp.path());

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let plain = engine.index();
    println!(
        "dogfood framework (plain): files_indexed={} total_ms={} framework_ms={} scanned={}",
        plain.files_indexed,
        plain.duration_ms,
        plain.framework.duration_ms,
        plain.framework.files_scanned,
    );
    assert_eq!(
        plain.framework.files_scanned, 0,
        "Logos itself uses no web framework — the gate must parse nothing"
    );
    drop(engine);

    // Add a realistic framework surface and re-index.
    let mut axum_src = String::from(
        "use axum::routing::{get, post};\n\nfn app() {\n    let _ = axum::Router::new()\n",
    );
    for i in 0..12 {
        axum_src.push_str(&format!(
            "        .route(\"/api/resource{i}\", get(get_{i}).post(post_{i}))\n"
        ));
    }
    axum_src.push_str("        ;\n}\n\n");
    for i in 0..12 {
        axum_src.push_str(&format!(
            "async fn get_{i}() {{}}\nasync fn post_{i}() {{}}\n"
        ));
    }
    write(tmp.path(), "src/axum_app.rs", &axum_src);

    let mut actix_src = String::from("use actix_web::{get, web};\n\n");
    for i in 0..6 {
        actix_src.push_str(&format!(
            "#[get(\"/actix/{i}\")]\nasync fn actix_{i}() -> &'static str {{ \"ok\" }}\n\n"
        ));
    }
    write(tmp.path(), "src/actix_app.rs", &actix_src);

    let engine = Engine::start(tmp.path()).expect("engine restarts");
    let with_fw = engine.index();
    println!(
        "dogfood framework (with frameworks): files_indexed={} total_ms={} framework_ms={} scanned={} routes={} components={}",
        with_fw.files_indexed,
        with_fw.duration_ms,
        with_fw.framework.duration_ms,
        with_fw.framework.files_scanned,
        with_fw.framework.routes,
        with_fw.framework.components,
    );
    assert_eq!(with_fw.framework.routes, 30, "24 axum + 6 actix routes");
    assert_eq!(
        with_fw.framework.files_scanned, 2,
        "only the two framework files are parsed, never the whole tree"
    );
    // The budget ceiling: NFR-PE-02 grants 30s for a ~100k-LOC index; the
    // framework pass on this ~25k-LOC tree must be a rounding error. 3s is
    // two orders of magnitude above the observed cost — a regression guard,
    // not a benchmark.
    assert!(
        with_fw.framework.duration_ms < 3_000,
        "framework pass took {}ms — investigate against the 30s budget (OQ-07)",
        with_fw.framework.duration_ms
    );
}

// ── NFR-PE-02 / OQ-07: the cost signal rides every result ────────────────────

#[test]
fn framework_cost_is_measured_and_surfaced() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "src/lib.rs", "pub fn plain() {}\n");

    let engine = Engine::start(tmp.path()).expect("engine starts");
    let result = engine.index();

    // The stats struct is always present — the duration is the OQ-07
    // evidence channel and must be bounded by the whole run's duration.
    assert!(result.framework.duration_ms <= result.duration_ms);

    let synced = engine.sync(&[PathBuf::from("src/lib.rs")]);
    assert!(synced.framework.duration_ms <= synced.duration_ms);
}
