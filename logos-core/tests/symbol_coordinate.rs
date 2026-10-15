//! S-703 ([BR-74], [ADR-85]): every node symbol carries the repository
//! coordinate in its SCIP package slot — code, documentation, configuration,
//! framework routes, broker topics and policy nodes alike — and a store never
//! holds two coordinates.
//!
//! Each fixture pins `[project] id`, so no symbol here depends on the
//! machine's directory names.
//!
//! [BR-74]: ../../docs/specs/software-spec.md#35-extraction--symbol-ids
//! [ADR-85]: ../../docs/specs/architecture/decisions/ADR-85.md

#![cfg(all(feature = "lang-rust", feature = "lang-java", feature = "lang-markdown", feature = "lang-yaml"))]

use std::fs;
use std::path::{Path, PathBuf};

use logos_core::graph_store::SYMBOL_COORDINATE_KEY;
use logos_core::model::{LogosSymbol, NodeKind};
use logos_core::Engine;
use tempfile::TempDir;

#[path = "support/coordinate.rs"]
mod coordinate;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Set the project's coordinate, replacing whatever config was there — these
/// tests change it between runs.
fn pin(root: &Path, id: &str) {
    write(root, ".logos/config.toml", &coordinate::project_table(id));
}

/// A Spring service publishing to `orders` — the broker topic both repositories share.
const ORDER_SERVICE: &str = r#"
package com.acme;
class OrderService {
    private KafkaTemplate<String, String> kafkaTemplate;
    public void publish(String payload) {
        kafkaTemplate.send("orders", payload);
    }
}
"#;

/// One file of every kind that builds a symbol: Rust code with axum routes and
/// a shared-state type (framework promotion: a route and a component), a Java Kafka publish (topic, producer), a markdown
/// document, a YAML config file, and a `rules.toml` declaring two layers and a
/// boundary (policy nodes).
fn every_kind(root: &Path, id: &str) {
    pin(root, id);
    write(
        root,
        ".logos/rules.toml",
        "[[layers]]\nname = \"domain\"\npaths = [\"src/domain_*.rs\"]\norder = 1\n\n\
         [[layers]]\nname = \"web\"\npaths = [\"src/main.rs\"]\norder = 2\n\n\
         [[boundaries]]\nfrom = \"domain\"\nto = \"web\"\nreason = \"downward only\"\n",
    );
    write(
        root,
        "src/main.rs",
        "use axum::extract::State;\nuse axum::routing::get;\nuse axum::Router;\n\nmod domain_core;\n\n\
         pub struct AppState { pub hits: u64 }\n\n\
         fn app() -> Router {\n    Router::new().route(\"/health\", get(health)).route(\"/\", get(home))\n}\n\n\
         async fn health() { domain_core::compute(); }\n\n\
         async fn home(State(_state): State<AppState>) {}\n",
    );
    write(root, "src/domain_core.rs", "pub fn compute() {}\npub struct Order;\n");
    write(root, "src/OrderService.java", ORDER_SERVICE);
    write(root, "docs/guide.md", "# Guide\n\n## Running\n\nRun it.\n");
    write(root, "config/app.yaml", "server:\n  port: 8080\n");
    // Every other kind a file builds: the remaining Rust declaration kinds, a
    // Java interface and Kafka listener, swe-skills documents (requirement,
    // ADR, story), the build and deploy files, and the descriptor formats
    // whose nodes are anchored rather than extracted (OpenAPI, and — with
    // their grammars — proto, GraphQL, SQL and Terraform).
    write(
        root,
        "src/kinds.rs",
        "pub trait Shape { fn area(&self) -> f64; }\npub enum Colour { Red }\n\
         pub const LIMIT: u32 = 3;\npub static NAME: &str = \"x\";\npub type Id = u64;\n\
         macro_rules! twice { ($e:expr) => { $e * 2 }; }\npub struct Point { pub x: i32 }\n",
    );
    write(root, "src/OrderListener.java", ORDER_LISTENER);
    write(root, "docs/specs/requirements/FR-AB-01.md", "# FR-AB-01: Orders are kept\n\nKeep them.\n");
    write(root, "docs/decisions/ADR-01-storage.md", "# ADR-01: Storage\n\nUse a file.\n");
    write(root, "docs/journal.md", "# Journal\n\n## S-1: Keep orders\n\nAs a user.\n");
    write(root, "deploy.sh", "deploy() {\n  echo hi\n}\n");
    write(root, "Dockerfile", "FROM rust:1 AS build\nRUN true\nFROM debian AS run\n");
    write(root, "Makefile", "build:\n\tcargo build\n");
    write(
        root,
        "api/openapi.yaml",
        "openapi: 3.0.0\ninfo:\n  title: t\n  version: \"1\"\npaths:\n  /orders/{id}:\n    get:\n\
         \x20     operationId: getOrder\n      responses:\n        \"200\":\n          description: ok\n",
    );
    write(
        root,
        "proto/orders.proto",
        "syntax = \"proto3\";\npackage acme;\nmessage Order { string id = 1; }\n\
         service Orders { rpc Get(Order) returns (Order); }\n",
    );
    write(root, "graphql/schema.graphql", "type Order { id: ID! }\n");
    write(root, "db/schema.sql", "CREATE TABLE orders (id INT);\n");
    write(root, "main.tf", "resource \"aws_s3_bucket\" \"b\" {\n  bucket = \"x\"\n}\n");
}

/// A Spring Kafka listener on `orders` behind an interface (consumer, interface).
const ORDER_LISTENER: &str = r#"
package com.acme;
interface Listener { void on(String message); }
class OrderListener implements Listener {
    @KafkaListener(topics = "orders")
    public void on(String message) {}
}
"#;

/// Whether `every_kind` yields a node of `kind` in this build, and why not.
///
/// An exhaustive match over [`NodeKind`] with no wildcard arm: a new kind does
/// not compile until it is classified here, so the "every node carries the
/// coordinate" test cannot silently miss a kind it was never shown.
fn produced_by_the_fixture(kind: NodeKind) -> Result<(), &'static str> {
    use NodeKind::*;
    let with = |on: bool, off: &'static str| if on { Ok(()) } else { Err(off) };
    match kind {
        Module | Class | Interface | Trait | Struct | Enum | Function | Method | Field | Constant
        | Variable | TypeAlias | Macro | Route | Component | Layer | Boundary | DocFile
        | DocSection | Requirement | Adr | Story | ConfigFile | ConfigSection | ShellFunction
        | DockerfileStage | MakeTarget | ApiPath | ApiOperation | Topic | Producer | Consumer => Ok(()),
        ProtoMessage | ProtoService => with(cfg!(feature = "lang-protobuf"), "lang-protobuf is off"),
        GqlType => with(cfg!(feature = "lang-graphql"), "lang-graphql is off"),
        SqlObject => with(cfg!(feature = "lang-sql"), "lang-sql is off"),
        TfBlock => with(cfg!(feature = "lang-terraform"), "lang-terraform is off"),
    }
}

/// Every node as `(kind, symbol)`.
fn nodes(engine: &Engine) -> Vec<(NodeKind, LogosSymbol)> {
    engine
        .runtime()
        .unwrap()
        .submit_read(|s| Ok(s.all_nodes()?.into_iter().map(|n| (n.kind, n.symbol)).collect()))
        .unwrap()
}

/// The coordinate the store records for its symbols.
fn recorded(engine: &Engine) -> Option<String> {
    engine.runtime().unwrap().submit_read(|s| s.symbol_coordinate()).unwrap()
}

/// The symbol of the one node of `kind` named `name`.
fn symbol_of(engine: &Engine, kind: NodeKind, name: &str) -> LogosSymbol {
    let found: Vec<LogosSymbol> = engine
        .runtime()
        .unwrap()
        .submit_read(move |s| {
            Ok(s.all_nodes()?
                .into_iter()
                .filter(|n| n.kind == kind && n.name == name)
                .map(|n| n.symbol)
                .collect())
        })
        .unwrap();
    assert_eq!(found.len(), 1, "one {kind:?} named {name}: {found:?}");
    found.into_iter().next().unwrap()
}

#[test]
fn every_node_symbol_carries_the_configured_coordinate() {
    let tmp = TempDir::new().unwrap();
    every_kind(tmp.path(), "github.com/acme/svc-a");
    let engine = Engine::start(tmp.path()).unwrap();
    let result = engine.index();
    assert!(result.persist_failures.is_empty(), "{:?}", result.persist_failures);

    let all = nodes(&engine);
    // Every kind is classified, and every kind the fixture is classified as
    // producing is present — so "every symbol carries it" is not vacuous over
    // a kind that never appeared, and a kind it stops producing is noticed.
    for kind in NodeKind::ALL {
        let present = all.iter().any(|(k, _)| *k == kind);
        match produced_by_the_fixture(kind) {
            Ok(()) => assert!(present, "the fixture yields a {kind:?} node"),
            Err(why) => assert!(!present, "{kind:?} is classified as absent ({why}) but appeared"),
        }
    }
    let off: Vec<_> = all
        .iter()
        .filter(|(_, s)| s.coordinate() != Some("github.com/acme/svc-a"))
        .collect();
    assert!(off.is_empty(), "every node symbol carries the coordinate: {off:#?}");
    assert!(all.iter().all(|(_, s)| !s.as_str().starts_with("logos . . . ")));

    assert_eq!(
        symbol_of(&engine, NodeKind::Topic, "orders").as_str(),
        "logos . github.com/acme/svc-a . topic/orders#"
    );
    assert_eq!(
        symbol_of(&engine, NodeKind::Layer, "domain").as_str(),
        "logos policy github.com/acme/svc-a . layer/domain/"
    );
    assert_eq!(recorded(&engine).as_deref(), Some("github.com/acme/svc-a"));

    // `check_rules` re-materialises the policy nodes: under the store's
    // coordinate, not a coordinate-less one.
    engine.check_rules(None, false).unwrap();
    assert_eq!(
        symbol_of(&engine, NodeKind::Boundary, "domain->web").as_str(),
        "logos policy github.com/acme/svc-a . boundary/domain/web/"
    );
}

#[test]
fn two_repositories_orders_topics_are_two_symbols() {
    // BR-74: an equal topic name never makes one node of two repositories'
    // topics; the workspace bridge joins them on the name instead.
    let topic = |id: &str| {
        let tmp = TempDir::new().unwrap();
        pin(tmp.path(), id);
        write(tmp.path(), "src/OrderService.java", ORDER_SERVICE);
        let engine = Engine::start(tmp.path()).unwrap();
        engine.index();
        symbol_of(&engine, NodeKind::Topic, "orders")
    };
    let a = topic("github.com/acme/orders");
    let b = topic("github.com/acme/billing");
    assert_ne!(a, b, "two repositories' `orders` topics have different symbols");
    assert_eq!(a.descriptor_tail(), Some("topic/orders#"));
    assert_eq!(a.descriptor_tail(), b.descriptor_tail(), "and meet on the same name");
}

#[test]
fn a_sync_keeps_the_coordinate_the_store_carries_and_a_full_index_moves_it() {
    let tmp = TempDir::new().unwrap();
    pin(tmp.path(), "example.com/first");
    write(tmp.path(), ".logos/rules.toml", "[[layers]]\nname = \"core\"\npaths = [\"src/**\"]\norder = 1\n");
    write(tmp.path(), "src/lib.rs", "mod util;\npub fn run() { util::helper(); }\n");
    write(tmp.path(), "src/util.rs", "pub fn helper() {}\n");
    let engine = Engine::start(tmp.path()).unwrap();
    engine.index();
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/first"));

    // The project's coordinate changes under a running engine (a long-lived
    // server; the next open would migrate the store, S-704); a file is edited
    // and synced. The re-extracted file joins the store's coordinate, never a
    // second one.
    pin(tmp.path(), "example.com/second");
    write(tmp.path(), "src/util.rs", "pub fn helper() {}\npub fn added() {}\n");
    let synced = engine.sync(&[PathBuf::from("src/util.rs")]);
    assert_eq!(synced.files_modified, 1, "the edit re-extracted: {synced:?}");
    let added = symbol_of(&engine, NodeKind::Function, "added");
    assert_eq!(added.coordinate(), Some("example.com/first"));
    // The sync's annotation pass re-materialises the policy nodes: under the
    // store's coordinate too.
    assert_eq!(
        symbol_of(&engine, NodeKind::Layer, "core").as_str(),
        "logos policy example.com/first . layer/core/"
    );
    assert!(nodes(&engine).iter().all(|(_, s)| s.coordinate() == Some("example.com/first")));
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/first"));

    // A full index builds every symbol under the coordinate in force, and the
    // store records it.
    engine.index();
    assert!(nodes(&engine).iter().all(|(_, s)| s.coordinate() == Some("example.com/second")));
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/second"));
    assert_eq!(engine.status().project_id.as_deref(), Some("example.com/second"));
}

#[test]
fn an_index_that_stores_no_file_keeps_the_coordinate_of_the_graph_that_stands() {
    // Every admitted file fails to load, so the previous graph survives
    // untouched: the nodes the index's own passes rebuild (policy nodes here)
    // keep its coordinate, and the store does not record the new one.
    let tmp = TempDir::new().unwrap();
    pin(tmp.path(), "example.com/first");
    write(tmp.path(), ".logos/rules.toml", "[[layers]]\nname = \"core\"\npaths = [\"src/**\"]\norder = 1\n");
    write(tmp.path(), "src/lib.rs", "pub fn run() {}\n");
    let engine = Engine::start(tmp.path()).unwrap();
    engine.index();
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/first"));

    // Under the running engine: a restart would migrate the store to the new
    // coordinate before the index (S-704).
    pin(tmp.path(), "example.com/second");
    fs::write(tmp.path().join("src/lib.rs"), [0xff, 0xfe, 0x00]).unwrap();
    let result = engine.index();
    assert_eq!(result.files_indexed, 0, "no file loaded: {result:?}");
    assert!(
        nodes(&engine).iter().any(|(k, _)| *k == NodeKind::Function),
        "the previous graph stands"
    );
    let off: Vec<_> = nodes(&engine)
        .into_iter()
        .filter(|(_, s)| s.coordinate() != Some("example.com/first"))
        .collect();
    assert!(off.is_empty(), "one coordinate in the store: {off:#?}");
    assert_eq!(
        symbol_of(&engine, NodeKind::Layer, "core").as_str(),
        "logos policy example.com/first . layer/core/"
    );
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/first"));
    assert_eq!(engine.status().project_id.as_deref(), Some("example.com/second"));
}

#[test]
fn a_store_whose_first_write_is_a_sync_carries_the_coordinate() {
    // A commit hook or the watcher can write a store before any index: the
    // symbols it builds carry the coordinate all the same, the store records
    // it, and they are the symbols an index of the same tree builds.
    let synced = TempDir::new().unwrap();
    pin(synced.path(), "example.com/first-contact");
    write(synced.path(), "src/lib.rs", "mod util;\npub fn run() { util::helper(); }\n");
    write(synced.path(), "src/util.rs", "pub fn helper() {}\n");
    let engine = Engine::start(synced.path()).unwrap();
    assert_eq!(recorded(&engine), None, "nothing is recorded before the first write");
    let result = engine.sync(&[PathBuf::from("src/lib.rs"), PathBuf::from("src/util.rs")]);
    assert_eq!(result.files_added, 2, "{result:?}");
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/first-contact"));
    let mut via_sync: Vec<String> = nodes(&engine).into_iter().map(|(_, s)| s.to_string()).collect();
    via_sync.sort();
    assert!(
        via_sync.iter().all(|s| s.starts_with("logos . example.com/first-contact . ")),
        "{via_sync:#?}"
    );

    let indexed = TempDir::new().unwrap();
    pin(indexed.path(), "example.com/first-contact");
    write(indexed.path(), "src/lib.rs", "mod util;\npub fn run() { util::helper(); }\n");
    write(indexed.path(), "src/util.rs", "pub fn helper() {}\n");
    let engine = Engine::start(indexed.path()).unwrap();
    engine.index();
    let mut via_index: Vec<String> = nodes(&engine).into_iter().map(|(_, s)| s.to_string()).collect();
    via_index.sort();
    assert_eq!(via_sync, via_index, "sync-first and index-first build the same symbols");
}

#[test]
fn an_index_that_admits_nothing_records_the_coordinate_for_the_store_it_leaves() {
    // No file of an older graph stands, so the empty store is this index's: it
    // records the coordinate, and the first file a later sync adds carries it.
    let tmp = TempDir::new().unwrap();
    pin(tmp.path(), "example.com/empty-first");
    let engine = Engine::start(tmp.path()).unwrap();
    let result = engine.index();
    assert_eq!(result.files_indexed, 0, "{result:?}");
    assert_eq!(recorded(&engine).as_deref(), Some("example.com/empty-first"));

    write(tmp.path(), "src/lib.rs", "pub fn run() {}\n");
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    assert_eq!(
        symbol_of(&engine, NodeKind::Function, "run").as_str(),
        "logos . example.com/empty-first . src/`lib.rs`/run()."
    );
}

#[test]
fn a_store_that_holds_files_but_records_no_coordinate_syncs_coordinate_less() {
    // A store built before ADR-85 holds files and records no coordinate: a sync
    // keeps it in one form (`.`) rather than adding symbols in a second one,
    // and leaves the record absent for the migration to act on. The open-time
    // migration (S-704) moves such a store, so the state is reached here under
    // a running engine — as it is when that migration could not run.
    let tmp = TempDir::new().unwrap();
    pin(tmp.path(), "example.com/first");
    write(tmp.path(), "src/lib.rs", "pub fn run() {}\n");
    let engine = Engine::start(tmp.path()).unwrap();
    engine.index();
    let conn = rusqlite::Connection::open(tmp.path().join(".logos/logos.db")).unwrap();
    conn.execute("DELETE FROM project_metadata WHERE key = ?1", [SYMBOL_COORDINATE_KEY]).unwrap();
    drop(conn);

    write(tmp.path(), "src/lib.rs", "pub fn run() {}\npub fn added() {}\n");
    engine.sync(&[PathBuf::from("src/lib.rs")]);
    let added = symbol_of(&engine, NodeKind::Function, "added");
    assert!(added.is_coordinate_less(), "a keyless store that holds files renders `.`: {added}");
    assert_eq!(recorded(&engine), None);
}
